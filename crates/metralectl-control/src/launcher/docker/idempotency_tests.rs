// SPDX-License-Identifier: MIT OR Apache-2.0

//! Repeated launch, repeated stop, a restart and a race, against a daemon
//! that keeps state (`fake_docker.rs`) — so "one container" is counted, not
//! assumed from a scripted reply.

use super::*;
use crate::launcher::docker::fake_docker::FakeDocker;
use std::time::Duration;

fn daemon(run_delay: Duration) -> Arc<FakeDocker> {
    Arc::new(FakeDocker::new(run_delay))
}

/// A launcher over `daemon` — a fresh one per call is an agent restart.
fn on(daemon: &Arc<FakeDocker>) -> DockerLauncher {
    DockerLauncher::new(
        Arc::clone(daemon) as Arc<dyn ProcessRunner>,
        host(),
        &ROOTLESS_V1,
        Box::new(NvidiaDevices),
    )
}

/// A repeated launch returns the first launch and starts nothing. Sabotage:
/// skipping the running-container check runs `rm -f` + `run` again and the
/// run count is 2.
#[test]
fn launching_a_running_recipe_again_returns_that_launch() {
    let d = daemon(Duration::ZERO);
    let l = on(&d);
    let first = l
        .launch(&recipe(), &BTreeMap::new(), &UNOWNED)
        .expect("launches");
    let again = l
        .launch(&recipe(), &BTreeMap::new(), &UNOWNED)
        .expect("returns it");
    assert_eq!(again, first, "the same container and endpoint");
    assert_eq!(d.runs(), 1, "one docker run, not two");
    assert_eq!(d.running(), ["metrale-r"]);
}

/// The launcher holds no memory of what it started: after a restart, docker
/// is asked, and the running launch is found. Sabotage: deciding from an
/// in-memory record (or not at all) starts a second container here.
#[test]
fn a_restarted_agent_finds_the_launch_instead_of_starting_another() {
    let d = daemon(Duration::ZERO);
    let first = on(&d)
        .launch(&recipe(), &BTreeMap::new(), &UNOWNED)
        .expect("launches");
    let after_restart = on(&d);
    assert_eq!(
        after_restart
            .launch(&recipe(), &BTreeMap::new(), &UNOWNED)
            .expect("returns it"),
        first
    );
    assert_eq!(d.runs(), 1);
    after_restart
        .stop("r")
        .expect("stops what the old agent started");
    assert!(d.running().is_empty());
}

/// A launch with other settings is not this launch: refused, and the running
/// one is left alone. Sabotage: adopting on the name alone returns the 8888
/// launch as if 9100 had been applied.
#[test]
fn a_different_launch_of_a_running_recipe_is_refused_not_replaced() {
    let d = daemon(Duration::ZERO);
    let l = on(&d);
    l.launch(&recipe(), &BTreeMap::new(), &UNOWNED)
        .expect("launches");
    let other = BTreeMap::from([("port".to_string(), ScalarValue::Int(9100))]);
    let err = l
        .launch(&recipe(), &other, &UNOWNED)
        .expect_err("must refuse");
    assert!(matches!(err, AgentError::AlreadyRunning { .. }), "{err:?}");
    assert_eq!(d.runs(), 1, "nothing was replaced");
    assert_eq!(d.running(), ["metrale-r"]);
}

/// A stop of nothing running succeeds, and so does a second stop. Sabotage:
/// issuing `docker stop` with no ids fails the way docker does.
#[test]
fn stopping_nothing_and_stopping_twice_both_succeed() {
    let d = daemon(Duration::ZERO);
    let l = on(&d);
    l.stop("r").expect("nothing running is already stopped");
    l.launch(&recipe(), &BTreeMap::new(), &UNOWNED)
        .expect("launches");
    l.stop("r").expect("stops");
    l.stop("r").expect("a second stop is done already");
    assert!(d.running().is_empty());
}

/// Idempotent is not sticky: after a stop, a launch starts again.
#[test]
fn a_launch_after_a_stop_starts_again() {
    let d = daemon(Duration::ZERO);
    let l = on(&d);
    l.launch(&recipe(), &BTreeMap::new(), &UNOWNED)
        .expect("launches");
    l.stop("r").expect("stops");
    l.launch(&recipe(), &BTreeMap::new(), &UNOWNED)
        .expect("launches again");
    assert_eq!(d.runs(), 2);
    assert_eq!(d.running(), ["metrale-r"]);
}

/// Two launches at once — a browser click and a relayed retry — start one
/// container. Sabotage: without the per-recipe gate both read "not running"
/// during the first pull, both run, and docker refuses the second name.
#[test]
fn two_racing_launches_start_one_container() {
    let d = daemon(Duration::from_millis(300));
    let l = on(&d);
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| l.launch(&recipe(), &BTreeMap::new(), &UNOWNED));
        let b = s.spawn(|| l.launch(&recipe(), &BTreeMap::new(), &UNOWNED));
        (a.join().expect("thread"), b.join().expect("thread"))
    });
    assert_eq!(a.expect("first"), b.expect("second"));
    assert_eq!(d.runs(), 1);
}
