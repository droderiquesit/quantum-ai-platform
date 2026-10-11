//! The evidence-check stage of acquisition (R12-DATA-2012, R12-EVID-2031).
//!
//! §7's acquisition order is fetch → parse → **evidence check** → fact →
//! knowledge delta → world model. Until 2026-10-11 the connector runtime
//! went straight from parse to the record: an admitted event became a
//! [`SensedRecord`] with no evidence object, nothing checked its timeline,
//! weighed its authenticity or linked it to what other sources said, and
//! `qip_evidence::EvidenceRecord` was constructed nowhere outside its own
//! tests.
//!
//! [`ConnectorRuntime::ingest`](super::runtime::ConnectorRuntime) now builds
//! one [`EvidenceRecord`] per event that passed the per-event gates, with
//! the parser lineage that extracted it ([`ParserLineage`]), the licence it
//! was fetched under, the knowable instant as its publication and the event
//! instant as what it claims, and a hash signal naming the body it came
//! from. The record goes through an [`EvidenceFabric`] — the temporal,
//! geographic, authenticity and contradiction checks and the provenance
//! graph — and the verdict travels out on the poll report beside the record.
//! A claim on a physically impossible timeline is quarantined rather than
//! released.
//!
//! # Statements are per source
//!
//! A structured market record's statement names the source that served it.
//! Without that, three venues reporting one price at one instant would share
//! a normalised statement, the fabric would read them as one origin and
//! never let them corroborate each other, and the poisoning detector would
//! read three honest venues as a coordinated campaign. Text — a news
//! headline — is the case where identical wording across sources *is* the
//! signal, so a news statement is its headline alone.

use super::manifest::{SchemaVersion, SourceManifest};
use crate::adapter::SensedRecord;
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_evidence::evidence::{META_ATTRIBUTE, META_SUBJECT, META_VALUE};
use qip_evidence::{AuthenticitySignal, EvidenceRecord, EvidenceVerdict};
use serde::{Deserialize, Serialize};

/// Which parser extracted a record, at which version, against which schema
/// (R12-DATA-1034).
///
/// A re-fetch that hashes the same and parses differently is a change in
/// this platform, not in the source; without the parser's identity on the
/// record the two are indistinguishable after the fact.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ParserLineage {
    /// The connector's decoder, named by the source it decodes.
    pub parser: String,
    /// The version of the code that ran: this crate's, fixed at build.
    pub parser_version: String,
    /// The schema version the payload was admitted against.
    pub schema_version: SchemaVersion,
}

impl ParserLineage {
    /// The lineage of the shipped decoder for `manifest`'s source.
    pub fn of(manifest: &SourceManifest) -> Self {
        Self {
            parser: format!("qip-market-ingestion::connector::{}", manifest.source_id),
            parser_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: manifest.schema.version,
        }
    }

    /// One token for an extraction-method field.
    pub fn describe(&self) -> String {
        format!(
            "{}@{} schema {}.{}",
            self.parser,
            self.parser_version,
            self.schema_version.major(),
            self.schema_version.minor()
        )
    }
}

/// An admitted record's evidence, as checked.
#[derive(Clone, Debug, Serialize)]
pub struct CheckedEvidence {
    pub record: EvidenceRecord,
    /// SHA-256 of the body the record was extracted from.
    pub document_hash: String,
    pub verdict: EvidenceVerdict,
}

/// What a record asserts as an attribute and a value in canonical text, or
/// `None` for a record whose claim is not a single comparable value. A record
/// without one is still evidence, and is simply never in a contest.
///
/// A venue price is keyed *by venue*: two exchanges printing different
/// prices for one instrument at one instant are two facts, not a
/// contradiction, and keying them together would put every cross-venue
/// spread into a contest. A macro series has one true value per reference
/// date, so it is keyed by the series alone.
pub fn claimed_value(record: &SensedRecord) -> Option<(String, String)> {
    match record {
        SensedRecord::Tick(tick) => Some((format!("tick@{}", tick.venue), tick.price.to_string())),
        SensedRecord::Trade(trade) => {
            Some((format!("trade@{}", trade.venue), trade.price.to_string()))
        }
        SensedRecord::Quote(quote) => Some((
            format!("quote@{}", quote.venue),
            format!("{}/{}", quote.bid, quote.ask),
        )),
        SensedRecord::Bar(bar) => Some((format!("close@{}", bar.venue), bar.close.to_string())),
        SensedRecord::Macro(observation) => {
            Some(("level".to_string(), observation.value.to_string()))
        }
        _ => None,
    }
}

/// The instant a record's claim is dated at, for the temporal check.
///
/// For every record but a bar this is [`SensedRecord::occurred_at`]. A bar's
/// `occurred_at` is the *end* of its bucket, and a daily bucket is a
/// calendar day: an IEX daily bar opens at the session's midnight and its
/// bucket ends at the next midnight, seven hours after the session closed
/// and the source published it. Dating the claim at the bucket's end made
/// every daily bar "report an event after its own publication", so the
/// fabric ruled each one `PostdatesSource` and the runtime quarantined every
/// bar Alpaca served — four quarantined, none admitted. A bar is a claim
/// about the bucket that begins at `open_time`, which is also the instant
/// the payload stamps it with; a bar whose bucket *opens* after its
/// publication is the genuinely impossible one, and that still fails.
/// Releasing a bar before its bucket has run is the knowability gate's
/// refusal (the manifest's publication delay), not the evidence check's.
pub fn claimed_at(record: &SensedRecord) -> Timestamp {
    match record {
        SensedRecord::Bar(bar) => bar.open_time,
        other => other.occurred_at(),
    }
}

/// Build the evidence record for one admitted event.
///
/// `published_at` is the instant the record became knowable under the
/// manifest's publication delay — what the source made available, and when
/// — and `claimed_at` the instant the record says the fact was true.
#[allow(clippy::too_many_arguments)]
pub fn evidence_of(
    manifest: &SourceManifest,
    lineage: &ParserLineage,
    fingerprint: &str,
    upstream_key: &str,
    record: &SensedRecord,
    published_at: Timestamp,
    ingested_at: Timestamp,
    document_hash: &str,
    confidence: f64,
) -> Result<EvidenceRecord> {
    let topic = record.topic();
    let subject = record.subject_id().map(str::to_string);
    let value = claimed_value(record);
    let statement = match record {
        SensedRecord::News(item) => item.headline.clone(),
        _ => format!(
            "{} reports {:?} for {} at {}{}",
            manifest.source_id,
            topic,
            subject.as_deref().unwrap_or(upstream_key),
            record.occurred_at().to_rfc3339(),
            value
                .as_ref()
                .map(|(_, v)| format!(": {v}"))
                .unwrap_or_default()
        ),
    };
    let mut evidence = EvidenceRecord::new(
        fingerprint.to_string(),
        manifest.source_id.clone(),
        published_at,
        ingested_at,
        lineage.describe(),
        format!("{:?}", manifest.licensing).to_ascii_lowercase(),
        statement,
        // Refused, not clamped, by the record's constructor when outside
        // `[0, 1]`: a quality score out of range is a connector bug.
        confidence,
    )?
    .with_claimed_at(claimed_at(record))
    .with_location(format!("region:{:?}", manifest.region))
    .with_authenticity_signal(AuthenticitySignal::HashVerified {
        algorithm: "sha256".into(),
        hash: document_hash.to_string(),
    })
    .with_metadata("provider".into(), manifest.provider.clone())
    .with_metadata("upstream_key".into(), upstream_key.to_string());
    if let Some(subject) = subject {
        evidence = evidence.with_metadata(META_SUBJECT.into(), subject);
        if let Some((attribute, value)) = value {
            evidence = evidence
                .with_metadata(META_ATTRIBUTE.into(), attribute)
                .with_metadata(META_VALUE.into(), value);
        }
    }
    Ok(evidence)
}

/// A central tick, quote or trade shaped for the Tick Lake, or `None` for a
/// record the lake's market class does not hold.
///
/// A central connector runs no clock discipline, so no correction is
/// applied and the normalized time is the venue's own. The uncertainty is
/// the one bound a capture without a clock estimate can state: the event
/// happened no later than it was received, so the receive lag is the width
/// of the interval it lies in. Zero would claim a measurement nobody made.
pub fn captured_of(
    record: &SensedRecord,
    source_id: &str,
    entitlement: &str,
) -> Result<Option<qip_storage::tick_capture::CapturedMarketEvent>> {
    let (venue, instrument, sequence, at, capture) = match record {
        SensedRecord::Tick(t) => (
            &t.venue,
            t.object_id.as_str(),
            t.sequence,
            t.at,
            t.capture_time,
        ),
        SensedRecord::Quote(q) => (
            &q.venue,
            q.object_id.as_str(),
            q.sequence,
            q.at,
            q.capture_time,
        ),
        SensedRecord::Trade(t) => (
            &t.venue,
            t.object_id.as_str(),
            t.sequence,
            t.at,
            t.capture_time,
        ),
        _ => return Ok(None),
    };
    let captured = qip_storage::tick_capture::CapturedMarketEvent {
        venue: venue.clone(),
        feed: source_id.to_string(),
        instrument: instrument.to_string(),
        sequence,
        event_time: at,
        receive_time: capture,
        normalized_time: at,
        uncertainty: capture.since(at).max(qip_core::Duration::ZERO),
        corrections: Vec::new(),
        arrival_order: None,
        entitlement: entitlement.to_string(),
        canonical: serde_json::to_value(record)?,
        raw_sha256: None,
        raw: None,
    };
    captured.validate()?;
    Ok(Some(captured))
}
