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
}
