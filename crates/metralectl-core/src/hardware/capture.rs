// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a machine's own tools say it is, read from their output.
//!
//! Pure: each function takes what a tool printed and returns what it says,
//! with every field it did not answer left `None`. The callers run the tools
//! (the agent's telemetry and bench probes, and the fingerprint capture in
//! metralectl-control), so one parser serves each tool's output everywhere.
//! The vendor-neutral tools are read here; a vendor's own tool is read in its
//! provider module (`crate::telemetry::nvidia`).

/// A field a tool declines to answer, in any of its spellings (`[N/A]`,
/// `N/A`, `[Not Supported]`, empty).
pub(crate) fn declined(raw: &str) -> bool {
    let t = raw.trim();
    t.is_empty() || (t.starts_with('[') && t.ends_with(']')) || t.eq_ignore_ascii_case("N/A")
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
