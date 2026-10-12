// SPDX-License-Identifier: MIT OR Apache-2.0

//! Per-vendor tool readings. Each vendor's tool names and output formats live
//! in its own provider module here, so the rest of the workspace stays
//! vendor-neutral (the `vendor neutrality` CI job enforces it).

pub mod nvidia;
