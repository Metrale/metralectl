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
