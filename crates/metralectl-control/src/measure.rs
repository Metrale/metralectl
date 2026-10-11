// SPDX-License-Identifier: MIT OR Apache-2.0

//! Measured fields: what the engine's own `/metrics` and the device probes
//! say, in the shapes the wire carries (#28, #37). Pure; no transport.
//!
//! A host that reports them (this agent's browser surface, a managed node's
//! reporter) takes them from here, so every surface reads one derivation.
//! **Absent is never zero**: a series the engine does not export, a value no
//! counter can hold, a phase this build does not know, all stay `None`.
//!
//! The engine series are the ones `docs/qci/runtime-telemetry.md` of the
//! runtime documents: request and token counters, the TTFT, TPOT and queue
//! histograms, the startup phase, the build info, and (at a device telemetry
//! level) the GPU energy counter.

use metralectl_core::hardware::capture::{MemInfo, NvidiaSample, memory_is_unified};
use metralectl_core::metrics::{self, Scrape};
use metralectl_protocol::msg::LaunchReading;
use metralectl_protocol::telemetry::{DeviceStats, LaunchPhase, TelemetryCaps};

/// Clock below this while work is in flight counts as clamped.
///
/// The number matters. A GB10 idles around 200 MHz and runs above 2000 under
/// load; a clamp at 513 MHz once made every throughput measurement read two to
/// three times low while every correctness check stayed green. A threshold in
/// between catches that without firing at idle.
pub const HEALTHY_CLOCK_MHZ: u32 = 1500;

/// The startup phase the engine enters once it is listening.
const LISTENING: u64 = 11;
/// The startup phase in which it loads its weights.
const WEIGHT_LOAD: u64 = 5;

/// A counter's value as the whole number it counts; `None` for a value no
/// counter holds (negative, fractional, not finite).
#[must_use]
pub fn count(v: f64) -> Option<u64> {
    // Checked whole and within f64's exact integers before the cast.
    (v.is_finite() && v >= 0.0 && v.fract() == 0.0 && v <= 2f64.powi(53)).then_some(v as u64)
}

/// The median of histogram `name`, when it has observations.
fn median(s: &Scrape, name: &str) -> Option<f64> {
    s.buckets.get(name).and_then(|b| metrics::quantile(b, 0.5))
}

/// Everything one scrape says on its own; the windowed rates need two and are
/// left to the caller.
#[must_use]
pub fn point_in_time(s: &Scrape) -> LaunchReading {
    let ttft = s.buckets.get("metrale_time_to_first_token_seconds");
    LaunchReading {
        requests_total: s.get("metrale_requests_total"),
        requests_active: s.get("metrale_requests_active"),
        ttft_p50_s: ttft.and_then(|b| metrics::quantile(b, 0.50)),
        ttft_p90_s: ttft.and_then(|b| metrics::quantile(b, 0.90)),
        accept_rate: accept_rate(s),
        prefix_hit_rate: s.get("metrale_prefix_cache_hit_rate"),
        prompt_tokens_total: s.get("metrale_prompt_tokens_total").and_then(count),
        generation_tokens_total: s.get("metrale_generation_tokens_total").and_then(count),
        tpot_p50_s: median(s, "metrale_request_tpot_seconds"),
        queue_time_p50_s: median(s, "metrale_queue_time_seconds"),
        gpu_energy_mj_total: s
            .get("metrale_gpu_energy_millijoules_total")
            .and_then(count),
        phase: phase(s),
        engine_commit: engine_commit(s),
        ..LaunchReading::default()
    }
}

/// Draft acceptance, read per label because the sum of accepts and rejects is
/// a number with no meaning.
fn accept_rate(s: &Scrape) -> Option<f64> {
    const METRIC: &str = "metrale_spec_decode_verify_total";
    let accepted = s.sum_where(METRIC, "outcome", "accept")?;
    let total = s.get(METRIC)?;
    metrics::accept_rate(accepted, total)
}

/// Where the engine has got to, from the index of the last startup phase it
/// entered (`metrale_startup_phase`): weight load is loading weights, any
/// other phase before listening is starting, and once listening it is ready,
/// or serving while requests are in flight.
#[must_use]
pub fn phase(s: &Scrape) -> Option<LaunchPhase> {
    match s.get("metrale_startup_phase").and_then(count)? {
        WEIGHT_LOAD => Some(LaunchPhase::LoadingWeights),
        p if p < LISTENING => Some(LaunchPhase::Starting),
        LISTENING => match s.get("metrale_requests_active").and_then(count) {
            Some(active) if active > 0 => Some(LaunchPhase::Serving),
            _ => Some(LaunchPhase::Ready),
        },
        _ => None,
    }
}

/// The commit `metrale_build_info` names; `None` for an unstamped build.
#[must_use]
pub fn engine_commit(s: &Scrape) -> Option<String> {
    s.series
        .get("metrale_build_info")?
        .iter()
        .filter_map(|series| series.labels.get("commit"))
        .find(|c| c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|c| c.to_ascii_lowercase())
}

/// Whether a scrape came from the engine: it carries at least one of the
/// engine's own series. A port answering with anything else (another server,
/// an empty body) does not earn the capability.
#[must_use]
pub fn engine_answers(s: &Scrape) -> bool {
    let ours = |name: &String| name.starts_with("metrale_");
    s.values.keys().any(ours) || s.buckets.keys().any(ours) || s.series.keys().any(ours)
}

/// What this machine can answer, earned by readings: a device sample, whether
/// host memory is readable, and a scrape of an engine, each when there was
/// one.
#[must_use]
pub fn caps(
    device: Option<&NvidiaSample>,
    host_memory_readable: bool,
    engine: Option<&Scrape>,
) -> TelemetryCaps {
    let engine_metrics = engine.is_some_and(engine_answers);
    let Some(d) = device else {
        return TelemetryCaps {
            engine_metrics,
            ..TelemetryCaps::default()
        };
    };
    TelemetryCaps {
        gpu_util: d.util_pct.is_some(),
        sm_clock: d.sm_clock_mhz.is_some(),
        temperature: d.temperature_c.is_some(),
        power: d.power_w.is_some(),
        framebuffer_memory: d.memory_total_bytes.is_some(),
        // Host memory is the meaningful figure when there is no framebuffer.
        unified_memory: host_memory_readable,
        engine_metrics,
        sm_clock_healthy_mhz: d.sm_clock_mhz.map(|_| HEALTHY_CLOCK_MHZ),
    }
}

/// One device sample, reporting only what the capabilities allow.
///
/// `busy` says whether the engine has work in flight, which is what makes a
/// low clock meaningful: 208 MHz at idle is correct, and the same figure under
/// load is a problem.
#[must_use]
pub fn device(d: &NvidiaSample, caps: &TelemetryCaps, host: MemInfo, busy: bool) -> DeviceStats {
    // Idle is a verdict, not a gap: a low clock with nothing in flight is not
    // a clamp. Without a clock or a threshold there is no verdict at all.
    let clamped = match (busy, d.sm_clock_mhz, caps.sm_clock_healthy_mhz) {
        (true, Some(mhz), Some(healthy)) => Some(mhz < healthy),
        (false, Some(_), Some(_)) => Some(false),
        _ => None,
    };
    // The kind of memory is the probe's measurement; a discrete card whose
    // memory read failed this once is still not unified.
    let unified = if caps.framebuffer_memory {
        Some(false)
    } else {
        memory_is_unified(
            true,
            None,
            caps.unified_memory && host.total_bytes.is_some(),
        )
    };
    let (total, used) = match unified {
        Some(false) => (d.memory_total_bytes, d.memory_used_bytes),
        Some(true) => (host.total_bytes, host.used_bytes),
        None => (None, None),
    };
    DeviceStats {
        gpu_util_pct: caps.gpu_util.then_some(d.util_pct).flatten(),
        sm_clock_mhz: caps.sm_clock.then_some(d.sm_clock_mhz).flatten(),
        sm_clock_clamped: clamped,
        temperature_c: caps.temperature.then_some(d.temperature_c).flatten(),
        power_w: caps.power.then_some(d.power_w).flatten(),
        memory_total_bytes: total,
        memory_used_bytes: used,
        memory_is_unified: unified,
    }
}

#[cfg(test)]
#[path = "measure_tests.rs"]
mod tests;
