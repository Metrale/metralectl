// SPDX-License-Identifier: MIT OR Apache-2.0

//! `model-deployment.v1`: the desired state of one served model (its spec at
//! a generation) and the last state observed (its status).

use super::ids::{InstanceId, NodeId, Sha256Digest};
use super::scalars::{ModelName, Scope, UnixMs, V1, non_empty, present};
use crate::reconcile::contract::{DeploymentName, Generation};
use metralectl_core::hardware::HardwareClass;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

/// The shape of one replica.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    /// Nodes.
    pub nodes: NonZeroU32,
    /// Accelerators on each node.
    pub accelerators_per_node: NonZeroU32,
    /// Tensor-parallel degree.
    pub tensor_parallel: NonZeroU32,
    /// Pipeline-parallel degree.
    pub pipeline_parallel: NonZeroU32,
    /// Expert-parallel degree.
    pub expert_parallel: NonZeroU32,
}

/// Where the plane may run a deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentPlacement {
    /// Any node of this class.
    HardwareClass(HardwareClass),
    /// Exactly these nodes, at least one.
    Nodes(#[serde(deserialize_with = "non_empty")] Vec<NodeId>),
}

/// What a deployment should run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentSpec {
    /// The release manifest's content digest.
    pub release: Sha256Digest,
    /// The name requests use.
    pub served_model: ModelName,
    /// The shape of one replica.
    pub topology: Topology,
    /// How many replicas.
    pub replicas: NonZeroU32,
    /// Where.
    pub placement: DeploymentPlacement,
}

/// How far a deployment has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Nothing started yet.
    Pending,
    /// On its way.
    Progressing,
    /// Serving.
    Ready,
    /// Serving, but not fully.
    Degraded,
    /// Not serving.
    Failed,
}

/// What a condition is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionKind {
    /// The release's signature and digests verified.
    ReleaseVerified,
    /// A passing compatibility receipt covers it.
    ReceiptApproved,
    /// The hardware is what the release allows.
    HardwareCompatible,
    /// The runtime answers.
    RuntimeReady,
}

/// Whether a condition holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionStatus {
    /// It holds.
    True,
    /// It does not.
    False,
    /// Not known.
    Unknown,
}

/// One condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// About what.
    pub kind: ConditionKind,
    /// Whether it holds.
    pub status: ConditionStatus,
    /// Since when.
    pub since_unix_ms: UnixMs,
}

/// A runtime's health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// Loading.
    Starting,
    /// Serving.
    Ready,
    /// Serving with a fault.
    Degraded,
    /// Finishing what it has, taking nothing new.
    Draining,
    /// Not serving.
    Stopped,
}

/// One instance, as last observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedInstance {
    /// Which instance.
    pub instance_id: InstanceId,
    /// On which node.
    pub node_id: NodeId,
    /// The digest of that node's hardware fingerprint.
    pub hardware_fingerprint: Sha256Digest,
    /// The generation it was started for.
    pub generation: Generation,
    /// Its health.
    pub state: HealthState,
}

/// What was last observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentStatus {
    /// The generation it describes.
    pub observed_generation: Generation,
    /// How far it has got.
    pub phase: Phase,
    /// The conditions.
    pub conditions: Vec<Condition>,
    /// The instances.
    pub instances: Vec<ObservedInstance>,
    /// When it was written.
    pub updated_at_unix_ms: UnixMs,
}

/// One served model: desired spec at a generation, and the last status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDeployment {
    /// Always 1.
    pub version: V1,
    /// The deployment's name.
    pub name: DeploymentName,
    /// Whose.
    pub scope: Scope,
    /// The desired generation.
    pub generation: Generation,
    /// What it should run.
    pub spec: DeploymentSpec,
    /// What was last observed; `null` before anything was.
    #[serde(deserialize_with = "present")]
    pub status: Option<DeploymentStatus>,
}
