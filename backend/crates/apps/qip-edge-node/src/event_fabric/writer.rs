//! ADR 0100 §1: the node's composition of the reflex ring → spool → drain
//! chain — the segment writer, sharing `qip-storage::segment`'s format with
//! the drain and the broker rather than a second on-disk shape. SLICE-24.
//!
//! # The handoff, and why it has no blocking send
//!
//! ADR 0100 §6: "the decision thread hands a batch to a bounded channel with
//! `try_send`, and that is all it does." [`HandoffSender`] wraps the bounded
//! channel and exposes exactly one way in, [`HandoffSender::offer`], which
//! never waits. A full channel is returned to the caller as
//! [`Offer::Full`] carrying the value back, so the caller — the
//! [`FabricMirror`](super::mirror::FabricMirror), which leaves the entries
//! unshipped in the journal, or [`RecordedInputs`](super::inputs::RecordedInputs),
//! which keeps a bounded backlog — decides what a refusal means. A wrapper
//! with a `send` beside `offer` would be one autocomplete away from a
//! decision thread parked behind a disk.
//!
//! # What the writer records
//!
//! Every journal entry becomes a P2 `ReflexJournalRecorded` record. An entry
//! whose decision is an outcome ([`is_outcome`]) is also written to P1 as an
//! `OutcomeRecord` carrying its journal sequence and digest; a run of
//! non-outcome entries is covered on P1 by one `ChainSpan` whose tail digest
//! the next P1 record chains onto (red-team M12: P1 stays chain-verifiable
//! without carrying every entry). Pass markers and the tape events each pass
//! applied go to P2 unchanged; a recorded-inputs overflow arrives as a gap
//! and is written twice — once where the missing inputs would have been, on
//! P2, and once on P1 where it cannot be shed — so replay refuses the window
//! rather than reproducing something else (M8).
//!
//! # One spool, two streams
//!
//! A spool batch is sent by the drain to one stream as-is (ADR 0100 §1), so
//! every batch is homogeneous in its destination, and the writer says which
//! through the one header field the codec gives it for "the schema the
//! records were built against": [`P1_BATCH_SCHEMA_ID`] or
//! [`P2_BATCH_SCHEMA_ID`], read back by [`stream_of`]. Each record's own
//! `AnyEvent` still names its topic and schema version, which is what the
//! schema lock is keyed on. The writer sets no drain or broker field — the
//! sequence is the drain's and the offset the broker's.
//!
//! # Group fsync, and what an error means
//!
//! Everything received in one loop is written as at most one P1 batch and
//! one P2 batch (more only when a group would exceed the codec's ceiling),
//! P1 first because it is the lane that may not be shed. Each append is
//! fsynced before it returns ([`SegmentLog::append`]). An append that fails
//! leaves the segment in a state only reopening recovers, so the writer does
//! not retry on the same spool: it keeps what it could not write, stops
//! taking handoffs — the channel fills, the mirror refuses, the entries stay
//! in the journal — and marks the gauge unwritable, which halts new exposure.
//!
//! # The heartbeat
//!
//! The writer is the pressure gauge's heartbeat publisher (SLICE-54). It
//! waits on its channel with a bounded `recv_timeout` and beats on every
//! loop whether or not it wrote, after publishing the size and the
//! unwritable bit, so a fresh beat never vouches for stale bits. It touches
//! only local disk, so a stalled broker cannot stale the reading; only a
//! writer that has stopped can.

use crate::event_fabric::pressure::SpoolPublisher;
use crate::event_fabric::telemetry::OutboxTelemetry;
use qip_contracts::market_event::MarketEvent;
use qip_contracts::reflex::{ChainSpan, Decision, Gap, JournalEntry, OutcomeRecord};
use qip_contracts::replay::PassMarker;
use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use qip_core::{CorrelationId, EventId, Lineage, Timestamp, sha256_hex};
use qip_edge::journal::{Journal, MirrorBatch};
use qip_events::AnyEvent;
use qip_events::event_fabric::bindings::{
    EVENT_FABRIC_GAP, MARKET_EVENT_APPLIED, REFLEX_CHAIN_SPAN, REFLEX_JOURNAL_RECORDED,
    REFLEX_OUTCOME_RECORDED, REFLEX_PASS_MARKED, TopicBinding,
};
use qip_events::event_fabric::codec::{Batch, MAX_BATCH_LEN, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::QosClass;
use qip_storage::segment::log::SegmentLog;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};

/// The batch schema id of a spool batch bound for the P1 outcomes stream.
pub const P1_BATCH_SCHEMA_ID: u32 = 1;

/// The batch schema id of a spool batch bound for the P2 market-journal
/// stream.
pub const P2_BATCH_SCHEMA_ID: u32 = 2;

/// The only batch schema version this writer produces.
pub const BATCH_SCHEMA_VERSION: u32 = 1;

/// The manifest key the session counter is committed under.
const SESSION_KEY: &str = "outbox.session";

/// The producer every record's lineage names.
const PRODUCER: &str = "qip-edge-node.outbox";

/// A spool batch's records are cut before their payloads reach half the
/// codec's ceiling, leaving the other half for per-record framing and the
/// header, so an encode never fails on size for a group the writer built.
const BATCH_PAYLOAD_CAP: usize = MAX_BATCH_LEN / 2;

// --- the handoff ----------------------------------------------------------

/// What the decision thread hands the writer.
#[derive(Clone, Debug)]
pub enum Handoff {
    /// A journal batch, from [`FabricMirror`](super::mirror::FabricMirror).
    Journal(MirrorBatch),
    /// What one pass applied, from [`RecordedInputs`](super::inputs::RecordedInputs).
    /// Boxed because a marker is several times the size of the other
    /// variants, and a bounded channel preallocates every slot at the size
    /// of its largest value.
    Pass(Box<PassInputs>),
    /// A window of passes whose inputs were not recorded.
    InputGap(InputGap),
}

/// The exogenous inputs one pass applied (ADR 0100 §8): its marker, with the
/// readings it took, and the tape events it applied, in the order applied.
#[derive(Clone, Debug, PartialEq)]
pub struct PassInputs {
    pub marker: PassMarker,
    pub applied: Vec<MarketEvent>,
}

/// Passes `from_pass..=to_pass` whose inputs the backlog could not hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputGap {
    pub from_pass: u64,
    pub to_pass: u64,
    /// How many passes were dropped inside the window.
    pub passes: u64,
    /// How many applied tape events those passes carried.
    pub events: u64,
    /// The latest dropped pass's own clock reading, in nanoseconds.
    pub last_now_ns: i64,
    /// The backlog bound that was reached, in bytes.
    pub bound_bytes: u64,
}

/// What [`HandoffSender::offer`] did with a value.
#[derive(Debug)]
pub enum Offer {
    /// The writer's channel took it.
    Accepted,
    /// The channel is full; the value is handed back untouched.
    Full(Handoff),
    /// The writer has gone; the value is handed back untouched.
    Closed(Handoff),
}

/// The decision thread's end of the handoff channel. See the module
/// documentation for why it has no blocking send.
#[derive(Clone, Debug)]
pub struct HandoffSender {
    inner: SyncSender<Handoff>,
}

impl HandoffSender {
    /// Offer `handoff` to the writer without waiting.
    pub fn offer(&self, handoff: Handoff) -> Offer {
        match self.inner.try_send(handoff) {
            Ok(()) => Offer::Accepted,
            Err(TrySendError::Full(back)) => Offer::Full(back),
            Err(TrySendError::Disconnected(back)) => Offer::Closed(back),
        }
    }
}

/// The writer's end of the handoff channel.
#[derive(Debug)]
pub struct HandoffReceiver {
    inner: Receiver<Handoff>,
}

/// A bounded handoff channel holding at most `capacity` values.
///
/// Refuses a capacity of zero: a zero-capacity channel is a rendezvous, on
/// which every `try_send` fails unless the writer happens to be parked in a
/// receive at that instant — a channel that refuses nearly everything and
/// reads as an outage nobody caused.
pub fn channel(capacity: usize) -> Result<(HandoffSender, HandoffReceiver)> {
    if capacity == 0 {
        return Err(Error::invalid(
            "a handoff channel of capacity zero is a rendezvous that refuses almost every \
             offer; configure a positive capacity",
        ));
    }
    let (inner, receiver) = std::sync::mpsc::sync_channel(capacity);
    Ok((HandoffSender { inner }, HandoffReceiver { inner: receiver }))
}

// --- classification -------------------------------------------------------

/// Whether an entry carrying `decision` is an outcome ADR 0100 §5's P1 row
/// names — an order sent, filled or expired, a mass cancel, an internal
/// cross, a reconciliation break — and so is written to P1 as well as P2.
///
/// Exhaustive with no wildcard on purpose: a new `Decision` variant does not
/// compile here until somebody decides which lane it belongs on, rather than
/// defaulting to the lane that may be shed.
pub fn is_outcome(decision: &Decision) -> bool {
    match decision {
        Decision::OrderSent { .. }
        | Decision::Filled { .. }
        | Decision::OrderExpired { .. }
        | Decision::MassCancelled { .. }
        | Decision::CrossedInternally { .. }
        | Decision::ReconciliationBreak { .. } => true,
        Decision::Ingested { .. }
        | Decision::GapDetected { .. }
        | Decision::SignalRaised { .. }
        | Decision::EdgePriced { .. }
        | Decision::Refused { .. }
        | Decision::HaltChanged { .. }
        | Decision::PolicyApplied { .. }
        | Decision::CapitalRenewed { .. }
        | Decision::CyclePathAssigned { .. }
        | Decision::PathExtensionChecked { .. }
        | Decision::CycleCommitted { .. }
        | Decision::CycleDecomposed { .. }
        | Decision::CycleRested { .. }
        | Decision::CycleAbandoned { .. }
        | Decision::StrategyWithdrawn { .. }
        | Decision::RegionShareApplied { .. }
        | Decision::RegionOutlookChanged { .. }
        | Decision::ReconciliationRequired { .. }
        | Decision::VenueReconciled { .. }
        | Decision::VenueChosen { .. }
        | Decision::DispositionIntent { .. } => false,
    }
}

/// Which record a spool record is, and so which stream it is bound for and
/// which kind its event id is keyed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordKind {
    JournalRecorded,
    OutcomeRecorded,
    ChainSpan,
    PassMarked,
    MarketEventApplied,
    /// A gap, on P2 where the missing records would have been.
    GapP2,
    /// The same gap, on P1 where it cannot be shed.
    GapP1,
}

impl RecordKind {
    /// The stream-kind component of an event id. Distinct for every kind, so
    /// the P2 and P1 copies of one fact never share an id.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::JournalRecorded => "p2.journal_recorded",
            Self::OutcomeRecorded => "p1.outcome_recorded",
            Self::ChainSpan => "p1.chain_span",
            Self::PassMarked => "p2.pass_marked",
            Self::MarketEventApplied => "p2.market_event_applied",
            Self::GapP2 => "p2.gap",
            Self::GapP1 => "p1.gap",
        }
    }

    pub const fn binding(self) -> TopicBinding {
        match self {
            Self::JournalRecorded => REFLEX_JOURNAL_RECORDED,
            Self::OutcomeRecorded => REFLEX_OUTCOME_RECORDED,
            Self::ChainSpan => REFLEX_CHAIN_SPAN,
            Self::PassMarked => REFLEX_PASS_MARKED,
            Self::MarketEventApplied => MARKET_EVENT_APPLIED,
            Self::GapP2 | Self::GapP1 => EVENT_FABRIC_GAP,
        }
    }

    /// The stream this record is written to. Not the binding's class for a
    /// gap's P2 copy, which sits on the stream whose records went missing.
    pub const fn stream(self) -> QosClass {
        match self {
            Self::JournalRecorded | Self::PassMarked | Self::MarketEventApplied | Self::GapP2 => {
                QosClass::P2MarketJournal
            }
            Self::OutcomeRecorded | Self::ChainSpan | Self::GapP1 => QosClass::P1Outcomes,
        }
    }
}

/// The event id of the record of `kind` at `position` in `session` of
/// `cell`: `sha256(cell | session | position | kind)`.
///
/// For a journal record `position` is the journal sequence. The session is
/// the persisted counter, never a start time (red-team M1/F6): two starts in
/// one second are two sessions, and a crash loop cannot reissue an id.
pub fn event_id(cell: &str, session: u64, position: &str, kind: RecordKind) -> String {
    sha256_hex(format!("{cell}|{session}|{position}|{}", kind.as_str()).as_bytes())
}

/// Which stream a spool batch is bound for, from the schema id the writer
/// stamped. Refuses an id this writer never writes rather than guessing a
/// stream for it.
pub fn stream_of(batch: &Batch) -> Result<QosClass> {
    match batch.schema_id {
        P1_BATCH_SCHEMA_ID => Ok(QosClass::P1Outcomes),
        P2_BATCH_SCHEMA_ID => Ok(QosClass::P2MarketJournal),
        other => Err(Error::schema(format!(
            "spool batch schema id {other} names no outbox stream; this writer stamps only \
             {P1_BATCH_SCHEMA_ID} (P1) and {P2_BATCH_SCHEMA_ID} (P2)"
        ))),
    }
}

/// One record read back off a spool.
#[derive(Clone, Debug, PartialEq)]
pub struct SpooledRecord {
    pub offset: u64,
    pub stream: QosClass,
    pub event: AnyEvent,
}

/// Every record a spool holds, in spool order. The reader's side of the
/// writer: what the drain sends and what replay reads.
pub fn records(log: &SegmentLog) -> Result<Vec<SpooledRecord>> {
    let mut out = Vec::new();
    for segment in log.segments() {
        for offset in segment.start_offset..segment.end_offset {
            let batch = log.read(offset)?.ok_or_else(|| {
                Error::io(format!(
                    "the spool lists offset {offset} but serves nothing there"
                ))
            })?;
            let stream = stream_of(&batch)?;
            for record in &batch.records {
                out.push(SpooledRecord {
                    offset,
                    stream,
                    event: record.decode_payload(batch.encoding)?,
                });
            }
        }
    }
    Ok(out)
}

// --- the spool --------------------------------------------------------------

/// Bytes a spool holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpoolSizes {
    /// Every segment on disk, sealed and active.
    pub used: u64,
    /// Those the broker has not yet reported archived — the ones producer-
    /// retained durability still depends on (ADR 0100 §3).
    pub unarchived: u64,
}

/// What the writer needs of a spool. [`SegmentLog`] is the one production
/// implementation; the seam exists so a test can inject the write failure a
/// real disk produces rarely and at the worst moment.
pub trait Spool: Send + std::fmt::Debug {
    /// Append and fsync one batch; `Ok` only once the bytes are durable.
    fn append(&mut self, batch: &Batch) -> Result<u64>;
    fn sizes(&self) -> Result<SpoolSizes>;
    fn manifest_get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    /// Write a small record durably before returning.
    fn manifest_put(&mut self, key: &str, bytes: &[u8]) -> Result<()>;
}

impl Spool for SegmentLog {
    fn append(&mut self, batch: &Batch) -> Result<u64> {
        SegmentLog::append(self, batch)
    }

    fn sizes(&self) -> Result<SpoolSizes> {
        let mut used: u64 = 0;
        let mut unarchived: u64 = 0;
        let overflow = || Error::numeric("the spool's byte count overflows a u64");
        for segment in self.segments() {
            used = used.checked_add(segment.byte_length).ok_or_else(overflow)?;
            if !segment.sealed || !self.is_archived(segment.start_offset)? {
                unarchived = unarchived
                    .checked_add(segment.byte_length)
                    .ok_or_else(overflow)?;
            }
        }
        Ok(SpoolSizes { used, unarchived })
    }

    fn manifest_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        SegmentLog::manifest_get(self, key)
    }

    fn manifest_put(&mut self, key: &str, bytes: &[u8]) -> Result<()> {
        SegmentLog::manifest_put(self, key, bytes)
    }
}

/// The session counter as committed to the spool's manifest.
#[derive(Debug, Serialize, Deserialize)]
struct SessionRecord {
    session: u64,
    /// When the session started, kept beside the counter for an operator
    /// reading the manifest. Never the session's identity: see [`event_id`].
    started_ns: i64,
}

// --- the writer -------------------------------------------------------------

/// How a [`SpoolWriter`] runs.
#[derive(Clone, Debug)]
pub struct WriterConfig {
    cell: String,
    started: Timestamp,
    poll: std::time::Duration,
    max_handoffs_per_step: usize,
}

impl WriterConfig {
    /// A writer for `cell`, started at `started`, waiting at most `poll` for
    /// a handoff per loop and taking at most `max_handoffs_per_step` in one.
    ///
    /// Refuses an empty cell, which would key every event id on nothing; a
    /// zero poll, on which the writer spins a core; and a zero group size,
    /// on which it takes nothing.
    pub fn new(
        cell: impl Into<String>,
        started: Timestamp,
        poll: std::time::Duration,
        max_handoffs_per_step: usize,
    ) -> Result<Self> {
        let cell = cell.into();
        if cell.trim().is_empty() {
            return Err(Error::invalid(
                "a spool writer needs the cell it writes for; every event id is keyed on it",
            ));
        }
        if poll.is_zero() {
            return Err(Error::invalid(
                "a zero poll interval spins the writer on an empty channel; configure a bounded \
                 positive wait",
            ));
        }
        if max_handoffs_per_step == 0 {
            return Err(Error::invalid(
                "a writer that takes zero handoffs per loop never writes; configure at least one",
            ));
        }
        Ok(Self {
            cell,
            started,
            poll,
            max_handoffs_per_step,
        })
    }
}

/// What one [`SpoolWriter::step`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing arrived within the poll interval.
    Idle,
    /// This many records were made durable.
    Wrote(usize),
    /// The spool refused a write, now or earlier; nothing was appended.
    Failed,
}

/// The spool writer: the thread between the handoff channel and the spool.
#[derive(Debug)]
pub struct SpoolWriter<S: Spool> {
    spool: S,
    receiver: HandoffReceiver,
    publisher: SpoolPublisher,
    telemetry: Arc<OutboxTelemetry>,
    config: WriterConfig,
    session: u64,
    correlation: CorrelationId,
    /// The digest the next journal batch must chain onto.
    journal_tail: String,
    /// Batches built and not yet durable, in the order they must land.
    pending: VecDeque<Batch>,
    /// Why the spool is no longer written, once it has refused a write.
    failed: Option<Error>,
    disconnected: bool,
}

impl<S: Spool> SpoolWriter<S> {
    /// Open a writer over `spool`, committing this session's counter to the
    /// spool before any record is written.
    ///
    /// The counter is the last one committed plus one, checked; a record
    /// that exists and cannot be read is refused rather than restarted from
    /// one, which would reissue every event id of an earlier session.
    pub fn open(
        mut spool: S,
        receiver: HandoffReceiver,
        publisher: SpoolPublisher,
        telemetry: Arc<OutboxTelemetry>,
        config: WriterConfig,
    ) -> Result<Self> {
        let previous = match spool.manifest_get(SESSION_KEY)? {
            None => 0,
            Some(bytes) => {
                let record: SessionRecord = serde_json::from_slice(&bytes).map_err(|error| {
                    Error::schema(format!(
                        "the spool's session record does not parse ({error}); restarting the \
                         counter would reissue earlier sessions' event ids, so the spool is \
                         refused — inspect it rather than clear it"
                    ))
                })?;
                record.session
            }
        };
        let session = previous.checked_add(1).ok_or_else(|| {
            Error::numeric(
                "the spool's session counter is exhausted; a counter that wrapped would reissue \
                 the first session's event ids",
            )
        })?;
        let record = SessionRecord {
            session,
            started_ns: config.started.as_nanos(),
        };
        spool.manifest_put(SESSION_KEY, &serde_json::to_vec(&record)?)?;
        let correlation =
            CorrelationId::from_string(sha256_hex(format!("{}|{session}", config.cell).as_bytes()));
        Ok(Self {
            spool,
            receiver,
            publisher,
            telemetry,
            config,
            session,
            correlation,
            journal_tail: Journal::GENESIS.to_string(),
            pending: VecDeque::new(),
            failed: None,
            disconnected: false,
        })
    }

    /// This session's counter.
    pub fn session(&self) -> u64 {
        self.session
    }

    pub fn spool(&self) -> &S {
        &self.spool
    }

    /// Why the spool is no longer written, if it has refused a write.
    pub fn failure(&self) -> Option<&Error> {
        self.failed.as_ref()
    }

    /// Whether the decision thread's end of the channel has gone.
    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }

    /// One loop: wait at most the poll interval for handoffs, write what
    /// arrived, publish the spool's state, beat.
    ///
    /// Beats whether or not anything was written. An `Err` is a heartbeat
    /// that could not advance; the gauge then reads stale and halts, which is
    /// the answer for a writer whose liveness nobody can see.
    pub fn step(&mut self) -> Result<Step> {
        let step = if self.failed.is_some() {
            // Nothing is appended to a spool that refused a write: the bounded
            // wait here stands in for the channel wait, so a failed writer
            // still beats at the same cadence without spinning.
            std::thread::sleep(self.config.poll);
            Step::Failed
        } else {
            let handoffs = self.collect();
            if let Err(error) = self.build(handoffs) {
                self.failed = Some(error);
            }
            self.write_pending()
        };
        self.publish()?;
        Ok(step)
    }

    /// Loop until the decision thread's end has gone and everything received
    /// is durable, returning the writer; or, if the spool has refused a
    /// write by then, the reason.
    pub fn run(mut self) -> Result<Self> {
        loop {
            self.step()?;
            if self.disconnected {
                if let Some(error) = &self.failed {
                    return Err(error.clone());
                }
                if self.pending.is_empty() {
                    return Ok(self);
                }
            }
        }
    }

    fn collect(&mut self) -> Vec<Handoff> {
        let mut handoffs = Vec::new();
        match self.receiver.inner.recv_timeout(self.config.poll) {
            Ok(handoff) => handoffs.push(handoff),
            Err(RecvTimeoutError::Timeout) => return handoffs,
            Err(RecvTimeoutError::Disconnected) => {
                self.disconnected = true;
                return handoffs;
            }
        }
        while handoffs.len() < self.config.max_handoffs_per_step {
            match self.receiver.inner.try_recv() {
                Ok(handoff) => handoffs.push(handoff),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.disconnected = true;
                    break;
                }
            }
        }
        handoffs
    }

    /// Turn handoffs into P1 and P2 batches, appended to `pending`.
    fn build(&mut self, handoffs: Vec<Handoff>) -> Result<()> {
        if handoffs.is_empty() {
            return Ok(());
        }
        let mut p1 = Vec::new();
        let mut p2 = Vec::new();
        for handoff in handoffs {
            match handoff {
                Handoff::Journal(batch) => self.journal(batch, &mut p1, &mut p2)?,
                Handoff::Pass(inputs) => self.pass(&inputs, &mut p2)?,
                Handoff::InputGap(gap) => self.input_gap(&gap, &mut p1, &mut p2)?,
            }
        }
        let mut batches = cut(P1_BATCH_SCHEMA_ID, p1)?;
        batches.extend(cut(P2_BATCH_SCHEMA_ID, p2)?);
        self.pending.extend(batches);
        Ok(())
    }

    fn journal(
        &mut self,
        batch: MirrorBatch,
        p1: &mut Vec<Record>,
        p2: &mut Vec<Record>,
    ) -> Result<()> {
        if batch.cell != self.config.cell {
            return Err(Error::invalid(format!(
                "a journal batch from cell {} reached the spool writer for cell {}; two cells' \
                 decisions in one spool cannot be told apart",
                batch.cell, self.config.cell
            )));
        }
        if let Err(error) = batch.verify_against(&self.journal_tail) {
            // The entries cannot be placed on the chain this session has
            // written, so they are not written as if they could: the window
            // is declared on both lanes and the chain resumes after it.
            if let (Some(first), Some(last)) = (batch.entries.first(), batch.entries.last()) {
                let gap = Gap {
                    stream: REFLEX_JOURNAL_RECORDED.topic.name().to_string(),
                    from_seq: first.sequence,
                    to_seq: last.sequence,
                    reason: format!(
                        "a journal batch did not chain onto what this session wrote: {}",
                        error.message()
                    ),
                };
                self.gap(&gap, last.at, p1, p2)?;
            }
            self.journal_tail = batch.tail_digest();
            return Ok(());
        }
        let mut run: Option<(u64, u64, Timestamp, String)> = None;
        for entry in &batch.entries {
            let position = entry.sequence.to_string();
            p2.push(self.record(RecordKind::JournalRecorded, &position, entry.at, entry)?);
            if is_outcome(&entry.decision) {
                if let Some(span) = run.take() {
                    p1.push(self.span(span)?);
                }
                let outcome = OutcomeRecord {
                    cell: self.config.cell.clone(),
                    session: self.session,
                    journal_sequence: entry.sequence,
                    journal_digest: entry.digest.clone(),
                    entry: entry.clone(),
                };
                p1.push(self.record(RecordKind::OutcomeRecorded, &position, entry.at, &outcome)?);
            } else {
                run = Some(match run {
                    Some((first, _, _, _)) => {
                        (first, entry.sequence, entry.at, entry.digest.clone())
                    }
                    None => (
                        entry.sequence,
                        entry.sequence,
                        entry.at,
                        entry.digest.clone(),
                    ),
                });
            }
        }
        // A run still open at the end of the batch is closed here, so P1 is
        // continuous at every batch boundary rather than only at the next
        // outcome, which may never come.
        if let Some(span) = run.take() {
            p1.push(self.span(span)?);
        }
        self.journal_tail = batch.tail_digest();
        Ok(())
    }

    fn span(&self, (first, last, at, tail): (u64, u64, Timestamp, String)) -> Result<Record> {
        let span = ChainSpan {
            cell: self.config.cell.clone(),
            session: self.session,
            first_seq: first,
            last_seq: last,
            tail_digest: tail,
        };
        self.record(RecordKind::ChainSpan, &first.to_string(), at, &span)
    }

    fn pass(&self, inputs: &PassInputs, p2: &mut Vec<Record>) -> Result<()> {
        let pass = inputs.marker.pass;
        let at = Timestamp::from_nanos(inputs.marker.now_ns);
        p2.push(self.record(
            RecordKind::PassMarked,
            &pass.to_string(),
            at,
            &inputs.marker,
        )?);
        for (index, event) in inputs.applied.iter().enumerate() {
            p2.push(self.record_at(
                RecordKind::MarketEventApplied,
                &format!("{pass}.{index}"),
                event.event_time(),
                event.receive_time(),
                event,
            )?);
        }
        Ok(())
    }

    fn input_gap(&self, gap: &InputGap, p1: &mut Vec<Record>, p2: &mut Vec<Record>) -> Result<()> {
        let body = Gap {
            stream: REFLEX_PASS_MARKED.topic.name().to_string(),
            from_seq: gap.from_pass,
            to_seq: gap.to_pass,
            reason: format!(
                "the recorded-inputs backlog reached its {}-byte bound and {} passes' inputs \
                 ({} applied tape events) were not recorded; replay must refuse this window",
                gap.bound_bytes, gap.passes, gap.events
            ),
        };
        self.gap(&body, Timestamp::from_nanos(gap.last_now_ns), p1, p2)
    }

    /// A gap, written on P2 where the records are missing and on P1 where it
    /// cannot be shed.
    fn gap(
        &self,
        gap: &Gap,
        at: Timestamp,
        p1: &mut Vec<Record>,
        p2: &mut Vec<Record>,
    ) -> Result<()> {
        let position = format!("{}:{}", gap.stream, gap.from_seq);
        p2.push(self.record(RecordKind::GapP2, &position, at, gap)?);
        p1.push(self.record(RecordKind::GapP1, &position, at, gap)?);
        Ok(())
    }

    fn record<B: Serialize>(
        &self,
        kind: RecordKind,
        position: &str,
        at: Timestamp,
        body: &B,
    ) -> Result<Record> {
        self.record_at(kind, position, at, at, body)
    }

    fn record_at<B: Serialize>(
        &self,
        kind: RecordKind,
        position: &str,
        occurred_at: Timestamp,
        recorded_at: Timestamp,
        body: &B,
    ) -> Result<Record> {
        let binding = kind.binding();
        let payload = serde_json::to_value(body).map_err(|error| {
            Error::schema(format!(
                "a {} record would not serialise: {error}",
                kind.as_str()
            ))
        })?;
        let payload_hash = sha256_hex(canonical_json(&payload).as_bytes());
        let id = event_id(&self.config.cell, self.session, position, kind);
        let event = AnyEvent {
            event_id: EventId::from_string(id.clone()),
            topic: binding.topic,
            schema_version: binding.schema_version,
            occurred_at,
            recorded_at,
            sequence: 0,
            lineage: Lineage {
                correlation_id: self.correlation.clone(),
                causation_id: None,
                trace_id: None,
                producer: PRODUCER.to_string(),
            },
            idempotency_key: Some(id),
            payload,
            payload_hash,
        };
        Record::from_any_event(&event, PayloadCodec::CanonicalJson)
    }

    /// Append every pending batch in order, stopping at the first refusal.
    fn write_pending(&mut self) -> Step {
        let mut written = 0usize;
        while let Some(batch) = self.pending.pop_front() {
            if self.failed.is_some() {
                self.pending.push_front(batch);
                return Step::Failed;
            }
            match self.spool.append(&batch) {
                Ok(_) => written = written.saturating_add(batch.records.len()),
                Err(error) => {
                    self.pending.push_front(batch);
                    self.failed = Some(error);
                    return Step::Failed;
                }
            }
        }
        if self.failed.is_some() {
            Step::Failed
        } else if written == 0 {
            Step::Idle
        } else {
            Step::Wrote(written)
        }
    }

    /// Publish the size and the unwritable bit, then beat — in that order, so
    /// a fresh heartbeat never vouches for bits the writer has not yet set.
    fn publish(&mut self) -> Result<()> {
        match self.spool.sizes() {
            Ok(sizes) => {
                self.publisher.set_used_bytes(Some(sizes.used));
                self.telemetry.spool_bytes(sizes.used);
                self.telemetry.spool_unarchived_bytes(sizes.unarchived);
            }
            Err(_) => self.publisher.set_used_bytes(None),
        }
        self.publisher.set_unwritable(self.failed.is_some());
        self.publisher.beat().map(|_| ())
    }
}

/// Cut `records` into batches of one stream whose payloads stay under
/// [`BATCH_PAYLOAD_CAP`], in order.
fn cut(schema_id: u32, records: Vec<Record>) -> Result<Vec<Batch>> {
    let mut batches = Vec::new();
    let mut group: Vec<Record> = Vec::new();
    let mut bytes = 0usize;
    for record in records {
        let size = record.payload.len();
        if !group.is_empty() && bytes.saturating_add(size) > BATCH_PAYLOAD_CAP {
            batches.push(batch(schema_id, std::mem::take(&mut group))?);
            bytes = 0;
        }
        bytes = bytes.saturating_add(size);
        group.push(record);
    }
    if !group.is_empty() {
        batches.push(batch(schema_id, group)?);
    }
    Ok(batches)
}

/// The writer's stamp: only the fields the codec assigns to the writer.
fn batch(schema_id: u32, records: Vec<Record>) -> Result<Batch> {
    Batch::new(
        MessageType::Data,
        schema_id,
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        records,
    )
}

/// The journal entry a P2 `ReflexJournalRecorded` record carries.
pub fn journal_entry(event: &AnyEvent) -> Result<JournalEntry> {
    decode_as(event, REFLEX_JOURNAL_RECORDED)
}

/// The outcome a P1 `ReflexOutcomeRecorded` record carries.
pub fn outcome(event: &AnyEvent) -> Result<OutcomeRecord> {
    decode_as(event, REFLEX_OUTCOME_RECORDED)
}

/// The span a P1 `ReflexChainSpan` record carries.
pub fn chain_span(event: &AnyEvent) -> Result<ChainSpan> {
    decode_as(event, REFLEX_CHAIN_SPAN)
}

/// The gap an `EventFabricGap` record carries.
pub fn gap(event: &AnyEvent) -> Result<Gap> {
    decode_as(event, EVENT_FABRIC_GAP)
}

/// The marker a P2 `ReflexPassMarked` record carries.
pub fn pass_marker(event: &AnyEvent) -> Result<PassMarker> {
    decode_as(event, REFLEX_PASS_MARKED)
}

/// The tape event a P2 `MarketEventApplied` record carries.
pub fn market_event(event: &AnyEvent) -> Result<MarketEvent> {
    decode_as(event, MARKET_EVENT_APPLIED)
}

fn decode_as<T: for<'de> Deserialize<'de>>(event: &AnyEvent, binding: TopicBinding) -> Result<T> {
    if event.topic != binding.topic {
        return Err(Error::schema(format!(
            "cannot read a {} record as {}",
            event.topic, binding.topic
        )));
    }
    if event.schema_version > binding.schema_version {
        return Err(Error::schema(format!(
            "{} schema version {} is newer than the {} this build reads",
            event.topic, event.schema_version, binding.schema_version
        )));
    }
    serde_json::from_value(event.payload.clone()).map_err(|error| {
        Error::schema(format!(
            "a {} record's payload does not read back: {error}",
            event.topic
        ))
    })
}
