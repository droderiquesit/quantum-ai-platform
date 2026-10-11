//! Market events into the Tick Lake (R12-DATA-2004, R12-TICK-2042, 2050,
//! 2055, 2056, 2089, R12-DATA-1039).
//!
//! The lake could seal a market-class segment and nothing ever did: its only
//! production writer was `seal_internal_outcomes`, so every tick the
//! platform saw was gone once the process moved past it, and "exact replay
//! and retraining" had no input. This module is the writer a capture path
//! calls — the edge node for venue feeds, the connector feed for central
//! ticks — and it keeps what the time model needs distinct on every stored
//! event: event, receive and normalized time, the clock uncertainty, the
//! feed's sequence, and every clock correction applied between the venue's
//! time and the normalized one (R12-TICK-2089).
//!
//! # Retention is decided before a byte is buffered
//!
//! Each entitlement must have a [`RetentionRight`] in the writer's policy.
//! An entitlement the policy does not name is refused, not written under a
//! default: rights nobody evaluated are not rights. A right may permit the
//! canonical record and not the raw venue bytes, in which case the raw
//! payload's SHA-256 is kept and the bytes are not (the lake's own rule);
//! a right that permits neither refuses the event outright.
//!
//! # Bounded
//!
//! Events buffer per partition and are sealed as one segment when a
//! partition reaches `batch`, and every partition is sealed when the total
//! reaches `max_buffered`. Sealing is synchronous, so the buffer cannot grow
//! past the bound; a store that cannot be written makes `capture` return the
//! error rather than hold events it cannot place.
//!
//! # Deterministic
//!
//! A segment's events are ordered by receive time, then sequence, then
//! canonical text, and its id is derived from its content, so the same
//! events sealed twice produce the same segment and the second seal is a
//! no-op rather than a refusal.

use crate::blob::BlobStore;
use crate::lake::{Entitlement, Lake, Partition, RecordClass, SegmentRecord};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_events::envelope::canonical_json;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

// The captured-event types live in `qip_contracts::captured_event` so that the
// edge capture path (`qip-sequencing`) can build them without linking this
// crate, which holds the Redis, warehouse and HTTP clients a Lane 0 stage must
// never be able to reach (`architecture.rs` guards that).
pub use qip_contracts::captured_event::{CapturedMarketEvent, ClockCorrectionLineage};

/// What an entitlement's terms allow the lake to keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionRight {
    /// The canonical record may be kept.
    pub canonical: bool,
    /// The raw venue bytes may be kept too.
    pub raw: bool,
}

/// Lake path components allow letters, digits, `.`, `_`, `-` only.
fn path_safe(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Buffers captured market events per partition and seals them as lake
/// segments. See the module doc.
#[derive(Debug)]
pub struct TickLakeWriter {
    store: Arc<dyn BlobStore>,
    policy: BTreeMap<String, RetentionRight>,
    batch: usize,
    max_buffered: usize,
    buffers: BTreeMap<Partition, Vec<CapturedMarketEvent>>,
    buffered: usize,
    refused: u64,
}

impl TickLakeWriter {
    /// A writer sealing a segment every `batch` events per partition and
    /// everything at `max_buffered` total. Refuses a zero for either, and a
    /// batch above the total, which could never fill.
    pub fn new(
        store: Arc<dyn BlobStore>,
        policy: BTreeMap<String, RetentionRight>,
        batch: usize,
        max_buffered: usize,
    ) -> Result<Self> {
        if batch == 0 || max_buffered == 0 || batch > max_buffered {
            return Err(Error::invalid(format!(
                "a tick lake writer needs 0 < batch ({batch}) <= max_buffered ({max_buffered}); \
                 otherwise it either seals nothing or buffers without bound"
            )));
        }
        Ok(Self {
            store,
            policy,
            batch,
            max_buffered,
            buffers: BTreeMap::new(),
            buffered: 0,
            refused: 0,
        })
    }

    /// Events refused because their entitlement's right did not allow them.
    pub const fn refused(&self) -> u64 {
        self.refused
    }

    /// Events buffered and not yet sealed.
    pub const fn buffered(&self) -> usize {
        self.buffered
    }

    /// Capture one event. Returns the segments sealed as a result, if any.
    ///
    /// Refuses an invalid event and one whose entitlement has no right, or a
    /// right that permits no retention; the latter two are counted.
    pub fn capture(&mut self, event: CapturedMarketEvent) -> Result<Vec<SegmentRecord>> {
        event.validate()?;
        let Some(right) = self.policy.get(&event.entitlement).copied() else {
            self.refused = self.refused.saturating_add(1);
            return Err(Error::denied(format!(
                "entitlement `{}` has no retention right in this writer's policy; nothing is \
                 kept under terms nobody evaluated",
                event.entitlement
            )));
        };
        if !right.canonical {
            self.refused = self.refused.saturating_add(1);
            return Err(Error::denied(format!(
                "entitlement `{}` permits no retention; the event is used live and not kept",
                event.entitlement
            )));
        }
        let partition = Partition::new(
            RecordClass::Market,
            &path_safe(&event.venue),
            &event.event_time.to_date_string(),
            &path_safe(&event.instrument),
            Entitlement {
                id: path_safe(&event.entitlement),
                raw_retention_permitted: right.raw,
            },
        )?;
        let buffer = self.buffers.entry(partition.clone()).or_default();
        buffer.push(event);
        self.buffered += 1;
        let mut sealed = Vec::new();
        if buffer.len() >= self.batch {
            sealed.extend(self.seal(&partition)?);
        }
        if self.buffered >= self.max_buffered {
            sealed.extend(self.flush()?);
        }
        Ok(sealed)
    }

    /// Seal every buffered partition.
    pub fn flush(&mut self) -> Result<Vec<SegmentRecord>> {
        let partitions: Vec<Partition> = self.buffers.keys().cloned().collect();
        let mut sealed = Vec::new();
        for partition in partitions {
            sealed.extend(self.seal(&partition)?);
        }
        Ok(sealed)
    }

    fn seal(&mut self, partition: &Partition) -> Result<Option<SegmentRecord>> {
        let Some(events) = self.buffers.get(partition) else {
            return Ok(None);
        };
        if events.is_empty() {
            return Ok(None);
        }
        let record = Lake::new(self.store.as_ref()).write_market_segment(partition, events)?;
        if let Some(events) = self.buffers.remove(partition) {
            self.buffered = self.buffered.saturating_sub(events.len());
        }
        Ok(Some(record))
    }
}

impl Lake<'_> {
    /// Seal `events` as one market-class segment of `partition`.
    ///
    /// Ordered by receive time, sequence and canonical text; one canonical
    /// JSON line per event. The raw object holds each event's raw bytes,
    /// length-prefixed, only where the partition's entitlement permits raw
    /// retention and every event carries them; otherwise only the hashes
    /// survive, inside the canonical lines. The id is derived from the
    /// content, and re-sealing identical content is a no-op.
    pub fn write_market_segment(
        &self,
        partition: &Partition,
        events: &[CapturedMarketEvent],
    ) -> Result<SegmentRecord> {
        if partition.class != RecordClass::Market {
            return Err(Error::invalid(
                "market events are sealed under the market class only",
            ));
        }
        if events.is_empty() {
            return Err(Error::invalid(
                "a segment of no events describes nothing; seal at least one",
            ));
        }
        let mut lines: Vec<(Timestamp, Option<u64>, String, &CapturedMarketEvent)> = Vec::new();
        for event in events {
            event.validate()?;
            let mut stored = event.clone();
            if stored.raw_sha256.is_none() {
                stored.raw_sha256 = stored.raw.as_deref().map(sha256_hex);
            }
            let line = canonical_json(&serde_json::to_value(&stored)?);
            lines.push((event.receive_time, event.sequence, line, event));
        }
        lines.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut canonical = Vec::new();
        for (_, _, line, _) in &lines {
            canonical.extend_from_slice(line.as_bytes());
            canonical.push(b'\n');
        }
        let raw = if lines.iter().all(|(_, _, _, event)| event.raw.is_some()) {
            let mut raw = Vec::new();
            for (_, _, _, event) in &lines {
                let bytes = event.raw.as_deref().unwrap_or_default();
                raw.extend_from_slice(format!("{}\n", bytes.len()).as_bytes());
                raw.extend_from_slice(bytes);
                raw.push(b'\n');
            }
            Some(raw)
        } else {
            None
        };
        let canonical_sha256 = sha256_hex(&canonical);
        let first = &lines[0];
        let segment_id = format!(
            "seg-{}-{}-{}",
            first.0.as_nanos(),
            lines.len(),
            &canonical_sha256[..16]
        );
        if let Some((key, codec, existing)) = self.sealed(partition, &segment_id)?
            && existing == canonical_sha256
        {
            let raw_key = format!("{}/{segment_id}.raw", partition.prefix());
            let raw_kept = partition.entitlement.raw_retention_permitted
                && self.store.get(&raw_key)?.is_some();
            return Ok(SegmentRecord {
                partition: partition.clone(),
                key,
                canonical_sha256,
                raw_sha256: raw.as_deref().map(sha256_hex),
                raw_key: raw_kept.then_some(raw_key),
                codec,
            });
        }
        self.write_segment(partition, &segment_id, canonical, raw)
    }

    /// Read a market segment back, verifying its checksum, for exact replay.
    pub fn read_market_segment(&self, record: &SegmentRecord) -> Result<Vec<CapturedMarketEvent>> {
        let bytes = self.read_canonical(record)?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            Error::invalid(format!("segment {} is not UTF-8: {error}", record.key))
        })?;
        text.lines()
            .filter(|line| !line.is_empty())
            .map(|line| Ok(serde_json::from_str(line)?))
            .collect()
    }
}
