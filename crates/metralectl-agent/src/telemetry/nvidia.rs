// SPDX-License-Identifier: MIT OR Apache-2.0

//! Parsing `nvidia-smi` CSV output.
//!
//! Every field is optional, because on a GB10 several of them genuinely are.
//! Real output from that hardware:
//!
//! ```text
//! NVIDIA GB10, [N/A], [N/A], 0 %, 208 MHz, 50, 5.24 W
//! ```
//!
//! The two `[N/A]`s are the memory fields. Grace-Blackwell is unified memory,
//! so there is no framebuffer to report — the pool is the host's, and it comes
//! from `/proc/meminfo` instead.

pub use metralectl_core::hardware::capture::{
    NVIDIA_SAMPLE_QUERY as QUERY, NvidiaSample as Reading, nvidia_sample_argv as argv,
    parse_nvidia_sample as parse,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a real DGX Spark, not invented.
    const GB10: &str = "NVIDIA GB10, [N/A], [N/A], 0 %, 208 MHz, 50, 5.24 W";

    #[test]
    fn a_real_gb10_line_parses_with_no_memory_and_everything_else_present() {
        let r = parse(GB10);
        assert_eq!(r.name.as_deref(), Some("NVIDIA GB10"));
        // The whole reason capabilities are probed: this part has no framebuffer.
        assert_eq!(r.memory_total_bytes, None);
        assert_eq!(r.memory_used_bytes, None);
        assert_eq!(r.util_pct, Some(0.0));
        assert_eq!(r.sm_clock_mhz, Some(208));
        assert_eq!(r.temperature_c, Some(50.0));
        assert_eq!(r.power_w, Some(5.24));
    }

    #[test]
    fn a_discrete_card_reports_its_framebuffer() {
        let r = parse("NVIDIA A100, 40960 MiB, 1024 MiB, 73 %, 1410 MHz, 61, 250.5 W");
        assert_eq!(r.memory_total_bytes, Some(40960 * 1024 * 1024));
        assert_eq!(r.memory_used_bytes, Some(1024 * 1024 * 1024));
        assert_eq!(r.util_pct, Some(73.0));
    }

    #[test]
    fn every_spelling_of_unavailable_is_treated_as_absent() {
        for spelling in [
            "[N/A]",
            "N/A",
            "[Not Supported]",
            "[Unknown Error]",
            "  ",
            "",
        ] {
            let line = format!(
                "GPU, {spelling}, {spelling}, {spelling}, {spelling}, {spelling}, {spelling}"
            );
            let r = parse(&line);
            assert_eq!(r.util_pct, None, "{spelling:?} should be absent");
            assert_eq!(r.sm_clock_mhz, None, "{spelling:?} should be absent");
            assert_eq!(r.power_w, None, "{spelling:?} should be absent");
        }
    }

    #[test]
    fn a_truncated_line_does_not_panic_and_yields_absences() {
        let r = parse("NVIDIA GB10");
        assert_eq!(r.name.as_deref(), Some("NVIDIA GB10"));
        assert_eq!(r.util_pct, None);
    }

    #[test]
    fn empty_output_yields_nothing_rather_than_defaults_that_look_real() {
        assert_eq!(parse(""), Reading::default());
    }
}

// ── The bench surface's questions ──────────────────────────────────────────
//
// What a bench node reports about its accelerator beyond the per-second
// sample above: identity, the CUDA version, the clock ceiling, the thermal
// clock-event reasons, and the compute apps holding the device. Each is one
// `nvidia-smi` invocation, kept here — the provider module — so no neutral
// module carries the vendor's tool name. Parsing is separate from running,
// and tested on captured output.

/// Run `nvidia-smi` with `args`; `None` when it is absent or unhappy.
fn run(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("nvidia-smi")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The first GPU's identity, as far as `nvidia-smi` answered it.
pub fn identity() -> Option<metralectl_core::hardware::capture::NvidiaGpu> {
    use metralectl_core::hardware::capture::{nvidia_identity_argv, parse_nvidia_identity};
    let argv = nvidia_identity_argv();
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    parse_nvidia_identity(&run(&args)?).into_iter().next()
}

/// The CUDA version the driver banner names (`13.0`); `None` when unknown.
pub fn cuda_version() -> Option<String> {
    run(&[])
        .as_deref()
        .and_then(metralectl_core::hardware::capture::parse_cuda_version)
}

/// `clocks.max.sm` of the first GPU, MHz.
pub fn clock_max_mhz() -> Option<f64> {
    run(&["--query-gpu=clocks.max.sm", "--format=csv,noheader,nounits"])
        .as_deref()
        .and_then(parse_clock_max)
}

pub fn parse_clock_max(csv: &str) -> Option<f64> {
    let n: f64 = csv.lines().next()?.trim().parse().ok()?;
    (n > 0.0).then_some(n)
}

/// Whether any THERMAL reason under "Clocks Event Reasons" is `Active`.
/// `None` when the block is absent. SW Power Capping is excluded on
/// purpose: on GB10 it is the steady state of a power-limited part.
pub fn throttle_thermal() -> Option<bool> {
    run(&["-q", "-d", "PERFORMANCE"])
        .as_deref()
        .and_then(parse_throttle_thermal)
}

pub fn parse_throttle_thermal(text: &str) -> Option<bool> {
    let mut in_reasons = false;
    let mut seen = false;
    let mut any = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("Clocks Event Reasons Counters") {
            in_reasons = false;
            continue;
        }
        if t.starts_with("Clocks Event Reasons") {
            in_reasons = true;
            continue;
        }
        let Some((key, raw)) = t.split_once(':') else {
            if !t.is_empty() {
                in_reasons = false;
            }
            continue;
        };
        if !in_reasons {
            continue;
        }
        let active = match raw.trim() {
            "Active" => true,
            "Not Active" => false,
            _ => continue,
        };
        if matches!(
            key.trim(),
            "SW Thermal Slowdown" | "HW Thermal Slowdown" | "HW Power Brake Slowdown"
        ) {
            seen = true;
            any |= active;
        }
    }
    seen.then_some(any)
}

/// `pid, process_name` of every compute app on the device, one per line.
pub fn compute_apps() -> Vec<String> {
    run(&[
        "--query-compute-apps=pid,process_name",
        "--format=csv,noheader",
    ])
    .map(|text| {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod bench_probe_tests {
    use super::*;

    #[test]
    fn the_thermal_reasons_are_read_from_the_reasons_block_only() {
        // Verbatim from `nvidia-smi -q -d PERFORMANCE` on a GB10 (driver
        // 580), with HW Thermal Slowdown flipped to Active.
        let text = "\
    Clocks Event Reasons
        Idle                                           : Not Active
        Applications Clocks Setting                    : Not Active
        SW Power Cap                                   : Active
        HW Slowdown                                    : Not Active
            HW Thermal Slowdown                        : Active
            HW Power Brake Slowdown                    : Not Active
        Sync Boost                                     : Not Active
        SW Thermal Slowdown                            : Not Active
    Clocks Event Reasons Counters
        SW Thermal Slowdown                            : 12 us
";
        assert_eq!(parse_throttle_thermal(text), Some(true));
        let cool = text.replace(
            "HW Thermal Slowdown                        : Active",
            "HW Thermal Slowdown                        : Not Active",
        );
        // NEGATIVE CONTROL for the spelling: the counters-block name is not
        // the reasons-block name, and a fixture with the wrong one would pass
        // by never matching.
        assert_eq!(
            parse_throttle_thermal(
                "    Clocks Event Reasons\n        HW Power Brake Slowdown : Active\n"
            ),
            Some(true)
        );
        // SW power capping alone is not a thermal alert.
        assert_eq!(parse_throttle_thermal(&cool), Some(false));
        // NEGATIVE CONTROL: no reasons block at all is unknown, not false.
        assert_eq!(parse_throttle_thermal("    Performance State : P0\n"), None);
        // A counters block is not read as reasons.
        let only_counters =
            "    Clocks Event Reasons Counters\n        HW Thermal Slowdown : 5 us\n";
        assert_eq!(parse_throttle_thermal(only_counters), None);
    }

    #[test]
    fn the_clock_ceiling_parses_from_captured_output() {
        assert_eq!(parse_clock_max("3003\n"), Some(3003.0));
        assert_eq!(parse_clock_max("[N/A]\n"), None);
        assert_eq!(parse_clock_max("0\n"), None);
    }
}
