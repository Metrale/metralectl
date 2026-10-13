// SPDX-License-Identifier: MIT OR Apache-2.0

//! A release signer with real keys, and releases it signs, for tests.
//!
//! The keys come from fixed seeds and the signatures are the real scheme
//! (Ed25519 and ML-DSA-65 over `domain ‖ digest`), so every test of the gate
//! runs the production verifier, [`RegistryPrincipals`]. Signing lives here
//! because an agent only ever verifies.

use super::signing::{RegistryPrincipal, RegistryPrincipals, SigningKeys, signed_string};
use super::{Revoked, SignerId, ThisMachine, VerifiedRelease, verify};
use crate::platform::canonical::digest;
use crate::platform::ids::Sha256Digest;
use crate::platform::recipe::ModelRecipe;
use crate::platform::release::ReleaseManifest;
use crate::platform::scalars::{ModelRepository, V1};
use crate::platform::signature::{
    DocumentSignature, HexBytes, HybridSignature, ML_DSA_65_PUBLIC_LEN, ML_DSA_65_SIGNATURE_LEN,
    SignedKind,
};
use ed25519_dalek::Signer as _;
use ml_dsa::MlDsa65;
use ml_dsa::signature::{Keypair as _, MultipartSigner as _};

/// A signer from two 32-byte seeds.
pub(crate) struct TestSigner {
    ed25519: ed25519_dalek::SigningKey,
    ml_dsa_65: ml_dsa::SigningKey<MlDsa65>,
    principal: RegistryPrincipal,
}

impl TestSigner {
    pub(crate) fn from_seed(seed: u8) -> Self {
        let ed25519 = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        let ml_dsa_65 = ml_dsa::SigningKey::<MlDsa65>::from_seed(&ml_dsa::B32::from(
            [seed.wrapping_add(1); 32],
        ));
        let mut ml_public = [0u8; ML_DSA_65_PUBLIC_LEN];
        ml_public.copy_from_slice(ml_dsa_65.verifying_key().encode().as_slice());
        let signing = SigningKeys {
            ed25519: HexBytes::new(ed25519.verifying_key().to_bytes()),
            ml_dsa_65: HexBytes::new(ml_public),
        };
        let principal = RegistryPrincipal {
            node_id: signing.node_id(),
            signing,
        };
        Self {
            ed25519,
            ml_dsa_65,
            principal,
        }
    }

    pub(crate) fn id(&self) -> SignerId {
        self.principal.node_id
    }

    pub(crate) fn principal(&self) -> RegistryPrincipal {
        self.principal.clone()
    }

    /// Trust exactly this signer.
    pub(crate) fn anchor(&self) -> RegistryPrincipals {
        RegistryPrincipals::new(&[self.principal()]).expect("a valid principal")
    }

    /// Sign `document` as a `kind`.
    pub(crate) fn sign(&self, kind: SignedKind, document: &Sha256Digest) -> DocumentSignature {
        let signed = signed_string(kind, document);
        let mut ml = [0u8; ML_DSA_65_SIGNATURE_LEN];
        ml.copy_from_slice(
            self.ml_dsa_65
                .multipart_sign(&[signed.as_slice()])
                .encode()
                .as_slice(),
        );
        DocumentSignature {
            version: V1,
            kind,
            document: *document,
            signer: self.id(),
            signature: HybridSignature {
                ed25519: HexBytes::new(self.ed25519.sign(&signed).to_bytes()),
                ml_dsa_65: HexBytes::new(ml),
            },
        }
    }
}

/// The signer every release test trusts.
pub(crate) fn signer() -> TestSigner {
    TestSigner::from_seed(7)
}

/// Parse a vendored bundle fixture, embedded at compile time
/// (`include_bytes!`, so no test reads the filesystem at run time and the
/// path is fixed by the compiler; `../../..` is release/testing.rs to the
/// workspace root, then `schemas/platform/v1`).
macro_rules! fixture {
    ($ty:ty, $rel:literal) => {
        serde_json::from_slice::<$ty>(include_bytes!(concat!(
            "../../../../schemas/platform/v1/",
            $rel
        )))
        .expect("valid")
    };
}

pub(crate) use fixture;

/// The bundle's valid `gpt-oss-20b` recipe, as a release ships it.
pub(crate) fn recipe() -> ModelRecipe {
    fixture!(
        ModelRecipe,
        "fixtures/model-recipe/valid/gpt-oss-20b-mxfp4.json"
    )
}

/// The bundle's valid release manifest, bound to [`recipe`]. The bundle's
/// digests are synthetic, so the two fixtures do not name each other as they
/// are; this sets the manifest's recipe and weights digests to the recipe's.
pub(crate) fn manifest() -> ReleaseManifest {
    bind(
        fixture!(
            ReleaseManifest,
            "fixtures/release-manifest/valid/gpt-oss-20b-gb10.json"
        ),
        &recipe(),
    )
}

/// `m` with its recipe and weights digests taken from `r`.
pub(crate) fn bind(mut m: ReleaseManifest, r: &ModelRecipe) -> ReleaseManifest {
    m.recipe = digest(r).expect("integers");
    m.model.weights = digest(&r.model.weights).expect("integers");
    m
}

/// The fingerprint digest the bundle's manifest was qualified on.
pub(crate) fn qualified_fingerprint() -> Sha256Digest {
    manifest().hardware.fingerprints[0]
}

/// The bundle's manifest, serving `model`, verified on a GB10.
pub(crate) fn verified_serving(model: &str) -> VerifiedRelease {
    let mut r = recipe();
    r.model.source.repository = ModelRepository::try_from(model.to_owned()).expect("owner/name");
    let mut m = bind(manifest(), &r);
    m.model.source.repository = r.model.source.repository.clone();
    let d = digest(&m).expect("integers");
    let fp = qualified_fingerprint();
    let s = signer();
    verify(
        &m,
        &r,
        &d,
        &s.sign(SignedKind::ReleaseManifest, &d),
        &s.anchor(),
        &Revoked::default(),
        ThisMachine {
            accelerator: "NVIDIA GB10",
            fingerprint: Some(&fp),
            recipe_model: model,
        },
    )
    .expect("verifies")
}
