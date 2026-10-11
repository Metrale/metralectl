// SPDX-License-Identifier: MIT OR Apache-2.0

//! The hardware-compatibility and admission gates of the control core.
//!
//! Both refuse BEFORE the launcher is asked for anything, for every caller:
//! a relayed `Launch` goes through [`LocalControl::execute`] and the browser
//! through [`LocalControl::launch`], and both are exercised.

use super::*;
use crate::admission::{Admission, AdmissionGate};
use crate::launcher::{Call, RecordingLauncher};
use metralectl_core::io::MemFileSystem;
use metralectl_core::registry::RemoteRegistry;
use std::sync::Arc;

const GB10: &str = "NVIDIA GB10";
const NEEDS_GB10: &str = "needs-gb10";
const ANYWHERE: &str = "anywhere";

fn id(s: &str) -> RecipeId {
    RecipeId::parse(s).expect("valid id")
}

/// A registry with one recipe that declares GB10 and one that declares
/// nothing, neither in the built-in corpus.
fn registry() -> RegistrySet {
    let fs = Arc::new(MemFileSystem::new());
    let base = "model: org/m\ncontainer: img:tag\nruntime: metrale\n";
    fs.insert(
        format!("/cache/t/recipes/{NEEDS_GB10}.yaml"),
        format!("{base}requires:\n  hardware_class: gb10\n"),
    );
    fs.insert(format!("/cache/t/recipes/{ANYWHERE}.yaml"), base);
    let remote: RemoteRegistry =
        serde_yaml_ng::from_str("name: t\nurl: https://example.invalid/t.git\npath: /cache/t\n")
            .expect("registry parses");
    RegistrySet::with_remotes(vec![remote.with_fs(fs)])
}

struct Node {
    registry: RegistrySet,
    launcher: Arc<RecordingLauncher>,
    can_launch: Result<(), String>,
    admission: AdmissionGate,
}

impl Node {
    fn new() -> Self {
        Self {
            registry: registry(),
            launcher: Arc::new(RecordingLauncher::new()),
            can_launch: Ok(()),
            admission: AdmissionGate::new(Admission::Open),
        }
    }

    fn on(&self, accelerator: &'static str) -> LocalControl<'_> {
        LocalControl {
            registry: &self.registry,
            launcher: self.launcher.clone(),
            telemetry: None,
            can_launch: &self.can_launch,
            accelerator,
            admission: &self.admission,
        }
    }

    fn launched(&self) -> bool {
        self.launcher.launched_anything()
    }
}

async fn relayed_launch(c: &LocalControl<'_>, recipe: &str) -> Result<ControlRep, AgentError> {
    c.execute(
        ControlReq::Launch {
            recipe: id(recipe),
            settings: BTreeMap::new(),
        },
        None,
    )
    .await
}

fn refusal_reason(out: Result<ControlRep, AgentError>) -> String {
    match out {
        Err(AgentError::NotLaunchable { reason, .. }) => reason,
        other => panic!("expected NotLaunchable, got {other:?}"),
    }
}

#[tokio::test]
async fn a_recipe_declaring_its_hardware_launches_on_that_hardware() {
    let n = Node::new();
    let out = relayed_launch(&n.on(GB10), NEEDS_GB10).await;
    assert!(matches!(out, Ok(ControlRep::Started { .. })), "{out:?}");
    assert!(n.launched());
}

#[tokio::test]
async fn a_recipe_declaring_other_hardware_is_refused_before_the_launcher() {
    for accelerator in ["NVIDIA H100 80GB HBM3", ""] {
        let n = Node::new();
        let reason = refusal_reason(relayed_launch(&n.on(accelerator), NEEDS_GB10).await);
        assert!(reason.contains("gb10"), "{accelerator:?}: {reason}");
        assert!(!n.launched(), "{accelerator:?}: the launcher was asked");
        // The browser's path is the same core.
        let direct = n
            .on(accelerator)
            .launch(
                &id(NEEDS_GB10),
                &BTreeMap::new(),
                crate::launcher::LaunchFor {
                    owner: None,
                    release: None,
                    trace: None,
                },
            )
            .await;
        assert!(
            matches!(direct, Err(AgentError::NotLaunchable { .. })),
            "{direct:?}"
        );
        assert!(!n.launched());
    }
}

#[tokio::test]
async fn a_recipe_declaring_nothing_is_gated_by_nothing() {
    let n = Node::new();
    let out = relayed_launch(&n.on("NVIDIA H100 80GB HBM3"), ANYWHERE).await;
    assert!(matches!(out, Ok(ControlRep::Started { .. })), "{out:?}");
}

#[tokio::test]
async fn a_cordoned_or_draining_node_refuses_launches_and_still_stops() {
    for state in [
        Admission::Cordoned {
            reason: "retiring".into(),
        },
        Admission::Draining {
            reason: "retiring".into(),
        },
    ] {
        let n = Node::new();
        n.admission.set(state.clone());
        let reason = refusal_reason(relayed_launch(&n.on(GB10), ANYWHERE).await);
        assert!(
            reason.contains("no new launches") && reason.contains("retiring"),
            "{state:?}: {reason}"
        );
        assert!(!n.launched(), "{state:?}");
        // What runs can still be wound down and looked at.
        let stop = n
            .on(GB10)
            .execute(
                ControlReq::Stop {
                    recipe: id(ANYWHERE),
                },
                None,
            )
            .await;
        assert!(matches!(stop, Ok(ControlRep::Stopped { .. })), "{stop:?}");
        assert!(n.on(GB10).execute(ControlReq::Status, None).await.is_ok());
        assert!(n.launcher.calls().contains(&Call::Stop(ANYWHERE.into())));
    }
}

#[tokio::test]
async fn a_cordon_set_on_the_host_is_heard_by_its_control_core_and_lifted_live() {
    let launcher = Arc::new(RecordingLauncher::new());
    let host = ControlHost::new(
        registry(),
        Arc::clone(&launcher) as Arc<dyn crate::launcher::Launcher>,
        None,
        Ok(()),
        GB10.to_owned(),
        Arc::new(AdmissionGate::new(Admission::Open)),
    );
    host.admission().set(Admission::Cordoned {
        reason: "maintenance".into(),
    });
    let out = relayed_launch(&host.control(), ANYWHERE).await;
    assert!(
        matches!(out, Err(AgentError::NotLaunchable { .. })),
        "{out:?}"
    );
    assert!(!launcher.launched_anything());

    host.admission().set(Admission::Open);
    let out = relayed_launch(&host.control(), ANYWHERE).await;
    assert!(matches!(out, Ok(ControlRep::Started { .. })), "{out:?}");
}
