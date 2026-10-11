// SPDX-License-Identifier: MIT OR Apache-2.0

//! This machine's `hardware-fingerprint.v1`, captured from its own tools.
//!
//! [`ToolOutputs::read`] runs the tools (the only I/O here); [`capture`] reads
//! what they printed through the shared parsers in
//! `metralectl_core::hardware::capture` and classifies the accelerator with
//! `metralectl_core::hardware::classify`, the same classification the launch
//! gate and the reconciler use. Every quantity a tool did not answer is
//! written absent (`unavailable`), never zero or a guess (DECISIONS PC-V), so
//! a partly probed machine still has a document, with its own digest. Only
//! what identifies the hardware is required; without it
//! [`Capture::fingerprint`] names what is missing and makes no document.
//!
//! NVIDIA hosts are read today. A ROCm host (Strix Halo) leaves every
//! accelerator field absent: no captured `rocm-smi`/`amd-smi` output from
//! that part was available to test a parser against.

use crate::platform::field::{AbsentReason, Field};
use crate::platform::fingerprint::{Accelerator, CpuArch, HardwareFingerprint, Host, Vendor};
use crate::platform::scalars::{SoftwareVersion, Text, V1};
use metralectl_core::hardware::capture::{
    self, NvidiaGpu, memory_is_unified, parse_cuda_version, parse_lscpu_model, parse_meminfo,
    parse_nvidia_identity, sm_architecture,
};
use metralectl_core::hardware::{HardwareClass, classify};
use metralectl_core::io::{FileSystem, ProcessRunner};
use std::num::{NonZeroU32, NonZeroU64};
use std::path::Path;

/// What the machine's tools printed; `None` for a tool that did not answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOutputs {
    /// `nvidia-smi` identity query (`capture::nvidia_identity_argv`).
    pub nvidia_identity: Option<String>,
    /// `nvidia-smi` with no arguments: the banner naming the CUDA version.
    pub nvidia_banner: Option<String>,
    /// `/proc/meminfo`.
    pub meminfo: Option<String>,
    /// `lscpu`.
    pub lscpu: Option<String>,
    /// `uname -r`.
    pub os_kernel: Option<String>,
    /// The CPU architecture this agent was built for (`std::env::consts::ARCH`).
    pub cpu_arch: String,
}

impl ToolOutputs {
    /// Run the tools.
    #[must_use]
    pub fn read(runner: &dyn ProcessRunner, fs: &dyn FileSystem) -> Self {
        let run = |argv: Vec<String>| {
            runner
                .run(&argv)
                .ok()
                .filter(|o| o.success())
                .map(|o| o.stdout)
        };
        let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        Self {
            nvidia_identity: run(capture::nvidia_identity_argv()),
            nvidia_banner: run(words(&["nvidia-smi"])),
            meminfo: fs.read_to_string(Path::new("/proc/meminfo")).ok(),
            lscpu: run(words(&["lscpu"])),
            os_kernel: run(words(&["uname", "-r"])),
            cpu_arch: std::env::consts::ARCH.to_owned(),
        }
    }
}

/// One kind of accelerator, as far as the tools answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedAccelerator {
    /// Who made it.
    pub vendor: Vendor,
    /// What it calls itself.
    pub product: Option<String>,
    /// Its ISA target.
    pub architecture: Option<String>,
    /// Memory it can use: its framebuffer, or the host's when unified.
    pub memory_bytes: Option<u64>,
    /// How many of it the tool listed.
    pub count: u32,
}

/// What the machine is, field by field; `None` where nothing answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// The class of the first accelerator.
    pub hardware_class: Option<HardwareClass>,
    /// One entry per kind of accelerator.
    pub accelerators: Vec<CapturedAccelerator>,
    /// CPU architecture.
    pub cpu_arch: Option<CpuArch>,
    /// CPU model.
    pub cpu_model: Option<String>,
    /// Host memory, bytes.
    pub host_memory_bytes: Option<u64>,
    /// Whether the accelerators use host memory.
    pub unified_memory: Option<bool>,
    /// OS kernel release.
    pub os_kernel: Option<String>,
    /// Accelerator driver `(name, version)`.
    pub driver: Option<(String, String)>,
    /// Accelerator toolkit `(name, version)`.
    pub toolkit: Option<(String, String)>,
}

/// Read what the tools printed.
#[must_use]
pub fn capture(out: &ToolOutputs) -> Capture {
    let gpus = out
        .nvidia_identity
        .as_deref()
        .map(parse_nvidia_identity)
        .unwrap_or_default();
    let host_memory_bytes = out
        .meminfo
        .as_deref()
        .and_then(|m| parse_meminfo(m).total_bytes);
    let first = gpus.first();
    let unified = memory_is_unified(
        first.is_some(),
        first.and_then(|g| g.memory_total_bytes),
        host_memory_bytes.is_some(),
    );
    let mut accelerators: Vec<CapturedAccelerator> = Vec::new();
    for gpu in &gpus {
        let kind = accelerator_of(gpu, unified, host_memory_bytes);
        match accelerators.iter_mut().find(|a| same_kind(a, &kind)) {
            Some(a) => a.count += 1,
            None => accelerators.push(kind),
        }
    }
    let text = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    };
    Capture {
        hardware_class: first.and_then(|g| g.name.as_deref()).and_then(classify),
        accelerators,
        cpu_arch: match out.cpu_arch.as_str() {
            "aarch64" => Some(CpuArch::Aarch64),
            "x86_64" => Some(CpuArch::X86_64),
            _ => None,
        },
        cpu_model: out.lscpu.as_deref().and_then(parse_lscpu_model),
        host_memory_bytes,
        unified_memory: unified,
        os_kernel: text(&out.os_kernel),
        driver: first
            .and_then(|g| g.driver.clone())
            .map(|v| ("nvidia".to_owned(), v)),
        toolkit: out
            .nvidia_banner
            .as_deref()
            .and_then(parse_cuda_version)
            .filter(|_| first.is_some())
            .map(|v| ("cuda".to_owned(), v)),
    }
}

fn accelerator_of(
    gpu: &NvidiaGpu,
    unified: Option<bool>,
    host: Option<u64>,
) -> CapturedAccelerator {
    CapturedAccelerator {
        vendor: Vendor::Nvidia,
        product: gpu.name.clone(),
        architecture: gpu.compute_cap.map(sm_architecture),
        memory_bytes: match unified {
            Some(true) => host,
            Some(false) => gpu.memory_total_bytes,
            None => None,
        },
        count: 1,
    }
}

fn same_kind(a: &CapturedAccelerator, b: &CapturedAccelerator) -> bool {
    a.vendor == b.vendor
        && a.product == b.product
        && a.architecture == b.architecture
        && a.memory_bytes == b.memory_bytes
}

/// `v`, or a note in `missing` that `path` was not measured.
fn require<T>(missing: &mut Vec<String>, path: &str, v: Option<T>) -> Option<T> {
    if v.is_none() {
        missing.push(path.to_owned());
    }
    v
}

fn text(s: Option<&str>) -> Option<Text> {
    s.and_then(|s| Text::parse(s).ok())
}

fn version(v: Option<&(String, String)>) -> Option<SoftwareVersion> {
    let (name, version) = v?;
    Some(SoftwareVersion {
        name: Text::parse(name).ok()?,
        version: Text::parse(version).ok()?,
    })
}

/// A quantity a tool answered, or absent because it did not.
fn quantity<T>(v: Option<T>) -> Field<T> {
    Field::measured_or(v, AbsentReason::Unavailable)
}

impl Capture {
    /// The fingerprint document: every identifying field measured, every
    /// quantity measured or absent.
    ///
    /// # Errors
    /// Each identifying field the tools did not answer (or answered with
    /// something the contract refuses), by its path in the document.
    pub fn fingerprint(&self) -> Result<HardwareFingerprint, Vec<String>> {
        let mut missing: Vec<String> = Vec::new();
        let m = &mut missing;
        if self.accelerators.is_empty() {
            m.push("accelerators".to_owned());
        }
        let mut accelerators = Vec::new();
        for (i, a) in self.accelerators.iter().enumerate() {
            let at = |field: &str| format!("accelerators/{i}/{field}");
            let product = require(m, &at("product"), text(a.product.as_deref()));
            let architecture = require(m, &at("architecture"), text(a.architecture.as_deref()));
            let count = require(m, &at("count"), NonZeroU32::new(a.count));
            if let (Some(product), Some(architecture), Some(count)) = (product, architecture, count)
            {
                accelerators.push(Accelerator {
                    vendor: a.vendor,
                    product,
                    architecture,
                    memory_bytes: quantity(a.memory_bytes.and_then(NonZeroU64::new)),
                    count,
                });
            }
        }
        let class = require(m, "hardware_class", self.hardware_class.clone());
        let cpu_arch = require(m, "host/cpu_arch", self.cpu_arch);
        let unified_memory = require(m, "host/unified_memory", self.unified_memory);
        match (class, cpu_arch, unified_memory) {
            (Some(hardware_class), Some(cpu_arch), Some(unified_memory)) if missing.is_empty() => {
                Ok(HardwareFingerprint {
                    version: V1,
                    hardware_class,
                    accelerators,
                    host: Host {
                        cpu_arch,
                        cpu_model: quantity(text(self.cpu_model.as_deref())),
                        memory_bytes: quantity(self.host_memory_bytes.and_then(NonZeroU64::new)),
                        unified_memory,
                        os_kernel: quantity(text(self.os_kernel.as_deref())),
                    },
                    driver: quantity(version(self.driver.as_ref())),
                    toolkit: quantity(version(self.toolkit.as_ref())),
                })
            }
            _ => Err(missing),
        }
    }
}

#[cfg(test)]
#[path = "fingerprint_tests.rs"]
mod tests;
