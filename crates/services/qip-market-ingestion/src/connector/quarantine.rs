//! Where an event goes when it cannot become a record.
//!
//! The alternative to a quarantine is dropping the event, and a dropped event
//! is a source that has started sending something this platform cannot read
//! with nothing to show for it: the record count falls, no error fires, and
//! the first symptom is a model trained on a feed that quietly halved.
//!
//! So nothing is dropped silently. A payload that fails its schema, an event
//! this connector cannot map, a batch whose retries ran out — each is held
//! here with the reason and an excerpt of what arrived, and each becomes a
//! `qip_financial::intelligence::DataQualityFailure` on the bus, which is the
//! topic the platform already alarms on.
//!
//! # Why the store is bounded
//!
//! A source that has broken its schema breaks it for *every* event, so an
//! unbounded quarantine is a process that dies of memory during the incident.
//! The store holds the most recent [`Quarantine::capacity`] entries and counts
//! what it had to drop, so "we are losing quarantine entries too" is itself
//! visible rather than being the thing that hides the outage.

use super::dedup::EventFingerprint;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_financial::intelligence::DataQualityFailure;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Why an event could not become a record.
///
/// Separate variants rather than one string because the operator action
/// differs: a schema violation is a provider to call, a mapping failure is a
/// connector to fix, and exhausted retries are a source that is down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum QuarantineReason {
    /// The payload did not match the manifest's contract.
    SchemaViolation { detail: String },
    /// The source declared a schema version this connector does not read.
    VersionMismatch { detail: String },
    /// The body could not be parsed at all.
    DecodeFailure { detail: String },
    /// The record was built and is structurally impossible — a bar whose high
    /// is below its low, a negative size.
    ValidationFailure { issues: Vec<String> },
    /// The event decoded and could not be turned into a platform record —
    /// an unknown symbol, an interval this connector cannot name.
    MappingFailure { detail: String },
    /// The source failed every attempt the manifest permits.
    RetriesExhausted { attempts: u32, detail: String },
}

impl QuarantineReason {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SchemaViolation { .. } => "schema_violation",
            Self::VersionMismatch { .. } => "version_mismatch",
            Self::DecodeFailure { .. } => "decode_failure",
            Self::ValidationFailure { .. } => "validation_failure",
            Self::MappingFailure { .. } => "mapping_failure",
            Self::RetriesExhausted { .. } => "retries_exhausted",
        }
    }

    /// The reason as the issues a data-quality failure carries.
    pub fn issues(&self) -> Vec<String> {
        match self {
            Self::SchemaViolation { detail }
            | Self::VersionMismatch { detail }
            | Self::DecodeFailure { detail }
            | Self::MappingFailure { detail } => vec![detail.clone()],
            Self::ValidationFailure { issues } => issues.clone(),
            Self::RetriesExhausted { attempts, detail } => {
                vec![format!("{attempts} attempts exhausted: {detail}")]
            }
        }
    }
}

/// One held event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantinedEvent {
    pub source_id: String,
    /// Absent when the failure was upstream of fingerprinting — a body that
    /// did not parse has no event to fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<EventFingerprint>,
    /// The source's key, where there was one.
    pub key: String,
    pub reason: QuarantineReason,
    /// The first bytes of what arrived. Bounded: an entry quoting a megabyte
    /// of HTML is an entry nobody reads.
    pub payload_excerpt: String,
    pub quarantined_at: Timestamp,
}

impl QuarantinedEvent {
    /// The bus event this becomes.
    ///
    /// `rejected` is always true here. A quarantined event is by definition
    /// one that did not become an investment input, and a failure record that
    /// said otherwise would be counted as an admitted record downstream.
    pub fn as_quality_failure(&self, intended_topic: &str) -> DataQualityFailure {
        DataQualityFailure {
            source: self.source_id.clone(),
            intended_topic: intended_topic.to_string(),
            subject_id: Some(self.key.clone()),
            issues: self.reason.issues(),
            detected_at: self.quarantined_at,
            rejected: true,
        }
    }
}

/// A bounded dead-letter store.
#[derive(Clone, Debug)]
pub struct Quarantine {
    source_id: String,
    capacity: usize,
    entries: VecDeque<QuarantinedEvent>,
    /// Held entries dropped to stay inside the capacity.
    overflowed: u64,
    counts: BTreeMap<String, u64>,
}

impl Quarantine {
    pub fn new(source_id: impl Into<String>, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a quarantine with no capacity drops every failed event, which is the failure it \
                 exists to prevent",
            ));
        }
        Ok(Self {
            source_id: source_id.into(),
            capacity,
            entries: VecDeque::with_capacity(capacity.min(1024)),
            overflowed: 0,
            counts: BTreeMap::new(),
        })
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries dropped because the store was full. Non-zero means the
    /// quarantine is itself losing evidence, which is worth an alarm of its
    /// own.
    pub const fn overflowed(&self) -> u64 {
        self.overflowed
    }

    /// How many events each reason has held, ever — including ones since
    /// evicted, so the counts survive the bound.
    pub const fn counts(&self) -> &BTreeMap<String, u64> {
        &self.counts
    }

    pub fn count_of(&self, reason: &QuarantineReason) -> u64 {
        self.counts.get(reason.code()).copied().unwrap_or(0)
    }

    pub fn entries(&self) -> impl Iterator<Item = &QuarantinedEvent> {
        self.entries.iter()
    }

    /// The most recently held entries, newest last.
    pub fn recent(&self, limit: usize) -> Vec<&QuarantinedEvent> {
        let skip = self.entries.len().saturating_sub(limit);
        self.entries.iter().skip(skip).collect()
    }

    /// Hold one event.
    pub fn hold(
        &mut self,
        key: impl Into<String>,
        fingerprint: Option<EventFingerprint>,
        reason: QuarantineReason,
        payload: &str,
        at: Timestamp,
    ) {
        const EXCERPT: usize = 320;
        *self.counts.entry(reason.code().to_string()).or_insert(0) += 1;
        let mut payload_excerpt: String = payload.chars().take(EXCERPT).collect();
        if payload.chars().nth(EXCERPT).is_some() {
            payload_excerpt.push('…');
        }
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
            self.overflowed = self.overflowed.saturating_add(1);
        }
        self.entries.push_back(QuarantinedEvent {
            source_id: self.source_id.clone(),
            fingerprint,
            key: key.into(),
            reason,
            payload_excerpt,
            quarantined_at: at,
        });
    }

    /// Every held entry as a bus event, ready to publish.
    pub fn as_quality_failures(&self, intended_topic: &str) -> Vec<DataQualityFailure> {
        self.entries
            .iter()
            .map(|entry| entry.as_quality_failure(intended_topic))
            .collect()
    }
}
