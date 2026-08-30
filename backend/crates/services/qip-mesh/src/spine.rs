//! The central plane's half of the mesh spine: deltas in, capital out.
//!
//! # Which mesh this is
//!
//! The rest of this crate is the *data* mesh — point-in-time ports over a
//! lakehouse, a hot series, a graph. This module is the *network* mesh of ADR
//! 0011: the wire between the nine regional cells and the central plane. They
//! share a word and nothing else, and the module is here rather than in a crate
//! of its own because the centre's inbox is where a cell's state lands and this
//! crate is what the centre already holds for landing state in.
//!
//! # The two directions are not symmetrical, and the asymmetry is the point
//!
//! **Up: state deltas, not durable, not decoded here.** A cell publishes what
//! it holds and what it has refused. This module takes those frames off the
//! wire, refuses a redelivery it has already absorbed, and hands each one to a
//! [`CellDeltaSink`] the composition root supplies. It deliberately does not
//! decode the payload: the delta's type belongs to `qip-edge`, which is an edge
//! crate, and a service that reached down into a cell to name it would put the
//! cell's whole dependency graph — an order manager among it — behind every
//! crate that holds a data-mesh port. The composition root is the one place
//! that legitimately knows both `qip_edge::CellStateDelta` and
//! `qip_kernel::CellReport`, so the decode belongs there and the sink is the
//! seam it plugs into.
//!
//! **Down: signed capital envelopes, durable, fully typed.** A grant is
//! [`qip_contracts::capital::CapitalEnvelope`] — shared vocabulary, which both
//! ends may name — so this side builds the frame and the cell decodes it. And
//! this direction is spooled: [`CapitalDispatcher`] persists the frame before
//! it attempts a send and removes it only once the peer has acknowledged. A
//! lost state delta is replaceable by the next one, because the parts of it
//! that matter are absolute. A lost capital instruction is not replaceable by
//! anything: the cell simply never receives the authority it was granted, stops
//! when its current envelope expires, and nobody finds out until it is quiet.
//!
//! # What is not promised
//!
//! * **No authentication.** This wire is plaintext and identifies nobody; see
//!   `qip_transport`'s crate documentation and
//!   [`qip_transport::MeshPublisher::REQUIREMENTS`]. A cell does not trust an
//!   envelope because it arrived — it verifies the signature — and this module
//!   correspondingly does not trust a delta because it arrived. It absorbs it
//!   as a *claim by a cell*, which is all it ever was.
//! * **No exactly-once.** Delivery is at-least-once in both directions, and
//!   [`CellDeltaReceiver`] is idempotent because it must be, not as a courtesy.
//! * **One dispatcher per spool namespace.** The spool is not a queue shared
//!   between processes; two dispatchers over one namespace would both claim the
//!   same entries. `qip_transport::spool` says so, and this type inherits it.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use qip_contracts::capital::CapitalEnvelope;
use qip_core::error::{Error, Result};
use qip_core::{Clock, CorrelationId, Id, Lineage, Timestamp};
use qip_events::envelope::canonical_json;
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_storage::kv::KeyValueStore;
use qip_transport::breaker::{
    BreakerPolicy, BreakerState, CircuitBreaker, Decision as BreakerDecision, Outcome, Refusal,
};
use qip_transport::deadletter::DeadLetterReason;
use qip_transport::error::TransportError;
use qip_transport::retry::Sleeper;
use qip_transport::spool::{DurableSpool, SpoolEntry, SpoolStats};
use qip_transport::{DeadLetterSink, Delivery, MeshConfig, MeshEndpoint, MeshInbox, MeshPublisher};
use serde::{Deserialize, Serialize};

/// The topic a cell's state delta arrives on.
///
/// It has to match `qip_edge::CellStateDelta`'s and the two cannot share a
/// declaration: the edge crate may not be named from a service. This constant
/// and the vocabulary in `qip-contracts` are the whole of what the two ends
/// agree on, which is deliberately as narrow as a wire contract can be. A
/// mismatch here does not corrupt anything — the receiver counts the frame as
/// one it does not handle — but it does mean the centre goes quiet, so the
/// round-trip test in `tests/spine.rs` runs both ends against one socket.
pub const CELL_DELTA_TOPIC: Topic = Topic::PositionUpdated;

/// How many absorbed deltas a receiver remembers by default.
const DEFAULT_DELTA_MEMORY: usize = 4_096;

// --- down: signed capital envelopes -------------------------------------

/// A capital envelope, as it travels.
///
/// A transparent newtype rather than an `impl` on the envelope itself, because
/// the trait and the type both belong to other crates. Transparent so the
/// payload on the wire *is* the envelope: a cell decodes it with the vocabulary
/// alone and never needs to name this type, which is what lets an edge crate
/// receive from a service it may not depend on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapitalGrantFrame(pub CapitalEnvelope);

impl EventBody for CapitalGrantFrame {
    /// There is no `CapitalGranted` topic and adding one means editing
    /// `qip-events`, which every crate shares. `RiskApproved` carries this
    /// meaning — an approved bound on what may be risked — and its group is
    /// permanently retained, which is what a capital instruction needs.
    const TOPIC: Topic = Topic::RiskApproved;
    const SCHEMA_VERSION: u32 = 1;

    /// Cell, strategy and signature — the same string the cell keys its own
    /// idempotency on.
    ///
    /// Made of the grant rather than of the send, so a retry, a restart's
    /// re-send from the spool, and a re-issue of the identical grant are all
    /// one message to every layer that looks. The signature covers every field
    /// that bounds the cell, so two grants with the same signature grant the
    /// same thing.
    fn idempotency_key(&self) -> Option<String> {
        Some(grant_key(&self.0))
    }
}

/// The identity of a grant, independent of how it was framed.
///
/// Mirrors `qip_edge::mesh`'s key exactly. The two are written twice because
/// the dependency edge between an edge crate and a service only runs one way;
/// they are kept in step by the round-trip test rather than by the compiler,
/// and the test asserts the cell recognises the duplicate rather than asserting
/// the strings match, because recognition is the property that matters.
///
/// Public because the composition root needs the same identity: a serving
/// binary that re-dispatches the plane's envelopes each cycle must recognise
/// a grant it has already handed to [`CapitalDispatcher::dispatch`], or every
/// cycle would push the same envelope into the spool again. Deriving the key
/// here rather than there keeps one declaration of what a grant's identity
/// *is* on the centre's side of the wire.
pub fn grant_key(envelope: &CapitalEnvelope) -> String {
    format!(
        "{}|{}|{}",
        envelope.cell(),
        envelope.strategy().as_str(),
        envelope.signature()
    )
}

/// How a dispatcher to one cell is built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatcherConfig {
    /// The cell this dispatcher serves. One per cell: the spool is FIFO and per
    /// publisher, and pooling nine cells behind one would make one unreachable
    /// cell hold up the other eight's capital.
    pub cell: String,
    pub mesh: MeshConfig,
    /// How many undelivered envelopes may wait. At the bound a dispatch is
    /// refused rather than the spool growing, because a spool that grew without
    /// limit would trade a lost message for an exhausted disk.
    pub spool_capacity: usize,
    pub breaker: BreakerPolicy,
    pub breaker_seed: u64,
}

impl DispatcherConfig {
    pub fn new(cell: impl Into<String>, mesh: MeshConfig) -> Self {
        Self {
            cell: cell.into(),
            mesh,
            spool_capacity: 1_024,
            breaker: BreakerPolicy::default(),
            breaker_seed: 0,
        }
    }

    pub fn with_spool_capacity(mut self, capacity: usize) -> Self {
        self.spool_capacity = capacity;
        self
    }

    pub fn with_breaker(mut self, policy: BreakerPolicy, seed: u64) -> Self {
        self.breaker = policy;
        self.breaker_seed = seed;
        self
    }
}

/// Why an envelope is still in the spool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeldReason {
    /// The circuit to the cell is open. Nothing was sent and no retry ladder
    /// was spent — which is the point of holding it rather than trying.
    CircuitOpen(Refusal),
    /// Every attempt in the ladder failed without the peer answering.
    Undelivered { attempts: u32, last_error: String },
}

/// What happened to one envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapitalDispatch {
    /// The cell took it and the spool entry has been released.
    Delivered { sequence: u64, delivery: Delivery },
    /// Persisted and still waiting. A later [`CapitalDispatcher::recover`]
    /// sends it, including after a restart.
    Held { sequence: u64, reason: HeldReason },
    /// The cell answered and refused it on its merits, or the failure was one
    /// repeating cannot change. Recorded in the dead-letter sink with its frame
    /// and released from the spool.
    ///
    /// Released deliberately: the record is kept where an operator reads it,
    /// and an envelope the peer will never accept must not sit at the head of a
    /// FIFO spool blocking every later grant to that cell.
    Rejected {
        sequence: u64,
        reason: DeadLetterReason,
        attempts: u32,
        last_error: String,
    },
}

impl CapitalDispatch {
    pub const fn is_delivered(&self) -> bool {
        matches!(self, Self::Delivered { .. })
    }

    pub const fn sequence(&self) -> u64 {
        match self {
            Self::Delivered { sequence, .. }
            | Self::Held { sequence, .. }
            | Self::Rejected { sequence, .. } => *sequence,
        }
    }

    /// A stable code for a metric or a test asserting the outcome rather than
    /// matching on prose.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Delivered { .. } => "delivered",
            Self::Held { .. } => "held",
            Self::Rejected { .. } => "rejected",
        }
    }
}

/// What one pass over the backlog did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub outcomes: Vec<CapitalDispatch>,
    /// How many entries were still waiting when the pass stopped.
    pub remaining: usize,
}

impl RecoveryReport {
    pub fn delivered(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.is_delivered())
            .count()
    }
}

/// The centre's outbound capital path to one cell, over a durable spool.
///
/// # Persist, then send, then forget
///
/// The ordering is `qip_transport::spool`'s and the reason is worth restating
/// where it is used: a crash between the send and the commit re-sends the
/// envelope on restart, which is a *duplicate*, and the cell's own idempotency
/// already absorbs one. A crash between the send and a persist that had not
/// happened yet would lose a capital instruction with no record it existed.
#[derive(Debug)]
pub struct CapitalDispatcher {
    cell: String,
    peer: String,
    publisher: MeshPublisher,
    spool: DurableSpool,
    breaker: CircuitBreaker,
}

impl CapitalDispatcher {
    /// Open a dispatcher, adopting whatever a previous process left in the
    /// spool.
    ///
    /// Opening is a recovery but not a send: [`Self::recover`] is a separate
    /// call so that a start-up sequence decides when the backlog goes out,
    /// rather than a constructor blocking on a peer that may not be up yet.
    pub fn open(
        config: DispatcherConfig,
        store: Arc<dyn KeyValueStore>,
        clock: Arc<dyn Clock>,
        sleeper: Arc<dyn Sleeper>,
        dead_letters: Box<dyn DeadLetterSink>,
    ) -> Result<Self> {
        let breaker = CircuitBreaker::new(
            config.breaker,
            Arc::clone(&clock),
            config.breaker_seed,
            // One peer per dispatcher; the bound exists because the key is an
            // address and this transport authenticates nobody.
            4,
        )?;
        let spool = DurableSpool::open(
            store,
            format!("capital/{}", config.cell),
            config.spool_capacity,
        )?;
        let peer = config.mesh.peer.clone();
        let publisher = MeshPublisher::new(config.mesh, clock, sleeper, dead_letters)?;
        Ok(Self {
            cell: config.cell,
            peer,
            publisher,
            spool,
            breaker,
        })
    }

    pub fn cell(&self) -> &str {
        &self.cell
    }

    pub fn peer(&self) -> &str {
        &self.peer
    }

    pub const fn spool_stats(&self) -> SpoolStats {
        self.spool.stats()
    }

    pub fn circuit(&self) -> BreakerState {
        self.breaker.state(&self.peer)
    }

    /// How many envelopes are persisted and not yet acknowledged.
    pub fn pending(&self) -> Result<usize> {
        self.spool.depth()
    }

    /// Every envelope still waiting, oldest first.
    pub fn backlog(&self) -> Result<Vec<SpoolEntry>> {
        self.spool.backlog()
    }

    pub const fn publisher(&self) -> &MeshPublisher {
        &self.publisher
    }

    /// Persist an envelope and try to send it.
    ///
    /// Refuses an envelope for another cell before anything is written. A
    /// dispatcher is per cell precisely so that the cell's name is checked once
    /// here rather than trusted nine times downstream — and the receiving cell
    /// refuses it again anyway, because a correctly signed grant for a
    /// different cell is exactly the replay a signature alone does not stop.
    pub fn dispatch(
        &mut self,
        envelope: CapitalEnvelope,
        at: Timestamp,
    ) -> Result<CapitalDispatch> {
        if envelope.cell() != self.cell {
            return Err(Error::denied(format!(
                "an envelope for cell {} was handed to the dispatcher for {}",
                envelope.cell(),
                self.cell
            )));
        }
        let frame = frame_for(&envelope, at)?;
        // Persisted before any attempt to send it. Everything after this point
        // may fail without losing the instruction.
        let sequence = self.spool.push(frame.clone()).map_err(Error::from)?;
        self.send(sequence, frame, at)
    }

    /// Send everything the spool holds, oldest first.
    ///
    /// Stops at the first entry that is still held. FIFO with head-of-line
    /// blocking, deliberately: capital instructions to one cell are ordered,
    /// and skipping past a stuck one would deliver a later grant while an
    /// earlier one — possibly a narrower replacement — never arrived.
    ///
    /// The exception is an entry the peer *refused*, which is released rather
    /// than held, so a grant the cell will never accept cannot block the ones
    /// behind it forever.
    pub fn recover(&mut self, at: Timestamp) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        loop {
            let Some(entry) = self.spool.front()? else {
                break;
            };
            let outcome = self.send(entry.sequence, entry.frame, at)?;
            let held = matches!(outcome, CapitalDispatch::Held { .. });
            report.outcomes.push(outcome);
            if held {
                break;
            }
        }
        report.remaining = self.spool.depth()?;
        Ok(report)
    }

    /// One attempt at one spooled entry, with the circuit in front of it.
    fn send(&mut self, sequence: u64, frame: AnyEvent, at: Timestamp) -> Result<CapitalDispatch> {
        let permit = match self.breaker.admit(&self.peer) {
            BreakerDecision::Refused(refusal) => {
                // Not recorded as an attempt: the breaker refused before the
                // socket was touched, and counting it would spend the entry's
                // persisted retry budget on a call that never happened.
                return Ok(CapitalDispatch::Held {
                    sequence,
                    reason: HeldReason::CircuitOpen(refusal),
                });
            }
            BreakerDecision::Admitted(permit) => permit,
        };

        match self.publisher.publish_frame(frame, at) {
            Ok(delivery) => {
                self.breaker.record(permit, Outcome::Success);
                // Forgotten last, and only now. A commit before the peer
                // answered would be the ordering that loses instructions.
                self.spool.commit(sequence)?;
                Ok(CapitalDispatch::Delivered { sequence, delivery })
            }
            Err(TransportError::DeadLettered {
                attempts,
                reason,
                last_error,
                ..
            }) => match reason {
                // The peer never answered. This is the case the spool exists
                // for: keep the entry, count the attempt so the budget survives
                // a restart, and let `recover` try again.
                DeadLetterReason::RetriesExhausted => {
                    self.breaker
                        .record(permit, Outcome::Failure(last_error.clone()));
                    self.spool.record_attempt(sequence, &last_error)?;
                    Ok(CapitalDispatch::Held {
                        sequence,
                        reason: HeldReason::Undelivered {
                            attempts,
                            last_error,
                        },
                    })
                }
                // The peer answered and refused, or the failure cannot change
                // on repetition. A peer that refuses is a peer that is *up*, so
                // a rejection is not evidence for the circuit — treating it as
                // one would open a circuit to a healthy cell because the centre
                // sent it something it did not like.
                DeadLetterReason::Rejected | DeadLetterReason::PermanentFailure => {
                    let outcome = if matches!(reason, DeadLetterReason::Rejected) {
                        Outcome::Success
                    } else {
                        Outcome::Failure(last_error.clone())
                    };
                    self.breaker.record(permit, outcome);
                    self.spool.commit(sequence)?;
                    Ok(CapitalDispatch::Rejected {
                        sequence,
                        reason,
                        attempts,
                        last_error,
                    })
                }
            },
            Err(error) => {
                self.breaker.record(permit, Outcome::failed(&error));
                self.spool.record_attempt(sequence, &error)?;
                Err(Error::from(error))
            }
        }
    }
}

/// Wrap a grant in the platform's event envelope.
///
/// The event id and the correlation id are derived from the grant rather than
/// minted, so an envelope re-sent from the spool after a restart carries the
/// identity it had the first time. A freshly minted id would make one grant
/// look like two to everything downstream that keys on identity — including the
/// peer's own duplicate detection.
///
/// Both timestamps are the dispatch instant. `CapitalEnvelope` does not expose
/// when it was granted — the field is private and the vocabulary crate is not
/// this change's to widen — so stamping `occurred_at` with anything else would
/// mean inventing a moment. The grant's own window travels inside the payload,
/// where the cell reads it and checks it, which is the copy that matters.
fn frame_for(envelope: &CapitalEnvelope, at: Timestamp) -> Result<AnyEvent> {
    let key = grant_key(envelope);
    Envelope::new(
        Id::from_string(format!(
            "EVTGRANT{}",
            qip_core::hash::sha256_hex(key.as_bytes())
        )),
        at,
        at,
        Lineage::root(
            CorrelationId::from_string(format!(
                "CORGRANT{}",
                qip_core::hash::sha256_hex(key.as_bytes())
            )),
            "qip-mesh",
        ),
        CapitalGrantFrame(envelope.clone()),
    )
    .erase()
}

// --- up: cell state deltas ----------------------------------------------

/// Where an absorbed delta goes.
///
/// Takes the erased frame rather than a decoded report, because the decoded
/// type belongs to `qip-edge` and this crate may not name it — see the module
/// documentation. The composition root implements this over
/// `AnyEvent::decode::<CellStateDelta>()` and hands the result to
/// `qip_kernel::Platform::ingest_cell_report`.
///
/// Returning `Err` stops the drain at that frame rather than skipping it. That
/// is head-of-line blocking and it is the safe direction: a delta the centre
/// could not absorb is a hole in its view of a cell, and continuing past it
/// would leave the hole invisible while the cursor moved on.
pub trait CellDeltaSink: std::fmt::Debug {
    fn absorb(&mut self, frame: &AnyEvent) -> Result<()>;
}

/// Counters for the receiving side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverStats {
    pub drains: u64,
    /// Frames handed to the sink and accepted.
    pub absorbed: u64,
    /// Frames recognised as ones this receiver has already absorbed. Non-zero
    /// is the at-least-once guarantee being visible, not a fault.
    pub duplicates: u64,
    /// Frames on the inbox that were not cell deltas.
    pub ignored: u64,
    /// Frames refused because their payload no longer matched its hash.
    pub corrupt: u64,
    /// Times a sink refused a frame and stopped the drain.
    pub halts: u64,
    /// Keys forgotten because the memory filled. Past this, a redelivery is
    /// absorbed again and the sink's own idempotency is the only thing left.
    pub forgotten_keys: u64,
}

/// Why a drain stopped early.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrainHalt {
    /// The inbox position of the frame that was not absorbed. The cursor stays
    /// behind it, so the next drain offers it again.
    pub position: u64,
    pub key: String,
    pub reason: String,
}

/// What one drain did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub absorbed: usize,
    pub duplicates: Vec<String>,
    pub ignored: usize,
    pub corrupt: Vec<String>,
    pub halted: Option<DrainHalt>,
    /// How many frames the inbox still holds. Includes the ones this drain
    /// absorbed: the inbox discards up to an acknowledged position on the
    /// *next* read, so they are still there until then. A number that pretended
    /// otherwise would report a backlog that has not been read yet.
    pub remaining: usize,
}

impl DrainReport {
    pub fn is_clean(&self) -> bool {
        self.halted.is_none() && self.corrupt.is_empty()
    }
}

/// The centre's inbox for cell state deltas, and the idempotent read over it.
///
/// The inbox and the endpoint are `qip-transport`'s; what this adds is the
/// consumer half the transport documentation insists on — "every consumer of
/// this transport must be idempotent; that is the precondition for using it at
/// all". The inbox detects a duplicate within its own window and this detects
/// one past it, keyed on the frame's idempotency key, which for a cell delta is
/// the cell and its own sequence.
#[derive(Debug)]
pub struct CellDeltaReceiver {
    endpoint: MeshEndpoint,
    cursor: u64,
    absorbed: BTreeSet<String>,
    absorbed_order: VecDeque<String>,
    memory: usize,
    stats: ReceiverStats,
}

impl CellDeltaReceiver {
    /// An inbox holding at most `capacity` frames, detecting duplicates within
    /// `dedup_window` keys on arrival and `memory` keys on absorption.
    ///
    /// Two windows rather than one because they answer different questions. The
    /// inbox's window stops a *retry* from being queued twice; this one stops a
    /// *redelivery* — a poll repeated after a lost response, a receiver rebuilt
    /// after a restart — from being absorbed twice. The second outlives the
    /// first, so it is the larger by default.
    pub fn new(
        name: impl Into<String>,
        capacity: usize,
        dedup_window: usize,
        memory: usize,
    ) -> Result<Self> {
        if memory == 0 {
            return Err(Error::invalid(
                "a receiver that remembers nothing treats every redelivery as new, which is the \
                 duplicate the memory exists to absorb",
            ));
        }
        Ok(Self {
            endpoint: MeshEndpoint::new(MeshInbox::new(name, capacity, dedup_window)?),
            cursor: 0,
            absorbed: BTreeSet::new(),
            absorbed_order: VecDeque::new(),
            memory,
            stats: ReceiverStats::default(),
        })
    }

    /// A receiver with the default absorption memory.
    pub fn with_defaults(name: impl Into<String>, capacity: usize) -> Result<Self> {
        Self::new(name, capacity, capacity.max(1), DEFAULT_DELTA_MEMORY)
    }

    /// The endpoint whatever serves HTTP hands requests to.
    pub const fn endpoint(&self) -> &MeshEndpoint {
        &self.endpoint
    }

    pub fn inbox(&self) -> &MeshInbox {
        self.endpoint.inbox()
    }

    pub const fn stats(&self) -> ReceiverStats {
        self.stats
    }

    /// How far this receiver has absorbed. Positions at or below it are
    /// discarded by the inbox on the next drain.
    pub const fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Take everything the inbox held at or before `until` to the sink.
    ///
    /// The cursor advances only past frames the sink accepted, so a process
    /// that dies mid-drain re-offers the frames it had not finished with. That
    /// is the at-least-once read, and it is why absorption is keyed and not
    /// merely counted.
    pub fn drain(
        &mut self,
        until: Timestamp,
        limit: usize,
        sink: &mut dyn CellDeltaSink,
    ) -> Result<DrainReport> {
        self.stats.drains += 1;
        let response = self.inbox().read(self.cursor, until, limit.max(1));
        let mut report = DrainReport::default();

        for entry in response.frames {
            let key = entry.frame.dedup_key();

            if entry.frame.topic != CELL_DELTA_TOPIC {
                self.stats.ignored += 1;
                report.ignored += 1;
                self.cursor = entry.position;
                continue;
            }

            // Re-checked here as well as at the publishing endpoint, because
            // this is where the frame changes hands. It is not authentication
            // and does not pretend to be — it catches corruption and a naive
            // edit, and stops nobody who can also run SHA-256.
            if qip_core::hash::sha256_hex(canonical_json(&entry.frame.payload).as_bytes())
                != entry.frame.payload_hash
            {
                self.stats.corrupt += 1;
                report.corrupt.push(key);
                self.cursor = entry.position;
                continue;
            }

            if self.absorbed.contains(&key) {
                self.stats.duplicates += 1;
                report.duplicates.push(key);
                self.cursor = entry.position;
                continue;
            }

            match sink.absorb(&entry.frame) {
                Ok(()) => {
                    self.remember(key);
                    self.stats.absorbed += 1;
                    report.absorbed += 1;
                    self.cursor = entry.position;
                }
                Err(error) => {
                    self.stats.halts += 1;
                    report.halted = Some(DrainHalt {
                        position: entry.position,
                        key,
                        reason: error.message().to_string(),
                    });
                    break;
                }
            }
        }

        report.remaining = self.inbox().depth();
        Ok(report)
    }

    fn remember(&mut self, key: String) {
        self.absorbed.insert(key.clone());
        self.absorbed_order.push_back(key);
        while self.absorbed_order.len() > self.memory {
            if let Some(oldest) = self.absorbed_order.pop_front() {
                self.absorbed.remove(&oldest);
                self.stats.forgotten_keys += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // In a test the assertion is the deliverable; the workspace denies this for
    // production code, where a panic on the capital path would be a bug.
    #![allow(clippy::panic_in_result_fn)]

    use super::*;
    use qip_contracts::signal::StrategyId;
    use qip_contracts::venue::VenueId;
    use qip_core::Decimal;

    fn envelope(cell: &str, signature: &str) -> Result<CapitalEnvelope> {
        CapitalEnvelope::new(
            StrategyId::new("mean-reversion-1"),
            cell,
            Decimal::from_int(1_000_000),
            Decimal::from_int(100_000),
            Decimal::from_int(50_000),
            vec![VenueId::new("XLON")],
            Timestamp::from_secs(1_000),
            Timestamp::from_secs(5_000),
            "alice@example.com",
            signature,
        )
    }

    #[test]
    fn a_grant_frame_carries_the_envelope_itself_rather_than_a_wrapper_around_it() -> Result<()> {
        // The transparency is load bearing: an edge cell decodes this payload
        // with the shared vocabulary alone, because it may not name the type
        // this side used to send it.
        let grant = envelope("london-1", "abc123")?;
        let frame = frame_for(&grant, Timestamp::from_secs(1_100))?;
        let decoded: CapitalEnvelope = serde_json::from_value(frame.payload.clone())
            .map_err(|error| Error::schema(error.to_string()))?;
        assert_eq!(decoded, grant);
        assert_eq!(frame.topic, Topic::RiskApproved);
        Ok(())
    }

    #[test]
    fn one_grant_produces_one_identity_however_often_it_is_framed() -> Result<()> {
        // A re-send from the spool after a restart must look like the same
        // message, or the peer's duplicate detection has nothing to detect.
        let grant = envelope("london-1", "abc123")?;
        let first = frame_for(&grant, Timestamp::from_secs(1_100))?;
        let second = frame_for(&grant, Timestamp::from_secs(9_900))?;
        assert_eq!(first.event_id, second.event_id);
        assert_eq!(first.dedup_key(), second.dedup_key());
        assert_ne!(
            first.recorded_at, second.recorded_at,
            "the dispatch time is the one thing that legitimately differs"
        );
        Ok(())
    }
}
