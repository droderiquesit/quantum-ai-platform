//! Event fabric: in-tree single-node broker, client SDK and ledger contracts.
//!
//! See ADR 0100 for the event fabric's first build architecture.
//! This module scaffolds its submodules; each is implemented by the packet named in its doc comment.

pub mod bindings;
pub mod catalogue;
pub mod codec;
pub mod crc32c;
pub mod envelope;
pub mod hlc;
pub mod policy;
pub mod schema_id;
