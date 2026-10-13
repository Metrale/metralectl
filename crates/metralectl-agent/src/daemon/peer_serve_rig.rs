// SPDX-License-Identifier: MIT OR Apache-2.0

//! The rig the control-serving tests run on: the REAL dispatch
//! (`serve_peer_connection`) with a real pin store on disk, and only the
//! network and the container runtime faked. Split from `peer_serve_tests.rs`
//! for size; the tests are there and in `peer_serve_trace_tests.rs`.

use super::peer_serve::{PeerServe, serve_peer_connection};
use crate::identity::{Identity, PinStore};
use crate::peer::wire::{
    PEER_PROTOCOL_MAX, PEER_PROTOCOL_VERSION, PeerFrame, read_frame, write_frame,
};
use metralectl_control::control::ControlHost;
use metralectl_control::launcher::{Launcher, RecordingLauncher};
use metralectl_core::registry::RegistrySet;
use metralectl_protocol::fleet::{DisplayName, Launchability, NodeId};
use metralectl_protocol::msg::{AgentError, ControlRep};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub(super) struct Tmp(PathBuf);

impl Tmp {
    pub(super) fn new(tag: &str) -> Self {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "metralectl-peerserve-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&p).expect("scratch");
        Self(p)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shared handle onto the recording launcher, so a test can inspect what
/// the serving path did after handing ownership to the [`ControlHost`].
#[derive(Clone)]
pub(super) struct SharedLauncher(pub(super) Arc<RecordingLauncher>);

impl Launcher for SharedLauncher {
    fn preview(
        &self,
        recipe: &metralectl_core::Recipe,
        o: &std::collections::BTreeMap<String, metralectl_core::ScalarValue>,
    ) -> Result<metralectl_control::launcher::Preview, AgentError> {
        self.0.preview(recipe, o)
    }
    fn launch(
        &self,
        recipe: &metralectl_core::Recipe,
        o: &std::collections::BTreeMap<String, metralectl_core::ScalarValue>,
        launch_for: &metralectl_control::launcher::LaunchFor,
    ) -> Result<metralectl_control::launcher::Started, AgentError> {
        self.0.launch(recipe, o, launch_for)
    }
    fn fetch_image(
        &self,
        image: &metralectl_control::platform::scalars::OciImage,
    ) -> Result<(), String> {
        self.0.fetch_image(image)
    }
    fn stop(&self, recipe: &str) -> Result<(), AgentError> {
        self.0.stop(recipe)
    }
    fn running(&self) -> Result<Vec<metralectl_protocol::msg::RunningLaunch>, AgentError> {
        self.0.running()
    }
    fn workloads(&self) -> Result<Vec<metralectl_control::launcher::Workload>, AgentError> {
        self.0.workloads()
    }
}

/// A rank service that must never be reached by a control frame.
pub(super) struct Unreachable;

impl crate::rank::RankService for Unreachable {
    fn render(&self, _: &crate::cluster::RankAssignment) -> anyhow::Result<(String, Vec<String>)> {
        panic!("a control frame must not reach the rank service");
    }
    fn content_hash(&self, _: &str) -> anyhow::Result<String> {
        panic!("a control frame must not reach the rank service");
    }
    fn recipe_port(&self, _: &str) -> anyhow::Result<Option<u16>> {
        panic!("a control frame must not reach the rank service");
    }
    fn prepare(&self, _: &str, _: &crate::cluster::RankAssignment) -> crate::cluster::PrepareReply {
        panic!("a control frame must not reach the rank service");
    }
    fn commit(&self, _: &str) -> anyhow::Result<String> {
        panic!("a control frame must not reach the rank service");
    }
    fn alive(&self, _: &str) -> anyhow::Result<bool> {
        panic!("a control frame must not reach the rank service");
    }
    fn stop(&self, _: &str) -> anyhow::Result<()> {
        panic!("a control frame must not reach the rank service");
    }
    fn abort(&self, _: &str) {
        panic!("a control frame must not reach the rank service");
    }
}

pub(super) struct Rig {
    _tmp: Tmp,
    pub(super) pins: PinStore,
    pub(super) launcher: Arc<RecordingLauncher>,
    pub(super) serve: Arc<PeerServe>,
    pub(super) local: NodeId,
}

pub(super) fn rig(tag: &str) -> Rig {
    let tmp = Tmp::new(tag);
    let identity = Arc::new(Identity::load_or_create(&tmp.0).expect("identity"));
    let local = identity.id();
    let pins = PinStore::new(&tmp.0);
    let fleet = Arc::new(crate::fleet::LocalFleet::new(
        Identity::load_or_create(&tmp.0).expect("same identity from disk"),
        pins.clone(),
        DisplayName::new("serving-box"),
        Vec::new(),
        Launchability::yes(),
        "GB10".to_owned(),
    ));
    let launcher = Arc::new(RecordingLauncher::new());
    let serve = Arc::new(PeerServe {
        identity,
        pins: pins.clone(),
        fleet,
        rank: Arc::new(Unreachable),
        control: Arc::new(ControlHost::new(
            RegistrySet::builtin_only(),
            Arc::new(SharedLauncher(Arc::clone(&launcher))),
            None,
            Ok(()),
            "NVIDIA GB10".to_owned(),
            Arc::new(metralectl_control::admission::AdmissionGate::new(
                metralectl_control::admission::Admission::Open,
            )),
        )),
        peer_port: 34334,
        // Tests never reach a real dial; the budget only has to exist.
        answer_budget: Duration::from_secs(2),
        bench: None,
        bench_disabled: None,
    });
    Rig {
        _tmp: tmp,
        pins,
        launcher,
        serve,
        local,
    }
}

pub(super) fn sender() -> NodeId {
    NodeId::from_bytes([0x5e; 32])
}

pub(super) fn pin_sender(pins: &PinStore, granted: bool) {
    crate::fleet::record_pairing(
        pins,
        sender(),
        "aa",
        DisplayName::new("requester"),
        0,
        None,
        false,
    )
    .expect("pin");
    if granted {
        assert!(pins.set_controller(sender(), true).expect("grant"));
    }
}

/// Drive one serving task and return a client-side stream that has already
/// completed the hello exchange.
pub(super) async fn connected(rig: &Rig) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<()>) {
    let (mut ours, mut theirs) = tokio::io::duplex(1 << 16);
    let serve = Arc::clone(&rig.serve);
    let s = sender();
    let task = tokio::spawn(async move {
        serve_peer_connection(&mut theirs, &serve, s).await;
    });
    write_frame(
        &mut ours,
        &PeerFrame::Hello {
            version: PEER_PROTOCOL_VERSION,
            name: "client".to_owned(),
            can_launch: false,
            accelerator: String::new(),
            os: "Linux".to_owned(),
            addresses: Vec::new(),
            version_max: Some(PEER_PROTOCOL_MAX),
            vouched: None,
        },
    )
    .await
    .expect("hello");
    let hello = read_frame(&mut ours).await.expect("hello back");
    assert!(matches!(hello, PeerFrame::Hello { .. }));
    (ours, task)
}

pub(super) async fn ask(stream: &mut tokio::io::DuplexStream, frame: &PeerFrame) -> ControlRep {
    write_frame(stream, frame).await.expect("send");
    match read_frame(stream).await.expect("answered, not dropped") {
        PeerFrame::ControlReply { rep, .. } => rep,
        other => panic!("expected a control reply, got {other:?}"),
    }
}
