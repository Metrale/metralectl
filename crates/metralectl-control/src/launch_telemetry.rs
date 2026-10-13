// SPDX-License-Identifier: MIT OR Apache-2.0

//! The port through which the control core samples a running launch.

use metralectl_protocol::RecipeId;

/// Sampling a running launch.
///
/// A trait so the session stays I/O-free: sampling means opening a socket to
/// whatever the model is serving on, and a session that did that itself could
/// not be tested without a model.
pub trait LaunchTelemetry: Send + Sync {
    /// Take a reading of a running launch.
    ///
    /// # Errors
    /// If the model is not answering — the ordinary state of one still loading
    /// its weights, which the page shows as "loading" rather than as a fault.
    fn sample(&self, recipe: &RecipeId) -> Result<metralectl_protocol::msg::LaunchReading, String>;

    /// The tail of a launch's log.
    ///
    /// Read for diagnosis only: every number this project shows comes from the
    /// engine's `/metrics`, and nothing is parsed out of log text.
    ///
    /// # Errors
    /// If there is no container for that recipe.
    fn logs(&self, recipe: &RecipeId, lines: u32) -> Result<crate::logs::LogTail, String>;
}
