// SPDX-License-Identifier: MIT OR Apache-2.0

//! The bundle's fixed-width ids and digests. The trace id and the node id are
//! the protocol's own, so a frame and a document mean the same bytes.

pub use metralectl_protocol::{NodeId, TraceId};

metralectl_protocol::hex_id! {
    /// An idempotency key: 16 bytes.
    pub struct IdempotencyKey([u8; 16]); nonzero: false
}

metralectl_protocol::hex_id! {
    /// One running instance of a deployment: 16 bytes.
    pub struct InstanceId([u8; 16]); nonzero: false
}

metralectl_protocol::hex_id! {
    /// A SHA-256 digest.
    pub struct Sha256Digest([u8; 32]); nonzero: false
}

metralectl_protocol::hex_id! {
    /// A full git commit; never a branch or a tag.
    pub struct GitCommit([u8; 20]); nonzero: false
}
