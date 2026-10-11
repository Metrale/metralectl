// SPDX-License-Identifier: MIT OR Apache-2.0

//! `deployment-action.v1` and `deployment-action-result.v1`: the one command
//! the plane sends an agent about a deployment, and the agent's answer.

use super::ids::{IdempotencyKey, InstanceId, NodeId, TraceId};
use super::scalars::{Scope, UnixMs, V1, present};
use crate::reconcile::contract::{DeploymentName, Generation};
use serde::{Deserialize, Serialize};

/// What the plane asks of one deployment on one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// Realise the deployment's generation here.
    Apply,
    /// Serve nothing for the deployment here.
    Stop,
    /// Stop taking new work for the deployment here.
    Drain,
}

/// A typed, idempotent command from the plane to one agent; never a command
/// line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentAction {
    /// Always 1.
    pub version: V1,
    /// The same key with the same body is the same action.
    pub idempotency_key: IdempotencyKey,
    /// The trace the action belongs to.
    pub trace_id: TraceId,
    /// Whose deployment.
    pub scope: Scope,
    /// Which deployment.
    pub deployment: DeploymentName,
    /// The generation to realise.
    pub generation: Generation,
    /// The one node it is for.
    pub node_id: NodeId,
    /// What to do.
    pub action: ActionKind,
    /// When the plane issued it.
    pub issued_at_unix_ms: UnixMs,
}

/// Why an action was not applied. Closed; the platform's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    /// A newer generation runs here.
    StaleGeneration,
    /// This generation already means something else here.
    IdempotencyConflict,
    /// The release did not verify, or does not describe what is here.
    ReleaseUnverified,
    /// No passing compatibility receipt.
    ReceiptMissing,
    /// This node is not hardware the deployment allows.
    HardwareIncompatible,
    /// The action names another node.
    NotHere,
    /// Applying it would touch a workload that is not the deployment's.
    Busy,
}

/// A refusal, as the result spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refused {
    /// Why.
    pub reason: RefusalReason,
}

/// What came of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionOutcome {
    /// It was carried out.
    Applied,
    /// What it asks for already held; nothing was started or stopped.
    AlreadyApplied,
    /// It was not carried out.
    Refused(Refused),
}

/// The agent's answer to a [`DeploymentAction`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentActionResult {
    /// Always 1.
    pub version: V1,
    /// The action's key.
    pub idempotency_key: IdempotencyKey,
    /// The node answering.
    pub node_id: NodeId,
    /// What came of it.
    pub outcome: ActionOutcome,
    /// The newest generation of the deployment running here; `null` when
    /// none runs.
    #[serde(deserialize_with = "present")]
    pub running_generation: Option<Generation>,
    /// The deployment's instances running here.
    pub instances: Vec<InstanceId>,
    /// When the agent answered.
    pub answered_at_unix_ms: UnixMs,
}
