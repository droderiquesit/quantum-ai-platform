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
//! Declaring it once makes the class of mistake unreachable rather than merely
//! unlikely, which is the difference between a guarantee the compiler holds and
//! one a comment asserts.

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
pub const CELL_DELTA_SCHEMA_VERSION: u32 = 2;
