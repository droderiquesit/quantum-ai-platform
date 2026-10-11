//! The evidence-check stage of acquisition (R12-DATA-2012, R12-EVID-2031).
//!
//! §7's acquisition order is fetch → parse → **evidence check** → fact →
//! knowledge delta → world model. Until 2026-10-11 the connector runtime
//! went straight from parse to the record: an admitted event became a
//! [`SensedRecord`] with no evidence object, nothing checked its timeline,
//! weighed its authenticity or linked it to what other sources said, and
//! `qip_world_model::EvidenceRecord` was constructed nowhere outside its own
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
use qip_world_model::evidence::{META_ATTRIBUTE, META_SUBJECT, META_VALUE};
use qip_world_model::{AuthenticitySignal, EvidenceRecord, EvidenceVerdict};
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
    .with_claimed_at(record.occurred_at())
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
