// SPDX-License-Identifier: MIT OR Apache-2.0

//! `metralectl agent install --config <file>`: the agent step of an
//! installer's workflow (investor-mvp #47).
//!
//! The composition root for [`crate::service::config`]: read the file, hash
//! this binary, install through the same [`crate::service::install`] the
//! interactive path uses, and answer in one JSON line an installer can read
//! (`unit`, `changed`, `running`, `skipped`). A repeat with the same file and
//! binary installs nothing and restarts nothing. Nothing here listens or
//! opens a connection: what the agent listens on is the file's to say.

use crate::service::config::InstallConfig;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Install from `file`.
///
/// # Errors
/// If the file is unreadable or not a valid configuration, a global
/// `--config-dir` names another directory than the file, the install fails,
/// or the agent is not running afterwards (after printing the outcome).
pub fn install(file: &Path) -> Result<()> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("reading the install configuration {}", file.display()))?;
    let config = InstallConfig::parse(&text)?;
    // Two places naming the agent's state is one too many: refuse rather
    // than pick one and install an agent with another node's identity.
    if let Ok(dir) = std::env::var(crate::configdir::DIR_ENV)
        && !dir.trim().is_empty()
        && Path::new(&dir) != config.config_dir
    {
        bail!(
            "--config-dir {dir} disagrees with config_dir {} in {}; name it in the file only",
            config.config_dir.display(),
            file.display()
        );
    }
    let exe = std::env::current_exe().context("could not find this binary's own path")?;
    let home = crate::hostinfo::home_dir()?;
    let bench_node = config
        .config_dir
        .join(metralectl_agent::bench::config::FILE)
        .is_file();
    let invocation = config.invocation(&exe, &binary_sha256(&exe)?, bench_node);
    let done = crate::service::install(
        &metralectl_core::io::StdFileSystem,
        &metralectl_core::io::StdProcessRunner,
        &invocation,
        &home,
        super::service::uid_of(&home),
    )?;
    println!(
        "{}",
        serde_json::json!({
            "unit": done.unit_path.display().to_string(),
            "changed": done.changed,
            "running": done.running,
            "skipped": done.skipped,
        })
    );
    if !done.running {
        bail!(
            "the agent is installed but NOT running: {} is enabled and the agent exits at \
             startup; its log says why",
            done.unit_path.display()
        );
    }
    Ok(())
}

/// The SHA-256 of the file at `exe`, as lower-case hex.
///
/// # Errors
/// If the file cannot be read.
pub fn binary_sha256(exe: &Path) -> Result<String> {
    let bytes = std::fs::read(exe).with_context(|| format!("reading {}", exe.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}
