//! The canonical market-event envelope.
//!
//! Every connector's output converges here, so that everything downstream sees
//! one shape whatever the source spoke. The envelope is what carries the four
//! things a record cannot be trusted without: which source produced it, under
//! which schema, what its fingerprint is, and — separately — the three
//! instants it lives between.
//!
//! # Event time, ingest time and knowable time are three different instants
//!
//! Conflating them is the cheapest way to write a backtest that reads the
//! future, so they are three fields:
//!
//! * **event time** — when the fact was true in the world. Taken from the
//!   payload, never from a clock. This is what a time series is indexed by.
//! * **ingest time** — when this platform learned it. The caller's `until`
//!   rather than the wall clock, so the same fetch replayed produces the same
//!   envelope it produced live.
//! * **knowable time** — event time plus the source's dissemination delay: the
//!   earliest instant a decision could have used this. A record whose knowable
//!   time is after the poll's horizon is withheld, not published early — by
//!   the runtime's per-event gate, before an envelope is built; the envelope
//!   carries the instant as a record of the fact, not as a second gate.
//!
//! A source with a fifteen-minute delay produces records whose event time is
//! in the past and whose knowable time is not yet reached. A backtest that
//! filtered on event time would trade on them fifteen minutes before the
//! deployment was entitled to see them, and nothing downstream could tell.
//!
//! # Provenance and data quality travel with the record, not beside it
//!
//! `qip_financial::quality::DataQuality::default` asserts a *perfect*
//! measurement, which clears the decision floor. So an envelope is never built
//! without stating quality: [`MarketEventEnvelope::new`] takes it, and a
//! connector that observed a gap says so there rather than letting the default
//! assert it measured everything.

use super::dedup::EventFingerprint;
use super::manifest::{SchemaVersion, SourceManifest};
use crate::adapter::SensedRecord;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_financial::quality::{DataQuality, LicensingClass, Provenance};
use serde::{Deserialize, Serialize};

/// One event as the source stated it, before it becomes a platform record.
///
/// The connector's `decode` produces these; the runtime fingerprints, checks
/// and maps them. Keeping this step separate is what lets deduplication run on
/// the source's own bytes rather than on a mapped record whose fields this
/// code chose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawEvent {
    /// The source's identity for this record — a trade id, a series code, a
    /// currency pair. What a reconciliation against the provider joins on.
    pub key: String,
    /// When the fact was true in the world, as the source said it.
    pub event_time: Timestamp,
    /// The event's payload, as the source sent it.
    pub body: serde_json::Value,
}

impl RawEvent {
    pub fn new(key: impl Into<String>, event_time: Timestamp, body: serde_json::Value) -> Self {
        Self {
            key: key.into(),
            event_time,
            body,
        }
    }

    /// This event's fingerprint under a manifest.
    pub fn fingerprint(&self, manifest: &SourceManifest) -> EventFingerprint {
        EventFingerprint::of(
            &manifest.source_id,
            &manifest.schema.version.to_string(),
            &self.key,
            self.event_time,
            &self.body,
        )
    }
}

/// A record, plus everything needed to decide whether to believe it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketEventEnvelope {
    source_id: String,
    schema_version: SchemaVersion,
    fingerprint: EventFingerprint,
    /// The source's own key, kept so a reconciliation has something to join on.
    upstream_key: String,
    record: SensedRecord,
    event_time: Timestamp,
    ingest_time: Timestamp,
    knowable_at: Timestamp,
    provenance: Provenance,
    quality: DataQuality,
}

impl MarketEventEnvelope {
    /// Wrap a mapped record.
    ///
    /// `ingest_time` is the caller's horizon, not a clock read. Fails when it
    /// precedes the event time: a record the platform learned before it
    /// happened is a decoder reading the wrong field, and admitting it would
    /// put a negative ingestion latency into every downstream metric.
    pub fn new(
        manifest: &SourceManifest,
        raw: &RawEvent,
        record: SensedRecord,
        ingest_time: Timestamp,
        quality: DataQuality,
    ) -> Result<Self> {
        if ingest_time < raw.event_time {
            return Err(Error::invalid(format!(
                "`{}` produced an event at {} that this platform is said to have ingested at {}, \
                 which is earlier. A record ingested before it occurred is a decoder reading the \
                 wrong field, and it would put a negative latency into every downstream metric",
                manifest.source_id, raw.event_time, ingest_time
            )));
        }
        let knowable_at = raw.event_time.saturating_add(manifest.publication_delay());
        let provenance = Provenance::new(manifest.source_id.clone(), raw.event_time, ingest_time)
            .with_licensing(manifest.licensing)
            .with_upstream_id(raw.key.clone());
        Ok(Self {
            source_id: manifest.source_id.clone(),
            schema_version: manifest.schema.version,
            fingerprint: raw.fingerprint(manifest),
            upstream_key: raw.key.clone(),
            record,
            event_time: raw.event_time,
            ingest_time,
            knowable_at,
            provenance,
            quality,
        })
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub const fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    pub const fn fingerprint(&self) -> &EventFingerprint {
        &self.fingerprint
    }

    pub fn upstream_key(&self) -> &str {
        &self.upstream_key
    }

    pub const fn record(&self) -> &SensedRecord {
        &self.record
    }

    pub fn into_record(self) -> SensedRecord {
        self.record
    }

    /// When the fact was true in the world.
    pub const fn event_time(&self) -> Timestamp {
        self.event_time
    }

    /// When this platform learned it.
    pub const fn ingest_time(&self) -> Timestamp {
        self.ingest_time
    }

    /// The earliest instant a decision could have used this.
    ///
    /// Recorded here, gated elsewhere: `ConnectorRuntime::admit` withholds a
    /// raw event whose knowable instant is past the poll's horizon before any
    /// envelope exists, so every envelope a poll admits is already knowable at
    /// its own `ingest_time`. There is no `is_knowable_at` on the envelope on
    /// purpose. One existed, nothing consulted it, and at the only handover —
    /// `ConnectorFeed::poll`, which strips the envelope at the same horizon it
    /// polled at — it could never have refused anything.
    pub const fn knowable_at(&self) -> Timestamp {
        self.knowable_at
    }

    /// How long the platform took to learn a fact that was already true.
    pub fn ingestion_lag(&self) -> qip_core::Duration {
        self.ingest_time.since(self.event_time)
    }

    pub const fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub const fn quality(&self) -> &DataQuality {
        &self.quality
    }

    pub const fn licensing(&self) -> LicensingClass {
        self.provenance.licensing
    }

    /// Whether this record may drive a real capital decision — both the
    /// licence and the measured quality have to permit it.
    pub fn is_decision_grade(&self) -> bool {
        self.provenance.licensing.allows_production_decisions()
            && self
                .quality
                .meets(qip_financial::quality::DECISION_QUALITY_FLOOR)
    }
}
