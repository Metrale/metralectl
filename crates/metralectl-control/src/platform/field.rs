// SPDX-License-Identifier: MIT OR Apache-2.0

//! A quantity with its provenance: `{"measured": v}`, `{"derived": v}` or
//! `{"absent": reason}`. Absent is never written as zero (the bundle's
//! `common.v1` `field_*` definitions).

use serde::{Deserialize, Serialize};

/// Why a quantity has no value. Closed; the platform's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbsentReason {
    /// This machine or build cannot measure it.
    Unsupported,
    /// It could be measured, and the probe did not answer.
    Unavailable,
    /// It has no meaning here.
    NotApplicable,
    /// It was measured and the figure was lost.
    Lost,
    /// Not measured yet.
    NotYet,
}

/// A value read directly, one computed from read values, or none with why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field<T> {
    /// Read directly.
    Measured(T),
    /// Computed from values that were read.
    Derived(T),
    /// No value, for this reason.
    Absent(AbsentReason),
}

impl<T> Field<T> {
    /// `Measured(v)` for `Some(v)`; otherwise absent for `reason`.
    pub fn measured_or(v: Option<T>, reason: AbsentReason) -> Self {
        v.map_or(Self::Absent(reason), Self::Measured)
    }

    /// The value, measured or derived.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Measured(v) | Self::Derived(v) => Some(v),
            Self::Absent(_) => None,
        }
    }
}
