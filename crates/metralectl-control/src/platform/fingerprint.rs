// SPDX-License-Identifier: MIT OR Apache-2.0

//! `hardware-fingerprint.v1`: what a receipt's hardware key means.
//!
//! Accelerators, host, driver and toolkit; no time, serial number or
//! hostname, so identical machines share one digest and a driver or toolkit
//! change makes a new one on purpose. What identifies the hardware is
//! required; a quantity the probe could not read is absent with its reason,
//! and the absence is part of the digest (DECISIONS PC-V).

use super::field::Field;
use super::scalars::{SoftwareVersion, Text, V1, non_empty};
use metralectl_core::hardware::HardwareClass;
use serde::{Deserialize, Serialize};
use std::num::{NonZeroU32, NonZeroU64};

/// Accelerator vendors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    /// NVIDIA.
    Nvidia,
    /// AMD.
    Amd,
    /// Intel.
    Intel,
    /// Apple.
    Apple,
}

/// One kind of accelerator, and how many of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accelerator {
    /// Who made it.
    pub vendor: Vendor,
    /// What it calls itself.
    pub product: Text,
    /// Its ISA target (`sm_121`, `gfx1151`).
    pub architecture: Text,
    /// Memory it can use, bytes.
    pub memory_bytes: Field<NonZeroU64>,
    /// How many.
    pub count: NonZeroU32,
}

/// CPU architectures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuArch {
    /// 64-bit Arm.
    Aarch64,
    /// 64-bit x86.
    #[serde(rename = "x86_64")]
    X86_64,
}

/// The host around the accelerators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    /// CPU architecture.
    pub cpu_arch: CpuArch,
    /// CPU model.
    pub cpu_model: Field<Text>,
    /// Host memory, bytes.
    pub memory_bytes: Field<NonZeroU64>,
    /// Whether the accelerators use host memory.
    pub unified_memory: bool,
    /// The OS kernel release.
    pub os_kernel: Field<Text>,
}

/// A machine's hardware identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareFingerprint {
    /// Always 1.
    pub version: V1,
    /// The class it belongs to.
    pub hardware_class: HardwareClass,
    /// Its accelerators.
    #[serde(deserialize_with = "non_empty")]
    pub accelerators: Vec<Accelerator>,
    /// The host.
    pub host: Host,
    /// The accelerator driver.
    pub driver: Field<SoftwareVersion>,
    /// The accelerator toolkit.
    pub toolkit: Field<SoftwareVersion>,
}
