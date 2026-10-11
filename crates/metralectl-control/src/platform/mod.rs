// SPDX-License-Identifier: MIT OR Apache-2.0

//! This agent's binding of the platform contracts (`schemas/platform/` at the
//! repository root, vendored unchanged from the platform and pinned by
//! `schemas/platform.pin`).
//!
//! The schema defines each document's shape; these types are bound to it
//! mechanically: every key required and every object closed, each value a type
//! that refuses on parse what the schema refuses, so a document held in one of
//! these types is a valid one. `tests/platform_contracts.rs` proves it over
//! the bundle's fixtures. Bound here are the kinds the agent reads or writes:
//! `deployment-action`, `deployment-action-result`, `model-deployment`,
//! `hardware-fingerprint`, `release-manifest` and `model-recipe`.

pub mod action;
pub mod canonical;
pub mod deployment;
pub mod field;
pub mod fingerprint;
pub mod ids;
pub mod recipe;
pub mod release;
pub mod scalars;
