// SPDX-License-Identifier: MIT OR Apache-2.0

//! Refusal tests for the control-serving rules, over in-memory streams.
//!
//! Everything here runs against the REAL dispatch (`serve_peer_connection`)
//! with a real pin store on disk — only the network and the container
//! runtime are faked (`peer_serve_rig.rs`). The rules that need an actual
//! second and third agent (R4's dial, R5, R6's behavioural half) are proven in
//! `relay_tests.rs`; this file proves every refusal that must happen BEFORE
//! any dial.

use super::peer_serve_rig::*;
use crate::peer::wire::{PeerFrame, read_frame, write_frame};
use metralectl_protocol::fleet::{DisplayName, NodeId};
use metralectl_protocol::msg::{AgentError, ControlRep, ControlReq};

/// T2 — the retroactive-widening regression test: a peer that is PINNED but
/// not granted `controller` sends a terminal `Control` directly and is
/// refused, with the exact grant command in the refusal. This is the test
/// that fails if anyone reintroduces "pinned = authorized".
#[tokio::test]
async fn a_pinned_but_ungranted_sender_is_refused_terminal_control() {
    let r = rig("t2");
    pin_sender(&r.pins, false);
    let (mut c, task) = connected(&r).await;

    let rep = ask(
        &mut c,
        &PeerFrame::Control {
            trace: None,
            req: ControlReq::Status,
        },
    )
    .await;
    match rep {
        ControlRep::Refused {
            by,
            error: AgentError::ControlRefused { reason, .. },
        } => {
            assert_eq!(by, r.local, "the refusal must name who refused");
            assert!(
                reason.contains("peer grant-control"),
                "the fix must be copy-paste: {reason}"
            );
        }
        other => panic!("expected ControlRefused, got {other:?}"),
    }
    assert!(
        r.launcher.calls().is_empty(),
        "an ungranted sender must not reach the launcher: {:?}",
        r.launcher.calls()
    );
    drop(c);
    task.await.expect("serving task ends cleanly");
}

/// T2/T3 — with the grant, the verb executes through the shared core.
#[tokio::test]
async fn a_granted_sender_executes_through_the_local_core() {
    let r = rig("granted");
    pin_sender(&r.pins, true);
    let (mut c, task) = connected(&r).await;

    let rep = ask(
        &mut c,
        &PeerFrame::Control {
            trace: None,
            req: ControlReq::Status,
        },
    )
    .await;
    assert!(matches!(rep, ControlRep::Status { .. }), "got {rep:?}");
    assert!(
        r.launcher
            .calls()
            .iter()
            .any(|c| matches!(c, metralectl_control::launcher::Call::Running)),
        "the status must have come from the launcher"
    );
    drop(c);
    task.await.expect("serving task ends cleanly");
}

/// T2 — revocation is immediate: the pin file is re-read per frame, so a
/// grant withdrawn between two requests refuses the second WITHOUT any
/// restart or reconnect.
#[tokio::test]
async fn revoking_the_grant_takes_effect_on_the_very_next_frame() {
    let r = rig("revoke");
    pin_sender(&r.pins, true);
    let (mut c, task) = connected(&r).await;

    let first = ask(
        &mut c,
        &PeerFrame::Control {
            trace: None,
            req: ControlReq::Status,
        },
    )
    .await;
    assert!(matches!(first, ControlRep::Status { .. }));

    assert!(r.pins.set_controller(sender(), false).expect("revoke"));

    let second = ask(
        &mut c,
        &PeerFrame::Control {
            trace: None,
            req: ControlReq::Status,
        },
    )
    .await;
    assert!(
        matches!(
            second,
            ControlRep::Refused {
                error: AgentError::ControlRefused { .. },
                ..
            }
        ),
        "a revocation that needs a restart is not a revocation: {second:?}"
    );
    drop(c);
    task.await.expect("serving task ends cleanly");
}

/// T3 — a relayed launch runs the same schema validation the browser path
/// runs; garbage settings are refused before the launcher is touched.
#[tokio::test]
async fn a_relayed_launch_is_schema_validated_before_the_launcher() {
    let r = rig("schema");
    pin_sender(&r.pins, true);
    let (mut c, task) = connected(&r).await;

    let rep = ask(
        &mut c,
        &PeerFrame::Control {
            trace: None,
            req: ControlReq::Launch {
                recipe: metralectl_protocol::RecipeId::parse("qwen3.6-27b-fp8").expect("id"),
                settings: [(
                    "no_such_setting".to_owned(),
                    metralectl_protocol::settings::SettingValue::Bool(true),
                )]
                .into_iter()
                .collect(),
            },
        },
    )
    .await;
    assert!(
        matches!(
            rep,
            ControlRep::Refused {
                error: AgentError::BadSettings { .. },
                ..
            }
        ),
        "got {rep:?}"
    );
    assert!(!r.launcher.launched_anything());
    drop(c);
    task.await.expect("serving task ends cleanly");
}

/// R2 — the relay checks the requester's OWN grant before doing anything on
/// its behalf; an ungranted requester cannot spend the relay's authority at
/// a third machine (the confused deputy).
#[tokio::test]
async fn an_ungranted_sender_cannot_ask_for_a_forward() {
    let r = rig("r2");
    pin_sender(&r.pins, false);
    let (mut c, task) = connected(&r).await;

    let target = NodeId::from_bytes([0x7a; 32]);
    let rep = ask(
        &mut c,
        &PeerFrame::ControlTo {
            trace: None,
            node: target,
            req: ControlReq::Status,
        },
    )
    .await;
    match rep {
        ControlRep::Refused {
            by,
            error: AgentError::RelayRefused {
                node, via, detail, ..
            },
        } => {
            assert_eq!(by, r.local);
            assert_eq!(via, Some(r.local), "the relay names itself in the error");
            assert_eq!(node, target, "the refusal names the TARGET");
            assert!(
                detail.contains("peer grant-control"),
                "the fix must be copy-paste: {detail}"
            );
        }
        other => panic!("expected RelayRefused, got {other:?}"),
    }
    drop(c);
    task.await.expect("serving task ends cleanly");
}

/// R3 — a relay forwards only to machines in its OWN pin store: a vouched or
/// invented target is refused, which is the one-hop knowledge rule enforced
/// where it can be enforced (a relay cannot relay a relay).
#[tokio::test]
async fn a_relay_refuses_to_reach_a_node_it_has_not_itself_pinned() {
    let r = rig("r3");
    pin_sender(&r.pins, true);
    let (mut c, task) = connected(&r).await;

    let stranger = NodeId::from_bytes([0x7a; 32]);
    let rep = ask(
        &mut c,
        &PeerFrame::ControlTo {
            trace: None,
            node: stranger,
            req: ControlReq::Status,
        },
    )
    .await;
    match rep {
        ControlRep::Refused {
            by,
            error: AgentError::RelayRefused {
                node, via, detail, ..
            },
        } => {
            assert_eq!(by, r.local);
            assert_eq!(via, Some(r.local), "the relay names itself in the error");
            assert_eq!(node, stranger);
            assert!(detail.contains("not a peer"), "got {detail}");
        }
        other => panic!("expected RelayRefused, got {other:?}"),
    }
    drop(c);
    task.await.expect("serving task ends cleanly");
}

/// R4, the fail-closed half provable without a network: a pinned target with
/// no address in the relay's OWN state is refused — nothing else (a frame, a
/// digest) can supply one, because nothing else is consulted.
#[tokio::test]
async fn a_pinned_target_with_no_known_address_is_refused_not_guessed() {
    let r = rig("r4");
    pin_sender(&r.pins, true);
    let target = NodeId::from_bytes([0x7a; 32]);
    crate::fleet::record_pairing(&r.pins, target, "bb", DisplayName::new("t"), 0, None, false)
        .expect("pin");
    let (mut c, task) = connected(&r).await;

    let rep = ask(
        &mut c,
        &PeerFrame::ControlTo {
            trace: None,
            node: target,
            req: ControlReq::Status,
        },
    )
    .await;
    match rep {
        ControlRep::Refused {
            error: AgentError::RelayRefused { detail, .. },
            ..
        } => assert!(detail.contains("no known address"), "got {detail}"),
        other => panic!("expected RelayRefused, got {other:?}"),
    }
    drop(c);
    task.await.expect("serving task ends cleanly");
}

/// A pairing frame arriving mid-serving is refused by dispatch exactly as it
/// always was: the conversation ends, and no control machinery sees it.
#[tokio::test]
async fn a_pairing_frame_mid_control_serving_ends_the_conversation() {
    let r = rig("pairmid");
    pin_sender(&r.pins, true);
    let (mut c, task) = connected(&r).await;

    write_frame(
        &mut c,
        &PeerFrame::PairStart {
            message: "aa".to_owned(),
        },
    )
    .await
    .expect("send");
    // The server hangs up rather than answering; the read fails.
    assert!(read_frame(&mut c).await.is_err());
    drop(c);
    task.await.expect("serving task ends cleanly");
}
