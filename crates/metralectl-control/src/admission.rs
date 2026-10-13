// SPDX-License-Identifier: MIT OR Apache-2.0

//! Whether this node takes new launches.
//!
//! A cordoned or draining node refuses every new launch, from every caller,
//! because the refusal lives in the one control core they all go through.
//! What already runs is untouched: stop, status, stats and logs still answer,
//! so a node being retired can be wound down by the same verbs.

use std::sync::{PoisonError, RwLock};

/// A node's admission state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// New launches are taken.
    Open,
    /// No new launches; what runs keeps running.
    Cordoned {
        /// Why, for the refusal.
        reason: String,
    },
    /// No new launches while the host winds down what runs.
    Draining {
        /// Why, for the refusal.
        reason: String,
    },
}

impl Admission {
    /// Whether a new launch may start.
    ///
    /// # Errors
    /// The refusal's reason when the node is cordoned or draining.
    pub fn admits(&self) -> Result<(), String> {
        match self {
            Self::Open => Ok(()),
            Self::Cordoned { reason } => Err(format!(
                "this machine is cordoned and takes no new launches: {reason}"
            )),
            Self::Draining { reason } => Err(format!(
                "this machine is draining and takes no new launches: {reason}"
            )),
        }
    }
}

/// The admission state every caller of one node's control core reads.
///
/// Shared, not copied: a host that cordons the node through this gate is
/// heard by the browser session and the peer channel alike.
#[derive(Debug)]
pub struct AdmissionGate(RwLock<Admission>);

impl AdmissionGate {
    /// A gate in an explicit starting state.
    #[must_use]
    pub fn new(initial: Admission) -> Self {
        Self(RwLock::new(initial))
    }

    /// The state now.
    #[must_use]
    pub fn current(&self) -> Admission {
        // A poisoned lock means a writer panicked mid-assignment of a plain
        // value; the value is still whole, so it is read rather than lost.
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Move the node to `state`.
    pub fn set(&self, state: Admission) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = state;
    }
}
