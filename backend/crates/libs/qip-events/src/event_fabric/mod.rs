//! Event fabric: the I/O-free contract types and codec, per ADR 0100 §1.
//!
//! `FabricEnvelope`, `StreamPolicy`, the QoS classes, `SchemaId` and the
//! record/batch codec live here because `qip-events` already owns `AnyEvent`,
//! `Topic`, retention classes and the schema registry. The broker itself is
//! `qip-streaming::event_fabric`, the client SDK and protocol are
//! `qip-transport::event_fabric`, and the reflex ledger contract is
//! `qip-contracts::reflex` — none of those live here.
//!
//! This module scaffolds its submodules; each is implemented by the packet named in its doc comment.

pub mod bindings;
pub mod catalogue;
pub mod codec;
pub mod crc32c;
pub mod envelope;
pub mod hlc;
pub mod policy;
pub mod schema_id;
