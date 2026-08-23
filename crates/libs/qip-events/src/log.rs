//! The append-only event log.
//!
//! Serves three purposes at once: durable history, replay source, and audit
//! trail. Each record commits to its predecessor's hash, so removing or editing
//! a record breaks the chain at that point and every point after it —
//! [`EventLog::verify_chain`] finds exactly where.

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
    /// Cap on retained records; oldest lossy-tolerable events are dropped
    /// first when it is reached.
    capacity: Option<usize>,
    /// Whether an appended record is on the platter before `append` returns.
    durability: Durability,
}

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
            capacity: None,
            durability: Durability::Synchronous,
        }
    }

    /// Open a file-backed log, loading any existing records.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut log = Self::in_memory();
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
                log.index(record);
            }
        } else if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(log)
    }

    /// Bound the log's memory. Only events whose topic tolerates loss are
    /// evicted; an order fill is never dropped to save space.
    /// Trade the durability guarantee for throughput, deliberately.
    pub fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }

    /// What this log promises about an appended record.
    pub fn durability(&self) -> Durability {
        self.durability
    }

    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = Some(capacity);
        self
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Append an event, assigning its sequence number and chain hash.
    pub fn append(&mut self, event: &AnyEvent) -> Result<u64> {
        let sequence = self.next_sequence();
        let previous_hash = self
            .records
            .last()
            .map_or_else(|| GENESIS_HASH.to_string(), |r| r.record_hash.clone());

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
        self.enforce_capacity();
        Ok(sequence)
    }

    fn next_sequence(&self) -> u64 {
        self.records.last().map_or(1, |r| r.sequence + 1)
    }

    fn index(&mut self, record: LogRecord) {
        let position = self.records.len();
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

    /// Drop the oldest evictable records once over capacity.
    fn enforce_capacity(&mut self) {
        let Some(capacity) = self.capacity else {
            return;
        };
        if self.records.len() <= capacity {
            return;
        }
        let overflow = self.records.len() - capacity;
        let mut dropped = 0;
        // Retain in order: evict lossy-tolerable records from the front only.
        let retained: Vec<LogRecord> = self
            .records
            .drain(..)
            .filter(|r| {
                if dropped < overflow && r.event.topic.is_lossy_tolerable() {
                    dropped += 1;
                    false
                } else {
                    true
                }
            })
            .collect();
        self.records = retained;
        self.rebuild_indexes();
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
