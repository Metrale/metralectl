// SPDX-License-Identifier: MIT OR Apache-2.0

//! A repeated install, and the non-interactive configuration file (#47).
//!
//! The installer must be safe to run again: the same unit and binary with a
//! running agent is left alone (no write, no restart, one service), while a
//! new binary, a changed setting, or an agent that is not running is
//! installed again.

use super::super::config::{InstallConfig, Mode};
use super::super::plan::{ServiceKind, plan};
use super::super::*;
use super::{Recorder, agent, home};
use metralectl_core::io::MemFileSystem;
use std::path::PathBuf;

fn this_kind() -> ServiceKind {
    ServiceKind::detect().expect("a supervisor on the test host")
}

/// A machine whose supervisor already holds the unit `invocation` renders.
fn installed_with(invocation: &AgentInvocation) -> MemFileSystem {
    let fs = MemFileSystem::new();
    let p = plan(this_kind(), invocation, &home(), 1000);
    fs.insert(p.unit_path, p.unit_body);
    fs
}

/// Sabotage: dropping the comparison re-runs every activation step (a
/// restart) on a repeat.
#[test]
fn a_repeat_with_the_same_unit_and_a_running_agent_changes_nothing() {
    let fs = installed_with(&agent());
    let runner = Recorder::new();
    let done = install(&fs, &runner, &agent(), &home(), 1000).expect("installs");
    assert!(!done.changed);
    assert!(done.running);
    let verify = plan(this_kind(), &agent(), &home(), 1000).verify.join(" ");
    assert_eq!(runner.calls(), vec![verify], "only the running check ran");
}

/// The binary is part of the unit: replaced at the same path, the agent is
/// restarted onto it. Sabotage: a unit without the binary's digest compares
/// equal after an upgrade and keeps the old build running.
#[test]
fn a_new_binary_at_the_same_path_is_installed_again() {
    let fs = installed_with(&agent());
    let upgraded = AgentInvocation {
        binary_sha256: "cd".repeat(32),
        ..agent()
    };
    let runner = Recorder::new();
    let done = install(&fs, &runner, &upgraded, &home(), 1000).expect("installs");
    assert!(done.changed);
    let p = plan(this_kind(), &upgraded, &home(), 1000);
    let on_disk = fs.get(&p.unit_path).expect("written");
    assert!(on_disk.contains(&"cd".repeat(32)));
    for step in &p.activate {
        assert!(runner.calls().contains(&step.join(" ")), "{step:?} ran");
    }
}

/// The same unit with an agent that is not running is activated again
/// rather than reported healthy.
#[test]
fn the_same_unit_with_a_stopped_agent_is_activated_again() {
    let fs = installed_with(&agent());
    let runner = Recorder::failing("sleep 2");
    let done = install(&fs, &runner, &agent(), &home(), 1000).expect("installs");
    assert!(done.changed);
    assert!(!done.running);
}

/// Every supervisor's unit names the binary it starts by content.
#[test]
fn every_unit_carries_the_binary_digest() {
    for kind in [
        ServiceKind::Systemd,
        ServiceKind::Launchd,
        ServiceKind::ScheduledTask,
    ] {
        let body = plan(kind, &agent(), &home(), 1000).unit_body;
        assert!(
            body.contains(&format!("metralectl binary sha256: {}", "ab".repeat(32))),
            "{kind:?}: {body}"
        );
    }
}

const MANAGED: &str = r#"{"version": 1, "port": 34333, "mode": "node",
    "discovery": false, "browser": false, "config_dir": "/var/lib/metralectl"}"#;

/// A managed node's file installs an agent with no inbound listener.
#[test]
fn a_managed_configuration_installs_an_agent_with_no_inbound_listener() {
    let c = InstallConfig::parse(MANAGED).expect("valid");
    assert_eq!(c.mode, Mode::Node);
    let a = c.invocation(&PathBuf::from("/usr/local/bin/metralectl"), "ef", false);
    let argv = a.argv().join(" ");
    assert_eq!(
        argv,
        "/usr/local/bin/metralectl --config-dir /var/lib/metralectl agent run --port 34333 \
         --no-discovery --no-browser"
    );
    let client = MANAGED.replace("\"node\"", "\"client\"");
    let c = InstallConfig::parse(&client).expect("valid");
    assert!(c.invocation(&PathBuf::from("/m"), "ef", false).client);
}

/// Every key is required and no other is read; the version and an absolute
/// state directory are checked. Sabotage: a `#[serde(default)]` on any field
/// lets a file without it install today's default.
#[test]
fn a_configuration_missing_unknown_or_wrong_is_refused() {
    let full: serde_json::Value = serde_json::from_str(MANAGED).expect("json");
    for key in [
        "version",
        "port",
        "mode",
        "discovery",
        "browser",
        "config_dir",
    ] {
        let mut v = full.clone();
        v.as_object_mut().expect("object").remove(key);
        assert!(
            InstallConfig::parse(&v.to_string()).is_err(),
            "{key} may not be left out"
        );
    }
    let mut extra = full.clone();
    extra["join"] = "code@host".into();
    assert!(InstallConfig::parse(&extra.to_string()).is_err());
    let mut v2 = full.clone();
    v2["version"] = 2.into();
    assert!(InstallConfig::parse(&v2.to_string()).is_err());
    let mut relative = full;
    relative["config_dir"] = "state".into();
    let err = InstallConfig::parse(&relative.to_string()).expect_err("relative");
    assert!(format!("{err:#}").contains("absolute"), "{err:#}");
}
