// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn gb10() -> HardwareClass {
    HardwareClass::parse("gb10").expect("valid")
}

#[test]
fn the_family_is_classified_not_one_exact_string() {
    // A rename or a variant must keep the class rather than lose it.
    for name in [
        "NVIDIA GB10",
        "nvidia gb10 superchip",
        "NVIDIA Grace Blackwell GB10",
    ] {
        assert_eq!(classify(name), Some(gb10()), "{name}");
    }
    assert_eq!(classify("NVIDIA H100 80GB HBM3"), None);
    assert_eq!(classify(""), None);
}

#[test]
fn the_required_class_on_that_class_passes() {
    assert_eq!(check(&gb10(), "NVIDIA GB10"), Ok(()));
}

#[test]
fn another_class_is_refused_naming_both() {
    let other = HardwareClass::parse("strix-halo").expect("valid");
    let why = check(&other, "NVIDIA GB10").expect_err("a GB10 is not a Strix Halo");
    assert!(why.contains("strix-halo") && why.contains("gb10"), "{why}");
}

#[test]
fn an_unknown_machine_meets_no_requirement() {
    // Fail closed: "unknown" is not evidence of being the required hardware.
    let why = check(&gb10(), "NVIDIA H100 80GB HBM3").expect_err("unknown hardware");
    assert!(why.contains("H100"), "{why}");
    let why = check(&gb10(), "").expect_err("no accelerator at all");
    assert!(why.contains("no accelerator"), "{why}");
}

#[test]
fn a_class_is_a_dns_label() {
    for good in ["gb10", "strix-halo", "a", "x1-2"] {
        assert!(HardwareClass::parse(good).is_ok(), "{good}");
    }
    let long = "a".repeat(64);
    for bad in [
        "",
        "GB10",
        "-gb10",
        "gb10-",
        "gb_10",
        "gb 10",
        long.as_str(),
    ] {
        assert!(HardwareClass::parse(bad).is_err(), "{bad:?}");
    }
}
