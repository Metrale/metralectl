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
use crate::control::LocalControl;
use crate::launcher::{LaunchFor, Workload};
use crate::platform::action::{
    ActionKind, ActionOutcome, DeploymentAction, DeploymentActionResult, RefusalReason, Refused,
};
use crate::platform::ids::{InstanceId, NodeId};
use crate::platform::scalars::{UnixMs, V1};
use crate::reconcile::contract::Generation;
use metralectl_protocol::RecipeId;
use metralectl_protocol::msg::AgentError;
use std::collections::BTreeMap;
use std::num::NonZeroU32;

/// What a deployment runs, as the host resolved it from the deployment's spec
/// and its verified release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// This machine's recipe for the release.
    pub recipe: RecipeId,
    /// The model the release says that recipe serves.
    pub model: String,
    /// Where the deployment may run.
    pub placement: Placement,
}

/// The desired state an action asks for.
///
/// # Errors
/// For `drain`: winding down one deployment's requests needs the runtime to
/// stop admitting them, and the agent has no such verb to call.
pub fn desired_for(action: &DeploymentAction, target: &Target) -> Result<Desired, String> {
    let intent = match action.action {
        ActionKind::Apply => Intent::Run {
            recipe: target.recipe.clone(),
            model: target.model.clone(),
            placement: target.placement.clone(),
        },
        ActionKind::Stop => Intent::Stopped,
        ActionKind::Drain => {
            return Err(
                "drain is not carried out by this agent: it needs the runtime to stop \
                 admitting the deployment's requests, which no runtime verb does yet"
                    .to_owned(),
            );
        }
    };
    Ok(Desired {
        deployment: action.deployment.clone(),
        generation: action.generation,
        intent,
    })
}

/// What came of converging one deployment, in the result contract's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Applied, already applied, or refused with a contract reason.
    pub outcome: ActionOutcome,
    /// The newest generation of the deployment running here, if any.
    pub running_generation: Option<Generation>,
    /// The deployment's instances running here.
    pub instances: Vec<InstanceId>,
}

/// Converge `desired` on this node, taking at most `max_steps` launches and
/// stops.
///
/// # Errors
/// A launch or stop the control core refused or that failed (a cordoned node,
/// a docker failure), a launcher that cannot be observed, or no convergence
/// within `max_steps`. Each leaves the node as the launcher reports it, and
/// the next call starts from there.
pub async fn converge(
    control: &LocalControl<'_>,
    desired: &Desired,
    max_steps: NonZeroU32,
) -> Result<Answer, AgentError> {
    let mut acted = false;
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
            Step::Launch { recipe } => {
                let owner = Owner {
                    deployment: desired.deployment.clone(),
                    generation: desired.generation,
                };
                let launch_for = LaunchFor { owner: Some(owner) };
                control
                    .launch(&recipe, &BTreeMap::new(), launch_for)
                    .await?;
                acted = true;
                continue;
            }
            Step::Stop { recipe } => {
                control.stop(&recipe).await?;
                acted = true;
                continue;
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
        running_generation: ours.iter().map(|(_, g)| *g).max(),
        instances: ours.iter().map(|(w, _)| w.id).collect(),
    }
}

/// Answer one `DeploymentAction` addressed to `node`.
///
/// An action for another node is refused `not_here` and nothing is touched.
///
/// # Errors
/// As [`converge`]; and an action this agent does not carry out
/// ([`desired_for`]) is refused as `NotLaunchable`.
pub async fn answer_action(
    control: &LocalControl<'_>,
    node: NodeId,
    action: &DeploymentAction,
    target: &Target,
    max_steps: NonZeroU32,
    now: &(dyn Fn() -> UnixMs + Sync),
) -> Result<DeploymentActionResult, AgentError> {
    let desired = desired_for(action, target).map_err(|reason| AgentError::NotLaunchable {
        recipe: target.recipe.clone(),
        reason,
    })?;
    let answered = if action.node_id == node {
        converge(control, &desired, max_steps).await?
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
