//! The Tick/Internal Lake: sealed, partitioned, checksummed history.
//!
//! The lake holds exactly two kinds of record, market events and the
//! platform's own internal records ([`RecordClass`]). There is no variant for a
//! world-source document, so one cannot be written: the prohibition is the
//! closed enum, not a filter somebody must remember to call.
//!
//! Every segment lives under a path that encodes its partition keys, including
//! the entitlement it was obtained under, so a job restricted to one
//! entitlement is restricted by path before it reads a byte, and a writer that
//! has no entitlement to put in the path cannot form one.
//!
//! A segment is written once. A second write to a sealed key is refused, and
//! the writer has no delete: removal belongs to the governed retention policy
//! (blueprint TICK-037), which is not this API. The existence check here is a
//! read followed by a write, so two writers racing on one key can both pass it;
//! the Cloud Storage adapter must add `ifGenerationMatch=0` to close that, and
//! until it does this refuses the sequential overwrite and not the concurrent
//! one.
//!
//! Raw venue bytes are retained only where the entitlement permits it. Where
//! it does not, the segment record still carries the raw payload's SHA-256 so
//! lineage survives, and the bytes are not stored. Compression is not done
//! here: it needs a codec and the workspace has two dependencies.
//!
//! The platform's own outcomes reach the lake through
//! [`Lake::seal_internal_outcomes`], called by [`crate::ChainArchive::absorb`]
//! at the same hand-over that archives the event log (blueprint TICK-065).
//! Which records those are is not decided here: it is the topic's own
//! `requires_permanent_retention`, the declaration the event log already
//! refuses to evict on, so a topic cannot be permanent in the log and absent
//! from the lake because two lists disagreed.

use crate::blob::BlobStore;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_events::envelope::canonical_json;
use qip_events::log::LogRecord;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The blob namespace a composition root opens the lake under.
pub const LAKE_NAMESPACE: &str = "lake";

/// The entitlement the platform's own records are held under. Nobody licensed
/// them to the platform, so there is no vendor to name; the partition still
/// needs one, because a path with no entitlement cannot be formed.
pub const INTERNAL_ENTITLEMENT: &str = "internal";

/// The venue component of an internal partition: the records are the
/// platform's, whichever venue an order inside one names.
pub const INTERNAL_VENUE: &str = "platform";

/// The instrument component of an internal partition. One segment holds every
/// outcome of a hand-over, across instruments, in log order: splitting a fill
/// from the risk verdict that admitted it would separate the two records a
/// reader most needs side by side.
pub const INTERNAL_INSTRUMENT: &str = "all";

/// What the lake accepts. Deliberately closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordClass {
    /// Ticks, quotes, book updates and their canonical forms.
    Market,
    /// The platform's own orders, fills, decisions and execution telemetry.
    Internal,
}

impl RecordClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Internal => "internal",
        }
    }

    /// Parse a class name, refusing anything else by name.
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "market" => Ok(Self::Market),
            "internal" => Ok(Self::Internal),
            other => Err(Error::invalid(format!(
                "the lake holds market and internal history only; {other:?} is not a record class it accepts \
                 (world-source content is re-fetched from its origin, never archived here)"
            ))),
        }
    }
}

/// The entitlement a partition's data was obtained under.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Entitlement {
    pub id: String,
    /// Whether raw venue bytes may be kept under it.
    pub raw_retention_permitted: bool,
}

/// Where a segment lives. Every field is a path component.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Partition {
    pub class: RecordClass,
    pub venue: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub instrument: String,
    pub entitlement: Entitlement,
}

fn component(name: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(Error::invalid(format!(
            "partition {name} {value:?} must be non-empty letters, digits, '.', '_' or '-'; \
             the lake derives object paths from it"
        )));
    }
    Ok(())
}

impl Partition {
    /// Refuses a partition with no entitlement or an unsafe path component.
    pub fn new(
        class: RecordClass,
        venue: &str,
        date: &str,
        instrument: &str,
        entitlement: Entitlement,
    ) -> Result<Self> {
        if entitlement.id.is_empty() {
            return Err(Error::invalid(
                "a partition needs the entitlement its data was obtained under; \
                 name it, or do not write the data",
            ));
        }
        component("venue", venue)?;
        component("instrument", instrument)?;
        component("entitlement", &entitlement.id)?;
        let shaped = date.len() == 10
            && date.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            });
        if !shaped {
            return Err(Error::invalid(format!(
                "partition date {date:?} must be YYYY-MM-DD"
            )));
        }
        Ok(Self {
            class,
            venue: venue.to_string(),
            date: date.to_string(),
            instrument: instrument.to_string(),
            entitlement,
        })
    }

    fn prefix(&self) -> String {
        format!(
            "lake/class={}/venue={}/date={}/instrument={}/entitlement={}",
            self.class.as_str(),
            self.venue,
            self.date,
            self.instrument,
            self.entitlement.id
        )
    }
}

/// What was written for one segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentRecord {
    pub partition: Partition,
    /// Object key of the canonical bytes.
    pub key: String,
    pub canonical_sha256: String,
    /// Hash of the raw payload as captured, kept even when the bytes are not.
    pub raw_sha256: Option<String>,
    /// Object key of the raw bytes, present only where retention is permitted.
    pub raw_key: Option<String>,
}

/// A named, stored description of an input set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub segments: Vec<SegmentRecord>,
}

/// What a job read, and what it was not entitled to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Materialized {
    pub segments: Vec<(SegmentRecord, Vec<u8>)>,
    /// The output lineage: every entitlement actually read.
    pub entitlements_read: BTreeSet<String>,
    /// Segments in the manifest held under an entitlement the job lacks.
    pub withheld: Vec<String>,
}

/// Writes and reads the lake over any [`BlobStore`].
#[derive(Debug)]
pub struct Lake<'a> {
    store: &'a dyn BlobStore,
}

impl<'a> Lake<'a> {
    pub fn new(store: &'a dyn BlobStore) -> Self {
        Self { store }
    }

    /// Seal one segment. A second write to the same segment id is refused.
    pub fn write_segment(
        &self,
        partition: &Partition,
        segment_id: &str,
        canonical: Vec<u8>,
        raw: Option<Vec<u8>>,
    ) -> Result<SegmentRecord> {
        component("segment id", segment_id)?;
        let key = format!("{}/{segment_id}.canonical", partition.prefix());
        if self.store.get(&key)?.is_some() {
            return Err(Error::denied(format!(
                "{key} is sealed; history is never overwritten, write a new segment that references it"
            )));
        }
        let canonical_sha256 = sha256_hex(&canonical);
        let raw_sha256 = raw.as_deref().map(sha256_hex);
        let raw_key = match raw {
            Some(bytes) if partition.entitlement.raw_retention_permitted => {
                let raw_key = format!("{}/{segment_id}.raw", partition.prefix());
                self.store.put(&raw_key, bytes)?;
                Some(raw_key)
            }
            _ => None,
        };
        self.store.put(&key, canonical)?;
        Ok(SegmentRecord {
            partition: partition.clone(),
            key,
            canonical_sha256,
            raw_sha256,
            raw_key,
        })
    }

    /// Seal the platform's own outcomes among `records` as internal history,
    /// one segment per calendar day they occurred on, each line one record in
    /// canonical JSON with its log linkage intact.
    ///
    /// A record is an outcome when its topic requires permanent retention:
    /// orders, fills, positions, risk and compliance verdicts, the kill
    /// switch, and the cycle's own decision record, its attributions and
    /// lessons. Not only the `Irreplaceable` row of the retention table: the
    /// cycle journal is filed as an episode, and a filter on that one row
    /// kept every fill and dropped the record of why nothing was traded. A
    /// market tick or a fetched document in the same slice is left out, so
    /// the lake's internal class holds what cannot be re-fetched and nothing
    /// that can.
    ///
    /// Sealing the same records under the same id again is not an error and
    /// writes nothing: the caller hands over before it advances its own
    /// watermark, so a hand-over that failed after this step is retried with
    /// the same id and must not be refused for having half-succeeded. A
    /// *different* body under a sealed id is still refused.
    pub fn seal_internal_outcomes(
        &self,
        segment_id: &str,
        records: &[&LogRecord],
    ) -> Result<Vec<SegmentRecord>> {
        let mut by_date: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for record in records {
            if !record.event.topic.requires_permanent_retention() {
                continue;
            }
            let line = canonical_json(&serde_json::to_value(record)?);
            let body = by_date
                .entry(record.event.occurred_at.to_date_string())
                .or_default();
            body.extend_from_slice(line.as_bytes());
            body.push(b'\n');
        }
        let mut sealed = Vec::new();
        for (date, canonical) in by_date {
            let partition = Partition::new(
                RecordClass::Internal,
                INTERNAL_VENUE,
                &date,
                INTERNAL_INSTRUMENT,
                Entitlement {
                    id: INTERNAL_ENTITLEMENT.to_string(),
                    raw_retention_permitted: false,
                },
            )?;
            let key = format!("{}/{segment_id}.canonical", partition.prefix());
            let canonical_sha256 = sha256_hex(&canonical);
            if self.store.digest(&key)?.as_deref() == Some(canonical_sha256.as_str()) {
                sealed.push(SegmentRecord {
                    partition,
                    key,
                    canonical_sha256,
                    raw_sha256: None,
                    raw_key: None,
                });
                continue;
            }
            sealed.push(self.write_segment(&partition, segment_id, canonical, None)?);
        }
        Ok(sealed)
    }

    /// There is no delete. Removal goes through the retention policy.
    pub fn delete_segment(&self, key: &str) -> Result<()> {
        Err(Error::denied(format!(
            "{key} cannot be deleted through the lake writer; removal happens only through the governed retention policy"
        )))
    }

    /// Store a manifest. Manifests are sealed like segments.
    pub fn write_manifest(&self, manifest: &Manifest) -> Result<()> {
        component("manifest name", &manifest.name)?;
        let key = format!("lake/_manifests/{}.json", manifest.name);
        if self.store.get(&key)?.is_some() {
            return Err(Error::denied(format!(
                "manifest {} already exists; an input set is described once, name the new one differently",
                manifest.name
            )));
        }
        self.store.put(&key, serde_json::to_vec(manifest)?)
    }

    fn read_manifest(&self, name: &str) -> Result<Manifest> {
        let key = format!("lake/_manifests/{name}.json");
        let bytes = self
            .store
            .get(&key)?
            .ok_or_else(|| Error::not_found(format!("no manifest named {name}")))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Keys of every segment whose stored bytes no longer match the manifest.
    pub fn verify(&self, manifest_name: &str) -> Result<Vec<String>> {
        let manifest = self.read_manifest(manifest_name)?;
        let mut bad = Vec::new();
        for record in &manifest.segments {
            let stored = self.store.get(&record.key)?;
            if stored.as_deref().map(sha256_hex).as_deref() != Some(&record.canonical_sha256) {
                bad.push(record.key.clone());
            }
        }
        Ok(bad)
    }

    /// Re-materialize a manifest for a job granted `granted` entitlements.
    ///
    /// Segments under any other entitlement are not read. A segment whose bytes
    /// do not match its recorded checksum fails the whole read, naming it: a
    /// training input that silently differs from its manifest is not that input.
    pub fn materialize(&self, manifest_name: &str, granted: &[&str]) -> Result<Materialized> {
        let manifest = self.read_manifest(manifest_name)?;
        let mut out = Materialized {
            segments: Vec::new(),
            entitlements_read: BTreeSet::new(),
            withheld: Vec::new(),
        };
        for record in manifest.segments {
            let entitlement = record.partition.entitlement.id.clone();
            if !granted.contains(&entitlement.as_str()) {
                out.withheld.push(record.key);
                continue;
            }
            let bytes = self.store.get(&record.key)?.ok_or_else(|| {
                Error::not_found(format!(
                    "segment {} named by the manifest is missing",
                    record.key
                ))
            })?;
            if sha256_hex(&bytes) != record.canonical_sha256 {
                return Err(Error::invalid(format!(
                    "segment {} fails its checksum; it is not the segment the manifest recorded",
                    record.key
                )));
            }
            out.entitlements_read.insert(entitlement);
            out.segments.push((record, bytes));
        }
        Ok(out)
    }
}
