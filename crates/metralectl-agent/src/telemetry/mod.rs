// SPDX-License-Identifier: MIT OR Apache-2.0

//! Live numbers about a running model and the machine underneath it.

pub mod meminfo;
pub mod nvidia;

use metralectl_core::io::ProcessRunner;
use metralectl_protocol::telemetry::{DeviceStats, TelemetryCaps};

/// Clock below this while work is in flight counts as clamped.
pub use metralectl_control::measure::HEALTHY_CLOCK_MHZ as DEFAULT_HEALTHY_CLOCK_MHZ;

/// One device sample from the vendor tool; `None` when it did not answer.
fn device_sample(runner: &dyn ProcessRunner) -> Option<nvidia::Reading> {
    let out = runner.run(&nvidia::argv()).ok().filter(|o| o.success())?;
    Some(nvidia::parse(out.stdout.lines().next().unwrap_or_default()))
}

/// Ask the machine what it can actually answer.
///
/// Run once at startup. Capabilities are earned by a reading, never assumed:
/// on unified-memory hardware `nvidia-smi` returns `[N/A]` for memory, and a
/// dashboard that assumed otherwise would render zeros as measurements. No
/// engine has been scraped at startup, so the engine-metrics capability is not
/// earned here; `metralectl_control::measure::caps` grants it to a host that
/// passes an engine scrape.
pub fn probe(runner: &dyn ProcessRunner) -> TelemetryCaps {
    metralectl_control::measure::caps(device_sample(runner).as_ref(), meminfo::available(), None)
}

/// What this machine's accelerator calls itself, for display.
///
/// Already parsed out of the capability probe's own output — `name` is the
/// first field of the query — so this costs one extra invocation at startup
/// and no new vendor-specific code. The fleet showed an empty string here
/// while the reading it came from said "NVIDIA GB10".
///
/// `None` when there is no accelerator, or when it declines to name itself.
/// An empty string is not a name, and rendering one puts a separator with
/// nothing after it in the interface.
pub fn accelerator_name(runner: &dyn ProcessRunner) -> Option<String> {
    device_sample(runner)?
        .name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
}

/// Take one device sample, reporting only what the capabilities allow.
///
/// `busy` says whether the engine has work in flight, which is what makes a low
/// clock meaningful: 208 MHz at idle is correct, and the same figure under load
/// is a problem.
pub fn sample_device(runner: &dyn ProcessRunner, caps: &TelemetryCaps, busy: bool) -> DeviceStats {
    let reading = device_sample(runner).unwrap_or_default();
    // Host memory is read only where it stands in for the accelerator's.
    let host = if caps.framebuffer_memory || !caps.unified_memory {
        meminfo::MemInfo::default()
    } else {
        meminfo::read()
    };
    metralectl_control::measure::device(&reading, caps, host, busy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metralectl_core::io::RecordingRunner;
    use metralectl_core::io::process::Output;

    /// Captured from a real DGX Spark.
    const GB10: &str = "NVIDIA GB10, [N/A], [N/A], 0 %, 208 MHz, 50, 5.24 W";

    fn runner_with(stdout: &str, status: i32) -> RecordingRunner {
        let r = RecordingRunner::new();
        r.push_result(Output {
            status,
            stdout: stdout.into(),
            stderr: String::new(),
        });
        r
    }

    #[test]
    fn probing_a_gb10_finds_no_framebuffer_but_everything_else() {
        let caps = probe(&runner_with(GB10, 0));
        assert!(caps.gpu_util && caps.sm_clock && caps.temperature && caps.power);
        assert!(!caps.framebuffer_memory, "this part reports no framebuffer");
        assert_eq!(caps.sm_clock_healthy_mhz, Some(DEFAULT_HEALTHY_CLOCK_MHZ));
    }

    #[test]
    fn probing_a_machine_without_nvidia_smi_claims_nothing() {
        let caps = probe(&runner_with("", 127));
        assert!(!caps.gpu_util);
        assert!(!caps.sm_clock);
        assert_eq!(caps.sm_clock_healthy_mhz, None);
    }

    #[test]
    fn an_idle_low_clock_is_not_reported_as_clamped() {
        // 208 MHz with nothing running is correct, not a fault.
        let caps = probe(&runner_with(GB10, 0));
        let d = sample_device(&runner_with(GB10, 0), &caps, false);
        assert_eq!(d.sm_clock_mhz, Some(208));
        assert_eq!(
            d.sm_clock_clamped,
            Some(false),
            "idle must not raise a clamp warning"
        );
    }

    #[test]
    fn the_same_clock_under_load_is_reported_as_clamped() {
        // This is the failure that hid for weeks: throughput reads low while
        // every correctness check stays green.
        let caps = probe(&runner_with(GB10, 0));
        let clamped = "NVIDIA GB10, [N/A], [N/A], 96 %, 513 MHz, 71, 42.0 W";
        let d = sample_device(&runner_with(clamped, 0), &caps, true);
        assert_eq!(
            d.sm_clock_clamped,
            Some(true),
            "a clamped clock under load must be flagged"
        );
    }

    #[test]
    fn a_healthy_clock_under_load_is_not_flagged() {
        let caps = probe(&runner_with(GB10, 0));
        let healthy = "NVIDIA GB10, [N/A], [N/A], 96 %, 2405 MHz, 71, 92.0 W";
        assert_eq!(
            sample_device(&runner_with(healthy, 0), &caps, true).sm_clock_clamped,
            Some(false)
        );
    }

    #[test]
    fn memory_on_a_unified_part_comes_from_the_host_and_says_so() {
        let caps = probe(&runner_with(GB10, 0));
        let d = sample_device(&runner_with(GB10, 0), &caps, false);
        if caps.unified_memory {
            assert_eq!(
                d.memory_is_unified,
                Some(true),
                "the client must be told this is not VRAM"
            );
            assert!(d.memory_total_bytes.unwrap_or(0) > 0);
        }
    }

    #[test]
    fn a_capability_that_was_not_probed_is_never_reported() {
        // Even if a later reading contains the field, an unprobed capability
        // stays absent — otherwise the dashboard would flicker into life.
        let caps = TelemetryCaps::default();
        let d = sample_device(&runner_with(GB10, 0), &caps, true);
        assert_eq!(d.gpu_util_pct, None);
        assert_eq!(d.sm_clock_mhz, None);
        assert_eq!(d.sm_clock_clamped, None, "unknown is not \"not clamped\"");
        assert_eq!(d.memory_is_unified, None, "no memory figures, no kind");
    }
}
