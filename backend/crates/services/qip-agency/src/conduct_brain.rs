//! Communications & Conduct Brain: gates every external intervention through
//! seven checks, and a set of interventions through one more.
//!
//! AGENCY-037, R12-AGENCY-1056..1069. Every proposed external communication —
//! and [`crate::engine::execute`] treats every adapter call as one, because
//! an order, a quote and a post are all something the world sees — is checked
//! for:
//! 1. Factual support — claims backed by well-formed evidence citations
//! 2. Provenance — every cited source is an authorized one
//! 3. Identity — the sender is an authorized identity
//! 4. Required disclosures — each required kind present, with text; a
//!    policy requiring no kind is refused at construction (R12-AGENCY-1059)
//! 5. Audience/channel permissions — the channel is configured *and* the
//!    audience is one that channel may address
//! 6. Jurisdiction — a permitted jurisdiction
//! 7. Market conduct — every [`ConductRule`] the gate holds, matched as whole
//!    words, never substrings
//!
//! and, across every draft released together, [`checks::check_coordinated_voices`]:
//! the same message under more than one identity is fake consensus however
//! each draft reads alone.
//!
//! What this replaced, because it is the failure each check now prevents:
//! check 7 took a `_conduct_rules` argument it never read and refused only
//! the substring "guaranteed", so a policy naming `no_manipulation` refused
//! no manipulation; check 5 passed whenever the channel key existed and never
//! read the audience list it mapped to; and nothing anywhere addressed fake
//! consensus or rumour engineering. None of it was called before an adapter.
//!
//! Shadow-only form: drafting and gating, with no live channel anywhere.

use qip_contracts::action::{MANIPULATION_PHRASES, contains_phrase, words};
use qip_core::Error;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A proposed external communication awaiting conduct review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommunicationDraft {
    /// Unique identifier for this draft
    pub id: String,
    /// The message content: for a market step, the order as the venue and
    /// the tape will see it described.
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
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceCitation {
    pub source_id: String,
    pub evidence_id: String,
    /// Basis points, `0..=10000`. Outside that range the citation is refused,
    /// not clamped.
    pub confidence_bps: i64,
}

/// A required disclosure for regulatory/policy compliance.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

impl CheckResult {
    fn pass(check: &str) -> Self {
        Self {
            check_name: check.to_string(),
            passed: true,
            reason: None,
        }
    }

    fn fail(check: &str, reason: String) -> Self {
        Self {
            check_name: check.to_string(),
            passed: false,
            reason: Some(reason),
        }
    }
}

/// Promissory language: a statement that an uncertain outcome is certain.
const FALSE_CLAIM_PHRASES: &[&str] = &[
    "guaranteed",
    "guarantee",
    "guarantees",
    "risk free",
    "riskless",
    "cannot lose",
    "can not lose",
    "cant lose",
    "sure thing",
    "no risk",
    "certain profit",
    "certain profits",
    "free money",
];

/// A claim that a crowd already holds a view, which is how manufactured
/// consensus reads from the inside of one message (R12-AGENCY-1064).
const FAKE_CONSENSUS_PHRASES: &[&str] = &[
    "everyone is buying",
    "everybody is buying",
    "everyone is selling",
    "everybody is selling",
    "everyone agrees",
    "everybody agrees",
    "all analysts agree",
    "every analyst agrees",
    "the whole market agrees",
    "the crowd agrees",
    "smart money is buying",
    "smart money is selling",
    "nobody is selling",
    "nobody is buying",
    "join thousands of investors",
];

/// Hearsay presented as information: the vocabulary of a rumour being started
/// or passed on (R12-AGENCY-1069).
const RUMOUR_PHRASES: &[&str] = &[
    "rumour",
    "rumours",
    "rumoured",
    "rumor",
    "rumors",
    "rumored",
    "rumour has it",
    "rumor has it",
    "sources say",
    "insiders say",
    "people are saying",
    "unconfirmed reports",
    "word is",
    "i heard",
    "whispers",
];

/// One market-conduct rule. Parsed from a closed vocabulary so a rule nobody
/// implemented cannot be configured: a policy naming `no_insider_tips`, which
/// no detector reads, is refused at construction rather than accepted as a
/// rule that silently never fires.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConductRule {
    /// `no_false_claims`: promissory language — "guaranteed", "risk-free".
    NoFalseClaims,
    /// `no_manipulation`: the shared [`MANIPULATION_PHRASES`].
    NoManipulation,
    /// `no_fake_consensus`: a claim that a crowd already agrees.
    NoFakeConsensus,
    /// `no_rumour_engineering`: hearsay presented as information.
    NoRumourEngineering,
    /// `prohibit:<phrase>`: a phrase this deployment refuses, as whole words.
    Prohibit(String),
}

/// The rules every conduct policy must hold. R12-AGENCY-1063..1069 say the
/// platform *never* does these things; a policy that could be configured not
/// to check one is a policy that eventually will be.
pub const MANDATORY_RULES: [&str; 4] = [
    "no_false_claims",
    "no_manipulation",
    "no_fake_consensus",
    "no_rumour_engineering",
];

impl ConductRule {
    /// Parse one rule, refusing anything outside the vocabulary.
    pub fn parse(rule: &str) -> Result<Self, Error> {
        let trimmed = rule.trim();
        if let Some(phrase) = trimmed.strip_prefix("prohibit:") {
            if words(phrase).is_empty() {
                return Err(Error::invalid(format!(
                    "conduct rule `{rule}` prohibits no words; a blank phrase matches nothing"
                )));
            }
            return Ok(Self::Prohibit(phrase.trim().to_string()));
        }
        match trimmed {
            "no_false_claims" => Ok(Self::NoFalseClaims),
            "no_manipulation" => Ok(Self::NoManipulation),
            "no_fake_consensus" => Ok(Self::NoFakeConsensus),
            "no_rumour_engineering" => Ok(Self::NoRumourEngineering),
            other => Err(Error::invalid(format!(
                "conduct rule `{other}` is not one this gate can evaluate; use one of {} or \
                 `prohibit:<phrase>`",
                MANDATORY_RULES.join(", ")
            ))),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Self::NoFalseClaims => "no_false_claims".to_string(),
            Self::NoManipulation => "no_manipulation".to_string(),
            Self::NoFakeConsensus => "no_fake_consensus".to_string(),
            Self::NoRumourEngineering => "no_rumour_engineering".to_string(),
            Self::Prohibit(phrase) => format!("prohibit:{phrase}"),
        }
    }

    /// The first phrase of this rule that `content` contains as whole words.
    pub fn breach<'a>(&'a self, content: &str) -> Option<&'a str> {
        let phrases: &[&str] = match self {
            Self::NoFalseClaims => FALSE_CLAIM_PHRASES,
            Self::NoManipulation => MANIPULATION_PHRASES,
            Self::NoFakeConsensus => FAKE_CONSENSUS_PHRASES,
            Self::NoRumourEngineering => RUMOUR_PHRASES,
            Self::Prohibit(phrase) => {
                return contains_phrase(content, phrase).then_some(phrase.as_str());
            }
        };
        phrases
            .iter()
            .copied()
            .find(|phrase| contains_phrase(content, phrase))
    }
}

/// The seven per-draft checks and the one across drafts, each a pure
/// function of what it is given.
pub mod checks {
    use super::*;

    /// Check 1: Factual Support. Content with claims needs citations, and a
    /// citation with no evidence id or a confidence outside `0..=10000` is not
    /// support: it is the shape of a citation with nothing behind it.
    pub fn check_factual_support(draft: &CommunicationDraft) -> CheckResult {
        const CHECK: &str = "factual_support";
        if draft.evidence_citations.is_empty() && !draft.content.trim().is_empty() {
            return CheckResult::fail(
                CHECK,
                "Communication contains claims but provides no evidence citations".to_string(),
            );
        }
        for citation in &draft.evidence_citations {
            if citation.evidence_id.trim().is_empty() {
                return CheckResult::fail(
                    CHECK,
                    format!(
                        "a citation of source '{}' names no evidence; cite the item, not the source",
                        citation.source_id
                    ),
                );
            }
            if !(0..=10_000).contains(&citation.confidence_bps) {
                return CheckResult::fail(
                    CHECK,
                    format!(
                        "citation '{}' claims confidence {} bps, outside 0..=10000",
                        citation.evidence_id, citation.confidence_bps
                    ),
                );
            }
        }
        CheckResult::pass(CHECK)
    }

    /// Check 2: Provenance. Every cited source must be an authorized one.
    pub fn check_provenance(
        draft: &CommunicationDraft,
        authorized_sources: &[String],
    ) -> CheckResult {
        const CHECK: &str = "provenance";
        match draft
            .evidence_citations
            .iter()
            .find(|c| !authorized_sources.contains(&c.source_id))
        {
            Some(citation) => CheckResult::fail(
                CHECK,
                format!(
                    "Evidence source '{}' is not in authorized sources list",
                    citation.source_id
                ),
            ),
            None => CheckResult::pass(CHECK),
        }
    }

    /// Check 3: Identity. The sender must be an authorized identity: a name
    /// nobody authorized is a fabricated identity whatever it calls itself.
    pub fn check_identity(
        draft: &CommunicationDraft,
        authorized_identities: &[String],
    ) -> CheckResult {
        const CHECK: &str = "identity";
        if !authorized_identities.contains(&draft.sender_identity) {
            return CheckResult::fail(
                CHECK,
                format!(
                    "Sender identity '{}' is not authorized to communicate",
                    draft.sender_identity
                ),
            );
        }
        CheckResult::pass(CHECK)
    }

    /// Check 4: Required Disclosures. Each required kind must be present
    /// with text: a `paid_promotion` disclosure whose text is blank discloses
    /// nothing, and promotion behind it is undisclosed (R12-AGENCY-1066).
    pub fn check_disclosures(
        draft: &CommunicationDraft,
        required_disclosures: &[String],
    ) -> CheckResult {
        const CHECK: &str = "disclosures";
        for required in required_disclosures {
            let present = draft
                .disclosures
                .iter()
                .any(|d| &d.kind == required && !d.text.trim().is_empty());
            if !present {
                return CheckResult::fail(
                    CHECK,
                    format!(
                        "Required disclosure '{required}' is missing from communication, or its text is blank"
                    ),
                );
            }
        }
        CheckResult::pass(CHECK)
    }

    /// Check 5: Audience/Channel Permissions. The channel must be configured
    /// and the draft's audience must be one that channel may address. An
    /// empty audience list permits nobody: fail closed.
    pub fn check_audience_permissions(
        draft: &CommunicationDraft,
        channel_policies: &BTreeMap<String, Vec<String>>,
    ) -> CheckResult {
        const CHECK: &str = "audience_permissions";
        match channel_policies.get(&draft.channel) {
            None => CheckResult::fail(
                CHECK,
                format!(
                    "Channel '{}' is not configured in channel policies",
                    draft.channel
                ),
            ),
            Some(audiences) if !audiences.contains(&draft.target_audience) => CheckResult::fail(
                CHECK,
                format!(
                    "Channel '{}' may not address audience '{}'; it permits {:?}",
                    draft.channel, draft.target_audience, audiences
                ),
            ),
            Some(_) => CheckResult::pass(CHECK),
        }
    }

    /// Check 6: Jurisdiction.
    pub fn check_jurisdiction(
        draft: &CommunicationDraft,
        permitted_jurisdictions: &[String],
    ) -> CheckResult {
        const CHECK: &str = "jurisdiction";
        if !permitted_jurisdictions.contains(&draft.jurisdiction) {
            return CheckResult::fail(
                CHECK,
                format!(
                    "Jurisdiction '{}' is not in permitted list for communications",
                    draft.jurisdiction
                ),
            );
        }
        CheckResult::pass(CHECK)
    }

    /// Check 7: Market conduct. Evaluates exactly the rules it is given,
    /// against the content, as whole words. The first breach is reported with
    /// the rule and the phrase, so an operator can see which rule fired.
    pub fn check_market_conduct(
        draft: &CommunicationDraft,
        conduct_rules: &[ConductRule],
    ) -> CheckResult {
        const CHECK: &str = "market_conduct";
        for rule in conduct_rules {
            if let Some(phrase) = rule.breach(&draft.content) {
                return CheckResult::fail(
                    CHECK,
                    format!(
                        "Communication breaches conduct rule `{}` with '{phrase}'",
                        rule.name()
                    ),
                );
            }
        }
        CheckResult::pass(CHECK)
    }

    /// Across drafts: the same message, by its words, released under more
    /// than one identity is one voice posing as several — fake consensus
    /// (R12-AGENCY-1064) and sockpuppetry (R12-AGENCY-1063), however
    /// innocent each draft reads alone. One identity repeating itself on two
    /// channels is not consensus and passes.
    pub fn check_coordinated_voices(drafts: &[&CommunicationDraft]) -> CheckResult {
        const CHECK: &str = "coordinated_voices";
        let mut voices: BTreeMap<Vec<String>, BTreeSet<&str>> = BTreeMap::new();
        for draft in drafts {
            let said = words(&draft.content);
            if said.is_empty() {
                continue;
            }
            voices
                .entry(said)
                .or_default()
                .insert(draft.sender_identity.as_str());
        }
        match voices.values().find(|senders| senders.len() > 1) {
            Some(senders) => CheckResult::fail(
                CHECK,
                format!(
                    "the same message is released under {} identities ({}); one voice posing \
                     as several is manufactured consensus",
                    senders.len(),
                    senders.iter().copied().collect::<Vec<_>>().join(", ")
                ),
            ),
            None => CheckResult::pass(CHECK),
        }
    }
}

/// The gate's configuration as a document, so a composition root or a
/// request can carry it; [`ConductGate::new`] still refuses a bad one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConductPolicy {
    pub authorized_sources: Vec<String>,
    pub authorized_identities: Vec<String>,
    pub required_disclosures: Vec<String>,
    /// Channel to the audiences it may address.
    pub channel_policies: BTreeMap<String, Vec<String>>,
    pub permitted_jurisdictions: Vec<String>,
    pub conduct_rules: Vec<String>,
}

/// The Conduct Gate that runs all seven checks.
#[derive(Debug)]
pub struct ConductGate {
    authorized_sources: Vec<String>,
    authorized_identities: Vec<String>,
    required_disclosures: Vec<String>,
    channel_policies: BTreeMap<String, Vec<String>>,
    permitted_jurisdictions: Vec<String>,
    conduct_rules: Vec<ConductRule>,
}

impl ConductGate {
    /// Create a new conduct gate with the given policies.
    ///
    /// Refuses no identities, no jurisdictions, no required disclosure
    /// kind, a conduct rule outside the vocabulary, and a rule set missing
    /// any of [`MANDATORY_RULES`].
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
        // R12-AGENCY-1059 / 1066: a policy requiring no disclosure is a gate
        // whose fourth check passes every draft, and undisclosed promotion is
        // exactly a draft with no disclosure. Like identities and
        // jurisdictions, an empty list is refused rather than read as "none
        // required": a deployment that truly requires none says which kind
        // it requires and why in the text the draft carries.
        if required_disclosures
            .iter()
            .all(|kind| kind.trim().is_empty())
        {
            return Err(Error::invalid(
                "required_disclosures must name at least one disclosure kind; a gate that \
                 requires no disclosure cannot refuse an undisclosed promotion",
            ));
        }
        let parsed = conduct_rules
            .iter()
            .map(|r| ConductRule::parse(r))
            .collect::<Result<BTreeSet<_>, _>>()?;
        for mandatory in MANDATORY_RULES {
            let rule = ConductRule::parse(mandatory)?;
            if !parsed.contains(&rule) {
                return Err(Error::invalid(format!(
                    "conduct_rules omits `{mandatory}`; the platform never does what it forbids, \
                     so no conduct policy may leave it unchecked"
                )));
            }
        }
        Ok(Self {
            authorized_sources,
            authorized_identities,
            required_disclosures,
            channel_policies,
            permitted_jurisdictions,
            conduct_rules: parsed.into_iter().collect(),
        })
    }

    pub fn from_policy(policy: ConductPolicy) -> Result<Self, Error> {
        Self::new(
            policy.authorized_sources,
            policy.authorized_identities,
            policy.required_disclosures,
            policy.channel_policies,
            policy.permitted_jurisdictions,
            policy.conduct_rules,
        )
    }

    /// The rules this gate evaluates, in a fixed order.
    pub fn rules(&self) -> &[ConductRule] {
        &self.conduct_rules
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

    /// Every failure across a set of drafts released together: each draft's
    /// own seven, tagged with its id, then the cross-draft check. Empty means
    /// the whole set may be released.
    pub fn campaign_failures(&self, drafts: &[&CommunicationDraft]) -> Vec<(String, CheckResult)> {
        let mut failed: Vec<(String, CheckResult)> = drafts
            .iter()
            .flat_map(|d| self.failures(d).into_iter().map(move |f| (d.id.clone(), f)))
            .collect();
        let voices = checks::check_coordinated_voices(drafts);
        if !voices.passed {
            failed.push(("*".to_string(), voices));
        }
        failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Vec<String> {
        MANDATORY_RULES.iter().map(|r| r.to_string()).collect()
    }

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
        channel_policies.insert("press".to_string(), vec!["institutional".to_string()]);

        ConductGate::new(
            vec!["source-001".to_string()],
            vec!["analyst-01".to_string()],
            vec!["conflict_of_interest".to_string()],
            channel_policies,
            vec!["US".to_string()],
            rules(),
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
    fn a_citation_with_no_evidence_id_or_an_impossible_confidence_is_not_factual_support() {
        assert!(
            checks::check_factual_support(&basic_draft()).passed,
            "premise"
        );
        let mut draft = basic_draft();
        draft.evidence_citations[0].evidence_id = " ".to_string();
        assert!(!checks::check_factual_support(&draft).passed);
        for bps in [-1, 10_001] {
            let mut draft = basic_draft();
            draft.evidence_citations[0].confidence_bps = bps;
            assert!(!checks::check_factual_support(&draft).passed, "{bps}");
        }
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
    fn a_required_disclosure_present_by_kind_with_blank_text_discloses_nothing() {
        let gate = basic_gate();
        assert!(
            checks::check_disclosures(&basic_draft(), &gate.required_disclosures).passed,
            "premise"
        );
        let mut draft = basic_draft();
        draft.disclosures[0].text = "   ".to_string();
        assert!(!checks::check_disclosures(&draft, &gate.required_disclosures).passed);
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

    /// R12-AGENCY-1061 was INCORRECT: the check passed on `Some(_)` whatever
    /// the list held. Each assertion below changes only the list.
    #[test]
    fn the_audience_list_a_channel_maps_to_decides_the_verdict() {
        let draft = basic_draft();
        let policy = |audiences: &[&str]| {
            BTreeMap::from([(
                "press".to_string(),
                audiences.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            )])
        };
        assert!(
            checks::check_audience_permissions(&draft, &policy(&["institutional"])).passed,
            "premise: the configured audience is admitted"
        );
        let refused = checks::check_audience_permissions(&draft, &policy(&["retail"]));
        assert!(!refused.passed);
        assert!(refused.reason.unwrap().contains("institutional"));
        assert!(
            !checks::check_audience_permissions(&draft, &policy(&[])).passed,
            "an empty audience list permits nobody"
        );
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
        assert!(result.reason.unwrap().contains("no_false_claims"));
    }

    /// R12-AGENCY-1060 was INCORRECT: the rules argument was `_conduct_rules`
    /// and changed nothing. Here the same content is judged under rule sets
    /// that differ by exactly one rule, and the verdict follows the rule.
    #[test]
    fn the_rules_given_decide_the_market_conduct_verdict() {
        let mut draft = basic_draft();
        let cases: [(&str, ConductRule); 5] = [
            ("returns are guaranteed", ConductRule::NoFalseClaims),
            ("we will spoof the bid", ConductRule::NoManipulation),
            ("everyone is buying this name", ConductRule::NoFakeConsensus),
            (
                "rumour has it the CEO is leaving",
                ConductRule::NoRumourEngineering,
            ),
            (
                "a pre-announcement allocation is open",
                ConductRule::Prohibit("pre announcement allocation".to_string()),
            ),
        ];
        for (content, rule) in cases {
            draft.content = content.to_string();
            assert!(
                checks::check_market_conduct(&draft, &[]).passed,
                "premise: with no rule, `{content}` passes"
            );
            let verdict = checks::check_market_conduct(&draft, std::slice::from_ref(&rule));
            assert!(
                !verdict.passed,
                "{} did not fire on `{content}`",
                rule.name()
            );
            assert!(verdict.reason.unwrap().contains(&rule.name()));
        }
    }

    /// The substring test this replaced would refuse every line below; whole
    /// words refuse none of them, under every mandatory rule at once.
    #[test]
    fn a_word_that_merely_contains_a_prohibited_word_does_not_trip_its_rule() {
        let gate = basic_gate();
        let mut draft = basic_draft();
        draft.content = "we will spoof the bid".to_string();
        assert!(
            !checks::check_market_conduct(&draft, gate.rules()).passed,
            "premise: the gate's rules do fire on the whole word"
        );
        for honest in [
            "unwashed inventory and a dishwasher maker in Washington",
            "spoofproof feed handlers",
            "a guarantor bank reported",
            "every analyst disagrees with every other",
            "the storefront running costs fell",
        ] {
            draft.content = honest.to_string();
            let verdict = checks::check_market_conduct(&draft, gate.rules());
            assert!(verdict.passed, "{honest}: {:?}", verdict.reason);
        }
    }

    #[test]
    fn a_conduct_policy_missing_a_mandatory_rule_or_naming_an_unknown_one_is_refused() {
        let build = |rules: Vec<String>| {
            ConductGate::new(
                vec![],
                vec!["a".to_string()],
                vec!["conflict_of_interest".to_string()],
                BTreeMap::new(),
                vec!["US".to_string()],
                rules,
            )
        };
        assert!(build(rules()).is_ok(), "premise: the mandatory set builds");
        for omitted in MANDATORY_RULES {
            let partial = rules().into_iter().filter(|r| r != omitted).collect();
            let err = build(partial).unwrap_err();
            assert!(err.message().contains(omitted), "{err}");
        }
        let mut unknown = rules();
        unknown.push("no_insider_tips".to_string());
        assert!(build(unknown).is_err());
        let mut blank = rules();
        blank.push("prohibit:  -- ".to_string());
        assert!(build(blank).is_err());
    }

    #[test]
    fn the_same_message_under_two_identities_is_fake_consensus_and_under_one_is_not() {
        let a = basic_draft();
        let mut same_voice_elsewhere = basic_draft();
        same_voice_elsewhere.channel = "social".to_string();
        assert!(
            checks::check_coordinated_voices(&[&a, &same_voice_elsewhere]).passed,
            "premise: one identity on two channels is one voice"
        );
        let mut puppet = basic_draft();
        puppet.sender_identity = "analyst-02".to_string();
        // Different case and punctuation are the same words.
        puppet.content = "MARKET UPDATE -- stock x is rising!".to_string();
        let verdict = checks::check_coordinated_voices(&[&a, &puppet]);
        assert!(!verdict.passed);
        assert!(verdict.reason.unwrap().contains("analyst-02"));
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
            rules(),
        );
        assert!(result.is_err());
    }

    /// R12-AGENCY-1059: the disclosure check can be configured inert by an
    /// empty list, and then undisclosed promotion passes it. Refused at
    /// construction like the other two empty lists.
    #[test]
    fn gate_new_rejects_empty_or_blank_required_disclosures() {
        let build = |required: Vec<String>| {
            ConductGate::new(
                vec![],
                vec!["analyst".to_string()],
                required,
                BTreeMap::new(),
                vec!["US".to_string()],
                rules(),
            )
        };
        assert!(
            build(vec!["conflict_of_interest".to_string()]).is_ok(),
            "premise: one named kind builds"
        );
        let err = build(vec![]).unwrap_err();
        assert!(err.message().contains("required_disclosures"), "{err}");
        assert!(build(vec!["   ".to_string()]).is_err());
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
            rules(),
        );
        assert!(result.is_err());
    }
}
