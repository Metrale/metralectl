// SPDX-License-Identifier: MIT OR Apache-2.0
#![deny(warnings)]
#![deny(clippy::all)]

//! Recipe model and deterministic recipe-to-docker translation.

pub mod chain;
pub mod docker;
pub mod flags;
pub mod hfcache;
pub mod host;
pub mod io;
pub mod metrics;
pub mod nearest;
pub mod platform;
pub mod recipe;
pub mod registry;
pub mod scalar;
pub mod secretfile;
pub mod settings;

pub use docker::{DockerCommand, LaunchProfile};
pub use recipe::{NotLaunchable, Provenance, Recipe, RecipeError, RuntimeKind, Topology};
pub use scalar::ScalarValue;
