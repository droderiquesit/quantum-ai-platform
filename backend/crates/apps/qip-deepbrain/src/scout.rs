//! The scout's side of ADR 0144: what a research connector fetch becomes when
//! it crosses from the scout job to the deep brain, and the deep brain's
//! reader of it (R12-DATA-8112).
//!
//! The scout polls the catalogued research connectors once and exits, under an
//! identity with no trading role. What it hands over is not a body — the body
//! is transient (ADR 0180 decision 4) — but the records the connector
//! decoded, beside the version-2 manifest of the fetch that produced them.
//!
//! # Immutable, and re-validated on read
//!
//! Each record is written under `<source>/<sha256>.json` with
//! `put_if_absent`, the store's `ifGenerationMatch=0`: a second write of the
//! same fetch leaves the first untouched, and nothing the scout can do
//! replaces an object the deep brain already read. The reader holds the
//! other half of the isolation. It trusts nothing a bucket holds: a record
//! whose key does not name its own source and hash, whose manifest does not
//! match its hash, whose manifest is not version 2, or which does not decode
//! is **absent** and counted, never half-read. A compromised scout can
//! write what it likes; it cannot make the deep brain believe a record that
//! does not describe itself consistently.
//!
//! Nothing here names a venue, an order or a trading identity.

use qip_core::error::{Error, Result};
use qip_financial::manifest::SourceManifest;
use qip_market_ingestion::adapter::SensedRecord;
use qip_storage::BlobStore;
use serde::{Deserialize, Serialize};

/// One fetch, as the scout hands it over.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoutRecord {
    source_id: String,
    /// The version-2 manifest of the fetch: hash, locator, lineage, re-fetch.
    manifest: SourceManifest,
    /// What the connector decoded from the bytes. Never the bytes.
    records: Vec<SensedRecord>,
}

/// A source id is a path segment of the key, so it is held to the characters a
/// catalogue id uses. Anything else could climb out of its own prefix.
fn valid_source_id(source_id: &str) -> bool {
    !source_id.is_empty()
        && source_id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

impl ScoutRecord {
    /// Bind a fetch's manifest to what it decoded. Refuses what the reader
    /// would reject, so the scout never writes an object the deep brain
    /// treats as absent.
    pub fn new(
        source_id: impl Into<String>,
        manifest: SourceManifest,
        records: Vec<SensedRecord>,
    ) -> Result<Self> {
        let record = Self {
            source_id: source_id.into(),
            manifest,
            records,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn manifest(&self) -> &SourceManifest {
        &self.manifest
    }

    pub fn records(&self) -> &[SensedRecord] {
        &self.records
    }

    /// The object's name: the source and the digest of the fetch.
    pub fn key(&self) -> String {
        format!("{}/{}.json", self.source_id, self.manifest.sha256())
    }

    fn validate(&self) -> Result<()> {
        if !valid_source_id(&self.source_id) {
            return Err(Error::invalid(format!(
                "scout source id {:?} is not a catalogue id (lowercase letters, digits, - and _); \
                 it names the object's prefix and may not escape it",
                self.source_id
            )));
        }
        if self.manifest.source() != self.source_id {
            return Err(Error::invalid(format!(
                "the manifest names `{}` and the record `{}`; one fetch has one source",
                self.manifest.source(),
                self.source_id
            )));
        }
        if self.manifest.schema_version() != 2 || !self.manifest.records_the_full_descriptor() {
            return Err(Error::invalid(
                "a scout record carries a version-2 manifest, or a re-fetch could not be made \
                 from it",
            ));
        }
        if self.records.is_empty() {
            return Err(Error::invalid(
                "a scout record with no decoded records describes a fetch that yielded nothing",
            ));
        }
        Ok(())
    }
}

/// Write one record, immutably. `Ok(false)` is a fetch already written, left
/// as it was.
pub fn write_scout_record(store: &dyn BlobStore, record: &ScoutRecord) -> Result<bool> {
    let bytes = serde_json::to_vec(record)
        .map_err(|e| Error::invalid(format!("a scout record did not encode: {e}")))?;
    store.put_if_absent(&record.key(), bytes)
}

/// What one scout execution did.
#[derive(Debug, Default, PartialEq)]
pub struct ScoutReport {
    /// Sources whose fetch was written.
    pub written: Vec<String>,
    /// Sources whose fetch was already in the bucket, left as it was.
    pub already_written: Vec<String>,
    /// Sources that yielded nothing this execution.
    pub empty: Vec<String>,
    /// Sources that failed, each with why. One source failing does not stop
    /// the others: they are isolated from each other as from trading.
    pub failed: Vec<(String, String)>,
}

/// One scout execution: poll every arm once, write what each fetched.
pub fn run_once(
    arms: &mut [crate::connectors::ConnectorArm],
    store: &dyn BlobStore,
    until: qip_core::Timestamp,
) -> ScoutReport {
    let mut report = ScoutReport::default();
    for arm in arms {
        let source = arm.source_id().to_string();
        match arm.scout_poll(until) {
            Ok(Some(record)) => match write_scout_record(store, &record) {
                Ok(true) => report.written.push(source),
                Ok(false) => report.already_written.push(source),
                Err(error) => report.failed.push((source, error.message().to_string())),
            },
            Ok(None) => report.empty.push(source),
            Err(error) => report.failed.push((source, error.message().to_string())),
        }
    }
    report
}

/// What the deep brain read from the scout bucket.
#[derive(Debug, Default, PartialEq)]
pub struct ScoutRead {
    /// Records that validated, in key order.
    pub valid: Vec<ScoutRecord>,
    /// Objects treated as absent, each with why.
    pub rejected: Vec<(String, String)>,
}

/// Read every record under `source_id`'s prefix, re-validating each.
pub fn read_scout_records(store: &dyn BlobStore, source_id: &str) -> Result<ScoutRead> {
    if !valid_source_id(source_id) {
        return Err(Error::invalid(format!(
            "{source_id:?} is not a catalogue id; nothing is listed under it"
        )));
    }
    let mut read = ScoutRead::default();
    for key in store.list(&format!("{source_id}/"))? {
        let verdict = (|| -> Result<ScoutRecord> {
            let bytes = store
                .get(&key)?
                .ok_or_else(|| Error::not_found("listed and then gone"))?;
            let record: ScoutRecord = serde_json::from_slice(&bytes)
                .map_err(|e| Error::invalid(format!("does not decode: {e}")))?;
            record.validate()?;
            if record.source_id != source_id || record.key() != key {
                return Err(Error::denied(format!(
                    "the object is named {key} and describes {}; the name and the contents \
                     disagree",
                    record.key()
                )));
            }
            Ok(record)
        })();
        match verdict {
            Ok(record) => read.valid.push(record),
            Err(reason) => read.rejected.push((key, reason.message().to_string())),
        }
    }
    Ok(read)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic_in_result_fn)]

    use super::*;
    use qip_core::{Decimal, ObjectId, Timestamp};
    use qip_market::quote::Tick;
    use qip_storage::MemoryBlobStore;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn tick() -> SensedRecord {
        SensedRecord::Tick(Tick {
            object_id: ObjectId::from_string("S"),
            venue: "TEST".to_string(),
            at: at(),
            capture_time: at(),
            sequence: None,
            price: Decimal::from_int(10),
            volume: Decimal::ZERO,
            quality: Default::default(),
        })
    }

    fn record(source: &str, body: &[u8]) -> Result<ScoutRecord> {
        let manifest = SourceManifest::from_hash(
            source,
            "/v1/rates",
            at(),
            body.len() as u64,
            qip_core::sha256_hex(body),
        )?
        .with_v2(qip_financial::manifest::ManifestV2::new(
            None,
            qip_contracts::governance::Entitlement::Granted {
                dataset: source.to_string(),
                usage: qip_contracts::governance::Usage::Research,
                expires_at: at().saturating_add(qip_core::Duration::from_hours(1)),
            },
            None,
            at().saturating_add(qip_core::Duration::from_hours(1)),
            "scout-test",
            "1",
            qip_financial::manifest::Refetch::Locator,
        )?)?;
        ScoutRecord::new(source, manifest, vec![tick()])
    }

    /// A record the scout wrote reads back, and writing the same fetch again
    /// leaves the first object untouched (`Ok(false)`). Mutation: replace
    /// `put_if_absent` with `put` in `write_scout_record` — the second write
    /// reports `true` and the assertion fails.
    #[test]
    fn a_written_record_reads_back_and_is_never_replaced() -> Result<()> {
        let store = MemoryBlobStore::new();
        let first = record("rates", b"body-1")?;
        assert!(write_scout_record(&store, &first)?);
        assert!(
            !write_scout_record(&store, &first)?,
            "second write is refused"
        );
        let read = read_scout_records(&store, "rates")?;
        assert_eq!(read.valid, vec![first]);
        assert!(read.rejected.is_empty());
        Ok(())
    }

    /// An object the scout (or anyone with the bucket) wrote that does not
    /// describe itself consistently is absent to the deep brain: garbage, a
    /// record filed under another fetch's name, and a record of another
    /// source under this source's prefix. The one honest record beside them is
    /// still read, and the premise is that the store really holds all four.
    /// Mutation: delete the `record.key() != key` comparison in
    /// `read_scout_records` — the misfiled record is read as valid and the
    /// `valid.len()` assertion fails.
    #[test]
    fn an_object_that_does_not_describe_itself_is_absent_and_counted() -> Result<()> {
        let store = MemoryBlobStore::new();
        let good = record("rates", b"body-1")?;
        write_scout_record(&store, &good)?;
        store.put("rates/garbage.json", b"not json".to_vec())?;
        let other = record("rates", b"body-2")?;
        store.put(
            &format!("rates/{}.json", qip_core::sha256_hex(b"somebody-else")),
            serde_json::to_vec(&other).map_err(|e| Error::invalid(e.to_string()))?,
        )?;
        let stranger = record("elsewhere", b"body-3")?;
        store.put(
            &format!("rates/{}.json", stranger.manifest().sha256()),
            serde_json::to_vec(&stranger).map_err(|e| Error::invalid(e.to_string()))?,
        )?;
        assert_eq!(store.list("rates/")?.len(), 4, "premise: four objects");
        let read = read_scout_records(&store, "rates")?;
        assert_eq!(read.valid, vec![good]);
        assert_eq!(read.rejected.len(), 3, "{:?}", read.rejected);
        Ok(())
    }

    /// A source id that could climb out of its prefix, a version-1 manifest
    /// and an empty record are refused where the scout builds them.
    #[test]
    fn the_scout_refuses_to_write_what_the_reader_would_reject() -> Result<()> {
        assert!(record("../trading", b"b").is_err());
        let v1 = SourceManifest::of("rates", "/v1", at(), b"abc")?;
        assert!(ScoutRecord::new("rates", v1, vec![tick()]).is_err());
        let good = record("rates", b"b")?;
        assert!(ScoutRecord::new("rates", good.manifest().clone(), Vec::new()).is_err());
        assert!(read_scout_records(&MemoryBlobStore::new(), "../x").is_err());
        Ok(())
    }
}
