//! ADR 0100 §1: "Reflex ring → spool → drain" — the node's composition of
//! that chain behind the existing `qip_edge::journal::Mirror` seam. This
//! module only declares the seams below; each names the packet that fills
//! it in its own doc comment. SLICE-52.

pub mod control;
pub mod drain;
pub mod inputs;
pub mod mirror;
pub mod pressure;
pub mod telemetry;
pub mod writer;
