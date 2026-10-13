// SPDX-License-Identifier: MIT OR Apache-2.0

//! A docker daemon in memory, for the idempotency and convergence suites.
//!
//! The recording runner answers every call with whatever was scripted, so it
//! cannot say whether a second `docker run` would have collided with the
//! first. This keeps containers as state that outlives any one launcher (an
//! agent restart is a new `DockerLauncher` over the same daemon) and refuses
//! what docker refuses: a second container under a name in use, and a
//! `docker stop` with nothing to stop. Only the seven commands the launcher
//! issues are understood; anything else panics, so a new call cannot pass
//! unnoticed. `docker ps --format` renders the launcher's own template, so a
//! column the template asks for is answered from the container's labels
//! rather than from a layout this file assumes.

use metralectl_core::io::ProcessRunner;
use metralectl_core::io::process::Output;
use std::sync::Mutex;
use std::time::Duration;

struct Container {
    /// 64 hex digits, as `docker ps --no-trunc` prints it.
    id: String,
    name: String,
    labels: Vec<(String, String)>,
    image: String,
    cmd: Vec<String>,
    env: Vec<String>,
    running: bool,
}

impl Container {
    fn label(&self, key: &str) -> &str {
        self.labels
            .iter()
            .find(|(k, _)| k == key)
            .map_or("", |(_, v)| v.as_str())
    }

    /// `template` with this container's values, as docker renders it: an
    /// absent label renders empty.
    fn render(&self, template: &str) -> String {
        let mut out = template
            .replace("{{.ID}}", &self.id)
            .replace("{{.Names}}", &self.name);
        const OPEN: &str = "{{.Label \"";
        while let Some(start) = out.find(OPEN) {
            let key_at = start + OPEN.len();
            let key_len = out[key_at..].find('"').expect("a closing quote");
            let key = out[key_at..key_at + key_len].to_owned();
            let end = key_at + key_len + "\"}}".len();
            out.replace_range(start..end, self.label(&key));
        }
        out
    }
}

#[derive(Default)]
struct State {
    containers: Vec<Container>,
    runs: usize,
    /// Ids are never reused, as docker's are not.
    minted: usize,
    /// The next `docker run` fails before creating anything.
    fail_next_run: bool,
    /// Images present on this machine, by reference.
    images: Vec<String>,
    /// How many `docker pull`s were issued.
    pulls: usize,
    /// The next `docker pull` fails and fetches nothing.
    fail_next_pull: bool,
    /// The next container `docker run` creates exits at once.
    exit_next_run: bool,
}

pub(crate) struct FakeDocker {
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
    pub(crate) fn new(run_delay: Duration) -> Self {
        Self {
            run_delay,
            state: Mutex::new(State::default()),
        }
    }

    /// How many `docker run`s were issued — successful or not.
    pub(crate) fn runs(&self) -> usize {
        self.state.lock().expect("lock").runs
    }

    /// Make the next `docker run` fail the way a failed pull does: nothing is
    /// created.
    pub(crate) fn fail_next_run(&self) {
        self.state.lock().expect("lock").fail_next_run = true;
    }

    /// Make the next container exit as soon as it starts, the way an engine
    /// that dies at startup does: `docker run -d` succeeds, nothing runs.
    pub(crate) fn exit_next_run(&self) {
        self.state.lock().expect("lock").exit_next_run = true;
    }

    /// Make the next `docker pull` fail the way an unreachable registry or an
    /// unknown digest does: nothing is fetched.
    pub(crate) fn fail_next_pull(&self) {
        self.state.lock().expect("lock").fail_next_pull = true;
    }

    /// How many `docker pull`s were issued — successful or not.
    pub(crate) fn pulls(&self) -> usize {
        self.state.lock().expect("lock").pulls
    }

    fn image_inspect(&self, reference: &str) -> Output {
        let state = self.state.lock().expect("lock");
        if state.images.iter().any(|i| i == reference) {
            out(0, "sha256:present\n", "")
        } else {
            out(1, "", "Error: No such image")
        }
    }

    fn pull(&self, reference: &str) -> Output {
        let mut state = self.state.lock().expect("lock");
        state.pulls += 1;
        if std::mem::take(&mut state.fail_next_pull) {
            return out(1, "", "Error response from daemon: manifest unknown");
        }
        state.images.push(reference.to_owned());
        out(0, "Status: Downloaded newer image", "")
    }

    /// A running container's labels.
    pub(crate) fn labels_of(&self, name: &str) -> Vec<(String, String)> {
        let state = self.state.lock().expect("lock");
        state
            .containers
            .iter()
            .find(|c| c.running && c.name == name)
            .map(|c| c.labels.clone())
            .unwrap_or_default()
    }

    /// The image a running container was started from.
    pub(crate) fn image_of(&self, name: &str) -> Option<String> {
        let state = self.state.lock().expect("lock");
        state
            .containers
            .iter()
            .find(|c| c.running && c.name == name)
            .map(|c| c.image.clone())
    }

    /// The names of the containers that are running now.
    pub(crate) fn running(&self) -> Vec<String> {
        let state = self.state.lock().expect("lock");
        state
            .containers
            .iter()
            .filter(|c| c.running)
            .map(|c| c.name.clone())
            .collect()
    }

    fn ps(&self, template: &str) -> Output {
        let state = self.state.lock().expect("lock");
        let rows: String = state
            .containers
            .iter()
            .filter(|c| c.running)
            .map(|c| format!("{}\n", c.render(template)))
            .collect();
        out(0, rows, "")
    }

    fn inspect(&self, id: &str) -> Output {
        let state = self.state.lock().expect("lock");
        match state.containers.iter().find(|c| c.id == id) {
            Some(c) => {
                let labels: serde_json::Map<String, serde_json::Value> = c
                    .labels
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect();
                let config = serde_json::json!({
                    "Image": c.image, "Cmd": c.cmd, "Env": c.env, "Labels": labels,
                });
                out(0, config.to_string(), "")
            }
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
        {
            let mut state = self.state.lock().expect("lock");
            state.runs += 1;
            if std::mem::take(&mut state.fail_next_run) {
                return out(125, "", "Error response from daemon: pull access denied");
            }
        }
        // Outside the lock: a second `docker run` races this one exactly as a
        // real pull would let it.
        std::thread::sleep(self.run_delay);
        let name = values_of(argv, "--name").pop().expect("a named container");
        // The image is the argument before the in-container command, which
        // every launch this launcher renders starts with `met serve`.
        let at = argv
            .windows(2)
            .position(|w| w[0] == "met" && w[1] == "serve")
            .expect("a `met serve` command")
            - 1;
        let labels = values_of(argv, "--label")
            .into_iter()
            .map(|l| {
                let (k, v) = l.split_once('=').expect("a key=value label");
                (k.to_owned(), v.to_owned())
            })
            .collect();
        let mut state = self.state.lock().expect("lock");
        if state.containers.iter().any(|c| c.name == name) {
            return out(125, "", "Conflict. The container name is already in use");
        }
        state.minted += 1;
        let id = format!("{:064x}", state.minted);
        let running = !std::mem::take(&mut state.exit_next_run);
        state.containers.push(Container {
            id: id.clone(),
            name,
            labels,
            image: argv[at].clone(),
            cmd: argv[at + 1..].to_vec(),
            env: values_of(argv, "-e"),
            running,
        });
        out(0, id, "")
    }
}

impl ProcessRunner for FakeDocker {
    fn run(&self, argv: &[String]) -> anyhow::Result<Output> {
        let words: Vec<&str> = argv.iter().map(String::as_str).collect();
        Ok(match words.as_slice() {
            ["docker", "ps", "--no-trunc", "--format", template] => self.ps(template),
            ["docker", "inspect", "--format", _, id] => self.inspect(id),
            ["docker", "rm", "-f", name] => self.rm(name),
            ["docker", "image", "inspect", "--format", _, reference] => {
                self.image_inspect(reference)
            }
            ["docker", "pull", reference] => self.pull(reference),
            ["docker", "stop", ..] => self.stop(&argv[2..]),
            ["docker", "run", ..] => self.run_container(argv),
            other => panic!("the fake daemon does not know {other:?}"),
        })
    }

    fn run_streaming(&self, argv: &[String]) -> anyhow::Result<i32> {
        panic!("the launcher never streams, got {argv:?}");
    }
}
