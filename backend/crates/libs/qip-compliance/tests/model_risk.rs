//! Control 3 — model risk and explainability.
//!
//! The tests try to get a number out of a model and into a decision: without a
//! risk file, with an overdue one, outside the range the model was validated
//! over, and with an explanation whose arithmetic does not hold.

#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::{EvaluationRecord, ModelCard, ModelRegistry};
use qip_compliance::model_risk::{
    Contribution, Explanation, ModelRiskFile, ModelRiskRegister, PerformanceBoundary,
    ValidationEvidence, ValidationKind,
};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, HypothesisId, ModelId, Timestamp, dec};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

const REFERENCE: &str = "vol-forecast@2.1.0";

/// A model that passes every check `qip_ai` makes: production, evaluated
/// recently, undrifted. Model *risk* is a separate question from here on.
fn eligible_model() -> Result<ModelRegistry> {
    let mut registry = ModelRegistry::new();
    let card = ModelCard::new(
        ModelId::from_string("m-vol-forecast"),
        "vol-forecast",
        "2.1.0",
        "quant-research",
        now().saturating_sub(Duration::from_days(200)),
    )
    .with_purpose("forecast realised volatility one day ahead");
    registry.register(card);
    registry.record_evaluation(
        REFERENCE,
        EvaluationRecord {
            evaluated_at: now().saturating_sub(Duration::from_days(10)),
            dataset: "holdout.2024".to_string(),
            metrics: Default::default(),
            passed: true,
        },
    )?;
    registry.promote(REFERENCE, now().saturating_sub(Duration::from_days(5)))?;
    Ok(registry)
}

fn evidence() -> Result<Vec<ValidationEvidence>> {
    Ok(vec![
        ValidationEvidence::new(
            ValidationKind::HoldOut,
            "holdout.2024",
            "j.okafor",
            now().saturating_sub(Duration::from_days(30)),
            "root mean squared error within the acceptance band on held-out data",
        )?,
        ValidationEvidence::new(
            ValidationKind::IndependentReview,
            "holdout.2024",
            "model-risk-committee",
            now().saturating_sub(Duration::from_days(25)),
            "reviewed by somebody who did not build it; approved with limitations",
        )?,
    ])
}

/// A risk file due for review a month from now.
fn current_risk_file() -> Result<ModelRiskFile> {
    ModelRiskFile::open(
        REFERENCE,
        "one-day-ahead realised volatility for liquid single names",
        "quant-research",
        now().saturating_sub(Duration::from_days(30)),
        now().saturating_add(Duration::from_days(30)),
        vec!["degrades in the first hour after an earnings release".to_string()],
        evidence()?,
        vec![PerformanceBoundary::new(
            "adv_participation",
            Some(Decimal::ZERO),
            Some(dec!("0.05")),
        )?],
    )
}

/// An explanation that reconciles: 0.10 baseline + 0.05 + 0.03 = 0.18.
fn honest_explanation() -> Result<Explanation> {
    Explanation::reconciled(
        REFERENCE,
        dec!("0.18"),
        dec!("0.10"),
        vec![
            Contribution {
                input: "adv_participation".to_string(),
                value: dec!("0.01"),
                contribution: dec!("0.05"),
            },
            Contribution {
                input: "realised_vol_5d".to_string(),
                value: dec!("0.22"),
                contribution: dec!("0.03"),
            },
        ],
        now(),
        None,
    )
}

#[test]
fn a_model_without_a_risk_file_cannot_drive_a_decision() -> Result<()> {
    // The model is impeccable by `qip_ai`'s standards — production, evaluated,
    // undrifted — and still refused, because nobody has recorded what it is
    // for, what it is bad at, or who validated it.
    let models = eligible_model()?;
    assert!(models.require_for_decision(REFERENCE, now()).is_ok());

    let mut register = ModelRiskRegister::new();
    let error = register
        .admit(&models, honest_explanation()?, now())
        .expect_err("a model with no risk file must not drive a decision");
    assert!(error.message().contains(REFERENCE));
    assert!(error.message().contains("risk file"));
    Ok(())
}

#[test]
fn an_overdue_review_blocks_use_until_somebody_reviews_it() -> Result<()> {
    let models = eligible_model()?;
    let mut register = ModelRiskRegister::new();
    register.file(current_risk_file()?);

    // Today it is fine.
    assert!(
        register
            .admit(&models, honest_explanation()?, now())
            .is_ok()
    );

    // Two months on, the review has fallen due. Nothing about the model
    // changed; the obligation to look at it did.
    let later = now().saturating_add(Duration::from_days(60));
    let error = register
        .admit(&models, honest_explanation()?, later)
        .expect_err("an overdue risk file must block use");
    assert!(error.message().contains("due for review"));
    assert_eq!(register.overdue(later).len(), 1);

    // A review with a named reviewer and a finding restores it.
    let file = register
        .get_mut(REFERENCE)
        .ok_or_else(|| qip_core::error::Error::not_found("risk file"))?;
    file.reviewed(
        "model-risk-committee",
        later,
        later.saturating_add(Duration::from_days(180)),
        "re-validated against the last six months; limitations unchanged",
    )?;
    assert!(
        register
            .admit(&models, honest_explanation()?, later)
            .is_ok()
    );
    Ok(())
}

#[test]
fn a_review_that_names_nobody_or_finds_nothing_is_refused() -> Result<()> {
    let mut file = current_risk_file()?;
    let next = now().saturating_add(Duration::from_days(180));

    assert!(
        file.reviewed("", now(), next, "everything is fine here")
            .is_err()
    );
    assert!(file.reviewed("a.reviewer", now(), next, "ok").is_err());
    // The due date must actually move forward.
    assert!(
        file.reviewed("a.reviewer", now(), now(), "reviewed and unchanged")
            .is_err()
    );
    Ok(())
}

#[test]
fn an_explanation_whose_contributions_do_not_reconcile_cannot_be_constructed() {
    // 0.10 + 0.05 + 0.03 is 0.18, not 0.25. The value does not exist, so no
    // reviewer can be shown arithmetic that was never done.
    let error = Explanation::reconciled(
        REFERENCE,
        dec!("0.25"),
        dec!("0.10"),
        vec![
            Contribution {
                input: "adv_participation".to_string(),
                value: dec!("0.01"),
                contribution: dec!("0.05"),
            },
            Contribution {
                input: "realised_vol_5d".to_string(),
                value: dec!("0.22"),
                contribution: dec!("0.03"),
            },
        ],
        now(),
        None,
    )
    .expect_err("an explanation that does not add up must be refused");

    assert!(error.message().contains("does not reconcile"));
    // The residual is stated, because the fix is to find the missing 0.07.
    assert!(error.message().contains("0.07"));
}

#[test]
fn an_explanation_with_no_inputs_explains_nothing_and_is_refused() {
    let error =
        Explanation::reconciled(REFERENCE, Decimal::ZERO, Decimal::ZERO, vec![], now(), None)
            .expect_err("an explanation naming no inputs is not an explanation");
    assert!(error.message().contains("at least one input"));
}

#[test]
fn a_model_used_outside_its_validated_range_is_refused() -> Result<()> {
    // The risk file says the model was validated up to 5% participation. The
    // output below was produced at 40%, where nobody has measured it.
    let models = eligible_model()?;
    let mut register = ModelRiskRegister::new();
    register.file(current_risk_file()?);

    let out_of_range = Explanation::reconciled(
        REFERENCE,
        dec!("0.18"),
        dec!("0.10"),
        vec![
            Contribution {
                input: "adv_participation".to_string(),
                value: dec!("0.40"),
                contribution: dec!("0.05"),
            },
            Contribution {
                input: "realised_vol_5d".to_string(),
                value: dec!("0.22"),
                contribution: dec!("0.03"),
            },
        ],
        now(),
        None,
    )?;

    let error = register
        .admit(&models, out_of_range, now())
        .expect_err("a model used outside its validated range must be refused");
    assert!(error.message().contains("adv_participation"));
    assert!(error.message().contains("actual 0.4"));
    Ok(())
}

#[test]
fn a_retired_model_cannot_drive_a_decision_however_good_its_risk_file() -> Result<()> {
    // Model risk composes with `qip_ai`'s eligibility rather than replacing it.
    let mut models = eligible_model()?;
    let mut register = ModelRiskRegister::new();
    register.file(current_risk_file()?);
    models.retire(REFERENCE, now())?;

    let error = register
        .admit(&models, honest_explanation()?, now())
        .expect_err("a retired model must not drive a decision");
    assert!(error.message().contains(REFERENCE));
    Ok(())
}

#[test]
fn a_risk_file_missing_what_a_reviewer_needs_cannot_be_opened() -> Result<()> {
    let due = now().saturating_add(Duration::from_days(30));
    let approved = now().saturating_sub(Duration::from_days(30));

    // No stated limitations: a model with none has been described, not reviewed.
    assert!(
        ModelRiskFile::open(
            REFERENCE,
            "one-day-ahead realised volatility for liquid single names",
            "quant-research",
            approved,
            due,
            vec![],
            evidence()?,
            vec![],
        )
        .is_err()
    );
    // No validation evidence: an opinion.
    assert!(
        ModelRiskFile::open(
            REFERENCE,
            "one-day-ahead realised volatility for liquid single names",
            "quant-research",
            approved,
            due,
            vec!["degrades after earnings".to_string()],
            vec![],
            vec![],
        )
        .is_err()
    );
    // No review date in the future: a file that never falls due.
    assert!(
        ModelRiskFile::open(
            REFERENCE,
            "one-day-ahead realised volatility for liquid single names",
            "quant-research",
            approved,
            approved,
            vec!["degrades after earnings".to_string()],
            evidence()?,
            vec![],
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn every_admission_decision_is_recorded_whichever_way_it_went() -> Result<()> {
    let models = eligible_model()?;
    let mut register = ModelRiskRegister::new();

    let _ = register.admit(&models, honest_explanation()?, now());
    register.file(current_risk_file()?);
    let admitted = register.admit(&models, honest_explanation()?, now())?;

    assert_eq!(admitted.output(), dec!("0.18"));
    assert_eq!(admitted.model_reference(), REFERENCE);
    // The drivers are ordered by how much they moved the number.
    assert_eq!(
        admitted.explanation().drivers()[0].input,
        "adv_participation"
    );

    assert_eq!(register.admissions().len(), 2);
    assert!(!register.admissions()[0].admitted);
    assert!(register.admissions()[1].admitted);
    Ok(())
}

/// §43.3 hop: an explanation may name the claim/hypothesis whose belief
/// produced its inputs. Built with a known id, `reference()` — here
/// `upstream()` — must return exactly that id, and the id must survive a
/// serde round trip rather than being an in-memory-only convenience.
#[test]
fn an_explanation_built_with_a_known_upstream_reference_returns_it_and_survives_the_serde_round_trip()
-> Result<()> {
    let upstream = HypothesisId::from_string("hyp-vol-regime-shift");
    let explanation = Explanation::reconciled(
        REFERENCE,
        dec!("0.18"),
        dec!("0.10"),
        vec![
            Contribution {
                input: "adv_participation".to_string(),
                value: dec!("0.01"),
                contribution: dec!("0.05"),
            },
            Contribution {
                input: "realised_vol_5d".to_string(),
                value: dec!("0.22"),
                contribution: dec!("0.03"),
            },
        ],
        now(),
        Some(upstream.clone()),
    )?;

    // Premise first: the id we handed in must actually be present before
    // checking it is the exact one that comes back.
    assert!(explanation.upstream().is_some());
    assert_eq!(explanation.upstream(), Some(&upstream));

    let wire = serde_json::to_string(&explanation)
        .map_err(|e| qip_core::error::Error::invalid(format!("did not serialise: {e}")))?;
    let restored: Explanation = serde_json::from_str(&wire)
        .map_err(|e| qip_core::error::Error::invalid(format!("did not deserialise: {e}")))?;

    assert_eq!(restored.upstream(), Some(&upstream));
    // The whole value round-trips exactly, not merely the field under test.
    assert_eq!(restored, explanation);
    Ok(())
}

/// An explanation built with no upstream cause must report `None`, not
/// merely "whatever the type defaults to" — the constructor takes the
/// argument, so `None` here means somebody looked and found none.
#[test]
fn an_explanation_built_with_an_explicit_none_upstream_reports_none() -> Result<()> {
    let explanation = honest_explanation()?;
    assert_eq!(explanation.upstream(), None);
    Ok(())
}

/// Carrying an upstream reference is additive: it must not relax the
/// reconciliation check that already refuses arithmetic that does not add
/// up. 0.10 + 0.05 + 0.03 is 0.18, not 0.25, with or without a claim behind
/// the inputs.
#[test]
fn an_explanation_with_an_upstream_reference_that_does_not_reconcile_is_still_refused() {
    let upstream = HypothesisId::from_string("hyp-vol-regime-shift");
    let error = Explanation::reconciled(
        REFERENCE,
        dec!("0.25"),
        dec!("0.10"),
        vec![
            Contribution {
                input: "adv_participation".to_string(),
                value: dec!("0.01"),
                contribution: dec!("0.05"),
            },
            Contribution {
                input: "realised_vol_5d".to_string(),
                value: dec!("0.22"),
                contribution: dec!("0.03"),
            },
        ],
        now(),
        Some(upstream),
    )
    .expect_err("carrying an upstream reference must not relax the reconciliation check");

    assert!(error.message().contains("does not reconcile"));
}
