// SPDX-License-Identifier: MIT OR Apache-2.0

//! `release-manifest.v1`: one accepted release, bound by content.
//!
//! Its identity is the SHA-256 of its canonical JSON, which is what a
//! deployment's `spec.release` names. Nothing in it is a mutable name: the
//! image is a repository and a digest, the model a full commit.

use super::ids::{GitCommit, Sha256Digest};
use super::scalars::{ArtifactRef, DnsLabel, ModelRevision, OciImage, UnixMs, V1, non_empty};
use metralectl_core::hardware::HardwareClass;
use serde::{Deserialize, Serialize};

/// The runtime of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRuntime {
    /// The commit it was built from.
    pub git_commit: GitCommit,
    /// The image, by digest.
    pub image: OciImage,
}

/// The model of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseModel {
    /// The repository at a commit.
    pub source: ModelRevision,
    /// The digest of the recipe's weights list.
    pub weights: Sha256Digest,
}

/// The hardware a release was qualified on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseHardware {
    /// The class.
    pub hardware_class: HardwareClass,
    /// Fingerprint digests a passing receipt covers.
    #[serde(deserialize_with = "non_empty")]
    pub fingerprints: Vec<Sha256Digest>,
}

/// SBOM formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SbomFormat {
    /// SPDX JSON.
    SpdxJson,
    /// CycloneDX JSON.
    CyclonedxJson,
}

/// The release's software bill of materials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sbom {
    /// Its format.
    pub format: SbomFormat,
    /// The file.
    pub artifact: ArtifactRef,
}

/// One accepted release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    /// Always 1.
    pub version: V1,
    /// The release's name.
    pub release: DnsLabel,
    /// When it was made.
    pub created_at_unix_ms: UnixMs,
    /// The runtime.
    pub runtime: ReleaseRuntime,
    /// The digest of the recipe document.
    pub recipe: Sha256Digest,
    /// The model.
    pub model: ReleaseModel,
    /// The hardware.
    pub hardware: ReleaseHardware,
    /// The SBOM.
    pub sbom: Sbom,
    /// The compatibility receipts, by digest.
    #[serde(deserialize_with = "non_empty")]
    pub receipts: Vec<Sha256Digest>,
}
