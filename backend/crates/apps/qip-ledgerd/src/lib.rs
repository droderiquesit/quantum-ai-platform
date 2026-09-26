//! Ledger composition root: the sole writer of the ledger chain of
//! double-entry postings for fills, and its own read-side API (ADR 0100 § 1,
//! § 2). Posting logic itself is pure double-entry arithmetic in
//! `qip-portfolio`; this crate is the durable store and the write/read seams
//! around it.
//!
//! Each module below is a doc-only stub; its own comment names the packet
//! that fills it.

pub mod config;
pub mod consumer;
pub mod read_api;
pub mod store;
pub mod telemetry;
