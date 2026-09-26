// SPDX-License-Identifier: AGPL-3.0-only

//! The vendored corpus must parse. Every recipe, every time.
//!
//! This is what makes recipe-only pull requests earn their CI run: a malformed
//! or unloadable recipe fails the build rather than failing on someone's box.

use metralectl_core::flags;
use metralectl_core::recipe::{NotLaunchable, Provenance, Recipe, RuntimeKind};
use std::collections::BTreeSet;

fn load_all() -> Vec<(String, Result<Recipe, metralectl_core::RecipeError>)> {
    metrale_recipes_data::all()
        .into_iter()
        .map(|e| {
            let prov = Provenance::Builtin {
                path: e.path.to_string(),
            };
            (e.name.to_string(), Recipe::parse(e.name, e.yaml, prov))
        })
        .collect()
}

#[test]
fn every_vendored_recipe_parses() {
    let mut failures = Vec::new();
    for (name, result) in load_all() {
        if let Err(e) = result {
            failures.push(format!("  {name}: {e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "recipes failed to parse:\n{}",
        failures.join("\n")
    );
}

#[test]
fn recipes_carrying_executable_content_load_but_never_launch() {
    // Two legacy diffusion-gemma recipes carry `mods:`, which copies a directory
    // into the container and runs its script. They must remain visible — a
    // recipe that vanishes from `recipe list` is harder to reason about than one
    // that says why it will not run — while being firmly unlaunchable.
    let mut carriers = Vec::new();
    for (name, result) in load_all() {
        let r = result.expect("every vendored recipe must load");
        if r.refused_keys.is_empty() {
            continue;
        }
        carriers.push(name.clone());
        assert!(
            matches!(r.launchable(), Err(NotLaunchable::ExecutableContent(_))),
            "{name} carries {:?} yet reports launchable",
            r.refused_keys
        );
    }
    carriers.sort();
    assert_eq!(
        carriers,
        ["diffusion-gemma-bf16", "diffusion-gemma-fp8-dynamic"],
        "the set of recipes carrying executable content changed — that needs a \
         deliberate decision, not a quiet update"
    );
}

#[test]
fn the_corpus_contains_launchable_metrale_recipes() {
    let loaded: Vec<Recipe> = load_all().into_iter().filter_map(|(_, r)| r.ok()).collect();
    let launchable = loaded.iter().filter(|r| r.is_launchable()).count();
    assert!(
        launchable >= 25,
        "expected the metrale lineup, found {launchable} launchable"
    );

    // The legacy diffusion-gemma pair declares no runtime and must not be
    // mistaken for launchable.
    for r in loaded
        .iter()
        .filter(|r| r.name.starts_with("diffusion-gemma"))
    {
        assert!(!r.is_launchable(), "{} must not be launchable", r.name);
        assert_eq!(r.runtime, RuntimeKind::Unsupported(String::new()));
    }
}

#[test]
fn multi_node_recipes_are_exactly_the_ep2_lineup() {
    let mut multi: Vec<String> = load_all()
        .into_iter()
        .filter_map(|(_, r)| r.ok())
        .filter(|r| r.topology.is_multi_node())
        .map(|r| r.name)
        .collect();
    multi.sort();
    assert_eq!(
        multi,
        [
            "deepseek-v4-flash-nvfp4-ep2",
            "minimax-m2.7-nvfp4-ep2",
            "qwen3.5-122b-a10b-nvfp4-ep2",
        ],
        "the set of multi-node recipes changed"
    );
}

#[test]
fn no_launchable_recipe_sets_both_aliases_of_the_prefill_flag() {
    for r in load_all()
        .into_iter()
        .filter_map(|(_, r)| r.ok())
        .filter(Recipe::is_launchable)
    {
        flags::validate_no_alias_conflict(&r.defaults)
            .unwrap_or_else(|e| panic!("{}: {e}", r.name));
    }
}

/// Report — not assert — which recipe settings the flag table does not claim.
///
/// These are silently discarded by the reference implementation today. The test
/// passes so the corpus stays shippable, but it prints the inventory so the
/// follow-up work has a current list rather than a stale one.
#[test]
fn report_recipe_settings_the_flag_table_does_not_claim() {
    let mut rows: Vec<(String, Vec<String>)> = Vec::new();
    for r in load_all()
        .into_iter()
        .filter_map(|(_, r)| r.ok())
        .filter(Recipe::is_launchable)
    {
        let (_, unmapped) = flags::render(&r.defaults, &[]);
        if !unmapped.is_empty() {
            rows.push((
                r.name.clone(),
                unmapped.into_iter().map(|u| u.key).collect(),
            ));
        }
    }
    if !rows.is_empty() {
        eprintln!("\nrecipe settings not claimed by the flag table (not applied at launch):");
        for (name, keys) in &rows {
            eprintln!("  {name}: {}", keys.join(", "));
        }
        eprintln!();
    }
}

/// Every `METRALE_*` name a shipped recipe mentions, in its `env:` block or its
/// prose, is a lever the engine declares.
///
/// `met serve` refuses to start with an undeclared `METRALE_*` variable in its
/// environment, so a recipe that tells an operator to export one describes a
/// launch that cannot happen. The declared set is the lever table in the
/// vendored snapshot, which `met dump-serve-options` writes.
#[test]
fn every_lever_a_recipe_names_is_one_the_engine_declares() {
    let snapshot: serde_json::Value =
        serde_json::from_str(include_str!("../../../vendor/serve-options.v2.json"))
            .expect("the vendored snapshot is JSON");
    let declared: BTreeSet<&str> = snapshot["levers"]
        .as_array()
        .expect("the snapshot carries a lever table")
        .iter()
        .map(|l| l["env"].as_str().expect("every lever has a name"))
        .collect();
    assert!(
        declared.contains("METRALE_HOME"),
        "the lever table is not the one this test was written against"
    );
    let mut undeclared = Vec::new();
    for e in metrale_recipes_data::all() {
        for name in lever_names(e.yaml) {
            if !declared.contains(name) {
                undeclared.push(format!("  {}: {name}", e.name));
            }
        }
    }
    assert!(
        undeclared.is_empty(),
        "recipes name METRALE_* variables the engine does not declare:\n{}",
        undeclared.join("\n")
    );
}

/// The `METRALE_[A-Z0-9_]+` words in `text`.
fn lever_names(text: &str) -> Vec<&str> {
    const PREFIX: &str = "METRALE_";
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = Vec::new();
    for (at, _) in text.match_indices(PREFIX) {
        if text[..at].chars().next_back().is_some_and(word) {
            continue;
        }
        let rest = &text[at + PREFIX.len()..];
        let len = rest
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len());
        if len > 0 {
            out.push(&text[at..at + PREFIX.len() + len]);
        }
    }
    out
}
