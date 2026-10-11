// SPDX-License-Identifier: MIT OR Apache-2.0

//! `revocation-list.v1`: every release manifest, compatibility receipt and
//! release signer the registry has withdrawn, cumulative and numbered. It is
//! trusted only as a `document-signature` of kind `revocation_list`
//! verifies it (`release::revocations`).

use super::ids::{NodeId, Sha256Digest, SignerId};
use super::scalars::{UnixMs, V1, non_empty};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

/// What was withdrawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevokedSubject {
    /// A release manifest, by digest.
    ReleaseManifest(Sha256Digest),
    /// A compatibility receipt, by digest.
    CompatibilityReceipt(Sha256Digest),
    /// A release signer.
    Signer(SignerId),
}

/// Why it was withdrawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationReason {
    /// A newer one replaced it.
    Superseded,
    /// It is wrong.
    Defect,
    /// The evidence behind it does not hold.
    EvidenceInvalid,
    /// It may no longer be used.
    LicenseWithdrawn,
    /// The signer's key is compromised.
    KeyCompromised,
}

/// One withdrawal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revocation {
    /// What.
    pub subject: RevokedSubject,
    /// Why.
    pub reason: RevocationReason,
    /// When.
    pub revoked_at_unix_ms: UnixMs,
}

/// The registry's withdrawals, all of them, at one sequence number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevocationList {
    /// Always 1.
    pub version: V1,
    /// The registry that issued it.
    pub issuer: NodeId,
    /// One more with every revocation; a reader refuses one below a
    /// sequence it already holds.
    pub sequence: NonZeroU64,
    /// When it was issued.
    pub issued_at_unix_ms: UnixMs,
    /// Every withdrawal so far.
    #[serde(deserialize_with = "non_empty")]
    pub revoked: Vec<Revocation>,
}
