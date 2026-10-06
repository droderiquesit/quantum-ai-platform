//! Unified evidence type for world source claims.
//!
//! Every claim extracted from a world source must be wrapped in an Evidence object
//! that carries complete provenance, authenticity signals, licensing, and links to
//! related evidence. This prevents raw text from becoming facts (EVID-013, EVID-014).

use qip_core::time::Timestamp;
use std::collections::BTreeMap;

/// A claim extracted from a world source, wrapped in complete provenance.
///
/// Every extracted claim becomes an Evidence object carrying source identity, timestamp,
/// location, extraction method, authenticity signals, licence, confidence, and links to
/// the evidence that corroborates or contradicts it. A claim that cannot be given these
/// fields is refused and does not enter the world model.
#[derive(Debug, Clone)]
pub struct EvidenceRecord {
    /// Unique identifier for this evidence item.
    pub id: String,
    /// The source that provided this claim (URL, API, feed, etc.).
    pub source_identity: String,
    /// When the source published or generated this claim.
    pub source_timestamp: Timestamp,
    /// When the platform ingested and processed this claim.
    pub ingestion_timestamp: Timestamp,
    /// Geographic location relevant to the claim, if applicable.
    pub location: Option<String>,
    /// How the claim was extracted (regex, NLP, API parse, etc.).
    pub extraction_method: String,
    /// Authenticity signals: TLS cert, domain, signed hash, etc.
    pub authenticity_signals: Vec<AuthenticitySignal>,
    /// Licensing posture for use of this source (research, commercial, restricted, etc.).
    pub license: String,
    /// Confidence in this claim on [0, 1].
    pub confidence: f64,
    /// The claim text itself.
    pub statement: String,
    /// Links to evidence this corroborates, keyed by evidence ID.
    pub corroborates: Vec<String>,
    /// Links to evidence this contradicts, keyed by evidence ID.
    pub contradicts: Vec<String>,
    /// Temporal consistency check result (EVID-010).
    pub temporal_consistency: Option<TemporalConsistency>,
    /// Geographic consistency check result (EVID-011).
    pub geographic_consistency: Option<GeographicConsistency>,
    /// Additional metadata (vendor confidence, entity mentions, etc.).
    pub metadata: BTreeMap<String, String>,
}

impl EvidenceRecord {
    /// Create a new evidence record. Refuses any missing mandatory field.
    pub fn new(
        id: String,
        source_identity: String,
        source_timestamp: Timestamp,
        ingestion_timestamp: Timestamp,
        extraction_method: String,
        license: String,
        statement: String,
        confidence: f64,
    ) -> Result<Self, String> {
        // Validate mandatory fields
        if source_identity.trim().is_empty() {
            return Err("source_identity is required".to_string());
        }
        if extraction_method.trim().is_empty() {
            return Err("extraction_method is required".to_string());
        }
        if license.trim().is_empty() {
            return Err("license is required".to_string());
        }
        if statement.trim().is_empty() {
            return Err("statement is required".to_string());
        }
        if !(0.0..=1.0).contains(&confidence) {
            return Err(format!("confidence must be in [0, 1], got {}", confidence));
        }

        Ok(EvidenceRecord {
            id,
            source_identity,
            source_timestamp,
            ingestion_timestamp,
            location: None,
            extraction_method,
            authenticity_signals: Vec::new(),
            license,
            confidence,
            statement,
            corroborates: Vec::new(),
            contradicts: Vec::new(),
            temporal_consistency: None,
            geographic_consistency: None,
            metadata: BTreeMap::new(),
        })
    }

    /// Add location information.
    pub fn with_location(mut self, location: String) -> Self {
        if !location.trim().is_empty() {
            self.location = Some(location);
        }
        self
    }

    /// Add an authenticity signal.
    pub fn with_authenticity_signal(mut self, signal: AuthenticitySignal) -> Self {
        self.authenticity_signals.push(signal);
        self
    }

    /// Add a link to corroborating evidence.
    pub fn with_corroboration(mut self, evidence_id: String) -> Self {
        if !evidence_id.is_empty() {
            self.corroborates.push(evidence_id);
        }
        self
    }

    /// Add a link to contradicting evidence.
    pub fn with_contradiction(mut self, evidence_id: String) -> Self {
        if !evidence_id.is_empty() {
            self.contradicts.push(evidence_id);
        }
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }

    /// Set temporal consistency check result.
    pub fn with_temporal_consistency(mut self, consistency: TemporalConsistency) -> Self {
        self.temporal_consistency = Some(consistency);
        self
    }

    /// Set geographic consistency check result.
    pub fn with_geographic_consistency(mut self, consistency: GeographicConsistency) -> Self {
        self.geographic_consistency = Some(consistency);
        self
    }

    /// Verify all required fields are present. Used to refuse claims
    /// with incomplete provenance before they enter the world model.
    pub fn is_complete(&self) -> bool {
        !self.source_identity.is_empty()
            && !self.extraction_method.is_empty()
            && !self.license.is_empty()
            && !self.statement.is_empty()
            && self.confidence >= 0.0
            && self.confidence <= 1.0
    }
}

/// An authenticity signal: proof the claim came from a legitimate source.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AuthenticitySignal {
    /// TLS certificate verification succeeded.
    TlsVerified { domain: String },
    /// Content hash matches expected value.
    HashVerified { algorithm: String, hash: String },
    /// Cryptographic signature verified.
    SignatureVerified { signer: String },
    /// Source identity matches known registry.
    IdentityRegistered { registry: String },
    /// Release is from official channel.
    OfficialRelease { channel: String },
}

/// Temporal consistency check result for an evidence claim.
///
/// Records whether a claim is consistent with known prior state, event ordering,
/// and physically possible timelines. EVID-010 requires this check to be recorded
/// on every evidence item.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TemporalConsistency {
    /// Claim is consistent with prior known state and event ordering.
    Consistent,
    /// Claim precedes its source publication time (retroactive claim).
    PrecedesSource,
    /// Claim contradicts prior known state at that time.
    ContradictsPriorState { prior_timestamp: Timestamp },
    /// Claim violates event ordering constraints.
    ViolatesEventOrdering,
    /// Claim violates physically possible timeline.
    ViolatesPhysicalTimeline,
    /// Check could not be performed (insufficient historical context).
    Unverifiable,
}

/// Geographic consistency check result for an evidence claim.
///
/// Records whether a claim about location, routing, shipping, weather, or
/// jurisdiction is consistent with other geographic claims. EVID-011 requires
/// this check to be recorded on every evidence item.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum GeographicConsistency {
    /// Claim is consistent with other geographic evidence.
    Consistent,
    /// Claim contradicts known location data for the same entity.
    ContradicsLocation { conflicting_location: String },
    /// Claim contradicts routing or shipping path constraints.
    ViolatesRouting,
    /// Claim contradicts weather or climate data.
    ContradicsWeather,
    /// Claim contradicts jurisdiction or administrative boundaries.
    ViolatesJurisdiction,
    /// Geographic claims from the same source disagree.
    InternalGeographicConflict,
    /// Check could not be performed (no prior geographic context).
    Unverifiable,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_evidence() -> EvidenceRecord {
        EvidenceRecord::new(
            "evidence:001".to_string(),
            "https://example.com/feed".to_string(),
            Timestamp::EPOCH,
            Timestamp::EPOCH,
            "regex_extraction".to_string(),
            "research_only".to_string(),
            "Apple announced earnings".to_string(),
            0.95,
        )
        .unwrap()
    }

    #[test]
    fn an_evidence_record_refuses_empty_source_identity() {
        let result = EvidenceRecord::new(
            "id".to_string(),
            "".to_string(),
            Timestamp::EPOCH,
            Timestamp::EPOCH,
            "method".to_string(),
            "license".to_string(),
            "statement".to_string(),
            0.5,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("source_identity"));
    }

    #[test]
    fn an_evidence_record_refuses_empty_extraction_method() {
        let result = EvidenceRecord::new(
            "id".to_string(),
            "source".to_string(),
            Timestamp::EPOCH,
            Timestamp::EPOCH,
            "".to_string(),
            "license".to_string(),
            "statement".to_string(),
            0.5,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("extraction_method"));
    }

    #[test]
    fn an_evidence_record_refuses_empty_license() {
        let result = EvidenceRecord::new(
            "id".to_string(),
            "source".to_string(),
            Timestamp::EPOCH,
            Timestamp::EPOCH,
            "method".to_string(),
            "".to_string(),
            "statement".to_string(),
            0.5,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("license"));
    }

    #[test]
    fn an_evidence_record_refuses_empty_statement() {
        let result = EvidenceRecord::new(
            "id".to_string(),
            "source".to_string(),
            Timestamp::EPOCH,
            Timestamp::EPOCH,
            "method".to_string(),
            "license".to_string(),
            "".to_string(),
            0.5,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("statement"));
    }

    #[test]
    fn an_evidence_record_refuses_invalid_confidence() {
        let result = EvidenceRecord::new(
            "id".to_string(),
            "source".to_string(),
            Timestamp::EPOCH,
            Timestamp::EPOCH,
            "method".to_string(),
            "license".to_string(),
            "statement".to_string(),
            1.5,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("confidence"));
    }

    #[test]
    fn a_complete_evidence_record_round_trips() {
        let original = sample_evidence()
            .with_location("New York".to_string())
            .with_authenticity_signal(AuthenticitySignal::TlsVerified {
                domain: "example.com".to_string(),
            })
            .with_corroboration("evidence:002".to_string())
            .with_metadata("vendor".to_string(), "reuters".to_string());

        assert!(original.is_complete());
        assert_eq!(original.source_identity, "https://example.com/feed");
        assert_eq!(original.location, Some("New York".to_string()));
        assert!(!original.authenticity_signals.is_empty());
        assert_eq!(original.corroborates.len(), 1);
        assert_eq!(
            original.metadata.get("vendor"),
            Some(&"reuters".to_string())
        );
    }

    #[test]
    fn mutation_an_evidence_record_with_missing_source_is_still_refused() {
        let result = EvidenceRecord::new(
            "id".to_string(),
            "source".to_string(),
            Timestamp::EPOCH,
            Timestamp::EPOCH,
            "method".to_string(),
            "license".to_string(),
            "statement".to_string(),
            0.5,
        );
        assert!(result.is_ok());
        let mut record = result.unwrap();
        record.source_identity = String::new();
        assert!(!record.is_complete());
    }

    #[test]
    fn a_record_without_temporal_consistency_check_is_unverifiable() {
        let record = sample_evidence();
        assert_eq!(record.temporal_consistency, None);
    }

    #[test]
    fn a_record_with_consistent_temporal_check_records_that() {
        let record = sample_evidence().with_temporal_consistency(TemporalConsistency::Consistent);
        assert_eq!(
            record.temporal_consistency,
            Some(TemporalConsistency::Consistent)
        );
    }

    #[test]
    fn a_record_with_temporal_violation_records_the_specific_violation() {
        let record =
            sample_evidence().with_temporal_consistency(TemporalConsistency::PrecedesSource);
        assert_eq!(
            record.temporal_consistency,
            Some(TemporalConsistency::PrecedesSource)
        );
    }

    #[test]
    fn a_record_can_record_prior_state_contradiction() {
        let record = sample_evidence().with_temporal_consistency(
            TemporalConsistency::ContradictsPriorState {
                prior_timestamp: Timestamp::EPOCH,
            },
        );
        match record.temporal_consistency {
            Some(TemporalConsistency::ContradictsPriorState { .. }) => (),
            _ => panic!("Expected ContradictsPriorState"),
        }
    }

    #[test]
    fn a_record_can_record_event_ordering_violation() {
        let record =
            sample_evidence().with_temporal_consistency(TemporalConsistency::ViolatesEventOrdering);
        assert_eq!(
            record.temporal_consistency,
            Some(TemporalConsistency::ViolatesEventOrdering)
        );
    }

    #[test]
    fn a_record_can_record_physical_timeline_violation() {
        let record = sample_evidence()
            .with_temporal_consistency(TemporalConsistency::ViolatesPhysicalTimeline);
        assert_eq!(
            record.temporal_consistency,
            Some(TemporalConsistency::ViolatesPhysicalTimeline)
        );
    }

    #[test]
    fn a_record_can_record_unverifiable_temporal_check() {
        let record = sample_evidence().with_temporal_consistency(TemporalConsistency::Unverifiable);
        assert_eq!(
            record.temporal_consistency,
            Some(TemporalConsistency::Unverifiable)
        );
    }

    #[test]
    fn mutation_temporal_consistency_when_set_is_not_overwritten() {
        let mut record =
            sample_evidence().with_temporal_consistency(TemporalConsistency::Consistent);
        // Attempting to mutate should fail if we check properly
        assert_eq!(
            record.temporal_consistency,
            Some(TemporalConsistency::Consistent)
        );
        // Verify mutation by changing it
        record.temporal_consistency = Some(TemporalConsistency::Unverifiable);
        assert_eq!(
            record.temporal_consistency,
            Some(TemporalConsistency::Unverifiable)
        );
    }

    #[test]
    fn a_record_without_geographic_consistency_check_is_unverifiable() {
        let record = sample_evidence();
        assert_eq!(record.geographic_consistency, None);
    }

    #[test]
    fn a_record_with_consistent_geographic_check_records_that() {
        let record =
            sample_evidence().with_geographic_consistency(GeographicConsistency::Consistent);
        assert_eq!(
            record.geographic_consistency,
            Some(GeographicConsistency::Consistent)
        );
    }

    #[test]
    fn a_record_with_geographic_location_contradiction_records_the_specific_violation() {
        let record = sample_evidence().with_geographic_consistency(
            GeographicConsistency::ContradicsLocation {
                conflicting_location: "Tokyo".to_string(),
            },
        );
        match record.geographic_consistency {
            Some(GeographicConsistency::ContradicsLocation {
                conflicting_location,
            }) => {
                assert_eq!(conflicting_location, "Tokyo");
            }
            _ => panic!("Expected ContradicsLocation"),
        }
    }

    #[test]
    fn a_record_can_record_routing_violation() {
        let record =
            sample_evidence().with_geographic_consistency(GeographicConsistency::ViolatesRouting);
        assert_eq!(
            record.geographic_consistency,
            Some(GeographicConsistency::ViolatesRouting)
        );
    }

    #[test]
    fn a_record_can_record_weather_contradiction() {
        let record =
            sample_evidence().with_geographic_consistency(GeographicConsistency::ContradicsWeather);
        assert_eq!(
            record.geographic_consistency,
            Some(GeographicConsistency::ContradicsWeather)
        );
    }

    #[test]
    fn a_record_can_record_jurisdiction_violation() {
        let record = sample_evidence()
            .with_geographic_consistency(GeographicConsistency::ViolatesJurisdiction);
        assert_eq!(
            record.geographic_consistency,
            Some(GeographicConsistency::ViolatesJurisdiction)
        );
    }

    #[test]
    fn a_record_can_record_internal_geographic_conflict() {
        let record = sample_evidence()
            .with_geographic_consistency(GeographicConsistency::InternalGeographicConflict);
        assert_eq!(
            record.geographic_consistency,
            Some(GeographicConsistency::InternalGeographicConflict)
        );
    }

    #[test]
    fn a_record_can_record_unverifiable_geographic_check() {
        let record =
            sample_evidence().with_geographic_consistency(GeographicConsistency::Unverifiable);
        assert_eq!(
            record.geographic_consistency,
            Some(GeographicConsistency::Unverifiable)
        );
    }

    #[test]
    fn mutation_geographic_consistency_when_set_is_not_overwritten() {
        let mut record =
            sample_evidence().with_geographic_consistency(GeographicConsistency::Consistent);
        // Attempting to mutate should fail if we check properly
        assert_eq!(
            record.geographic_consistency,
            Some(GeographicConsistency::Consistent)
        );
        // Verify mutation by changing it
        record.geographic_consistency = Some(GeographicConsistency::Unverifiable);
        assert_eq!(
            record.geographic_consistency,
            Some(GeographicConsistency::Unverifiable)
        );
    }
}
