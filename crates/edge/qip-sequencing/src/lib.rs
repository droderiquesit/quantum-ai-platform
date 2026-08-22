//! `qip-sequencing` — sequence, gap, clock and failover discipline.
//!
//! This is what stands between a feed and a book that is quietly wrong. A
//! decoder's output is a stream of messages that may arrive late, twice, or not
//! at all, and each of those, mishandled, produces a book that looks perfectly
//! healthy and is not. There is no alarm for a book that is wrong; there is only
//! this layer.
//!
//! Four pieces, each guarding one way the stream can lie:
//!
//! * [`Sequencer`] and [`SequenceTracker`] — reorder within a bounded window,
//!   drop duplicates, and when a hole will not fill, say so with
//!   [`qip_contracts::MessageBody::Reset`] rather than carrying on.
//! * [`ClockDiscipline`] — estimate what a venue's timestamps mean in this
//!   cell's clock, publish how much to trust the estimate, and never let a
//!   correction move a timestamp backwards.
//! * [`LineArbiter`] — merge the redundant A and B lines a venue publishes,
//!   taking whichever copy arrives first and reporting each line's health.
//! * [`FailoverReconciler`] — change source mid-stream without dropping or
//!   double-applying anything.
//!
//! Three invariants hold across all of them, and the tests assert them as
//! properties rather than as fixed outputs:
//!
//! 1. **A watermark is the highest *contiguous* position, never the highest
//!    seen.** It only moves past a hole behind a reset that has already been
//!    released to the consumer.
//! 2. **Every buffer is bounded.** The reorder buffer and the arbitration window
//!    both fill under exactly the fault they exist to survive, so neither may
//!    grow without limit.
//! 3. **Nothing here reads a clock or draws a random number.** Times arrive as
//!    parameters, so a replay of a capture produces the same releases, the same
//!    resets and the same watermarks as the live run did.

pub mod arbitration;
pub mod clock;
pub mod failover;
pub mod identity;
pub mod tracker;

pub use arbitration::{ArbitrationEvent, ArbitrationOutcome, LineArbiter, LineHealth};
pub use clock::{ClockDiscipline, ClockEstimate, ClockObservation};
pub use failover::{FailoverEvent, FailoverOutcome, FailoverReconciler, FailoverStats};
pub use identity::{reset_message, synthetic_id};
pub use tracker::{
    GapReason, ReorderPolicy, SequencedBatch, SequenceEvent, SequenceTracker, Sequencer,
    StreamStats, delivery_units,
};
