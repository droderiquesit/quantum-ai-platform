//! Unified evidence type for world source claims.
//!
//! Every claim extracted from a world source must be wrapped in an Evidence object
//! that carries complete provenance, authenticity signals, licensing, and links to
//! related evidence. This prevents raw text from becoming facts (EVID-013, EVID-014).

use qip_core::error::{Error, Result};
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
    /// When the claim says the event it describes happened, if it says.
    /// Distinct from `source_timestamp`: a report published at noon may
    /// describe a halt at nine.
    pub claimed_at: Option<Timestamp>,
    /// Evidence IDs of events this claim's event depends on, which must
    /// therefore have happened no later than it (EVID-010 event ordering).
    pub caused_by: Vec<String>,
    /// Temporal consistency check result (EVID-010), written by
    /// [`EvidenceRecord::check_temporal_consistency`].
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
    ) -> Result<Self> {
        // Validate mandatory fields
        if source_identity.trim().is_empty() {
            return Err(Error::invalid(
                "source_identity is required; a claim without it has no provenance",
            ));
        }
        if extraction_method.trim().is_empty() {
            return Err(Error::invalid(
                "extraction_method is required; a claim without it has no provenance",
            ));
        }
        if license.trim().is_empty() {
            return Err(Error::invalid(
                "license is required; a claim without it has no provenance",
            ));
        }
        if statement.trim().is_empty() {
            return Err(Error::invalid(
                "statement is required; a claim without it has no provenance",
            ));
        }
        if !(0.0..=1.0).contains(&confidence) {
            return Err(Error::invalid(format!(
                "confidence must be in [0, 1], got {confidence}"
            )));
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
            claimed_at: None,
            caused_by: Vec::new(),
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

    /// Record when the claim says its event happened.
    pub fn with_claimed_at(mut self, at: Timestamp) -> Self {
        self.claimed_at = Some(at);
        self
    }

    /// Record that this claim's event depends on the event in `evidence_id`.
    pub fn with_cause(mut self, evidence_id: String) -> Self {
        if !evidence_id.is_empty() {
            self.caused_by.push(evidence_id);
        }
        self
    }

    /// Check this claim against a physically possible timeline, the ordering
    /// of the events it depends on, and the state already known when it
    /// arrived (EVID-010), and return the first conflict with the fact it
    /// conflicts with.
    ///
    /// Before this existed the evidence type had a slot for the outcome and
    /// nothing that computed one, so every record carried `None` and a claim
    /// dated after its own publication read the same as a sound one.
    ///
    /// `prior` is the evidence already held; a cause or contradiction link
    /// naming an ID not in it cannot be judged, and the result says
    /// `Unverifiable` rather than `Consistent`, because a check that could
    /// not run is not a check that passed.
    pub fn check_temporal_consistency(&self, prior: &[EvidenceRecord]) -> TemporalConsistency {
        // Ingested before it was published: no possible timeline.
        if self.ingestion_timestamp < self.source_timestamp {
            return TemporalConsistency::ViolatesPhysicalTimeline {
                published_at: self.source_timestamp,
                ingested_at: self.ingestion_timestamp,
            };
        }
        // A source reporting an event dated after its own publication.
        if let Some(claimed_at) = self.claimed_at
            && claimed_at > self.source_timestamp
        {
            return TemporalConsistency::PostdatesSource {
                claimed_at,
                published_at: self.source_timestamp,
            };
        }
        let find = |id: &str| prior.iter().find(|p| p.id == id);
        let mut unverifiable = false;
        for cause_id in &self.caused_by {
            let (Some(cause), Some(effect_at)) = (find(cause_id), self.claimed_at) else {
                unverifiable = true;
                continue;
            };
            let cause_at = cause.claimed_at.unwrap_or(cause.source_timestamp);
            if cause_at > effect_at {
                return TemporalConsistency::ViolatesEventOrdering {
                    cause_id: cause_id.clone(),
                    cause_at,
                };
            }
        }
        for prior_id in &self.contradicts {
            let Some(known) = find(prior_id) else {
                unverifiable = true;
                continue;
            };
            if known.ingestion_timestamp <= self.ingestion_timestamp {
                return TemporalConsistency::ContradictsPriorState {
                    prior_id: prior_id.clone(),
                    prior_timestamp: known.ingestion_timestamp,
                };
            }
        }
        if unverifiable {
            TemporalConsistency::Unverifiable
        } else {
            TemporalConsistency::Consistent
        }
    }

    /// Run [`Self::check_temporal_consistency`] and record its outcome on
    /// the record, replacing any earlier outcome.
    pub fn checked(mut self, prior: &[EvidenceRecord]) -> Self {
        self.temporal_consistency = Some(self.check_temporal_consistency(prior));
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
        !self.source_identity.trim().is_empty()
            && !self.extraction_method.trim().is_empty()
            && !self.license.trim().is_empty()
            && !self.statement.trim().is_empty()
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
/// and physically possible timelines, and on failure names the conflicting fact.
/// EVID-010 requires this check to be recorded on every evidence item.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TemporalConsistency {
    /// Claim is consistent with prior known state and event ordering.
    Consistent,
    /// The claimed event is dated after the source that reports it was published.
    PostdatesSource {
        claimed_at: Timestamp,
        published_at: Timestamp,
    },
    /// Claim contradicts evidence already known when it arrived.
    ContradictsPriorState {
        prior_id: String,
        prior_timestamp: Timestamp,
    },
    /// The claimed event precedes an event it names as its cause.
    ViolatesEventOrdering {
        cause_id: String,
        cause_at: Timestamp,
    },
    /// The claim was ingested before its source published it.
    ViolatesPhysicalTimeline {
        published_at: Timestamp,
        ingested_at: Timestamp,
    },
    /// Check could not be performed (a linked item is not held, or the claim
    /// has a cause but no date of its own).
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

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    fn record(id: &str, published: i64, ingested: i64) -> EvidenceRecord {
        EvidenceRecord::new(
            id.to_string(),
            "https://example.com/feed".to_string(),
            at(published),
            at(ingested),
            "regex_extraction".to_string(),
            "research_only".to_string(),
            "Apple announced earnings".to_string(),
            0.95,
        )
        .unwrap()
    }

    fn sample_evidence() -> EvidenceRecord {
        record("evidence:001", 100, 200)
    }

    fn refusal(source: &str, method: &str, license: &str, statement: &str, c: f64) -> String {
        EvidenceRecord::new(
            "id".to_string(),
            source.to_string(),
            Timestamp::EPOCH,
            Timestamp::EPOCH,
            method.to_string(),
            license.to_string(),
            statement.to_string(),
            c,
        )
        .unwrap_err()
        .to_string()
    }

    #[test]
    fn an_evidence_record_refuses_empty_source_identity() {
        assert!(refusal("", "m", "l", "s", 0.5).contains("source_identity is required"));
    }

    #[test]
    fn an_evidence_record_refuses_empty_extraction_method() {
        assert!(refusal("src", " ", "l", "s", 0.5).contains("extraction_method is required"));
    }

    #[test]
    fn an_evidence_record_refuses_empty_license() {
        assert!(refusal("src", "m", "", "s", 0.5).contains("license is required"));
    }

    #[test]
    fn an_evidence_record_refuses_empty_statement() {
        assert!(refusal("src", "m", "l", "", 0.5).contains("statement is required"));
    }

    #[test]
    fn an_evidence_record_refuses_invalid_confidence() {
        assert!(refusal("src", "m", "l", "s", 1.5).contains("confidence must be in [0, 1]"));
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
    fn a_record_whose_source_is_blanked_after_construction_is_incomplete() {
        let mut record = sample_evidence();
        assert!(record.is_complete());
        record.source_identity = "   ".to_string();
        assert!(!record.is_complete());
    }

    #[test]
    fn a_record_without_temporal_consistency_check_is_unverifiable() {
        let record = sample_evidence();
        assert_eq!(record.temporal_consistency, None);
    }

    // EVID-010: the check itself. Each violation names the conflicting fact.

    #[test]
    fn a_consistent_claim_passes_the_temporal_check_unflagged() {
        let cause = record("cause", 10, 20).with_claimed_at(at(5));
        let older = record("older", 10, 300);
        let claim = record("claim", 100, 200)
            .with_claimed_at(at(50))
            .with_cause("cause".to_string())
            .with_contradiction("older".to_string())
            .checked(&[cause, older]);
        // `older` contradicts the claim but arrived after it, so it was not
        // prior state when the claim was ingested.
        assert_eq!(
            claim.temporal_consistency,
            Some(TemporalConsistency::Consistent)
        );
    }

    #[test]
    fn a_claim_ingested_before_its_source_published_violates_the_physical_timeline() {
        let claim = record("claim", 200, 100).checked(&[]);
        assert_eq!(
            claim.temporal_consistency,
            Some(TemporalConsistency::ViolatesPhysicalTimeline {
                published_at: at(200),
                ingested_at: at(100),
            })
        );
    }

    #[test]
    fn a_claim_dated_after_its_source_was_published_is_flagged() {
        let claim = record("claim", 100, 200).with_claimed_at(at(150));
        assert_eq!(
            claim.check_temporal_consistency(&[]),
            TemporalConsistency::PostdatesSource {
                claimed_at: at(150),
                published_at: at(100),
            }
        );
        // The same claim dated at its publication instant is sound.
        let sound = record("claim", 100, 200).with_claimed_at(at(100));
        assert_eq!(
            sound.check_temporal_consistency(&[]),
            TemporalConsistency::Consistent
        );
    }

    #[test]
    fn a_claim_that_precedes_its_cause_names_the_cause() {
        let cause = record("cause", 90, 95).with_claimed_at(at(80));
        let claim = record("claim", 100, 200)
            .with_claimed_at(at(60))
            .with_cause("cause".to_string());
        assert_eq!(
            claim.check_temporal_consistency(std::slice::from_ref(&cause)),
            TemporalConsistency::ViolatesEventOrdering {
                cause_id: "cause".to_string(),
                cause_at: at(80),
            }
        );
    }

    #[test]
    fn a_claim_contradicting_state_known_before_it_arrived_names_that_state() {
        let known = record("known", 10, 150);
        let claim = record("claim", 100, 200).with_contradiction("known".to_string());
        assert_eq!(
            claim.check_temporal_consistency(std::slice::from_ref(&known)),
            TemporalConsistency::ContradictsPriorState {
                prior_id: "known".to_string(),
                prior_timestamp: at(150),
            }
        );
    }

    #[test]
    fn a_link_to_evidence_not_held_is_unverifiable_rather_than_consistent() {
        let claim = record("claim", 100, 200)
            .with_claimed_at(at(50))
            .with_cause("missing".to_string());
        assert_eq!(
            claim.check_temporal_consistency(&[]),
            TemporalConsistency::Unverifiable
        );
        let undated = record("claim", 100, 200).with_cause("cause".to_string());
        let cause = record("cause", 10, 20);
        assert_eq!(
            undated.check_temporal_consistency(&[cause]),
            TemporalConsistency::Unverifiable
        );
    }

    #[test]
    fn a_recheck_replaces_an_earlier_temporal_result() {
        let claim = record("claim", 200, 100)
            .with_temporal_consistency(TemporalConsistency::Consistent)
            .checked(&[]);
        assert!(matches!(
            claim.temporal_consistency,
            Some(TemporalConsistency::ViolatesPhysicalTimeline { .. })
        ));
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
