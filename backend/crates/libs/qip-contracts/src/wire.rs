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

use crate::message::BookSide;
use crate::signal::StrategyId;
use crate::venue::VenueId;
use qip_core::{Decimal, ObjectId, Timestamp};
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

/// One strategy's share of a venue fill, as the cell attributed it.
///
/// A named pair rather than a tuple because a tuple serialises as a
/// positional array, and a reader that swapped the two positions would
/// book a quantity as a strategy id with nothing on the wire to say so.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillShare {
    pub strategy: StrategyId,
    /// This strategy's part of the fill's quantity. Positive; the side is
    /// the fill's. The shares of one fill sum to its quantity exactly.
    pub quantity: Decimal,
}

/// One fill the venue reported on an order the cell sent, as it crosses
/// the uplink (§43.4: the attribution chain starts at the fill).
///
/// This record exists because the delta's `orders` list is a list of what
/// the cell *sent*, and for one slice the centre read it as a list of what
/// *filled* — attributing, charging the risk aggregate and moving strategy
/// books for orders that were still resting at the venue, or had expired
/// unfilled. Two claims about one fact, and the louder one was wrong. A
/// fill now travels only when the venue reported one, in this record, and
/// the centre bills from nothing else.
///
/// `shares` is the cell's own pro-rata split of `quantity` across the
/// order's contributors, computed from what the venue said traded rather
/// than from what was sent: an order that filled in three parts is shipped
/// three times, each summing to its own part. The centre books the shares
/// as shipped and refuses a fill whose shares do not sum to its quantity,
/// rather than re-splitting on a vector the fill no longer carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillRecord {
    /// The order this fill was reported against — the id the cell sent in
    /// the same or an earlier delta's `orders`. A fill naming an order the
    /// centre never saw sent is a reconciliation break, not a position.
    pub order_id: String,
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// The side of the book the order took: `Ask` bought, `Bid` sold.
    pub side: BookSide,
    pub quantity: Decimal,
    pub price: Decimal,
    /// From the gateway's answer at the time the order was sent. A paper
    /// fill counted as real is the single most consequential bit in the
    /// execution path, and it stays that way on the wire.
    pub simulated: bool,
    /// When the venue reported it, as the cell recorded it.
    pub at: Timestamp,
    pub shares: Vec<FillShare>,
}

/// How many fills one delta carries before it starts counting instead.
///
/// Bounded like the crosses, and counted beside the list for the same
/// reason. The consequence is stated rather than softened: a fill that did
/// not fit is a fill the centre never bills, and the counter is what makes
/// that visible in the same delta rather than discoverable at the next
/// reconciliation.
pub const MAX_FILLS_PER_DELTA: usize = 64;

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
/// * **4** — `fills`: the fills the venue confirmed, each with the cell's
///   own attribution. Before this the centre had only `orders` — what was
///   sent — and billed every one of them as a fill: attributing, charging
///   the risk aggregate and settling positions for orders still resting or
///   already expired. An older centre reading a newer delta would do that
///   again, which is why this is a bump and not a defaulted field alone: a
///   centre behind this version goes quiet about the cells ahead of it
///   rather than charging them for orders that never traded.
pub const CELL_DELTA_SCHEMA_VERSION: u32 = 4;
