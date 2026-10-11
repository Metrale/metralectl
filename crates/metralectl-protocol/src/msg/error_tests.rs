// SPDX-License-Identifier: MIT OR Apache-2.0

//! Wire compatibility of the refusal fields added after the first release.

use super::*;

fn node(b: u8) -> crate::fleet::NodeId {
    crate::fleet::NodeId::from_bytes([b; 32])
}

/// A relay built before `transient` existed sends no such field. It must
/// still decode, and decode as a DECISION: reading an old relay's refusal as
/// a lost leg would retry what it decided, while the reverse only costs one
/// retry. Sabotage: dropping `#[serde(default)]` makes the old frame fail to
/// decode at all.
#[test]
fn a_relay_refusal_without_transient_decodes_as_a_decision() {
    let old = format!(
        r#"{{"code":"relay_refused","node":"{}","via":"{}","detail":"no grant"}}"#,
        node(1),
        node(2)
    );
    match serde_json::from_str::<AgentError>(&old).expect("an old relay's frame decodes") {
        AgentError::RelayRefused { transient, .. } => assert!(!transient),
        other => panic!("expected RelayRefused, got {other:?}"),
    }
}

/// A lost leg survives the wire as a lost leg.
#[test]
fn a_lost_relay_leg_round_trips() {
    let lost = AgentError::RelayRefused {
        node: node(1),
        via: Some(node(2)),
        detail: "did not answer within 60s".to_owned(),
        transient: true,
    };
    let s = serde_json::to_string(&lost).expect("encodes");
    assert!(s.contains(r#""transient":true"#), "{s}");
    assert_eq!(
        serde_json::from_str::<AgentError>(&s).expect("decodes"),
        lost
    );
}
