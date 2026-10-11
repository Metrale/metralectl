// SPDX-License-Identifier: MIT OR Apache-2.0

//! A revocation list, accepted only as its own signature allows.
//!
//! [`accept`] is the one way a host turns a `revocation-list` into the
//! [`Revoked`] set the release gate reads. It refuses, each for its reason:
//! a list whose signature does not verify against the trusted principals; a
//! list signed by a signer the list itself revokes; a list older than one
//! already accepted (a rollback to a list that predates a revocation); and a
//! list issued longer ago than the host's maximum age. The maximum age and
//! the sequence already held are the host's to state: nothing defaults them.

use super::{Revoked, SignatureVerifier, SignerId};
use crate::platform::canonical::digest;
use crate::platform::revocation::{RevocationList, RevokedSubject};
use crate::platform::signature::{DocumentSignature, SignedKind};
use std::num::NonZeroU64;

/// What the host knows before it reads a new list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListPolicy {
    /// A list issued longer ago than this is stale.
    pub max_age_ms: u64,
    /// The highest sequence accepted before; `None` when none was.
    pub held_sequence: Option<NonZeroU64>,
    /// Now.
    pub now_unix_ms: u64,
}

/// Why a list was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListRefused {
    /// Its signature does not verify.
    Unsigned(String),
    /// It is signed by a signer it revokes.
    SignedByARevokedSigner(SignerId),
    /// It is older than the list already held.
    Rollback {
        /// The sequence held.
        held: NonZeroU64,
        /// This list's.
        got: NonZeroU64,
    },
    /// It was issued longer ago than the maximum age.
    Stale {
        /// How long ago.
        age_ms: u64,
        /// The most allowed.
        max_age_ms: u64,
    },
}

impl std::fmt::Display for ListRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsigned(why) => write!(f, "the revocation list is not signed: {why}"),
            Self::SignedByARevokedSigner(s) => {
                write!(f, "the revocation list is signed by {s}, which it revokes")
            }
            Self::Rollback { held, got } => write!(
                f,
                "the revocation list is sequence {got}, older than the {held} already held"
            ),
            Self::Stale { age_ms, max_age_ms } => write!(
                f,
                "the revocation list was issued {age_ms} ms ago; at most {max_age_ms} ms is accepted"
            ),
        }
    }
}

/// A list that passed [`accept`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// What it withdraws.
    pub revoked: Revoked,
    /// Its sequence: the host holds this from now on.
    pub sequence: NonZeroU64,
}

/// Accept `list`, signed by `signature`, under `policy`.
///
/// # Errors
/// The first refusal, in the order of the module documentation.
pub fn accept(
    verifier: &dyn SignatureVerifier,
    list: &RevocationList,
    signature: &DocumentSignature,
    policy: ListPolicy,
) -> Result<Accepted, ListRefused> {
    let document = digest(list).map_err(ListRefused::Unsigned)?;
    let signer = verifier
        .verify(SignedKind::RevocationList, &document, signature)
        .map_err(ListRefused::Unsigned)?;
    let mut revoked = Revoked::default();
    for r in &list.revoked {
        match r.subject {
            RevokedSubject::ReleaseManifest(d) => revoked.manifests.insert(d),
            RevokedSubject::CompatibilityReceipt(d) => revoked.receipts.insert(d),
            RevokedSubject::Signer(s) => revoked.signers.insert(s),
        };
    }
    if revoked.signers.contains(&signer) {
        return Err(ListRefused::SignedByARevokedSigner(signer));
    }
    if let Some(held) = policy.held_sequence
        && list.sequence < held
    {
        return Err(ListRefused::Rollback {
            held,
            got: list.sequence,
        });
    }
    let age_ms = policy
        .now_unix_ms
        .saturating_sub(list.issued_at_unix_ms.get());
    if age_ms > policy.max_age_ms {
        return Err(ListRefused::Stale {
            age_ms,
            max_age_ms: policy.max_age_ms,
        });
    }
    Ok(Accepted {
        revoked,
        sequence: list.sequence,
    })
}

#[cfg(test)]
#[path = "revocations_tests.rs"]
mod tests;
