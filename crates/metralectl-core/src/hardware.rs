// SPDX-License-Identifier: MIT OR Apache-2.0

//! What kind of machine this is, and whether that is the kind a launch needs.
//!
//! Pure. The machine's side is the name its accelerator reports for itself,
//! the same string the GB10 setting caution reads. It is classified here once,
//! so the caution, the launch gate and the reconciler cannot disagree about
//! what a machine is.

use serde::{Deserialize, Serialize};

/// A hardware class: the key that placements, compatibility receipts and
/// release manifests use (`gb10`). A DNS label, as the platform contract
/// spells it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HardwareClass(String);

impl HardwareClass {
    /// Parse a class name.
    ///
    /// # Errors
    /// When it is not a DNS label: 1 to 63 of `a-z`, `0-9` and `-`, neither
    /// first nor last a `-`.
    pub fn parse(s: &str) -> Result<Self, String> {
        if is_dns_label(s) {
            Ok(Self(s.to_owned()))
        } else {
            Err(format!("{s:?} is not a hardware class (a DNS label)"))
        }
    }

    /// The class name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `s` is a DNS label: 1 to 63 of `a-z`, `0-9` and `-`, neither first
/// nor last a `-`. The platform contract names classes and deployments this way.
#[must_use]
pub fn is_dns_label(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=63).contains(&b.len())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b.first().is_some_and(|c| *c != b'-')
        && b.last().is_some_and(|c| *c != b'-')
}

impl TryFrom<String> for HardwareClass {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Self::parse(&s)
    }
}

impl From<HardwareClass> for String {
    fn from(c: HardwareClass) -> Self {
        c.0
    }
}

impl std::fmt::Display for HardwareClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a recipe declares it needs of the machine it runs on.
///
/// Closed: an unknown key fails the recipe's parse, because a misspelt
/// requirement that silently gated nothing is worse than none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareRequirement {
    /// The class the recipe runs correctly on.
    pub hardware_class: HardwareClass,
}

/// The unified-memory DGX Spark part.
pub const GB10: &str = "gb10";

/// The class of a machine whose accelerator calls itself `accelerator`, when
/// this build knows that accelerator. `None` for an empty name (the probe
/// found nothing) and for hardware this build has never classified.
///
/// Matched on the family rather than an exact string: the vendor tool reports
/// "NVIDIA GB10" today, and a rename or a variant should keep the class rather
/// than silently lose it.
#[must_use]
pub fn classify(accelerator: &str) -> Option<HardwareClass> {
    let a = accelerator.to_ascii_lowercase();
    (a.contains("gb10") || a.contains("grace blackwell")).then(|| HardwareClass(GB10.to_owned()))
}

/// Whether this machine is of the `required` class.
///
/// Fails closed: a machine whose class cannot be determined does not meet any
/// requirement, because a requirement is a claim that only some hardware runs
/// the launch correctly, and "unknown" is not evidence of being that hardware.
///
/// # Errors
/// The reason, naming both the class required and what this machine is.
pub fn check(required: &HardwareClass, accelerator: &str) -> Result<(), String> {
    match classify(accelerator) {
        Some(here) if here == *required => Ok(()),
        Some(here) => Err(format!(
            "it requires {required} hardware and this machine is {here} ({accelerator})"
        )),
        None if accelerator.trim().is_empty() => Err(format!(
            "it requires {required} hardware and this machine reported no accelerator"
        )),
        None => Err(format!(
            "it requires {required} hardware and this machine's accelerator \
             ({accelerator}) is not a class this build recognises"
        )),
    }
}

#[cfg(test)]
#[path = "hardware_tests.rs"]
mod tests;
