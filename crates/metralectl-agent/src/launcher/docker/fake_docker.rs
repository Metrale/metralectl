// SPDX-License-Identifier: MIT OR Apache-2.0

//! A docker daemon in memory, for the idempotency suite.
//!
//! The recording runner answers every call with whatever was scripted, so it
//! cannot say whether a second `docker run` would have collided with the
//! first. This keeps containers as state that outlives any one launcher (an
//! agent restart is a new `DockerLauncher` over the same daemon) and refuses
//! what docker refuses: a second container under a name in use, and a
//! `docker stop` with nothing to stop. Only the five commands the launcher
//! issues are understood; anything else panics, so a new call cannot pass
//! unnoticed.

use metralectl_core::io::ProcessRunner;
use metralectl_core::io::process::Output;
use std::sync::Mutex;
use std::time::Duration;

struct Container {
    id: String,
    name: String,
    recipe_label: Option<String>,
    image: String,
    cmd: Vec<String>,
    env: Vec<String>,
    running: bool,
}

#[derive(Default)]
struct State {
    containers: Vec<Container>,
    runs: usize,
    /// Ids are never reused, as docker's are not.
    minted: usize,
}

pub(super) struct FakeDocker {
    /// The image the fixture recipes run, which is where a `docker run`
    /// argv divides into options and the in-container command.
    image: String,
    /// How long a `docker run` takes before its container exists, standing in
    /// for an image pull — the window two racing launches both fall into.
    run_delay: Duration,
    state: Mutex<State>,
}

fn out(status: i32, stdout: impl Into<String>, stderr: &str) -> Output {
    Output {
        status,
        stdout: stdout.into(),
        stderr: stderr.to_owned(),
    }
}

/// The value after each occurrence of `flag`.
fn values_of(argv: &[String], flag: &str) -> Vec<String> {
    argv.windows(2)
        .filter(|w| w[0] == flag)
        .map(|w| w[1].clone())
        .collect()
}

impl FakeDocker {
    pub(super) fn new(image: &str, run_delay: Duration) -> Self {
        Self {
            image: image.to_owned(),
            run_delay,
            state: Mutex::new(State::default()),
        }
    }

    /// How many `docker run`s were issued — successful or not.
    pub(super) fn runs(&self) -> usize {
        self.state.lock().expect("lock").runs
    }

    /// The names of the containers that are running now.
    pub(super) fn running(&self) -> Vec<String> {
        let state = self.state.lock().expect("lock");
        state
            .containers
            .iter()
            .filter(|c| c.running)
            .map(|c| c.name.clone())
            .collect()
    }

    fn ps(&self) -> Output {
        let state = self.state.lock().expect("lock");
        let rows: String = state
            .containers
            .iter()
            .filter(|c| c.running)
            .map(|c| {
                format!(
                    "{}\t{}\t{}\n",
                    c.id,
                    c.name,
                    c.recipe_label.as_deref().unwrap_or_default()
                )
            })
            .collect();
        out(0, rows, "")
    }

    fn inspect(&self, id: &str) -> Output {
        let state = self.state.lock().expect("lock");
        match state.containers.iter().find(|c| c.id == id) {
            Some(c) => out(
                0,
                serde_json::json!({ "Image": c.image, "Cmd": c.cmd, "Env": c.env }).to_string(),
                "",
            ),
            None => out(1, "", "Error: No such object"),
        }
    }

    fn rm(&self, name: &str) -> Output {
        let mut state = self.state.lock().expect("lock");
        let before = state.containers.len();
        state.containers.retain(|c| c.name != name);
        if state.containers.len() < before {
            out(0, name, "")
        } else {
            out(1, "", "Error: No such container")
        }
    }

    fn stop(&self, ids: &[String]) -> Output {
        if ids.is_empty() {
            return out(1, "", "\"docker stop\" requires at least 1 argument.");
        }
        let mut state = self.state.lock().expect("lock");
        for id in ids {
            match state.containers.iter_mut().find(|c| &c.id == id) {
                Some(c) => c.running = false,
                None => return out(1, "", "Error: No such container"),
            }
        }
        out(0, ids.join("\n"), "")
    }

    fn run_container(&self, argv: &[String]) -> Output {
        self.state.lock().expect("lock").runs += 1;
        // Outside the lock: a second `docker run` races this one exactly as a
        // real pull would let it.
        std::thread::sleep(self.run_delay);
        let name = values_of(argv, "--name").pop().expect("a named container");
        let at = argv
            .iter()
            .position(|a| *a == self.image)
            .expect("the fixture image");
        let recipe_label = values_of(argv, "--label")
            .into_iter()
            .find_map(|l| l.strip_prefix("io.metralectl.recipe=").map(str::to_owned));
        let mut state = self.state.lock().expect("lock");
        if state.containers.iter().any(|c| c.name == name) {
            return out(125, "", "Conflict. The container name is already in use");
        }
        state.minted += 1;
        let id = format!("c{}", state.minted);
        state.containers.push(Container {
            id: id.clone(),
            name,
            recipe_label,
            image: self.image.clone(),
            cmd: argv[at + 1..].to_vec(),
            env: values_of(argv, "-e"),
            running: true,
        });
        out(0, id, "")
    }
}

impl ProcessRunner for FakeDocker {
    fn run(&self, argv: &[String]) -> anyhow::Result<Output> {
        let words: Vec<&str> = argv.iter().map(String::as_str).collect();
        Ok(match words.as_slice() {
            ["docker", "ps", "--format", _] => self.ps(),
            ["docker", "inspect", "--format", _, id] => self.inspect(id),
            ["docker", "rm", "-f", name] => self.rm(name),
            ["docker", "stop", ..] => self.stop(&argv[2..]),
            ["docker", "run", ..] => self.run_container(argv),
            other => panic!("the fake daemon does not know {other:?}"),
        })
    }

    fn run_streaming(&self, argv: &[String]) -> anyhow::Result<i32> {
        panic!("the launcher never streams, got {argv:?}");
    }
}
