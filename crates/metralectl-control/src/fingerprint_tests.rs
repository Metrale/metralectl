// SPDX-License-Identifier: MIT OR Apache-2.0

//! The capture against tool output, and against the platform's own fixture.

use super::*;
use crate::platform::canonical::digest;
use metralectl_core::io::process::Output;
use metralectl_core::io::{MemFileSystem, RecordingRunner};

/// Tool output in each tool's own shape, carrying the values of the bundle's
/// `hardware-fingerprint/valid/gb10.json` (whose CPU model and memory are the
/// fixture's synthetic ones).
fn gb10_outputs() -> ToolOutputs {
    ToolOutputs {
        nvidia_identity: Some("NVIDIA GB10, 580.95.05, 1, 12.1, [N/A]\n".into()),
        nvidia_banner: Some(
            "| NVIDIA-SMI 580.95.05    Driver Version: 580.95.05    CUDA Version: 13.0     |\n"
                .into(),
        ),
        meminfo: Some(
            "MemTotal:       125829120 kB\nMemFree:  1 kB\nMemAvailable:   100 kB\n".into(),
        ),
        lscpu: Some("Architecture:  aarch64\nModel name:  Grace 20-core\n".into()),
        os_kernel: Some("6.17.0\n".into()),
        cpu_arch: "aarch64".into(),
    }
}

fn fixture() -> HardwareFingerprint {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas/platform/v1/fixtures/hardware-fingerprint/valid/gb10.json");
    serde_json::from_slice(&std::fs::read(path).expect("the vendored fixture")).expect("valid")
}

/// A GB10's tools produce exactly the platform's GB10 fingerprint, byte for
/// byte in canonical JSON. Sabotage: taking the accelerator memory from a
/// framebuffer the part does not have leaves it absent, and the unified flag
/// assumed rather than measured is caught by the next test.
#[test]
fn a_gb10_captures_the_platforms_own_gb10_fingerprint() {
    let fp = capture(&gb10_outputs())
        .fingerprint()
        .expect("fully measured");
    assert_eq!(fp, fixture());
    assert_eq!(digest(&fp), digest(&fixture()));
}

/// A discrete card's memory is its framebuffer and the flag is false; two
/// cards of one kind are one entry with a count of two.
#[test]
fn a_discrete_host_is_measured_not_unified_and_counted_per_kind() {
    let mut out = gb10_outputs();
    out.nvidia_identity = Some(
        "NVIDIA H100 80GB HBM3, 580.95.05, 2, 9.0, 81559\n\
         NVIDIA H100 80GB HBM3, 580.95.05, 2, 9.0, 81559\n"
            .into(),
    );
    let c = capture(&out);
    assert_eq!(c.unified_memory, Some(false));
    assert_eq!(c.accelerators.len(), 1);
    assert_eq!(c.accelerators[0].count, 2);
    assert_eq!(c.accelerators[0].memory_bytes, Some(81559 * 1024 * 1024));
    assert_eq!(c.accelerators[0].architecture.as_deref(), Some("sm_90"));
    // No class this build knows: the document is not produced, rather than
    // produced with a guessed class.
    assert_eq!(c.fingerprint(), Err(vec!["hardware_class".to_owned()]));
}

/// What identifies the hardware is required: without it no document is
/// made, and each missing identifying field is named. Sabotage: a default (an
/// empty class, `false` for the unified flag) in place of an absent value
/// produces a document here.
#[test]
fn a_machine_without_its_identity_makes_no_document() {
    let c = capture(&ToolOutputs {
        cpu_arch: "riscv64".into(),
        ..ToolOutputs::default()
    });
    assert!(c.accelerators.is_empty());
    assert_eq!(c.unified_memory, None);
    assert_eq!(
        c.fingerprint(),
        Err([
            "accelerators",
            "hardware_class",
            "host/cpu_arch",
            "host/unified_memory"
        ]
        .map(str::to_owned)
        .to_vec())
    );

    let mut no_meminfo = gb10_outputs();
    no_meminfo.meminfo = None;
    let c = capture(&no_meminfo);
    assert_eq!(c.unified_memory, None, "no framebuffer and no host figure");
    assert_eq!(c.fingerprint(), Err(vec!["host/unified_memory".to_owned()]));
}

/// A quantity a tool did not answer is written absent, never zero, and the
/// document is still made, with a digest of its own (DECISIONS PC-V).
/// Sabotage: refusing a document with any absent quantity (the pre-PC-V
/// producer) fails the `expect`.
#[test]
fn a_partly_measured_machine_has_a_document_with_absent_quantities() {
    let mut out = gb10_outputs();
    out.nvidia_banner = None;
    out.lscpu = None;
    let fp = capture(&out)
        .fingerprint()
        .expect("identity measured; quantities may be absent");
    let mut want = serde_json::to_value(fixture()).expect("serializes");
    want["toolkit"] = serde_json::json!({"absent": "unavailable"});
    want["host"]["cpu_model"] = serde_json::json!({"absent": "unavailable"});
    assert_eq!(serde_json::to_value(&fp).expect("serializes"), want);
    let back: HardwareFingerprint = serde_json::from_value(want).expect("a valid document");
    assert_eq!(back, fp);
    assert_ne!(
        digest(&fp),
        digest(&fixture()),
        "absence is part of the digest"
    );
}

/// The tools run are exactly these, and a failing one is absent.
#[test]
fn the_tools_are_read_through_the_ports() {
    let runner = RecordingRunner::new();
    let ok = |stdout: &str| Output {
        status: 0,
        stdout: stdout.into(),
        stderr: String::new(),
    };
    runner.push_result(ok("NVIDIA GB10, 580.95.05, 1, 12.1, [N/A]\n"));
    runner.push_result(Output {
        status: 9,
        stdout: "half a banner".into(),
        stderr: String::new(),
    });
    runner.push_result(ok("Model name: Grace 20-core\n"));
    runner.push_result(ok("6.17.0\n"));
    let fs = MemFileSystem::new();
    fs.insert("/proc/meminfo", "MemTotal: 4 kB\n");
    let out = ToolOutputs::read(&runner, &fs);
    assert_eq!(
        runner.calls(),
        vec![
            nvidia::nvidia_identity_argv(),
            vec!["nvidia-smi".to_owned()],
            vec!["lscpu".to_owned()],
            vec!["uname".to_owned(), "-r".to_owned()],
        ]
    );
    assert_eq!(out.nvidia_banner, None, "a failed run is not output");
    assert_eq!(out.meminfo.as_deref(), Some("MemTotal: 4 kB\n"));
    assert_eq!(out.os_kernel.as_deref(), Some("6.17.0\n"));
}
