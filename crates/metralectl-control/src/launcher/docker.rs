// SPDX-License-Identifier: MIT OR Apache-2.0

//! The production launcher: translate a recipe, then run the command.

use super::idempotent::{self, LaunchedAs, PsRow};
use super::{Launcher, Preview, Started};
use metralectl_core::chain::UserConfig;
use metralectl_core::docker::collective::NcclRoce;
use metralectl_core::docker::profile::{DeviceProfile, LaunchProfile};
use metralectl_core::docker::translate::{LABEL_MANAGED, LaunchContext, Placement, translate};
use metralectl_core::host::HostSnapshot;
use metralectl_core::io::ProcessRunner;
use metralectl_core::{Recipe, ScalarValue};
use metralectl_protocol::RecipeId;
use metralectl_protocol::msg::{AgentError, RunningLaunch};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

/// Launches recipes through the docker CLI.
///
/// One per agent process: the per-recipe gates below only serialise the
/// requests that share them, so every caller (browser session and peer
/// channel alike) must hold the same instance.
pub struct DockerLauncher {
    runner: Arc<dyn ProcessRunner>,
    host: HostSnapshot,
    profile: &'static LaunchProfile,
    devices: Box<dyn DeviceProfile>,
    /// One gate per recipe, held by a launch or stop from its first `docker
    /// ps` to its last command, so two requests for one recipe cannot both
    /// see "not running" and both start it. Recipes are a closed corpus, so
    /// the map is bounded by it.
    gates: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
}

impl DockerLauncher {
    /// Build a launcher.
    pub fn new(
        runner: Arc<dyn ProcessRunner>,
        host: HostSnapshot,
        profile: &'static LaunchProfile,
        devices: Box<dyn DeviceProfile>,
    ) -> Self {
        Self {
            runner,
            host,
            profile,
            devices,
            gates: Mutex::new(BTreeMap::new()),
        }
    }

    /// The gate for `recipe`. A poisoned gate only means an earlier launch
    /// panicked; docker, not the gate, holds the state, so it is reused.
    fn gate(&self, recipe: &str) -> Arc<Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(gates.entry(recipe.to_owned()).or_default())
    }

    /// Every running container, in the shape the idempotency decisions read.
    fn ps(&self) -> Result<Vec<PsRow>, AgentError> {
        let out = self
            .runner
            .run(&[
                "docker".into(),
                "ps".into(),
                "--format".into(),
                idempotent::ps_format(),
            ])
            .map_err(|e| AgentError::DockerUnavailable {
                detail: format!("{e:#}"),
            })?;
        if !out.success() {
            return Err(AgentError::DockerUnavailable {
                detail: out.stderr.trim().to_string(),
            });
        }
        idempotent::parse_ps(&out.stdout).map_err(|detail| AgentError::LaunchFailed { detail })
    }

    /// What a running container was started with.
    fn inspect(&self, id: &str) -> Result<LaunchedAs, AgentError> {
        let out = self
            .runner
            .run(&[
                "docker".into(),
                "inspect".into(),
                "--format".into(),
                idempotent::INSPECT_FORMAT.into(),
                id.to_owned(),
            ])
            .map_err(|e| AgentError::LaunchFailed {
                detail: format!("{e:#}"),
            })?;
        if !out.success() {
            return Err(AgentError::LaunchFailed {
                detail: format!(
                    "docker inspect exited {}: {}",
                    out.status,
                    out.stderr.trim()
                ),
            });
        }
        idempotent::parse_inspect(&out.stdout).map_err(|detail| AgentError::LaunchFailed { detail })
    }

    fn plan(
        &self,
        recipe: &Recipe,
        overrides: &BTreeMap<String, ScalarValue>,
    ) -> Result<metralectl_core::docker::LaunchPlan, AgentError> {
        // A browser launch is always single-node. Multi-node needs a rank per
        // machine, which is a fleet decision rather than something one page
        // should be able to assert on its own.
        let ctx = LaunchContext {
            profile: self.profile,
            devices: self.devices.as_ref(),
            collective: &NcclRoce,
        };
        let mut plan = translate(
            recipe,
            overrides,
            &UserConfig::new(),
            &self.host,
            &Placement::Solo,
            &ctx,
        )
        .map_err(|e| AgentError::NotLaunchable {
            recipe: recipe_id(recipe),
            reason: format!("{e:#}"),
        })?;
        // The agent removes a container by name before it starts one, and on
        // stop, so it owns this lifecycle already. Auto-remove therefore buys
        // nothing and costs the only evidence there is: a rank that dies takes
        // its logs with it, and the operator's next question is always "why".
        // That happened for real -- a rank died a second after starting and the
        // container was gone before anyone could read it.
        plan.docker.auto_remove = false;
        Ok(plan)
    }
}

impl Launcher for DockerLauncher {
    fn preview(
        &self,
        recipe: &Recipe,
        overrides: &BTreeMap<String, ScalarValue>,
    ) -> Result<Preview, AgentError> {
        let plan = self.plan(recipe, overrides)?;
        Ok(Preview {
            command: plan.docker.to_string(),
            // Settings the recipe carries that this build does not understand.
            // Reported so the client can show them: the tool this replaces
            // discarded them silently.
            unapplied: plan.unmapped.into_iter().map(|u| u.key).collect(),
        })
    }

    fn launch(
        &self,
        recipe: &Recipe,
        overrides: &BTreeMap<String, ScalarValue>,
    ) -> Result<Started, AgentError> {
        let plan = self.plan(recipe, overrides)?;
        let name = plan.docker.name.clone();

        // The same guard `metralectl run` applies, because this path has an
        // operator too — they are just in a browser instead of a terminal, and
        // they get LESS to work with, not more: the launch runs offline, so a
        // missing model fails inside a container that has already exited, and
        // all the browser can show is that it stopped.
        //
        // Skipped when the recipe brings its own weights (`--model-from-path`),
        // since then the Hub cache is never consulted.
        if !plan.docker.command.iter().any(|a| a == "--model-from-path")
            && let Some(dir) = metralectl_core::hfcache::hub_dir(
                std::path::Path::new(&self.host.hf_cache_dir),
                &recipe.model,
            )
        {
            use metralectl_core::hfcache::CacheState;
            let detail = match metralectl_core::hfcache::cache_state(&dir) {
                CacheState::Weights => None,
                CacheState::Absent => Some(format!(
                    "the model `{}` is not in this machine's HuggingFace cache ({}). \
                     The launch runs offline, so it cannot download it — fetch it \
                     first with `hf download {}`.",
                    recipe.model, self.host.hf_cache_dir, recipe.model
                )),
                // Named separately: telling someone it is "not there" when they
                // can see the directory reads as a broken tool, and they stop
                // trusting the instruction that follows.
                CacheState::MetadataOnly => Some(format!(
                    "the model `{}` has a cache directory in {} but no weight \
                     files — what an interrupted or metadata-only download \
                     leaves behind. Complete it with `hf download {}`.",
                    recipe.model, self.host.hf_cache_dir, recipe.model
                )),
            };
            if let Some(detail) = detail {
                return Err(AgentError::LaunchFailed { detail });
            }
        }

        // Held until this launch has started or failed: a second request for
        // the recipe waits here, then finds this one running.
        let gate = self.gate(&recipe.name);
        let _held = gate.lock().unwrap_or_else(PoisonError::into_inner);

        // Already running is not a reason to start again. The same launch is
        // returned as it is — the answer to a retry whose first reply was
        // lost, or to a request sent after this agent restarted. A different
        // launch of the recipe is refused rather than replaced: the caller
        // stops it first, on purpose.
        let rows = self.ps()?;
        if let Some(row) = idempotent::running_as(&name, &rows) {
            if !idempotent::is_same_launch(&plan.docker, &self.inspect(&row.id)?) {
                return Err(AgentError::AlreadyRunning {
                    recipe: recipe_id(recipe),
                });
            }
            return Ok(Started {
                container: name,
                endpoint: idempotent::endpoint_of(&plan.docker.command),
            });
        }

        // Clear a stopped container of the same name. Failure is expected when
        // nothing is there; the launch below is what has to succeed.
        let _ = self
            .runner
            .run(&["docker".into(), "rm".into(), "-f".into(), name.clone()]);

        let out =
            self.runner
                .run(&plan.docker.to_argv())
                .map_err(|e| AgentError::LaunchFailed {
                    detail: format!("{e:#}"),
                })?;
        if !out.success() {
            return Err(AgentError::LaunchFailed {
                detail: format!("docker run exited {}: {}", out.status, out.stderr.trim()),
            });
        }

        Ok(Started {
            container: name,
            endpoint: idempotent::endpoint_of(&plan.docker.command),
        })
    }

    fn stop(&self, recipe: &str) -> Result<(), AgentError> {
        // By LABEL, not by name. A solo launch is `metrale-{recipe}`, but a rank of
        // a cluster is `metrale-{recipe}-rank{n}` -- so stopping by name found the
        // solo container and never a rank. That is the state an operator is left
        // in when the head agent restarts while a cluster runs: the driver's
        // record is memory-only, `StopCluster` answers "this agent did not start
        // a cluster", and this path -- the only other one -- could not see the
        // containers either. Both GPUs held, no button that works, `docker rm -f`
        // on every machine by hand.
        //
        // The label is already on every container this agent starts, put there
        // by the launch plan for exactly this kind of question. A container an
        // older agent started before the label existed is matched by its exact
        // solo name (`idempotent::stop_targets`).
        let gate = self.gate(recipe);
        let _held = gate.lock().unwrap_or_else(PoisonError::into_inner);
        let ids = idempotent::stop_targets(recipe, &self.ps()?);
        if ids.is_empty() {
            // Nothing of this recipe is running, which is what a stop asks
            // for: a repeated stop, or one whose first reply was lost, is done.
            return Ok(());
        }

        let mut argv: Vec<String> = vec!["docker".into(), "stop".into()];
        argv.extend(ids);
        let out = self
            .runner
            .run(&argv)
            .map_err(|e| AgentError::LaunchFailed {
                detail: format!("{e:#}"),
            })?;
        if out.success() {
            Ok(())
        } else {
            Err(AgentError::LaunchFailed {
                detail: out.stderr.trim().to_string(),
            })
        }
    }

    fn running(&self) -> Result<Vec<RunningLaunch>, AgentError> {
        let out = self
            .runner
            .run(&[
                "docker".into(),
                "ps".into(),
                "--filter".into(),
                format!("label={LABEL_MANAGED}=1"),
                "--format".into(),
                "{{.Names}}\t{{.Status}}".into(),
            ])
            .map_err(|e| AgentError::DockerUnavailable {
                detail: format!("{e:#}"),
            })?;
        if !out.success() {
            return Err(AgentError::DockerUnavailable {
                detail: out.stderr.trim().to_string(),
            });
        }
        Ok(out
            .stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                let mut parts = line.split('\t');
                let container = parts.next().unwrap_or_default().to_string();
                let status = parts.next().unwrap_or_default().to_string();
                // Recover the recipe from the container name, which we control.
                let recipe = container
                    .strip_prefix("metrale-")
                    .and_then(|s| RecipeId::parse(s).ok());
                RunningLaunch {
                    container,
                    recipe,
                    status,
                }
            })
            .collect())
    }
}

/// The wire id of a recipe this launcher rendered. Its name parsed as an id
/// when the request arrived, so the fallback is never expected to be used.
fn recipe_id(recipe: &Recipe) -> RecipeId {
    RecipeId::parse(&recipe.name)
        .unwrap_or_else(|_| RecipeId::parse("unknown").expect("literal is a valid id"))
}

#[cfg(test)]
mod tests;
