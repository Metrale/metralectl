// SPDX-License-Identifier: MIT OR Apache-2.0

//! How a session actually starts, stops, and inspects launches.
//!
//! Behind a trait so the session's decisions — which are the security-relevant
//! part — are tested against a recording mock rather than a docker daemon.

use crate::platform::ids::InstanceId;
use crate::reconcile::contract::{Instance, Owner};
use crate::release::VerifiedRelease;
use metralectl_core::{Recipe, ScalarValue};
use metralectl_protocol::msg::{AgentError, ControlTrace, RunningLaunch};
use std::collections::BTreeMap;

/// A rendered command, for the client to inspect before committing.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    /// The command, shell-quoted for display.
    pub command: String,
    /// Recipe settings this agent version does not understand.
    pub unapplied: Vec<String>,
}

/// A launch that started.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    /// Container name.
    pub container: String,
    /// Where the model is served, when it serves.
    pub endpoint: Option<String>,
}

/// Who a launch is for, beyond what it runs.
///
/// Never part of what makes two launches the same launch: a retried launch
/// for the same deployment and generation is the first one, whatever else
/// differs about the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchFor {
    /// The deployment and generation it realises, written on the container
    /// as labels; `None` for a launch outside any deployment (a browser, the
    /// command line).
    pub owner: Option<Owner>,
    /// The verified release it runs: its image is run by digest in place of
    /// the recipe's tag. `None` for a launch that names no release.
    pub release: Option<VerifiedRelease>,
    /// The attempt and trace of the request that asked for it, written on the
    /// container it starts. A retried launch that finds the container
    /// running leaves the first attempt's record in place.
    pub trace: Option<ControlTrace>,
}

/// One running workload, as the launcher's own state reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workload {
    /// The instance: the first 16 bytes of its container id, so the id the
    /// plane is told is one the container runtime also answers to.
    pub id: InstanceId,
    /// What it runs, and for which deployment.
    pub instance: Instance,
}

pub mod docker;
pub mod idempotent;

pub use docker::DockerLauncher;

/// Runs launches.
pub trait Launcher: Send + Sync {
    /// Render the command without running it.
    fn preview(
        &self,
        recipe: &Recipe,
        overrides: &BTreeMap<String, ScalarValue>,
    ) -> Result<Preview, AgentError>;

    /// Start a recipe — idempotently, from the launcher's own state.
    ///
    /// The same launch already running is returned as it is, never started a
    /// second time: this is what makes a retried `Launch` after a lost reply,
    /// or one sent after the agent restarted, safe without a token. A
    /// DIFFERENT launch of the recipe already running is `AlreadyRunning`,
    /// and so is the same launch running for another owner than
    /// `launch_for` names, when it names one.
    fn launch(
        &self,
        recipe: &Recipe,
        overrides: &BTreeMap<String, ScalarValue>,
        launch_for: &LaunchFor,
    ) -> Result<Started, AgentError>;

    /// Stop a recipe by name. Nothing of it running is success: the state a
    /// stop asks for already holds.
    fn stop(&self, recipe: &str) -> Result<(), AgentError>;

    /// What is currently running.
    fn running(&self) -> Result<Vec<RunningLaunch>, AgentError>;

    /// Every running workload a stop could reach, with the deployment it was
    /// launched for: what the reconciler observes.
    fn workloads(&self) -> Result<Vec<Workload>, AgentError>;
}

#[cfg(any(test, feature = "test-mocks"))]
mod mock {
    use super::*;
    use std::sync::Mutex;

    /// What a mock launcher was asked to do.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Call {
        /// A preview was rendered.
        Preview(String),
        /// A launch was started. Boxed for size: it is the largest call.
        Launch(String, BTreeMap<String, ScalarValue>, Box<LaunchFor>),
        /// A launch was stopped.
        Stop(String),
        /// Running launches were listed.
        Running,
        /// Workloads were observed.
        Workloads,
    }

    /// Records what the session asked for, and answers as scripted.
    #[derive(Debug, Default)]
    pub struct RecordingLauncher {
        calls: Mutex<Vec<Call>>,
        fail_launch: Mutex<Option<AgentError>>,
    }

    impl RecordingLauncher {
        /// A launcher that succeeds at everything.
        pub fn new() -> Self {
            Self::default()
        }

        /// Make the next launch fail.
        pub fn failing(error: AgentError) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail_launch: Mutex::new(Some(error)),
            }
        }

        /// Everything it was asked to do, in order.
        pub fn calls(&self) -> Vec<Call> {
            self.calls.lock().expect("lock").clone()
        }

        /// Whether it was asked to launch anything at all.
        pub fn launched_anything(&self) -> bool {
            self.calls().iter().any(|c| matches!(c, Call::Launch(..)))
        }

        fn record(&self, c: Call) {
            self.calls.lock().expect("lock").push(c);
        }
    }

    impl Launcher for RecordingLauncher {
        fn preview(
            &self,
            recipe: &Recipe,
            _o: &BTreeMap<String, ScalarValue>,
        ) -> Result<Preview, AgentError> {
            self.record(Call::Preview(recipe.name.clone()));
            Ok(Preview {
                command: format!("docker run … {}", recipe.model),
                unapplied: vec![],
            })
        }

        fn launch(
            &self,
            recipe: &Recipe,
            o: &BTreeMap<String, ScalarValue>,
            launch_for: &LaunchFor,
        ) -> Result<Started, AgentError> {
            self.record(Call::Launch(
                recipe.name.clone(),
                o.clone(),
                Box::new(launch_for.clone()),
            ));
            if let Some(e) = self.fail_launch.lock().expect("lock").take() {
                return Err(e);
            }
            Ok(Started {
                container: format!("metrale-{}", recipe.name),
                endpoint: Some("http://localhost:8888/v1".into()),
            })
        }

        fn stop(&self, recipe: &str) -> Result<(), AgentError> {
            self.record(Call::Stop(recipe.to_string()));
            Ok(())
        }

        fn running(&self) -> Result<Vec<RunningLaunch>, AgentError> {
            self.record(Call::Running);
            Ok(Vec::new())
        }

        fn workloads(&self) -> Result<Vec<Workload>, AgentError> {
            self.record(Call::Workloads);
            Ok(Vec::new())
        }
    }
}

#[cfg(any(test, feature = "test-mocks"))]
mod mock_arc {
    use super::*;
    use std::sync::Arc;

    /// A shared handle is a launcher too, so a test can keep inspecting a
    /// [`RecordingLauncher`] after handing ownership to a `ControlHost`.
    /// Test-only: production owners hold exactly one launcher each.
    impl<T: Launcher> Launcher for Arc<T> {
        fn preview(
            &self,
            recipe: &Recipe,
            o: &BTreeMap<String, ScalarValue>,
        ) -> Result<Preview, AgentError> {
            self.as_ref().preview(recipe, o)
        }
        fn launch(
            &self,
            recipe: &Recipe,
            o: &BTreeMap<String, ScalarValue>,
            launch_for: &LaunchFor,
        ) -> Result<Started, AgentError> {
            self.as_ref().launch(recipe, o, launch_for)
        }
        fn stop(&self, recipe: &str) -> Result<(), AgentError> {
            self.as_ref().stop(recipe)
        }
        fn running(&self) -> Result<Vec<RunningLaunch>, AgentError> {
            self.as_ref().running()
        }
        fn workloads(&self) -> Result<Vec<Workload>, AgentError> {
            self.as_ref().workloads()
        }
    }
}

#[cfg(any(test, feature = "test-mocks"))]
pub use mock::{Call, RecordingLauncher};
