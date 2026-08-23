//! The transport ports: publish, and drain to a watermark.
//!
//! # The pull contract, and why it is not negotiable
//!
//! [`Subscriber::poll`] takes an `until: Timestamp` and returns what is
//! available up to it. It is the same shape as
//! `qip_market_ingestion::adapter::DataAdapter::poll`, and for the same reason:
//! **the caller owns the clock**.
//!
//! A push-based consumer — a callback the transport invokes when a message
//! lands — cannot be driven by a simulated clock. Under a backtest there is
//! nothing to invoke it, and under a replay the order in which callbacks fire
//! depends on the runtime rather than on the data. The result is a strategy
//! that runs one way in research and another way in production, with no test
//! that can catch the difference because the difference *is* the test harness.
//!
//! Pull removes the question. The same loop drives a live run, a replay and a
//! backtest; only the clock differs, and the clock is a parameter. That is what
//! backtest/live parity means concretely, and it is why a durable buffer can
//! sit in front of the decision loop without the loop losing its clock.
//!
//! # `until` is known-time, always
//!
//! `poll(until)` returns events whose **ingest** timestamp is at or before
//! `until`, never their event timestamp. A consumer at `until` can only have
//! been told things the platform already knew, and filtering on valid-time
//! instead is the single mistake that makes a backtest profitable and a live
//! run not. The mesh's read ports say the same thing about `as_of`; this is
//! that rule applied to a stream.

use qip_contracts::time::Watermark;
use qip_core::Timestamp;
use qip_core::error::Result;
use serde::{Deserialize, Serialize};

use crate::envelope::StreamEnvelope;
use crate::routing::TransportPath;

/// What a transport is and what a deployment must supply for it.
///
/// The `production_requirement` field mirrors
/// `qip_market_ingestion::adapter::SourceDescriptor`: a transport that is a
/// port awaiting credentials says so in the same shape everything else does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportDescriptor {
    /// Stable transport name, recorded on every receipt.
    pub name: String,
    /// Which of the two paths this transport is.
    pub path: TransportPath,
    /// Whether messages survive the process.
    pub durable: bool,
    /// Whether this transport works in this build at all.
    pub available: bool,
    /// What production must supply, when this is a port rather than a working
    /// implementation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production_requirement: Option<String>,
}

/// What a publish produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishReceipt {
    pub transport: String,
    pub path: TransportPath,
    /// Position in the transport's own ordering. For the durable log this is
    /// the log sequence; for the local queue it counts admissions.
    pub position: u64,
    /// The log record hash, where the transport chains its records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_hash: Option<String>,
    /// When the transport accepted it, as supplied by the caller.
    pub accepted_at: Timestamp,
}

/// Somewhere an envelope can be sent.
pub trait Publisher: std::fmt::Debug {
    fn descriptor(&self) -> TransportDescriptor;

    /// Send one envelope. `at` is the caller's clock, not the transport's.
    fn publish(&mut self, envelope: StreamEnvelope, at: Timestamp) -> Result<PublishReceipt>;

    /// Send a batch.
    ///
    /// The default is a loop that stops at the first failure, because a
    /// transport that accepted half a batch and reported an error has left the
    /// caller with no way to know which half. A transport that can do better
    /// than that overrides this.
    fn publish_batch(
        &mut self,
        envelopes: Vec<StreamEnvelope>,
        at: Timestamp,
    ) -> Result<Vec<PublishReceipt>> {
        let mut receipts = Vec::with_capacity(envelopes.len());
        for envelope in envelopes {
            receipts.push(self.publish(envelope, at)?);
        }
        Ok(receipts)
    }
}

/// Somewhere envelopes can be drained from, on the caller's clock.
pub trait Subscriber: std::fmt::Debug {
    fn descriptor(&self) -> TransportDescriptor;

    /// Every envelope known to the transport at or before `until`, in order,
    /// removed from the subscription as it is returned.
    ///
    /// `until` is compared against [`StreamEnvelope::ingest_timestamp`]. See
    /// the module documentation for why it is never the event timestamp.
    fn poll(&mut self, until: Timestamp) -> Result<Vec<StreamEnvelope>>;

    /// How far this subscriber has been drained.
    ///
    /// `None` before the first drain. The position is contiguous by
    /// construction here — the transport hands out its own ordering, so unlike
    /// a source stream there is no hole for it to run past.
    fn watermark(&self) -> Option<Watermark>;

    /// Whether anything is waiting at or before `until`, without draining it.
    ///
    /// For a scheduler deciding whether to run the loop at all; a caller that
    /// intends to consume should just poll.
    fn has_pending(&self, until: Timestamp) -> bool;
}
