// SPDX-License-Identifier: MIT OR Apache-2.0
#![deny(warnings)]
#![deny(clippy::all)]

//! The single-node lifecycle core: the seven closed control verbs
//! ([`control::LocalControl`]), the launcher that realises them
//! ([`launcher::Launcher`], [`launcher::DockerLauncher`]) with idempotent
//! launch and stop decided from the launcher's own state, and the client-side
//! bounded retry ([`control::retry`]).
//!
//! A library, not a service: it carries no web server, TLS, service discovery
//! or telemetry transport, so a host process embeds it in-process and owns its
//! own network surface. The metralectl agent is one such host.

/// The boxed future a `dyn`-used trait method returns.
///
/// Several traits are held as trait objects (`dyn ControlRelay`,
/// `dyn RankTransport`, `dyn ClusterControl`, `dyn PeerPairing` in
/// metralectl-agent; the send closure of [`control::retry::drive`]), and an
/// `async fn` in a trait is not dyn-compatible. Boxing is the way those methods
/// get to `await` instead of blocking a runtime worker — and writing the full
/// `Pin<Box<dyn Future<Output = …> + Send + 'a>>` at every one of them is what
/// `clippy::type_complexity` objects to, correctly.
pub type BoxFut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

pub mod control;
mod launch_telemetry;
pub mod launcher;
pub mod logs;

pub use launch_telemetry::LaunchTelemetry;
