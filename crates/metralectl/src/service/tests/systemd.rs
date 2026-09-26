// SPDX-License-Identifier: MIT OR Apache-2.0

//! The systemd plan's activate/verify contract, split from [`super`] for size
//! and to sit beside its Windows counterpart in `windows.rs`.

use crate::service::plan::{ServiceKind, plan};
use std::path::Path;

use super::{agent, home};

/// The Linux counterpart of the Windows verify test. `systemctl restart` returns
/// once ExecStart is SPAWNED for a Type=simple unit, and the agent then checks
/// its config dir, loads its token, probes docker and binds its port -- so every
/// failure this check exists to name happens after an instantaneous read. The
/// install said "installed and started" and the operator paired a browser
/// against a crash loop.
#[test]
fn a_systemd_verify_waits_and_then_confirms() {
    let p = plan(ServiceKind::Systemd, &agent(), &home(), 1000);
    let v = p.verify.join(" ");
    assert!(v.contains("is-active"), "{v}");
    assert!(
        v.contains("sleep"),
        "an instantaneous read races ExecStart: {v}"
    );
    // Two reads, not one: a single delayed read still blesses a unit on its way
    // down, which is exactly what a crash loop looks like at any one instant.
    assert_eq!(
        v.matches("is-active").count(),
        2,
        "one read cannot tell 'up' from 'up so far': {v}"
    );
}

/// systemd needs none of this — `enable --now` is idempotent and restarts a
/// live unit with the new binary. Adding a teardown there would stop an agent
/// that did not need stopping.
#[test]
fn systemd_needs_no_pre_step() {
    let p = plan(ServiceKind::Systemd, &agent(), Path::new("/home/x"), 1000);
    assert!(p.pre_activate.is_empty(), "{:?}", p.pre_activate);
}

fn memory_lines(bench_node: bool) -> Vec<String> {
    let a = crate::service::plan::AgentInvocation {
        bench_node,
        ..agent()
    };
    plan(ServiceKind::Systemd, &a, &home(), 1000)
        .unit_body
        .lines()
        .filter(|l| l.starts_with("MemoryMax"))
        .map(str::to_owned)
        .collect()
}

/// An ordinary agent keeps its hard ceiling.
#[test]
fn an_ordinary_agent_is_capped_at_256m() {
    assert_eq!(memory_lines(false), ["MemoryMax=256M"]);
}

/// A bench node's gate builds and runs are the agent's children, in its
/// cgroup: a 256M cap there kills `cargo build` and the engine, not the agent.
#[test]
fn a_bench_node_is_not_memory_capped() {
    assert!(memory_lines(true).is_empty(), "{:?}", memory_lines(true));
}
