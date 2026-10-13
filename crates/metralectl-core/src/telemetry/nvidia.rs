// SPDX-License-Identifier: MIT OR Apache-2.0

//! What NVIDIA's `nvidia-smi` says about this machine: the argument lists to
//! run it with and the parsers for what it prints. The provider module for
//! NVIDIA readings; the vendor-neutral tools are read in
//! `crate::hardware::capture`.

use crate::hardware::capture::declined;

/// The tool every argument list here starts with.
pub const TOOL: &str = "nvidia-smi";

/// Arguments for the banner (the tool with no arguments), which names the
/// CUDA version [`parse_cuda_version`] reads.
#[must_use]
pub fn banner_argv() -> Vec<String> {
    vec![TOOL.into()]
}

/// The `nvidia-smi` query [`parse_nvidia_identity`] reads, in this order.
pub const NVIDIA_IDENTITY_QUERY: &str = "name,driver_version,count,compute_cap,memory.total";

/// Arguments for an identity query of every NVIDIA accelerator.
#[must_use]
pub fn nvidia_identity_argv() -> Vec<String> {
    vec![
        TOOL.into(),
        format!("--query-gpu={NVIDIA_IDENTITY_QUERY}"),
        "--format=csv,noheader,nounits".into(),
    ]
}

/// One NVIDIA accelerator, as far as `nvidia-smi` answers for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NvidiaGpu {
    /// What it calls itself.
    pub name: Option<String>,
    /// The driver version.
    pub driver: Option<String>,
    /// How many accelerators the driver reports in all.
    pub count: Option<u32>,
    /// Compute capability, `(major, minor)`.
    pub compute_cap: Option<(u32, u32)>,
    /// Its own framebuffer, bytes; `None` on a part with none (unified memory).
    pub memory_total_bytes: Option<u64>,
}

/// Parse [`nvidia_identity_argv`] output: one accelerator per line.
#[must_use]
pub fn parse_nvidia_identity(csv: &str) -> Vec<NvidiaGpu> {
    csv.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let f: Vec<&str> = line.split(',').collect();
            let at = |i: usize| f.get(i).copied().filter(|v| !declined(v)).map(str::trim);
            NvidiaGpu {
                name: at(0).map(str::to_owned),
                driver: at(1).map(str::to_owned),
                count: at(2).and_then(|v| v.parse().ok()),
                compute_cap: at(3).and_then(|v| {
                    let (major, minor) = v.split_once('.')?;
                    Some((major.parse().ok()?, minor.parse().ok()?))
                }),
                // `nounits`: MiB as a bare number.
                memory_total_bytes: at(4)
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|mib| *mib > 0)
                    .map(|mib| mib * 1024 * 1024),
            }
        })
        .collect()
}

/// The `nvidia-smi` per-sample query [`parse_nvidia_sample`] reads, in this
/// order.
pub const NVIDIA_SAMPLE_QUERY: &str =
    "name,memory.total,memory.used,utilization.gpu,clocks.current.sm,temperature.gpu,power.draw";

/// Arguments for one telemetry sample.
#[must_use]
pub fn nvidia_sample_argv() -> Vec<String> {
    vec![
        TOOL.into(),
        format!("--query-gpu={NVIDIA_SAMPLE_QUERY}"),
        "--format=csv,noheader".into(),
    ]
}

/// One accelerator's live readings, as far as it reports them. On a GB10 the
/// memory fields are absent: it has no framebuffer, the pool is the host's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NvidiaSample {
    /// Device name.
    pub name: Option<String>,
    /// Framebuffer total, bytes.
    pub memory_total_bytes: Option<u64>,
    /// Framebuffer in use, bytes.
    pub memory_used_bytes: Option<u64>,
    /// Utilisation, percent.
    pub util_pct: Option<f64>,
    /// Current SM clock, MHz.
    pub sm_clock_mhz: Option<u32>,
    /// Temperature, Celsius.
    pub temperature_c: Option<f64>,
    /// Power draw, Watts.
    pub power_w: Option<f64>,
}

/// The number in front of a field's unit (`208 MHz`), unless declined.
fn number(raw: &str) -> Option<f64> {
    if declined(raw) {
        return None;
    }
    raw.split_whitespace().next()?.parse::<f64>().ok()
}

/// Parse one line of [`nvidia_sample_argv`] output.
#[must_use]
pub fn parse_nvidia_sample(line: &str) -> NvidiaSample {
    let f: Vec<&str> = line.split(',').collect();
    let at = |i: usize| f.get(i).copied().unwrap_or("");
    // nvidia-smi prints whole MiB and MHz; a negative one is not a reading.
    let whole = |v: f64| (v >= 0.0).then_some(v as u64);
    NvidiaSample {
        name: (!declined(at(0))).then(|| at(0).trim().to_string()),
        memory_total_bytes: number(at(1)).and_then(whole).map(|m| m * 1024 * 1024),
        memory_used_bytes: number(at(2)).and_then(whole).map(|m| m * 1024 * 1024),
        util_pct: number(at(3)),
        sm_clock_mhz: number(at(4))
            .and_then(whole)
            .and_then(|v| u32::try_from(v).ok()),
        temperature_c: number(at(5)),
        power_w: number(at(6)),
    }
}

/// The ISA target a compute capability names: `(12, 1)` is `sm_121`.
#[must_use]
pub fn sm_architecture((major, minor): (u32, u32)) -> String {
    format!("sm_{major}{minor}")
}

/// The CUDA version `nvidia-smi`'s banner names (`13.0`).
#[must_use]
pub fn parse_cuda_version(banner: &str) -> Option<String> {
    let i = banner.find("CUDA Version:")?;
    banner[i + "CUDA Version:".len()..]
        .split_whitespace()
        .next()
        .filter(|v| !declined(v))
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "nvidia_tests.rs"]
mod tests;
