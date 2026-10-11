// SPDX-License-Identifier: MIT OR Apache-2.0

//! A release manifest verified before anything of it is launched (#51).
//!
//! [`verify`] is the one way to hold a [`VerifiedRelease`], and a launch for a
//! deployment carries one: the launcher then runs the manifest's image by its
//! digest, never by a tag, and the control core refuses a recipe that serves
//! another model than the release names. Verification binds the manifest by
//! content and refuses, each for its own reason:
//!
//! - **changed**: its canonical digest is not the one the deployment names;
//! - **revoked**: the manifest, or the key that signed it, is withdrawn, or
//!   every compatibility receipt it cites is;
//! - **unsigned**: no trusted signer's signature over that digest;
//! - **unbound**: the recipe shipped with it (`recipe.json`, a
//!   `model-recipe` document, DECISIONS PC-W) is not the one its `recipe`
//!   digest names, or disagrees with it on the model, the weights, the image
//!   or the hardware class;
//! - **mismatched**: it was qualified for another hardware class, not for this
//!   machine's fingerprint, or for another model than this machine's recipe
//!   serves.
//!
//! The signature is a `document-signature` (the platform's hybrid scheme);
//! [`SignatureVerifier`] checks it against the host's trust anchor.
//! [`signing::RegistryPrincipals`] is the scheme itself, over registry
//! principals the host names explicitly, and [`revocations`] builds
//! [`Revoked`] from a revocation list only once its own signature verifies.

use crate::platform::canonical::digest;
use crate::platform::ids::Sha256Digest;
pub use crate::platform::ids::SignerId;
use crate::platform::recipe::ModelRecipe;
use crate::platform::release::ReleaseManifest;
use crate::platform::scalars::{ModelRepository, OciImage};
use crate::platform::signature::{DocumentSignature, SignedKind};
use metralectl_core::hardware::{HardwareClass, classify};
use std::collections::BTreeSet;

pub mod revocations;
pub mod signing;

/// Checks a `document-signature` against the trust anchors the host holds.
pub trait SignatureVerifier: Send + Sync {
    /// The signer, when `signature` is a trusted signer's valid signature of
    /// a `kind` document over exactly `document`.
    ///
    /// # Errors
    /// Why it is not: another kind or document, an untrusted signer, or a
    /// half that does not verify.
    fn verify(
        &self,
        kind: SignedKind,
        document: &Sha256Digest,
        signature: &DocumentSignature,
    ) -> Result<SignerId, String>;
}

/// What the platform has withdrawn, as the host last learned it from a
/// verified revocation list ([`revocations::accept`]); empty when no list
/// has been accepted yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Revoked {
    /// Release manifests, by digest.
    pub manifests: BTreeSet<Sha256Digest>,
    /// Compatibility receipts, by digest.
    pub receipts: BTreeSet<Sha256Digest>,
    /// Release signers.
    pub signers: BTreeSet<SignerId>,
}

/// What this machine is, for the release to be matched against.
#[derive(Debug, Clone, Copy)]
pub struct ThisMachine<'a> {
    /// What its accelerator calls itself.
    pub accelerator: &'a str,
    /// The digest of its hardware fingerprint; `None` when it could not be
    /// fully measured, which no release can then be shown to cover.
    pub fingerprint: Option<&'a Sha256Digest>,
    /// The model this machine's recipe for the release serves.
    pub recipe_model: &'a str,
}

/// Why a manifest was not verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unverified {
    /// Its digest is not the one the deployment names.
    Changed {
        /// What the deployment names.
        expected: Sha256Digest,
        /// What this manifest is.
        actual: Sha256Digest,
    },
    /// The manifest is withdrawn.
    ManifestRevoked,
    /// The key that signed it is withdrawn.
    SignerRevoked(SignerId),
    /// Every compatibility receipt it cites is withdrawn, so nothing
    /// qualifies it any more.
    ReceiptsRevoked,
    /// No trusted signature over its digest.
    Unsigned(String),
    /// The shipped recipe is not the one the manifest binds, or disagrees
    /// with it at this manifest pointer.
    Recipe {
        /// Where they disagree.
        at: &'static str,
    },
    /// It was qualified for another class of hardware.
    HardwareClass {
        /// The release's class.
        release: HardwareClass,
        /// This machine's, when it has one this build knows.
        here: Option<HardwareClass>,
    },
    /// This machine's fingerprint is not one the release was qualified on.
    FingerprintNotQualified,
    /// The release serves another model than this machine's recipe.
    Model {
        /// The release's model.
        release: String,
        /// This machine's recipe's model.
        recipe: String,
    },
}

impl std::fmt::Display for Unverified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Changed { expected, actual } => write!(
                f,
                "the release manifest is {actual}, not the {expected} the deployment names"
            ),
            Self::ManifestRevoked => f.write_str("the release manifest is revoked"),
            Self::SignerRevoked(s) => write!(f, "the release was signed by revoked signer {s}"),
            Self::ReceiptsRevoked => {
                f.write_str("every compatibility receipt the release cites is revoked")
            }
            Self::Unsigned(why) => write!(f, "the release manifest is not signed: {why}"),
            Self::Recipe { at } => write!(
                f,
                "the recipe shipped with the release does not match the manifest at {at}"
            ),
            Self::HardwareClass { release, here } => match here {
                Some(here) => write!(f, "the release is for {release} hardware; this is {here}"),
                None => write!(
                    f,
                    "the release is for {release} hardware; this machine's class is unknown"
                ),
            },
            Self::FingerprintNotQualified => {
                f.write_str("the release was not qualified on this machine's hardware fingerprint")
            }
            Self::Model { release, recipe } => write!(
                f,
                "the release serves {release}; this machine's recipe serves {recipe}"
            ),
        }
    }
}

/// A manifest that passed [`verify`]. Its fields can be read but not built
/// any other way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRelease {
    digest: Sha256Digest,
    image: OciImage,
    model: ModelRepository,
    signer: SignerId,
}

impl VerifiedRelease {
    /// The manifest's content digest.
    #[must_use]
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    /// The runtime image, by digest.
    #[must_use]
    pub fn image(&self) -> &OciImage {
        &self.image
    }

    /// The model it serves.
    #[must_use]
    pub fn model(&self) -> &str {
        self.model.as_str()
    }

    /// Who signed it.
    #[must_use]
    pub fn signer(&self) -> &SignerId {
        &self.signer
    }
}

/// Verify `manifest` as the release a deployment names by `expected`.
///
/// # Errors
/// The first check it fails, in the order of the module documentation.
pub fn verify(
    manifest: &ReleaseManifest,
    recipe: &ModelRecipe,
    expected: &Sha256Digest,
    signature: &DocumentSignature,
    verifier: &dyn SignatureVerifier,
    revoked: &Revoked,
    here: ThisMachine<'_>,
) -> Result<VerifiedRelease, Unverified> {
    let actual = digest(manifest).map_err(Unverified::Unsigned)?;
    if actual != *expected {
        return Err(Unverified::Changed {
            expected: *expected,
            actual,
        });
    }
    if revoked.manifests.contains(&actual) {
        return Err(Unverified::ManifestRevoked);
    }
    let signer = verifier
        .verify(SignedKind::ReleaseManifest, &actual, signature)
        .map_err(Unverified::Unsigned)?;
    if revoked.signers.contains(&signer) {
        return Err(Unverified::SignerRevoked(signer));
    }
    if manifest
        .receipts
        .iter()
        .all(|r| revoked.receipts.contains(r))
    {
        return Err(Unverified::ReceiptsRevoked);
    }
    bound(manifest, recipe)?;
    let class = classify(here.accelerator);
    if class.as_ref() != Some(&manifest.hardware.hardware_class) {
        return Err(Unverified::HardwareClass {
            release: manifest.hardware.hardware_class.clone(),
            here: class,
        });
    }
    if !here
        .fingerprint
        .is_some_and(|fp| manifest.hardware.fingerprints.contains(fp))
    {
        return Err(Unverified::FingerprintNotQualified);
    }
    let model = &manifest.model.source.repository;
    if model.as_str() != here.recipe_model {
        return Err(Unverified::Model {
            release: model.as_str().to_owned(),
            recipe: here.recipe_model.to_owned(),
        });
    }
    Ok(VerifiedRelease {
        digest: actual,
        image: manifest.runtime.image.clone(),
        model: model.clone(),
        signer,
    })
}

/// Whether `recipe` is the recipe `manifest` binds, by digest and by what
/// both say (the same checks the platform's offline verifier makes).
fn bound(manifest: &ReleaseManifest, recipe: &ModelRecipe) -> Result<(), Unverified> {
    let disagree = |at| Err(Unverified::Recipe { at });
    if digest(recipe).ok() != Some(manifest.recipe) {
        return disagree("/recipe");
    }
    if recipe.model.source != manifest.model.source {
        return disagree("/model/source");
    }
    if digest(&recipe.model.weights).ok() != Some(manifest.model.weights) {
        return disagree("/model/weights");
    }
    if recipe.runtime.image != manifest.runtime.image {
        return disagree("/runtime/image");
    }
    if !recipe
        .runtime
        .hardware_classes
        .contains(&manifest.hardware.hardware_class)
    {
        return disagree("/hardware/hardware_class");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
