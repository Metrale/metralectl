// SPDX-License-Identifier: MIT OR Apache-2.0
#![deny(warnings)]
#![deny(clippy::all)]

//! The local agent: lets the Metrale Engine website launch recipes on this machine.

pub use metralectl_control::BoxFut;

pub mod bench;
pub mod cluster;
pub mod clusterdriver;
pub mod daemon;
pub mod discovery;
pub mod fabric;
pub mod fleet;
pub mod guard;
pub mod identity;
pub mod joining;
pub mod pairing;
pub mod peer;
pub mod rank;
pub mod rendezvous;
pub mod server;
pub mod session;
pub mod telemetry;
pub mod token;
pub mod transport;

/// Port the browser control channel listens on.
pub use metralectl_protocol::DEFAULT_BROWSER_PORT as DEFAULT_PORT;
