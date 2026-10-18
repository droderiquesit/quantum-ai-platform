//! `ChallengeRequest` and `VerificationResult` (blueprint v12.0 §27,
//! R12-REASON-9032): a verdict is checked against the findings it summarises,
//! and a result against the request it answers.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::challenge::{
    ChallengeRequest, ChallengeRequestFields, ChallengeTarget, ChallengeVerdict, CriticOutcome,
    CriticReport, VerificationResult, VerificationResultFields,
};
use qip_core::Timestamp;

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn target() -> ChallengeTarget {
    ChallengeTarget::Prediction {
        hypothesis_id: "hyp-1".to_string(),
        subject: "obj-AAA".to_string(),
    }
}

fn request_fields() -> ChallengeRequestFields {
    ChallengeRequestFields {
        schema_version: 1,
        request_id: "challenge-1".to_string(),
        target: target(),
        critics: vec!["bull".to_string(), "tail_risk".to_string()],
        requested_by: "kernel/test".to_string(),
        requested_at: at(),
    }
}

fn report(critic: &str, outcome: CriticOutcome) -> CriticReport {
    CriticReport {
        critic: critic.to_string(),
        outcome,
    }
}

fn survived(critic: &str) -> CriticReport {
    report(
        critic,
        CriticOutcome::Survived {
            examined: "looked".to_string(),
        },
    )
}

fn refuted(critic: &str) -> CriticReport {
    report(
        critic,
        CriticOutcome::Refuted {
            reason: "fragile".to_string(),
        },
    )
}

fn result_fields(
    reports: Vec<CriticReport>,
    verdict: ChallengeVerdict,
) -> VerificationResultFields {
    VerificationResultFields {
        schema_version: 1,
        request_id: "challenge-1".to_string(),
        target: target(),
        reports,
        verdict,
        concluded_at: at(),
    }
}

#[test]
fn a_request_and_the_result_that_answers_it_round_trip_and_match() {
    let request = ChallengeRequest::new(request_fields()).unwrap();
    let result = VerificationResult::new(result_fields(
        vec![survived("bull"), refuted("tail_risk")],
        ChallengeVerdict::Refuted {
            by: vec!["tail_risk".to_string()],
        },
    ))
    .unwrap();

    let decoded = VerificationResult::decode(&result.encode().unwrap()).unwrap();

    assert_eq!(decoded, result);
    decoded.answers(&request).unwrap();
}

/// The defect the pair exists to stop: a verdict of `survived` over a
/// refuting finding decodes as a plain struct and releases the forecast the
/// critic held back. Mutation: delete the `Survived if !refuting.is_empty()`
/// arm in `VerificationResultFields::check` — the result is accepted and the
/// first assertion fails. Mutation: delete the `if !refuting.is_empty()` check
/// in the `Unverified` arm — the unverified-over-refutation assertion fails.
#[test]
fn a_verdict_that_contradicts_its_own_reports_is_refused() {
    let over_refutation = result_fields(
        vec![survived("bull"), refuted("tail_risk")],
        ChallengeVerdict::Survived,
    );
    assert!(
        VerificationResult::new(over_refutation).is_err(),
        "survived was accepted over a refuting finding"
    );

    // Unverified over a refuting report: the count (2 examined) is right, so
    // only the `Unverified` arm's refutation check can stop it. Without it a
    // receiver decodes a refuted forecast as merely unverified.
    let unverified_over_refutation = result_fields(
        vec![survived("bull"), refuted("tail_risk")],
        ChallengeVerdict::Unverified { examined: 2 },
    );
    assert!(
        VerificationResult::new(unverified_over_refutation).is_err(),
        "unverified was accepted over a refuting finding"
    );

    // Premise for the rest: the honest versions of the same reports pass.
    assert!(
        VerificationResult::new(result_fields(
            vec![survived("bull"), survived("tail_risk")],
            ChallengeVerdict::Survived,
        ))
        .is_ok()
    );

    let wrong_critic = result_fields(
        vec![survived("bull"), refuted("tail_risk")],
        ChallengeVerdict::Refuted {
            by: vec!["bull".to_string()],
        },
    );
    assert!(
        VerificationResult::new(wrong_critic).is_err(),
        "a refutation was filed against a critic that raised none"
    );

    let no_refutation = result_fields(
        vec![survived("bull")],
        ChallengeVerdict::Refuted {
            by: vec!["bull".to_string()],
        },
    );
    assert!(VerificationResult::new(no_refutation).is_err());

    let miscounted = result_fields(
        vec![survived("bull"), survived("tail_risk")],
        ChallengeVerdict::Unverified { examined: 1 },
    );
    assert!(
        VerificationResult::new(miscounted).is_err(),
        "an examined count that is not the number that examined"
    );
}

#[test]
fn a_result_is_refused_when_it_answers_another_request_or_another_critic() {
    let request = ChallengeRequest::new(request_fields()).unwrap();
    let ok = VerificationResult::new(result_fields(
        vec![survived("bull")],
        ChallengeVerdict::Unverified { examined: 1 },
    ))
    .unwrap();
    ok.answers(&request)
        .expect("premise: the matching result answers");

    let mut other_id = result_fields(
        vec![survived("bull")],
        ChallengeVerdict::Unverified { examined: 1 },
    );
    other_id.request_id = "challenge-2".to_string();
    assert!(
        VerificationResult::new(other_id)
            .unwrap()
            .answers(&request)
            .is_err()
    );

    let mut other_target = result_fields(
        vec![survived("bull")],
        ChallengeVerdict::Unverified { examined: 1 },
    );
    other_target.target = ChallengeTarget::Model {
        reference: "m@1".to_string(),
    };
    assert!(
        VerificationResult::new(other_target)
            .unwrap()
            .answers(&request)
            .is_err()
    );

    let unasked = VerificationResult::new(result_fields(
        vec![survived("manipulation")],
        ChallengeVerdict::Unverified { examined: 1 },
    ))
    .unwrap();
    assert!(
        unasked.answers(&request).is_err(),
        "a report from a critic nobody asked was filed under the request"
    );
}

#[test]
fn a_request_with_no_critic_or_a_repeated_one_and_a_result_with_no_report_are_refused() {
    let mut none = request_fields();
    none.critics.clear();
    assert!(ChallengeRequest::new(none).is_err());

    let mut twice = request_fields();
    twice.critics = vec!["bull".to_string(), "bull".to_string()];
    assert!(ChallengeRequest::new(twice).is_err());

    assert!(
        VerificationResult::new(result_fields(Vec::new(), ChallengeVerdict::Survived)).is_err()
    );
    assert!(
        VerificationResult::new(result_fields(
            vec![survived("bull"), survived("bull")],
            ChallengeVerdict::Survived,
        ))
        .is_err(),
        "a critic answered twice"
    );
}
