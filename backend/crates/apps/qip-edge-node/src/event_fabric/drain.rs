//! ADR 0100 §1: the drain — the thread that reads the spool the writer
//! fsynced and delivers each batch to every configured destination, off the
//! decision thread (§6). SLICE-32.
//!
//! # Why one cursor per sink
//!
//! A batch is delivered to each [`Sink`] independently and each sink's
//! position is committed to the spool's own manifest after it takes the
//! batch. One shared cursor would force a choice on a partial failure: send
//! the batch again to the sink that already took it — a second copy of the
//! same record at the broker or a second lake segment — or skip it for the
//! sink that refused it, which is a hole. Separate cursors make neither
//! necessary: the sink that took it moves on, the one that refused it tries
//! the same batch again on the next step.
//!
//! # What retires a segment, and why that bounds the spool
//!
//! A sealed segment every sink has read past is marked archived and then
//! deleted by `SegmentLog::retain_before`. That is what keeps the spool —
//! and therefore the pressure gauge's reading — bounded while the
//! destinations keep up. A destination that stops leaves its cursor where it
//! was; the segments behind that cursor stay; the spool grows; the gauge
//! narrows and then halts new exposure. The cell is never told that a record
//! is safe while some destination has not taken it.
//!
//! The active segment is sealed on a cadence ([`DrainConfig::new`]'s
//! `seal_every`), because a segment is sealed by size alone otherwise, and a
//! quiet cell's last few batches would sit in an active segment nothing
//! would ever retire.
//!
//! # The destinations
//!
//! - [`FabricSink`]: `qip-fabricd`, through `qip-transport`'s idempotent
//!   producer — one per stream, P1 at the quorum acknowledgement its class
//!   requires and P2 at leader-only (FABRIC-067). Fenced is passed to the
//!   gauge's drain publisher, which halts new exposure: a newer producer
//!   epoch owns the partition and nothing this process spools will be
//!   accepted again.
//! - [`LakeSink`]: the tick lake (`qip_storage::lake`), for the market
//!   events each pass applied. Each event keeps the venue's instant, the
//!   node's capture instant and the feed's own sequence number, because the
//!   lake is where a backtest asks "what did we know then", and a tick
//!   without its sequence cannot show what was missing.
//!
//! A node with no sink configured still runs this thread: segments then
//! retire as soon as they seal, and the spool is the bounded local record
//! the pressure gauge measures rather than an unbounded one.

use crate::event_fabric::pressure::DrainPublisher;
use crate::event_fabric::telemetry::OutboxTelemetry;
use crate::event_fabric::writer::{market_event, stream_of};
use qip_contracts::market_event::MarketEvent;
use qip_core::error::{Error, Result};
use qip_events::event_fabric::bindings::MARKET_EVENT_APPLIED;
use qip_events::event_fabric::codec::Batch;
use qip_events::event_fabric::policy::QosClass;
use qip_storage::BlobStore;
use qip_storage::lake::{Entitlement, Lake, Partition, RecordClass};
use qip_storage::lake_codec::SegmentCodec;
use qip_storage::segment::log::SegmentLog;
use qip_storage::tick_capture::RetentionRight;
use qip_transport::event_fabric::producer::Producer;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The drain thread's name.
pub const DRAIN_THREAD: &str = "qip-spool-drain";

/// The P1 stream the reflex outcomes are produced to (streams.local.json).
pub const OUTCOMES_STREAM: &str = "reflex-outcomes.local";

/// The P2 stream the reflex journal, pass markers and tape events are
/// produced to (streams.local.json).
pub const JOURNAL_STREAM: &str = "reflex-journal.local";

/// The manifest key prefix a sink's cursor is committed under.
const CURSOR_PREFIX: &str = "drain.cursor.";

/// One destination the drain delivers spool batches to.
pub trait Sink: Send + std::fmt::Debug {
    /// The sink's name: a source literal, because it is a manifest key and a
    /// metric label, and a runtime string in either would be unbounded.
    fn name(&self) -> &'static str;

    /// Deliver the batch at spool `offset`, bound for `stream`. `Ok` only
    /// once the destination holds it durably by its own definition.
    fn deliver(&mut self, offset: u64, stream: QosClass, batch: &Batch) -> Result<()>;

    /// Whether the destination has told this process it may never write to
    /// it again. Only the fabric can; the default is no.
    fn fenced(&self) -> bool {
        false
    }
}

/// How the drain runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrainConfig {
    poll: Duration,
    seal_every: Duration,
    max_batches_per_step: usize,
}

impl DrainConfig {
    /// Sleep `poll` when a step delivered nothing, seal the active segment
    /// every `seal_every`, and deliver at most `max_batches_per_step` per
    /// sink per step.
    ///
    /// Refuses a zero poll (a spinning thread), a zero seal cadence (a seal
    /// per step, a segment per batch), and a zero batch count (a drain that
    /// delivers nothing and lets the spool fill until the cell halts).
    pub fn new(poll: Duration, seal_every: Duration, max_batches_per_step: usize) -> Result<Self> {
        if poll.is_zero() {
            return Err(Error::invalid(
                "a drain poll of zero spins on an idle spool; configure a bounded wait",
            ));
        }
        if seal_every.is_zero() {
            return Err(Error::invalid(
                "a seal cadence of zero seals a segment per batch; configure a positive period",
            ));
        }
        if max_batches_per_step == 0 {
            return Err(Error::invalid(
                "a drain that takes zero batches per step never delivers, and the spool fills \
                 until the cell halts; configure at least one",
            ));
        }
        Ok(Self {
            poll,
            seal_every,
            max_batches_per_step,
        })
    }
}

/// What one [`Drain::step`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrainStep {
    /// Batch deliveries that succeeded, summed over sinks.
    pub delivered: usize,
    /// Sinks that refused a batch this step.
    pub refused: Vec<(&'static str, String)>,
    /// Sealed segments retired from the spool.
    pub retired: usize,
}

#[derive(Debug)]
struct Cursor {
    sink: Box<dyn Sink>,
    next: u64,
}

/// The drain: the spool's reader and retirer.
#[derive(Debug)]
pub struct Drain {
    log: Arc<SegmentLog>,
    cursors: Vec<Cursor>,
    publisher: DrainPublisher,
    telemetry: Arc<OutboxTelemetry>,
    config: DrainConfig,
    last_seal: Instant,
}

impl Drain {
    /// A drain over `log` delivering to `sinks`, each resuming from the
    /// cursor the spool's manifest holds for it.
    ///
    /// Refuses two sinks under one name, which would share a cursor, and a
    /// committed cursor past what the spool holds: that is a manifest from
    /// some other spool, and resuming from it would skip records.
    pub fn open(
        log: Arc<SegmentLog>,
        sinks: Vec<Box<dyn Sink>>,
        publisher: DrainPublisher,
        telemetry: Arc<OutboxTelemetry>,
        config: DrainConfig,
    ) -> Result<Self> {
        let high_water = log.high_water();
        let mut seen = std::collections::BTreeSet::new();
        let mut cursors = Vec::with_capacity(sinks.len());
        for sink in sinks {
            let name = sink.name();
            if !seen.insert(name) {
                return Err(Error::invalid(format!(
                    "two drain sinks are both named {name}; they would share one cursor and \
                     each would skip what the other took"
                )));
            }
            let next = match log.manifest_get(&format!("{CURSOR_PREFIX}{name}"))? {
                None => 0,
                Some(bytes) => parse_cursor(name, &bytes)?,
            };
            if next > high_water {
                return Err(Error::invalid(format!(
                    "the {name} drain cursor is at {next} and the spool holds only up to \
                     {high_water}; that cursor was committed against another spool, and \
                     resuming from it would skip records — inspect the spool rather than clear it"
                )));
            }
            cursors.push(Cursor { sink, next });
        }
        Ok(Self {
            log,
            cursors,
            publisher,
            telemetry,
            config,
            last_seal: Instant::now(),
        })
    }

    /// Each sink's next offset, by name.
    pub fn cursors(&self) -> BTreeMap<&'static str, u64> {
        self.cursors
            .iter()
            .map(|cursor| (cursor.sink.name(), cursor.next))
            .collect()
    }

    /// Deliver what each sink has not yet taken, seal on the cadence, and
    /// retire what every sink has read past.
    pub fn step(&mut self) -> Result<DrainStep> {
        let mut report = DrainStep::default();
        let high_water = self.log.high_water();
        for cursor in &mut self.cursors {
            let mut taken = 0usize;
            while cursor.next < high_water && taken < self.config.max_batches_per_step {
                let Some(batch) = self.log.read(cursor.next)? else {
                    break;
                };
                let stream = stream_of(&batch)?;
                match cursor.sink.deliver(cursor.next, stream, &batch) {
                    Ok(()) => {
                        cursor.next = cursor.next.saturating_add(1);
                        self.log.manifest_put(
                            &format!("{CURSOR_PREFIX}{}", cursor.sink.name()),
                            cursor.next.to_string().as_bytes(),
                        )?;
                        taken += 1;
                        self.telemetry.drain("delivered");
                    }
                    Err(error) => {
                        self.telemetry.drain("refused");
                        if cursor.sink.fenced() {
                            self.publisher.mark_fenced();
                        }
                        report
                            .refused
                            .push((cursor.sink.name(), error.message().to_string()));
                        break;
                    }
                }
            }
            report.delivered += taken;
        }
        let connected = report.refused.is_empty();
        self.publisher.set_connected(connected);
        self.telemetry.connected(connected);

        if self.last_seal.elapsed() >= self.config.seal_every {
            self.log.seal_active()?;
            self.last_seal = Instant::now();
        }
        let floor = self
            .cursors
            .iter()
            .map(|cursor| cursor.next)
            .min()
            .unwrap_or(high_water);
        for segment in self.log.segments() {
            if segment.sealed
                && segment.end_offset <= floor
                && !self.log.is_archived(segment.start_offset)?
            {
                self.log.mark_archived(segment.start_offset)?;
            }
        }
        report.retired = self.log.retain_before(floor)?.deleted.len();
        Ok(report)
    }

    /// Run [`Self::step`] on a thread of its own until stopped.
    pub fn spawn(mut self) -> Result<DrainThread> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let poll = self.config.poll;
        let handle = std::thread::Builder::new()
            .name(DRAIN_THREAD.to_string())
            .spawn(move || {
                let mut failures: u64 = 0;
                while !thread_stop.load(Ordering::Acquire) {
                    match self.step() {
                        Ok(step) => {
                            for (sink, reason) in &step.refused {
                                if failures.is_multiple_of(100) {
                                    eprintln!(
                                        "qip-edge-node: the {sink} sink refused a spool batch \
                                         ({reason}); it stays on the spool and is retried"
                                    );
                                }
                                failures = failures.saturating_add(1);
                            }
                            if step.delivered == 0 {
                                std::thread::sleep(poll);
                            }
                        }
                        Err(error) => {
                            if failures.is_multiple_of(100) {
                                eprintln!(
                                    "qip-edge-node: the spool drain failed a step ({}); the \
                                     spool is retried and fills meanwhile",
                                    error.message()
                                );
                            }
                            failures = failures.saturating_add(1);
                            self.publisher.set_connected(false);
                            std::thread::sleep(poll);
                        }
                    }
                }
            })
            .map_err(|error| {
                Error::io(format!("cannot start the {DRAIN_THREAD} thread: {error}"))
            })?;
        Ok(DrainThread {
            stop,
            handle: Some(handle),
        })
    }
}

fn parse_cursor(name: &str, bytes: &[u8]) -> Result<u64> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .ok_or_else(|| {
            Error::schema(format!(
                "the {name} drain cursor in the spool manifest does not read as an offset; \
                 restarting it from zero would deliver everything again — inspect it"
            ))
        })
}

/// The running drain. Dropping it stops the thread after its current step;
/// it is not joined, because a step blocked on an unreachable fabric would
/// make the drop hang with it.
#[derive(Debug)]
pub struct DrainThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl DrainThread {
    pub fn is_running(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }
}

impl Drop for DrainThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

// --- the fabric -------------------------------------------------------------

/// Delivers P1 batches to [`OUTCOMES_STREAM`] and P2 batches to
/// [`JOURNAL_STREAM`] on `qip-fabricd`, one idempotent producer per stream.
#[derive(Debug)]
pub struct FabricSink {
    outcomes: Producer,
    journal: Producer,
}

impl FabricSink {
    /// A sink over two producers, which must name the two reflex streams:
    /// a producer pointed at the wrong stream would put outcomes where the
    /// ledger never reads them.
    pub fn new(outcomes: Producer, journal: Producer) -> Result<Self> {
        if outcomes.stream() != OUTCOMES_STREAM || journal.stream() != JOURNAL_STREAM {
            return Err(Error::invalid(format!(
                "the fabric sink produces outcomes to {OUTCOMES_STREAM} and the journal to \
                 {JOURNAL_STREAM}; it was handed producers for {} and {}",
                outcomes.stream(),
                journal.stream()
            )));
        }
        Ok(Self { outcomes, journal })
    }
}

impl Sink for FabricSink {
    fn name(&self) -> &'static str {
        "fabric"
    }

    fn deliver(&mut self, _offset: u64, stream: QosClass, batch: &Batch) -> Result<()> {
        let producer = match stream {
            QosClass::P1Outcomes => &mut self.outcomes,
            QosClass::P2MarketJournal => &mut self.journal,
            other => {
                return Err(Error::schema(format!(
                    "the spool holds a {} batch, and this node's writer only writes P1 and P2",
                    other.as_str()
                )));
            }
        };
        // An epoch is taken on first use and again after a fence was for an
        // epoch this producer no longer holds; `init` is the broker's
        // statement of where this producer's dense sequence continues.
        if producer.epoch().is_none() {
            producer.init()?;
        }
        producer.send(batch.clone()).map(|_| ())
    }

    fn fenced(&self) -> bool {
        self.outcomes.is_fenced() || self.journal.is_fenced()
    }
}

// --- the tick lake -------------------------------------------------------------

/// The variable naming which datasets' ticks the lake may keep, and how:
/// `dataset=canonical[|raw];dataset=canonical;…` (R12-DATA-1039).
pub const RETENTION_VARIABLE: &str = "QIP_TICK_RETENTION";

/// The variable naming the codec new lake segments are sealed with:
/// `identity` (the default), `zstd` or `zstd:<level>` (R12-TICK-2057, ADR
/// 0113 §3 — off until a measurement says otherwise).
pub const CODEC_VARIABLE: &str = "QIP_TICK_LAKE_CODEC";

/// The retention policy a node starts with when nothing is configured: the
/// simulator's own depth, canonical only, and nothing else.
///
/// The simulated dataset is generated in this process by the platform's own
/// matching engine, so no venue's terms govern it and keeping it breaches
/// nobody's rights. Every licensed dataset is absent until an operator names
/// what its terms allow — rights nobody evaluated are not rights — and its
/// ticks are used live and not kept.
pub fn default_retention() -> BTreeMap<String, RetentionRight> {
    BTreeMap::from([(
        crate::feed::SIMULATED_DATASET.to_string(),
        RetentionRight {
            canonical: true,
            raw: false,
        },
    )])
}

/// Parse [`RETENTION_VARIABLE`] over [`default_retention`]. Refuses a clause
/// it cannot read and a right it does not know, by name: a typo that read as
/// "retain nothing" would silently drop a licensed history the desk was
/// promised, and one that read as "retain everything" would keep bytes the
/// licence forbids.
pub fn parse_retention(text: Option<&str>) -> Result<BTreeMap<String, RetentionRight>> {
    let mut policy = default_retention();
    let Some(text) = text else {
        return Ok(policy);
    };
    for clause in text.split(';').map(str::trim).filter(|c| !c.is_empty()) {
        let (dataset, rights) = clause.split_once('=').ok_or_else(|| {
            Error::invalid(format!(
                "{RETENTION_VARIABLE} clause {clause:?} is not dataset=canonical[|raw]"
            ))
        })?;
        let dataset = dataset.trim();
        if dataset.is_empty() {
            return Err(Error::invalid(format!(
                "{RETENTION_VARIABLE} clause {clause:?} names no dataset"
            )));
        }
        let mut right = RetentionRight {
            canonical: false,
            raw: false,
        };
        for token in rights.split('|').map(str::trim).filter(|t| !t.is_empty()) {
            match token {
                "canonical" => right.canonical = true,
                "raw" => right.raw = true,
                "none" => {}
                other => {
                    return Err(Error::invalid(format!(
                        "{RETENTION_VARIABLE} grants {dataset} the right {other:?}; the rights                          are canonical, raw and none"
                    )));
                }
            }
        }
        if right.raw && !right.canonical {
            return Err(Error::invalid(format!(
                "{RETENTION_VARIABLE} lets {dataset} keep raw bytes and not the canonical \
                 record; the lake keeps raw bytes only beside the record they decode to"
            )));
        }
        policy.insert(dataset.to_string(), right);
    }
    Ok(policy)
}

/// Delivers the market events in P2 batches to the tick lake, one sealed
/// segment per spool offset per partition. Every other record is not the
/// lake's: a journal entry is the platform's own record and reaches the lake
/// through the central archive, not from a cell.
///
/// What is kept is the retention policy's to decide, per dataset
/// (R12-DATA-1039): a tick whose dataset has no right, or a right that does
/// not permit the canonical record, is used live and not written, and is
/// counted in [`LakeSink::withheld`]. The batch still completes — withholding
/// is the policy working, not a failed delivery, and retrying it would stall
/// the spool until the cell halted.
#[derive(Debug)]
pub struct LakeSink {
    store: Arc<dyn BlobStore>,
    cell: String,
    retention: BTreeMap<String, RetentionRight>,
    codec: SegmentCodec,
    withheld: u64,
}

impl LakeSink {
    /// A lake sink for `cell`, keeping what `retention` permits and sealing
    /// with `codec`. Refuses a cell id the lake cannot put in a segment id —
    /// refused here at start, rather than as a sink that refuses every batch
    /// and lets the spool fill until the cell halts.
    ///
    /// The policy and codec are arguments rather than defaults so that no
    /// caller can build a sink without having stated what it keeps; a
    /// caller with nothing configured passes [`default_retention`] and
    /// [`SegmentCodec::Identity`] and says so.
    pub fn new(
        store: Arc<dyn BlobStore>,
        cell: impl Into<String>,
        retention: BTreeMap<String, RetentionRight>,
        codec: SegmentCodec,
    ) -> Result<Self> {
        let cell = cell.into();
        if cell.is_empty()
            || !cell
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(Error::invalid(format!(
                "cell id {cell:?} cannot name a lake segment: use letters, digits, '.', '_' or \
                 '-'"
            )));
        }
        Ok(Self {
            store,
            cell,
            retention,
            codec,
            withheld: 0,
        })
    }

    /// Ticks the retention policy kept out of the lake so far.
    pub const fn withheld(&self) -> u64 {
        self.withheld
    }
}

impl Sink for LakeSink {
    fn name(&self) -> &'static str {
        "lake"
    }

    fn deliver(&mut self, offset: u64, stream: QosClass, batch: &Batch) -> Result<()> {
        if stream != QosClass::P2MarketJournal {
            return Ok(());
        }
        // Keyed by partition so one segment holds one venue, day,
        // instrument and licence; a BTreeMap so the write order — and a
        // retry's — is the same every time.
        let mut by_partition: BTreeMap<(String, String, String, String), Vec<u8>> = BTreeMap::new();
        for record in &batch.records {
            let event = record.decode_payload(batch.encoding)?;
            if event.topic != MARKET_EVENT_APPLIED.topic {
                continue;
            }
            let tick: MarketEvent = market_event(&event)?;
            let permitted = self
                .retention
                .get(tick.entitlement().dataset())
                .is_some_and(|right| right.canonical);
            if !permitted {
                self.withheld = self.withheld.saturating_add(1);
                continue;
            }
            let line = tick_line(&tick)?;
            let key = (
                tick.origin().venue.as_str().to_string(),
                tick.event_time().to_date_string(),
                tick.payload().object_id.as_str().to_string(),
                tick.entitlement().dataset().to_string(),
            );
            let body = by_partition.entry(key).or_default();
            body.extend_from_slice(line.as_bytes());
            body.push(b'\n');
        }
        let lake = Lake::new(self.store.as_ref()).with_codec(self.codec);
        let segment_id = format!("{}-o{offset:020}", self.cell);
        for ((venue, date, instrument, dataset), canonical) in by_partition {
            // The sink holds decoded events, not the venue's wire bytes, so
            // there is no raw payload to keep here; the partition still
            // records what the dataset's right would have allowed.
            let raw_retention_permitted = self.retention.get(&dataset).is_some_and(|r| r.raw);
            let partition = Partition::new(
                RecordClass::Market,
                &venue,
                &date,
                &instrument,
                Entitlement {
                    id: dataset,
                    raw_retention_permitted,
                },
            )?;
            match lake.write_segment(&partition, &segment_id, canonical, None) {
                Ok(_) => {}
                // A segment id is one spool offset of one cell, so a sealed
                // one is this sink's own earlier write of these same records,
                // retried after a later partition of the batch failed. Any
                // other refusal is returned and the batch retried.
                Err(error) if error.code() == "denied" => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// One tick as the lake holds it: the venue's instant, the node's capture
/// instant and the feed's own sequence named beside the event, so a reader
/// of the lake does not have to know the event's serde shape to find the
/// three facts it is partitioned and ordered by.
fn tick_line(tick: &MarketEvent) -> Result<String> {
    let origin = tick.origin();
    let value = serde_json::json!({
        "exchange_time_ns": tick.event_time().as_nanos(),
        "capture_time_ns": tick.receive_time().as_nanos(),
        "feed": origin.feed,
        "partition": origin.partition,
        "sequence": origin.sequence,
        "event": serde_json::to_value(tick)?,
    });
    Ok(qip_core::canonical::canonical_json(&value))
}
