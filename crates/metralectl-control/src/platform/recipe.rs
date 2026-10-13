// SPDX-License-Identifier: MIT OR Apache-2.0

//! `model-recipe.v1`: one served model, fully pinned.
//!
//! The recipe a release binds (DECISIONS PC-W): `ReleaseManifest.recipe` is
//! the canonical-JSON SHA-256 of this document, shipped beside the manifest
//! as `recipe.json`. An agent hashes the document it received, never its own
//! engine configuration.

use super::ids::Sha256Digest;
use super::scalars::{
    ArtifactRef, ModelRevision, OciImage, Text, V1, checked_string, non_empty, word,
};
use metralectl_core::hardware::HardwareClass;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::num::NonZeroU32;

fn lower_digit(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

fn recipe_segment(s: &str) -> bool {
    word(
        s,
        lower_digit,
        |c| lower_digit(c) || matches!(c, b'.' | b'_' | b'-'),
        63,
    )
}

checked_string!(
    /// A recipe id: `family/stem` or `stem`, lower case.
    RecipeId,
    "a recipe id",
    |s| match s.split_once('/') {
        Some((family, stem)) => recipe_segment(family) && recipe_segment(stem),
        None => recipe_segment(s),
    }
);

checked_string!(
    /// A serve flag's key, `snake_case`.
    FlagKey,
    "a flag key",
    |s| word(
        s,
        |c| c.is_ascii_lowercase(),
        |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_',
        63
    )
);

checked_string!(
    /// An environment variable's name, `UPPER_CASE`.
    EnvKey,
    "an environment key",
    |s| word(
        s,
        |c| c.is_ascii_uppercase(),
        |c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_',
        127
    )
);

checked_string!(
    /// A flag or environment value: up to 1024 characters.
    Value,
    "a value of at most 1024 characters",
    |s| s.chars().count() <= 1024
);

/// One weights file, by the SHA-256 of its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeightFile {
    /// Its path in the checkpoint.
    pub path: Text,
    /// What it is.
    pub sha256: Sha256Digest,
    /// Its size.
    pub bytes: u64,
}

/// The checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeModel {
    /// The repository at a commit.
    pub source: ModelRevision,
    /// Every weights file.
    #[serde(deserialize_with = "non_empty")]
    pub weights: Vec<WeightFile>,
}

/// The architecture and its text-side artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Architecture {
    /// The model family.
    pub family: Text,
    /// The tokenizer.
    pub tokenizer: ArtifactRef,
    /// The chat template.
    pub chat_template: ArtifactRef,
}

/// A precision for stored values: never adaptive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoragePrecision {
    /// bfloat16.
    Bf16,
    /// float16.
    Fp16,
    /// FP8.
    Fp8,
    /// NVFP4.
    Nvfp4,
    /// MXFP4.
    Mxfp4,
    /// int8.
    Int8,
    /// int4.
    Int4,
}

/// A compute precision: a stored one, or chosen at run time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Precision {
    /// bfloat16.
    Bf16,
    /// float16.
    Fp16,
    /// FP8.
    Fp8,
    /// NVFP4.
    Nvfp4,
    /// MXFP4.
    Mxfp4,
    /// int8.
    Int8,
    /// int4.
    Int4,
    /// Chosen by the runtime.
    Adaptive,
}

/// The formats of weights, activations and KV cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quantization {
    /// Weights.
    pub weights: StoragePrecision,
    /// Activations.
    pub activations: Precision,
    /// KV cache.
    pub kv_cache: StoragePrecision,
}

/// GPU memory, permille: 1 to 1000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Permille(u16);

impl TryFrom<u16> for Permille {
    type Error = String;
    fn try_from(v: u16) -> Result<Self, String> {
        if (1..=1000).contains(&v) {
            Ok(Self(v))
        } else {
            Err(format!("{v} is not a permille of 1 to 1000"))
        }
    }
}

impl From<Permille> for u16 {
    fn from(p: Permille) -> Self {
        p.0
    }
}

/// Serving limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// The longest context.
    pub max_context_tokens: NonZeroU32,
    /// The most sequences at once.
    pub max_concurrent_sequences: NonZeroU32,
    /// GPU memory it may take.
    pub gpu_memory_permille: Permille,
}

/// Serving engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// The Metrale runtime.
    Metrale,
    /// A stand-in engine.
    Sim,
    /// vLLM.
    Vllm,
    /// SGLang.
    Sglang,
}

/// How it is launched: the block an agent derives a launch from (PC-P).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeRuntime {
    /// The engine.
    pub engine: Engine,
    /// The image, by digest.
    pub image: OciImage,
    /// The classes it runs on.
    #[serde(deserialize_with = "non_empty")]
    pub hardware_classes: Vec<HardwareClass>,
    /// The fewest nodes it needs.
    pub min_nodes: NonZeroU32,
    /// The engine's serve flags.
    pub serve_flags: BTreeMap<FlagKey, Value>,
    /// The engine's environment.
    pub env: BTreeMap<EnvKey, Value>,
}

/// A parser a supported capability is served through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Supported {
    /// The parser.
    pub parser: Text,
}

/// Whether a capability is served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Not served.
    Unsupported,
    /// Served, through this parser.
    Supported(Supported),
}

/// Modalities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// Text.
    Text,
    /// Images.
    Image,
    /// Audio.
    Audio,
    /// Video.
    Video,
}

/// What it serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Semantics {
    /// Tool calls.
    pub tool_calls: Capability,
    /// Structured output.
    pub structured_output: Capability,
    /// Reasoning.
    pub reasoning: Capability,
    /// What it reads.
    #[serde(deserialize_with = "non_empty")]
    pub input_modalities: Vec<Modality>,
    /// What it writes.
    #[serde(deserialize_with = "non_empty")]
    pub output_modalities: Vec<Modality>,
}

/// Kinds of evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// A benchmark run.
    Benchmark,
    /// A coherence check.
    Coherence,
    /// A certification.
    Certification,
}

/// One piece of evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    /// What it is.
    pub kind: EvidenceKind,
    /// The file.
    pub artifact: ArtifactRef,
}

/// One served model, fully pinned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRecipe {
    /// Always 1.
    pub version: V1,
    /// Its id.
    pub recipe_id: RecipeId,
    /// The checkpoint.
    pub model: RecipeModel,
    /// The architecture.
    pub architecture: Architecture,
    /// The formats.
    pub quantization: Quantization,
    /// The limits.
    pub limits: Limits,
    /// How it is launched.
    pub runtime: RecipeRuntime,
    /// What it serves.
    pub semantics: Semantics,
    /// The evidence behind it.
    pub evidence: Vec<Evidence>,
}
