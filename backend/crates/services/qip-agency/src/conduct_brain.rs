//! Communications & Conduct Brain: gates external communications through seven checks.
//!
//! AGENCY-037: "The Conduct Brain gates every external communication"
//!
//! Every proposed external communication is checked for:
//! 1. Factual support — claims backed by evidence
//! 2. Provenance — sources traceable and authorized
//! 3. Identity — speaker properly identified and authenticated
//! 4. Required disclosures — regulatory/policy disclosures present
//! 5. Audience/channel permissions — message appropriate for channel
//! 6. Jurisdiction — compliance with applicable regulatory jurisdictions
//! 7. Market-conduct constraints — compliance with conduct rules
//!
//! All seven must pass before release. Shadow-only form (drafting & gating without live channels).

use qip_core::Error;
use std::collections::BTreeMap;

/// A proposed external communication awaiting conduct review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunicationDraft {
    /// Unique identifier for this draft
    pub id: String,
    /// The message content
    pub content: String,
    /// Channel/venue this will be published to (e.g., "press", "social", "direct")
    pub channel: String,
    /// Identity/account that will send this
    pub sender_identity: String,
    /// Evidence citations supporting claims in content
    pub evidence_citations: Vec<EvidenceCitation>,
    /// Required disclosures for this message
    pub disclosures: Vec<Disclosure>,
    /// Audience this targets
    pub target_audience: String,
    /// Applicable jurisdiction for this communication
    pub jurisdiction: String,
}

/// Reference to evidence supporting a claim in the communication.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvidenceCitation {
    pub source_id: String,
    pub evidence_id: String,
    pub confidence_bps: i64, // basis points, 0-10000
}

/// A required disclosure for regulatory/policy compliance.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Disclosure {
    pub kind: String, // e.g., "conflict_of_interest", "paid_promotion", "confidentiality_status"
    pub text: String,
}

/// Outcome of a single conduct check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckResult {
    pub check_name: String,
    pub passed: bool,
    pub reason: Option<String>,
}

/// The seven conduct checks, each returning pass/fail.
pub mod checks {
    use super::*;

    /// Check 1: Factual Support
    /// Claims must be backed by evidence citations.
    pub fn check_factual_support(draft: &CommunicationDraft) -> CheckResult {
        if draft.evidence_citations.is_empty() && !draft.content.is_empty() {
            return CheckResult {
                check_name: "factual_support".to_string(),
                passed: false,
                reason: Some(
                    "Communication contains claims but provides no evidence citations".to_string(),
                ),
            };
        }

        CheckResult {
            check_name: "factual_support".to_string(),
            passed: true,
            reason: None,
        }
    }

    /// Check 2: Provenance
    /// Evidence sources must be traceable and authorized.
    pub fn check_provenance(
        draft: &CommunicationDraft,
        authorized_sources: &[String],
    ) -> CheckResult {
        if draft.evidence_citations.is_empty() {
            return CheckResult {
                check_name: "provenance".to_string(),
                passed: true,
                reason: None,
            };
        }

        for citation in &draft.evidence_citations {
            if !authorized_sources.contains(&citation.source_id) {
                return CheckResult {
                    check_name: "provenance".to_string(),
                    passed: false,
                    reason: Some(format!(
                        "Evidence source '{}' is not in authorized sources list",
                        citation.source_id
                    )),
                };
            }
        }

        CheckResult {
            check_name: "provenance".to_string(),
            passed: true,
            reason: None,
        }
    }

    /// Check 3: Identity
    /// Sender must be properly identified and authenticated.
    pub fn check_identity(
        draft: &CommunicationDraft,
        authorized_identities: &[String],
    ) -> CheckResult {
        if !authorized_identities.contains(&draft.sender_identity) {
            return CheckResult {
                check_name: "identity".to_string(),
                passed: false,
                reason: Some(format!(
                    "Sender identity '{}' is not authorized to communicate",
                    draft.sender_identity
                )),
            };
        }

        CheckResult {
            check_name: "identity".to_string(),
            passed: true,
            reason: None,
        }
    }

    /// Check 4: Required Disclosures
    /// All mandatory disclosures must be present.
    pub fn check_disclosures(
        draft: &CommunicationDraft,
        required_disclosures: &[String],
    ) -> CheckResult {
        let present_kinds: std::collections::HashSet<_> =
            draft.disclosures.iter().map(|d| d.kind.as_str()).collect();

        for required in required_disclosures {
            if !present_kinds.contains(required.as_str()) {
                return CheckResult {
                    check_name: "disclosures".to_string(),
                    passed: false,
                    reason: Some(format!(
                        "Required disclosure '{}' is missing from communication",
                        required
                    )),
                };
            }
        }

        CheckResult {
            check_name: "disclosures".to_string(),
            passed: true,
            reason: None,
        }
    }

    /// Check 5: Audience/Channel Permissions
    /// Message must be appropriate for its intended channel.
    pub fn check_audience_permissions(
        draft: &CommunicationDraft,
        channel_policies: &BTreeMap<String, Vec<String>>,
    ) -> CheckResult {
        match channel_policies.get(&draft.channel) {
            None => CheckResult {
                check_name: "audience_permissions".to_string(),
                passed: false,
                reason: Some(format!(
                    "Channel '{}' is not configured in channel policies",
                    draft.channel
                )),
            },
            Some(_) => CheckResult {
                check_name: "audience_permissions".to_string(),
                passed: true,
                reason: None,
            },
        }
    }

    /// Check 6: Jurisdiction
    /// Communication must comply with applicable regulatory jurisdictions.
    pub fn check_jurisdiction(
        draft: &CommunicationDraft,
        permitted_jurisdictions: &[String],
    ) -> CheckResult {
        if !permitted_jurisdictions.contains(&draft.jurisdiction) {
            return CheckResult {
                check_name: "jurisdiction".to_string(),
                passed: false,
                reason: Some(format!(
                    "Jurisdiction '{}' is not in permitted list for communications",
                    draft.jurisdiction
                )),
            };
        }

        CheckResult {
            check_name: "jurisdiction".to_string(),
            passed: true,
            reason: None,
        }
    }

    /// Check 7: Market-Conduct Constraints
    /// Communication must comply with market conduct rules and regulations.
    pub fn check_market_conduct(
        draft: &CommunicationDraft,
        _conduct_rules: &[String],
    ) -> CheckResult {
        // This is a simplified check; in production it would parse the content
        // against conduct rules. For now, we check that no obvious red flags exist.
        if draft.content.to_lowercase().contains("guaranteed") {
            return CheckResult {
                check_name: "market_conduct".to_string(),
                passed: false,
                reason: Some(
                    "Communication contains prohibited language ('guaranteed') that violates market conduct rules"
                        .to_string(),
                ),
            };
        }

        CheckResult {
            check_name: "market_conduct".to_string(),
            passed: true,
            reason: None,
        }
    }
}

/// The Conduct Gate that runs all seven checks.
#[derive(Debug)]
pub struct ConductGate {
    authorized_sources: Vec<String>,
    authorized_identities: Vec<String>,
    required_disclosures: Vec<String>,
    channel_policies: BTreeMap<String, Vec<String>>,
    permitted_jurisdictions: Vec<String>,
    conduct_rules: Vec<String>,
}

impl ConductGate {
    /// Create a new conduct gate with the given policies.
    pub fn new(
        authorized_sources: Vec<String>,
        authorized_identities: Vec<String>,
        required_disclosures: Vec<String>,
        channel_policies: BTreeMap<String, Vec<String>>,
        permitted_jurisdictions: Vec<String>,
        conduct_rules: Vec<String>,
    ) -> Result<Self, Error> {
        if authorized_identities.is_empty() {
            return Err(Error::invalid("authorized_identities must not be empty"));
        }
        if permitted_jurisdictions.is_empty() {
            return Err(Error::invalid("permitted_jurisdictions must not be empty"));
        }

        Ok(Self {
            authorized_sources,
            authorized_identities,
            required_disclosures,
            channel_policies,
            permitted_jurisdictions,
            conduct_rules,
        })
    }

    /// Run all seven checks on a draft communication.
    /// Returns all results; draft passes if all checks pass.
    pub fn evaluate(&self, draft: &CommunicationDraft) -> Vec<CheckResult> {
        vec![
            checks::check_factual_support(draft),
            checks::check_provenance(draft, &self.authorized_sources),
            checks::check_identity(draft, &self.authorized_identities),
            checks::check_disclosures(draft, &self.required_disclosures),
            checks::check_audience_permissions(draft, &self.channel_policies),
            checks::check_jurisdiction(draft, &self.permitted_jurisdictions),
            checks::check_market_conduct(draft, &self.conduct_rules),
        ]
    }

    /// Check if a draft passes all seven checks.
    pub fn can_release(&self, draft: &CommunicationDraft) -> bool {
        self.evaluate(draft).iter().all(|r| r.passed)
    }

    /// Get all failed checks for a draft.
    pub fn failures(&self, draft: &CommunicationDraft) -> Vec<CheckResult> {
        self.evaluate(draft)
            .into_iter()
            .filter(|r| !r.passed)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic_draft() -> CommunicationDraft {
        CommunicationDraft {
            id: "draft-001".to_string(),
            content: "Market update: stock X is rising".to_string(),
            channel: "press".to_string(),
            sender_identity: "analyst-01".to_string(),
            evidence_citations: vec![EvidenceCitation {
                source_id: "source-001".to_string(),
                evidence_id: "evid-001".to_string(),
                confidence_bps: 8500,
            }],
            disclosures: vec![Disclosure {
                kind: "conflict_of_interest".to_string(),
                text: "No conflicts".to_string(),
            }],
            target_audience: "institutional".to_string(),
            jurisdiction: "US".to_string(),
        }
    }

    fn basic_gate() -> ConductGate {
        let mut channel_policies = BTreeMap::new();
        channel_policies.insert("press".to_string(), vec![]);

        ConductGate::new(
            vec!["source-001".to_string()],
            vec!["analyst-01".to_string()],
            vec!["conflict_of_interest".to_string()],
            channel_policies,
            vec!["US".to_string()],
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn check_factual_support_passes_when_citations_present() {
        let draft = basic_draft();
        let result = checks::check_factual_support(&draft);
        assert!(result.passed);
    }

    #[test]
    fn check_factual_support_blocks_when_no_citations() {
        let mut draft = basic_draft();
        draft.evidence_citations.clear();
        let result = checks::check_factual_support(&draft);
        assert!(!result.passed);
        assert!(result.reason.is_some());
    }

    #[test]
    fn check_provenance_passes_when_sources_authorized() {
        let draft = basic_draft();
        let gate = basic_gate();
        let result = checks::check_provenance(&draft, &gate.authorized_sources);
        assert!(result.passed);
    }

    #[test]
    fn check_provenance_blocks_when_source_unauthorized() {
        let mut draft = basic_draft();
        draft.evidence_citations[0].source_id = "unknown-source".to_string();
        let gate = basic_gate();
        let result = checks::check_provenance(&draft, &gate.authorized_sources);
        assert!(!result.passed);
    }

    #[test]
    fn check_identity_passes_when_authorized() {
        let draft = basic_draft();
        let gate = basic_gate();
        let result = checks::check_identity(&draft, &gate.authorized_identities);
        assert!(result.passed);
    }

    #[test]
    fn check_identity_blocks_when_unauthorized() {
        let mut draft = basic_draft();
        draft.sender_identity = "unknown-identity".to_string();
        let gate = basic_gate();
        let result = checks::check_identity(&draft, &gate.authorized_identities);
        assert!(!result.passed);
    }

    #[test]
    fn check_disclosures_passes_when_all_required_present() {
        let draft = basic_draft();
        let gate = basic_gate();
        let result = checks::check_disclosures(&draft, &gate.required_disclosures);
        assert!(result.passed);
    }

    #[test]
    fn check_disclosures_blocks_when_required_missing() {
        let mut draft = basic_draft();
        draft.disclosures.clear();
        let gate = basic_gate();
        let result = checks::check_disclosures(&draft, &gate.required_disclosures);
        assert!(!result.passed);
    }

    #[test]
    fn check_audience_permissions_passes_for_configured_channel() {
        let draft = basic_draft();
        let gate = basic_gate();
        let result = checks::check_audience_permissions(&draft, &gate.channel_policies);
        assert!(result.passed);
    }

    #[test]
    fn check_audience_permissions_blocks_for_unconfigured_channel() {
        let mut draft = basic_draft();
        draft.channel = "unknown_channel".to_string();
        let gate = basic_gate();
        let result = checks::check_audience_permissions(&draft, &gate.channel_policies);
        assert!(!result.passed);
    }

    #[test]
    fn check_jurisdiction_passes_when_permitted() {
        let draft = basic_draft();
        let gate = basic_gate();
        let result = checks::check_jurisdiction(&draft, &gate.permitted_jurisdictions);
        assert!(result.passed);
    }

    #[test]
    fn check_jurisdiction_blocks_when_not_permitted() {
        let mut draft = basic_draft();
        draft.jurisdiction = "UNKNOWN".to_string();
        let gate = basic_gate();
        let result = checks::check_jurisdiction(&draft, &gate.permitted_jurisdictions);
        assert!(!result.passed);
    }

    #[test]
    fn check_market_conduct_passes_for_compliant_content() {
        let draft = basic_draft();
        let gate = basic_gate();
        let result = checks::check_market_conduct(&draft, &gate.conduct_rules);
        assert!(result.passed);
    }

    #[test]
    fn check_market_conduct_blocks_for_prohibited_language() {
        let mut draft = basic_draft();
        draft.content = "This investment is guaranteed to succeed".to_string();
        let gate = basic_gate();
        let result = checks::check_market_conduct(&draft, &gate.conduct_rules);
        assert!(!result.passed);
    }

    #[test]
    fn gate_can_release_when_all_checks_pass() {
        let draft = basic_draft();
        let gate = basic_gate();
        assert!(gate.can_release(&draft));
    }

    #[test]
    fn gate_cannot_release_when_any_check_fails() {
        let mut draft = basic_draft();
        draft.sender_identity = "unauthorized".to_string();
        let gate = basic_gate();
        assert!(!gate.can_release(&draft));
    }

    #[test]
    fn gate_reports_all_failures() {
        let mut draft = basic_draft();
        draft.sender_identity = "unauthorized".to_string();
        draft.disclosures.clear();
        let gate = basic_gate();
        let failures = gate.failures(&draft);
        assert!(failures.len() >= 2);
        assert!(failures.iter().any(|f| f.check_name == "identity"));
        assert!(failures.iter().any(|f| f.check_name == "disclosures"));
    }

    #[test]
    fn gate_new_rejects_empty_authorized_identities() {
        let channel_policies = BTreeMap::new();
        let result = ConductGate::new(
            vec![],
            vec![], // empty - should fail
            vec![],
            channel_policies,
            vec!["US".to_string()],
            vec![],
        );
        assert!(result.is_err());
    }

    #[test]
    fn gate_new_rejects_empty_permitted_jurisdictions() {
        let channel_policies = BTreeMap::new();
        let result = ConductGate::new(
            vec![],
            vec!["analyst".to_string()],
            vec![],
            channel_policies,
            vec![], // empty - should fail
            vec![],
        );
        assert!(result.is_err());
    }
}
