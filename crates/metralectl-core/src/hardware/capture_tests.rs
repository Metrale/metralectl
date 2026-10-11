// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

/// In the CSV shape `nvidia-smi --format=csv,noheader,nounits` prints. The
/// GB10 line carries the values this repository already records for that
/// part (driver 580.126.09, compute capability 12.1, no framebuffer).
const GB10: &str = "NVIDIA GB10, 580.126.09, 1, 12.1, [N/A]\n";
const TWO_H100: &str = "NVIDIA H100 80GB HBM3, 580.126.09, 2, 9.0, 81559\n\
                        NVIDIA H100 80GB HBM3, 580.126.09, 2, 9.0, 81559\n";

#[test]
fn a_gb10_identity_has_no_framebuffer_and_an_sm_121_target() {
    let gpus = parse_nvidia_identity(GB10);
    assert_eq!(
        gpus,
        vec![NvidiaGpu {
            name: Some("NVIDIA GB10".into()),
            driver: Some("580.126.09".into()),
            count: Some(1),
            compute_cap: Some((12, 1)),
            memory_total_bytes: None,
        }]
    );
    assert_eq!(sm_architecture((12, 1)), "sm_121");
}

#[test]
fn a_discrete_card_reports_its_framebuffer_per_line() {
    let gpus = parse_nvidia_identity(TWO_H100);
    assert_eq!(gpus.len(), 2);
    assert_eq!(gpus[0].memory_total_bytes, Some(81559 * 1024 * 1024));
    assert_eq!(gpus[0].count, Some(2));
    assert_eq!(sm_architecture((9, 0)), "sm_90");
}

#[test]
fn every_declined_field_is_absent_never_zero_or_a_default() {
    let gpus = parse_nvidia_identity("[N/A], [Not Supported], N/A, [N/A], 0\n");
    assert_eq!(gpus, vec![NvidiaGpu::default()]);
    assert!(parse_nvidia_identity("").is_empty());
}

#[test]
fn the_cuda_version_comes_from_the_banner() {
    assert_eq!(
        parse_cuda_version(
            "| NVIDIA-SMI 580.126.09    Driver Version: 580.126.09    CUDA Version: 13.0     |"
        ),
        Some("13.0".into())
    );
    assert_eq!(parse_cuda_version("no banner"), None);
    assert_eq!(parse_cuda_version("CUDA Version: N/A |"), None);
}

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
