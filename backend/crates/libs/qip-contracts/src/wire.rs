//! Wire-contract constants the edge plane and the centre must agree on.
//!
//! The two ends of the cell uplink cannot share a type. `qip-edge` declares
//! what a cell sends and `qip-mesh` declares what the centre reads, and the
//! dependency direction forbids a service naming the edge crate — so the
//! agreement is made of constants and of the vocabulary in this crate, which
//! both ends already depend on.
//!
//! A constant that only *ought* to match on both sides is the weakest form of
//! that agreement. [`CELL_DELTA_SCHEMA_VERSION`] used to be written twice, once
//! at each end, under a comment claiming the round-trip tests held the two
//! equal. They did not: `qip-mesh`'s wire type is private, nothing compared the
//! two numbers, and the claim survived because nobody had yet had cause to
//! change one. The first change to reach it was the one that added the
//! contributor vector, and the pair was kept in step by hand and by memory.
//!
//! Declaring it once removes the need to change two numbers together. It does
//! **not** make the mistake structurally impossible, and the first version of
//! this note claimed it did: nothing in the language stops either end writing
//! a literal again, and a test asserting each end equals this constant cannot
//! notice, because expected and actual are then the same number read from the
//! same place. What holds the property is
//! `architecture.rs::neither_end_of_the_cell_uplink_declares_its_schema_version_as_a_literal`,
//! which reads the two source files and refuses a numeric literal on either
//! declaration. A convention plus a test that can actually fail — weaker than
//! a compiler guarantee, and worth saying so rather than overstating it.

use crate::signal::StrategyId;
use crate::venue::VenueId;
use qip_core::{Decimal, ObjectId};
use serde::{Deserialize, Serialize};

/// One internal cross a cell booked, as it crosses the uplink (§27.1).
///
/// Declared here rather than mirrored at each end. `DeltaOrder` and
/// `DeltaRefusal` are each written twice, once in `qip-edge` and once in
/// `qip-mesh`, and a review found that nothing tested the pair against real
/// bytes — a rename on either side emptied the contributor vector at the
/// centre with the whole workspace green. Repeating that arrangement for a
/// record the blueprint calls a regulatory expectation would be knowingly
/// building the same hazard again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossRecord {
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// The matched size: what never needed a venue.
    pub quantity: Decimal,
    /// The prevailing mid at the netting instant — a price neither side chose.
    pub price: Decimal,
    pub bought: Vec<StrategyId>,
    pub sold: Vec<StrategyId>,
}

/// How many crosses one delta carries before it starts counting instead.
///
/// Bounded like everything else on this wire. The counter beside the list is
/// what keeps a truncation visible; a cross the centre never hears about and
/// is never told it missed is the failure mode this pairing exists to prevent.
pub const MAX_CROSSES_PER_DELTA: usize = 64;

/// The schema version of a cell's state delta, for both ends of the uplink.
///
/// Raise it whenever the delta's wire shape changes in a way an older reader
/// would misunderstand. The reader refuses a payload written by a version
/// newer than its own rather than decoding it partially, so a bump means an
/// unupgraded centre goes quiet about the cells that have moved ahead of it —
/// loud, and recoverable, and much better than silently attributing a netted
/// fill to whichever strategy happened to be largest.
///
/// * **1** — the original shape.
/// * **2** — `DeltaOrder::contributors`: which strategies an order was netted
///   from, with each one's signed share and the feature revisions it reasoned
///   from. Before this a netted order named only its largest contributor.
/// * **3** — `crosses`: the internal crosses the cell booked. Before this the
///   centre heard a cross *refusal*, because refusals travel, but never the
///   cross itself — so the one thing §27.1 calls a ledger entry was the one
///   thing that stopped at the cell, in a plane built to keep working while
///   partitioned from the centre.
pub const CELL_DELTA_SCHEMA_VERSION: u32 = 3;
