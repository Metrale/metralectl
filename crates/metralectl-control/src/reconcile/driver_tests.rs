// SPDX-License-Identifier: MIT OR Apache-2.0

//! The driver over the real decision core, the real control core and the real
//! docker launcher. The one seam is the process runner: an in-memory docker
//! daemon (`fake_docker.rs`) that keeps containers and their labels as state
//! and refuses what docker refuses. A recording mock could not show that a
//! second apply finds the first container, because only state that outlives
//! one launcher can; every launcher here is built fresh per call, which is an
//! agent restart between every step.

use super::*;
use crate::admission::{Admission, AdmissionGate};
use crate::launcher::DockerLauncher;
use crate::launcher::docker::fake_docker::FakeDocker;
use crate::platform::ids::IdempotencyKey;
use crate::reconcile::contract::DeploymentName;
use crate::release::testing::verified_serving;
use metralectl_core::docker::profile::{NvidiaDevices, ROOTLESS_V1};
use metralectl_core::hardware::HardwareClass;
use metralectl_core::host::{HostSnapshot, PosixUser};
use metralectl_core::io::ProcessRunner;
use metralectl_core::registry::RegistrySet;
use std::sync::Arc;
use std::time::Duration;

/// Two real recipes of the compiled-in corpus, different models.
const A: &str = "qwen3.6-27b-fp8";
const B: &str = "qwen3.6-35b-a3b-fp8-mtp";
const GB10: &str = "NVIDIA GB10";

/// A Hugging Face cache holding both recipes' weights, so the launcher's
/// offline-cache guard passes without being weakened.
fn cache() -> &'static str {
    static ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let root =
            std::env::temp_dir().join(format!("metralectl-driver-cache-{}", std::process::id()));
        for model in ["Qwen/Qwen3.6-27B-FP8", "Qwen/Qwen3.6-35B-A3B-FP8"] {
            let snap = root
                .join("hub")
                .join(format!("models--{}", model.replace('/', "--")))
                .join("snapshots/deadbeef");
            std::fs::create_dir_all(&snap).expect("mkdir cache");
            std::fs::write(snap.join("config.json"), b"{}").expect("config");
            std::fs::write(snap.join("model.safetensors"), b"w").expect("weights");
        }
        root.display().to_string()
    })
}

/// One machine: a docker daemon that outlives every agent built over it.
struct Node {
    daemon: Arc<FakeDocker>,
    registry: RegistrySet,
    can_launch: Result<(), String>,
    admission: AdmissionGate,
}

impl Node {
    fn new() -> Self {
        Self {
            daemon: Arc::new(FakeDocker::new(Duration::ZERO)),
            registry: RegistrySet::builtin_only(),
            can_launch: Ok(()),
            admission: AdmissionGate::new(Admission::Open),
        }
    }

    /// The control core of a freshly started agent on this machine.
    fn agent(&self) -> LocalControl<'_> {
        let host = HostSnapshot {
            posix_user: Some(PosixUser {
                uid: 1000,
                gid: 1000,
            }),
            home: "/home/user".into(),
            hf_cache_dir: cache().to_owned(),
            env: BTreeMap::new(),
        };
        let runner = Arc::clone(&self.daemon) as Arc<dyn ProcessRunner>;
        LocalControl {
            registry: &self.registry,
            launcher: Arc::new(DockerLauncher::new(
                runner,
                host,
                &ROOTLESS_V1,
                Box::new(NvidiaDevices),
            )),
            telemetry: None,
            can_launch: &self.can_launch,
            accelerator: GB10,
            admission: &self.admission,
        }
    }

    /// A launch from the browser: outside any deployment.
    async fn browser_launch(&self, recipe: &str) {
        self.agent()
            .launch(
                &rid(recipe),
                &BTreeMap::new(),
                LaunchFor {
                    owner: None,
                    release: None,
                },
            )
            .await
            .expect("the browser launches");
    }

    /// Converge with the release a `Run` serves, verified as the host would.
    async fn converge(&self, desired: &Desired) -> Result<Answer, AgentError> {
        let release = match &desired.intent {
            Intent::Run { model, .. } => Some(verified_serving(model)),
            Intent::Stopped => None,
        };
        converge(&self.agent(), desired, release.as_ref(), steps()).await
    }

    fn label(&self, container: &str, key: &str) -> Option<String> {
        self.daemon
            .labels_of(container)
            .into_iter()
            .find_map(|(k, v)| (k == key).then_some(v))
    }
}

fn rid(s: &str) -> RecipeId {
    RecipeId::parse(s).expect("valid id")
}

fn steps() -> NonZeroU32 {
    NonZeroU32::new(4).expect("non-zero")
}

fn generation(n: u64) -> Generation {
    Generation::new(n).expect("non-zero")
}

fn gb10() -> Placement {
    Placement::HardwareClass(HardwareClass::parse("gb10").expect("class"))
}

fn target(recipe: &str, model: &str) -> Target {
    Target {
        recipe: rid(recipe),
        release: verified_serving(model),
        placement: gb10(),
    }
}

fn run(recipe: &str, model: &str, g: u64) -> Desired {
    Desired {
        deployment: DeploymentName::parse("chat").expect("label"),
        generation: generation(g),
        intent: Intent::Run {
            recipe: rid(recipe),
            model: model.to_owned(),
            placement: gb10(),
        },
    }
}

fn run_a(g: u64) -> Desired {
    run(A, "Qwen/Qwen3.6-27B-FP8", g)
}

fn stopped(g: u64) -> Desired {
    Desired {
        deployment: DeploymentName::parse("chat").expect("label"),
        generation: generation(g),
        intent: Intent::Stopped,
    }
}

const DEPLOYMENT: &str = "io.metralectl.deployment";
const GENERATION: &str = "io.metralectl.generation";

/// Sabotage: launching without the owner labels makes the second apply see a
/// foreign workload (`busy`), and the label assertions fail.
#[tokio::test]
async fn a_fresh_apply_launches_once_and_the_container_records_its_owner() {
    let node = Node::new();
    let answer = node.converge(&run_a(1)).await.expect("converges");
    assert_eq!(answer.outcome, ActionOutcome::Applied);
    assert_eq!(answer.running_generation, Some(generation(1)));
    assert_eq!(node.daemon.runs(), 1);
    let container = format!("metrale-{A}");
    assert_eq!(node.label(&container, DEPLOYMENT).as_deref(), Some("chat"));
    assert_eq!(node.label(&container, GENERATION).as_deref(), Some("1"));
    let seen = node.agent().workloads().await.expect("observes");
    assert_eq!(answer.instances, vec![seen[0].id]);
}

/// A release's image runs by its digest, never the recipe's tag. Sabotage:
/// leaving the recipe's image in place runs `:latest`.
#[tokio::test]
async fn the_release_image_runs_by_digest() {
    let node = Node::new();
    node.converge(&run_a(1)).await.expect("converges");
    let release = verified_serving("Qwen/Qwen3.6-27B-FP8");
    assert_eq!(
        node.daemon.image_of(&format!("metrale-{A}")),
        Some(release.image().reference())
    );
    assert!(release.image().reference().contains("@sha256:"));
}

/// Nothing is served without a verified release, and a release for another
/// model is no release for this one. Sabotage: launching on the recipe alone
/// when no release came runs an unverified image.
#[tokio::test]
async fn serving_without_a_matching_verified_release_is_refused_untouched() {
    let node = Node::new();
    let unverified = ActionOutcome::Refused(Refused {
        reason: RefusalReason::ReleaseUnverified,
    });
    let none = converge(&node.agent(), &run_a(1), None, steps()).await;
    assert_eq!(none.expect("answers").outcome, unverified);
    let other = verified_serving("Qwen/Qwen3.6-35B-A3B-FP8");
    let wrong = converge(&node.agent(), &run_a(1), Some(&other), steps()).await;
    assert_eq!(wrong.expect("answers").outcome, unverified);
    assert_eq!(node.daemon.runs(), 0);
    // The control core holds the same line for any caller.
    let err = node
        .agent()
        .launch(
            &rid(A),
            &BTreeMap::new(),
            LaunchFor {
                owner: None,
                release: Some(other),
            },
        )
        .await
        .expect_err("another model's release");
    assert!(matches!(err, AgentError::NotLaunchable { .. }), "{err:?}");
    assert_eq!(node.daemon.runs(), 0);
}

/// The same apply again, resent after a lost acknowledgement, or arriving at
/// a restarted agent: each observes the first container and starts nothing.
/// Sabotage: deciding from anything but the launcher's state (or not
/// observing at all) launches again, and docker refuses the name.
#[tokio::test]
async fn a_repeated_apply_after_a_lost_ack_or_a_restart_starts_nothing() {
    let node = Node::new();
    let first = node.converge(&run_a(1)).await.expect("converges");
    for _ in 0..2 {
        let again = node.converge(&run_a(1)).await.expect("converges");
        assert_eq!(again.outcome, ActionOutcome::AlreadyApplied);
        assert_eq!(again.instances, first.instances);
    }
    assert_eq!(node.daemon.runs(), 1, "one docker run for three applies");
}

/// A failed release (the image pull fails) leaves nothing, and the retry
/// launches exactly once. Sabotage: recording "launched" before the launch
/// succeeds answers the retry `already_applied` with nothing running.
#[tokio::test]
async fn a_failed_launch_leaves_nothing_and_the_retry_launches_once() {
    let node = Node::new();
    node.daemon.fail_next_run();
    let err = node.converge(&run_a(1)).await.expect_err("the pull failed");
    assert!(matches!(err, AgentError::LaunchFailed { .. }), "{err:?}");
    assert!(node.daemon.running().is_empty());
    let retry = node.converge(&run_a(1)).await.expect("converges");
    assert_eq!(retry.outcome, ActionOutcome::Applied);
    assert_eq!(node.daemon.running(), [format!("metrale-{A}")]);
}

/// A new generation replaces this deployment's own older one; a workload
/// the browser started keeps running throughout. Sabotage: a stop that is not
/// limited to this deployment's recipe takes the browser's workload with it.
#[tokio::test]
async fn a_new_generation_replaces_only_its_own_and_unrelated_work_runs_on() {
    let node = Node::new();
    node.browser_launch(B).await;
    node.converge(&run_a(1)).await.expect("generation 1");
    let second = node.converge(&run_a(2)).await.expect("generation 2");
    assert_eq!(second.outcome, ActionOutcome::Applied);
    assert_eq!(second.running_generation, Some(generation(2)));
    let mut running = node.daemon.running();
    running.sort();
    assert_eq!(running, [format!("metrale-{A}"), format!("metrale-{B}")]);
    assert_eq!(
        node.label(&format!("metrale-{A}"), GENERATION).as_deref(),
        Some("2")
    );
    assert_eq!(node.label(&format!("metrale-{B}"), DEPLOYMENT), None);
}

#[tokio::test]
async fn stop_stops_only_the_deployment_and_a_repeat_is_already_applied() {
    let node = Node::new();
    node.browser_launch(B).await;
    node.converge(&run_a(1)).await.expect("runs");
    let stop = node.converge(&stopped(2)).await.expect("stops");
    assert_eq!(stop.outcome, ActionOutcome::Applied);
    assert_eq!(stop.running_generation, None);
    assert!(stop.instances.is_empty());
    assert_eq!(node.daemon.running(), [format!("metrale-{B}")]);
    let again = node.converge(&stopped(2)).await.expect("nothing to do");
    assert_eq!(again.outcome, ActionOutcome::AlreadyApplied);
}

/// Another deployment's work is neither counted nor touched. Sabotage:
/// answering from every owned workload reports the other deployment's
/// generation and instance as this one's.
#[tokio::test]
async fn the_answer_names_only_this_deployments_instances() {
    let node = Node::new();
    let other = Desired {
        deployment: DeploymentName::parse("other").expect("label"),
        ..run(B, "Qwen/Qwen3.6-35B-A3B-FP8", 5)
    };
    node.converge(&other)
        .await
        .expect("the other deployment runs");
    let answer = node.converge(&run_a(1)).await.expect("converges");
    assert_eq!(answer.running_generation, Some(generation(1)));
    assert_eq!(answer.instances.len(), 1);
    assert_eq!(node.daemon.running().len(), 2);
}

/// The browser already runs the recipe: the deployment neither adopts it nor
/// launches beside it. Sabotage: adopting on the name alone (the launcher's
/// owner check) answers `applied` for a container that is not the
/// deployment's, and a later stop of the deployment would stop it.
#[tokio::test]
async fn a_recipe_the_browser_runs_is_busy_not_adopted() {
    let node = Node::new();
    node.browser_launch(A).await;
    let answer = node.converge(&run_a(1)).await.expect("answers");
    assert_eq!(
        answer.outcome,
        ActionOutcome::Refused(Refused {
            reason: RefusalReason::Busy
        })
    );
    assert_eq!(node.daemon.runs(), 1);
    assert_eq!(node.label(&format!("metrale-{A}"), DEPLOYMENT), None);
}

/// The owner check lives in the launcher too, below the reconciler: a launch
/// for one deployment does not adopt the identical container another runs.
#[tokio::test]
async fn the_launcher_does_not_adopt_another_deployments_container() {
    let node = Node::new();
    node.converge(&run_a(1)).await.expect("runs");
    let other = Owner {
        deployment: DeploymentName::parse("other").expect("label"),
        generation: generation(1),
    };
    let err = node
        .agent()
        .launch(
            &rid(A),
            &BTreeMap::new(),
            LaunchFor {
                owner: Some(other),
                release: None,
            },
        )
        .await
        .expect_err("not adopted");
    assert!(matches!(err, AgentError::AlreadyRunning { .. }), "{err:?}");
}

/// A cordoned node refuses the reconciler's launch exactly as it refuses the
/// browser's, because both go through one control core.
#[tokio::test]
async fn a_cordoned_node_launches_nothing_for_a_deployment() {
    let node = Node::new();
    node.admission.set(Admission::Cordoned {
        reason: "retiring".into(),
    });
    let err = node.converge(&run_a(1)).await.expect_err("cordoned");
    assert!(matches!(err, AgentError::NotLaunchable { .. }), "{err:?}");
    assert_eq!(node.daemon.runs(), 0);
}

fn action(node: &str, kind: &str) -> DeploymentAction {
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "idempotency_key": "21212121212121212121212121212121",
        "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
        "scope": {"tenant": "acme", "project": "assistants"},
        "deployment": "chat",
        "generation": 1,
        "node_id": node,
        "action": kind,
        "issued_at_unix_ms": 1_791_000_000_000_u64
    }))
    .expect("a valid action")
}

/// The answer is the contract's result document, and it survives the
/// binding's own parse. An action for another node is `not_here` and touches
/// nothing. Sabotage: skipping the node check launches for an action that was
/// not this node's.
#[tokio::test]
async fn an_action_is_answered_in_the_result_contract_and_only_for_this_node() {
    let node = Node::new();
    let here = NodeId::parse(&"66".repeat(32)).expect("node id");
    let now = || UnixMs::new(1_791_000_001_200).expect("in range");
    let elsewhere = action(&"93".repeat(32), "apply");
    let t = target(A, "Qwen/Qwen3.6-27B-FP8");
    let refused = answer_action(&node.agent(), here, &elsewhere, &t, steps(), &now)
        .await
        .expect("answers");
    assert_eq!(
        refused.outcome,
        ActionOutcome::Refused(Refused {
            reason: RefusalReason::NotHere
        })
    );
    assert_eq!(node.daemon.runs(), 0);

    let mine = action(&"66".repeat(32), "apply");
    let result = answer_action(&node.agent(), here, &mine, &t, steps(), &now)
        .await
        .expect("answers");
    let json = serde_json::to_value(&result).expect("serializes");
    let back: DeploymentActionResult = serde_json::from_value(json.clone()).expect("valid");
    assert_eq!(back, result);
    assert_eq!(json["outcome"], "applied");
    assert_eq!(json["running_generation"], 1);
    assert_eq!(
        json["idempotency_key"],
        IdempotencyKey::parse(&"21".repeat(16))
            .expect("key")
            .to_string()
    );
    assert_eq!(json["instances"].as_array().map(Vec::len), Some(1));

    let drain = action(&"66".repeat(32), "drain");
    assert!(
        answer_action(&node.agent(), here, &drain, &t, steps(), &now)
            .await
            .is_err()
    );
}
