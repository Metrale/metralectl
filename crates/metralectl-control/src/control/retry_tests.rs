// SPDX-License-Identifier: MIT OR Apache-2.0

//! The retry machine, driven by scripted answers: no sockets, no fleet, and
//! nothing faked but the answers themselves.

use super::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

fn target() -> NodeId {
    NodeId::from_bytes([7; 32])
}

fn relay() -> NodeId {
    NodeId::from_bytes([3; 32])
}

fn recipe() -> RecipeId {
    RecipeId::parse("qwen3.6-27b-fp8").expect("valid id")
}

fn launch() -> ControlReq {
    ControlReq::Launch {
        recipe: recipe(),
        settings: BTreeMap::new(),
    }
}

fn budget(attempts: u32) -> RetryBudget {
    RetryBudget {
        attempts: NonZeroU32::new(attempts).expect("non-zero"),
        pause: Duration::from_millis(250),
    }
}

fn lost() -> Attempt {
    Attempt::Lost {
        error: AgentError::NotRoutable {
            node: target(),
            reason: "could not reach it directly: connection refused".to_owned(),
        },
    }
}

fn relay_said(transient: bool) -> Attempt {
    Attempt::Answered {
        rep: ControlRep::Refused {
            by: relay(),
            error: AgentError::RelayRefused {
                node: target(),
                via: Some(relay()),
                detail: "did not answer the control request".to_owned(),
                transient,
            },
        },
        via: Some(relay()),
    }
}

fn status(running: &[&str]) -> Attempt {
    Attempt::Answered {
        rep: ControlRep::Status {
            running: running
                .iter()
                .map(|r| RunningLaunch {
                    container: format!("metrale-{r}"),
                    recipe: RecipeId::parse(r).ok(),
                    status: "Up 2 minutes".to_owned(),
                })
                .collect(),
        },
        via: None,
    }
}

fn started() -> Attempt {
    Attempt::Answered {
        rep: ControlRep::Started {
            recipe: recipe(),
            container: "metrale-qwen3.6-27b-fp8".to_owned(),
            endpoint: Some("http://localhost:8888/v1".to_owned()),
        },
        via: None,
    }
}

/// Run `req` against scripted answers; returns every request sent, the
/// pauses before them, and the outcome. Panics if the run asks for more
/// sends than the script holds — an unbounded run fails loudly here.
fn run(
    req: ControlReq,
    b: RetryBudget,
    script: Vec<Attempt>,
) -> (Vec<ControlReq>, Vec<Duration>, Outcome) {
    let mut answers = script.into_iter();
    let (mut machine, mut step) = ControlRun::start(target(), req, b);
    let (mut sent, mut pauses) = (Vec::new(), Vec::new());
    loop {
        match step {
            Step::Done(outcome) => return (sent, pauses, outcome),
            Step::Send { req, pause } => {
                sent.push(req);
                pauses.push(pause);
                let answer = answers
                    .next()
                    .unwrap_or_else(|| panic!("asked for send {} beyond the script", sent.len()));
                step = machine.record(answer);
            }
        }
    }
}

/// A refusal is an answer: one send, whoever refused, and the refusal comes
/// back naming them. Sabotage: treating every `Refused` as lost turns this
/// into a retry and the send count fails.
#[test]
fn a_refusal_ends_the_run_on_the_send_that_drew_it() {
    let target_said_no = Attempt::Answered {
        rep: ControlRep::Refused {
            by: target(),
            error: AgentError::ControlRefused {
                node: target(),
                reason: "not granted".to_owned(),
            },
        },
        via: None,
    };
    for (answer, who) in [(target_said_no, target()), (relay_said(false), relay())] {
        let (sent, _, outcome) = run(launch(), budget(5), vec![answer]);
        assert_eq!(sent, vec![launch()], "a refusal must not be retried");
        match outcome {
            Outcome::Refused { by, attempts, .. } => {
                assert_eq!(by, who, "the refusal must name who refused");
                assert_eq!(attempts, 1);
            }
            other => panic!("expected the refusal back, got {other:?}"),
        }
    }
}

/// The lost-reply case: the launch may have happened, so the next send is a
/// `Status` read, and a recipe found running ends the run with no second
/// `Launch`. Sabotage: resending the `Launch` straight after the loss sends
/// a second one and the sequence assertion fails.
#[test]
fn a_lost_launch_reads_status_and_stops_when_the_recipe_is_running() {
    let (sent, _, outcome) = run(
        launch(),
        budget(5),
        vec![relay_said(true), status(&["qwen3.6-27b-fp8"])],
    );
    assert_eq!(sent, vec![launch(), ControlReq::Status], "no second launch");
    match outcome {
        Outcome::AlreadyRunning {
            launch, attempts, ..
        } => {
            assert_eq!(launch.container, "metrale-qwen3.6-27b-fp8");
            assert_eq!(attempts, 2);
        }
        other => panic!("expected AlreadyRunning, got {other:?}"),
    }
}

/// Only the asked-for recipe counts as "already running": another recipe in
/// the status is not this launch. Sabotage: matching any running launch
/// stops here with the wrong one.
#[test]
fn a_lost_launch_whose_recipe_is_not_running_is_launched_again() {
    let (sent, pauses, outcome) = run(
        launch(),
        budget(5),
        vec![lost(), status(&["gemma-4-31b-nvfp4"]), started()],
    );
    assert_eq!(sent, vec![launch(), ControlReq::Status, launch()]);
    assert_eq!(
        pauses,
        vec![Duration::ZERO, budget(5).pause, budget(5).pause],
        "only the first send goes without waiting"
    );
    assert!(
        matches!(
            outcome,
            Outcome::Answered {
                rep: ControlRep::Started { .. },
                attempts: 3,
                ..
            }
        ),
        "{outcome:?}"
    );
}

/// The budget counts every send and the run reports how many it made and
/// who answered last. Sabotage: letting the count reach `attempts + 1`
/// (`>` for `>=`) asks the script for a fourth send and panics.
#[test]
fn a_spent_budget_reports_the_attempts_and_the_last_responder() {
    let (sent, _, outcome) = run(
        ControlReq::Status,
        budget(3),
        vec![relay_said(true), lost(), relay_said(true)],
    );
    assert_eq!(sent.len(), 3, "bounded: exactly the budget");
    match outcome {
        Outcome::Exhausted {
            attempts,
            last_responder,
            lost,
        } => {
            assert_eq!(attempts, 3);
            assert_eq!(last_responder, Some(relay()));
            assert_eq!(lost.by, Some(relay()), "the last send was the relay's");
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

/// A launch that keeps being lost never launches twice in a row: every
/// retry is preceded by a `Status` read, and the budget ends it. Sabotage:
/// skipping the probe makes the sequence `Launch, Launch, Launch`.
#[test]
fn every_repeated_launch_is_preceded_by_a_status_read() {
    let (sent, _, outcome) = run(
        launch(),
        budget(4),
        vec![lost(), status(&[]), lost(), status(&[])],
    );
    assert_eq!(
        sent,
        vec![launch(), ControlReq::Status, launch(), ControlReq::Status]
    );
    match outcome {
        Outcome::Exhausted {
            attempts,
            last_responder,
            lost,
        } => {
            assert_eq!(attempts, 4);
            assert_eq!(last_responder, Some(target()), "the status read answered");
            assert_eq!(lost.by, None, "the launch itself was never answered");
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

/// `Stop` and the read-only verbs are resent unchanged — the target makes
/// them safe to repeat — and never trigger a status probe. Sabotage:
/// probing for every verb puts a `Status` in this sequence.
#[test]
fn stop_and_reads_are_resent_as_they_are() {
    let stop = ControlReq::Stop { recipe: recipe() };
    let stopped = Attempt::Answered {
        rep: ControlRep::Stopped { recipe: recipe() },
        via: None,
    };
    let (sent, _, outcome) = run(stop.clone(), budget(3), vec![lost(), stopped]);
    assert_eq!(sent, vec![stop.clone(), stop]);
    assert!(matches!(outcome, Outcome::Answered { attempts: 2, .. }));
}

/// A budget of one is no retry at all. Sabotage: probing before checking
/// the budget sends a second request.
#[test]
fn a_budget_of_one_never_resends() {
    let (sent, _, outcome) = run(launch(), budget(1), vec![lost()]);
    assert_eq!(sent, vec![launch()]);
    assert!(matches!(outcome, Outcome::Exhausted { attempts: 1, .. }));
}

/// What the browser is told when the budget runs out with nobody answering:
/// this agent's own error, saying how hard it tried. Sabotage: dropping the
/// note leaves an error indistinguishable from a single failed dial.
#[test]
fn an_unanswered_run_is_this_agents_error_and_names_the_attempts() {
    let (_, _, outcome) = run(ControlReq::Status, budget(3), vec![lost(), lost(), lost()]);
    match outcome.into_relay() {
        Err(AgentError::NotRoutable { reason, .. }) => {
            assert!(reason.contains("3 attempts"), "{reason}");
            assert!(reason.contains("no machine answered"), "{reason}");
        }
        other => panic!("expected this agent's NotRoutable, got {other:?}"),
    }
}

/// When a relay answered last, the spent budget stays the relay's refusal,
/// so the browser still sees whose leg failed — and the count.
#[test]
fn a_run_a_relay_answered_last_is_the_relays_refusal_with_the_count() {
    let (_, _, outcome) = run(
        ControlReq::Status,
        budget(2),
        vec![lost(), relay_said(true)],
    );
    match outcome.into_relay() {
        Ok((
            ControlRep::Refused {
                by,
                error: AgentError::RelayRefused { detail, .. },
            },
            via,
        )) => {
            assert_eq!(by, relay());
            assert_eq!(via, Some(relay()));
            assert!(detail.contains("2 attempts"), "{detail}");
            assert!(detail.contains(&relay().short()), "{detail}");
        }
        other => panic!("expected the relay's refusal, got {other:?}"),
    }
}

/// A launch found running is reported as such, not as a fresh start.
#[test]
fn a_launch_found_running_is_reported_as_already_running() {
    let (_, _, outcome) = run(
        launch(),
        budget(3),
        vec![lost(), status(&["qwen3.6-27b-fp8"])],
    );
    assert_eq!(
        outcome.into_relay(),
        Err(AgentError::AlreadyRunning { recipe: recipe() })
    );
}

/// The driver sends through its port and stops on a local refusal at once.
/// Sabotage: retrying the port's `Err` sends a second request.
#[tokio::test]
async fn the_driver_ends_on_a_local_refusal_without_another_send() {
    let sends = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&sends);
    let b = RetryBudget {
        attempts: NonZeroU32::new(3).expect("non-zero"),
        pause: Duration::ZERO,
    };
    let result = drive(target(), launch(), b, move |req| {
        log.lock().expect("lock").push(req);
        Box::pin(async {
            Err(AgentError::NotRoutable {
                node: target(),
                reason: "not paired".to_owned(),
            })
        })
    })
    .await;
    assert!(matches!(result, Err(AgentError::NotRoutable { .. })));
    assert_eq!(sends.lock().expect("lock").len(), 1);
}

/// The driver carries a lost launch through the status read to the answer.
#[tokio::test]
async fn the_driver_follows_the_machine_to_its_outcome() {
    let script = Arc::new(Mutex::new(vec![started(), status(&[]), lost()]));
    let sends = Arc::new(Mutex::new(Vec::new()));
    let (log, answers) = (Arc::clone(&sends), Arc::clone(&script));
    let b = RetryBudget {
        attempts: NonZeroU32::new(3).expect("non-zero"),
        pause: Duration::ZERO,
    };
    let outcome = drive(target(), launch(), b, move |req| {
        log.lock().expect("lock").push(req);
        let answer = answers.lock().expect("lock").pop().expect("scripted");
        Box::pin(async move { Ok(answer) })
    })
    .await
    .expect("no local refusal");
    assert_eq!(
        *sends.lock().expect("lock"),
        vec![launch(), ControlReq::Status, launch()]
    );
    assert!(matches!(outcome, Outcome::Answered { attempts: 3, .. }));
}
