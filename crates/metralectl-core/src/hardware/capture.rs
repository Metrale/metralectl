// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a machine's own tools say it is, read from their output.
//!
//! Pure: each function takes what a tool printed and returns what it says,
//! with every field it did not answer left `None`. The callers run the tools
//! (the agent's telemetry and bench probes, and the fingerprint capture in
//! metralectl-control), so one parser serves each tool's output everywhere.

/// The `nvidia-smi` query [`parse_nvidia_identity`] reads, in this order.
pub const NVIDIA_IDENTITY_QUERY: &str = "name,driver_version,count,compute_cap,memory.total";

/// Arguments for an identity query of every NVIDIA accelerator.
#[must_use]
pub fn nvidia_identity_argv() -> Vec<String> {
    vec![
        "nvidia-smi".into(),
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

/// A field `nvidia-smi` declines to answer, in any of its spellings.
fn declined(raw: &str) -> bool {
    let t = raw.trim();
    t.is_empty() || (t.starts_with('[') && t.ends_with(']')) || t.eq_ignore_ascii_case("N/A")
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
        "nvidia-smi".into(),
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

/// Total and in-use host memory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemInfo {
    /// Total, bytes.
    pub total_bytes: Option<u64>,
    /// In use, bytes: total minus what is actually available.
    pub used_bytes: Option<u64>,
}

/// Parse `/proc/meminfo`.
///
/// Uses `MemAvailable` rather than `MemFree`: free memory excludes reclaimable
/// page cache, so on a box that has just loaded 100 GB of weights it reads
/// alarmingly low while the memory is in fact obtainable.
#[must_use]
pub fn parse_meminfo(body: &str) -> MemInfo {
    let field = |name: &str| -> Option<u64> {
        body.lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    };
    let total = field("MemTotal:");
    let avail = field("MemAvailable:");
    MemInfo {
        total_bytes: total,
        used_bytes: match (total, avail) {
            (Some(t), Some(a)) => Some(t.saturating_sub(a)),
            _ => None,
        },
    }
}

/// The CPU model `lscpu` names. A part with several core types (Arm
/// big.LITTLE) lists each; they are joined in `lscpu`'s order, so the same
/// machine always reads the same.
#[must_use]
pub fn parse_lscpu_model(text: &str) -> Option<String> {
    let mut models: Vec<&str> = Vec::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(':')
            && key.trim() == "Model name"
        {
            let v = value.trim();
            if !declined(v) && v != "-" && !models.contains(&v) {
                models.push(v);
            }
        }
    }
    (!models.is_empty()).then(|| models.join(" + "))
}

/// Whether an accelerator uses host memory, from what the probes answered:
/// a framebuffer of its own means no; none, beside readable host memory,
/// means yes. With no accelerator answering, or no host figure to stand in
/// for its memory, it is unknown, never a guess.
#[must_use]
pub fn memory_is_unified(
    accelerator_answered: bool,
    framebuffer: Option<u64>,
    host_memory_known: bool,
) -> Option<bool> {
    match (accelerator_answered, framebuffer, host_memory_known) {
        (false, _, _) => None,
        (true, Some(_), _) => Some(false),
        (true, None, true) => Some(true),
        (true, None, false) => None,
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
