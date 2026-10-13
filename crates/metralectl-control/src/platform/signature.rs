// SPDX-License-Identifier: MIT OR Apache-2.0

//! `document-signature.v1`: a release signer's detached hybrid signature
//! over one platform document's canonical-JSON SHA-256.
//!
//! The signed string is the kind's domain followed by the 32 digest bytes.
//! The document carries the signer's identity and never its keys: a verifier
//! takes the keys from its own trust anchor (`release::signing`), so a
//! signature cannot vouch for itself.

use super::ids::{Sha256Digest, SignerId};
use super::scalars::V1;
use metralectl_protocol::hexid;
use serde::{Deserialize, Serialize};

/// What a signature is over. Closed: each kind has its own domain, so a
/// signature over one kind never verifies as another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignedKind {
    /// A `release-manifest` document.
    ReleaseManifest,
    /// A `revocation-list` document.
    RevocationList,
}

impl SignedKind {
    /// The domain-separation prefix of this kind's signed string.
    #[must_use]
    pub const fn domain(self) -> &'static [u8] {
        match self {
            Self::ReleaseManifest => b"mfs/release-manifest/v1",
            Self::RevocationList => b"mfs/revocation-list/v1",
        }
    }
}

/// Exactly `N` bytes, written as hex (lower case; either case is read).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HexBytes<const N: usize>(Box<[u8; N]>);

impl<const N: usize> HexBytes<N> {
    /// Wrap raw bytes.
    #[must_use]
    pub fn new(bytes: [u8; N]) -> Self {
        Self(Box::new(bytes))
    }

    /// The bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; N] {
        &self.0
    }
}

impl<const N: usize> TryFrom<String> for HexBytes<N> {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        hexid::decode::<N>(&s).map(Self::new)
    }
}

impl<const N: usize> From<HexBytes<N>> for String {
    fn from(b: HexBytes<N>) -> Self {
        hexid::encode(b.as_bytes())
    }
}

/// Bytes of an Ed25519 public key.
pub const ED25519_PUBLIC_LEN: usize = 32;
/// Bytes of an Ed25519 signature.
pub const ED25519_SIGNATURE_LEN: usize = 64;
/// Bytes of an ML-DSA-65 public key.
pub const ML_DSA_65_PUBLIC_LEN: usize = 1952;
/// Bytes of an ML-DSA-65 signature.
pub const ML_DSA_65_SIGNATURE_LEN: usize = 3309;

/// Both halves of a hybrid signature, over the same signed string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HybridSignature {
    /// Ed25519 (RFC 8032).
    pub ed25519: HexBytes<ED25519_SIGNATURE_LEN>,
    /// ML-DSA-65 (FIPS 204, deterministic, empty context).
    pub ml_dsa_65: HexBytes<ML_DSA_65_SIGNATURE_LEN>,
}

/// One signature over one document's digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSignature {
    /// Always 1.
    pub version: V1,
    /// What the document is.
    pub kind: SignedKind,
    /// The document's canonical-JSON SHA-256.
    pub document: Sha256Digest,
    /// Who signed it, by registry identity.
    pub signer: SignerId,
    /// The signature.
    pub signature: HybridSignature,
}
