//! ADR 0100 §8: re-drives `run_pass` from recorded exogenous inputs — tape
//! events, control frames and clock ticks — against a `ManualClock`, never a
//! recording layer wrapped around the venue. SLICE-39.
//!
//! # What is replayed, and from where
//!
//! A running node records, per pass, a [`PassMarker`] and the tape events
//! the pass applied (`event_fabric::inputs`, `event_fabric::writer`); both
//! land on the P2 stream of the node's spool beside the journal entries the
//! pass wrote. [`RecordedSession::from_spool`] reads them back and groups
//! them by pass. The marker carries everything else a pass depends on that
//! did not come from the venue: its clock reading, the fabric positions of
//! the control frames the pass boundary consumed, the wire readings it
//! applied, the configuration and plan digests it ran under and the seed
//! that fixes the simulated venue's matching stream.
//!
//! [`replay`] then rebuilds the node — the caller assembles the cell, feed
//! and seeded gateway exactly as the composition root does, because the
//! marker's `config_digest` is checked against the rebuild and a mismatch
//! is a refusal, not a warning — and re-drives every recorded pass in
//! order: the clock is set to the marker's instant, the control frames at
//! the recorded positions are fetched from a [`ControlSource`] and verified
//! through the same `CapitalDownlink` and `PolicyDownlink` the node verifies
//! with, applied at a [`Handoff`] boundary as the node applies them, the
//! recorded wire readings are re-applied, and `run_pass` runs against the
//! rebuilt simulated venue. After each pass the tape the replayed feed
//! captured is hashed and compared with the marker's `tape_digest`; after
//! the last, the rebuilt cell's journal is compared entry by entry with the
//! journal the spool recorded. Fills and halt flags are journal entries
//! (`Decision::Filled`, `Decision::HaltChanged`), so "every journal entry,
//! fill and flag" is one comparison.
//!
//! # What makes a window unreproducible
//!
//! - A gap record on the pass-marker stream: passes whose inputs the
//!   backlog dropped (red-team M8). The session refuses to load rather than
//!   re-driving the passes either side and calling the middle reproduced.
//! - Applied events that do not hash to their marker's `tape_digest`: the
//!   spool's account of the pass disagrees with itself.
//! - A control position the source does not hold, or holds under another
//!   event id: the frame the pass consumed is not the frame on offer.
//! - A wire reading the marker records in words this module cannot rebuild.
//!   The marker carries each reading as a state word and the value's own
//!   description; an engaged halt's reason and a dark region set are read
//!   back from the description, and an unreadable wire — whose detail is a
//!   reader's error text, not a value the cell took — is refused by pass
//!   number, because applying a guess and comparing journals afterwards
//!   would report the guess as the cause.
//!
//! # A stated limit
//!
//! Control that reached a node over the mesh carries no fabric position and
//! is not a recorded input; only control consumed from the P0 control
//! stream is. ADR 0100 §8 runs the slice with the mesh uplink off and
//! control on P0 for exactly this reason. A session whose control came over
//! the mesh replays with its policy and capital as the rebuild left them,
//! and the journal comparison reports the first entry that differs.
//!
//! # Replay manifests (R12-CONTRACT-7073)
//!
//! Beside the re-drive, [`manifest_for`] and [`manifests`] turn the pass
//! markers a node spooled into [`ReplayManifest`]s that state what the run
//! assumed (ADR 0155 section 16), and refuse a marker recorded under a
//! configuration other than the one the replay assumes.
//!
//! ## Why the refusal is on the digest
//!
//! A marker proves which inputs a pass applied. The assumptions a manifest
//! states — latency, queue, fill, impact — are rendered from the gateway
//! settings the *replaying* side believes the run used, and nothing in the
//! marker names those settings except `config_digest`, which the node widens
//! to cover them on the simulated path (`gateway::widen_config_digest`). So
//! the digest is the only evidence that the assumptions written beside a
//! marker are the run's. A manifest built without comparing it would state
//! the default fill model for a run under another, and its reproducibility
//! hash would certify that statement.
//!
//! ## What this reads, and what it refuses to read
//!
//! [`markers_in`] opens the spool through [`SegmentLog::open`], which is the
//! writer's open: it recovers a torn tail. So it copies the spool's files to
//! a scratch directory and opens the copy, and a running node's spool is
//! never truncated by a replay read (the in-process guard that refuses a
//! second open of one directory does not reach across processes). It
//! refuses a directory that does not exist rather than creating one and
//! reporting no markers. A copy taken mid-append may end in a partial
//! batch, which recovery drops from the copy only.

use crate::control::{Delivery, Handoff};
use crate::event_fabric::writer::{self, SpooledRecord};
use crate::feed::SimulatedFeed;
use crate::gateway::{SimulatedGateway, assumptions_of, widen_config_digest};
use crate::pass::{PassOutcome, PassStats, run_pass};
use crate::reprice::Requoter;
use crate::strategies::StrategyInstaller;
use qip_brokers::exchange::ExchangeSettings;
use qip_contracts::market_event::MarketEvent;
use qip_contracts::reflex::{Decision, JournalEntry};
use qip_contracts::replay::{
    ControlPosition, PassMarker, PressureRecord, ReplayManifest, WireRecord,
};
use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Decimal, ManualClock, Timestamp, sha256_hex};
use qip_edge::cell::{Cell, PolledHalt};
use qip_edge::mesh::{CapitalDownlink, PolicyDownlink, ScriptedFrames};
use qip_edge::pressure::{Exhaustion, JournalPressure, Narrowing};
use qip_edge::region::RegionOutlook;
use qip_edge::trust::TrustRoot;
use qip_events::event_fabric::bindings::{
    EVENT_FABRIC_GAP, MARKET_EVENT_APPLIED, REFLEX_JOURNAL_RECORDED, REFLEX_PASS_MARKED,
};
use qip_events::event_fabric::policy::QosClass;
use qip_events::{AnyEvent, Topic};
use qip_storage::segment::archive::ArchiveReader;
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The most control positions one pass boundary is replayed with. A marker
/// naming more is refused: the node's own boundary drains at most its
/// handoff capacity per pass, so a marker past this bound was not written
/// by the node this module replays.
pub const MAX_CONTROL_PER_PASS: usize = 1024;

// --- the recorded session --------------------------------------------------

/// One pass as the spool recorded it: its marker and the tape events it
/// applied, in the order applied.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedPass {
    pub marker: PassMarker,
    pub applied: Vec<MarketEvent>,
}

/// Every recorded pass of one node session, with the journal the session
/// wrote beside them.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedSession {
    cell: String,
    session: String,
    passes: Vec<RecordedPass>,
    journal: Vec<JournalEntry>,
}

impl RecordedSession {
    /// Read a session back off a spool, in spool order.
    pub fn from_spool(log: &SegmentLog) -> Result<Self> {
        Self::from_records(&writer::records(log)?)
    }

    /// Read a session back off the spool directory a node printed at
    /// start-up, through [`with_spool_copy`]: the directory is copied and the
    /// copy opened, so a torn tail the writer was mid-way through is
    /// recovered on the copy and the node's own spool is left as it was.
    pub fn from_spool_directory(directory: &Path) -> Result<Self> {
        with_spool_copy(directory, Self::from_spool)
    }

    /// Read a session back off the fabric's indexed archive of the stream the
    /// node's spool was relayed to — the source once the local spool has been
    /// retained away (FABRIC-8079), and the "GCS" of the requirement when the
    /// blob store is the bucket.
    pub fn from_archive(reader: &ArchiveReader, stream: &str, partition: u32) -> Result<Self> {
        Self::from_records(&records_from_archive(reader, stream, partition)?)
    }

    /// Group spooled records into passes. See the module documentation for
    /// what is refused.
    pub fn from_records(records: &[SpooledRecord]) -> Result<Self> {
        let mut passes: Vec<RecordedPass> = Vec::new();
        let mut journal: BTreeMap<u64, JournalEntry> = BTreeMap::new();
        for record in records {
            let topic = &record.event.topic;
            if *topic == EVENT_FABRIC_GAP.topic {
                let gap = writer::gap(&record.event)?;
                if gap.stream == REFLEX_PASS_MARKED.topic.name() {
                    return Err(Error::invalid(format!(
                        "passes {}..={} were not recorded ({}); the window cannot be replayed and \
                         re-driving the passes either side would report the middle as reproduced",
                        gap.from_seq, gap.to_seq, gap.reason
                    )));
                }
                continue;
            }
            if record.stream != QosClass::P2MarketJournal {
                continue;
            }
            if *topic == REFLEX_PASS_MARKED.topic {
                let marker = writer::pass_marker(&record.event)?;
                if let Some(last) = passes.last()
                    && last.marker.pass >= marker.pass
                {
                    return Err(Error::invalid(format!(
                        "pass {} was recorded after pass {}; the spool is not in pass order",
                        marker.pass, last.marker.pass
                    )));
                }
                passes.push(RecordedPass {
                    marker,
                    applied: Vec::new(),
                });
            } else if *topic == MARKET_EVENT_APPLIED.topic {
                let event = writer::market_event(&record.event)?;
                let Some(current) = passes.last_mut() else {
                    return Err(Error::invalid(
                        "an applied tape event was recorded before any pass marker; the spool \
                         does not say which pass applied it",
                    ));
                };
                current.applied.push(event);
            } else if *topic == REFLEX_JOURNAL_RECORDED.topic {
                let entry = writer::journal_entry(&record.event)?;
                if let Some(seen) = journal.get(&entry.sequence)
                    && *seen != entry
                {
                    return Err(Error::invalid(format!(
                        "journal sequence {} was recorded twice with different contents; the \
                         spool holds two accounts of one decision",
                        entry.sequence
                    )));
                }
                journal.insert(entry.sequence, entry);
            }
        }
        let Some(first) = passes.first() else {
            return Err(Error::invalid(
                "the spool holds no pass marker; there is no recorded pass to replay",
            ));
        };
        let cell = first.marker.cell.clone();
        let session = first.marker.session.clone();
        for pass in &passes {
            if pass.marker.cell != cell || pass.marker.session != session {
                return Err(Error::invalid(format!(
                    "pass {} belongs to {}/{} but the spool opened on {cell}/{session}; one \
                     replay covers one session of one cell",
                    pass.marker.pass, pass.marker.cell, pass.marker.session
                )));
            }
            let digest = tape_digest(&pass.applied)?;
            if digest != pass.marker.tape_digest {
                return Err(Error::invalid(format!(
                    "pass {}'s recorded tape events hash to {digest} but its marker says {}; \
                     the spool's account of the pass disagrees with itself",
                    pass.marker.pass, pass.marker.tape_digest
                )));
            }
            if pass.marker.control.len() > MAX_CONTROL_PER_PASS {
                return Err(Error::invalid(format!(
                    "pass {} names {} control positions, past the {MAX_CONTROL_PER_PASS} a node \
                     boundary can consume; this marker was not written by the node",
                    pass.marker.pass,
                    pass.marker.control.len()
                )));
            }
        }
        Ok(Self {
            cell,
            session,
            passes,
            journal: journal.into_values().collect(),
        })
    }

    pub fn cell(&self) -> &str {
        &self.cell
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn passes(&self) -> &[RecordedPass] {
        &self.passes
    }

    /// The journal the session wrote, by sequence.
    pub fn journal(&self) -> &[JournalEntry] {
        &self.journal
    }

    /// The first recorded pass's instant: entries at or after it are what a
    /// replay compares, because entries before it were written by the
    /// node's assembly, at its wall clock, before the first recorded input.
    pub fn first_instant(&self) -> Option<Timestamp> {
        self.passes
            .first()
            .map(|pass| Timestamp::from_nanos(pass.marker.now_ns))
    }
}

/// Every record the archive holds for `stream`/`partition`, in offset order,
/// as the spool reader would have served them: the archive indexes every
/// batch offset, so the walk is one read per batch from offset zero until
/// the archive serves nothing.
pub fn records_from_archive(
    reader: &ArchiveReader,
    stream: &str,
    partition: u32,
) -> Result<Vec<SpooledRecord>> {
    let mut out = Vec::new();
    let mut offset = 0u64;
    // One read per batch offset; the next segment's manifest answers the
    // offset after the previous segment's last, and the walk ends at the
    // first offset no manifest covers.
    while let Some(batch) = reader.read_indexed(stream, partition, offset)? {
        let qos = writer::stream_of(&batch)?;
        for record in &batch.records {
            out.push(SpooledRecord {
                offset,
                stream: qos,
                event: record.decode_payload(batch.encoding)?,
            });
        }
        offset = offset
            .checked_add(1)
            .ok_or_else(|| Error::invalid("the archive's offsets ran past what a u64 can count"))?;
    }
    Ok(out)
}

/// The digest a marker carries over the tape a pass applied, computed as
/// the scheduler computes it.
pub fn tape_digest(applied: &[MarketEvent]) -> Result<String> {
    let tape = serde_json::to_value(applied)?;
    Ok(sha256_hex(canonical_json(&tape).as_bytes()))
}

// --- control frames --------------------------------------------------------

/// Where a replay fetches the control frame a recorded position names.
pub trait ControlSource {
    /// The frame at `position`, or `None` when the source holds nothing
    /// there. A frame at the offset under another event id is an error: the
    /// stream was rewritten under the position the pass consumed.
    fn frame_at(&self, position: &ControlPosition) -> Result<Option<AnyEvent>>;
}

/// Control frames read from the fabric's archive of the control stream
/// (`qip_storage::segment::archive::ArchiveReader` over a blob store), by
/// stream, partition and offset.
///
/// A position's offset is read as the control consumer computed it: the
/// batch's base offset plus the record's index in it. The archive indexes a
/// batch by the offset it was appended under, so a frame is found when its
/// batch's base offset is that offset — one record per control batch, which
/// is how the fabric writes P0 control — and reported as absent otherwise,
/// never guessed from a neighbouring batch.
#[derive(Debug)]
pub struct ArchivedControl {
    reader: ArchiveReader,
}

impl ArchivedControl {
    pub fn new(reader: ArchiveReader) -> Self {
        Self { reader }
    }
}

impl ControlSource for ArchivedControl {
    fn frame_at(&self, position: &ControlPosition) -> Result<Option<AnyEvent>> {
        let Some(batch) =
            self.reader
                .read_indexed(&position.stream, position.partition, position.offset)?
        else {
            return Ok(None);
        };
        let index = position
            .offset
            .checked_sub(batch.base_offset)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| {
                Error::io(format!(
                    "the archive served a batch at base offset {} for offset {}, which is before \
                     it",
                    batch.base_offset, position.offset
                ))
            })?;
        let Some(record) = batch.records.get(index) else {
            return Ok(None);
        };
        let frame = record.decode_payload(batch.encoding)?;
        check_event_id(position, &frame)?;
        Ok(Some(frame))
    }
}

/// Control frames held in memory by position: a labelled fixture, or the
/// frames a test fed the recorded node.
#[derive(Debug, Default)]
pub struct FixtureControl {
    frames: BTreeMap<(String, u32, u64), AnyEvent>,
}

impl FixtureControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hold `frame` at `position`. A second frame at one position is refused:
    /// a stream has one record per offset.
    pub fn insert(&mut self, position: &ControlPosition, frame: AnyEvent) -> Result<()> {
        let key = (position.stream.clone(), position.partition, position.offset);
        if self.frames.contains_key(&key) {
            return Err(Error::invalid(format!(
                "{}:{}:{} already holds a frame; a stream has one record per offset",
                position.stream, position.partition, position.offset
            )));
        }
        self.frames.insert(key, frame);
        Ok(())
    }

    /// Forget the frame at `position`, returning whether one was held.
    pub fn remove(&mut self, position: &ControlPosition) -> bool {
        self.frames
            .remove(&(position.stream.clone(), position.partition, position.offset))
            .is_some()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

impl ControlSource for FixtureControl {
    fn frame_at(&self, position: &ControlPosition) -> Result<Option<AnyEvent>> {
        let Some(frame) =
            self.frames
                .get(&(position.stream.clone(), position.partition, position.offset))
        else {
            return Ok(None);
        };
        check_event_id(position, frame)?;
        Ok(Some(frame.clone()))
    }
}

fn check_event_id(position: &ControlPosition, frame: &AnyEvent) -> Result<()> {
    if frame.event_id != position.event_id {
        return Err(Error::invalid(format!(
            "{}:{}:{} holds event {} but the pass consumed event {} there; the control stream \
             was rewritten under a position a pass already applied",
            position.stream,
            position.partition,
            position.offset,
            frame.event_id.as_str(),
            position.event_id.as_str()
        )));
    }
    Ok(())
}

fn one_frame(frame: &AnyEvent) -> Result<ScriptedFrames> {
    let mut source = ScriptedFrames::new(1)?;
    source.push(frame.clone())?;
    Ok(source)
}

// --- the rebuilt node ------------------------------------------------------

/// The node as the caller rebuilt it for the replay: assembled the way the
/// composition root assembles it, with the feed attached to the cell and
/// the simulated venue seeded as the recorded run seeded it.
pub struct Rebuilt {
    pub cell: Cell,
    pub gateway: SimulatedGateway,
    pub feed: SimulatedFeed,
    pub requoter: Option<Requoter>,
    pub strategies: Option<StrategyInstaller>,
    /// The digest the composition root would stamp on this configuration:
    /// `sha256` of the cell configuration's debug form, widened by the
    /// simulated gateway's settings (`gateway::widen_config_digest`).
    pub config_digest: String,
    pub gateway_seed: u64,
}

impl std::fmt::Debug for Rebuilt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rebuilt")
            .field("cell", &self.cell.config().cell_id)
            .field("venue", &self.gateway.venue())
            .field("config_digest", &self.config_digest)
            .field("gateway_seed", &self.gateway_seed)
            .finish_non_exhaustive()
    }
}

/// What the replayed control frames are verified against.
pub struct ReplayTrust {
    pub trust: TrustRoot,
    /// How many applied grants the capital downlink remembers, as the node
    /// configures its own.
    pub grant_memory: usize,
}

impl std::fmt::Debug for ReplayTrust {
    /// The root's keys are not printed: a debug line is a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayTrust")
            .field("trust", &self.trust.describe())
            .field("grant_memory", &self.grant_memory)
            .finish()
    }
}

// --- the verdict -----------------------------------------------------------

/// The first point at which the replay stopped agreeing with the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Divergence {
    /// The rebuilt cell's configuration digest is not the one the passes
    /// ran under. Nothing was re-driven.
    Configuration { recorded: String, rebuilt: String },
    /// The rebuilt venue's seed is not the recorded one. Nothing was
    /// re-driven.
    GatewaySeed { recorded: u64, rebuilt: u64 },
    /// The compiled plan the rebuild deploys is not the one the pass ran
    /// under. Reported on the pass where the digests first differ.
    Plan {
        pass: u64,
        recorded: String,
        rebuilt: String,
    },
    /// The control source holds nothing at a position the pass consumed.
    ControlFrame {
        pass: u64,
        position: ControlPosition,
    },
    /// The replayed feed captured a different tape than the pass applied.
    Tape {
        pass: u64,
        recorded: String,
        replayed: String,
    },
    /// The journals differ at this index (counted from the first entry at
    /// or after the first recorded pass's instant). `None` on one side is a
    /// journal that ended first.
    Journal {
        index: usize,
        recorded: Option<String>,
        replayed: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every pass applied the recorded tape and the journals are equal.
    Reproduced,
    Diverged(Divergence),
}

/// What the replay did and found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    pub cell: String,
    pub session: String,
    /// Passes re-driven before the verdict.
    pub passes: u64,
    pub halted_passes: u64,
    /// Control frames fetched and verified; those the downlinks refused.
    pub control_frames: u64,
    pub control_refused: u64,
    /// Journal entries compared, and among them the fills and halt flags.
    pub journal_entries: u64,
    pub fills: u64,
    pub halt_flags: u64,
    pub verdict: Verdict,
}

impl ReplayReport {
    pub fn is_reproduced(&self) -> bool {
        self.verdict == Verdict::Reproduced
    }

    /// One line per fact, for the operator.
    pub fn describe(&self) -> String {
        let verdict = match &self.verdict {
            Verdict::Reproduced => "REPRODUCED".to_string(),
            Verdict::Diverged(divergence) => format!("DIVERGED: {divergence:?}"),
        };
        format!(
            "replay of {}/{}: {} passes ({} halted), {} control frames ({} refused), {} journal \
             entries compared ({} fills, {} halt flags): {verdict}",
            self.cell,
            self.session,
            self.passes,
            self.halted_passes,
            self.control_frames,
            self.control_refused,
            self.journal_entries,
            self.fills,
            self.halt_flags
        )
    }
}

// --- the replay ------------------------------------------------------------

/// Re-drive `session` through `rebuilt`. See the module documentation.
///
/// `clock` is the manual clock the rebuilt cell was assembled with; it is
/// set to each marker's instant before the pass runs, so anything inside
/// the cell that reads a clock reads the recorded one.
pub fn replay(
    session: &RecordedSession,
    mut rebuilt: Rebuilt,
    control: &dyn ControlSource,
    trust: &ReplayTrust,
    clock: &ManualClock,
) -> Result<ReplayReport> {
    let mut report = ReplayReport {
        cell: session.cell.clone(),
        session: session.session.clone(),
        passes: 0,
        halted_passes: 0,
        control_frames: 0,
        control_refused: 0,
        journal_entries: 0,
        fills: 0,
        halt_flags: 0,
        verdict: Verdict::Reproduced,
    };
    let Some(first) = session.passes.first() else {
        return Err(Error::invalid("the session holds no pass to replay"));
    };
    if first.marker.config_digest != rebuilt.config_digest {
        report.verdict = Verdict::Diverged(Divergence::Configuration {
            recorded: first.marker.config_digest.clone(),
            rebuilt: rebuilt.config_digest,
        });
        return Ok(report);
    }
    if first.marker.gateway_seed != rebuilt.gateway_seed {
        report.verdict = Verdict::Diverged(Divergence::GatewaySeed {
            recorded: first.marker.gateway_seed,
            rebuilt: rebuilt.gateway_seed,
        });
        return Ok(report);
    }
    // Every reading is checked before anything is re-driven: a refusal on
    // pass 40 after 39 passes ran would leave a half-replayed cell behind.
    for pass in &session.passes {
        readings_of(&pass.marker)?;
    }

    let cell_name = session.cell.clone();
    let mut capital = CapitalDownlink::from_source_trusted(
        cell_name.clone(),
        trust.trust.clone(),
        trust.grant_memory,
        Box::new(ScriptedFrames::new(1)?),
    )?;
    let mut policy = PolicyDownlink::from_source_trusted(
        cell_name,
        trust.trust.clone(),
        Box::new(ScriptedFrames::new(1)?),
    );
    let (sender, mut handoff) = Handoff::bounded(MAX_CONTROL_PER_PASS)?;
    let mut stats = PassStats::default();

    for pass in &session.passes {
        let marker = &pass.marker;
        let now = Timestamp::from_nanos(marker.now_ns);
        clock.set(now);
        let readings = readings_of(marker)?;

        // The node's turn order: wires, control boundary, pressure, pass.
        if let Some(halt) = readings.halt {
            rebuilt.cell.apply_polled_halt(halt, now);
        }
        if let Some(outlook) = readings.region {
            rebuilt.cell.apply_region_outlook(outlook, now);
        }
        for position in &marker.control {
            let Some(frame) = control.frame_at(position)? else {
                report.verdict = Verdict::Diverged(Divergence::ControlFrame {
                    pass: marker.pass,
                    position: position.clone(),
                });
                return Ok(report);
            };
            report.control_frames = report.control_frames.saturating_add(1);
            let grants = capital.poll_from(&mut one_frame(&frame)?, now)?;
            let payloads = policy.poll_from(&mut one_frame(&frame)?, now)?;
            report.control_refused = report
                .control_refused
                .saturating_add(grants.refused.len() as u64)
                .saturating_add(payloads.refused.len() as u64);
            for envelope in grants.verified {
                sender.send(Delivery::envelope(position.clone(), envelope))?;
            }
            for halt in payloads.halts {
                sender.send(Delivery::halt(position.clone(), halt))?;
            }
            for payload in payloads.verified {
                sender.send(Delivery::policy(position.clone(), payload))?;
            }
        }
        let boundary = handoff.boundary(&mut rebuilt.cell, rebuilt.strategies.as_mut(), now);
        if boundary.producers_gone {
            return Err(Error::invalid(
                "the replay's own control handoff closed; this is a fault in the replay, not \
                 in the record",
            ));
        }
        let plan_digest = rebuilt
            .strategies
            .as_ref()
            .and_then(StrategyInstaller::compiled_digest)
            .unwrap_or_default();
        if plan_digest != marker.plan_digest {
            report.verdict = Verdict::Diverged(Divergence::Plan {
                pass: marker.pass,
                recorded: marker.plan_digest.clone(),
                rebuilt: plan_digest.to_string(),
            });
            return Ok(report);
        }
        if let Some(pressure) = readings.pressure {
            rebuilt.cell.apply_journal_pressure(pressure, now)?;
        }

        // The node tolerates a pass that errs — the journal holds the
        // refusal and the loop keeps turning — so the replay does too.
        let outcome = run_pass(
            &mut rebuilt.cell,
            &mut rebuilt.gateway,
            &mut rebuilt.feed,
            rebuilt.requoter.as_mut(),
            &mut stats,
            now,
        );
        report.passes = report.passes.saturating_add(1);
        if let Ok(PassOutcome::Halted { .. }) = outcome {
            report.halted_passes = report.halted_passes.saturating_add(1);
        }
        let applied = rebuilt.feed.take_captured();
        let replayed = tape_digest(&applied)?;
        if replayed != marker.tape_digest {
            report.verdict = Verdict::Diverged(Divergence::Tape {
                pass: marker.pass,
                recorded: marker.tape_digest.clone(),
                replayed,
            });
            return Ok(report);
        }
    }

    // The journals, from the first recorded instant on.
    let from = Timestamp::from_nanos(first.marker.now_ns);
    let recorded: Vec<&JournalEntry> = session
        .journal
        .iter()
        .filter(|entry| entry.at >= from)
        .collect();
    let replayed: Vec<&JournalEntry> = rebuilt
        .cell
        .journal()
        .entries()
        .iter()
        .filter(|entry| entry.at >= from)
        .collect();
    let longest = recorded.len().max(replayed.len());
    for index in 0..longest {
        let left = recorded.get(index).copied();
        let right = replayed.get(index).copied();
        match (left, right) {
            (Some(left), Some(right)) if left.decision == right.decision && left.at == right.at => {
                report.journal_entries = report.journal_entries.saturating_add(1);
                match &left.decision {
                    Decision::Filled { .. } => report.fills = report.fills.saturating_add(1),
                    Decision::HaltChanged { .. } => {
                        report.halt_flags = report.halt_flags.saturating_add(1);
                    }
                    _ => {}
                }
            }
            _ => {
                report.verdict = Verdict::Diverged(Divergence::Journal {
                    index,
                    recorded: left.map(describe_entry),
                    replayed: right.map(describe_entry),
                });
                return Ok(report);
            }
        }
    }
    Ok(report)
}

fn describe_entry(entry: &JournalEntry) -> String {
    format!(
        "seq {} at {}: {}",
        entry.sequence,
        entry.at.as_nanos(),
        serde_json::to_string(&entry.decision).unwrap_or_else(|_| format!("{:?}", entry.decision))
    )
}

// --- readings --------------------------------------------------------------

/// The wire readings a marker recorded, rebuilt as values the cell takes.
struct Readings {
    pressure: Option<JournalPressure>,
    halt: Option<PolledHalt>,
    region: Option<RegionOutlook>,
}

fn readings_of(marker: &PassMarker) -> Result<Readings> {
    Ok(Readings {
        pressure: marker
            .readings
            .journal_pressure
            .as_ref()
            .map(|record| pressure_of(marker.pass, record))
            .transpose()?,
        halt: marker
            .readings
            .halt_flag
            .as_ref()
            .map(|record| halt_of(marker.pass, record))
            .transpose()?,
        region: marker
            .readings
            .region_wire
            .as_ref()
            .map(|record| region_of(marker.pass, record))
            .transpose()?,
    })
}

/// The scheduler records `state_of(pressure)` and `pressure.describe()`;
/// this is the inverse, and refuses what it cannot invert.
fn pressure_of(pass: u64, record: &PressureRecord) -> Result<JournalPressure> {
    match record.state.as_str() {
        "normal" => Ok(JournalPressure::Normal),
        "narrow" => {
            let multiplier = record
                .detail
                .strip_prefix("narrow to ")
                .and_then(Decimal::parse)
                .ok_or_else(|| unreadable(pass, "journal pressure", record))?;
            Ok(JournalPressure::Narrow(Narrowing::new(multiplier)?))
        }
        "exhausted" => {
            let cause = record
                .detail
                .strip_prefix("exhausted (")
                .and_then(|rest| rest.strip_suffix(')'))
                .ok_or_else(|| unreadable(pass, "journal pressure", record))?;
            let cause = [
                Exhaustion::Fenced,
                Exhaustion::Unwritable,
                Exhaustion::Stale,
                Exhaustion::OverBudget,
            ]
            .into_iter()
            .find(|candidate| candidate.as_str() == cause)
            .ok_or_else(|| unreadable(pass, "journal pressure", record))?;
            Ok(JournalPressure::Exhausted(cause))
        }
        _ => Err(unreadable(pass, "journal pressure", record)),
    }
}

/// The scheduler records `halt_word(halt)` and `halt.describe()`; an
/// engaged flag's reason is in the description and is read back from it.
fn halt_of(pass: u64, record: &WireRecord) -> Result<PolledHalt> {
    match record.state.as_str() {
        "absent" => Ok(PolledHalt::Absent),
        "released" => Ok(PolledHalt::Released),
        "engaged" => record
            .detail
            .strip_prefix("is engaged: ")
            .map(|reason| PolledHalt::Engaged(reason.to_string()))
            .ok_or_else(|| unreadable_wire(pass, "polled halt flag", record)),
        _ => Err(unreadable_wire(pass, "polled halt flag", record)),
    }
}

/// The scheduler records `region_word(outlook)` and `outlook.describe()`;
/// the dark set is in the description and is read back from it. An
/// unreadable wire is refused: its detail is a reader's error text, not a
/// value the cell took.
fn region_of(pass: u64, record: &WireRecord) -> Result<RegionOutlook> {
    match record.state.as_str() {
        "all_lit" => Ok(RegionOutlook::AllLit),
        "dark" => {
            let named = record
                .detail
                .split_once(" dark: ")
                .map(|(_, names)| names)
                .ok_or_else(|| unreadable_wire(pass, "region wire", record))?;
            let regions: std::collections::BTreeSet<String> = named
                .split(", ")
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect();
            if regions.is_empty() {
                return Err(unreadable_wire(pass, "region wire", record));
            }
            Ok(RegionOutlook::Dark(regions))
        }
        _ => Err(unreadable_wire(pass, "region wire", record)),
    }
}

fn unreadable_wire(pass: u64, wire: &str, record: &WireRecord) -> Error {
    Error::invalid(format!(
        "pass {pass} ran under a {wire} reading '{}' ({}) this build cannot rebuild; the window \
         cannot be replayed without guessing it",
        record.state, record.detail
    ))
}

fn unreadable(pass: u64, wire: &str, record: &PressureRecord) -> Error {
    Error::invalid(format!(
        "pass {pass}'s marker records a {wire} of '{}' ({}) this build cannot rebuild; the \
         window cannot be replayed",
        record.state, record.detail
    ))
}

// --- replay manifests (R12-CONTRACT-7073) ----------------------------------

/// The manifest for `marker`, stating the assumptions of a simulated gateway
/// built with `settings` on a node whose cell configuration digests to
/// `cell_digest`.
///
/// Refused when the marker's `config_digest` is not that pair widened: the
/// pass ran under another configuration or other gateway settings, and the
/// assumptions this would state are not the run's.
pub fn manifest_for(
    marker: PassMarker,
    cell_digest: &str,
    settings: &ExchangeSettings,
) -> Result<ReplayManifest> {
    let expected = widen_config_digest(cell_digest, settings);
    if marker.config_digest != expected {
        return Err(Error::invalid(format!(
            "pass {} of cell {} was recorded under configuration digest {}, and the replay \
             assumes {expected}; the pass ran under another configuration or other gateway \
             settings, so the assumptions a manifest would state are not the run's. Replay \
             under the configuration the node printed at start-up",
            marker.pass, marker.cell, marker.config_digest
        )));
    }
    ReplayManifest::with_assumptions(
        marker,
        assumptions_of(settings),
        SimulatedGateway::replay_model_versions(),
    )
}

/// The manifest of every marker in `markers`, refused as a whole on the first
/// one [`manifest_for`] refuses, and refused when there are none — an empty
/// replay agrees with every configuration.
pub fn manifests(
    markers: Vec<PassMarker>,
    cell_digest: &str,
    settings: &ExchangeSettings,
) -> Result<Vec<ReplayManifest>> {
    if markers.is_empty() {
        return Err(Error::invalid(
            "the spool holds no pass markers, so there is nothing to replay; a node writes one \
             per pass only while QIP_VENUE_FEED=simulated runs passes",
        ));
    }
    markers
        .into_iter()
        .map(|marker| manifest_for(marker, cell_digest, settings))
        .collect()
}

/// The pass markers among `records`, in spool order.
pub fn markers(records: &[SpooledRecord]) -> Result<Vec<PassMarker>> {
    records
        .iter()
        .filter(|record| record.event.topic == Topic::ReflexPassMarked)
        .map(|record| writer::pass_marker(&record.event))
        .collect()
}

/// `qip-edge-node replay-manifests --spool <dir> --cell-digest <hex>`: one
/// manifest per spooled pass marker, as one JSON line each, under the
/// simulated gateway's default settings — the settings the node runs the
/// simulator with ([`SimulatedGateway::default_settings`]). The cell digest
/// is the one the node prints at start-up; it is taken rather than
/// recomputed because recomputing it would need the whole node
/// configuration, and a replay that guessed at it would refuse every run.
pub fn run(arguments: &[String]) -> Result<Vec<String>> {
    let flag = |name: &str| {
        arguments
            .iter()
            .position(|argument| argument == name)
            .and_then(|index| arguments.get(index + 1))
            .ok_or_else(|| {
                Error::invalid(format!(
                    "qip-edge-node replay-manifests --spool <dir> --cell-digest <hex>: {name} \
                     is required"
                ))
            })
    };
    let markers = markers_in(Path::new(flag("--spool")?))?;
    manifests(
        markers,
        flag("--cell-digest")?,
        &SimulatedGateway::default_settings(),
    )?
    .iter()
    .map(|manifest| {
        serde_json::to_string(manifest)
            .map_err(|error| Error::invalid(format!("the manifest: {error}")))
    })
    .collect()
}

/// The pass markers in the spool at `directory`. See the module
/// documentation for why the directory must exist and must not be a running
/// node's.
pub fn markers_in(directory: &Path) -> Result<Vec<PassMarker>> {
    with_spool_copy(directory, |log| markers(&writer::records(log)?))
}

/// Open a copy of the spool at `directory` and hand it to `read`.
///
/// Opened on a copy, never in place: `SegmentLog::open` is the writer's open
/// and truncates a torn tail, which on a running node's spool is the batch
/// its writer is appending. The copy is the only thing recovery can touch,
/// and it is removed on the way out whatever the read found. Both readers of
/// a spool directory — the manifests and the re-drive — come through here, so
/// neither can be the one that opens in place.
pub fn with_spool_copy<T>(
    directory: &Path,
    read: impl FnOnce(&SegmentLog) -> Result<T>,
) -> Result<T> {
    if !directory.is_dir() {
        return Err(Error::invalid(format!(
            "{} is not a spool directory; name the directory the node printed as its spool",
            directory.display()
        )));
    }
    static COPIES: AtomicU64 = AtomicU64::new(0);
    // Named for this binary and this purpose, so the copy can collide with
    // no spool a test or an operator names in the same temporary directory:
    // a copy that landed on a live spool's path would be removed with it.
    let copy = std::env::temp_dir().join(format!(
        "qip-edge-node-replay-copy-{}-{}",
        std::process::id(),
        COPIES.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&copy);
    let outcome = copy_spool(directory, &copy).and_then(|()| {
        // The log's clock only stamps segment rolls, and a read rolls nothing.
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(0)));
        let log = SegmentLog::open(&copy, SegmentLogConfig::new(clock))?;
        read(&log)
    });
    let _ = std::fs::remove_dir_all(&copy);
    outcome
}

/// Copy the spool's files and directories (its segments and its manifest
/// directory) into `to`. Symbolic links are not followed: a spool holds
/// none, and following one would copy whatever it pointed at.
fn copy_spool(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_spool(&entry.path(), &to.join(entry.file_name()))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(state: &str, detail: &str) -> PressureRecord {
        PressureRecord::new(state, detail)
    }

    #[test]
    fn every_pressure_the_scheduler_can_record_is_rebuilt_to_the_value_it_described() {
        let cases = [
            JournalPressure::Normal,
            JournalPressure::Narrow(Narrowing::half()),
            JournalPressure::Exhausted(Exhaustion::Fenced),
            JournalPressure::Exhausted(Exhaustion::Unwritable),
            JournalPressure::Exhausted(Exhaustion::Stale),
            JournalPressure::Exhausted(Exhaustion::OverBudget),
        ];
        for pressure in cases {
            let state = match pressure {
                JournalPressure::Normal => "normal",
                JournalPressure::Narrow(_) => "narrow",
                JournalPressure::Exhausted(_) => "exhausted",
            };
            let rebuilt = pressure_of(1, &record(state, &pressure.describe()))
                .expect("a described pressure reads back");
            assert_eq!(rebuilt, pressure);
        }
    }

    #[test]
    fn a_pressure_reading_this_build_cannot_invert_is_refused_by_pass_number() {
        let error = pressure_of(7, &record("exhausted", "exhausted (never_applied)"))
            .expect_err("never_applied is the cell's own state, not a reading");
        assert!(
            error
                .message()
                .starts_with("pass 7's marker records a journal pressure")
        );
        let error = pressure_of(8, &record("narrow", "narrow to lots"))
            .expect_err("a multiplier that is not a number");
        assert!(error.message().starts_with("pass 8's marker"));
    }

    #[test]
    fn halt_and_region_readings_are_rebuilt_from_their_descriptions_or_refused() {
        assert_eq!(
            halt_of(1, &WireRecord::new("absent", "")).expect("absent reads back"),
            PolledHalt::Absent
        );
        assert_eq!(
            halt_of(3, &WireRecord::new("engaged", "is engaged: operator")).expect("read back"),
            PolledHalt::Engaged("operator".to_string())
        );
        assert!(halt_of(3, &WireRecord::new("engaged", "operator")).is_err());
        assert_eq!(
            region_of(1, &WireRecord::new("all_lit", "")).expect("all lit reads back"),
            RegionOutlook::AllLit
        );
        assert_eq!(
            region_of(
                4,
                &WireRecord::new("dark", "2 region(s) dark: eu-west1, us-east1")
            )
            .expect("read back"),
            RegionOutlook::Dark(["eu-west1".to_string(), "us-east1".to_string()].into())
        );
        assert!(region_of(4, &WireRecord::new("unreadable", "the wire failed")).is_err());
    }
}
