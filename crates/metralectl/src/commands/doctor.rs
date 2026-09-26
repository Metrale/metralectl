// SPDX-License-Identifier: MIT OR Apache-2.0

//! `doctor` — check this machine for problems.

use anyhow::Result;
use metralectl_core::io::{ProcessRunner, StdProcessRunner};

use super::doctor_checks::{self, ConfigDirState, Finding};

/// SHA-256 of the lower-cased `owner/repo` of the registry sparkrun redirects
/// recipes to. A digest rather than the name, so this check does not advertise
/// the repository it warns about; `scripts/install.sh` carries the same value.
const REDIRECT_REGISTRY_SHA256: &str =
    "8af2c83739d271e6fb45661fca08da72beaddeabf404da9783dd8c682045adab";

/// Run every check and report.
pub fn run() -> Result<()> {
    let mut problems = 0;

    problems += check_docker();
    // The three that were added after the fact, each one a failure somebody hit
    // during onboarding and diagnosed from an error naming the wrong thing.
    for f in [
        check_config_dir(),
        check_agent(),
        check_peer_channel(),
        check_reachable(),
        check_disk(),
    ] {
        println!("{}", f.line);
        problems += usize::from(f.problem);
    }
    problems += check_redirected_registry();

    println!();
    if problems == 0 {
        println!("no problems found");
        return Ok(());
    }

    // A diagnostic that always exits 0 cannot be gated on. SECURITY.md tells an
    // operator to run this to find a compromised sparkrun install -- a registry
    // redirect that lets someone else's recipes run shell commands on this host
    // -- and a script wrapping that check had no way to see the answer, because
    // the report went to stdout and the status was always success. Reporting a
    // finding is not the same as failing to run, but every tool that gates on
    // this one can only read the status, so `brew doctor`'s convention applies:
    // print the report, then exit non-zero.
    Err(anyhow::anyhow!(
        "{problems} problem(s) found — see the report above"
    ))
}

fn check_docker() -> usize {
    match StdProcessRunner.run(&[
        "docker".into(),
        "version".into(),
        "--format".into(),
        "{{.Server.Version}}".into(),
    ]) {
        Ok(out) if out.success() => {
            println!("docker:   ok (server {})", out.stdout.trim());
            0
        }
        Ok(_) => {
            println!(
                "docker:   PROBLEM — the docker CLI is present but the daemon did not answer.\n\
                 \x20         `metralectl run` needs a working daemon; `recipe list` and\n\
                 \x20         `run --print` work without one."
            );
            1
        }
        Err(_) => {
            println!(
                "docker:   PROBLEM — no docker on PATH. `metralectl run` needs docker and the\n\
                 \x20         NVIDIA container runtime; inspection commands work without them."
            );
            1
        }
    }
}

/// Look for a sparkrun install whose registry has been redirected.
///
/// This is why `doctor` exists. sparkrun 0.3.6 rewrites a recipe registry URL
/// to a repository under an organisation Metrale Corp. does not control, and
/// marks it trusted — which lets recipe-supplied shell commands run on the host. We
/// report it and print the exact removal commands. We never delete a user's
/// files: that behaviour is precisely what makes a tool untrustworthy.
fn check_redirected_registry() -> usize {
    let Ok(home) = std::env::var("HOME") else {
        return 0;
    };
    let config = std::path::Path::new(&home).join(".config/sparkrun/registries.yaml");
    let installed = metralectl_core::platform::which("sparkrun").is_some();
    let redirected = std::fs::read_to_string(&config)
        .map(|s| names_registry(&s, REDIRECT_REGISTRY_SHA256))
        .unwrap_or(false);

    if !installed && !redirected {
        println!("sparkrun: not installed");
        return 0;
    }

    println!("sparkrun: PROBLEM — a sparkrun install was found.");
    if redirected {
        println!(
            "\x20         Its config at {} names a registry known to redirect",
            config.display()
        );
        println!("\x20         recipes to a third-party source Metrale Corp. does not control.");
        println!(
            "\x20         Editing the file is not enough: the redirect is compiled into\n\
             \x20         sparkrun, so it is reapplied the next time the tool runs."
        );
    }
    println!("\x20         A trusted registry's recipes can run shell commands on this host.");
    println!("\x20         To remove it:");
    println!("\x20           pipx uninstall sparkrun     # or: uv tool uninstall sparkrun");
    println!("\x20           rm -rf ~/.config/sparkrun ~/.cache/sparkrun");
    println!(
        "\x20         Review those directories first; metralectl will not delete them for you."
    );
    1
}

/// Whether `config` names a repository whose lower-cased `owner/repo` hashes
/// to `sha256_hex`.
fn names_registry(config: &str, sha256_hex: &str) -> bool {
    use sha2::{Digest, Sha256};
    registry_slugs(config).any(|slug| hex::encode(Sha256::digest(slug.as_bytes())) == sha256_hex)
}

/// The lower-cased `owner/repo` of every URL-shaped word in a config file:
/// its last two path segments, without a trailing `/` or `.git`, reading an
/// scp-style `host:owner/repo` as a path. `registry_slugs` in
/// `scripts/install.sh` extracts the same words.
fn registry_slugs(config: &str) -> impl Iterator<Item = String> + '_ {
    config
        .split(|c: char| !(c.is_ascii_alphanumeric() || "._:/@~+-".contains(c)))
        .filter_map(|word| {
            let word = word.to_ascii_lowercase();
            let word = word.trim_end_matches('/');
            let word = word.strip_suffix(".git").unwrap_or(word).replace(':', "/");
            let mut segments = word.rsplit('/');
            let repo = segments.next()?;
            let owner = segments.next()?;
            (!repo.is_empty() && !owner.is_empty()).then(|| format!("{owner}/{repo}"))
        })
}

/// Free space where docker and the model cache live.
///
/// Measured at the HF cache, not at the current directory. It used to read
/// `Path::new(".")`, which answers about whatever volume the operator happened
/// to `cd` into: on a box whose models live on a separate mount, doctor printed
/// `disk: ok` while `run` immediately warned, and run from a small partition it
/// failed a healthy machine -- which `disk_unknown`'s own doc says must not
/// happen. `run` has always measured `host.hf_cache_dir` (run.rs:96); this now
/// asks the same question of the same volume.
///
/// A cache directory that does not exist yet -- the ordinary state of a machine
/// that has never pulled a model -- is measured at its nearest existing
/// ancestor, so the answer is about the filesystem that WOULD hold it. It is
/// deliberately not measured at `.`: that is a different volume, and failing a
/// healthy machine because the operator `cd`-ed somewhere small is the
/// regression this doc used to record.
///
/// Read with `df -Pk`: POSIX output is one line per filesystem with a fixed
/// column order, which `df` without `-P` does not guarantee — a long device
/// name wraps and the figure moves to the next line.
///
/// A machine we cannot measure is reported as ok rather than as a problem: an
/// unreadable `df` says nothing about whether there is room, and doctor now
/// exits non-zero on a problem, so guessing here would fail a healthy box.
fn check_disk() -> Finding {
    // Walk the cache path up to the nearest EXISTING directory and label THAT.
    //
    // `free_bytes` already walks up (platform.rs:170), so the number was always
    // the right volume; the label was not. An unmounted `/mnt/models` reported
    // ROOT's free space under the cache's name -- a figure from one filesystem
    // wearing another's. Naming the ancestor makes the two agree.
    //
    // This deliberately does NOT fall back to `.`: `disk_space` fails below an
    // absolute floor, so on a box that has never pulled a model -- the ordinary
    // state -- running doctor from a small partition failed a healthy machine.
    // The `(None, Some)` arm below is therefore unreachable for any absolute
    // cache path, and is kept for a relative or empty `HF_HOME`, where the walk
    // terminates immediately and `free_bytes("")` answers `None`.
    // The nearest EXISTING ancestor of the cache, not the cwd.
    //
    // Falling back to `.` was wrong in the direction this file forbids: on a
    // machine that has never pulled a model -- "the ordinary state" -- it
    // measured whatever volume the operator happened to `cd` into, and
    // `disk_space` fails below an absolute floor. Run from a small partition,
    // doctor then FAILS a healthy box, which is exactly the regression the doc
    // above records.
    //
    // Walking up gives the filesystem that WOULD hold the cache, which is the
    // question being asked, and labelling it with the ancestor keeps the number
    // and the path describing the same volume.
    let cache = crate::hostinfo::snapshot()
        .ok()
        .map(|h| h.hf_cache_dir)
        .map(|d| {
            let mut probe = std::path::PathBuf::from(&d);
            while !probe.exists() {
                match probe.parent() {
                    Some(up) => probe = up.to_path_buf(),
                    None => break,
                }
            }
            probe.display().to_string()
        });
    let at_cache = cache
        .as_deref()
        .and_then(|d| metralectl_core::platform::free_bytes(std::path::Path::new(d)));
    let at_cwd = metralectl_core::platform::free_bytes(std::path::Path::new("."));
    disk_finding(cache.as_deref().zip(at_cache), at_cwd)
}

/// Which measurement to report, given what could be read.
///
/// Split from the I/O so the ORDER is testable: preferring the cache is the
/// whole point of the fix, and falling back to `.` rather than to "unknown"
/// is what keeps a machine that has never pulled a model from losing a usable
/// answer. Both are one-line mistakes to make and invisible without a test.
fn disk_finding(at_cache: Option<(&str, u64)>, at_cwd: Option<u64>) -> Finding {
    match (at_cache, at_cwd) {
        (Some((dir, bytes)), _) => doctor_checks::disk_space(bytes, dir),
        (None, Some(bytes)) => doctor_checks::disk_space(bytes, "."),
        (None, None) => doctor_checks::disk_unknown(),
    }
}

/// Where the agent keeps its identity, its pins and its browser token.
fn check_config_dir() -> Finding {
    match crate::hostinfo::usable_config_dir() {
        Ok(dir) => doctor_checks::config_dir(ConfigDirState::Writable(dir.display().to_string())),
        // `usable_config_dir` already distinguishes "cannot create" from "not
        // writable" and says so; doctor reuses that judgement rather than
        // re-deriving it and risking a second, disagreeing opinion.
        Err(e) => doctor_checks::config_dir(ConfigDirState::Unusable(format!("{e:#}"))),
    }
}

/// Whether this machine's own agent is answering.
fn check_agent() -> Finding {
    let port = metralectl_agent::DEFAULT_PORT;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let up =
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok();
    doctor_checks::agent(up, port)
}

/// Whether the peer listener — the one other machines dial — is open.
///
/// Probed the same way as the browser port, and reported separately, because
/// the two are separate listeners: the agent can be perfectly healthy for its
/// own browser while accepting no peers at all.
fn check_peer_channel() -> Finding {
    let port = metralectl_agent::peer::DEFAULT_PEER_PORT;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let up =
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok();
    doctor_checks::peer_channel(up, port)
}

/// Whether another machine could reach this one.
fn check_reachable() -> Finding {
    use metralectl_agent::fabric::FabricProvider as _;
    // Same provider selection as `agent run`, so doctor answers about the
    // interfaces the agent would actually enumerate rather than a second
    // opinion that could disagree with it.
    #[cfg(target_os = "macos")]
    let fabric = metralectl_agent::fabric::macos::MacFabric::new();
    #[cfg(not(target_os = "macos"))]
    let fabric = metralectl_agent::fabric::linux::LinuxFabric::new();

    // NOT `unwrap_or_default()`. An enumeration that failed is not a machine
    // with no addresses, and the advice for the two is unrelated — telling
    // someone to plug in a network cable because `/sys/class/net` could not be
    // listed sends them to fix the wrong thing.
    match fabric.addresses() {
        Ok(list) => {
            let addrs = list
                .into_iter()
                .map(|a| (a.iface, a.addr))
                .collect::<Vec<_>>();
            doctor_checks::reachable(&addrs)
        }
        Err(e) => doctor_checks::unreadable_interfaces(&format!("{e:#}")),
    }
}

#[cfg(test)]
mod disk_tests {
    use super::*;

    const PLENTY: u64 = 500 * 1024 * 1024 * 1024;

    /// The cache volume wins when it can be read, and the line SAYS which
    /// volume it measured — the old code reported `.` and meant it.
    #[test]
    fn the_model_cache_is_preferred_and_named() {
        let f = disk_finding(Some(("/mnt/models", PLENTY)), Some(1));
        assert!(f.line.contains("/mnt/models"), "got: {}", f.line);
        assert!(!f.line.contains(" on ."), "got: {}", f.line);
    }

    /// A machine that has never pulled a model has no cache directory, and
    /// answering "unknown" there would be a regression from a usable answer.
    #[test]
    fn an_unreadable_cache_falls_back_to_the_cwd_not_to_unknown() {
        let f = disk_finding(None, Some(PLENTY));
        assert!(f.line.contains('.'), "got: {}", f.line);
        assert!(!f.problem, "plenty of space is not a fault");
    }

    /// Only when NEITHER can be read is it unknown — reported as ok, because
    /// an unreadable `df` says nothing about whether there is room.
    #[test]
    fn unknown_only_when_nothing_could_be_measured() {
        let f = disk_finding(None, None);
        assert!(!f.problem, "an unmeasurable box must not be failed");
    }
}

#[cfg(test)]
mod redirect_tests {
    use super::*;

    /// SHA-256 of `example-org/example-recipes`, a stand-in for the real
    /// registry, which this public source does not name.
    const STAND_IN: &str = "1ea1ea8ee8ea47212f97c4d36f31d2c489fd67e4e8287f3dfaaab7a7acdaf6d5";

    fn config_with(url: &str) -> String {
        format!(
            "config_version: 1\nregistries:\n- name: official\n  url: https://github.com/other-org/recipe-registry.git\n  trusted: true\n- name: x\n  url: {url}\n  subpath: recipes\n  trusted: true\n"
        )
    }

    /// The spellings git resolves to one repository all match: sparkrun writes
    /// mixed case with `.git`, and a hand-edited file may use scp form, quotes,
    /// a trailing slash or flow style.
    #[test]
    fn the_registry_is_found_in_every_spelling_git_accepts() {
        for url in [
            "https://github.com/Example-Org/example-recipes.git",
            "\"https://github.com/example-org/EXAMPLE-RECIPES/\"",
            "git@github.com:Example-Org/example-recipes.git",
            "ssh://git@github.com/example-org/example-recipes",
        ] {
            assert!(names_registry(&config_with(url), STAND_IN), "{url}");
        }
        let flow = "registries: [{name: x, url: https://github.com/example-org/example-recipes}]";
        assert!(names_registry(flow, STAND_IN));
    }

    /// A different repository is not the registry, however close its name.
    #[test]
    fn a_near_miss_is_not_the_registry() {
        for url in [
            "https://github.com/example-org/example-recipe.git",
            "https://github.com/example-org/example-recipes-fork.git",
            "https://github.com/example-orgs/example-recipes.git",
            "https://github.com/example-org/example-recipes/extra",
            "https://github.com/example-recipes.git",
        ] {
            assert!(!names_registry(&config_with(url), STAND_IN), "{url}");
        }
        assert!(!names_registry("example-org example-recipes", STAND_IN));
    }

    /// `names_registry` compares against `hex::encode`, which is lower case
    /// and 64 characters; a digest in any other form can never match, and
    /// the check would pass every machine in silence.
    #[test]
    fn the_shipped_digest_is_in_the_form_it_is_compared_in() {
        assert_eq!(REDIRECT_REGISTRY_SHA256.len(), 64);
        assert!(
            REDIRECT_REGISTRY_SHA256
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
    }
}
