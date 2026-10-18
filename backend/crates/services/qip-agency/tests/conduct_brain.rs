//! Acceptance tests for AGENCY-037: Communications & Conduct Brain
//!
//! "For each of the seven checks, a draft that fails only that check is blocked
//! before any channel adapter is called, and a draft that passes all seven is released."

use qip_agency::conduct_brain::*;
use std::collections::BTreeMap;

fn basic_gate() -> ConductGate {
    let mut channel_policies = BTreeMap::new();
    // Each channel names the audiences it may address. These were empty lists
    // when the check never read them; an empty list now permits nobody.
    channel_policies.insert("press".to_string(), vec!["investors".to_string()]);
    channel_policies.insert("social".to_string(), vec!["investors".to_string()]);

    #[allow(clippy::unwrap_used)]
    {
        ConductGate::new(
            vec!["financial_data".to_string(), "news_wire".to_string()],
            vec![
                "communications_team".to_string(),
                "exec_speaker".to_string(),
            ],
            vec![
                "conflict_of_interest".to_string(),
                "regulatory_status".to_string(),
            ],
            channel_policies,
            vec!["US".to_string(), "EU".to_string()],
            vec![
                "no_false_claims".to_string(),
                "no_manipulation".to_string(),
                "no_fake_consensus".to_string(),
                "no_rumour_engineering".to_string(),
            ],
        )
        .unwrap()
    }
}

fn compliant_draft() -> CommunicationDraft {
    CommunicationDraft {
        id: "draft-001".to_string(),
        content: "Quarterly earnings show stable growth".to_string(),
        channel: "press".to_string(),
        sender_identity: "communications_team".to_string(),
        evidence_citations: vec![EvidenceCitation {
            source_id: "financial_data".to_string(),
            evidence_id: "q4-earnings-001".to_string(),
            confidence_bps: 9000,
        }],
        disclosures: vec![
            Disclosure {
                kind: "conflict_of_interest".to_string(),
                text: "No conflicts disclosed".to_string(),
            },
            Disclosure {
                kind: "regulatory_status".to_string(),
                text: "Statement prepared in compliance with SEC Rule 10b5".to_string(),
            },
        ],
        target_audience: "investors".to_string(),
        jurisdiction: "US".to_string(),
    }
}

#[test]
fn factual_support_check_blocks_when_missing_evidence() {
    let mut draft = compliant_draft();
    draft.evidence_citations.clear();
    let gate = basic_gate();

    let results = gate.evaluate(&draft);
    let factual_check = results
        .iter()
        .find(|r| r.check_name == "factual_support")
        .unwrap();

    assert!(
        !factual_check.passed,
        "Factual support check should block when evidence missing"
    );
    assert!(factual_check.reason.is_some());
    // Verify only this check failed, others still pass
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "factual_support")
        .collect();
    assert!(
        other_failures.is_empty(),
        "Other checks should pass when only evidence is missing"
    );
}

#[test]
fn provenance_check_blocks_when_source_unauthorized() {
    let mut draft = compliant_draft();
    draft.evidence_citations[0].source_id = "unknown_source".to_string();
    let gate = basic_gate();

    let results = gate.evaluate(&draft);
    let provenance_check = results
        .iter()
        .find(|r| r.check_name == "provenance")
        .unwrap();

    assert!(
        !provenance_check.passed,
        "Provenance check should block unauthorized source"
    );
    assert!(provenance_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "provenance")
        .collect();
    assert!(
        other_failures.is_empty(),
        "Other checks should pass when only provenance is bad"
    );
}

#[test]
fn identity_check_blocks_when_sender_unauthorized() {
    let mut draft = compliant_draft();
    draft.sender_identity = "unknown_person".to_string();
    let gate = basic_gate();

    let results = gate.evaluate(&draft);
    let identity_check = results.iter().find(|r| r.check_name == "identity").unwrap();

    assert!(
        !identity_check.passed,
        "Identity check should block unauthorized sender"
    );
    assert!(identity_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "identity")
        .collect();
    assert!(
        other_failures.is_empty(),
        "Other checks should pass when only identity is bad"
    );
}

#[test]
fn disclosures_check_blocks_when_required_missing() {
    let mut draft = compliant_draft();
    // Remove conflict_of_interest disclosure
    draft
        .disclosures
        .retain(|d| d.kind != "conflict_of_interest");
    let gate = basic_gate();

    let results = gate.evaluate(&draft);
    let disclosure_check = results
        .iter()
        .find(|r| r.check_name == "disclosures")
        .unwrap();

    assert!(
        !disclosure_check.passed,
        "Disclosures check should block when required disclosure missing"
    );
    assert!(disclosure_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "disclosures")
        .collect();
    assert!(
        other_failures.is_empty(),
        "Other checks should pass when only disclosures are missing"
    );
}

#[test]
fn audience_permissions_check_blocks_when_channel_unconfigured() {
    let mut draft = compliant_draft();
    draft.channel = "unknown_channel".to_string();
    let gate = basic_gate();

    let results = gate.evaluate(&draft);
    let audience_check = results
        .iter()
        .find(|r| r.check_name == "audience_permissions")
        .unwrap();

    assert!(
        !audience_check.passed,
        "Audience check should block unconfigured channel"
    );
    assert!(audience_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "audience_permissions")
        .collect();
    assert!(
        other_failures.is_empty(),
        "Other checks should pass when only channel is bad"
    );
}

#[test]
fn jurisdiction_check_blocks_when_not_permitted() {
    let mut draft = compliant_draft();
    draft.jurisdiction = "UNKNOWN".to_string();
    let gate = basic_gate();

    let results = gate.evaluate(&draft);
    let jurisdiction_check = results
        .iter()
        .find(|r| r.check_name == "jurisdiction")
        .unwrap();

    assert!(
        !jurisdiction_check.passed,
        "Jurisdiction check should block non-permitted jurisdiction"
    );
    assert!(jurisdiction_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "jurisdiction")
        .collect();
    assert!(
        other_failures.is_empty(),
        "Other checks should pass when only jurisdiction is bad"
    );
}

#[test]
fn market_conduct_check_blocks_when_prohibited_language_present() {
    let mut draft = compliant_draft();
    draft.content = "This investment is guaranteed to make you rich".to_string();
    let gate = basic_gate();

    let results = gate.evaluate(&draft);
    let conduct_check = results
        .iter()
        .find(|r| r.check_name == "market_conduct")
        .unwrap();

    assert!(
        !conduct_check.passed,
        "Market conduct check should block prohibited language"
    );
    assert!(conduct_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "market_conduct")
        .collect();
    assert!(
        other_failures.is_empty(),
        "Other checks should pass when only conduct is bad"
    );
}

#[test]
fn compliant_draft_passes_all_seven_checks() {
    let draft = compliant_draft();
    let gate = basic_gate();

    let results = gate.evaluate(&draft);
    assert_eq!(results.len(), 7, "Should evaluate all seven checks");

    for result in &results {
        assert!(
            result.passed,
            "Check '{}' should pass for compliant draft",
            result.check_name
        );
    }

    assert!(
        gate.can_release(&draft),
        "Compliant draft should be released"
    );
}

#[test]
fn draft_with_multiple_failures_blocks_on_all_failures() {
    let mut draft = compliant_draft();
    // Fail three checks
    draft.evidence_citations.clear(); // fails factual_support
    draft.sender_identity = "unauthorized".to_string(); // fails identity
    draft.jurisdiction = "UNKNOWN".to_string(); // fails jurisdiction

    let gate = basic_gate();
    let results = gate.evaluate(&draft);
    let failures: Vec<_> = results.iter().filter(|r| !r.passed).collect();

    assert_eq!(failures.len(), 3);
    assert!(failures.iter().any(|f| f.check_name == "factual_support"));
    assert!(failures.iter().any(|f| f.check_name == "identity"));
    assert!(failures.iter().any(|f| f.check_name == "jurisdiction"));

    assert!(!gate.can_release(&draft));
}

#[test]
fn each_check_independently_enforced_factual_support() {
    let gate = basic_gate();
    let mut draft = compliant_draft();
    draft.evidence_citations.clear();

    let results = gate.evaluate(&draft);
    let failed_check = results
        .iter()
        .find(|r| r.check_name == "factual_support")
        .unwrap();
    assert!(!failed_check.passed);
    assert!(failed_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "factual_support")
        .collect();
    assert!(other_failures.is_empty());
}

#[test]
fn each_check_independently_enforced_provenance() {
    let gate = basic_gate();
    let mut draft = compliant_draft();
    draft.evidence_citations[0].source_id = "unknown_source".to_string();

    let results = gate.evaluate(&draft);
    let failed_check = results
        .iter()
        .find(|r| r.check_name == "provenance")
        .unwrap();
    assert!(!failed_check.passed);
    assert!(failed_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "provenance")
        .collect();
    assert!(other_failures.is_empty());
}

#[test]
fn each_check_independently_enforced_identity() {
    let gate = basic_gate();
    let mut draft = compliant_draft();
    draft.sender_identity = "bad_identity".to_string();

    let results = gate.evaluate(&draft);
    let failed_check = results.iter().find(|r| r.check_name == "identity").unwrap();
    assert!(!failed_check.passed);
    assert!(failed_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "identity")
        .collect();
    assert!(other_failures.is_empty());
}

#[test]
fn each_check_independently_enforced_disclosures() {
    let gate = basic_gate();
    let mut draft = compliant_draft();
    draft.disclosures.clear();

    let results = gate.evaluate(&draft);
    let failed_check = results
        .iter()
        .find(|r| r.check_name == "disclosures")
        .unwrap();
    assert!(!failed_check.passed);
    assert!(failed_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "disclosures")
        .collect();
    assert!(other_failures.is_empty());
}

#[test]
fn each_check_independently_enforced_audience() {
    let gate = basic_gate();
    let mut draft = compliant_draft();
    draft.channel = "bad_channel".to_string();

    let results = gate.evaluate(&draft);
    let failed_check = results
        .iter()
        .find(|r| r.check_name == "audience_permissions")
        .unwrap();
    assert!(!failed_check.passed);
    assert!(failed_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "audience_permissions")
        .collect();
    assert!(other_failures.is_empty());
}

#[test]
fn each_check_independently_enforced_jurisdiction() {
    let gate = basic_gate();
    let mut draft = compliant_draft();
    draft.jurisdiction = "BAD".to_string();

    let results = gate.evaluate(&draft);
    let failed_check = results
        .iter()
        .find(|r| r.check_name == "jurisdiction")
        .unwrap();
    assert!(!failed_check.passed);
    assert!(failed_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "jurisdiction")
        .collect();
    assert!(other_failures.is_empty());
}

#[test]
fn each_check_independently_enforced_market_conduct() {
    let gate = basic_gate();
    let mut draft = compliant_draft();
    draft.content = "This is guaranteed to work".to_string();

    let results = gate.evaluate(&draft);
    let failed_check = results
        .iter()
        .find(|r| r.check_name == "market_conduct")
        .unwrap();
    assert!(!failed_check.passed);
    assert!(failed_check.reason.is_some());
    let other_failures: Vec<_> = results
        .iter()
        .filter(|r| !r.passed && r.check_name != "market_conduct")
        .collect();
    assert!(other_failures.is_empty());
}

#[test]
fn gate_failures_method_returns_only_failures() {
    let mut draft = compliant_draft();
    draft.sender_identity = "bad".to_string();
    draft.evidence_citations.clear();

    let gate = basic_gate();
    let failures = gate.failures(&draft);

    assert!(failures.len() >= 2);
    assert!(failures.iter().all(|f| !f.passed));
    assert!(failures.iter().any(|f| f.check_name == "identity"));
    assert!(failures.iter().any(|f| f.check_name == "factual_support"));
}

/// R12-AGENCY-6099: a coordination pattern with no listed phrase anywhere
/// in it is still blocked, because the gate reads the pattern rather than
/// the vocabulary. Three shapes, each clean word for word: the same message
/// re-cased and re-punctuated under a second identity (one voice posing as
/// two), the same message flooded above the per-pass rate, and an action
/// whose method is manipulative behind an innocent draft. The cross-pass
/// form of the first is the guard's, proven in
/// `tests/agency.rs::the_same_message_released_under_a_second_identity_in_a_later_pass_is_withheld_as_fake_consensus`.
/// Mutation: in `checks::check_coordinated_voices`, key voices on the raw
/// `content` instead of its `words` — the re-cased copy reads as a
/// different message and the first assertion fails.
#[test]
fn a_coordination_pattern_with_no_listed_phrase_is_still_blocked() {
    use qip_contracts::action::Method;
    let gate = basic_gate();
    let first = compliant_draft();
    let mut second = compliant_draft();
    second.id = "draft-002".to_string();
    second.sender_identity = "exec_speaker".to_string();
    second.content = "QUARTERLY earnings show -- stable growth!".to_string();
    for draft in [&first, &second] {
        assert!(
            gate.can_release(draft),
            "premise: each draft alone breaches no rule and no phrase"
        );
    }
    let failed = gate.campaign_failures(&[&first, &second]);
    assert!(
        failed
            .iter()
            .any(|(id, f)| id == "*" && f.check_name == "coordinated_voices"),
        "{failed:?}"
    );

    let face = |method| StepConduct {
        tool: "press-release",
        method,
        draft: &first,
    };
    let flood = vec![face(Method::Communication); MAX_RELEASES_PER_MESSAGE + 1];
    assert!(
        gate.step_failure(&flood[..MAX_RELEASES_PER_MESSAGE])
            .is_none()
    );
    let refused = gate.step_failure(&flood).expect("a flood is refused");
    assert_eq!(refused.check_name, "step_conduct");
    let refused = gate
        .step_failure(&[face(Method::PumpAndDump)])
        .expect("a manipulative method is refused behind a clean draft");
    assert!(refused.reason.unwrap_or_default().contains("PumpAndDump"));
}
