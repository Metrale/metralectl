// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded retry of one control request toward another machine.
//!
//! The decisions are pure: [`ControlRun`] says what to send next, given what
//! the last send produced. [`drive`] is the only part that waits, and it
//! reaches the network solely through the `attempt` it is handed, so the
//! machine is tested without sockets and the driver without a fleet.
//!
//! There are no idempotency tokens. The target's launcher state answers the
//! question a token would: before a second `Launch` the run reads `Status`,
//! and a recipe already running there ends the retry. A repeated `Stop` and
//! every read-only verb are sent again as they are, because the target makes
//! them safe to repeat (`launcher::idempotent`).
//!
//! A refusal is an answer. It ends the run on the send that drew it, whoever
//! made it, and is never retried as if the network had eaten it. Only a LOST
//! send is retried: no answer at all, or a relay reporting that its own leg
//! to the target was lost (`RelayRefused { transient: true }`).

use metralectl_protocol::RecipeId;
use metralectl_protocol::fleet::NodeId;
use metralectl_protocol::msg::{AgentError, ControlRep, ControlReq, RunningLaunch};
use std::num::NonZeroU32;
use std::time::Duration;

/// How hard one control request is tried. Every field is required (PCND).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryBudget {
    /// Sends allowed, the `Status` read before a repeated `Launch` included.
    pub attempts: NonZeroU32,
    /// The wait before every send after the first, so a peer that is down is
    /// not hammered while it comes back.
    pub pause: Duration,
}

/// What one send produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Attempt {
    /// Somebody answered. A refusal is an answer.
    Answered {
        /// The answer.
        rep: ControlRep,
        /// The relay it came through, when it was forwarded.
        via: Option<NodeId>,
    },
    /// Nobody answered: the dial failed or the answer budget elapsed. Whether
    /// the target executed the request is unknown.
    Lost {
        /// What the transport reported, in the operator's terms.
        error: AgentError,
    },
}

/// The last send that went unanswered, kept for the report when the budget
/// runs out.
#[derive(Debug, Clone, PartialEq)]
pub struct LostSend {
    /// The relay that reported its leg lost, or `None` when nobody answered.
    pub by: Option<NodeId>,
    /// What was reported.
    pub error: AgentError,
    /// The relay the send went through, when it was forwarded.
    pub via: Option<NodeId>,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The target answered the request.
    Answered {
        /// The answer.
        rep: ControlRep,
        /// The relay it came through.
        via: Option<NodeId>,
        /// Sends made.
        attempts: u32,
    },
    /// Someone on the path said no, and said who they are.
    Refused {
        /// Who refused.
        by: NodeId,
        /// Why.
        error: AgentError,
        /// The relay the refusal came through.
        via: Option<NodeId>,
        /// Sends made.
        attempts: u32,
    },
    /// A `Launch` whose reply was lost, found running by the `Status` read
    /// that precedes a second `Launch` — so no second one was sent.
    AlreadyRunning {
        /// The recipe that was asked for.
        recipe: RecipeId,
        /// The launch the target reported.
        launch: RunningLaunch,
        /// The relay the status came through.
        via: Option<NodeId>,
        /// Sends made.
        attempts: u32,
    },
    /// Every send in the budget went unanswered or ended inconclusively.
    Exhausted {
        /// Sends made.
        attempts: u32,
        /// The machine that answered anything last, or `None` when no
        /// machine answered any send.
        last_responder: Option<NodeId>,
        /// The send that ended the run.
        lost: LostSend,
    },
}

/// What to do next.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Send this, after waiting `pause`.
    Send {
        /// The request.
        req: ControlReq,
        /// How long to wait first.
        pause: Duration,
    },
    /// The run is over.
    Done(Outcome),
}

/// One request's run: the pure half of the retry.
#[derive(Debug)]
pub struct ControlRun {
    target: NodeId,
    req: ControlReq,
    budget: RetryBudget,
    sent: u32,
    /// True while the last send was the `Status` read before a repeated
    /// `Launch`.
    probing: bool,
    last_responder: Option<NodeId>,
    last_lost: Option<LostSend>,
}

impl ControlRun {
    /// Begin a run of `req` at `target`, returning it and its first step.
    #[must_use]
    pub fn start(target: NodeId, req: ControlReq, budget: RetryBudget) -> (Self, Step) {
        let mut run = Self {
            target,
            req: req.clone(),
            budget,
            sent: 0,
            probing: false,
            last_responder: None,
            last_lost: None,
        };
        let first = run.send(req);
        (run, first)
    }

    /// Record what the last send produced and decide the next step.
    pub fn record(&mut self, attempt: Attempt) -> Step {
        let (rep, via) = match attempt {
            Attempt::Lost { error } => {
                return self.lost(LostSend {
                    by: None,
                    error,
                    via: None,
                });
            }
            Attempt::Answered { rep, via } => (rep, via),
        };
        match rep {
            ControlRep::Refused {
                by,
                error:
                    error @ AgentError::RelayRefused {
                        transient: true, ..
                    },
            } => {
                self.last_responder = Some(by);
                self.lost(LostSend {
                    by: Some(by),
                    error,
                    via,
                })
            }
            ControlRep::Refused { by, error } => Step::Done(Outcome::Refused {
                by,
                error,
                via,
                attempts: self.sent,
            }),
            rep => {
                self.last_responder = Some(self.target);
                match (rep, self.launched_recipe()) {
                    (ControlRep::Status { running }, Some(recipe)) if self.probing => {
                        self.probed(recipe, running, via)
                    }
                    (rep, _) => Step::Done(Outcome::Answered {
                        rep,
                        via,
                        attempts: self.sent,
                    }),
                }
            }
        }
    }

    fn send(&mut self, req: ControlReq) -> Step {
        let pause = if self.sent == 0 {
            Duration::ZERO
        } else {
            self.budget.pause
        };
        self.probing = matches!(req, ControlReq::Status) && self.launched_recipe().is_some();
        self.sent += 1;
        Step::Send { req, pause }
    }

    /// A send went unanswered: retry within the budget, reading `Status`
    /// first when the request was a `Launch`.
    fn lost(&mut self, lost: LostSend) -> Step {
        if self.sent >= self.budget.attempts.get() {
            return self.exhausted(lost);
        }
        self.last_lost = Some(lost);
        if self.launched_recipe().is_some() {
            return self.send(ControlReq::Status);
        }
        self.send(self.req.clone())
    }

    /// The `Status` read before a repeated `Launch` came back.
    fn probed(
        &mut self,
        recipe: RecipeId,
        running: Vec<RunningLaunch>,
        via: Option<NodeId>,
    ) -> Step {
        if let Some(launch) = running
            .into_iter()
            .find(|l| l.recipe.as_ref() == Some(&recipe))
        {
            return Step::Done(Outcome::AlreadyRunning {
                recipe,
                launch,
                via,
                attempts: self.sent,
            });
        }
        // Not running: the lost launch did not happen (or has not yet), so a
        // second one is safe — the target serialises it behind the first.
        match self.last_lost.take() {
            Some(lost) if self.sent >= self.budget.attempts.get() => self.exhausted(lost),
            _ => self.send(self.req.clone()),
        }
    }

    fn exhausted(&self, lost: LostSend) -> Step {
        Step::Done(Outcome::Exhausted {
            attempts: self.sent,
            last_responder: self.last_responder,
            lost,
        })
    }

    /// The recipe of a `Launch` run; `None` for every other verb.
    fn launched_recipe(&self) -> Option<RecipeId> {
        match &self.req {
            ControlReq::Launch { recipe, .. } => Some(recipe.clone()),
            _ => None,
        }
    }
}

impl Outcome {
    /// The shape the session's relay port answers in.
    ///
    /// A refusal keeps naming who refused. A launch found already running is
    /// THIS agent's conclusion from the target's status, so it is this agent's
    /// error, not a refusal put in the target's mouth. A spent budget reports
    /// the last thing that went wrong, with the attempt count and the last
    /// machine that answered written into it — as the relay's refusal when a
    /// relay was the one that answered, as this agent's error when nobody did.
    ///
    /// # Errors
    /// `AlreadyRunning`, or the last lost send's error when nobody answered.
    pub fn into_relay(self) -> Result<(ControlRep, Option<NodeId>), AgentError> {
        match self {
            Self::Answered { rep, via, .. } => Ok((rep, via)),
            Self::Refused { by, error, via, .. } => Ok((ControlRep::Refused { by, error }, via)),
            Self::AlreadyRunning { recipe, .. } => Err(AgentError::AlreadyRunning { recipe }),
            Self::Exhausted {
                attempts,
                last_responder,
                lost,
            } => {
                let error = noted(lost.error, attempts, last_responder);
                match lost.by {
                    Some(by) => Ok((ControlRep::Refused { by, error }, lost.via)),
                    None => Err(error),
                }
            }
        }
    }
}

/// `error` with the attempt count and the last responder written into its
/// operator-facing text, so a browser shows how hard this agent tried.
fn noted(error: AgentError, attempts: u32, last_responder: Option<NodeId>) -> AgentError {
    let note = match last_responder {
        Some(n) => format!(
            " (gave up after {attempts} attempts; last answer from {})",
            n.short()
        ),
        None => format!(" (gave up after {attempts} attempts; no machine answered)"),
    };
    match error {
        AgentError::NotRoutable { node, reason } => AgentError::NotRoutable {
            node,
            reason: reason + &note,
        },
        AgentError::RelayRefused {
            node,
            via,
            detail,
            transient,
        } => AgentError::RelayRefused {
            node,
            via,
            detail: detail + &note,
            transient,
        },
        other => other,
    }
}

/// Run `req` at `target` to an [`Outcome`], sending through `attempt`.
///
/// `attempt` is the transport: one send, its answer or its loss. Its `Err`
/// is a local refusal no retry can change — no route at all — and ends the
/// run at once.
///
/// # Errors
/// Whatever `attempt` refuses with.
pub async fn drive<'a, F>(
    target: NodeId,
    req: ControlReq,
    budget: RetryBudget,
    mut attempt: F,
) -> Result<Outcome, AgentError>
where
    F: FnMut(ControlReq) -> crate::BoxFut<'a, Result<Attempt, AgentError>>,
{
    let (mut run, mut step) = ControlRun::start(target, req, budget);
    loop {
        match step {
            Step::Done(outcome) => return Ok(outcome),
            Step::Send { req, pause } => {
                if !pause.is_zero() {
                    tokio::time::sleep(pause).await;
                }
                step = run.record(attempt(req).await?);
            }
        }
    }
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
