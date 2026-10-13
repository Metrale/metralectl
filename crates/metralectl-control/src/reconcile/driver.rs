// SPDX-License-Identifier: MIT OR Apache-2.0

//! The host half of convergence: observe what the launcher runs, ask
//! [`decide`] for the next step, carry it out through the control core, and
//! ask again, until the answer is converged or a refusal; then answer the
//! plane in the `deployment-action-result` shape.
//!
//! Nothing here remembers a decision. Every round re-reads the launcher's own
//! state, which carries the deployment and generation as container labels
//! written at launch, so a repeated action, one resent after a lost
//! acknowledgement, and one arriving after a restart all observe the same
//! thing and come to the same answer. Every launch and stop goes through
//! [`LocalControl`], so the admission, hardware and settings gates the
//! browser and peer paths pass are the reconciler's too.

use super::contract::{Desired, Instance, Intent, Observed, Owner, Placement, Step};
use super::decide;
use crate::admission::Admission;
use crate::control::LocalControl;
use crate::launcher::{LaunchFor, Workload};
use crate::platform::action::{
    ActionKind, ActionOutcome, DeploymentAction, DeploymentActionResult, Failed, FailureReason,
    RefusalReason, Refused,
};
use crate::platform::ids::{InstanceId, NodeId};
use crate::platform::scalars::{UnixMs, V1};
use crate::reconcile::contract::Generation;
use crate::release::VerifiedRelease;
use metralectl_protocol::RecipeId;
use metralectl_protocol::msg::{AgentError, AttemptId, ControlTrace};
use std::collections::BTreeMap;
use std::num::NonZeroU32;

/// What a deployment runs, as the host resolved it from the deployment's spec
/// and its release, verified (`release::verify`) before anything launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// This machine's recipe for the release.
    pub recipe: RecipeId,
    /// The release.
    pub release: VerifiedRelease,
    /// Where the deployment may run.
    pub placement: Placement,
}

/// The desired state an action asks for. The contract's actions are `apply`
/// and `stop`; draining is the node's admission state ([`crate::admission`]),
/// never an action about one deployment.
#[must_use]
pub fn desired_for(action: &DeploymentAction, target: &Target) -> Desired {
    let intent = match action.action {
        ActionKind::Apply => Intent::Run {
            recipe: target.recipe.clone(),
            model: target.release.model().to_owned(),
            placement: target.placement.clone(),
        },
        ActionKind::Stop => Intent::Stopped,
    };
    Desired {
        deployment: action.deployment.clone(),
        generation: action.generation,
        intent,
    }
}

/// What came of converging one deployment, in the result contract's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Applied, already applied, refused or failed with a contract reason.
    pub outcome: ActionOutcome,
    /// What the container runtime said when an attempt failed, for the
    /// host's own log. Never sent: the result carries only the closed reason.
    pub detail: Option<String>,
    /// The newest generation of the deployment running here, if any.
    pub running_generation: Option<Generation>,
    /// The deployment's instances running here.
    pub instances: Vec<InstanceId>,
}

/// Converge `desired` on this node, taking at most `max_steps` launches and
/// stops. A launch runs `release`; serving anything needs one, and serving
/// nothing does not, so a `Run` without a release is refused
/// `release_unverified` before anything is touched. A launch records `trace`
/// on the container it starts.
///
/// A launch on a cordoned or draining node is refused `cordoned` or
/// `draining`, and nothing is started; a stop still runs, because admission
/// gates only new work. A launch that was attempted and left nothing running
/// is `failed`: `image_unavailable` when the release's image could not be
/// made present, `launch_failed` when the container runtime refused it, and
/// `exited` when it started and is gone by the next observation. This
/// launcher does not wait for readiness, so it never answers `not_ready`.
///
/// # Errors
/// A launch or stop the control core refused for a reason the contract has
/// no word for (settings, an unknown recipe), a launcher that cannot be
/// observed, or no convergence within `max_steps`. Each leaves the node as
/// the launcher reports it, and the next call starts from there.
pub async fn converge(
    control: &LocalControl<'_>,
    desired: &Desired,
    release: Option<&VerifiedRelease>,
    trace: Option<ControlTrace>,
    max_steps: NonZeroU32,
) -> Result<Answer, AgentError> {
    if let Intent::Run { model, .. } = &desired.intent
        && release.is_none_or(|r| r.model() != model)
    {
        let refused = ActionOutcome::Refused(Refused {
            reason: RefusalReason::ReleaseUnverified,
        });
        return Ok(answer(refused, desired, &control.workloads().await?));
    }
    let mut acted = false;
    let mut launched: Option<RecipeId> = None;
    for _ in 0..=max_steps.get() {
        let workloads = control.workloads().await?;
        let instances: Vec<Instance> = workloads.iter().map(|w| w.instance.clone()).collect();
        let recipe_model = match &desired.intent {
            Intent::Run { recipe, .. } => control.recipe_model(recipe),
            Intent::Stopped => None,
        };
        let observed = Observed {
            instances: &instances,
            accelerator: control.accelerator,
            recipe_model: recipe_model.as_deref(),
        };
        let outcome = match decide(desired, &observed) {
            Step::Converged if acted => ActionOutcome::Applied,
            Step::Converged => ActionOutcome::AlreadyApplied,
            Step::Refuse(refusal) => ActionOutcome::Refused(Refused {
                reason: refusal.reason(),
            }),
            // It was started in this call and is not running now.
            Step::Launch { recipe } if launched.as_ref() == Some(&recipe) => {
                let exited = failed(FailureReason::Exited);
                return Ok(answer(exited, desired, &control.workloads().await?));
            }
            Step::Launch { recipe } => {
                let attempt = Attempt {
                    desired,
                    recipe: &recipe,
                    release,
                    trace,
                };
                if let Some((outcome, detail)) = attempt.launch(control).await? {
                    let mut a = answer(outcome, desired, &control.workloads().await?);
                    a.detail = detail;
                    return Ok(a);
                }
                launched = Some(recipe);
                acted = true;
                continue;
            }
            // A stop on the way to serving makes room for a launch; a node
            // that will not take the launch keeps what it serves instead of
            // tearing it down for nothing.
            Step::Stop { recipe } => {
                let closed = match desired.intent {
                    Intent::Run { .. } => admission_refusal(control),
                    Intent::Stopped => None,
                };
                if let Some(closed) = closed {
                    closed
                } else {
                    control.stop(&recipe).await?;
                    acted = true;
                    continue;
                }
            }
        };
        return Ok(answer(outcome, desired, &workloads));
    }
    Err(AgentError::LaunchFailed {
        detail: format!(
            "deployment {} did not converge within {max_steps} launches and stops",
            desired.deployment.as_str()
        ),
    })
}

/// One launch of `recipe` for `desired`, as the reconciler attempts it.
struct Attempt<'a> {
    desired: &'a Desired,
    recipe: &'a RecipeId,
    release: Option<&'a VerifiedRelease>,
    trace: Option<ControlTrace>,
}

impl Attempt<'_> {
    /// `None` when it started; otherwise the outcome that answers the action
    /// and what the runtime said.
    async fn launch(
        self,
        control: &LocalControl<'_>,
    ) -> Result<Option<(ActionOutcome, Option<String>)>, AgentError> {
        if let Some(closed) = admission_refusal(control) {
            return Ok(Some((closed, None)));
        }
        if let Some(release) = self.release
            && let Err(why) = control.fetch_image(release.image()).await
        {
            return Ok(Some((failed(FailureReason::ImageUnavailable), Some(why))));
        }
        let owner = Owner {
            deployment: self.desired.deployment.clone(),
            generation: self.desired.generation,
        };
        let launch_for = LaunchFor {
            owner: Some(owner),
            release: self.release.cloned(),
            trace: self.trace,
        };
        match control
            .launch(self.recipe, &BTreeMap::new(), launch_for)
            .await
        {
            Ok(_) => Ok(None),
            Err(e @ (AgentError::LaunchFailed { .. } | AgentError::DockerUnavailable { .. })) => {
                Ok(Some((
                    failed(FailureReason::LaunchFailed),
                    Some(e.to_string()),
                )))
            }
            // The node was cordoned between the check above and the launch.
            Err(e @ AgentError::NotLaunchable { .. }) => match admission_refusal(control) {
                Some(closed) => Ok(Some((closed, None))),
                None => Err(e),
            },
            Err(e) => Err(e),
        }
    }
}

/// The refusal a cordoned or draining node answers a launch with.
fn admission_refusal(control: &LocalControl<'_>) -> Option<ActionOutcome> {
    let reason = match control.admission.current() {
        Admission::Open => return None,
        Admission::Cordoned { .. } => RefusalReason::Cordoned,
        Admission::Draining { .. } => RefusalReason::Draining,
    };
    Some(ActionOutcome::Refused(Refused { reason }))
}

fn failed(reason: FailureReason) -> ActionOutcome {
    ActionOutcome::Failed(Failed { reason })
}

/// The answer for `outcome`, from what this deployment runs here.
fn answer(outcome: ActionOutcome, desired: &Desired, workloads: &[Workload]) -> Answer {
    let ours: Vec<(&Workload, Generation)> = workloads
        .iter()
        .filter_map(|w| match &w.instance.owner {
            Some(o) if o.deployment == desired.deployment => Some((w, o.generation)),
            _ => None,
        })
        .collect();
    Answer {
        outcome,
        detail: None,
        running_generation: ours.iter().map(|(_, g)| *g).max(),
        instances: ours.iter().map(|(w, _)| w.id).collect(),
    }
}

/// Answer one `DeploymentAction` addressed to `node`.
///
/// An action for another node is refused `not_here` and nothing is touched.
///
/// # Errors
/// As [`converge`].
pub async fn answer_action(
    control: &LocalControl<'_>,
    node: NodeId,
    action: &DeploymentAction,
    target: &Target,
    max_steps: NonZeroU32,
    now: &(dyn Fn() -> UnixMs + Sync),
) -> Result<DeploymentActionResult, AgentError> {
    let desired = desired_for(action, target);
    let answered = if action.node_id == node {
        // The plane's attempt at a deployment action is its idempotency
        // key: every resend of the action carries the same one.
        let trace = ControlTrace {
            attempt: AttemptId::from_bytes(*action.idempotency_key.as_bytes())
                .map_err(|detail| AgentError::LaunchFailed { detail })?,
            trace_id: action.trace_id,
        };
        let release = Some(&target.release);
        converge(control, &desired, release, Some(trace), max_steps).await?
    } else {
        let refused = ActionOutcome::Refused(Refused {
            reason: RefusalReason::NotHere,
        });
        answer(refused, &desired, &control.workloads().await?)
    };
    Ok(DeploymentActionResult {
        version: V1,
        idempotency_key: action.idempotency_key,
        node_id: node,
        outcome: answered.outcome,
        running_generation: answered.running_generation,
        instances: answered.instances,
        answered_at_unix_ms: now(),
    })
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
