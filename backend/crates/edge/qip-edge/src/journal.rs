//! The cell's local decision record, and the mirror that ships it.
//!
//! A cell decides without asking the centre, so the centre can only ever learn
//! what a cell did after the fact. That makes the record the whole audit story
//! for everything the platform does at speed, and it has to survive the cell
//! dying: hash-chained here, drained to durable storage by an explicit
//! [`Mirror::ship`] call that never happens on the hot path.
//!
//! Nothing here writes to a file during [`crate::Cell::on_bytes`] or
//! [`crate::Cell::work`]. That is not an optimisation, it is the reason the
//! mirror is asynchronous at all: a decision loop that blocks on a disk is a
//! decision loop whose latency is a storage system's problem.
//!
//! [`Decision`] and [`JournalEntry`] themselves live in
//! [`qip_contracts::reflex`] and are re-exported here, so a reader that must
//! not depend on `qip-edge` — the ledger, the API — can read the journal
//! contract without reaching the cell, the order manager or a venue adapter
//! that happen to share this crate (ADR 0100 §1). This module keeps the
//! in-memory [`Journal`], the [`Mirror`] that ships it and [`MirrorBatch`],
//! none of which a reader needs in order to verify a chain it was handed.

use qip_contracts::reflex::{ChainVersion, seal_v2};
pub use qip_contracts::reflex::{Decision, JournalEntry};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An append-only, hash-chained record of everything the cell decided.
///
/// The chain is what lets the centre detect a cell that dropped entries: a
/// mirror batch whose first entry does not chain onto the last one received is
/// a gap, whatever the sequence numbers claim.
///
/// # Retention
///
/// By default every entry is kept for the session, shipped or not, so a
/// replay can reconstruct it from memory and every existing reader of
/// [`Self::entries`] sees what it always saw. A journal built with
/// [`Self::trimmed_on_ship`] instead drops what [`ship`] has handed to a
/// mirror (red-team m6: the default grows without bound on a cell that runs
/// for days). Trimming never renumbers: `base_sequence` is how many entries
/// have been dropped, the next entry's sequence is `base_sequence` plus what
/// is retained, and `trimmed_tail` is the digest the first retained entry
/// chains onto — so a sequence is never issued twice and a batch shipped
/// after a trim still chains onto the one before it (F2).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "JournalRecord")]
pub struct Journal {
    entries: Vec<JournalEntry>,
    /// How many entries of the session have been handed to a mirror, counted
    /// from the session's first entry and not from the first retained one —
    /// which is what it always counted, since nothing was trimmed before.
    shipped: usize,
    #[serde(skip_serializing_if = "is_zero")]
    base_sequence: usize,
    #[serde(skip_serializing_if = "is_genesis")]
    trimmed_tail: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    trim_on_ship: bool,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn is_genesis(value: &str) -> bool {
    value == Journal::GENESIS
}

fn genesis() -> String {
    Journal::GENESIS.to_string()
}

/// The largest `base_sequence` a deserialised journal may claim.
///
/// A journal read from a file supplies its own counters, and a counter near
/// `usize::MAX` would make the next sequence wrap to zero — a silent
/// reordering of the chain. At this bound a cell recording a billion
/// decisions a second takes 292 years to exhaust the rest of the range.
pub const MAX_BASE_SEQUENCE: usize = i64::MAX as usize;

/// The wire form of a [`Journal`], checked before it becomes one.
#[derive(Deserialize)]
struct JournalRecord {
    entries: Vec<JournalEntry>,
    shipped: usize,
    #[serde(default)]
    base_sequence: usize,
    #[serde(default = "genesis")]
    trimmed_tail: String,
    #[serde(default)]
    trim_on_ship: bool,
}

impl TryFrom<JournalRecord> for Journal {
    type Error = Error;

    fn try_from(record: JournalRecord) -> Result<Self> {
        if record.base_sequence > MAX_BASE_SEQUENCE {
            return Err(Error::invalid(format!(
                "a journal claiming to have trimmed {} entries is beyond the {MAX_BASE_SEQUENCE} a \
                 session can reach; its counters were not written by a journal",
                record.base_sequence
            )));
        }
        let recorded = record
            .base_sequence
            .checked_add(record.entries.len())
            .ok_or_else(|| {
                Error::invalid("a journal's trimmed and retained entries overflow a count")
            })?;
        if record.shipped < record.base_sequence || record.shipped > recorded {
            return Err(Error::invalid(format!(
                "a journal claims {} entries shipped with {} trimmed and {recorded} recorded; \
                 shipped must lie between the two, since only shipped entries are trimmed and \
                 nothing unrecorded ships",
                record.shipped, record.base_sequence
            )));
        }
        Ok(Self {
            entries: record.entries,
            shipped: record.shipped,
            base_sequence: record.base_sequence,
            trimmed_tail: record.trimmed_tail,
            trim_on_ship: record.trim_on_ship,
        })
    }
}

impl Default for Journal {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            shipped: 0,
            base_sequence: 0,
            trimmed_tail: genesis(),
            trim_on_ship: false,
        }
    }
}

impl Journal {
    /// A journal that keeps every entry for the session.
    pub fn new() -> Self {
        Self::default()
    }

    /// A journal that drops every entry [`ship`] hands to a mirror.
    ///
    /// For a cell whose mirror is the durable record — the fabric-mode cell
    /// (SLICE-36) — where keeping shipped entries in memory is an unbounded
    /// second copy. Opt-in because every reader of [`Self::entries`] written
    /// before this existed assumes it sees the whole session.
    pub fn trimmed_on_ship() -> Self {
        Self {
            trim_on_ship: true,
            ..Self::default()
        }
    }

    /// Whether [`ship`] trims this journal behind what it shipped.
    pub fn trims_on_ship(&self) -> bool {
        self.trim_on_ship
    }

    /// The digest an empty chain starts from.
    ///
    /// A fixed, named value rather than an empty string, so a batch that
    /// claims to be the first is distinguishable from one whose predecessor
    /// went missing.
    pub const GENESIS: &'static str = "genesis";

    /// Seal `decision` at the next sequence under chain v2.
    ///
    /// A decision with no canonical form is sealed as a refusal under
    /// `qip_contracts::reflex::GATE_JOURNAL_ENCODING` naming its kind —
    /// see `seal_v2` — so the returned entry is not always the decision
    /// passed in, and a caller that needs to know reads its `decision`.
    pub fn record(&mut self, decision: Decision, at: Timestamp) -> &JournalEntry {
        // `base_sequence` is bounded by `MAX_BASE_SEQUENCE` on every path
        // that sets it and grows by one per entry recorded, so this cannot
        // reach `usize::MAX` inside any session a clock can measure; the
        // saturation is unreachable, and is not a wrap if it were reached.
        let sequence = self.base_sequence.saturating_add(self.entries.len()) as u64;
        let previous = self
            .entries
            .last()
            .map_or_else(|| self.trimmed_tail.clone(), |entry| entry.digest.clone());
        let (decision, digest) = seal_v2(&previous, sequence, at, decision);
        self.entries.push(JournalEntry {
            sequence,
            at,
            decision,
            digest,
            version: ChainVersion::V2,
        });
        self.entries
            .last()
            .unwrap_or_else(|| unreachable!("an entry was just pushed"))
    }

    /// The entries held in memory: the whole session on a journal that does
    /// not trim, and what has not yet shipped on one that does.
    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    /// How many entries the session has recorded, trimmed or retained.
    ///
    /// Means what it meant before trimming existed, so a count taken on a
    /// trimming journal and one that does not are the same number for the
    /// same decisions. [`Self::retained`] is the in-memory count.
    pub fn len(&self) -> usize {
        self.base_sequence.saturating_add(self.entries.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many entries are held in memory.
    pub fn retained(&self) -> usize {
        self.entries.len()
    }

    /// The sequence of the first retained entry — how many have been trimmed.
    pub fn base_sequence(&self) -> u64 {
        self.base_sequence as u64
    }

    /// How many decisions of each kind the retained entries hold.
    pub fn tally(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for entry in &self.entries {
            *counts.entry(entry.decision.kind()).or_insert(0) += 1;
        }
        counts
    }

    /// Verify the retained chain, returning the sequence where it first
    /// breaks.
    ///
    /// Each entry is checked under the version it names. Once a v2 entry
    /// has been seen a later v1 entry is a break: nothing seals v1 any more,
    /// so a v1 entry after a v2 one is either a rolled-back writer or an
    /// entry relabelled to escape v2's sub-second check. A sequence that is
    /// not the next one is a break too, whatever its digest says.
    pub fn verify(&self) -> std::result::Result<(), u64> {
        verify_run(
            &self.trimmed_tail,
            Some(self.base_sequence as u64),
            &self.entries,
        )
    }

    /// Everything not yet handed to a mirror.
    pub fn unshipped(&self) -> &[JournalEntry] {
        let start = self
            .shipped
            .saturating_sub(self.base_sequence)
            .min(self.entries.len());
        &self.entries[start..]
    }

    /// The digest the next batch [`ship`] builds must chain onto: that of
    /// the last shipped entry, or the chain's start if nothing has shipped.
    ///
    /// Read from what the journal holds — the retained entry or the trimmed
    /// tail — never by indexing [`Self::entries`] with a sequence number,
    /// which after a trim names a different entry or none.
    fn shipped_tail(&self) -> String {
        match self.shipped.checked_sub(self.base_sequence) {
            Some(0) | None => self.trimmed_tail.clone(),
            Some(count) => self
                .entries
                .get(count - 1)
                .map_or_else(|| self.trimmed_tail.clone(), |entry| entry.digest.clone()),
        }
    }

    fn mark_shipped(&mut self, count: usize) {
        self.shipped = self.shipped.saturating_add(count).min(self.len());
    }

    /// Drop every retained entry up to and including `sequence`, keeping the
    /// sequence count and the digest the next retained entry chains onto.
    ///
    /// Refused for an entry not yet shipped: a trimmed entry exists nowhere
    /// else, and dropping one before a mirror holds it is a hole in the only
    /// record there is. Returns how many were dropped, zero when `sequence`
    /// is already behind the base.
    pub fn trim_through(&mut self, sequence: u64) -> Result<usize> {
        let through = usize::try_from(sequence)
            .ok()
            .and_then(|sequence| sequence.checked_add(1))
            .ok_or_else(|| Error::invalid(format!("sequence {sequence} is beyond any journal")))?;
        if through <= self.base_sequence {
            return Ok(0);
        }
        if through > self.shipped {
            return Err(Error::invalid(format!(
                "cannot trim through sequence {sequence}: only {} entries have shipped, and an \
                 entry dropped before a mirror holds it is lost; ship first",
                self.shipped
            )));
        }
        let count = through - self.base_sequence;
        let Some(last) = self.entries.get(count - 1) else {
            return Err(Error::invalid(format!(
                "cannot trim through sequence {sequence}: the journal holds only {} entries",
                self.len()
            )));
        };
        self.trimmed_tail = last.digest.clone();
        self.entries.drain(..count);
        self.base_sequence = through;
        Ok(count)
    }
}

/// Verify a run of entries chaining onto `previous`, returning the sequence
/// where it first breaks. `first` pins the first entry's sequence where the
/// caller knows it; a mirror batch does not.
fn verify_run(
    previous: &str,
    first: Option<u64>,
    entries: &[JournalEntry],
) -> std::result::Result<(), u64> {
    let mut previous = previous.to_string();
    let mut expected_sequence = first;
    let mut seen_v2 = false;
    for entry in entries {
        if expected_sequence.is_some_and(|expected| expected != entry.sequence) {
            return Err(entry.sequence);
        }
        if seen_v2 && entry.version == ChainVersion::V1 {
            return Err(entry.sequence);
        }
        seen_v2 |= entry.version == ChainVersion::V2;
        match entry.expected_digest(&previous) {
            Ok(expected) if expected == entry.digest => {}
            _ => return Err(entry.sequence),
        }
        previous = entry.digest.clone();
        expected_sequence = entry.sequence.checked_add(1);
    }
    Ok(())
}

/// A batch of journal entries, carrying enough chain to be checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MirrorBatch {
    pub cell: String,
    pub at: Timestamp,
    /// The digest the first entry chains onto — [`Journal::GENESIS`] for the
    /// first batch of a session.
    pub chains_onto: String,
    pub entries: Vec<JournalEntry>,
    /// Stream watermarks as of this batch, so the centre knows how far the
    /// cell had consumed when it made these decisions.
    pub watermarks: Vec<(String, u64)>,
}

impl MirrorBatch {
    /// Whether this batch chains onto `previous_digest` and is internally
    /// consistent.
    pub fn verify_against(&self, previous_digest: &str) -> Result<()> {
        if self.chains_onto != previous_digest {
            return Err(Error::invalid(format!(
                "mirror batch from {} chains onto {} but the last received was {previous_digest}",
                self.cell, self.chains_onto
            )));
        }
        verify_run(&self.chains_onto, None, &self.entries).map_err(|sequence| {
            Error::invalid(format!(
                "mirror batch from {} breaks its chain at sequence {sequence}",
                self.cell
            ))
        })
    }

    /// The digest a following batch must chain onto.
    pub fn tail_digest(&self) -> String {
        self.entries
            .last()
            .map_or_else(|| self.chains_onto.clone(), |entry| entry.digest.clone())
    }
}

/// Somewhere durable a cell ships its journal to.
///
/// Called only from [`crate::Cell::flush`], never from the hot path. An
/// implementation is free to block; that is the whole point of it being here
/// rather than inline.
pub trait Mirror: std::fmt::Debug {
    fn ship(&mut self, batch: MirrorBatch) -> Result<()>;

    /// What this would need in production, empty when it is usable as is.
    fn required_configuration(&self) -> Vec<String> {
        Vec::new()
    }
}

/// A mirror that keeps batches in memory, for tests and for a cell whose
/// durable target is unreachable.
#[derive(Debug, Default)]
pub struct MemoryMirror {
    batches: Vec<MirrorBatch>,
}

impl MemoryMirror {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn batches(&self) -> &[MirrorBatch] {
        &self.batches
    }

    /// Verify every batch chains onto the last, in order.
    ///
    /// What the centre does on receipt, exercised here so the property is
    /// tested without a durable store.
    pub fn verify_continuity(&self) -> Result<()> {
        let mut previous = Journal::GENESIS.to_string();
        for batch in &self.batches {
            batch.verify_against(&previous)?;
            previous = batch.tail_digest();
        }
        Ok(())
    }
}

impl Mirror for MemoryMirror {
    fn ship(&mut self, batch: MirrorBatch) -> Result<()> {
        self.batches.push(batch);
        Ok(())
    }
}

/// A mirror that appends JSON batches to a file.
#[derive(Debug)]
pub struct FileMirror {
    path: std::path::PathBuf,
}

impl FileMirror {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl Mirror for FileMirror {
    fn ship(&mut self, batch: MirrorBatch) -> Result<()> {
        use std::io::Write;
        let line = serde_json::to_string(&batch).map_err(|error| {
            Error::schema(format!("a mirror batch would not serialize: {error}"))
        })?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| {
                Error::io(format!(
                    "cannot open the mirror at {}: {error}",
                    self.path.display()
                ))
            })?;
        writeln!(file, "{line}")
            .map_err(|error| Error::io(format!("cannot write to the mirror: {error}")))
    }
}

/// Drain a journal into a batch and hand it to a mirror.
///
/// Public so a caller holding a journal without a whole [`crate::Cell`] can
/// ship it, and so the chaining property can be tested without one.
///
/// On a journal built with [`Journal::trimmed_on_ship`] the shipped entries
/// are then dropped from memory; on any other journal they are kept, as they
/// always were.
pub fn ship(
    journal: &mut Journal,
    mirror: &mut dyn Mirror,
    cell: &str,
    watermarks: Vec<(String, u64)>,
    now: Timestamp,
) -> Result<usize> {
    let pending = journal.unshipped().to_vec();
    if pending.is_empty() {
        return Ok(0);
    }
    // From the journal's own record of what it last shipped, not from
    // `entries()[first - 1]`: after a trim the retained entries no longer
    // start at sequence zero, and indexing by sequence reads the wrong entry
    // or none — and none reads as genesis, a batch that claims to start the
    // session in the middle of it.
    let chains_onto = journal.shipped_tail();

    let count = pending.len();
    let last_shipped = pending.last().map(|entry| entry.sequence);
    mirror.ship(MirrorBatch {
        cell: cell.to_string(),
        at: now,
        chains_onto,
        entries: pending,
        watermarks,
    })?;
    journal.mark_shipped(count);
    // Only after the mirror accepted the batch: a batch the mirror refused
    // is still pending, and trimming it would lose the only copy.
    if let (true, Some(through)) = (journal.trim_on_ship, last_shipped) {
        journal.trim_through(through)?;
    }
    Ok(count)
}
