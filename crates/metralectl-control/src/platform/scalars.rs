// SPDX-License-Identifier: MIT OR Apache-2.0

//! The bundle's shared definitions (`common.v1.schema.json`) that the bound
//! documents use. Each is a type that refuses, when it is parsed, what the
//! schema refuses, so a document held in one of these types is already valid.

use super::ids::{GitCommit, Sha256Digest};
use metralectl_core::hardware::is_dns_label;
use serde::{Deserialize, Deserializer, Serialize};

/// A required key whose value may be `null`: present, never defaulted.
///
/// serde treats a missing `Option` field as `None`; a field read through
/// this function is refused when missing, as every bundle schema requires.
///
/// # Errors
/// When the key is absent or its value is neither `null` nor a `T`.
pub fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d)
}

/// A list the schema requires at least one item of.
///
/// # Errors
/// When the list is empty, or an item is not a `T`.
pub fn non_empty<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let items = Vec::<T>::deserialize(d)?;
    if items.is_empty() {
        return Err(serde::de::Error::custom("the list needs at least one item"));
    }
    Ok(items)
}

/// A string the schema constrains by a pattern, checked when it is parsed.
macro_rules! checked_string {
    ($(#[$meta:meta])* $name:ident, $what:literal, $check:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// The string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(s: String) -> Result<Self, String> {
                let check: fn(&str) -> bool = $check;
                if check(&s) {
                    Ok(Self(s))
                } else {
                    Err(format!("{s:?} is not {}", $what))
                }
            }
        }

        impl From<$name> for String {
            fn from(v: $name) -> Self {
                v.0
            }
        }
    };
}

/// `[first][rest]{0,max}` with the given byte classes.
pub(crate) fn word(s: &str, first: fn(u8) -> bool, rest: fn(u8) -> bool, max: usize) -> bool {
    match s.as_bytes().split_first() {
        Some((f, r)) => first(*f) && r.len() <= max && r.iter().all(|c| rest(*c)),
        None => false,
    }
}

pub(crate) use checked_string;

checked_string!(
    /// A served model name: the `model` a client sends and the Gateway
    /// routes on (DECISIONS PC-R). A route, never an identity.
    ModelName,
    "a model name (1 to 128 of A-Z a-z 0-9 . _ : / -, starting alphanumeric)",
    |s| word(
        s,
        |c| c.is_ascii_alphanumeric(),
        |c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'/' | b'-'),
        127
    )
);

/// The schema version of every v1 document. A reader refuses any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct V1;

impl TryFrom<u64> for V1 {
    type Error = String;
    fn try_from(v: u64) -> Result<Self, String> {
        if v == 1 {
            Ok(Self)
        } else {
            Err(format!(
                "version {v} is not implemented; this reader reads 1"
            ))
        }
    }
}

impl From<V1> for u64 {
    fn from(_: V1) -> Self {
        1
    }
}

/// Text of 1 to 256 characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Text(String);

impl Text {
    /// Parse text.
    ///
    /// # Errors
    /// When it is empty or longer than 256 characters.
    pub fn parse(s: &str) -> Result<Self, String> {
        let n = s.chars().count();
        if (1..=256).contains(&n) {
            Ok(Self(s.to_owned()))
        } else {
            Err(format!("text is 1 to 256 characters, got {n}"))
        }
    }

    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Text {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Self::parse(&s)
    }
}

impl From<Text> for String {
    fn from(t: Text) -> Self {
        t.0
    }
}

/// A DNS label: a tenant, a project or a release name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DnsLabel(String);

impl DnsLabel {
    /// Parse a label.
    ///
    /// # Errors
    /// When it is not a DNS label.
    pub fn parse(s: &str) -> Result<Self, String> {
        if is_dns_label(s) {
            Ok(Self(s.to_owned()))
        } else {
            Err(format!("{s:?} is not a DNS label"))
        }
    }

    /// The label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DnsLabel {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Self::parse(&s)
    }
}

impl From<DnsLabel> for String {
    fn from(l: DnsLabel) -> Self {
        l.0
    }
}

/// UTC Unix time in milliseconds, never before the epoch and within `i64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct UnixMs(u64);

impl UnixMs {
    /// A timestamp.
    ///
    /// # Errors
    /// When it does not fit a signed 64-bit integer, as the schema bounds it.
    pub fn new(ms: u64) -> Result<Self, String> {
        if i64::try_from(ms).is_ok() {
            Ok(Self(ms))
        } else {
            Err(format!("{ms} ms is past the schema's timestamp range"))
        }
    }

    /// The milliseconds.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<u64> for UnixMs {
    type Error = String;
    fn try_from(ms: u64) -> Result<Self, String> {
        Self::new(ms)
    }
}

impl From<UnixMs> for u64 {
    fn from(t: UnixMs) -> Self {
        t.0
    }
}

/// A tenant and project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    /// The tenant.
    pub tenant: DnsLabel,
    /// The project.
    pub project: DnsLabel,
}

/// A piece of software and its version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoftwareVersion {
    /// What it is (`nvidia`, `cuda`).
    pub name: Text,
    /// Which version.
    pub version: Text,
}

/// Whether `s` is lower-case path segments, with no tag, digest or port.
fn is_image_repository(s: &str) -> bool {
    let segment = |seg: &str| {
        let parts: Vec<&str> = seg.split(['.', '_', '-']).collect();
        !parts.is_empty()
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            })
    };
    s.len() <= 255 && s.split('/').all(segment)
}

/// A container image repository: lower-case path segments, never a tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ImageRepository(String);

impl TryFrom<String> for ImageRepository {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        if is_image_repository(&s) {
            Ok(Self(s))
        } else {
            Err(format!(
                "{s:?} is not an image repository (lower-case path, no tag, digest or port)"
            ))
        }
    }
}

impl From<ImageRepository> for String {
    fn from(r: ImageRepository) -> Self {
        r.0
    }
}

/// An OCI content digest, `sha256:` and 64 lower-case hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OciDigest(String);

impl TryFrom<String> for OciDigest {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        let ok = s.strip_prefix("sha256:").is_some_and(|h| {
            h.len() == 64
                && h.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        });
        if ok {
            Ok(Self(s))
        } else {
            Err(format!(
                "{s:?} is not a sha256: digest; a tag never names an image"
            ))
        }
    }
}

impl From<OciDigest> for String {
    fn from(d: OciDigest) -> Self {
        d.0
    }
}

/// An image named by content: a repository and a digest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciImage {
    /// Where it lives.
    pub repository: ImageRepository,
    /// What it is.
    pub digest: OciDigest,
}

impl OciImage {
    /// The reference a container runtime pulls: `repository@sha256:…`, which
    /// names exactly these bytes and cannot be moved the way a tag can.
    #[must_use]
    pub fn reference(&self) -> String {
        format!("{}@{}", self.repository.0, self.digest.0)
    }
}

/// Whether `s` is a model repository `owner/name`.
fn is_model_repository(s: &str) -> bool {
    let part = |p: &str| {
        let b = p.as_bytes();
        (1..=96).contains(&b.len())
            && b[0].is_ascii_alphanumeric()
            && b.iter()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    };
    matches!(s.split_once('/'), Some((owner, name)) if part(owner) && part(name))
}

/// A model repository, `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModelRepository(String);

impl ModelRepository {
    /// The `owner/name`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ModelRepository {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        if is_model_repository(&s) {
            Ok(Self(s))
        } else {
            Err(format!("{s:?} is not a model repository (owner/name)"))
        }
    }
}

impl From<ModelRepository> for String {
    fn from(r: ModelRepository) -> Self {
        r.0
    }
}

/// A model at a full commit; never a branch or tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRevision {
    /// The repository.
    pub repository: ModelRepository,
    /// The commit.
    pub revision: GitCommit,
}

/// A file named by the SHA-256 of its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    /// Where to find it.
    pub uri: Text,
    /// What it is.
    pub sha256: Sha256Digest,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_other_than_1_is_refused() {
        // No bundle fixture of a kind bound here carries another version.
        assert!(serde_json::from_str::<V1>("1").is_ok());
        for other in ["0", "2"] {
            assert!(serde_json::from_str::<V1>(other).is_err(), "{other}");
        }
    }

    #[test]
    fn an_image_repository_refuses_a_tag_a_port_and_upper_case() {
        for ok in ["metrale/metrale-inference-gb10", "a", "a.b_c-d/e"] {
            assert!(is_image_repository(ok), "{ok}");
        }
        for bad in [
            "repo:latest",
            "localhost:5000/x",
            "Repo/x",
            "a//b",
            "a-",
            "",
            "a/-b",
        ] {
            assert!(!is_image_repository(bad), "{bad}");
        }
    }

    #[test]
    fn a_model_repository_is_owner_slash_name() {
        assert!(is_model_repository("openai/gpt-oss-20b"));
        for bad in ["gpt-oss-20b", "/x", "a/b/c", "-a/b", "a/.b"] {
            assert!(!is_model_repository(bad), "{bad}");
        }
    }

    #[test]
    fn an_image_by_digest_is_pulled_by_that_digest() {
        let image: OciImage = serde_json::from_str(&format!(
            r#"{{"repository":"metrale/x","digest":"sha256:{}"}}"#,
            "a".repeat(64)
        ))
        .expect("valid");
        assert_eq!(
            image.reference(),
            format!("metrale/x@sha256:{}", "a".repeat(64))
        );
        assert!(OciDigest::try_from(format!("sha256:{}", "A".repeat(64))).is_err());
    }
}
