// SPDX-License-Identifier: MIT OR Apache-2.0

//! The platform's release-signature scheme (`document-signature.v1`) over an
//! explicit set of registry principals.
//!
//! The signed string is the kind's domain followed by the 32 raw digest
//! bytes. Both halves are checked over it and both must verify: Ed25519
//! (RFC 8032, `verify_strict`, so no small-order key or signature component
//! passes) and ML-DSA-65 (FIPS 204, deterministic, empty context string).
//! The keys come only from the principals the host was given: nothing is
//! trusted by default, and a signature's own fields never name a key.
//!
//! Crates: `ed25519-dalek` 3.0.0 and `ml-dsa` 0.1.1 (RustCrypto, Apache-2.0
//! OR MIT) are the ones Metrale/mfs signs and verifies with
//! (`metrale-crypto`), and `sha3` 0.12.0 derives the principal's identity as
//! mfs does. None of them carries a network surface.

use super::{SignatureVerifier, SignerId};
use crate::platform::ids::Sha256Digest;
use crate::platform::signature::{
    DocumentSignature, ED25519_PUBLIC_LEN, HexBytes, ML_DSA_65_PUBLIC_LEN, SignedKind,
};
use ml_dsa::{EncodedVerifyingKey, MlDsa65};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};

/// The domain of a principal's identity derivation.
const NODE_ID_DOMAIN: &[u8] = b"mfs/node-id/v1";

/// A principal's two public signing keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SigningKeys {
    /// Ed25519.
    pub ed25519: HexBytes<ED25519_PUBLIC_LEN>,
    /// ML-DSA-65.
    pub ml_dsa_65: HexBytes<ML_DSA_65_PUBLIC_LEN>,
}

impl SigningKeys {
    /// The identity these keys belong to:
    /// `SHA3-256("mfs/node-id/v1" ‖ Ed25519 key ‖ ML-DSA-65 key)`.
    #[must_use]
    pub fn node_id(&self) -> SignerId {
        let digest = Sha3_256::new()
            .chain_update(NODE_ID_DOMAIN)
            .chain_update(self.ed25519.as_bytes())
            .chain_update(self.ml_dsa_65.as_bytes())
            .finalize();
        let mut id = [0u8; 32];
        id.copy_from_slice(&digest);
        SignerId::from_bytes(id).unwrap_or_else(|_| unreachable!("a signer id may be any bytes"))
    }
}

/// A registry principal (a Roster entry with role `registry`), as the host
/// is given it: the same shape as the bundle's `v1/vectors/release-signer.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryPrincipal {
    /// Its identity, which must be the one its keys derive.
    pub node_id: SignerId,
    /// Its keys.
    pub signing: SigningKeys,
}

/// One trusted principal, keys decoded.
struct Trusted {
    id: SignerId,
    ed25519: ed25519_dalek::VerifyingKey,
    ml_dsa_65: ml_dsa::VerifyingKey<MlDsa65>,
}

/// The release signers this host trusts, and nothing else.
pub struct RegistryPrincipals(Vec<Trusted>);

impl std::fmt::Debug for RegistryPrincipals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter().map(|t| t.id)).finish()
    }
}

impl RegistryPrincipals {
    /// Trust exactly `principals`.
    ///
    /// # Errors
    /// When none is given (there is no default anchor), one is listed twice,
    /// one's identity is not the one its keys derive, or its Ed25519 key is
    /// not a curve point.
    pub fn new(principals: &[RegistryPrincipal]) -> Result<Self, String> {
        if principals.is_empty() {
            return Err("no registry principal is trusted: name at least one".to_owned());
        }
        let mut trusted: Vec<Trusted> = Vec::with_capacity(principals.len());
        for p in principals {
            let derived = p.signing.node_id();
            if derived != p.node_id {
                return Err(format!(
                    "registry principal {} has keys of {derived}",
                    p.node_id
                ));
            }
            if trusted.iter().any(|t| t.id == p.node_id) {
                return Err(format!("registry principal {} is listed twice", p.node_id));
            }
            let ed25519 = ed25519_dalek::VerifyingKey::from_bytes(p.signing.ed25519.as_bytes())
                .map_err(|e| format!("registry principal {}: Ed25519 key: {e}", p.node_id))?;
            let encoded =
                EncodedVerifyingKey::<MlDsa65>::try_from(p.signing.ml_dsa_65.as_bytes().as_slice())
                    .map_err(|_| format!("registry principal {}: ML-DSA-65 key", p.node_id))?;
            trusted.push(Trusted {
                id: p.node_id,
                ed25519,
                ml_dsa_65: ml_dsa::VerifyingKey::<MlDsa65>::decode(&encoded),
            });
        }
        Ok(Self(trusted))
    }
}

/// `domain ‖ digest`, the string both halves sign.
#[must_use]
pub fn signed_string(kind: SignedKind, document: &Sha256Digest) -> Vec<u8> {
    [kind.domain(), document.as_bytes().as_slice()].concat()
}

impl SignatureVerifier for RegistryPrincipals {
    fn verify(
        &self,
        kind: SignedKind,
        document: &Sha256Digest,
        signature: &DocumentSignature,
    ) -> Result<SignerId, String> {
        if signature.kind != kind {
            return Err(format!(
                "the signature is over a {:?}, not a {kind:?}",
                signature.kind
            ));
        }
        if signature.document != *document {
            return Err(format!(
                "the signature is over {}, not {document}",
                signature.document
            ));
        }
        let key = self
            .0
            .iter()
            .find(|t| t.id == signature.signer)
            .ok_or_else(|| format!("{} is not a trusted registry principal", signature.signer))?;
        let signed = signed_string(kind, document);
        let ed = ed25519_dalek::Signature::from_bytes(signature.signature.ed25519.as_bytes());
        key.ed25519
            .verify_strict(&signed, &ed)
            .map_err(|_| "the Ed25519 half does not verify".to_owned())?;
        let ml = ml_dsa::Signature::<MlDsa65>::try_from(
            signature.signature.ml_dsa_65.as_bytes().as_slice(),
        )
        .map_err(|_| "the ML-DSA-65 half is not an encoded signature".to_owned())?;
        if !key.ml_dsa_65.verify_with_context(&signed, &[], &ml) {
            return Err("the ML-DSA-65 half does not verify".to_owned());
        }
        Ok(signature.signer)
    }
}

#[cfg(test)]
#[path = "signing_tests.rs"]
mod tests;
