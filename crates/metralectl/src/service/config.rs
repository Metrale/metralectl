// SPDX-License-Identifier: MIT OR Apache-2.0

//! The install configuration file: everything `agent install --config` puts
//! into the unit, stated by whoever lays the node down, so an installer runs
//! the step non-interactively and the same file always installs the same
//! agent.
//!
//! Every key is required and no other key is read (PCND): an installer that
//! leaves one out is refused, rather than given today's default baked into a
//! unit that outlives it. `discovery: false` with `browser: false` is a node
//! with no inbound listener at all: no browser channel, no peer listener, no
//! advertisement, which is what a managed node runs (its administration
//! arrives through its own outbound control plane, never inbound SSH).
//!
//! ```json
//! {"version": 1, "port": 34333, "mode": "node", "discovery": false,
//!  "browser": false, "config_dir": "/var/lib/metralectl"}
//! ```

use super::plan::AgentInvocation;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// What this machine does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Runs models.
    Node,
    /// Discovers, pairs and monitors; never runs a model.
    Client,
}

/// One install, fully stated.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallConfig {
    /// The file's format version: 1.
    pub version: u32,
    /// The browser channel's port (bound on loopback; unused when `browser`
    /// is false, and still written so the unit never inherits a default).
    pub port: u16,
    /// What this machine does.
    pub mode: Mode,
    /// Whether to advertise on, and listen to, the network for peers.
    pub discovery: bool,
    /// Whether to serve the browser channel.
    pub browser: bool,
    /// Where the agent keeps its identity and pins: absolute.
    pub config_dir: PathBuf,
}

impl InstallConfig {
    /// Parse and check a configuration file's text.
    ///
    /// # Errors
    /// When it is not the format (a key missing, unknown or mistyped),
    /// another version, or a relative `config_dir`.
    pub fn parse(text: &str) -> Result<Self> {
        let c: Self = serde_json::from_str(text).context("the install configuration")?;
        if c.version != 1 {
            bail!(
                "install configuration version {} is not implemented; this build reads 1",
                c.version
            );
        }
        if !c.config_dir.is_absolute() {
            bail!(
                "config_dir {} is relative; a service unit outlives the directory it was \
                 installed from, so it must be absolute",
                c.config_dir.display()
            );
        }
        Ok(c)
    }

    /// The agent the unit starts: this configuration, the binary at `exe`
    /// with content `binary_sha256`, and whether `config_dir` holds a bench
    /// configuration.
    #[must_use]
    pub fn invocation(&self, exe: &Path, binary_sha256: &str, bench_node: bool) -> AgentInvocation {
        AgentInvocation {
            exe: exe.to_path_buf(),
            port: self.port,
            client: self.mode == Mode::Client,
            discovery: self.discovery,
            browser: self.browser,
            config_dir: Some(self.config_dir.clone()),
            log_file: None,
            bench_node,
            binary_sha256: binary_sha256.to_owned(),
        }
    }
}
