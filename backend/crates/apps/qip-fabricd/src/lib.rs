//! Event fabric composition root: the in-tree single-node broker, sole writer
//! of every partition's batch chain and holder of its own segment-roll clock
//! (ADR 0100 § 1, § 2). Not a spool: the producer-side durable spool is
//! `apps/qip-edge-node::event_fabric`, behind the existing
//! `qip_edge::journal::Mirror` seam.
//!
//! Each module below is a doc-only stub; its own comment names the packet
//! that fills it.

pub mod archiver;
pub mod config;
pub mod health;
pub mod telemetry;
