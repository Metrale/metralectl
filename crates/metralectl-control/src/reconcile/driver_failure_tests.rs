// SPDX-License-Identifier: MIT OR Apache-2.0

//! What the driver answers when a launch is not taken or does not hold: the
//! contract's `refused {cordoned | draining}` and `failed {reason}`, never an
//! error, and always from what the launcher runs afterwards.

use super::*;

fn failed(reason: FailureReason) -> ActionOutcome {
    ActionOutcome::Failed(Failed { reason })
}

fn refused(reason: RefusalReason) -> ActionOutcome {
    ActionOutcome::Refused(Refused { reason })
}

/// A cordoned or draining node refuses the reconciler's launch with the
/// contract's word and touches nothing: a new generation does not tear down
/// the one it would replace. A stop still runs. Sabotage: dropping the
/// admission mapping answers an error (`NotLaunchable`); dropping the check
/// before the replacing stop leaves the deployment serving nothing.
#[tokio::test]
async fn a_cordoned_or_draining_node_refuses_the_launch_and_still_stops() {
    let node = Node::new();
    node.converge(&run_a(1))
        .await
        .expect("runs before the cordon");
    let fresh = Desired {
        deployment: DeploymentName::parse("fresh").expect("label"),
        ..run(B, "Qwen/Qwen3.6-35B-A3B-FP8", 1)
    };
    for (state, word) in [
        (
            Admission::Cordoned {
                reason: "retiring".into(),
            },
            RefusalReason::Cordoned,
        ),
        (
            Admission::Draining {
                reason: "maintenance".into(),
            },
            RefusalReason::Draining,
        ),
    ] {
        node.admission.set(state);
        let replace = node.converge(&run_a(2)).await.expect("answers");
        assert_eq!(replace.outcome, refused(word));
        assert_eq!(replace.running_generation, Some(generation(1)));
        let new = node.converge(&fresh).await.expect("answers");
        assert_eq!(new.outcome, refused(word));
    }
    assert_eq!(node.daemon.runs(), 1, "no launch while closed");
    assert_eq!(node.daemon.running(), [format!("metrale-{A}")]);
    let stop = node.converge(&stopped(3)).await.expect("stops");
    assert_eq!(stop.outcome, ActionOutcome::Applied);
    assert!(node.daemon.running().is_empty());
}

/// A release image that cannot be fetched is `image_unavailable`, and no
/// container is attempted; the retry fetches and launches once. Sabotage:
/// launching without the fetch step lets docker pull implicitly and the
/// answer is `applied` with zero pulls.
#[tokio::test]
async fn an_unavailable_image_fails_before_any_run_and_the_retry_launches_once() {
    let node = Node::new();
    node.daemon.fail_next_pull();
    let answer = node.converge(&run_a(1)).await.expect("answers");
    assert_eq!(answer.outcome, failed(FailureReason::ImageUnavailable));
    assert!(
        answer
            .detail
            .is_some_and(|d| d.contains("manifest unknown"))
    );
    assert_eq!(answer.running_generation, None);
    assert_eq!(node.daemon.runs(), 0);
    let retry = node.converge(&run_a(1)).await.expect("converges");
    assert_eq!(retry.outcome, ActionOutcome::Applied);
    assert_eq!(node.daemon.pulls(), 2);
    assert_eq!(node.daemon.runs(), 1);
    // Present now: another apply after a restart pulls nothing more.
    node.converge(&run_a(1)).await.expect("converged");
    assert_eq!(node.daemon.pulls(), 2);
}

/// `docker run` failing is `launch_failed` and leaves nothing; the retry
/// launches exactly once. Sabotage: recording "launched" before the launch
/// succeeds answers the retry `already_applied` with nothing running.
#[tokio::test]
async fn a_refused_run_is_launch_failed_and_the_retry_launches_once() {
    let node = Node::new();
    node.daemon.fail_next_run();
    let answer = node.converge(&run_a(1)).await.expect("answers");
    assert_eq!(answer.outcome, failed(FailureReason::LaunchFailed));
    assert!(node.daemon.running().is_empty());
    let retry = node.converge(&run_a(1)).await.expect("converges");
    assert_eq!(retry.outcome, ActionOutcome::Applied);
    assert_eq!(node.daemon.running(), [format!("metrale-{A}")]);
}

/// A container that started and is gone at the next observation is
/// `exited`, after one run, not a loop of relaunches. Sabotage: treating the
/// second `Launch` step like the first relaunches until the step budget and
/// answers an error.
#[tokio::test]
async fn a_container_that_dies_at_once_is_exited_after_one_run() {
    let node = Node::new();
    node.daemon.exit_next_run();
    let answer = node.converge(&run_a(1)).await.expect("answers");
    assert_eq!(answer.outcome, failed(FailureReason::Exited));
    assert_eq!(answer.running_generation, None);
    assert_eq!(node.daemon.runs(), 1);
}

/// `drain` is not an action word (PC-U): the binding refuses an action that
/// says it, so nothing reaches the driver.
#[test]
fn a_drain_action_does_not_parse() {
    let mut doc = serde_json::to_value(action(&"66".repeat(32), "apply")).expect("serializes");
    doc["action"] = "drain".into();
    assert!(serde_json::from_value::<DeploymentAction>(doc).is_err());
}
