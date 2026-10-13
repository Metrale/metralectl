// SPDX-License-Identifier: MIT OR Apache-2.0

//! Each refusal, from the bundle's own valid manifest with one thing changed.

use super::signing::{RegistryPrincipal, RegistryPrincipals};
use super::testing::{TestSigner, manifest, qualified_fingerprint, recipe, signer};
use super::*;
use crate::platform::canonical::sha256;

const GB10: &str = "NVIDIA GB10";
const MODEL: &str = "openai/gpt-oss-20b";

struct Case {
    manifest: ReleaseManifest,
    recipe: ModelRecipe,
    expected: Sha256Digest,
    signature: DocumentSignature,
    trusted: Vec<RegistryPrincipal>,
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
            signature: signer().sign(SignedKind::ReleaseManifest, &d),
            manifest: m,
            recipe: recipe(),
            expected: d,
            trusted: vec![signer().principal()],
            revoked: Revoked::default(),
            accelerator: GB10,
            fingerprint: Some(qualified_fingerprint()),
            recipe_model: MODEL,
        }
    }

    fn run(&self) -> Result<VerifiedRelease, Unverified> {
        verify(
            &self.manifest,
            &self.recipe,
            &self.expected,
            &self.signature,
            &RegistryPrincipals::new(&self.trusted).expect("an anchor"),
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
    assert_eq!(v.signer(), &signer().id());
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
    c.revoked.signers.insert(signer().id());
    assert_eq!(c.run(), Err(Unverified::SignerRevoked(signer().id())));
    // One receipt left unrevoked still qualifies it; none left does not.
    let mut c = Case::good();
    c.revoked.receipts.insert(sha256(b"another receipt"));
    assert!(c.run().is_ok());
    let mut c = Case::good();
    c.revoked
        .receipts
        .extend(c.manifest.receipts.iter().copied());
    assert_eq!(c.run(), Err(Unverified::ReceiptsRevoked));
}

/// Sabotage: accepting any signature from a trusted signer verifies one made
/// over another manifest.
#[test]
fn an_unsigned_untrusted_or_misdirected_signature_is_refused() {
    let mut c = Case::good();
    c.trusted = vec![TestSigner::from_seed(8).principal()];
    assert!(matches!(c.run(), Err(Unverified::Unsigned(_))));
    let mut c = Case::good();
    c.signature = signer().sign(SignedKind::ReleaseManifest, &sha256(b"another manifest"));
    assert!(matches!(c.run(), Err(Unverified::Unsigned(_))));
    // The right digest under the other kind's domain is not a release
    // signature, however the kind field is relabelled.
    let mut c = Case::good();
    c.signature = signer().sign(SignedKind::RevocationList, &c.expected);
    c.signature.kind = SignedKind::ReleaseManifest;
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

/// The recipe shipped as `recipe.json` must be the one the manifest's digest
/// names, and agree with it (DECISIONS PC-W). Sabotage: skipping the digest
/// comparison accepts a recipe with other serve flags under the same
/// manifest; skipping a field comparison accepts the matching edit below.
#[test]
fn a_recipe_the_manifest_does_not_bind_is_refused() {
    let mut c = Case::good();
    c.recipe.limits.max_context_tokens = std::num::NonZeroU32::MIN;
    assert_eq!(c.run(), Err(Unverified::Recipe { at: "/recipe" }));

    // Re-bound by digest but disagreeing on what both name: each is caught
    // by its own comparison, and the manifest stays signed.
    type Edit = (&'static str, fn(&mut ModelRecipe));
    let edits: [Edit; 4] = [
        ("/model/source", |r| {
            r.model.source.revision =
                crate::platform::ids::GitCommit::parse(&"ab".repeat(20)).expect("a commit");
        }),
        ("/model/weights", |r| r.model.weights[0].bytes += 1),
        ("/runtime/image", |r| {
            r.runtime.image.digest =
                serde_json::from_value(serde_json::json!(format!("sha256:{}", "c".repeat(64))))
                    .expect("a digest");
        }),
        ("/hardware/hardware_class", |r| {
            r.runtime.hardware_classes =
                vec![metralectl_core::hardware::HardwareClass::parse("strix-halo").expect("class")];
        }),
    ];
    for (at, edit) in edits {
        let mut c = Case::good();
        edit(&mut c.recipe);
        c.manifest.recipe = digest(&c.recipe).expect("integers");
        c.expected = digest(&c.manifest).expect("integers");
        c.signature = signer().sign(SignedKind::ReleaseManifest, &c.expected);
        assert_eq!(c.run(), Err(Unverified::Recipe { at }), "{at}");
    }
}
