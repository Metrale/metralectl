// SPDX-License-Identifier: MIT OR Apache-2.0

//! The vendored platform-contract bundle (`schemas/platform/`, investor-mvp #41)
//! is the unchanged copy its pin names, and the agent's binding agrees with
//! every fixture of the document kinds the agent reads or writes: each `valid`
//! fixture parses and re-serializes to the canonical JSON the index records;
//! each `invalid` fixture is refused. Every valid fixture of every kind also
//! reproduces its `jcs_sha256`, which holds this canonical JSON to the
//! platform's. Every closed word the agent binds is the schema's.

use metralectl_control::platform::action::{
    ActionKind, ActionOutcome, DeploymentAction, DeploymentActionResult, RefusalReason,
};
use metralectl_control::platform::canonical::{digest, sha256};
use metralectl_control::platform::deployment::ModelDeployment;
use metralectl_control::platform::fingerprint::HardwareFingerprint;
use metralectl_control::platform::release::ReleaseManifest;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The kinds this repository binds to its own types.
const BOUND: [&str; 5] = [
    "deployment-action",
    "deployment-action-result",
    "model-deployment",
    "hardware-fingerprint",
    "release-manifest",
];

fn bundle() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/platform")
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn json(path: &Path) -> Value {
    serde_json::from_slice(&read(path)).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn pin(key: &str) -> String {
    let text = String::from_utf8(read(&bundle().with_extension("pin"))).expect("utf-8");
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix(&format!("{key} = ")))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_else(|| panic!("platform.pin has no {key}"))
}

fn files_under(dir: &Path, root: &Path, out: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            files_under(&path, root, out);
        } else {
            let rel = path
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy();
            out.insert(rel.replace('\\', "/"));
        }
    }
}

#[test]
fn the_copy_is_the_pinned_bundle_unchanged() {
    let root = bundle();
    let sums = read(&root.join("SHA256SUMS"));
    assert_eq!(
        sha256(&sums).to_string(),
        pin("sha256sums"),
        "SHA256SUMS differs from the pin"
    );
    assert_eq!(pin("mfs_commit").len(), 40);
    let mut listed = BTreeSet::new();
    for line in String::from_utf8(sums).expect("utf-8").lines() {
        let (want, file) = line.split_once("  ").expect("sha256sum line");
        let got = sha256(&read(&root.join(file))).to_string();
        assert_eq!(got, want, "{file} was edited");
        listed.insert(file.to_string());
    }
    let mut present = BTreeSet::new();
    files_under(&root, &root, &mut present);
    present.remove("SHA256SUMS");
    assert_eq!(
        present, listed,
        "a file in the copy is missing from SHA256SUMS, or the reverse"
    );
}

/// `Ok` with the canonical digest of the re-serialized document when the
/// binding accepts `doc` as `kind`; `Err` naming why it refuses it.
fn bind(kind: &str, doc: &Value) -> Result<String, String> {
    fn take<T: serde::de::DeserializeOwned + serde::Serialize>(
        doc: &Value,
    ) -> Result<String, String> {
        let typed: T = serde_json::from_value(doc.clone()).map_err(|e| e.to_string())?;
        digest(&typed).map(|d| d.to_string())
    }
    match kind {
        "deployment-action" => take::<DeploymentAction>(doc),
        "deployment-action-result" => take::<DeploymentActionResult>(doc),
        "model-deployment" => take::<ModelDeployment>(doc),
        "hardware-fingerprint" => take::<HardwareFingerprint>(doc),
        "release-manifest" => take::<ReleaseManifest>(doc),
        other => panic!("{other} is not bound"),
    }
}

#[test]
fn the_binding_agrees_with_every_fixture_verdict() {
    let root = bundle().join("v1");
    let index = json(&root.join("INDEX.json"));
    let mut seen_valid = BTreeSet::new();
    let mut seen_invalid = BTreeSet::new();
    for entry in index["valid"].as_array().expect("valid list") {
        let file = entry["file"].as_str().expect("file");
        let kind = entry["schema"].as_str().expect("schema");
        let want = entry["jcs_sha256"].as_str().expect("digest");
        let doc = json(&root.join(file));
        assert_eq!(
            digest(&doc).expect("integers").to_string(),
            want,
            "canonical JSON of {file}"
        );
        if BOUND.contains(&kind) {
            let got = bind(kind, &doc).unwrap_or_else(|e| panic!("{file} refused: {e}"));
            assert_eq!(got, want, "{file} does not round-trip through the binding");
            seen_valid.insert(kind);
        }
    }
    for entry in index["invalid"].as_array().expect("invalid list") {
        let file = entry["file"].as_str().expect("file");
        let kind = entry["schema"].as_str().expect("schema");
        if BOUND.contains(&kind) {
            let rule = entry["rule"].as_str().expect("rule");
            assert!(
                bind(kind, &json(&root.join(file))).is_err(),
                "{file} breaks {rule} but was accepted"
            );
            seen_invalid.insert(kind);
        }
    }
    let all: BTreeSet<&str> = BOUND.into_iter().collect();
    assert_eq!(
        seen_valid, all,
        "a bound kind has no valid fixture in the index"
    );
    assert_eq!(
        seen_invalid, all,
        "a bound kind has no invalid fixture in the index"
    );
}

/// The words of the closed enum at `pointer` in schema `file`.
fn schema_words(file: &str, pointer: &str) -> BTreeSet<String> {
    json(&bundle().join("v1").join(file))
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{file} has no enum at {pointer}"))
        .iter()
        .map(|w| w.as_str().expect("a word").to_owned())
        .collect()
}

fn word<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|w| w.as_str().map(str::to_owned))
        .expect("a unit variant serializes as a word")
}

/// Every variant: the match has no wildcard, so a new variant fails to compile
/// until it is listed here too.
fn every_reason() -> Vec<RefusalReason> {
    use RefusalReason as R;
    let all = [
        R::StaleGeneration,
        R::IdempotencyConflict,
        R::ReleaseUnverified,
        R::ReceiptMissing,
        R::HardwareIncompatible,
        R::NotHere,
        R::Busy,
    ];
    for r in all {
        match r {
            R::StaleGeneration
            | R::IdempotencyConflict
            | R::ReleaseUnverified
            | R::ReceiptMissing
            | R::HardwareIncompatible
            | R::NotHere
            | R::Busy => {}
        }
    }
    all.to_vec()
}

fn every_action() -> Vec<ActionKind> {
    let all = [ActionKind::Apply, ActionKind::Stop, ActionKind::Drain];
    for a in all {
        match a {
            ActionKind::Apply | ActionKind::Stop | ActionKind::Drain => {}
        }
    }
    all.to_vec()
}

#[test]
fn the_refusal_reasons_and_actions_are_exactly_the_schema_words() {
    let reasons = schema_words(
        "deployment-action-result.v1.schema.json",
        "/properties/outcome/oneOf/2/properties/refused/properties/reason/enum",
    );
    let ours: BTreeSet<String> = every_reason().iter().map(word).collect();
    assert_eq!(ours, reasons);
    let actions = schema_words(
        "deployment-action.v1.schema.json",
        "/properties/action/enum",
    );
    let ours: BTreeSet<String> = every_action().iter().map(word).collect();
    assert_eq!(ours, actions);
    for unit in ["applied", "already_applied"] {
        let outcome: ActionOutcome =
            serde_json::from_value(Value::String(unit.into())).expect("a schema outcome");
        assert_eq!(word(&outcome), unit);
    }
}

#[test]
fn every_other_closed_word_of_a_bound_kind_parses() {
    use metralectl_control::platform::deployment::{
        ConditionKind, ConditionStatus, HealthState, Phase,
    };
    use metralectl_control::platform::fingerprint::{CpuArch, Vendor};
    use metralectl_control::platform::release::SbomFormat;
    fn each<T: serde::de::DeserializeOwned + serde::Serialize>(file: &str, pointer: &str) {
        for w in schema_words(file, pointer) {
            let parsed: T = serde_json::from_value(Value::String(w.clone()))
                .unwrap_or_else(|e| panic!("{w} ({file} {pointer}): {e}"));
            assert_eq!(word(&parsed), w);
        }
    }
    let status = "/properties/status/oneOf/1/properties";
    each::<Phase>(
        "model-deployment.v1.schema.json",
        &format!("{status}/phase/enum"),
    );
    each::<ConditionKind>(
        "model-deployment.v1.schema.json",
        &format!("{status}/conditions/items/properties/kind/enum"),
    );
    each::<ConditionStatus>(
        "model-deployment.v1.schema.json",
        &format!("{status}/conditions/items/properties/status/enum"),
    );
    each::<HealthState>("common.v1.schema.json", "/$defs/health_state/enum");
    each::<Vendor>(
        "hardware-fingerprint.v1.schema.json",
        "/properties/accelerators/items/properties/vendor/enum",
    );
    each::<CpuArch>(
        "hardware-fingerprint.v1.schema.json",
        "/properties/host/properties/cpu_arch/enum",
    );
    each::<SbomFormat>(
        "release-manifest.v1.schema.json",
        "/properties/sbom/properties/format/enum",
    );
}
