//! The cell's half of the mesh spine: state up, capital down.
//!
//! ADR 0011 replaced the managed message bus with `qip-transport`, and ADR 0008
//! says what has to cross it. A regional cell pushes its own state up, because
//! aggregate exposure across nine cells is the one number no cell can compute;
//! the central plane pushes signed capital envelopes back down, because a cell
//! that cannot be granted capital is a cell that stops trading when its current
//! grant expires. The transport underneath was built and tested against a real
//! socket before anything used it. This module is the cell end of the wire
//! being used.
//!
//! # Arriving over the mesh is not a reason to trust anything
//!
//! The most important property here is about a path that does *not* exist:
//! there is no route from a polled frame to a [`VerifiedEnvelope`] that skips
//! [`VerifiedEnvelope::verify`]. [`CapitalDownlink::poll`] decodes a
//! [`CapitalEnvelope`] out of the frame and then verifies it against the cell's
//! own key exactly as if an operator had typed it in, and an envelope that does
//! not verify is refused and recorded rather than delivered. The wire is
//! plaintext and authenticates nobody — `qip_transport`'s own crate
//! documentation says so — so delivery cannot be evidence of approval. The
//! signature is the evidence, and it is checked here.
//!
//! The frame's payload hash is recomputed on the way in as well. That is not
//! authentication either and does not pretend to be: it catches a payload that
//! no longer matches the hash the sender computed, which is corruption in
//! transit or a naive edit. It stops nobody who can also run SHA-256. It is
//! here because a poll is the one path into the cell that the publishing
//! endpoint's own check never covered — that check runs on the peer, over the
//! frames a publisher *sent it*, not over the frames this cell *pulled back*.
//!
//! # The mesh is one source, and every source gets the same checks
//!
//! Control does not have to arrive over the mesh. ADR 0100 §8 moves it onto
//! the event fabric "through the SAME verify code as the mesh downlink", and
//! [`FrameSource`] is where that sameness is held. Both downlinks take their
//! frames from a source — the mesh when built by `connect`, anything else
//! through `from_source` or `poll_from` — and every frame, whatever handed it
//! over, goes through the one absorb each downlink has: topic, schema
//! ceiling, payload hash, decode, the grant memory where the downlink keeps
//! one, signature. A source hands over [`AnyEvent`] frames and nothing else,
//! and the absorb takes a frame and nothing about where it came from, so
//! there is no argument by which one source could be checked less than
//! another. A second verification path is how an unsigned grant gets
//! through, and opening one here would first need a parameter this code
//! deliberately does not have.
//!
//! Moving the wire moves nothing about the signature. It is HMAC over a key
//! the cell shares with the centre, over the fabric exactly as over the mesh:
//! it proves possession of that key, not who held it (ADR 0043's first gap).
//!
//! # What is durable in which direction, and why they differ
//!
//! **Nothing on the uplink is spooled.** A state delta whose circuit is open is
//! not sent and not queued, and the honest accounting of what that costs is:
//! the cumulative half of the next delta — utilisation, halt state,
//! reconciliation breaks — is absolute rather than incremental, so the next
//! delta to arrive supersedes every one that did not. What is genuinely lost is
//! the incremental half, the orders and refusals of that interval, and those
//! are in the cell's hash-chained journal, which ships to durable storage on a
//! path that has nothing to do with this one. A delta is replaceable within a
//! cycle; paying a durable write for it would buy nothing and would put a store
//! between the cell and the wire.
//!
//! **The downward capital path is spooled, at the sending end.** That end is
//! the central plane, and the spool lives with it in `qip_mesh::spine`. It is
//! not here because there is nothing for a receiver to spool: a grant this cell
//! never received is a grant it cannot re-request, and the centre re-sends it
//! from its own spool. See [`qip_transport::spool`] for why persist-send-forget
//! is the ordering and what it converts a restart into.
//!
//! # Idempotency is this cell's own job
//!
//! Delivery is at-least-once in both directions and the peer's duplicate
//! detection is bounded by a window. Past that window a redelivery is
//! indistinguishable from a new message on the wire, and the consumer's own
//! idempotency is the only thing left — which is why [`CapitalDownlink`] keys
//! what it has applied on the *grant* rather than on anything the wire chose.
//! A signature covers every field that bounds what the cell may do, so two
//! frames carrying the same signature carry the same authority, whatever event
//! ids or idempotency keys were stamped on them in between.

use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::sync::Arc;

use qip_contracts::capital::{CapitalEnvelope, Utilisation};
use qip_contracts::intent::Contributor;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_contracts::wire::{CrossRecord, FillRecord, MAX_CROSSES_PER_DELTA, MAX_FILLS_PER_DELTA};
use qip_core::error::{Error, Result};
use qip_core::{Clock, CorrelationId, Decimal, Id, Lineage, ObjectId, Timestamp};
use qip_events::envelope::canonical_json;
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_transport::breaker::{
    BreakerPolicy, BreakerState, CircuitBreaker, Decision as BreakerDecision, Outcome, Refusal,
};
use qip_transport::deadletter::DeadLetterReason;
use qip_transport::error::TransportError;
use qip_transport::retry::Sleeper;
use qip_transport::{DeadLetterSink, Delivery, MeshConfig, MeshPublisher, RemoteSubscriber};
use serde::{Deserialize, Serialize};

use crate::envelope::VerifiedEnvelope;
use crate::policy::{VerifiedHalt, VerifiedPolicy};
use qip_contracts::policy::{HaltCommand, PolicyPayload};

/// How many refusals one delta carries before it starts counting instead.
///
/// A pass that refuses on every gate for every deployed strategy would
/// otherwise put an unbounded list on the wire, and the inbox at the far end is
/// bounded by frames rather than by bytes — so one pathological cell could fill
/// the centre's memory without ever exceeding its message count. The gates that
/// did not fit are counted in [`CellStateDelta::refusals_omitted`], because a
/// truncation nobody can see is worse than a smaller list.
const MAX_REFUSALS_PER_DELTA: usize = 64;

/// How many applied grants a downlink remembers by default.
///
/// The same shape as the inbox's dedup window and with the same consequence at
/// the bound: a redelivery arriving after this many newer grants is applied
/// again. Applying the same envelope twice installs the same bounded authority
/// twice, which changes nothing the cell can act on — the visible cost is a
/// second journal entry for one grant, which is why the number is generous
/// rather than tuned.
///
/// Public so a downlink built over a source other than the mesh
/// ([`CapitalDownlink::from_source`]) can name the same bound rather than
/// inventing its own.
pub const DEFAULT_GRANT_MEMORY: usize = 512;

// --- what a cell says about itself --------------------------------------

/// One order the cell sent during the interval a delta covers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeltaOrder {
    pub order_id: String,
    pub strategy: StrategyId,
    pub object_id: ObjectId,
    pub venue: VenueId,
    pub side: BookSide,
    pub quantity: Decimal,
    pub price: Decimal,
    /// Taken from the cell's own record of what the gateway said, never from
    /// the order. A paper fill counted as real is the single most consequential
    /// bit in the execution path, and it stays that way on the wire.
    pub simulated: bool,
    /// Every strategy whose intent went into this order, with its signed share
    /// and the feature revisions it reasoned from.
    ///
    /// `strategy` above is the largest contributor, kept so every existing
    /// reader still resolves one strategy. It stopped being the whole truth the
    /// moment netting collapsed several intents into one order, and a centre
    /// that attributes a netted fill to the largest contributor alone credits
    /// one strategy with another's trade.
    ///
    /// `#[serde(default)]` so a delta written before this field existed still
    /// decodes: the event log is sealed and hash-chained, and a replay that
    /// refused its own history would be worse than one that reads an older
    /// record as having named no contributors — which is exactly what it did.
    #[serde(default)]
    pub contributors: Vec<Contributor>,
}

/// What one strategy has committed against its envelope, absolute.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyUtilisation {
    pub strategy: StrategyId,
    pub utilisation: Utilisation,
    /// When the grant this utilisation is measured against runs out. The centre
    /// needs it to know which cells are about to stop trading on their own.
    pub envelope_expires_at: Timestamp,
}

/// A gate that refused, and what it said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeltaRefusal {
    pub gate: String,
    pub reason: String,
    /// The venue the refused intent was bound for, carried only for a
    /// feasibility refusal — the one kind of refusal that is *about* a venue
    /// — so the centre can keep the window blueprint §12.3's fourth row is
    /// judged over. Absent for every other gate, and absent on the wire
    /// rather than `null`, so a delta written before the field existed and
    /// one written by a gate that names no venue read the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venue: Option<String>,
}

/// What one cell tells the centre about itself.
///
/// # The two halves have different arithmetic, and are labelled
///
/// [`Self::utilisation`], [`Self::halted`] and [`Self::reconciliation_breaks`]
/// are **absolute**: they describe the cell as it stands, not what changed. A
/// centre that accumulated them would drift from the cell it is describing at
/// exactly the moment a message was lost, and aggregate exposure is the one
/// number that has to be right during an incident. This is the same reasoning
/// `qip_kernel`'s `CellReport` gives for carrying whole positions rather than
/// position deltas, and it is deliberately the same answer.
///
/// [`Self::orders`], [`Self::fills`] and [`Self::refusals`] are
/// **incremental**: they cover the interval since the previous delta. A
/// receiver that overwrote them would lose the orders of every interval it
/// did not sample.
///
/// # Orders are what was sent; fills are what traded
///
/// [`Self::orders`] is the record of what the cell *sent* and the venue
/// accepted. [`Self::fills`] is what the venue *reported filled*, and it is
/// the only list the centre may bill from. For one slice the centre read the
/// first as the second, and attributed, charged and settled orders that were
/// still resting or had expired unfilled — two claims about the same fact,
/// with the louder one wrong. A resting order is an open order and not a
/// position, and the wire now says which is which.
///
/// # It is not a position book
///
/// A cell holds books it rebuilt from a venue feed and fills it has recorded;
/// it does not hold a reconciled position book, and inventing one here would
/// hand the centre a number nobody has checked against a custodian. What
/// crosses is what the cell actually knows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellStateDelta {
    pub cell: String,
    pub region: String,
    /// This cell's own monotonic counter. It is the idempotency key, so a
    /// redelivered delta is one fact rather than two, and it is per cell
    /// because there is no global order on this transport to be part of.
    pub sequence: u64,
    pub at: Timestamp,
    /// Whether the cell has stopped itself. First field an incident reader
    /// looks at, and absolute for that reason.
    pub halted: bool,
    pub utilisation: Vec<StrategyUtilisation>,
    /// Orders the venue accepted this interval. Sent, not filled.
    pub orders: Vec<DeltaOrder>,
    /// Fills the venue reported this interval, on orders from this interval
    /// or an earlier one, each with the cell's own attribution — taken from
    /// the confirmed set, never inferred from `orders`.
    ///
    /// `#[serde(default)]` so a delta sealed before the field existed still
    /// replays, reading as having confirmed nothing — which is what it said.
    #[serde(default)]
    pub fills: Vec<FillRecord>,
    /// Fills that did not fit in [`MAX_FILLS_PER_DELTA`]. Each one is a
    /// fill the centre will never bill, so the count travels in the same
    /// delta rather than waiting for a reconciliation to find the gap.
    #[serde(default)]
    pub fills_omitted: u32,
    pub refusals: Vec<DeltaRefusal>,
    /// Refusals that did not fit in [`MAX_REFUSALS_PER_DELTA`].
    #[serde(default)]
    pub refusals_omitted: u32,
    /// Every disagreement between the cell's fills and the venue's own account,
    /// as the cell described them. Absolute: a break does not stop being true
    /// because a later delta was sent.
    pub reconciliation_breaks: Vec<String>,
    /// Breaks the cell recorded but no longer retains. Non-zero means the list
    /// above understates the incident, which the centre has to be told rather
    /// than left to infer from a suspiciously round number.
    #[serde(default)]
    pub reconciliation_breaks_omitted: u32,
    /// Every internal cross the cell booked in this interval (§27.1).
    ///
    /// Incremental, like `orders` and `refusals`: a receiver adds these rather
    /// than replacing what it holds. Until this existed the centre heard a
    /// cross *refusal* — refusals travel — and never the cross itself, so the
    /// one thing §27.1 calls a ledger entry and a regulatory expectation was
    /// the only part of the story that stopped at the cell. In a plane whose
    /// whole design is that a region keeps working while cut off from the
    /// centre, that is exactly the record that has to survive the partition.
    #[serde(default)]
    pub crosses: Vec<CrossRecord>,
    /// Crosses that did not fit in [`MAX_CROSSES_PER_DELTA`].
    #[serde(default)]
    pub crosses_omitted: u32,
}

impl EventBody for CellStateDelta {
    /// There is no `CellStateReported` topic and adding one would mean editing
    /// `qip-events`, which every crate in the workspace shares. `PositionUpdated`
    /// is the topic whose meaning this carries — what a cell holds and what it
    /// has committed — and it is in the `Act` group, which
    /// `Topic::requires_permanent_retention` already keeps forever.
    const TOPIC: Topic = Topic::PositionUpdated;
    /// Declared once in `qip-contracts`, which both ends of the uplink already
    /// depend on, so this end and the centre's cannot drift apart. The bump to
    /// two is what makes an old centre refuse a new cell's delta outright
    /// instead of decoding it and silently attributing every netted fill to one
    /// strategy — `contributors` is defaulted for replay of sealed records, and
    /// a default a live peer could reach would be a silent wrong answer.
    const SCHEMA_VERSION: u32 = qip_contracts::wire::CELL_DELTA_SCHEMA_VERSION;

    /// Cell and sequence, so a retry that the peer already accepted is
    /// recognised rather than counted twice.
    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.cell, self.sequence))
    }
}

impl CellStateDelta {
    /// Wrap this delta in the platform's own event envelope.
    ///
    /// The event id and correlation id are derived from the cell and the
    /// sequence rather than minted, so a delta that is rebuilt after a restart
    /// and re-sent carries the identity it had the first time. A freshly minted
    /// id would make the same fact look like a new one to everything downstream
    /// that keys on identity.
    pub fn to_frame(&self) -> Result<AnyEvent> {
        Envelope::new(
            Id::from_string(format!("EVTCELL{}{:0>20}", self.cell, self.sequence)),
            self.at,
            self.at,
            Lineage::root(
                CorrelationId::from_string(format!("CORCELL{}{:0>20}", self.cell, self.sequence)),
                "qip-edge",
            ),
            self.clone(),
        )
        .erase()
    }

    /// Truncate the refusal list to what may travel, counting the rest.
    pub(crate) fn bound_refusals(&mut self) {
        if self.refusals.len() > MAX_REFUSALS_PER_DELTA {
            let omitted = self.refusals.len() - MAX_REFUSALS_PER_DELTA;
            self.refusals.truncate(MAX_REFUSALS_PER_DELTA);
            self.refusals_omitted = u32::try_from(omitted).unwrap_or(u32::MAX);
        }
        // Crosses are bounded on the same principle and counted separately.
        // Sharing the refusal budget would let a noisy gate silently evict a
        // ledger entry, which is the one record here that an examiner asks for
        // by name.
        if self.crosses.len() > MAX_CROSSES_PER_DELTA {
            let omitted = self.crosses.len() - MAX_CROSSES_PER_DELTA;
            self.crosses.truncate(MAX_CROSSES_PER_DELTA);
            self.crosses_omitted = u32::try_from(omitted).unwrap_or(u32::MAX);
        }
        // Fills have their own budget for the same reason, and the stakes
        // are higher: a fill evicted by a noisy gate would be a trade the
        // centre never bills, so it must be counted here and not shared.
        if self.fills.len() > MAX_FILLS_PER_DELTA {
            let omitted = self.fills.len() - MAX_FILLS_PER_DELTA;
            self.fills.truncate(MAX_FILLS_PER_DELTA);
            self.fills_omitted = u32::try_from(omitted).unwrap_or(u32::MAX);
        }
    }
}

// --- the uplink ---------------------------------------------------------

/// How a cell's uplink is built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UplinkConfig {
    pub cell: String,
    pub region: String,
    /// The peer, the retry ladder, the queue bound and the jitter seed.
    pub mesh: MeshConfig,
    pub breaker: BreakerPolicy,
    /// Separate from [`MeshConfig::seed`] so the retry jitter and the cooldown
    /// jitter of one cell are not the same sequence drawn twice.
    pub breaker_seed: u64,
}

impl UplinkConfig {
    pub fn new(cell: impl Into<String>, region: impl Into<String>, mesh: MeshConfig) -> Self {
        Self {
            cell: cell.into(),
            region: region.into(),
            mesh,
            breaker: BreakerPolicy::default(),
            breaker_seed: 0,
        }
    }

    pub fn with_breaker(mut self, policy: BreakerPolicy, seed: u64) -> Self {
        self.breaker = policy;
        self.breaker_seed = seed;
        self
    }
}

/// What happened to one delta.
///
/// Three outcomes rather than a `Result`, because two of them are not failures
/// of this cell and the third is not a failure it can do anything about. A
/// caller that wants to alert needs to tell "the peer is down and we knew"
/// from "the peer is down and we just found out again".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dispatch {
    /// The peer took it.
    Delivered(Delivery),
    /// The circuit to the peer is open. Nothing was sent, no retry ladder was
    /// spent, and this delta is gone: the next one supersedes it.
    CircuitOpen(Refusal),
    /// Every attempt the ladder allowed failed. The frame is in the
    /// dead-letter sink with its reason, and this delta is gone for the same
    /// reason it is survivable — the next one carries the absolute state again.
    DeadLettered {
        key: String,
        attempts: u32,
        reason: DeadLetterReason,
        last_error: String,
    },
}

impl Dispatch {
    pub const fn is_delivered(&self) -> bool {
        matches!(self, Self::Delivered(_))
    }

    /// A stable code for a metric or a test that asserts the outcome rather
    /// than matching on prose.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Delivered(_) => "delivered",
            Self::CircuitOpen(_) => "circuit_open",
            Self::DeadLettered { .. } => "dead_lettered",
        }
    }
}

/// Counters an operator needs from the uplink.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UplinkStats {
    pub published: u64,
    pub delivered: u64,
    /// Deltas the circuit refused without touching the network. The number the
    /// breaker exists to make non-zero during an outage.
    pub circuit_refusals: u64,
    /// Deltas that spent the whole ladder and were recorded as dead letters.
    /// One of the two numbers that say a delta never arrived.
    pub dead_lettered: u64,
}

/// The cell's outbound half: state deltas to the central plane.
///
/// `&mut self` throughout, exactly as [`MeshPublisher`] takes it: one uplink is
/// one ordered stream, and that per-publisher FIFO is the only ordering this
/// transport offers.
#[derive(Debug)]
pub struct CellUplink {
    cell: String,
    region: String,
    peer: String,
    publisher: MeshPublisher,
    breaker: CircuitBreaker,
    sequence: u64,
    stats: UplinkStats,
}

impl CellUplink {
    /// Build the uplink.
    ///
    /// The clock, the sleeper and the dead-letter sink are parameters for the
    /// reason `qip-transport` makes them parameters: they are where this would
    /// otherwise reach for something ambient, and each is what a test has to
    /// replace to assert on a retry ladder instead of spending it.
    pub fn connect(
        config: UplinkConfig,
        clock: Arc<dyn Clock>,
        sleeper: Arc<dyn Sleeper>,
        dead_letters: Box<dyn DeadLetterSink>,
    ) -> Result<Self> {
        let breaker = CircuitBreaker::new(
            config.breaker,
            Arc::clone(&clock),
            config.breaker_seed,
            // One peer. The bound exists at all because the breaker's key is an
            // address, and a cell publishes to exactly one central plane.
            4,
        )?;
        let publisher = MeshPublisher::new(config.mesh, clock, sleeper, dead_letters)?;
        Ok(Self {
            cell: config.cell,
            region: config.region,
            peer: publisher.peer().to_string(),
            publisher,
            breaker,
            sequence: 0,
            stats: UplinkStats::default(),
        })
    }

    pub fn cell(&self) -> &str {
        &self.cell
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn peer(&self) -> &str {
        &self.peer
    }

    pub const fn stats(&self) -> UplinkStats {
        self.stats
    }

    /// The next sequence a delta will be stamped with.
    pub const fn next_sequence(&self) -> u64 {
        self.sequence + 1
    }

    /// Where the circuit to the central plane is.
    pub fn circuit(&self) -> BreakerState {
        self.breaker.state(&self.peer)
    }

    /// The publisher, for the counters and dead letters this type does not
    /// restate.
    pub const fn publisher(&self) -> &MeshPublisher {
        &self.publisher
    }

    /// Stamp a delta with the next sequence and publish it.
    ///
    /// The sequence is assigned here rather than by the caller so that one
    /// uplink's stream is gapless by construction: a caller that assigned its
    /// own would eventually skip one, and the centre would have no way to tell
    /// a skipped sequence from a lost delta.
    ///
    /// A delta that already names a *different* cell is refused rather than
    /// relabelled. An uplink configured for one cell and handed another's state
    /// is a deployment mistake, and quietly restamping it would file one cell's
    /// positions under another's name — which is worse than the delta not
    /// arriving, because the centre would believe it.
    pub fn publish(&mut self, mut delta: CellStateDelta, at: Timestamp) -> Result<Dispatch> {
        if !delta.cell.is_empty() && delta.cell != self.cell {
            return Err(Error::denied(format!(
                "the uplink for {} was handed a delta belonging to {}",
                self.cell, delta.cell
            )));
        }
        self.sequence += 1;
        delta.cell = self.cell.clone();
        delta.region = self.region.clone();
        delta.sequence = self.sequence;
        delta.bound_refusals();
        let frame = delta.to_frame()?;

        // Admissibility is settled before the breaker is consulted, because a
        // frame that may not travel this way at all is a fact about the frame
        // and feeding it to a circuit would open one to a healthy peer.
        if let Some(detail) = MeshPublisher::refusal(&frame) {
            return Err(Error::invalid(detail));
        }

        let permit = match self.breaker.admit(&self.peer) {
            BreakerDecision::Refused(refusal) => {
                self.stats.circuit_refusals += 1;
                return Ok(Dispatch::CircuitOpen(refusal));
            }
            BreakerDecision::Admitted(permit) => permit,
        };

        self.stats.published += 1;
        // `publish_frame` enqueues and flushes in one call, so the outbound
        // queue is empty on entry and one frame always fits: `OutboundQueue`
        // refuses a zero capacity outright, so `QueueFull` cannot arise here.
        match self.publisher.publish_frame(frame, at) {
            Ok(delivery) => {
                self.breaker.record(permit, Outcome::Success);
                self.stats.delivered += 1;
                Ok(Dispatch::Delivered(delivery))
            }
            Err(TransportError::DeadLettered {
                key,
                attempts,
                reason,
                last_error,
            }) => {
                // A peer that read the message and refused it on its merits is
                // a peer that is up. Counting that as a circuit failure would
                // open a circuit to a healthy central plane because this cell
                // sent it something it did not like.
                let outcome = match reason {
                    DeadLetterReason::Rejected => Outcome::Success,
                    DeadLetterReason::RetriesExhausted | DeadLetterReason::PermanentFailure => {
                        Outcome::Failure(last_error.clone())
                    }
                };
                self.breaker.record(permit, outcome);
                self.stats.dead_lettered += 1;
                Ok(Dispatch::DeadLettered {
                    key,
                    attempts,
                    reason,
                    last_error,
                })
            }
            Err(error) => {
                self.breaker.record(permit, Outcome::failed(&error));
                Err(Error::from(error))
            }
        }
    }
}

// --- where downlink frames come from ------------------------------------

/// What one ask of a [`FrameSource`] produced.
///
/// Two arms rather than an empty list standing in for the second, because
/// "the source had nothing" and "the source was not asked" are different
/// facts: the second is an outage the circuit already knows about, and a
/// downlink that reported it as an empty batch would tell its caller the
/// centre had gone quiet.
#[derive(Clone, Debug, PartialEq)]
pub enum SourcePoll {
    /// Every frame the source knew at or before the instant asked about, in
    /// the order it knew them. Unverified: a source hands over frames, never
    /// authority.
    Frames(Vec<AnyEvent>),
    /// The source declined to be asked — a mesh circuit that is open — and
    /// nothing was fetched.
    CircuitOpen(Refusal),
}

/// Where a downlink's frames come from: the mesh, or anything else that can
/// hand over [`AnyEvent`] frames — a scripted list of event-fabric records
/// among them.
///
/// # A source delivers frames; it never delivers authority
///
/// The only thing a source can produce is an [`AnyEvent`], and the only
/// routes from one to a [`VerifiedEnvelope`], a [`VerifiedPolicy`] or a
/// [`VerifiedHalt`] are those types' own `verify`, called from one absorb per
/// downlink that takes the frame and nothing about its origin. A source
/// cannot construct a verified value and cannot tell the absorb it is
/// special, so it cannot be the second verification path through which an
/// unsigned grant reaches the cell.
///
/// It takes frames rather than fabric records so that `qip-edge` gains no
/// event-fabric type or dependency: an adapter on the far side of this seam
/// hands over the frames its records carry, and the cell never learns which
/// wire they rode.
pub trait FrameSource: fmt::Debug + Send {
    /// Take every frame this source knew at or before `now`.
    ///
    /// `now` bounds what is returned for the reason the mesh's own poll is
    /// bounded: the downlink verifies at `now`, and a frame the source only
    /// learned of later must not be judged at an instant before it was
    /// knowable.
    fn fetch(&mut self, now: Timestamp) -> Result<SourcePoll>;

    /// Where the circuit to this source stands, for a health surface.
    fn circuit(&self) -> BreakerState;
}

/// A scripted list of frames, as a [`FrameSource`].
///
/// Event-fabric records replayed in order, or a test's script. Frames are
/// handed over in the order they were pushed, up to the first one recorded
/// after the instant asked about — the rule the mesh inbox's own read
/// follows — so a frame stamped later than `now` is held rather than judged
/// early, and one pushed behind it waits with it rather than being jumped
/// over. Each frame is handed over once.
///
/// Nothing here deduplicates, on purpose. The memory that makes a grant
/// applied once belongs to the downlink and is keyed on the grant's
/// signature; a second memory here would be a second place deciding "applied
/// once", keyed on something other than the grant, and a grant arriving by
/// this source and the mesh would be judged by two memories that cannot see
/// each other.
#[derive(Debug)]
pub struct ScriptedFrames {
    capacity: usize,
    pending: VecDeque<AnyEvent>,
}

impl ScriptedFrames {
    /// A source that holds at most `capacity` frames not yet fetched.
    ///
    /// Bounded for the reason every buffer in the mesh is: a list that grows
    /// with whatever it is handed is an unbounded allocation on the path
    /// control arrives by.
    pub fn new(capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a scripted source that can hold no frame refuses every push, which presents as a \
                 centre that never sends rather than as a misconfiguration; give it a capacity",
            ));
        }
        Ok(Self {
            capacity,
            pending: VecDeque::new(),
        })
    }

    /// Queue one frame behind those already pushed.
    ///
    /// Refused rather than evicting the oldest when full: an evicted frame
    /// might be the halt, and a script that silently lost its first line is
    /// worse than one that was told to fetch before pushing more.
    pub fn push(&mut self, frame: AnyEvent) -> Result<()> {
        if self.pending.len() >= self.capacity {
            return Err(Error::invalid(format!(
                "the scripted source already holds {} frames, its bound; fetch before pushing more",
                self.capacity
            )));
        }
        self.pending.push_back(frame);
        Ok(())
    }

    /// How many frames are waiting to be fetched.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

impl FrameSource for ScriptedFrames {
    fn fetch(&mut self, now: Timestamp) -> Result<SourcePoll> {
        let knowable = self
            .pending
            .iter()
            .take_while(|frame| frame.recorded_at <= now)
            .count();
        Ok(SourcePoll::Frames(self.pending.drain(..knowable).collect()))
    }

    /// Always closed: a list in memory has no peer to be unreachable, so
    /// there is no circuit that could open, and closed is the state in which
    /// every ask is answered — which is what this source does.
    fn circuit(&self) -> BreakerState {
        BreakerState::Closed
    }
}

/// The mesh, as a [`FrameSource`]: the subscription and the circuit to its
/// peer.
///
/// The circuit lives with the source rather than the downlink because it is
/// a fact about a wire, and a scripted list has none. It is keyed by the
/// peer's address rather than the subscription's name: two subscriptions
/// against one unreachable plane are one outage, and a breaker keyed by name
/// would discover it twice.
#[derive(Debug)]
struct MeshFrames {
    peer: String,
    subscriber: RemoteSubscriber,
    breaker: CircuitBreaker,
}

impl MeshFrames {
    fn connect(
        mesh: MeshConfig,
        policy: BreakerPolicy,
        seed: u64,
        clock: Arc<dyn Clock>,
        sleeper: Arc<dyn Sleeper>,
    ) -> Result<Self> {
        // One peer, as for the uplink's breaker: the bound exists at all
        // because the breaker's key is an address.
        let breaker = CircuitBreaker::new(policy, clock, seed, 4)?;
        let peer = mesh.peer.clone();
        let subscriber = RemoteSubscriber::new(mesh, sleeper)?;
        Ok(Self {
            peer,
            subscriber,
            breaker,
        })
    }
}

impl FrameSource for MeshFrames {
    fn fetch(&mut self, now: Timestamp) -> Result<SourcePoll> {
        let permit = match self.breaker.admit(&self.peer) {
            BreakerDecision::Refused(refusal) => return Ok(SourcePoll::CircuitOpen(refusal)),
            BreakerDecision::Admitted(permit) => permit,
        };
        match self.subscriber.poll(now) {
            Ok(frames) => {
                self.breaker.record(permit, Outcome::Success);
                Ok(SourcePoll::Frames(frames))
            }
            Err(error) => {
                self.breaker.record(permit, Outcome::failed(&error));
                Err(Error::from(error))
            }
        }
    }

    fn circuit(&self) -> BreakerState {
        self.breaker.state(&self.peer)
    }
}

// --- the downlink -------------------------------------------------------

/// How a cell's capital downlink is built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownlinkConfig {
    /// The cell an envelope must name. An envelope for another cell is a replay
    /// and is refused even though its signature is genuine.
    pub cell: String,
    pub mesh: MeshConfig,
    pub breaker: BreakerPolicy,
    pub breaker_seed: u64,
    /// How many applied grants to remember. See [`DEFAULT_GRANT_MEMORY`].
    pub grant_memory: usize,
}

impl DownlinkConfig {
    pub fn new(cell: impl Into<String>, mesh: MeshConfig) -> Self {
        Self {
            cell: cell.into(),
            mesh,
            breaker: BreakerPolicy::default(),
            breaker_seed: 0,
            grant_memory: DEFAULT_GRANT_MEMORY,
        }
    }

    pub fn with_breaker(mut self, policy: BreakerPolicy, seed: u64) -> Self {
        self.breaker = policy;
        self.breaker_seed = seed;
        self
    }

    pub fn with_grant_memory(mut self, grants: usize) -> Self {
        self.grant_memory = grants;
        self
    }
}

/// An envelope that arrived and was not accepted, and why.
///
/// Carried rather than logged and dropped, because "the centre thinks this cell
/// has capital and the cell disagrees" is the disagreement that presents as a
/// cell mysteriously trading nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedGrant {
    /// The frame's own event id, so the refusal can be matched against what the
    /// centre says it sent even when the payload was unreadable.
    pub event_id: String,
    pub reason: String,
}

/// Counters for the downlink.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownlinkStats {
    pub polls: u64,
    pub frames: u64,
    pub verified: u64,
    /// Grants recognised as ones this cell has already applied.
    pub duplicates: u64,
    /// Grants that did not verify. Non-zero means somebody is sending this cell
    /// capital it will not act on, which is an incident either way.
    pub refused: u64,
    /// Frames on the inbox that were not capital grants at all.
    pub ignored: u64,
    /// Applied grants forgotten because the memory filled. Past this a
    /// redelivery is applied again.
    pub forgotten_grants: u64,
}

/// What one poll brought back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DownlinkBatch {
    /// Envelopes this cell verified against its own key, in arrival order.
    pub verified: Vec<VerifiedEnvelope>,
    /// Grants recognised as already applied, by grant key.
    pub duplicates: Vec<String>,
    pub refused: Vec<RefusedGrant>,
    /// Set when the circuit to the peer is open, in which case no poll was
    /// made. Distinct from an empty batch, which means the peer had nothing.
    pub circuit_open: Option<Refusal>,
}

impl DownlinkBatch {
    pub fn is_empty(&self) -> bool {
        self.verified.is_empty() && self.duplicates.is_empty() && self.refused.is_empty()
    }
}

/// The cell's inbound half: signed capital envelopes from the central plane.
#[derive(Debug)]
pub struct CapitalDownlink {
    cell: String,
    /// The shared secret the cell verifies grants against. An empty key is
    /// refused at construction: a cell that cannot verify capital must not
    /// trade, and one that accepted an empty key would verify nothing while
    /// looking like it verified everything.
    key: Vec<u8>,
    /// Where [`Self::poll`] takes frames from — the mesh, when built by
    /// [`Self::connect`].
    source: Box<dyn FrameSource>,
    applied: BTreeSet<String>,
    applied_order: VecDeque<String>,
    grant_memory: usize,
    stats: DownlinkStats,
}

impl CapitalDownlink {
    /// A downlink that polls the mesh.
    pub fn connect(
        config: DownlinkConfig,
        key: &[u8],
        clock: Arc<dyn Clock>,
        sleeper: Arc<dyn Sleeper>,
    ) -> Result<Self> {
        // Before the mesh is touched, so a downlink with no key and a bad
        // peer is refused for the key: the fault that would make it deliver
        // unchecked grants outranks the one that would make it deliver none.
        Self::refuse_unverifiable(key, config.grant_memory)?;
        let mesh = MeshFrames::connect(
            config.mesh,
            config.breaker,
            config.breaker_seed,
            clock,
            sleeper,
        )?;
        Self::from_source(config.cell, key, config.grant_memory, Box::new(mesh))
    }

    /// A downlink that polls any [`FrameSource`] — the event fabric's
    /// control records, a scripted list — through the same checks, the same
    /// grant memory and the same key refusal [`Self::connect`] applies.
    ///
    /// [`Self::connect`] builds its downlink through this, so the refusal of
    /// an empty key or an empty memory is one piece of code for every source
    /// rather than a check a second constructor could forget.
    pub fn from_source(
        cell: impl Into<String>,
        key: &[u8],
        grant_memory: usize,
        source: Box<dyn FrameSource>,
    ) -> Result<Self> {
        Self::refuse_unverifiable(key, grant_memory)?;
        Ok(Self {
            cell: cell.into(),
            key: key.to_vec(),
            source,
            applied: BTreeSet::new(),
            applied_order: VecDeque::new(),
            grant_memory,
            stats: DownlinkStats::default(),
        })
    }

    fn refuse_unverifiable(key: &[u8], grant_memory: usize) -> Result<()> {
        if key.is_empty() {
            return Err(Error::denied(
                "a downlink with no envelope key cannot verify a grant, so every envelope it \
                 delivered would be one nobody checked",
            ));
        }
        if grant_memory == 0 {
            return Err(Error::invalid(
                "a downlink that remembers no applied grant treats every redelivery as new, \
                 which is the duplicate the memory exists to absorb",
            ));
        }
        Ok(())
    }

    pub fn cell(&self) -> &str {
        &self.cell
    }

    pub const fn stats(&self) -> DownlinkStats {
        self.stats
    }

    /// Where the circuit to this downlink's own source stands.
    pub fn circuit(&self) -> BreakerState {
        self.source.circuit()
    }

    /// Whether this grant has already been applied.
    pub fn has_applied(&self, envelope: &CapitalEnvelope) -> bool {
        self.applied.contains(&grant_key(envelope))
    }

    /// Pull everything the centre knew by `now`, verifying each grant.
    ///
    /// `now` is both the upper bound on what is returned and the instant the
    /// envelopes are verified against, which is the same instant on purpose: an
    /// envelope that expired before the caller's clock reached it must not be
    /// admitted because the poll boundary was drawn somewhere else.
    ///
    /// One unverifiable envelope does not fail the poll. It is recorded in
    /// [`DownlinkBatch::refused`] and the rest of the batch is delivered,
    /// because a centre that sent one bad grant has not thereby revoked the
    /// good ones — and a poll that returned `Err` would leave the caller unable
    /// to say which.
    pub fn poll(&mut self, now: Timestamp) -> Result<DownlinkBatch> {
        let fetched = self.source.fetch(now);
        self.receive(fetched, now)
    }

    /// Take frames from a second source through this downlink's own checks
    /// and its own grant memory.
    ///
    /// For a cell that hears the centre on two wires at once — the mesh and
    /// the event fabric, while control moves from one to the other. It is a
    /// method on this downlink rather than a second downlink because a grant
    /// the centre published on both wires is one grant, and only one memory
    /// can say so: two downlinks would each apply it, and nothing in the two
    /// frames — different event ids, different wires — would tell them they
    /// carried the same authority.
    pub fn poll_from(
        &mut self,
        source: &mut dyn FrameSource,
        now: Timestamp,
    ) -> Result<DownlinkBatch> {
        let fetched = source.fetch(now);
        self.receive(fetched, now)
    }

    /// Everything one ask of a source produced, through [`Self::absorb`].
    fn receive(&mut self, fetched: Result<SourcePoll>, now: Timestamp) -> Result<DownlinkBatch> {
        let frames = match fetched {
            Ok(SourcePoll::CircuitOpen(refusal)) => {
                return Ok(DownlinkBatch {
                    circuit_open: Some(refusal),
                    ..DownlinkBatch::default()
                });
            }
            Ok(SourcePoll::Frames(frames)) => {
                self.stats.polls += 1;
                frames
            }
            // A source that was asked and failed was still polled: the
            // counter says how often the centre was asked, not how often it
            // answered.
            Err(error) => {
                self.stats.polls += 1;
                return Err(error);
            }
        };

        let mut batch = DownlinkBatch::default();
        for frame in &frames {
            self.stats.frames += 1;
            self.absorb(frame, now, &mut batch);
        }
        Ok(batch)
    }

    /// Take one frame through every check, or refuse it.
    ///
    /// The one absorb for every source. It takes the frame and not the
    /// source, so a frame from the fabric and a frame from the mesh cannot be
    /// told apart here — which is the property: a check that could depend on
    /// the wire is a check one wire could skip.
    fn absorb(&mut self, frame: &AnyEvent, now: Timestamp, batch: &mut DownlinkBatch) {
        if frame.topic != CapitalGrantTopic::TOPIC {
            // Not addressed to this concern. Counted rather than refused: an
            // inbox shared with another consumer is a deployment's choice and
            // not this cell's business.
            self.stats.ignored += 1;
            return;
        }
        let event_id = frame.event_id.as_str().to_string();

        if frame.schema_version > CapitalGrantTopic::SCHEMA_VERSION {
            // The same rule `AnyEvent::decode` applies, applied here because
            // this path decodes by hand: silently ignoring fields a newer
            // centre added would mean acting on a grant this cell only
            // partially understands, and the fields it did not understand are
            // the ones that bound it.
            self.refuse(
                batch,
                event_id,
                &format!(
                    "the grant was written by schema version {} and this cell understands {}",
                    frame.schema_version,
                    CapitalGrantTopic::SCHEMA_VERSION
                ),
            );
            return;
        }

        if qip_core::hash::sha256_hex(canonical_json(&frame.payload).as_bytes())
            != frame.payload_hash
        {
            self.refuse(
                batch,
                event_id,
                "the payload no longer matches the hash the sender computed, so it was corrupted \
                 or edited between the centre and this cell",
            );
            return;
        }

        let envelope: CapitalEnvelope = match serde_json::from_value(frame.payload.clone()) {
            Ok(envelope) => envelope,
            Err(error) => {
                self.refuse(
                    batch,
                    event_id,
                    &format!("the frame does not carry a capital envelope: {error}"),
                );
                return;
            }
        };

        let key = grant_key(&envelope);
        if self.applied.contains(&key) {
            self.stats.duplicates += 1;
            batch.duplicates.push(key);
            return;
        }

        // The whole point of the module. Arriving over the mesh, or over any
        // other source, has bought this envelope nothing; it is verified here
        // exactly as one handed over by any other route would be.
        match VerifiedEnvelope::verify(envelope, &self.key, &self.cell, now) {
            Ok(verified) => {
                self.remember(key);
                self.stats.verified += 1;
                batch.verified.push(verified);
            }
            // Deliberately not remembered. A grant refused now for a reason
            // that is about *now* — it is not yet live, or the clock had not
            // reached its window — must be allowed to verify on a later
            // delivery, and remembering the refusal would make a transient
            // condition permanent.
            Err(error) => self.refuse(batch, event_id, error.message()),
        }
    }

    fn refuse(&mut self, batch: &mut DownlinkBatch, event_id: String, reason: &str) {
        self.stats.refused += 1;
        batch.refused.push(RefusedGrant {
            event_id,
            reason: reason.to_string(),
        });
    }

    /// Remember a grant, forgetting the oldest once the memory is full.
    ///
    /// Bounded because the key comes off the wire and this transport
    /// authenticates nobody: a set keyed by whatever a peer sends is an
    /// unbounded allocation, which is the failure every other bound in the mesh
    /// refuses.
    fn remember(&mut self, key: String) {
        self.applied.insert(key.clone());
        self.applied_order.push_back(key);
        while self.applied_order.len() > self.grant_memory {
            if let Some(oldest) = self.applied_order.pop_front() {
                self.applied.remove(&oldest);
                self.stats.forgotten_grants += 1;
            }
        }
    }
}

/// The identity of a grant, independent of how it was framed.
///
/// The signature covers every field that bounds what the cell may do, so two
/// frames carrying the same signature carry the same authority however they
/// were addressed. Keying on this rather than on the frame's idempotency key is
/// what makes receipt idempotent past the transport's own dedup window: the
/// wire's key is the sender's choice and its memory is bounded, while the
/// grant's signature is the fact itself. The cell is included because a
/// signature is only meaningful together with the cell it was issued for.
fn grant_key(envelope: &CapitalEnvelope) -> String {
    format!(
        "{}|{}|{}",
        envelope.cell(),
        envelope.strategy().as_str(),
        envelope.signature()
    )
}

/// The topic a policy payload travels on, named once — the same zero-sized
/// carrier pattern as [`CapitalGrantTopic`], for the same reason: the two ends
/// of the mesh agree through a shared constant rather than a shared frame
/// type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyPayloadTopic;

impl PolicyPayloadTopic {
    /// `Topic::PolicyDistributed` exists for exactly this: the twelve-item
    /// shipment of blueprint §41.5. It sits in the Decide group, so every
    /// payload is retained permanently — a policy a cell acted under is an
    /// audit fact.
    pub const TOPIC: Topic = Topic::PolicyDistributed;
    /// Bumped when the wire shape of a payload changes; the downlink refuses a
    /// payload written by a newer schema than it understands, because the
    /// fields it would not understand are the ones that narrow it.
    pub const SCHEMA_VERSION: u32 = 1;
}

/// The topic a halt command travels on. `Topic::KillSwitchEngaged` is
/// exactly what the frame means, and its retention is permanent by name — a
/// halt somebody sent is an audit fact whether or not it arrived.
///
/// A separate topic from the payload on purpose: a halt is a command, not
/// staleness, and it must be deliverable when the payload pipeline is exactly
/// the thing a bad deploy has wedged. Same fabric, same key, different code
/// path — mechanism independence, honestly short of the blueprint's two
/// independent *wires*, which needs a managed-store path and is recorded as
/// backlog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HaltTopic;

impl HaltTopic {
    pub const TOPIC: Topic = Topic::KillSwitchEngaged;
    pub const SCHEMA_VERSION: u32 = 1;
}

/// A policy payload the downlink could not deliver, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedPolicy {
    pub event_id: String,
    pub reason: String,
}

/// What one policy poll brought back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PolicyBatch {
    /// Payloads this cell verified against its own key, in arrival order.
    /// Sequence discipline is the *cell's* job at application, not the
    /// downlink's: the downlink proves authenticity and address, and the cell
    /// owns what it last applied.
    pub verified: Vec<VerifiedPolicy>,
    /// Halt commands this cell verified, in arrival order. Delivered beside
    /// the payloads rather than instead of them: a poll that found both must
    /// hand the caller both, and the caller applies halts first because a
    /// halt is never improved by waiting.
    pub halts: Vec<VerifiedHalt>,
    pub refused: Vec<RefusedPolicy>,
    /// Set when the circuit to the peer is open, in which case no poll was
    /// made. Distinct from an empty batch, which means the peer had nothing.
    pub circuit_open: Option<Refusal>,
}

impl PolicyBatch {
    pub fn is_empty(&self) -> bool {
        self.verified.is_empty() && self.halts.is_empty() && self.refused.is_empty()
    }
}

/// Statistics the policy downlink keeps about itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PolicyDownlinkStats {
    pub polls: u64,
    pub frames: u64,
    pub verified: u64,
    pub refused: u64,
    pub ignored: u64,
}

/// The cell's inbound half for policy: signed twelve-item payloads from the
/// central plane.
///
/// A deliberate mirror of [`CapitalDownlink`] — poll, topic filter, schema
/// ceiling, integrity hash, decode, verify, refuse-and-record, and the same
/// [`FrameSource`] seam in front of them — because the payload deserves
/// exactly the guard capital has and a second, different discipline would be
/// a second thing to get wrong.
#[derive(Debug)]
pub struct PolicyDownlink {
    cell: String,
    key: Vec<u8>,
    /// Where [`Self::poll`] takes frames from — the mesh, when built by
    /// [`Self::connect`].
    source: Box<dyn FrameSource>,
    stats: PolicyDownlinkStats,
}

impl PolicyDownlink {
    /// A downlink that polls the mesh.
    pub fn connect(
        config: DownlinkConfig,
        key: &[u8],
        clock: Arc<dyn Clock>,
        sleeper: Arc<dyn Sleeper>,
    ) -> Result<Self> {
        // Before the mesh is touched, for the reason `CapitalDownlink::connect`
        // gives: the fault that would deliver unchecked payloads outranks the
        // one that would deliver none.
        Self::refuse_unverifiable(key)?;
        let mesh = MeshFrames::connect(
            config.mesh,
            config.breaker,
            config.breaker_seed,
            clock,
            sleeper,
        )?;
        Self::from_source(config.cell, key, Box::new(mesh))
    }

    /// A downlink that polls any [`FrameSource`] through the same checks and
    /// the same key refusal [`Self::connect`] applies — which builds its own
    /// downlink through this, so there is one refusal of an empty key for
    /// every source.
    pub fn from_source(
        cell: impl Into<String>,
        key: &[u8],
        source: Box<dyn FrameSource>,
    ) -> Result<Self> {
        Self::refuse_unverifiable(key)?;
        Ok(Self {
            cell: cell.into(),
            key: key.to_vec(),
            source,
            stats: PolicyDownlinkStats::default(),
        })
    }

    fn refuse_unverifiable(key: &[u8]) -> Result<()> {
        if key.is_empty() {
            return Err(Error::denied(
                "a downlink with no policy key cannot verify a payload, so every payload it \
                 delivered would be one nobody checked",
            ));
        }
        Ok(())
    }

    pub fn cell(&self) -> &str {
        &self.cell
    }

    pub const fn stats(&self) -> PolicyDownlinkStats {
        self.stats
    }

    /// Where the circuit to this downlink's own source stands.
    pub fn circuit(&self) -> BreakerState {
        self.source.circuit()
    }

    /// Pull everything the centre knew by `now`, verifying each payload.
    ///
    /// One unverifiable payload does not fail the poll, for the same reason
    /// one bad grant does not: a centre that sent one bad payload has not
    /// revoked the good ones, and an `Err` would leave the caller unable to
    /// say which was which.
    pub fn poll(&mut self, now: Timestamp) -> Result<PolicyBatch> {
        let fetched = self.source.fetch(now);
        self.receive(fetched, now)
    }

    /// Take frames from a second source through this downlink's own checks.
    ///
    /// The payload has no memory here to share — "last applied" is the
    /// cell's fact, held where it applies — so what this keeps in one place
    /// is the key, the cell a frame must name and the counters. A halt that
    /// rides the fabric while payloads still ride the mesh is then verified
    /// against the same key and counted in the same place as every other
    /// frame the centre sent, rather than by a second downlink whose numbers
    /// an operator would have to know to add up.
    pub fn poll_from(
        &mut self,
        source: &mut dyn FrameSource,
        now: Timestamp,
    ) -> Result<PolicyBatch> {
        let fetched = source.fetch(now);
        self.receive(fetched, now)
    }

    /// Everything one ask of a source produced, through [`Self::absorb`].
    fn receive(&mut self, fetched: Result<SourcePoll>, now: Timestamp) -> Result<PolicyBatch> {
        let frames = match fetched {
            Ok(SourcePoll::CircuitOpen(refusal)) => {
                return Ok(PolicyBatch {
                    circuit_open: Some(refusal),
                    ..PolicyBatch::default()
                });
            }
            Ok(SourcePoll::Frames(frames)) => {
                self.stats.polls += 1;
                frames
            }
            // Counted for the reason the capital downlink counts it: the
            // centre was asked, whether or not it answered.
            Err(error) => {
                self.stats.polls += 1;
                return Err(error);
            }
        };

        let mut batch = PolicyBatch::default();
        for frame in &frames {
            self.stats.frames += 1;
            self.absorb(frame, now, &mut batch);
        }
        Ok(batch)
    }

    /// Take one frame through every check, or refuse it.
    ///
    /// The one absorb for every source, and it dispatches on the frame's
    /// topic alone: a halt is a halt whichever wire carried it, and a routing
    /// decision that consulted the wire is how a halt sent over the fabric
    /// would come to be read as a malformed payload and never stop anything.
    fn absorb(&mut self, frame: &AnyEvent, now: Timestamp, batch: &mut PolicyBatch) {
        if frame.topic == HaltTopic::TOPIC {
            self.absorb_halt(frame, now, batch);
            return;
        }
        if frame.topic != PolicyPayloadTopic::TOPIC {
            self.stats.ignored += 1;
            return;
        }
        let event_id = frame.event_id.as_str().to_string();

        if frame.schema_version > PolicyPayloadTopic::SCHEMA_VERSION {
            self.refuse(
                batch,
                event_id,
                &format!(
                    "the payload was written by schema version {} and this cell understands {}",
                    frame.schema_version,
                    PolicyPayloadTopic::SCHEMA_VERSION
                ),
            );
            return;
        }

        if qip_core::hash::sha256_hex(canonical_json(&frame.payload).as_bytes())
            != frame.payload_hash
        {
            self.refuse(
                batch,
                event_id,
                "the payload no longer matches the hash the sender computed, so it was corrupted \
                 or edited between the centre and this cell",
            );
            return;
        }

        let payload: PolicyPayload = match serde_json::from_value(frame.payload.clone()) {
            Ok(payload) => payload,
            Err(error) => {
                self.refuse(
                    batch,
                    event_id,
                    &format!("the frame does not carry a policy payload: {error}"),
                );
                return;
            }
        };

        // Arriving over the mesh, or over any other source, has bought this
        // payload nothing; it is verified here exactly as one handed over by
        // any other route would be.
        match VerifiedPolicy::verify(payload, &self.key, &self.cell, now) {
            Ok(verified) => {
                self.stats.verified += 1;
                batch.verified.push(verified);
            }
            Err(error) => self.refuse(batch, event_id, error.message()),
        }
    }

    /// A halt frame, through the same checks a payload gets.
    fn absorb_halt(&mut self, frame: &AnyEvent, now: Timestamp, batch: &mut PolicyBatch) {
        let event_id = frame.event_id.as_str().to_string();
        if frame.schema_version > HaltTopic::SCHEMA_VERSION {
            self.refuse(
                batch,
                event_id,
                &format!(
                    "the halt was written by schema version {} and this cell understands {}",
                    frame.schema_version,
                    HaltTopic::SCHEMA_VERSION
                ),
            );
            return;
        }
        if qip_core::hash::sha256_hex(canonical_json(&frame.payload).as_bytes())
            != frame.payload_hash
        {
            self.refuse(
                batch,
                event_id,
                "the halt no longer matches the hash the sender computed",
            );
            return;
        }
        let command: HaltCommand = match serde_json::from_value(frame.payload.clone()) {
            Ok(command) => command,
            Err(error) => {
                self.refuse(
                    batch,
                    event_id,
                    &format!("the frame does not carry a halt command: {error}"),
                );
                return;
            }
        };
        match VerifiedHalt::verify(command, &self.key, &self.cell, now) {
            Ok(verified) => {
                self.stats.verified += 1;
                batch.halts.push(verified);
            }
            Err(error) => self.refuse(batch, event_id, error.message()),
        }
    }

    fn refuse(&mut self, batch: &mut PolicyBatch, event_id: String, reason: &str) {
        self.stats.refused += 1;
        batch.refused.push(RefusedPolicy {
            event_id,
            reason: reason.to_string(),
        });
    }
}

/// The topic a capital grant travels on, named once.
///
/// A zero-sized carrier for the constant rather than a bare `const`, so the
/// downlink's topic check and any future encoder read the same declaration.
/// `qip-edge` cannot name the central plane's frame type — `qip-mesh` is a
/// service and this is a library, and that edge only runs one way — so the two
/// ends agree through `qip-contracts`' vocabulary and this constant, which is
/// the narrowest thing they can share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapitalGrantTopic;

impl CapitalGrantTopic {
    /// There is no `CapitalGranted` topic in `qip-events` and adding one means
    /// editing a crate every crate shares. `RiskApproved` is the topic whose
    /// meaning an envelope carries — an approved bound on what may be risked —
    /// and its group is retained permanently, which is what a capital
    /// instruction needs.
    pub const TOPIC: Topic = Topic::RiskApproved;
    /// Bumped when the wire shape of a grant changes. `AnyEvent::decode`
    /// refuses a payload written by a newer schema than the reader knows, and
    /// the downlink's own decode is held to the same rule by this constant.
    pub const SCHEMA_VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of no particular meaning, recorded at `secs`.
    fn frame_at(name: &str, secs: i64) -> AnyEvent {
        #[derive(Clone, Debug, Serialize, Deserialize)]
        struct Marker {
            name: String,
        }
        impl EventBody for Marker {
            const TOPIC: Topic = Topic::ServiceStarted;
            const SCHEMA_VERSION: u32 = 1;
        }
        let at = Timestamp::from_secs(1_000 + secs);
        Envelope::new(
            Id::from_string(format!("EVT-{name}")),
            at,
            at,
            Lineage::root(CorrelationId::from_string(format!("COR-{name}")), "tests"),
            Marker {
                name: name.to_string(),
            },
        )
        .erase()
        .expect("a marker frame erases")
    }

    fn fetched_ids(source: &mut ScriptedFrames, secs: i64) -> Vec<String> {
        match source
            .fetch(Timestamp::from_secs(1_000 + secs))
            .expect("a scripted source always answers")
        {
            SourcePoll::Frames(frames) => frames
                .iter()
                .map(|frame| frame.event_id.as_str().to_string())
                .collect(),
            SourcePoll::CircuitOpen(refusal) => panic!("a list has no circuit: {refusal:?}"),
        }
    }

    #[test]
    fn a_scripted_source_holds_a_frame_until_the_instant_it_became_knowable() {
        // The mesh inbox's read rule, held by the scripted source too. A
        // downlink verifies at `now`; a frame handed over before it was
        // knowable would be judged at an instant it did not yet exist, which
        // is point-in-time leakage on the control path. The frame pushed
        // behind a held one waits with it rather than being jumped over, so a
        // replayed script is read in the order it was recorded.
        let mut source = ScriptedFrames::new(8).expect("a bounded source");
        for (name, secs) in [("EARLY", 10), ("LATE", 50), ("BEHIND", 20)] {
            source.push(frame_at(name, secs)).expect("room for three");
        }
        assert_eq!(source.pending(), 3);

        assert_eq!(fetched_ids(&mut source, 30), vec!["EVT-EARLY"]);
        assert_eq!(
            source.pending(),
            2,
            "a frame recorded after the instant asked about was handed over"
        );
        assert_eq!(fetched_ids(&mut source, 60), vec!["EVT-LATE", "EVT-BEHIND"]);
        assert_eq!(
            source.pending(),
            0,
            "a frame was handed over twice or never"
        );
    }

    #[test]
    fn a_scripted_source_refuses_rather_than_drops_a_frame_it_cannot_hold() {
        // Evicting the oldest to make room could evict the halt. The source
        // refuses the push instead, and what it already holds is untouched.
        assert!(
            ScriptedFrames::new(0).is_err(),
            "a source that can hold nothing was built"
        );
        let mut source = ScriptedFrames::new(1).expect("a bounded source");
        source.push(frame_at("FIRST", 10)).expect("room for one");
        assert!(
            source.push(frame_at("SECOND", 11)).is_err(),
            "a full source took another frame"
        );
        assert_eq!(source.pending(), 1);
        assert_eq!(fetched_ids(&mut source, 30), vec!["EVT-FIRST"]);
    }

    #[test]
    fn a_downlink_over_any_source_refuses_to_exist_without_a_key() {
        // `connect` checks the key before it touches the mesh, and the
        // existing mesh suite holds that half. This holds the other: a
        // downlink built straight over a source — the fabric's route — is
        // refused an empty key by the same code, so the route that is not
        // the mesh cannot be the one that verifies nothing.
        let capital = CapitalDownlink::from_source(
            "london-1",
            b"",
            DEFAULT_GRANT_MEMORY,
            Box::new(ScriptedFrames::new(1).expect("a bounded source")),
        )
        .expect_err("a capital downlink with no key was built over a scripted source");
        assert!(
            capital.message().contains("cannot verify a grant"),
            "{}",
            capital.message()
        );
        let forgetful = CapitalDownlink::from_source(
            "london-1",
            b"a-key",
            0,
            Box::new(ScriptedFrames::new(1).expect("a bounded source")),
        )
        .expect_err("a capital downlink that remembers no grant was built");
        assert!(
            forgetful.message().contains("remembers no applied grant"),
            "{}",
            forgetful.message()
        );
        let policy = PolicyDownlink::from_source(
            "london-1",
            b"",
            Box::new(ScriptedFrames::new(1).expect("a bounded source")),
        )
        .expect_err("a policy downlink with no key was built over a scripted source");
        assert!(
            policy.message().contains("cannot verify a payload"),
            "{}",
            policy.message()
        );
    }

    #[test]
    fn a_grant_key_changes_when_any_bound_changes() {
        // The key is what makes receipt idempotent. If it did not follow the
        // signature, a widened envelope would be mistaken for one already
        // applied and silently ignored — the failure mode where a cell is
        // *denied* capital it was granted, which looks like a quiet cell.
        let envelope = |signature: &str| {
            CapitalEnvelope::new(
                StrategyId::new("s1"),
                "london-1",
                Decimal::from_int(1_000),
                Decimal::from_int(100),
                Decimal::from_int(50),
                vec![VenueId::new("XLON")],
                Timestamp::from_secs(1_000),
                Timestamp::from_secs(2_000),
                "alice",
                signature,
            )
        };
        let first = envelope("aaaa").expect("a well-formed envelope");
        let second = envelope("bbbb").expect("a well-formed envelope");
        assert_ne!(grant_key(&first), grant_key(&second));
        assert_eq!(grant_key(&first), grant_key(&first.clone()));
    }

    #[test]
    fn a_delta_that_refuses_more_than_it_may_carry_says_how_many_it_dropped() {
        let mut delta = CellStateDelta {
            cell: "london-1".to_string(),
            region: "eu-west".to_string(),
            sequence: 1,
            at: Timestamp::from_secs(1_000),
            halted: false,
            utilisation: Vec::new(),
            orders: Vec::new(),
            // Over the fill bound by a third amount. A fill dropped without a
            // count is a trade the centre never bills and never hears it
            // missed.
            fills: (0..MAX_FILLS_PER_DELTA + 5)
                .map(|index| FillRecord {
                    order_id: format!("ord-{index}"),
                    object_id: ObjectId::from_string("ACME"),
                    venue: VenueId::new("XLON"),
                    side: qip_contracts::message::BookSide::Ask,
                    quantity: Decimal::from_int(1),
                    price: Decimal::from_int(100),
                    simulated: true,
                    at: Timestamp::from_secs(1_000),
                    shares: vec![qip_contracts::wire::FillShare {
                        strategy: StrategyId::new("alpha"),
                        quantity: Decimal::from_int(1),
                    }],
                })
                .collect(),
            fills_omitted: 0,
            refusals: (0..MAX_REFUSALS_PER_DELTA + 7)
                .map(|index| DeltaRefusal {
                    gate: format!("gate-{index}"),
                    reason: "no".to_string(),
                    venue: None,
                })
                .collect(),
            refusals_omitted: 0,
            reconciliation_breaks: Vec::new(),
            reconciliation_breaks_omitted: 0,
            // Over the cross bound as well, and by a different amount, so the
            // two counters cannot be confused for one another. Crosses are
            // bounded on their own budget: sharing the refusals' would let a
            // noisy gate evict a ledger entry.
            crosses: (0..MAX_CROSSES_PER_DELTA + 3)
                .map(|_| CrossRecord {
                    object_id: ObjectId::from_string("ACME"),
                    venue: VenueId::new("XLON"),
                    quantity: Decimal::from_int(1),
                    price: Decimal::from_int(100),
                    bought: vec![StrategyId::new("alpha")],
                    sold: vec![StrategyId::new("beta")],
                })
                .collect(),
            crosses_omitted: 0,
        };
        delta.bound_refusals();
        assert_eq!(delta.refusals.len(), MAX_REFUSALS_PER_DELTA);
        assert_eq!(
            delta.refusals_omitted, 7,
            "a truncated list that does not say it was truncated is a lie about a quiet cell"
        );
        assert_eq!(delta.crosses.len(), MAX_CROSSES_PER_DELTA);
        assert_eq!(
            delta.crosses_omitted, 3,
            "a dropped ledger entry the centre is never told about is the one \
             omission an examiner asks about"
        );
        assert_eq!(delta.fills.len(), MAX_FILLS_PER_DELTA);
        assert_eq!(
            delta.fills_omitted, 5,
            "a fill dropped from the wire without a count is a trade the centre \
             never bills and is never told it missed"
        );
    }
}
