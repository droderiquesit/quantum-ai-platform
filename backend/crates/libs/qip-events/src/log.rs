//! The append-only event log.
//!
//! Serves three purposes at once: durable history, replay source, and audit
//! trail. Each record commits to its predecessor's hash, so removing or editing
//! a record breaks the chain at that point and every point after it —
//! [`EventLog::verify_chain`] finds exactly where.
//!
//! # Retention is a bound, and the bound has three tiers
//!
//! The in-memory index used to be capped only if a caller asked for a cap, and
//! no production caller ever did: `qip-kernel` builds the log through
//! `EventLog::open`, which left the capacity unset. An event log with no
//! ceiling is a process that dies of memory during exactly the incident it
//! exists to explain.
//!
//! So retention is now always bounded, and what happens at the ceiling depends
//! on what the record is:
//!
//! 1. **Replaceable observations go first.** A record whose
//!    [`Topic::is_lossy_tolerable`] is true — ticks, quotes, book snapshots,
//!    computed features — is replaced by the next one within milliseconds.
//!    These are evicted oldest-first and counted in
//!    [`EventLog::evicted_replaceable`].
//! 2. **Then other observations, reluctantly.** Trades, bars, corporate
//!    actions, news and alternative data are not replaceable in the same
//!    breath, but they are *observations of the outside world*: re-readable
//!    from the source, and still on disk for a file-backed log, because the
//!    file is the durable copy and this index is a working set. They are
//!    evicted only after every replaceable record has already gone, and
//!    counted separately in [`EventLog::evicted_observations`] — a non-zero
//!    count is the signal that retention is too small for the traffic, which
//!    the first counter alone would not distinguish.
//! 3. **The audit trail is never evicted; the append is refused instead.**
//!    A record whose [`Topic::requires_permanent_retention`] is true — reason,
//!    decide, act, learn, the kill switch and autonomy changes — is why this
//!    log exists. Dropping one to make room for the next would leave the
//!    platform acting with no account of what it did, which is worse than
//!    stopping. When nothing evictable remains, [`EventLog::append`] returns a
//!    refusal naming the fix and writes nothing, in memory or to the file.
//!
//! # What this does not fix
//!
//! Two limits are stated here rather than papered over:
//!
//! * **Eviction breaks in-memory chain verification.** The chain is over what
//!   was written; evicting a record from the middle of the retained span
//!   leaves [`EventLog::verify_chain`] reporting the first link whose
//!   predecessor is gone. That was already true of any capped log and is a
//!   further reason the audit-class records are never evicted. For a
//!   file-backed log the file still verifies end to end.
//! * **The JSONL file is still append-only.** Nothing here truncates or rolls
//!   it, so a file-backed log bounds memory but not disk. Segmenting the file
//!   — sealing a segment, recording its final hash as the next segment's
//!   genesis, and archiving it — is the remaining half of retention and is a
//!   separate change; it needs an ADR because it changes what "the log" means
//!   to a replay. Until then, disk is bounded only by the refusal above and by
//!   whatever the deployment archives out of band.

use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{CorrelationId, EventId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::envelope::{AnyEvent, canonical_json};
use crate::topic::{Topic, TopicGroup};

/// One record as written to storage: the event plus its chain linkage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogRecord {
    pub sequence: u64,
    /// Hash of the previous record, or 64 zeroes for the first.
    pub previous_hash: String,
    /// Hash over the sequence, previous hash and canonical event.
    pub record_hash: String,
    pub event: AnyEvent,
}

/// The genesis predecessor hash.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Append-only, hash-chained event storage.
///
/// Records are held in memory and optionally mirrored to a JSONL file. The file
/// is the durable copy; the in-memory index is what queries run against.
#[derive(Debug)]
pub struct EventLog {
    records: Vec<LogRecord>,
    by_correlation: BTreeMap<String, Vec<usize>>,
    by_topic: BTreeMap<Topic, Vec<usize>>,
    by_event_id: BTreeMap<String, usize>,
    path: Option<PathBuf>,
    /// Cap on retained records. Always set: an unbounded default is how this
    /// grew without limit in every production construction.
    capacity: usize,
    /// Whether an appended record is on the platter before `append` returns.
    durability: Durability,
    evicted_replaceable: u64,
    evicted_observations: u64,
    appends_refused: u64,
    /// The highest sequence ever indexed, and the hash of the record that
    /// carried it. Held separately from `records` because eviction can empty
    /// the tail: deriving either from the last retained record would restart
    /// the sequence at 1 and re-anchor the chain at genesis, which would make
    /// two different records share a sequence number and the break invisible.
    last_sequence: u64,
    last_hash: String,
}

/// Records retained by default.
///
/// Chosen to match the event bus's drain and queue ceilings, so a bus that
/// will accept a million events meets a log that will retain them, and no
/// deployment discovers a new limit merely by upgrading. It is a ceiling, not
/// a target: a long-lived file-backed deployment should set a smaller one
/// deliberately, because a million retained records is a gigabyte-scale
/// working set.
pub const DEFAULT_CAPACITY: usize = 1_000_000;

/// Whether an appended record has reached the disk when `append` returns.
///
/// The same choice `qip_storage`'s engine offers, stated the same way,
/// because it is the same question: a write that has reached the operating
/// system has not reached the platter, and the difference only shows up when
/// the power does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    /// `fsync` the file before returning. The default, because this log is
    /// the platform's evidence: a decision record that a power cut can
    /// silently remove is not a record anybody can rely on afterwards.
    #[default]
    Synchronous,
    /// Return once the operating system has the bytes. Survives `kill -9`;
    /// does not survive power loss. Choose it only where losing the last
    /// records is acceptable and say why at the call site.
    OsBuffered,
}

impl Durability {
    /// Whether an appended record survives loss of power.
    pub fn survives_power_loss(self) -> bool {
        matches!(self, Self::Synchronous)
    }
}

impl Default for EventLog {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl EventLog {
    pub fn in_memory() -> Self {
        Self {
            records: Vec::new(),
            by_correlation: BTreeMap::new(),
            by_topic: BTreeMap::new(),
            by_event_id: BTreeMap::new(),
            path: None,
            capacity: DEFAULT_CAPACITY,
            durability: Durability::Synchronous,
            evicted_replaceable: 0,
            evicted_observations: 0,
            appends_refused: 0,
            last_sequence: 0,
            last_hash: GENESIS_HASH.to_string(),
        }
    }

    /// Open a file-backed log at the default capacity, loading any existing
    /// records.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_capacity(path, DEFAULT_CAPACITY)
    }

    /// Open a file-backed log with an explicit retention ceiling.
    ///
    /// The ceiling has to be given here rather than chained afterwards: a file
    /// holding more audit records than the default retains cannot be loaded at
    /// the default at all, and `EventLog::open(p)?.with_capacity(n)` would have
    /// refused before the caller's larger ceiling was ever applied.
    pub fn open_with_capacity(path: impl AsRef<Path>, capacity: usize) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut log = Self::in_memory().with_capacity(capacity)?;
        log.path = Some(path.clone());
        if path.exists() {
            let file = std::fs::File::open(&path)?;
            for (line_number, line) in BufReader::new(file).lines().enumerate() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let record: LogRecord = serde_json::from_str(&line).map_err(|e| {
                    Error::schema(format!(
                        "corrupt log record at line {}: {e}",
                        line_number + 1
                    ))
                })?;
                // Same duplicate-id refusal as a live append: a file that
                // reused an id (corruption, a hand edit, two processes
                // appending to the same path) must fail to load rather than
                // load with `by_event_id` silently pointing at only the
                // later of the two records.
                log.reject_duplicate_event_id(record.event.event_id.as_str())?;
                // Make room before indexing, so loading a file larger than the
                // ceiling never puts the whole file in memory first — which is
                // the failure the ceiling exists to prevent, arriving at
                // start-up instead of during the run.
                log.make_room(record.event.topic)?;
                log.index(record);
            }
        } else if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(log)
    }

    /// Trade the durability guarantee for throughput, deliberately.
    pub fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }

    /// What this log promises about an appended record.
    pub fn durability(&self) -> Durability {
        self.durability
    }

    /// Bound the log's retained records.
    ///
    /// Zero is refused rather than read as one. A log retaining a single
    /// record refuses the second audit record it is given, so the platform
    /// would stop at its first decision — and a log silently promoted from
    /// zero to one would do the same while claiming the operator asked for it.
    /// Neither is a retention policy; both are configuration mistakes, and the
    /// one that stops at construction is the one somebody can fix.
    pub fn with_capacity(mut self, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "an event log with zero capacity retains nothing and refuses every audit record; \
                 give with_capacity the number of records this deployment should hold in memory",
            ));
        }
        self.capacity = capacity;
        Ok(self)
    }

    /// The retention ceiling in force.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Replaceable records — ticks, quotes, books, features — dropped to stay
    /// inside the ceiling.
    pub const fn evicted_replaceable(&self) -> u64 {
        self.evicted_replaceable
    }

    /// Non-replaceable observations — trades, bars, news, alternative data —
    /// dropped once no replaceable record was left to drop. Non-zero means
    /// retention is too small for this traffic, and it is deliberately a
    /// separate number from the replaceable evictions so that signal is not
    /// buried under the routine one.
    pub const fn evicted_observations(&self) -> u64 {
        self.evicted_observations
    }

    /// Appends refused because the ceiling was reached and every retained
    /// record requires permanent retention. Non-zero means the platform
    /// stopped rather than acted without a record.
    pub const fn appends_refused(&self) -> u64 {
        self.appends_refused
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Append an event, assigning its sequence number and chain hash.
    ///
    /// Refuses when retention is full of records that may not be evicted, and
    /// refuses when `event.event_id` is already indexed. The latter is not a
    /// courtesy check: `index` keys `by_event_id` with a plain map `insert`,
    /// so a second record arriving under an id already in the log would
    /// silently overwrite that mapping — [`EventLog::get`] would then return
    /// the newer record for a lookup naming the older one's id, with no error
    /// and no signal that the first record became unreachable by identity.
    /// The record itself would still sit in the hash chain (a second append
    /// always gets a new sequence and a new `record_hash`, so the two can
    /// never be byte-identical the way a retried block can), so
    /// `verify_chain` would keep passing while the audit index quietly lied
    /// about which content a given id names — the same shape of gap as a
    /// chain-position hash reused for different contents, one layer up, in
    /// the index rather than the chain. The refusal comes before the file
    /// write, so a refused append leaves no half-recorded event: nothing in
    /// memory, nothing on disk, and a caller that knows its event was not
    /// recorded.
    pub fn append(&mut self, event: &AnyEvent) -> Result<u64> {
        self.reject_duplicate_event_id(event.event_id.as_str())?;
        self.make_room(event.topic)?;
        let sequence = self.next_sequence();
        let previous_hash = self.last_hash.clone();

        let mut stored = event.clone();
        stored.sequence = sequence;
        let record_hash = compute_record_hash(sequence, &previous_hash, &stored)?;
        let record = LogRecord {
            sequence,
            previous_hash,
            record_hash,
            event: stored,
        };

        if let Some(path) = &self.path {
            let line = serde_json::to_string(&record)?;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            writeln!(file, "{line}")?;
            if self.durability.survives_power_loss() {
                // The chain is only evidence if the record outlives the
                // machine. Without this the append reaches the page cache and
                // a power cut removes a decision nobody can then account for —
                // and because the chain is over what was *retained*, the
                // surviving log still verifies, so the loss is silent.
                file.sync_all()?;
            }
        }
        self.index(record);
        Ok(sequence)
    }

    fn next_sequence(&self) -> u64 {
        self.last_sequence.saturating_add(1)
    }

    /// Refuse an event id already present in the index.
    ///
    /// Called on every append and on every record loaded from a file, so a
    /// collision is caught at the same seam whether it arrives live or is
    /// discovered replaying disk — a corrupt or tampered file that reused an
    /// id must fail to load rather than load with a silently shadowed record.
    fn reject_duplicate_event_id(&self, event_id: &str) -> Result<()> {
        if self.by_event_id.contains_key(event_id) {
            return Err(Error::invalid(format!(
                "event id {event_id} is already recorded in this log; a second record cannot \
                 reuse it, because doing so would silently replace the lookup index's only path \
                 to the first record while both remained in the hash chain — mint a new event id \
                 for genuinely new content, or if this is a redelivery, suppress it before it \
                 reaches the log"
            )));
        }
        Ok(())
    }

    fn index(&mut self, record: LogRecord) {
        let position = self.records.len();
        self.last_sequence = self.last_sequence.max(record.sequence);
        self.last_hash = record.record_hash.clone();
        self.by_correlation
            .entry(record.event.lineage.correlation_id.as_str().to_string())
            .or_default()
            .push(position);
        self.by_topic
            .entry(record.event.topic)
            .or_default()
            .push(position);
        self.by_event_id
            .insert(record.event.event_id.as_str().to_string(), position);
        self.records.push(record);
    }

    /// Make room for one more record, or refuse.
    ///
    /// Evicts the oldest replaceable record; failing that, the oldest
    /// observation; failing that, refuses, because everything left is the audit
    /// trail. Called *before* a record is written rather than after, so the log
    /// never writes something it is about to drop and never drops something to
    /// make room for what it then refuses.
    fn make_room(&mut self, incoming: Topic) -> Result<()> {
        while self.records.len() >= self.capacity {
            // Two passes rather than one: every replaceable record must be gone
            // before an observation is touched, so the counters mean what they
            // say and the cheap loss is always taken first.
            let victim = self
                .records
                .iter()
                .position(|r| r.event.topic.is_lossy_tolerable())
                .map(|index| (index, true))
                .or_else(|| {
                    self.records
                        .iter()
                        .position(|r| !r.event.topic.requires_permanent_retention())
                        .map(|index| (index, false))
                });
            let Some((index, replaceable)) = victim else {
                self.appends_refused = self.appends_refused.saturating_add(1);
                return Err(Error::guard(format!(
                    "event log is full at {} records and every retained record requires permanent \
                     retention, so none may be dropped to admit a {} record; archive the log and \
                     start a new one, or open it with a larger capacity — this log will not \
                     discard an audit record to keep running",
                    self.capacity,
                    incoming.name()
                )));
            };
            self.records.remove(index);
            if replaceable {
                self.evicted_replaceable = self.evicted_replaceable.saturating_add(1);
            } else {
                self.evicted_observations = self.evicted_observations.saturating_add(1);
            }
            self.rebuild_indexes();
        }
        Ok(())
    }

    fn rebuild_indexes(&mut self) {
        self.by_correlation.clear();
        self.by_topic.clear();
        self.by_event_id.clear();
        for (position, record) in self.records.iter().enumerate() {
            self.by_correlation
                .entry(record.event.lineage.correlation_id.as_str().to_string())
                .or_default()
                .push(position);
            self.by_topic
                .entry(record.event.topic)
                .or_default()
                .push(position);
            self.by_event_id
                .insert(record.event.event_id.as_str().to_string(), position);
        }
    }

    pub fn records(&self) -> &[LogRecord] {
        &self.records
    }

    pub fn events(&self) -> impl Iterator<Item = &AnyEvent> {
        self.records.iter().map(|r| &r.event)
    }

    pub fn get(&self, event_id: &EventId) -> Option<&AnyEvent> {
        self.by_event_id
            .get(event_id.as_str())
            .and_then(|i| self.records.get(*i))
            .map(|r| &r.event)
    }

    /// Every event in one lineage chain, in log order.
    ///
    /// This is the decision-reconstruction query: given the correlation id of
    /// an originating observation, it returns observation through learning.
    pub fn by_correlation(&self, correlation_id: &CorrelationId) -> Vec<&AnyEvent> {
        self.by_correlation
            .get(correlation_id.as_str())
            .map(|positions| {
                positions
                    .iter()
                    .filter_map(|i| self.records.get(*i))
                    .map(|r| &r.event)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn by_topic(&self, topic: Topic) -> Vec<&AnyEvent> {
        self.by_topic
            .get(&topic)
            .map(|positions| {
                positions
                    .iter()
                    .filter_map(|i| self.records.get(*i))
                    .map(|r| &r.event)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Direct children of an event, by causation id.
    pub fn children_of(&self, event_id: &EventId) -> Vec<&AnyEvent> {
        self.records
            .iter()
            .filter(|r| {
                r.event
                    .lineage
                    .causation_id
                    .as_ref()
                    .is_some_and(|c| c.as_str() == event_id.as_str())
            })
            .map(|r| &r.event)
            .collect()
    }

    /// Events matching a filter, in log order.
    pub fn query(&self, filter: &EventFilter) -> Vec<&AnyEvent> {
        self.records
            .iter()
            .map(|r| &r.event)
            .filter(|e| filter.matches(e))
            .collect()
    }

    /// Verify the hash chain. Returns the sequence of the first broken link.
    pub fn verify_chain(&self) -> std::result::Result<(), u64> {
        let mut expected_previous = GENESIS_HASH.to_string();
        for record in &self.records {
            if record.previous_hash != expected_previous {
                return Err(record.sequence);
            }
            let recomputed =
                compute_record_hash(record.sequence, &record.previous_hash, &record.event)
                    .map_err(|_| record.sequence)?;
            if recomputed != record.record_hash {
                return Err(record.sequence);
            }
            expected_previous = record.record_hash.clone();
        }
        Ok(())
    }

    /// Replay every event through `handler`, oldest first.
    pub fn replay<F>(&self, mut handler: F) -> Result<usize>
    where
        F: FnMut(&AnyEvent) -> Result<()>,
    {
        for record in &self.records {
            handler(&record.event)?;
        }
        Ok(self.records.len())
    }

    /// Replay only the events matching a filter.
    pub fn replay_filtered<F>(&self, filter: &EventFilter, mut handler: F) -> Result<usize>
    where
        F: FnMut(&AnyEvent) -> Result<()>,
    {
        let mut count = 0;
        for record in &self.records {
            if filter.matches(&record.event) {
                handler(&record.event)?;
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn stats(&self) -> LogStats {
        let mut by_group: BTreeMap<TopicGroup, u64> = BTreeMap::new();
        let mut by_topic: BTreeMap<Topic, u64> = BTreeMap::new();
        for record in &self.records {
            *by_group.entry(record.event.topic.group()).or_insert(0) += 1;
            *by_topic.entry(record.event.topic).or_insert(0) += 1;
        }
        LogStats {
            total: self.records.len() as u64,
            by_group,
            by_topic,
            first_event: self.records.first().map(|r| r.event.occurred_at),
            last_event: self.records.last().map(|r| r.event.occurred_at),
            correlations: self.by_correlation.len() as u64,
        }
    }
}

/// Hash committing to the record's position, its predecessor and its content.
fn compute_record_hash(sequence: u64, previous_hash: &str, event: &AnyEvent) -> Result<String> {
    let value = serde_json::to_value(event)?;
    let material = format!("{sequence}|{previous_hash}|{}", canonical_json(&value));
    Ok(sha256_hex(material.as_bytes()))
}

/// Query predicate over the log.
#[derive(Clone, Debug, Default)]
pub struct EventFilter {
    pub topics: Vec<Topic>,
    pub groups: Vec<TopicGroup>,
    pub correlation_id: Option<CorrelationId>,
    pub producer: Option<String>,
    /// Inclusive lower bound on `occurred_at`.
    pub from: Option<Timestamp>,
    /// Exclusive upper bound on `occurred_at`.
    pub until: Option<Timestamp>,
}

impl EventFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn topic(mut self, topic: Topic) -> Self {
        self.topics.push(topic);
        self
    }

    pub fn group(mut self, group: TopicGroup) -> Self {
        self.groups.push(group);
        self
    }

    pub fn correlation(mut self, id: CorrelationId) -> Self {
        self.correlation_id = Some(id);
        self
    }

    pub fn producer(mut self, producer: impl Into<String>) -> Self {
        self.producer = Some(producer.into());
        self
    }

    /// Restrict to `[from, until)`.
    pub fn between(mut self, from: Timestamp, until: Timestamp) -> Self {
        self.from = Some(from);
        self.until = Some(until);
        self
    }

    /// Everything strictly before `until` — the point-in-time view used to
    /// rebuild what the platform knew at a past instant.
    pub fn as_of(mut self, until: Timestamp) -> Self {
        self.until = Some(until);
        self
    }

    pub fn matches(&self, event: &AnyEvent) -> bool {
        if !self.topics.is_empty() && !self.topics.contains(&event.topic) {
            return false;
        }
        if !self.groups.is_empty() && !self.groups.contains(&event.topic.group()) {
            return false;
        }
        if let Some(correlation) = &self.correlation_id
            && event.lineage.correlation_id.as_str() != correlation.as_str()
        {
            return false;
        }
        if let Some(producer) = &self.producer
            && &event.lineage.producer != producer
        {
            return false;
        }
        if let Some(from) = self.from
            && event.occurred_at < from
        {
            return false;
        }
        if let Some(until) = self.until
            && event.occurred_at >= until
        {
            return false;
        }
        true
    }
}

/// Summary of a log's contents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogStats {
    pub total: u64,
    pub by_group: BTreeMap<TopicGroup, u64>,
    pub by_topic: BTreeMap<Topic, u64>,
    pub first_event: Option<Timestamp>,
    pub last_event: Option<Timestamp>,
    pub correlations: u64,
}
