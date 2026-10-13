// SPDX-License-Identifier: MIT OR Apache-2.0

//! The reconciler's inputs and outputs, and nothing else.
//!
//! Kept in one module so they can follow the platform's deployment contracts
//! (`model-deployment.v1`, `deployment-action.v1` and its result) without the
//! decision logic moving. The words already match them: a deployment is named
//! by a DNS label and realised at a generation of at least 1; placement is a
//! hardware class or an explicit node; a refusal is one of the contract's
//! closed reasons, [`RefusalReason`] in the vendored contract's binding
//! ([`crate::platform`]).

use crate::platform::action::RefusalReason;
use metralectl_core::hardware::{HardwareClass, is_dns_label};
use metralectl_protocol::RecipeId;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

/// A deployment's name: a DNS label, unique within its scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeploymentName(String);

impl DeploymentName {
    /// Parse a deployment name.
    ///
    /// # Errors
    /// When it is not a DNS label.
    pub fn parse(s: &str) -> Result<Self, String> {
        if is_dns_label(s) {
            Ok(Self(s.to_owned()))
        } else {
            Err(format!("{s:?} is not a deployment name (a DNS label)"))
        }
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A desired generation: 1 when a deployment is created, one more on every
/// change to what it should run. Zero does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Generation(NonZeroU64);

impl Generation {
    /// A generation, or `None` for zero.
    #[must_use]
    pub fn new(n: u64) -> Option<Self> {
        NonZeroU64::new(n).map(Self)
    }

    /// The number.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl TryFrom<String> for DeploymentName {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Self::parse(&s)
    }
}

impl From<DeploymentName> for String {
    fn from(n: DeploymentName) -> Self {
        n.0
    }
}

/// Which deployment, at which generation, a workload was started for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    /// The deployment.
    pub deployment: DeploymentName,
    /// The generation it was started to realise.
    pub generation: Generation,
}

/// Where the plane may run a deployment. Mirrors the contract's placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// Any node of this class; this node must be one.
    HardwareClass(HardwareClass),
    /// The plane named this node explicitly, so no class is required of it.
    ThisNode,
}

/// What the plane wants of one deployment on this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Serve: the contract's `apply`.
    Run {
        /// The recipe to launch. Its own settings are the launch's flags;
        /// nothing else is sent.
        recipe: RecipeId,
        /// The model the release says that recipe serves.
        model: String,
        /// Where the deployment may run.
        placement: Placement,
    },
    /// Serve nothing: the contract's `stop`.
    Stopped,
}

/// The desired state of one deployment on this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desired {
    /// The deployment.
    pub deployment: DeploymentName,
    /// The generation this state belongs to.
    pub generation: Generation,
    /// What it should be doing.
    pub intent: Intent,
}

/// One running workload, as the launcher reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    /// The recipe it runs.
    pub recipe: RecipeId,
    /// The deployment and generation it was started for; `None` for a
    /// workload started some other way (a browser, the command line), which
    /// the reconciler never stops.
    pub owner: Option<Owner>,
}

/// What this node is, and what it is running. Read from the launcher and the
/// machine, never from a memory of earlier decisions, so a restarted agent
/// observes exactly what it would have before the restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed<'a> {
    /// Every running workload, owned or not.
    pub instances: &'a [Instance],
    /// What this machine's accelerator reports itself to be; empty when it
    /// reported nothing.
    pub accelerator: &'a str,
    /// The model this node's own copy of the desired recipe serves; `None`
    /// when this node has no such recipe.
    pub recipe_model: Option<&'a str>,
}

/// Why a desired state is not applied. Closed; each is a contract reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// This node runs a newer generation of the deployment.
    StaleGeneration {
        /// The newest generation running here.
        running: Generation,
    },
    /// The generation is already realised here with a different recipe: one
    /// generation names one desired state, so this is a second meaning for it.
    IdempotencyConflict {
        /// What that generation runs here.
        running: RecipeId,
    },
    /// The release does not describe this node's recipe: the recipe is not
    /// here, or here it serves another model.
    ReleaseUnverified {
        /// What did not match.
        reason: String,
    },
    /// This node is not hardware the placement allows.
    HardwareIncompatible {
        /// The gate's reason.
        reason: String,
    },
    /// Realising it would touch a workload that is not this deployment's.
    Busy {
        /// The recipe that workload runs.
        recipe: RecipeId,
        /// Whose it is; `None` for one started outside any deployment.
        owner: Option<Owner>,
    },
}

impl Refusal {
    /// The contract's reason for this refusal.
    #[must_use]
    pub fn reason(&self) -> RefusalReason {
        match self {
            Self::StaleGeneration { .. } => RefusalReason::StaleGeneration,
            Self::IdempotencyConflict { .. } => RefusalReason::IdempotencyConflict,
            Self::ReleaseUnverified { .. } => RefusalReason::ReleaseUnverified,
            Self::HardwareIncompatible { .. } => RefusalReason::HardwareIncompatible,
            Self::Busy { .. } => RefusalReason::Busy,
        }
    }
}

/// The one thing to do next. Closed: the reconciler can ask for a launch or a
/// stop of a recipe and nothing else, and never for a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The desired generation is in place; nothing is started or stopped
    /// (the contract's `already_applied`).
    Converged,
    /// Launch this recipe for the deployment.
    Launch {
        /// The recipe.
        recipe: RecipeId,
    },
    /// Stop this deployment's workload running this recipe.
    Stop {
        /// The recipe.
        recipe: RecipeId,
    },
    /// Do nothing, for this reason.
    Refuse(Refusal),
}
