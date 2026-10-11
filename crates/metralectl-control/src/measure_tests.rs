// SPDX-License-Identifier: MIT OR Apache-2.0

//! The producers over exposition text in the runtime's documented series, and
//! over device samples.

use super::*;
use metralectl_core::hardware::capture::parse_nvidia_sample;

/// The series `docs/qci/runtime-telemetry.md` adds, rendered as the runtime's
/// exporter writes them, beside the long-standing counters.
const RUNTIME: &str = "\
metrale_requests_total 7
metrale_requests_active 2
metrale_prompt_tokens_total 4096
metrale_generation_tokens_total 1024
metrale_request_tpot_seconds_bucket{le=\"0.01\"} 1
metrale_request_tpot_seconds_bucket{le=\"0.02\"} 3
metrale_request_tpot_seconds_bucket{le=\"+Inf\"} 3
metrale_request_tpot_seconds_sum 0.05
metrale_request_tpot_seconds_count 3
metrale_queue_time_seconds_bucket{model=\"m\",le=\"0.1\"} 4
metrale_queue_time_seconds_bucket{model=\"m\",le=\"0.2\"} 4
metrale_queue_time_seconds_bucket{model=\"m\",le=\"+Inf\"} 4
metrale_queue_time_seconds_sum{model=\"m\"} 0.2
metrale_queue_time_seconds_count{model=\"m\"} 4
metrale_gpu_energy_millijoules_total 2000
metrale_build_info{version=\"1.0.0\",commit=\"99B2613072A69A8708048347F1B74D9CC7880ED7\"} 1
metrale_startup_phase 11
";

fn scrape(text: &str) -> Scrape {
    metrics::parse(text)
}

#[test]
fn the_runtime_series_become_measured_fields() {
    let r = point_in_time(&scrape(RUNTIME));
    assert_eq!(r.prompt_tokens_total, Some(4096));
    assert_eq!(r.generation_tokens_total, Some(1024));
    assert_eq!(r.requests_total, Some(7.0));
    // Half of three observations falls in the 10-20 ms bucket, a quarter of
    // the way through it.
    let tpot = r.tpot_p50_s.expect("tpot");
    assert!((tpot - 0.0125).abs() < 1e-9, "{tpot}");
    let queue = r.queue_time_p50_s.expect("queue");
    assert!((queue - 0.05).abs() < 1e-9, "{queue}");
    assert_eq!(r.gpu_energy_mj_total, Some(2000));
    assert_eq!(r.phase, Some(LaunchPhase::Serving));
    assert_eq!(
        r.engine_commit.as_deref(),
        Some("99b2613072a69a8708048347f1b74d9cc7880ed7")
    );
    // Rates need two scrapes; one scrape leaves them to the caller.
    assert_eq!(r.decode_tokens_per_s, None);
}

/// An engine that exports none of it (an older build, or device telemetry
/// off) yields absences. Sabotage: reading a missing series as 0 fills these.
#[test]
fn what_the_engine_does_not_export_is_absent_not_zero() {
    let r = point_in_time(&scrape("metrale_requests_total 1\n"));
    assert_eq!(r.prompt_tokens_total, None);
    assert_eq!(r.tpot_p50_s, None);
    assert_eq!(r.queue_time_p50_s, None);
    assert_eq!(r.gpu_energy_mj_total, None);
    assert_eq!(r.phase, None);
    assert_eq!(r.engine_commit, None);
    let unstamped = "metrale_build_info{version=\"1\",commit=\"unstamped\"} 1\n";
    assert_eq!(engine_commit(&scrape(unstamped)), None);
}

#[test]
fn a_counter_is_a_whole_number_or_nothing() {
    assert_eq!(count(4096.0), Some(4096));
    for not_a_count in [-1.0, 1.5, f64::NAN, f64::INFINITY, 1e300] {
        assert_eq!(count(not_a_count), None, "{not_a_count}");
    }
}

#[test]
fn the_startup_phase_maps_to_where_the_launch_has_got() {
    let at = |phase: &str, active: &str| {
        super::phase(&scrape(&format!("metrale_startup_phase {phase}\n{active}")))
    };
    assert_eq!(at("0", ""), Some(LaunchPhase::Starting));
    assert_eq!(at("5", ""), Some(LaunchPhase::LoadingWeights));
    assert_eq!(at("9", ""), Some(LaunchPhase::Starting));
    assert_eq!(at("11", ""), Some(LaunchPhase::Ready));
    assert_eq!(
        at("11", "metrale_requests_active 0\n"),
        Some(LaunchPhase::Ready)
    );
    assert_eq!(
        at("11", "metrale_requests_active 3\n"),
        Some(LaunchPhase::Serving)
    );
    // A phase this build does not know is not guessed at.
    assert_eq!(at("12", ""), None);
}

/// The capability is earned by a scrape that is the engine's. Sabotage: the
/// old hard-coded `false`, or `true` for any answer, fails one side here.
#[test]
fn the_engine_metrics_capability_is_earned_by_an_engine_scrape() {
    let engine = scrape(RUNTIME);
    let foreign = scrape("vllm:num_requests_running 0\n");
    assert!(engine_answers(&engine));
    assert!(!engine_answers(&foreign));
    assert!(!engine_answers(&scrape("")));
    assert!(caps(None, true, Some(&engine)).engine_metrics);
    assert!(!caps(None, true, Some(&foreign)).engine_metrics);
    assert!(!caps(None, true, None).engine_metrics);
}

/// Captured from a real DGX Spark.
const GB10: &str = "NVIDIA GB10, [N/A], [N/A], 0 %, 208 MHz, 50, 5.24 W";
const H100: &str = "NVIDIA H100, 81559 MiB, 1024 MiB, 73 %, 1410 MHz, 61, 250.5 W";

fn host() -> MemInfo {
    MemInfo {
        total_bytes: Some(128 << 30),
        used_bytes: Some(12 << 30),
    }
}

#[test]
fn a_gb10_reports_host_memory_and_says_it_is_unified() {
    let sample = parse_nvidia_sample(GB10);
    let c = caps(Some(&sample), true, None);
    assert!(!c.framebuffer_memory && c.unified_memory && c.gpu_util && c.power);
    let d = device(&sample, &c, host(), false);
    assert_eq!(d.memory_is_unified, Some(true));
    assert_eq!(d.memory_total_bytes, Some(128 << 30));
    assert_eq!(d.memory_used_bytes, Some(12 << 30));
    assert_eq!(d.gpu_util_pct, Some(0.0));
    assert_eq!(d.power_w, Some(5.24));
    assert_eq!(d.sm_clock_clamped, Some(false), "idle is not clamped");
}

/// Sabotage: deciding the kind from this sample's memory read reports a
/// discrete card whose read failed once as unified.
#[test]
fn a_discrete_card_reports_its_framebuffer_and_stays_discrete() {
    let sample = parse_nvidia_sample(H100);
    let c = caps(Some(&sample), true, None);
    let d = device(&sample, &c, host(), true);
    assert_eq!(d.memory_is_unified, Some(false));
    assert_eq!(d.memory_total_bytes, Some(81559 << 20));
    let failed_read = parse_nvidia_sample("NVIDIA H100, [N/A], [N/A], 73 %, 1410 MHz, 61, 250 W");
    let d = device(&failed_read, &c, host(), true);
    assert_eq!(d.memory_is_unified, Some(false));
    assert_eq!(d.memory_total_bytes, None);
    assert_eq!(d.sm_clock_clamped, Some(true), "1410 MHz under load");
}

#[test]
fn no_device_and_no_host_memory_claim_nothing() {
    let none = caps(None, true, None);
    assert_eq!(none, TelemetryCaps::default());
    let sample = parse_nvidia_sample(GB10);
    let no_host = caps(Some(&sample), false, None);
    let d = device(&sample, &no_host, MemInfo::default(), false);
    assert_eq!(d.memory_is_unified, None, "no memory figures, no kind");
    assert_eq!(d.memory_total_bytes, None);
}
