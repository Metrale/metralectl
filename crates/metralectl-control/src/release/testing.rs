// SPDX-License-Identifier: MIT OR Apache-2.0

//! A stand-in signature scheme and a verified release, for tests.
//!
//! Justification for the stand-in: the platform's release-signing scheme is
//! not yet in the vendored bundle, so no real verifier exists to test the
//! gate with. This one binds a signature to one signer and one digest, which
//! is the property the gate relies on; it is a keyed hash, not a signature,
//! and is compiled for tests only.

use super::{Revoked, SignatureVerifier, SignerId, ThisMachine, VerifiedRelease, verify};
use crate::platform::canonical::{digest, sha256};
use crate::platform::ids::Sha256Digest;
use crate::platform::release::ReleaseManifest;
use crate::platform::scalars::ModelRepository;

/// Accepts what [`sign`] produced for one of its signers.
pub(crate) struct TestVerifier(pub(crate) Vec<SignerId>);

fn tag(digest: &Sha256Digest) -> Sha256Digest {
    sha256(&[b"test-release-signature".as_slice(), digest.as_bytes()].concat())
}

/// `signer` "signs" `digest`.
pub(crate) fn sign(signer: &SignerId, digest: &Sha256Digest) -> Vec<u8> {
    [signer.as_bytes().as_slice(), tag(digest).as_bytes()].concat()
}

impl SignatureVerifier for TestVerifier {
    fn verify(&self, manifest: &Sha256Digest, signature: &[u8]) -> Result<SignerId, String> {
        let (who, tagged) = signature.split_at_checked(32).ok_or("short signature")?;
        let who = SignerId::from_bytes(who.try_into().map_err(|_| "short signature")?)?;
        if !self.0.contains(&who) {
            return Err(format!("{who} is not a trusted signer"));
        }
        if tagged != tag(manifest).as_bytes() {
            return Err("the signature is not over this manifest".into());
        }
        Ok(who)
    }
}

pub(crate) fn signer() -> SignerId {
    SignerId::from_bytes([7; 32]).expect("any bytes")
}

/// The bundle's valid release manifest.
pub(crate) fn manifest() -> ReleaseManifest {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas/platform/v1/fixtures/release-manifest/valid/gpt-oss-20b-gb10.json");
    serde_json::from_slice(&std::fs::read(path).expect("the vendored fixture")).expect("valid")
}

/// The fingerprint digest the bundle's manifest was qualified on.
pub(crate) fn qualified_fingerprint() -> Sha256Digest {
    manifest().hardware.fingerprints[0]
}

/// The bundle's manifest, serving `model`, verified on a GB10.
pub(crate) fn verified_serving(model: &str) -> VerifiedRelease {
    let mut m = manifest();
    m.model.source.repository = ModelRepository::try_from(model.to_owned()).expect("owner/name");
    let d = digest(&m).expect("integers");
    let fp = qualified_fingerprint();
    verify(
        &m,
        &d,
        &sign(&signer(), &d),
        &TestVerifier(vec![signer()]),
        &Revoked::default(),
        ThisMachine {
            accelerator: "NVIDIA GB10",
            fingerprint: Some(&fp),
            recipe_model: model,
        },
    )
    .expect("verifies")
}
