// SPDX-License-Identifier: MIT OR Apache-2.0

//! The attempt and trace a control frame carries reach the launch it asks for
//! and come back on the answer (#49), over the real dispatch.

use super::peer_serve_rig::*;
use crate::peer::wire::{PeerFrame, read_frame, write_frame};
use metralectl_control::launcher::{Call, LaunchFor};
use metralectl_protocol::TraceId;
use metralectl_protocol::msg::{AttemptId, ControlRep, ControlReq, ControlTrace};

/// Sabotage: executing without the frame's trace launches with none, and the
/// launcher assertion fails; replying without it fails the echo assertion.
#[tokio::test]
async fn a_traced_launch_carries_its_attempt_and_trace_to_the_launcher_and_back() {
    let r = rig("trace");
    pin_sender(&r.pins, true);
    let (mut c, task) = connected(&r).await;
    let trace = ControlTrace {
        attempt: AttemptId::from_bytes([0xa1; 16]).expect("bytes"),
        trace_id: TraceId::from_bytes([0x7c; 16]).expect("non-zero"),
    };
    let launch = PeerFrame::Control {
        req: ControlReq::Launch {
            recipe: metralectl_protocol::RecipeId::parse("qwen3.6-27b-fp8").expect("id"),
            settings: std::collections::BTreeMap::new(),
        },
        trace: Some(trace),
    };
    write_frame(&mut c, &launch).await.expect("send");
    match read_frame(&mut c).await.expect("answered") {
        PeerFrame::ControlReply { rep, trace: echoed } => {
            assert!(matches!(rep, ControlRep::Started { .. }), "{rep:?}");
            assert_eq!(echoed, Some(trace), "the answer names its request");
        }
        other => panic!("expected a control reply, got {other:?}"),
    }
    let launched: Vec<LaunchFor> = r
        .launcher
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Launch(_, _, launch_for) => Some(*launch_for),
            _ => None,
        })
        .collect();
    assert_eq!(launched.len(), 1);
    assert_eq!(launched[0].trace, Some(trace));
    assert_eq!(
        launched[0].owner, None,
        "a control request is no deployment's"
    );
    drop(c);
    task.await.expect("serving task ends cleanly");
}
