// SPDX-License-Identifier: MIT OR Apache-2.0

//! What the page may ask about a launch that is already running.

use super::{Session, err};
use metralectl_protocol::RecipeId;
use metralectl_protocol::msg::ServerMsg;

impl Session<'_> {
    /// How a running launch is doing.
    pub(super) fn launch_stats(&self, id: u32, recipe_id: &RecipeId) -> Vec<ServerMsg> {
        match self.control().stats(recipe_id) {
            Ok(stats) => vec![ServerMsg::Stats {
                id,
                recipe: recipe_id.clone(),
                stats: Box::new(stats),
                // This machine answering its own browser directly; see the
                // note in `session/launch.rs`.
                on: None,
                via: None,
            }],
            // The core already maps "not answering yet" to NotLaunchable
            // rather than LaunchFailed, so a model still loading its weights
            // reads as "loading" and not as a crash to hunt for.
            Err(e) => vec![err(Some(id), e)],
        }
    }

    /// The tail of a launch's log.
    pub(super) fn launch_logs(&self, id: u32, recipe_id: &RecipeId, lines: u32) -> Vec<ServerMsg> {
        match self.control().logs(recipe_id, lines) {
            Ok(tail) => vec![ServerMsg::Logs {
                id,
                recipe: recipe_id.clone(),
                container: tail.container,
                lines: tail.lines,
                running: tail.running,
                on: None,
                via: None,
            }],
            Err(e) => vec![err(Some(id), e)],
        }
    }
}
