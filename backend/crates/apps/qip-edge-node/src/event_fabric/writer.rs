//! ADR 0100 §1: the node's composition of the reflex ring → spool → drain
//! chain — the segment writer, sharing `qip-storage::segment`'s format with
//! the drain and the broker rather than a second on-disk shape. SLICE-24.
