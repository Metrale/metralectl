// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

/// Captured from a real DGX Spark.
const MEMINFO: &str =
    "MemTotal:       127601452 kB\nMemFree:        45557560 kB\nMemAvailable:   115669068 kB\n";

#[test]
fn a_real_gb10_meminfo_parses_to_roughly_119_gib() {
    let m = parse_meminfo(MEMINFO);
    assert_eq!(m.total_bytes, Some(127_601_452 * 1024));
    let gib = m.total_bytes.unwrap_or(0) as f64 / (1024.0 * 1024.0 * 1024.0);
    assert!((118.0..=123.0).contains(&gib), "got {gib} GiB");
}

#[test]
fn used_is_derived_from_available_not_free() {
    // MemFree would say ~78 GiB used on this box; MemAvailable says ~11.
    let m = parse_meminfo(MEMINFO);
    assert_eq!(m.used_bytes, Some((127_601_452 - 115_669_068) * 1024));
}

#[test]
fn missing_meminfo_fields_yield_absences_rather_than_zeros() {
    assert_eq!(parse_meminfo(""), MemInfo::default());
    assert_eq!(parse_meminfo("MemTotal:  100 kB\n").used_bytes, None);
}

#[test]
fn lscpu_models_are_read_in_order_once_each() {
    let arm = "Architecture:  aarch64\nVendor ID:  ARM\nModel name:  Cortex-X925\n\
               Model:  1\nModel name:  Cortex-A725\nModel name:  Cortex-X925\n";
    assert_eq!(
        parse_lscpu_model(arm).as_deref(),
        Some("Cortex-X925 + Cortex-A725")
    );
    let x86 = "Model name:                      Intel(R) Xeon(R) Platinum 8480+\n";
    assert_eq!(
        parse_lscpu_model(x86).as_deref(),
        Some("Intel(R) Xeon(R) Platinum 8480+")
    );
    assert_eq!(parse_lscpu_model("Architecture: x86_64\n"), None);
    // lscpu prints `-` for a model it cannot name.
    assert_eq!(parse_lscpu_model("Model name: -\n"), None);
}

#[test]
fn unified_memory_is_measured_from_the_probes_not_assumed() {
    assert_eq!(memory_is_unified(true, None, true), Some(true));
    assert_eq!(memory_is_unified(true, Some(1), true), Some(false));
    assert_eq!(memory_is_unified(true, Some(1), false), Some(false));
    assert_eq!(memory_is_unified(true, None, false), None);
    assert_eq!(memory_is_unified(false, None, true), None);
}
