// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a repeated `Launch` or `Stop` means, decided from docker's account.
//!
//! Pure: each function takes what docker printed and returns a decision; the
//! docker launcher runs the queries and carries the decision out (SBIO). No
//! token and no in-memory ledger is consulted. The containers docker lists
//! ARE the launcher's state, which is why a `Launch` retried after a lost
//! reply, or sent after this agent restarted, finds the first launch instead
//! of starting a second.

use metralectl_core::docker::translate::{LABEL_RECIPE, container_name};
use metralectl_core::docker::{DockerCommand, Placement};
use serde::Deserialize;

/// One running container, as [`ps_format`] prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsRow {
    /// Docker's container id.
    pub id: String,
    /// The container name.
    pub name: String,
    /// The recipe label, absent on a container this launcher did not label.
    pub recipe: Option<String>,
}

/// The `docker ps --format` template [`parse_ps`] reads. One definition, so
/// the query and the parser cannot drift apart.
#[must_use]
pub fn ps_format() -> String {
    format!("{{{{.ID}}}}\t{{{{.Names}}}}\t{{{{.Label \"{LABEL_RECIPE}\"}}}}")
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
            let label = parts.next().unwrap_or_default().trim();
            Ok(PsRow {
                id: id.to_owned(),
                name: name.to_owned(),
                recipe: (!label.is_empty()).then(|| label.to_owned()),
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
