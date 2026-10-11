//! ADR 0100 §1: "Reflex ring → spool → drain" — the node's composition of
//! that chain behind the existing `qip_edge::journal::Mirror` seam. SLICE-52.
//!
//! As `main.rs` wires it: the decision thread flushes through
//! [`mirror::FabricMirror`] and records each pass through
//! [`inputs::RecordedInputs`], both offering to one bounded channel; the
//! [`relay`] forwards every handoff to the [`writer`] and ships journal
//! batches to the configured store; the writer fsyncs to the spool and beats
//! the [`pressure`] gauge; the [`drain`] delivers the spool to the tick lake
//! and, when one is configured, the fabric. Every stage after the first is a
//! thread of its own.

pub mod control;
pub mod drain;
pub mod inputs;
pub mod mirror;
/// Where the spool writer runs: pinned to a core when the deployment names
/// one, and measured either way (FABRIC-7058).
pub mod placement;
pub mod pressure;
pub mod relay;
pub mod telemetry;
pub mod writer;
