//! ADR 0100 §6: the node's measurement of spool bytes against budget, feeding
//! `qip_edge::pressure`'s reading-style halt wire — Narrow and Exhausted are
//! read off this measurement, not asserted independently. SLICE-54.
