// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a repeated `Launch` or `Stop` means, decided from docker's account.
//!
//! Pure: each function takes what docker printed and returns a decision; the
//! docker launcher runs the queries and carries the decision out (SBIO). No
//! token and no in-memory ledger is consulted. The containers docker lists
//! ARE the launcher's state, which is why a `Launch` retried after a lost
//! reply, or sent after this agent restarted, finds the first launch instead
//! of starting a second.

use super::Workload;
use crate::platform::ids::InstanceId;
use crate::reconcile::contract::{DeploymentName, Generation, Instance, Owner};
use metralectl_core::docker::translate::{
    LABEL_ATTEMPT, LABEL_DEPLOYMENT, LABEL_GENERATION, LABEL_RECIPE, LABEL_TRACE, container_name,
};
use metralectl_core::docker::{DockerCommand, Placement};
use metralectl_protocol::RecipeId;
use metralectl_protocol::msg::ControlTrace;
use serde::Deserialize;
use std::collections::BTreeMap;

/// One running container, as [`ps_format`] prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsRow {
    /// Docker's container id.
    pub id: String,
    /// The container name.
    pub name: String,
    /// The recipe label, absent on a container this launcher did not label.
    pub recipe: Option<String>,
    /// The deployment label, absent on a launch outside any deployment.
    pub deployment: Option<String>,
    /// The generation label, absent on a launch outside any deployment.
    pub generation: Option<String>,
}

/// The `docker ps --format` template [`parse_ps`] reads. One definition, so
/// the query and the parser cannot drift apart.
#[must_use]
pub fn ps_format() -> String {
    let label = |key: &str| format!("{{{{.Label \"{key}\"}}}}");
    format!(
        "{{{{.ID}}}}\t{{{{.Names}}}}\t{}\t{}\t{}",
        label(LABEL_RECIPE),
        label(LABEL_DEPLOYMENT),
        label(LABEL_GENERATION)
    )
}

/// Parse `docker ps --format` output in the [`ps_format`] shape.
///
/// # Errors
/// A line without an id or a name: docker printed something this parser does
/// not understand, and deciding a launch or a stop from it would be a guess.
pub fn parse_ps(stdout: &str) -> Result<Vec<PsRow>, String> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            let id = parts.next().unwrap_or_default().trim();
            let name = parts.next().unwrap_or_default().trim();
            if id.is_empty() || name.is_empty() {
                return Err(format!(
                    "docker ps printed a line without an id and a name: {line:?}"
                ));
            }
            let mut label = || {
                let v = parts.next().unwrap_or_default().trim();
                (!v.is_empty()).then(|| v.to_owned())
            };
            Ok(PsRow {
                id: id.to_owned(),
                name: name.to_owned(),
                recipe: label(),
                deployment: label(),
                generation: label(),
            })
        })
        .collect()
}

/// The running container a launch named `name` would collide with.
#[must_use]
pub fn running_as<'a>(name: &str, rows: &'a [PsRow]) -> Option<&'a PsRow> {
    rows.iter().find(|r| r.name == name)
}

/// The `docker inspect --format` template [`parse_inspect`] reads.
pub const INSPECT_FORMAT: &str = "{{json .Config}}";

/// What a running container was started with, as `docker inspect` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LaunchedAs {
    /// The image reference it was run from.
    #[serde(rename = "Image")]
    pub image: String,
    /// The command after the image.
    #[serde(rename = "Cmd")]
    pub cmd: Option<Vec<String>>,
    /// Its environment, `KEY=VALUE`, the image's own entries included.
    #[serde(rename = "Env")]
    pub env: Option<Vec<String>>,
    /// Its labels, the image's own included.
    #[serde(rename = "Labels")]
    pub labels: Option<BTreeMap<String, String>>,
}

/// Parse [`INSPECT_FORMAT`] output.
///
/// # Errors
/// When docker printed something that is not a container config.
pub fn parse_inspect(stdout: &str) -> Result<LaunchedAs, String> {
    serde_json::from_str(stdout.trim())
        .map_err(|e| format!("docker inspect printed an unreadable config: {e}"))
}

/// Whether the running container IS the launch `plan` describes: the same
/// image, the same command, and every variable the plan sets.
///
/// Anything else is a different launch of the same recipe. Returning it as
/// this one would tell the caller its settings were applied when they were
/// not, so the launcher refuses instead of adopting it.
#[must_use]
pub fn is_same_launch(plan: &DockerCommand, launched: &LaunchedAs) -> bool {
    let env = launched.env.as_deref().unwrap_or_default();
    plan.image == launched.image
        && launched.cmd.as_deref().unwrap_or_default() == plan.command.as_slice()
        && plan
            .env
            .iter()
            .all(|(k, v)| env.iter().any(|e| *e == format!("{k}={v}")))
}

/// The labels that record `owner` on a container.
#[must_use]
pub fn owner_labels(owner: &Owner) -> [(String, String); 2] {
    [
        (
            LABEL_DEPLOYMENT.to_owned(),
            owner.deployment.as_str().to_owned(),
        ),
        (
            LABEL_GENERATION.to_owned(),
            owner.generation.get().to_string(),
        ),
    ]
}

/// The labels that record the request a container was started for.
#[must_use]
pub fn trace_labels(trace: &ControlTrace) -> [(String, String); 2] {
    [
        (LABEL_ATTEMPT.to_owned(), trace.attempt.to_string()),
        (LABEL_TRACE.to_owned(), trace.trace_id.to_string()),
    ]
}

/// The owner two labels record: both present and both well formed, or none.
///
/// A container with only one of them, or one that does not parse, is read as
/// owned by no deployment. The reconciler never stops such a container and
/// refuses to launch over it, so a damaged label costs a refusal, never an
/// unrelated workload.
#[must_use]
pub fn owner_of(deployment: Option<&str>, generation: Option<&str>) -> Option<Owner> {
    Some(Owner {
        deployment: DeploymentName::parse(deployment?).ok()?,
        generation: Generation::new(generation?.parse().ok()?)?,
    })
}

/// Whether a running container was launched for `owner`.
#[must_use]
pub fn is_owned_by(launched: &LaunchedAs, owner: &Owner) -> bool {
    let labels = launched.labels.as_ref();
    let get = |k: &str| labels.and_then(|l| l.get(k)).map(String::as_str);
    owner_of(get(LABEL_DEPLOYMENT), get(LABEL_GENERATION)).as_ref() == Some(owner)
}

/// Every workload a stop of its recipe would reach, read from `docker ps`.
///
/// A container counts when it carries a recipe label, or when it is unlabelled
/// but bears a recipe's solo name (an older agent's launch, which
/// [`stop_targets`] also reaches). Anything else on the machine is not this
/// launcher's and no verb can touch it, so it is left out.
///
/// # Errors
/// A container id that is not docker's full 64-hex id: the instance id is its
/// first 32 digits, and a guess at it would name the wrong instance.
pub fn workloads(rows: &[PsRow]) -> Result<Vec<Workload>, String> {
    rows.iter()
        .filter_map(|r| {
            let recipe = match &r.recipe {
                Some(label) => RecipeId::parse(label).ok(),
                None => r
                    .name
                    .strip_prefix("metrale-")
                    .and_then(|n| RecipeId::parse(n).ok())
                    .filter(|id| container_name(id.as_str(), &Placement::Solo) == r.name),
            }?;
            Some((r, recipe))
        })
        .map(|(r, recipe)| {
            let id =
                r.id.get(..32)
                    .filter(|_| r.id.len() == 64)
                    .ok_or_else(|| format!("docker printed {:?}, not a full container id", r.id))
                    .and_then(InstanceId::parse)?;
            Ok(Workload {
                id,
                instance: Instance {
                    recipe,
                    owner: owner_of(r.deployment.as_deref(), r.generation.as_deref()),
                },
            })
        })
        .collect()
}

/// Where a launch with this in-container command serves, when it serves.
///
/// One function for a fresh launch and an adopted one, so the endpoint a
/// repeated `Launch` reports cannot differ from the one the first reported.
#[must_use]
pub fn endpoint_of(command: &[String]) -> Option<String> {
    match command
        .windows(2)
        .find(|w| w[0] == "--port")
        .map(|w| w[1].as_str())
    {
        // A worker rank is bound to port 0 and serves nothing.
        Some("0") | None => None,
        Some(p) => Some(format!("http://localhost:{p}/v1")),
    }
}

/// The containers a `Stop` of `recipe` must stop: every one labelled with it,
/// cluster ranks included, plus the solo name for a container an older agent
/// started before the label existed. An exact name, never a pattern, so an
/// unrelated container can never be caught. Empty means nothing of the
/// recipe's is running, and the stop has nothing to do.
#[must_use]
pub fn stop_targets(recipe: &str, rows: &[PsRow]) -> Vec<String> {
    let solo = container_name(recipe, &Placement::Solo);
    rows.iter()
        .filter(|r| match &r.recipe {
            Some(label) => label == recipe,
            None => r.name == solo,
        })
        .map(|r| r.id.clone())
        .collect()
}

#[cfg(test)]
#[path = "idempotent_tests.rs"]
mod tests;
