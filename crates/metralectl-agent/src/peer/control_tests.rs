// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tests for the control-frame client: the version gate, the reply reader,
//! and the budget ordering the two legs of a forward depend on.

use super::*;
use crate::peer::link::Hello;
use crate::peer::wire::write_frame;
use metralectl_protocol::fleet::NodeId;
use metralectl_protocol::msg::{ControlRep, RunningLaunch};
use std::net::SocketAddr;
use std::time::Duration;

fn hello(version_max: Option<u32>) -> Hello {
    Hello {
        name: "old-box".to_owned(),
        can_launch: true,
        accelerator: String::new(),
        os: "Linux".to_owned(),
        addresses: Vec::new(),
        version_max,
        vouched: None,
    }
}

fn addr() -> SocketAddr {
    "10.0.0.1:34334".parse().expect("addr")
}

/// O5 at the connection: a hello without `version_max` is a v1 build, and a
/// v2 frame written at it is dropped by its decoder — indistinguishable from
/// the network eating the request. Refused locally, naming the version.
#[test]
fn a_v1_hello_refuses_control_by_version_before_any_frame_is_written() {
    let peer = NodeId::from_bytes([9; 32]);
    for h in [hello(None), hello(Some(1))] {
        let err = ensure_control_capable(&h, peer).expect_err("must refuse");
        let msg = format!("{err:#}");
        assert!(msg.contains("peer protocol 1"), "names the version: {msg}");
        assert!(msg.contains("old-box"), "names the machine: {msg}");
    }
    assert!(ensure_control_capable(&hello(Some(2)), peer).is_ok());
}

/// The strict budget ordering, asserted where the constants are defined: the
/// origin must always outwait the relay's whole leg (its dial plus its answer
/// budget), or the two legs deadlock waiting each other out.
#[test]
fn the_origin_budget_strictly_contains_the_relay_leg() {
    assert!(
        ORIGIN_ANSWER_BUDGET > RELAY_ANSWER_BUDGET.saturating_add(crate::peer::link::DIAL_TIMEOUT),
        "the origin gave up before the relay could even time out"
    );
}

#[tokio::test]
async fn the_reply_reader_skips_vitals_but_not_forever() {
    let (mut ours, mut theirs) = tokio::io::duplex(1 << 16);
    let sender = tokio::spawn(async move {
        for _ in 0..2 {
            write_frame(
                &mut theirs,
                &PeerFrame::Vitals {
                    vitals: Box::new(metralectl_protocol::fleet::NodeVitals::default()),
                },
            )
            .await
            .expect("vitals");
        }
        write_frame(
            &mut theirs,
            &PeerFrame::ControlReply {
                trace: None,
                rep: ControlRep::Status {
                    running: vec![RunningLaunch {
                        container: "c".into(),
                        recipe: None,
                        status: "Up".into(),
                    }],
                },
            },
        )
        .await
        .expect("reply");
        theirs
    });

    let rep = control_reply(&mut ours, addr(), Duration::from_secs(2), None)
        .await
        .expect("answered past the vitals");
    assert!(matches!(rep, ControlRep::Status { .. }));
    drop(sender.await.expect("sender"));
}

#[tokio::test]
async fn a_non_reply_frame_is_a_protocol_error_not_a_timeout() {
    let (mut ours, mut theirs) = tokio::io::duplex(1 << 16);
    write_frame(
        &mut theirs,
        &PeerFrame::RankStopped {
            container: "c".into(),
        },
    )
    .await
    .expect("frame");

    let err = control_reply(&mut ours, addr(), Duration::from_secs(2), None)
        .await
        .expect_err("a confused peer must be reported, not waited out");
    assert!(
        format!("{err:#}").contains("expected a control reply"),
        "got {err:#}"
    );
}

#[tokio::test]
async fn a_silent_peer_is_reported_within_the_budget() {
    let (mut ours, _held_open) = tokio::io::duplex(1 << 16);
    let budget = Duration::from_millis(100);
    let start = std::time::Instant::now();
    let err = control_reply(&mut ours, addr(), budget, None)
        .await
        .expect_err("must time out");
    assert!(start.elapsed() < Duration::from_secs(2), "bounded wait");
    assert!(format!("{err:#}").contains("did not answer"), "got {err:#}");
}

fn trace(attempt: u8, trace: u8) -> ControlTrace {
    ControlTrace {
        attempt: AttemptId::from_bytes([attempt; 16]).expect("bytes"),
        trace_id: TraceId::from_bytes([trace; 16]).expect("non-zero"),
    }
}

async fn reply_with(echo: Option<ControlTrace>, sent: ControlTrace) -> Result<ControlRep> {
    let (mut ours, mut theirs) = tokio::io::duplex(1 << 16);
    let frame = PeerFrame::ControlReply {
        rep: ControlRep::Status {
            running: Vec::new(),
        },
        trace: echo,
    };
    write_frame(&mut theirs, &frame).await.expect("reply");
    control_reply(&mut ours, addr(), Duration::from_secs(2), Some(sent)).await
}

/// The echo ties an answer to its request. Sabotage: ignoring the echo
/// accepts another attempt's answer as this one's.
#[tokio::test]
async fn an_answer_echoing_another_attempt_is_not_this_requests_answer() {
    let sent = trace(1, 1);
    assert!(reply_with(Some(sent), sent).await.is_ok());
    // A build without the field echoes nothing, and is answered as before.
    assert!(reply_with(None, sent).await.is_ok());
    let err = reply_with(Some(trace(2, 1)), sent)
        .await
        .expect_err("another attempt");
    assert!(format!("{err:#}").contains("not this request's"), "{err:#}");
}

#[test]
fn every_request_gets_a_fresh_attempt_and_trace() {
    let a = mint_trace();
    let b = mint_trace();
    assert_ne!(a.attempt, b.attempt);
    assert_ne!(a.trace_id, b.trace_id);
}
