// SPDX-License-Identifier: MIT OR Apache-2.0

//! Per-generation convergence of one deployment on this node.
//!
//! Pure and level-triggered: [`decide`] reads the desired state and what the
//! node runs right now, and names the ONE next step. A host carries it out
//! through the control core and asks again, until the answer is
//! [`Step::Converged`] or a refusal. Nothing is remembered between calls; the
//! launcher's own account of what runs is the only state. That is what makes
//! every failure case the same case:
//!
//! - a repeated apply, or one resent after its acknowledgement was lost,
//!   observes the workload the first one started and answers `Converged`;
//! - a restarted host observes exactly what it observed before;
//! - a launch that failed leaves nothing running, so the next call launches
//!   again, never alongside a first;
//! - a stop that failed leaves the workload running, so the next call stops it
//!   again rather than launching over it.
//!
//! Only this deployment's own workloads are ever stopped. Anything the
//! realisation would touch that is not its own is a refusal, not a step.

pub mod contract;

use contract::{DeploymentName, Desired, Instance, Intent, Observed, Placement, Refusal, Step};
use metralectl_protocol::msg::ControlReq;
use std::collections::BTreeMap;

/// The next step toward `desired`, given what this node runs.
#[must_use]
pub fn decide(desired: &Desired, observed: &Observed<'_>) -> Step {
    let ours = &desired.deployment;
    let mine: Vec<&Instance> = observed
        .instances
        .iter()
        .filter(|i| is_ours(i, ours))
        .collect();
    let generation_of = |i: &Instance| i.owner.as_ref().map(|o| o.generation);

    // Applying an older generation over a newer one would roll the deployment
    // back on the say-so of a delayed or replayed message.
    if let Some(running) = mine.iter().filter_map(|i| generation_of(i)).max()
        && running > desired.generation
    {
        return Step::Refuse(Refusal::StaleGeneration { running });
    }

    match &desired.intent {
        Intent::Stopped => match mine.first() {
            Some(own) => stop(&own.recipe, observed.instances, ours),
            None => Step::Converged,
        },
        Intent::Run {
            recipe,
            model,
            placement,
        } => {
            let (current, older): (Vec<&Instance>, Vec<&Instance>) = mine
                .iter()
                .partition(|i| generation_of(i) == Some(desired.generation));
            if let Some(other) = current.iter().find(|i| i.recipe != *recipe) {
                return Step::Refuse(Refusal::IdempotencyConflict {
                    running: other.recipe.clone(),
                });
            }
            if current.is_empty()
                && let Some(refusal) = launch_blocked(recipe, model, placement, observed, ours)
            {
                // Checked BEFORE the older generation is stopped: tearing down
                // what serves for a generation that cannot start here would
                // leave the deployment with nothing.
                return Step::Refuse(refusal);
            }
            match (older.first(), current.is_empty()) {
                (Some(old), _) => stop(&old.recipe, observed.instances, ours),
                (None, true) => Step::Launch {
                    recipe: recipe.clone(),
                },
                (None, false) => Step::Converged,
            }
        }
    }
}

/// Why `recipe` cannot be launched for this deployment here, if it cannot.
fn launch_blocked(
    recipe: &metralectl_protocol::RecipeId,
    model: &str,
    placement: &Placement,
    observed: &Observed<'_>,
    ours: &DeploymentName,
) -> Option<Refusal> {
    if let Placement::HardwareClass(class) = placement
        && let Err(reason) = metralectl_core::hardware::check(class, observed.accelerator)
    {
        return Some(Refusal::HardwareIncompatible { reason });
    }
    match observed.recipe_model {
        None => {
            return Some(Refusal::ReleaseUnverified {
                reason: format!("this node has no recipe {recipe}"),
            });
        }
        Some(here) if here != model => {
            return Some(Refusal::ReleaseUnverified {
                reason: format!(
                    "the release says {recipe} serves {model}; this node's {recipe} serves {here}"
                ),
            });
        }
        Some(_) => {}
    }
    // One recipe runs once per node. Launching it while another workload runs
    // it would either collide with that workload or adopt it as this
    // deployment's, and the second is how an unrelated workload gets stopped
    // later.
    foreign(recipe, observed.instances, ours).map(|other| Refusal::Busy {
        recipe: recipe.clone(),
        owner: other.owner.clone(),
    })
}

/// Stop this deployment's `recipe` — unless a stop of that recipe would also
/// stop a workload that is not this deployment's.
fn stop(
    recipe: &metralectl_protocol::RecipeId,
    instances: &[Instance],
    ours: &DeploymentName,
) -> Step {
    // A stop is by recipe, and every container of the recipe goes. A foreign
    // one sharing it (a cluster rank started by hand, say) would go too.
    match foreign(recipe, instances, ours) {
        Some(other) => Step::Refuse(Refusal::Busy {
            recipe: recipe.clone(),
            owner: other.owner.clone(),
        }),
        None => Step::Stop {
            recipe: recipe.clone(),
        },
    }
}

/// Whether a workload was started for deployment `ours`.
fn is_ours(instance: &Instance, ours: &DeploymentName) -> bool {
    instance
        .owner
        .as_ref()
        .is_some_and(|o| o.deployment == *ours)
}

/// A running workload of `recipe` that is not deployment `ours`'s.
fn foreign<'a>(
    recipe: &metralectl_protocol::RecipeId,
    instances: &'a [Instance],
    ours: &DeploymentName,
) -> Option<&'a Instance> {
    instances
        .iter()
        .find(|i| i.recipe == *recipe && !is_ours(i, ours))
}

impl Step {
    /// The control request that carries this step out, when it is an action.
    ///
    /// A launch sends no settings: the recipe's own values are the launch,
    /// which is what the release named. Only the closed `Launch` and `Stop`
    /// verbs can come out of here.
    #[must_use]
    pub fn request(&self) -> Option<ControlReq> {
        match self {
            Self::Launch { recipe } => Some(ControlReq::Launch {
                recipe: recipe.clone(),
                settings: BTreeMap::new(),
            }),
            Self::Stop { recipe } => Some(ControlReq::Stop {
                recipe: recipe.clone(),
            }),
            Self::Converged | Self::Refuse(_) => None,
        }
    }
}

#[cfg(test)]
#[path = "reconcile/tests.rs"]
mod tests;
