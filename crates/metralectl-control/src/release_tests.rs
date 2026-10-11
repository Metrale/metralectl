// SPDX-License-Identifier: MIT OR Apache-2.0

//! Each refusal, from the bundle's own valid manifest with one thing changed.

use super::testing::{TestVerifier, manifest, qualified_fingerprint, sign, signer};
use super::*;
use crate::platform::canonical::sha256;

const GB10: &str = "NVIDIA GB10";
const MODEL: &str = "openai/gpt-oss-20b";

struct Case {
    manifest: ReleaseManifest,
    expected: Sha256Digest,
    signature: Vec<u8>,
    trusted: Vec<SignerId>,
    revoked: Revoked,
    accelerator: &'static str,
    fingerprint: Option<Sha256Digest>,
    recipe_model: &'static str,
}

impl Case {
    /// Everything right: the bundle manifest, signed, on a qualified GB10.
    fn good() -> Self {
        let m = manifest();
        let d = digest(&m).expect("integers");
        Self {
            signature: sign(&signer(), &d),
            manifest: m,
            expected: d,
            trusted: vec![signer()],
            revoked: Revoked::default(),
            accelerator: GB10,
            fingerprint: Some(qualified_fingerprint()),
            recipe_model: MODEL,
        }
    }

    fn run(&self) -> Result<VerifiedRelease, Unverified> {
        verify(
            &self.manifest,
            &self.expected,
            &self.signature,
            &TestVerifier(self.trusted.clone()),
            &self.revoked,
            ThisMachine {
                accelerator: self.accelerator,
                fingerprint: self.fingerprint.as_ref(),
                recipe_model: self.recipe_model,
            },
        )
    }
}

#[test]
fn a_signed_qualified_manifest_verifies_and_pins_its_image_by_digest() {
    let v = Case::good().run().expect("verifies");
    assert_eq!(
        v.image().reference(),
        "metrale/metrale-inference-gb10@sha256:\
         6105d6cc76af400325e94d588ce511be5bfdbb73b437dc51eca43917d7a43e3d"
    );
    assert_eq!(v.model(), MODEL);
    assert_eq!(v.signer(), &signer());
    assert_eq!(v.digest(), &Case::good().expected);
}

/// Sabotage: skipping the digest comparison verifies an edited manifest
/// against the signature of the one the deployment names.
#[test]
fn a_changed_manifest_is_refused() {
    let mut c = Case::good();
    c.manifest.created_at_unix_ms = crate::platform::scalars::UnixMs::new(1).expect("in range");
    assert!(matches!(c.run(), Err(Unverified::Changed { .. })));
}

#[test]
fn a_revoked_manifest_or_signer_is_refused() {
    let mut c = Case::good();
    c.revoked.manifests.insert(c.expected);
    assert_eq!(c.run(), Err(Unverified::ManifestRevoked));
    let mut c = Case::good();
    c.revoked.signers.insert(signer());
    assert_eq!(c.run(), Err(Unverified::SignerRevoked(signer())));
}

/// Sabotage: accepting any signature from a trusted signer verifies one made
/// over another manifest.
#[test]
fn an_unsigned_untrusted_or_misdirected_signature_is_refused() {
    let mut c = Case::good();
    c.signature = Vec::new();
    assert!(matches!(c.run(), Err(Unverified::Unsigned(_))));
    let mut c = Case::good();
    c.trusted = vec![SignerId::from_bytes([8; 32]).expect("bytes")];
    assert!(matches!(c.run(), Err(Unverified::Unsigned(_))));
    let mut c = Case::good();
    c.signature = sign(&signer(), &sha256(b"another manifest"));
    assert!(matches!(c.run(), Err(Unverified::Unsigned(_))));
}

#[test]
fn another_class_an_unqualified_or_unmeasured_fingerprint_or_model_is_refused() {
    let mut c = Case::good();
    c.accelerator = "NVIDIA H100 80GB HBM3";
    assert!(matches!(
        c.run(),
        Err(Unverified::HardwareClass { here: None, .. })
    ));
    let mut c = Case::good();
    c.fingerprint = Some(sha256(b"another machine"));
    assert_eq!(c.run(), Err(Unverified::FingerprintNotQualified));
    let mut c = Case::good();
    c.fingerprint = None;
    assert_eq!(c.run(), Err(Unverified::FingerprintNotQualified));
    let mut c = Case::good();
    c.recipe_model = "Qwen/Qwen3.6-27B-FP8";
    assert!(matches!(c.run(), Err(Unverified::Model { .. })));
}
