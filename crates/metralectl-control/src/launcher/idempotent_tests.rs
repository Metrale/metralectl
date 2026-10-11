// SPDX-License-Identifier: MIT OR Apache-2.0

//! The pure launch/stop decisions, from literal docker output.

use super::*;
use std::collections::BTreeMap;

fn row(id: &str, name: &str, recipe: Option<&str>) -> PsRow {
    PsRow {
        id: id.to_owned(),
        name: name.to_owned(),
        recipe: recipe.map(str::to_owned),
        deployment: None,
        generation: None,
    }
}

/// A full 64-hex container id whose first half is `half` repeated.
fn full_id(half: char) -> String {
    format!("{}{}", half.to_string().repeat(32), "0".repeat(32))
}

fn owned(deployment: &str, generation: u64) -> Owner {
    Owner {
        deployment: DeploymentName::parse(deployment).expect("label"),
        generation: Generation::new(generation).expect("non-zero"),
    }
}

fn plan() -> DockerCommand {
    DockerCommand {
        detach: true,
        entrypoint: None,
        privileged: false,
        device_flags: Vec::new(),
        ipc: "host".to_owned(),
        shm_size: "16g".to_owned(),
        network: "host".to_owned(),
        user: None,
        security_opts: Vec::new(),
        cap_add: Vec::new(),
        ulimits: Vec::new(),
        devices: Vec::new(),
        memory: None,
        labels: Vec::new(),
        auto_remove: false,
        restart: None,
        name: "metrale-r".to_owned(),
        env: BTreeMap::from([("HF_HUB_OFFLINE".to_owned(), "1".to_owned())]),
        volumes: BTreeMap::new(),
        image: "img:tag".to_owned(),
        command: ["met", "serve", "org/m", "--port", "8888"]
            .map(str::to_owned)
            .to_vec(),
    }
}

fn launched(image: &str, cmd: &[&str], env: &[&str]) -> LaunchedAs {
    LaunchedAs {
        image: image.to_owned(),
        cmd: Some(cmd.iter().map(|s| (*s).to_owned()).collect()),
        env: Some(env.iter().map(|s| (*s).to_owned()).collect()),
        labels: None,
    }
}

/// The template asks docker for exactly the five columns the parser reads,
/// with each label by its constant. Sabotage: reordering the columns in
/// either place reads a deployment as a generation.
#[test]
fn the_ps_template_and_the_parser_agree() {
    assert_eq!(
        ps_format(),
        "{{.ID}}\t{{.Names}}\t{{.Label \"io.metralectl.recipe\"}}\t\
         {{.Label \"io.metralectl.deployment\"}}\t{{.Label \"io.metralectl.generation\"}}"
    );
    let rows = parse_ps("abc\tmetrale-r\tr\tchat\t3\ndef\tsomeone-else\t\t\t\n\n").expect("parses");
    let mut ours = row("abc", "metrale-r", Some("r"));
    ours.deployment = Some("chat".into());
    ours.generation = Some("3".into());
    assert_eq!(rows, vec![ours, row("def", "someone-else", None)]);
}

/// The owner is both labels, well formed, or nobody. Sabotage: reading the
/// deployment alone makes a half-labelled container this deployment's, and a
/// later stop reaches it.
#[test]
fn an_owner_needs_both_labels_well_formed() {
    assert_eq!(owner_of(Some("chat"), Some("3")), Some(owned("chat", 3)));
    for (d, g) in [
        (Some("chat"), None),
        (None, Some("3")),
        (Some("chat"), Some("0")),
        (Some("chat"), Some("x")),
        (Some("Chat"), Some("3")),
    ] {
        assert_eq!(owner_of(d, g), None, "{d:?} {g:?}");
    }
    let labelled = LaunchedAs {
        labels: Some(BTreeMap::from(owner_labels(&owned("chat", 3)))),
        ..launched("img:tag", &[], &[])
    };
    assert!(is_owned_by(&labelled, &owned("chat", 3)));
    assert!(!is_owned_by(&labelled, &owned("chat", 4)));
    assert!(!is_owned_by(&labelled, &owned("other", 3)));
    assert!(!is_owned_by(
        &launched("img:tag", &[], &[]),
        &owned("chat", 3)
    ));
}

/// What the reconciler observes: every container a stop of its recipe would
/// reach, with its owner, and an instance id that is the container id's first
/// half. Sabotage: dropping the unlabelled solo-name container hides a
/// workload a stop would still catch.
#[test]
fn workloads_are_what_a_stop_could_reach_with_their_owners() {
    let mut ours = row(&full_id('a'), "metrale-r", Some("r"));
    ours.deployment = Some("chat".into());
    ours.generation = Some("2".into());
    let rows = [
        ours,
        row(&full_id('b'), "metrale-q", None),
        row(&full_id('c'), "someone-elses", None),
        // The solo name of recipe `q-old`, which a stop of `q-old` reaches.
        row(&full_id('d'), "metrale-q-old", None),
        row(&full_id('e'), "metrale-Q", None),
    ];
    let seen = workloads(&rows).expect("full ids");
    let summary: Vec<(String, String, Option<Owner>)> = seen
        .iter()
        .map(|w| {
            (
                w.id.to_string(),
                w.instance.recipe.to_string(),
                w.instance.owner.clone(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("a".repeat(32), "r".to_owned(), Some(owned("chat", 2))),
            ("b".repeat(32), "q".to_owned(), None),
            ("d".repeat(32), "q-old".to_owned(), None),
        ]
    );
    assert!(workloads(&[row("abc123", "metrale-r", Some("r"))]).is_err());
}

/// A line docker should never print is an error, not a skipped row: a stop
/// decided from a half-read listing could miss the container it was for.
#[test]
fn an_unreadable_ps_line_is_an_error_not_a_skip() {
    assert!(parse_ps("abc\n").is_err());
    assert!(parse_ps("\tname\tr\n").is_err());
}

/// Docker's own config JSON, as `{{json .Config}}` prints it (abridged).
#[test]
fn an_inspected_config_is_read_from_dockers_json() {
    let json = r#"{"Hostname":"h","Image":"img:tag","Cmd":["met","serve"],"Env":["PATH=/bin","K=v"],"Labels":{}}"#;
    assert_eq!(
        parse_inspect(json).expect("parses"),
        LaunchedAs {
            labels: Some(BTreeMap::new()),
            ..launched("img:tag", &["met", "serve"], &["PATH=/bin", "K=v"])
        }
    );
    assert!(parse_inspect("not json").is_err());
}

/// The same image, command and plan variables is the same launch; the
/// image's own extra variables do not make it different.
#[test]
fn the_identical_launch_is_recognised_despite_image_variables() {
    let same = launched(
        "img:tag",
        &["met", "serve", "org/m", "--port", "8888"],
        &["PATH=/usr/bin", "HF_HUB_OFFLINE=1"],
    );
    assert!(is_same_launch(&plan(), &same));
}

/// Any difference in what was asked for is a different launch. Sabotage:
/// comparing names only (or dropping any one clause) adopts one of these.
#[test]
fn a_different_port_image_or_variable_is_a_different_launch() {
    let cmd = ["met", "serve", "org/m", "--port", "8888"];
    let other_port = launched(
        "img:tag",
        &["met", "serve", "org/m", "--port", "9100"],
        &["HF_HUB_OFFLINE=1"],
    );
    let other_image = launched("img:other", &cmd, &["HF_HUB_OFFLINE=1"]);
    let other_env = launched("img:tag", &cmd, &["HF_HUB_OFFLINE=0"]);
    let no_cmd = LaunchedAs {
        cmd: None,
        ..launched("img:tag", &[], &["HF_HUB_OFFLINE=1"])
    };
    for different in [other_port, other_image, other_env, no_cmd] {
        assert!(!is_same_launch(&plan(), &different), "{different:?}");
    }
}

#[test]
fn the_endpoint_follows_the_port_and_a_worker_serves_nothing() {
    let cmd = |port: &str| -> Vec<String> {
        ["met", "serve", "--port", port].map(str::to_owned).to_vec()
    };
    assert_eq!(
        endpoint_of(&cmd("9100")).as_deref(),
        Some("http://localhost:9100/v1")
    );
    assert_eq!(endpoint_of(&cmd("0")), None);
    assert_eq!(endpoint_of(&["met".to_owned()]), None);
}

#[test]
fn only_the_exact_container_name_collides() {
    let rows = [
        row("a", "metrale-r-rank0", Some("r")),
        row("b", "metrale-r", Some("r")),
    ];
    assert_eq!(
        running_as("metrale-r", &rows).map(|r| r.id.as_str()),
        Some("b")
    );
    assert_eq!(running_as("metrale-q", &rows), None);
}

/// Stop reaches every labelled container of the recipe — ranks included —
/// and an unlabelled one only by its exact solo name. Sabotage: matching the
/// name by prefix catches `metrale-r-old` and `metrale-rx`.
#[test]
fn stop_targets_are_the_recipes_containers_and_nothing_else() {
    let rows = [
        row("rank0", "metrale-r-rank0", Some("r")),
        row("rank1", "metrale-r-rank1", Some("r")),
        row("old", "metrale-r", None),
        row("lookalike", "metrale-r-old", None),
        row("prefix", "metrale-rx", None),
        row("other", "metrale-q", Some("q")),
        row("renamed", "metrale-r", Some("q")),
    ];
    assert_eq!(stop_targets("r", &rows), vec!["rank0", "rank1", "old"]);
}

/// Nothing of the recipe's running: nothing to stop, which `Stop` reports as
/// done. Sabotage: falling back to the bare name here re-creates the old
/// `docker stop metrale-r` that failed when nothing was there.
#[test]
fn a_recipe_with_nothing_running_has_no_stop_targets() {
    let rows = [row("other", "metrale-q", Some("q"))];
    assert!(stop_targets("r", &rows).is_empty());
    assert!(stop_targets("r", &[]).is_empty());
}
