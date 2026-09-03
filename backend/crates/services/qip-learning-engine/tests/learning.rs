//! Tests for the LEARN stage.
//!
//! The attribution test that matters is that it reconciles exactly; the
//! evaluation tests that matter are the ones about refusing to score early and
//! refusing to count luck as skill.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::testing::approx_eq;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_learning_engine::attribution::{Attributor, PositionPeriod, Source};
use qip_learning_engine::evaluation::{
    EvaluationPolicy, Outcome, ThesisClaim, ThesisEvaluator, Verdict,
};
use qip_learning_engine::feedback::{FeedbackEngine, LessonCandidate, PromotionBar};
use std::collections::BTreeMap;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn end() -> Timestamp {
    start().saturating_add(Duration::from_days(30))
}

// --- attribution ------------------------------------------------------------

fn position(symbol: &str, opening: &str, closing: &str) -> PositionPeriod {
    PositionPeriod {
        object_id: format!("obj-{symbol}"),
        hypotheses: vec![format!("hyp-{symbol}")],
        opening_quantity: dec!("10000"),
        opening_price: Decimal::parse(opening).unwrap(),
        closing_quantity: dec!("10000"),
        closing_price: Decimal::parse(closing).unwrap(),
        decision_price: Decimal::parse(opening).unwrap(),
        traded_quantity: Decimal::ZERO,
        traded_price: Decimal::ZERO,
        commission: dec!("100"),
        spread_cost: dec!("300"),
        impact_cost: dec!("200"),
        income: dec!("500"),
        financing: dec!("50"),
        realised_pnl: Decimal::ZERO,
        factor_returns: BTreeMap::from([("market".to_string(), 0.02)]),
        factor_betas: BTreeMap::from([("market".to_string(), 1.1)]),
        contract_multiplier: Decimal::from_int(1),
    }
}

/// Total P&L computed the way the books would, so the attribution has
/// something real to reconcile against.
fn total_of(positions: &[PositionPeriod]) -> Decimal {
    positions
        .iter()
        .map(|p| {
            let opening = p.opening_quantity * p.opening_price * p.contract_multiplier;
            let closing = p.closing_quantity * p.closing_price * p.contract_multiplier;
            let traded = p.traded_quantity * p.traded_price * p.contract_multiplier;
            closing - opening - traded + p.income
                - p.financing
                - p.commission
                - p.spread_cost
                - p.impact_cost
        })
        .fold(Decimal::ZERO, |a, b| a + b)
}

#[test]
fn an_attribution_reconciles_exactly() -> Result<()> {
    // The whole value of the module. An attribution that nearly adds up hides
    // exactly the thing nobody understood.
    let positions = vec![
        position("AAA", "100", "105"),
        position("BBB", "50", "48"),
        position("CCC", "200", "200"),
    ];
    let attribution = Attributor::new().attribute(
        &positions,
        total_of(&positions),
        dec!("10000000"),
        start(),
        end(),
    )?;

    assert!(attribution.reconciles());
    assert_eq!(attribution.residual(), Decimal::ZERO);
    assert!(attribution.validate().is_ok());
    Ok(())
}

#[test]
fn every_position_decomposes_into_components_that_sum_to_its_total() -> Result<()> {
    let positions = vec![position("AAA", "100", "108")];
    let attribution = Attributor::new().attribute(
        &positions,
        total_of(&positions),
        dec!("1000000"),
        start(),
        end(),
    )?;

    for entry in &attribution.positions {
        let sum = entry.components.values().fold(Decimal::ZERO, |a, b| a + *b);
        assert_eq!(sum, entry.total, "{} does not decompose", entry.object_id);
    }
    Ok(())
}

#[test]
fn an_attribution_that_does_not_reconcile_is_refused() {
    // Passing a total that does not match the positions is exactly what a bug
    // upstream would look like, and it must not produce a plausible report.
    let positions = vec![position("AAA", "100", "105")];
    let wrong_total = total_of(&positions) + dec!("50000");
    let error = Attributor::new()
        .attribute(&positions, wrong_total, dec!("1000000"), start(), end())
        .unwrap_err();
    assert!(
        error.message().contains("does not reconcile"),
        "{}",
        error.message()
    );
    assert!(error.message().contains("unexplained"));
}

#[test]
fn costs_are_negative_contributions() -> Result<()> {
    let positions = vec![position("AAA", "100", "100")];
    let attribution = Attributor::new().attribute(
        &positions,
        total_of(&positions),
        dec!("1000000"),
        start(),
        end(),
    )?;

    assert_eq!(attribution.source_total(Source::Commission), dec!("-100"));
    assert_eq!(attribution.source_total(Source::Spread), dec!("-300"));
    assert_eq!(attribution.source_total(Source::MarketImpact), dec!("-200"));
    assert_eq!(attribution.source_total(Source::Carry), dec!("500"));
    assert_eq!(attribution.source_total(Source::Financing), dec!("-50"));
    assert_eq!(attribution.implementation_cost(), dec!("-600"));
    Ok(())
}

#[test]
fn implementation_shortfall_measures_the_gap_from_the_decision_price() -> Result<()> {
    // Buying above the price the decision was made at is a cost, whatever the
    // position later does.
    let mut period = position("AAA", "100", "100");
    period.decision_price = dec!("100");
    period.traded_quantity = dec!("1000");
    period.traded_price = dec!("102");
    period.closing_quantity = dec!("11000");

    let positions = vec![period];
    let attribution = Attributor::new().attribute(
        &positions,
        total_of(&positions),
        dec!("1000000"),
        start(),
        end(),
    )?;

    // 1000 shares bought 2 above the decision price is 2000 of shortfall.
    assert_eq!(
        attribution.source_total(Source::ImplementationShortfall),
        dec!("-2000")
    );
    assert!(attribution.reconciles());
    Ok(())
}

#[test]
fn attribution_joins_pnl_to_the_hypotheses_that_produced_it() -> Result<()> {
    // The join that makes learning possible.
    let positions = vec![position("AAA", "100", "110"), position("BBB", "50", "45")];
    let attribution = Attributor::new().attribute(
        &positions,
        total_of(&positions),
        dec!("1000000"),
        start(),
        end(),
    )?;

    let by_hypothesis = attribution.by_hypothesis();
    assert_eq!(by_hypothesis.len(), 2);
    assert!(by_hypothesis["hyp-AAA"] > Decimal::ZERO);
    assert!(by_hypothesis["hyp-BBB"] < Decimal::ZERO);
    Ok(())
}

#[test]
fn a_position_expressing_several_theses_splits_its_pnl_evenly() -> Result<()> {
    // Any other split needs a weighting nobody recorded at the time.
    let mut period = position("AAA", "100", "110");
    period.hypotheses = vec!["hyp-1".to_string(), "hyp-2".to_string()];
    let positions = vec![period];
    let attribution = Attributor::new().attribute(
        &positions,
        total_of(&positions),
        dec!("1000000"),
        start(),
        end(),
    )?;

    let by_hypothesis = attribution.by_hypothesis();
    assert_eq!(by_hypothesis["hyp-1"], by_hypothesis["hyp-2"]);
    assert_eq!(
        by_hypothesis["hyp-1"] + by_hypothesis["hyp-2"],
        attribution.positions[0].total
    );
    Ok(())
}

#[test]
fn positions_are_ranked_worst_first() -> Result<()> {
    let positions = vec![
        position("AAA", "100", "110"),
        position("BBB", "100", "80"),
        position("CCC", "100", "101"),
    ];
    let attribution = Attributor::new().attribute(
        &positions,
        total_of(&positions),
        dec!("10000000"),
        start(),
        end(),
    )?;
    let worst = attribution.worst(1);
    assert_eq!(worst[0].object_id, "obj-BBB");
    Ok(())
}

#[test]
fn a_period_that_ends_before_it_starts_is_refused() {
    assert!(
        Attributor::new()
            .attribute(&[], Decimal::ZERO, dec!("1000000"), end(), start())
            .is_err()
    );
}

#[test]
fn a_non_finite_factor_return_is_refused_rather_than_silently_costed_as_zero() {
    // A NaN factor return must not be swallowed to zero and reappear,
    // unexplained, inside the idiosyncratic residual: that is exactly the
    // failure mode `Attribution::residual` exists to catch, reintroduced
    // through the one term that still touches a float before crossing back
    // to `Decimal`. Premise first: a finite return does attribute cleanly.
    let mut clean = position("AAA", "100", "105");
    clean.factor_returns = BTreeMap::from([("market".to_string(), 0.02)]);
    assert!(
        Attributor::new()
            .attribute(
                &[clean.clone()],
                total_of(&[clean.clone()]),
                dec!("1000000"),
                start(),
                end(),
            )
            .is_ok(),
        "the fixture must attribute cleanly before proving the NaN case is refused"
    );

    let mut poisoned = clean;
    poisoned.factor_returns = BTreeMap::from([("market".to_string(), f64::NAN)]);
    let error = Attributor::new()
        .attribute(
            &[poisoned.clone()],
            total_of(&[poisoned]),
            dec!("1000000"),
            start(),
            end(),
        )
        .unwrap_err();
    assert!(
        error.message().contains("does not convert to a decimal"),
        "{}",
        error.message()
    );
}

// --- thesis evaluation ------------------------------------------------------

fn claim(id: &str, expected_bps: f64, confidence: f64) -> ThesisClaim {
    ThesisClaim {
        hypothesis_id: id.to_string(),
        class: "funding-cost-pass-through".to_string(),
        subject: "obj-AAA".to_string(),
        formed_at: start(),
        resolves_at: end(),
        direction: 1.0,
        expected_move_bps: expected_bps,
        confidence,
        falsifiers: vec!["margin holds flat".to_string()],
        contributors: vec!["credit-analyst".to_string()],
    }
}

fn outcome(id: &str, realised_bps: f64) -> Outcome {
    Outcome {
        hypothesis_id: id.to_string(),
        observed_at: end(),
        realised_move_bps: realised_bps,
        realised_pnl: realised_bps * 100.0,
        falsifiers_triggered: Vec::new(),
        mechanism_confirmed: Some(true),
    }
}

#[test]
fn a_thesis_cannot_be_scored_before_its_horizon_closes() {
    // The easiest way to manufacture a track record.
    let evaluator = ThesisEvaluator::default();
    let too_early = start().saturating_add(Duration::from_days(10));
    let error = evaluator
        .evaluate(
            &claim("hyp-1", 100.0, 0.7),
            &outcome("hyp-1", 200.0),
            too_early,
        )
        .unwrap_err();
    assert!(
        error.message().contains("invent a track record"),
        "{}",
        error.message()
    );
}

#[test]
fn an_outcome_observed_before_the_horizon_closed_is_refused() {
    let evaluator = ThesisEvaluator::default();
    let mut early = outcome("hyp-1", 200.0);
    early.observed_at = start().saturating_add(Duration::from_days(5));
    assert!(
        evaluator
            .evaluate(&claim("hyp-1", 100.0, 0.7), &early, end())
            .unwrap_err()
            .message()
            .contains("before the horizon closed")
    );
}

#[test]
fn a_thesis_right_about_direction_and_magnitude_is_vindicated() -> Result<()> {
    let evaluator = ThesisEvaluator::default();
    let evaluation =
        evaluator.evaluate(&claim("hyp-1", 100.0, 0.7), &outcome("hyp-1", 120.0), end())?;
    assert_eq!(evaluation.verdict, Verdict::Vindicated);
    assert!(evaluation.verdict.counts_as_correct());
    assert!(approx_eq(evaluation.magnitude_ratio, 1.2, 1e-9));
    Ok(())
}

#[test]
fn a_thesis_wrong_about_magnitude_by_a_factor_of_ten_is_not_a_good_thesis() -> Result<()> {
    // Scoring on direction alone is how a platform convinces itself it is
    // skilled.
    let evaluator = ThesisEvaluator::default();
    let evaluation = evaluator.evaluate(
        &claim("hyp-1", 100.0, 0.9),
        &outcome("hyp-1", 1000.0),
        end(),
    )?;
    assert_eq!(evaluation.verdict, Verdict::DirectionallyRight);
    assert_ne!(evaluation.verdict, Verdict::Vindicated);
    assert!(evaluation.rationale.contains("not a good thesis"));
    Ok(())
}

#[test]
fn a_thesis_that_was_right_for_the_wrong_reason_does_not_count_as_correct() -> Result<()> {
    // Treating luck as skill reinforces the reasoning that produced it.
    let evaluator = ThesisEvaluator::default();
    let mut lucky = outcome("hyp-1", 110.0);
    lucky.mechanism_confirmed = Some(false);

    let evaluation = evaluator.evaluate(&claim("hyp-1", 100.0, 0.8), &lucky, end())?;
    assert_eq!(evaluation.verdict, Verdict::RightForTheWrongReason);
    assert!(!evaluation.verdict.counts_as_correct());
    assert!(!evaluation.verdict.reasoning_held());
    assert!(evaluation.verdict.is_informative());
    Ok(())
}

#[test]
fn a_falsifier_firing_is_the_strongest_refutation() -> Result<()> {
    let evaluator = ThesisEvaluator::default();
    let mut refuted = outcome("hyp-1", 150.0);
    refuted.falsifiers_triggered = vec!["margin holds flat".to_string()];

    // Even with the move going the right way, the thesis named this in advance
    // as what would make it wrong.
    let evaluation = evaluator.evaluate(&claim("hyp-1", 100.0, 0.8), &refuted, end())?;
    assert_eq!(evaluation.verdict, Verdict::Falsified);
    assert!(!evaluation.verdict.counts_as_correct());
    assert!(evaluation.rationale.contains("named"));
    Ok(())
}

#[test]
fn a_move_below_the_noise_floor_is_inconclusive_rather_than_a_win() -> Result<()> {
    // Without a floor, a one-basis-point move in the right direction counts as
    // a correct call and the hit rate becomes a coin flip dressed up.
    let evaluator = ThesisEvaluator::default();
    let evaluation =
        evaluator.evaluate(&claim("hyp-1", 100.0, 0.7), &outcome("hyp-1", 5.0), end())?;
    assert_eq!(evaluation.verdict, Verdict::Inconclusive);
    assert!(!evaluation.verdict.counts_as_correct());
    assert!(!evaluation.verdict.is_informative());
    Ok(())
}

#[test]
fn a_move_the_wrong_way_is_wrong() -> Result<()> {
    let evaluator = ThesisEvaluator::default();
    let evaluation = evaluator.evaluate(
        &claim("hyp-1", 100.0, 0.9),
        &outcome("hyp-1", -150.0),
        end(),
    )?;
    assert_eq!(evaluation.verdict, Verdict::Wrong);
    assert!(evaluation.was_overconfident());
    assert!(approx_eq(evaluation.brier_component(), 0.81, 1e-9));
    Ok(())
}

#[test]
fn an_outcome_for_a_different_thesis_is_refused() {
    let evaluator = ThesisEvaluator::default();
    assert!(
        evaluator
            .evaluate(&claim("hyp-1", 100.0, 0.7), &outcome("hyp-2", 120.0), end())
            .is_err()
    );
}

#[test]
fn a_batch_reports_what_it_could_not_score() {
    // An unscored thesis must be visible rather than silently absent.
    let evaluator = ThesisEvaluator::new(EvaluationPolicy::default());
    let claims = vec![claim("hyp-1", 100.0, 0.7), claim("hyp-2", 100.0, 0.7)];
    let outcomes = vec![outcome("hyp-1", 120.0)];

    let (evaluations, skipped) = evaluator.evaluate_all(&claims, &outcomes, end());
    assert_eq!(evaluations.len(), 1);
    assert_eq!(skipped, vec!["hyp-2".to_string()]);
}

// --- feedback ---------------------------------------------------------------

/// A batch of evaluations with a stated correct fraction and confidence.
fn batch(
    count: usize,
    correct: usize,
    confidence: f64,
    agents: &[&str],
) -> Vec<qip_learning_engine::evaluation::Evaluation> {
    let evaluator = ThesisEvaluator::default();
    (0..count)
        .map(|i| {
            let id = format!("hyp-{i}");
            let mut c = claim(&id, 100.0, confidence);
            c.contributors = vec![agents[i % agents.len()].to_string()];
            let realised = if i < correct { 110.0 } else { -110.0 };
            evaluator
                .evaluate(&c, &outcome(&id, realised), end())
                .expect("scoreable")
        })
        .collect()
}

#[test]
fn calibration_notices_systematic_overconfidence() -> Result<()> {
    let engine = FeedbackEngine::default();
    // Claiming 0.9 confidence and being right 40% of the time.
    let report = engine.calibrate(&batch(100, 40, 0.9, &["credit-analyst"]))?;

    assert!(report.is_overconfident, "{}", report.summarise());
    assert!(report.confidence_adjustment < 1.0);
    assert!(report.is_material());
    Ok(())
}

#[test]
fn calibration_does_not_flag_a_well_calibrated_platform() -> Result<()> {
    let engine = FeedbackEngine::default();
    // Claiming 0.7 and being right about 70% of the time.
    let report = engine.calibrate(&batch(100, 70, 0.7, &["credit-analyst"]))?;
    assert!(!report.is_overconfident, "{}", report.summarise());
    Ok(())
}

#[test]
fn a_rate_from_too_little_history_is_withheld_rather_than_reported() -> Result<()> {
    // Reporting a rate from four episodes would mislead.
    let engine = FeedbackEngine::default();
    let report = engine.calibrate(&batch(4, 2, 0.7, &["credit-analyst"]))?;
    assert!(report.hit_rate_by_agent.is_empty());
    assert!(!report.withheld.is_empty());
    assert!(report.withheld.iter().any(|w| w.contains("credit-analyst")));
    Ok(())
}

#[test]
fn a_small_calibration_adjustment_is_not_treated_as_material() -> Result<()> {
    // Applying one would make the platform chase its own sampling error.
    let engine = FeedbackEngine::default();
    let report = engine.calibrate(&batch(60, 40, 0.68, &["credit-analyst"]))?;
    assert!(
        !report.is_material(),
        "adjustment {} from {} evaluations should not be material",
        report.confidence_adjustment,
        report.evaluated
    );
    Ok(())
}

#[test]
fn a_lesson_from_one_agent_does_not_clear_the_bar() {
    // A pattern seen only through one agent's theses is a fact about that
    // agent.
    let bar = PromotionBar::default();
    let candidate = LessonCandidate {
        applies_when: "class=x".to_string(),
        statement: "x always works".to_string(),
        supporting: (0..12).map(|i| format!("hyp-{i}")).collect(),
        contradicting: Vec::new(),
        distinct_contributors: 1,
        proposed_at: end(),
    };
    let refusal = bar.assess(&candidate).unwrap_err();
    assert!(refusal.contains("fact about that agent"), "{refusal}");
}

#[test]
fn a_lesson_from_too_few_episodes_does_not_clear_the_bar() {
    let bar = PromotionBar::default();
    let candidate = LessonCandidate {
        applies_when: String::new(),
        statement: "it always works".to_string(),
        supporting: vec!["hyp-1".to_string(), "hyp-2".to_string()],
        contradicting: Vec::new(),
        distinct_contributors: 3,
        proposed_at: end(),
    };
    let refusal = bar.assess(&candidate).unwrap_err();
    assert!(
        refusal.contains("coincidence with a narrative attached"),
        "{refusal}"
    );
}

#[test]
fn a_well_supported_lesson_from_several_agents_clears_the_bar() {
    let bar = PromotionBar::default();
    let candidate = LessonCandidate {
        applies_when: "regime=stressed".to_string(),
        statement: "credit leads equity by two days in a stressed regime".to_string(),
        supporting: (0..10).map(|i| format!("hyp-{i}")).collect(),
        contradicting: vec!["hyp-x".to_string()],
        distinct_contributors: 3,
        proposed_at: end(),
    };
    assert!(bar.assess(&candidate).is_ok());
    assert!(candidate.smoothed_support() > 0.7);
    assert_eq!(candidate.episode_count(), 11);
}

#[test]
fn smoothing_stops_a_perfect_short_record_reading_as_certainty() {
    let candidate = LessonCandidate {
        applies_when: String::new(),
        statement: "works so far".to_string(),
        supporting: (0..3).map(|i| format!("hyp-{i}")).collect(),
        contradicting: Vec::new(),
        distinct_contributors: 2,
        proposed_at: end(),
    };
    assert!(approx_eq(candidate.support(), 1.0, 1e-12));
    assert!(approx_eq(candidate.smoothed_support(), 0.8, 1e-12));
}

#[test]
fn theses_right_for_the_wrong_reason_produce_a_lesson() -> Result<()> {
    // If this is common the platform is being rewarded for reasoning that does
    // not work, which is worse than being wrong.
    let evaluator = ThesisEvaluator::default();
    let mut evaluations = Vec::new();
    for i in 0..5 {
        let id = format!("hyp-lucky-{i}");
        let mut c = claim(&id, 100.0, 0.8);
        c.contributors = vec![format!("agent-{}", i % 3)];
        let mut lucky = outcome(&id, 110.0);
        lucky.mechanism_confirmed = Some(false);
        evaluations.push(evaluator.evaluate(&c, &lucky, end())?);
    }

    let candidates = FeedbackEngine::default().propose_lessons(&evaluations, end());
    assert!(
        candidates
            .iter()
            .any(|c| c.statement.contains("right for the wrong reason")),
        "{candidates:?}"
    );
    Ok(())
}

#[test]
fn falsifiers_that_keep_firing_produce_a_lesson() -> Result<()> {
    let evaluator = ThesisEvaluator::default();
    let mut evaluations = Vec::new();
    for i in 0..4 {
        let id = format!("hyp-refuted-{i}");
        let mut c = claim(&id, 100.0, 0.8);
        c.contributors = vec![format!("agent-{}", i % 2)];
        let mut refuted = outcome(&id, 120.0);
        refuted.falsifiers_triggered = vec!["margin holds flat".to_string()];
        evaluations.push(evaluator.evaluate(&c, &refuted, end())?);
    }

    let candidates = FeedbackEngine::default().propose_lessons(&evaluations, end());
    assert!(
        candidates
            .iter()
            .any(|c| c.statement.contains("named in advance")),
        "{candidates:?}"
    );
    Ok(())
}

#[test]
fn an_agent_with_a_poor_record_is_discounted_but_never_silenced() -> Result<()> {
    // An agent that is reliably wrong still carries information.
    let engine = FeedbackEngine::default();
    let report = engine.process(&batch(60, 12, 0.6, &["poor-analyst"]), end())?;

    let adjustment = report
        .agent_weight_adjustments
        .get("poor-analyst")
        .copied()
        .expect("a poor record should be discounted");
    assert!(adjustment < 1.0);
    assert!(
        adjustment >= 0.3,
        "silencing discards information: {adjustment}"
    );
    Ok(())
}

#[test]
fn a_feedback_pass_reports_whether_anything_should_change() -> Result<()> {
    let engine = FeedbackEngine::default();
    let quiet = engine.process(&batch(60, 42, 0.7, &["credit-analyst"]), end())?;
    assert!(!quiet.has_actions(), "{}", quiet.calibration.summarise());

    let alarming = engine.process(&batch(100, 30, 0.9, &["credit-analyst"]), end())?;
    assert!(alarming.has_actions());
    Ok(())
}

#[test]
fn a_rejected_lesson_says_why() -> Result<()> {
    let engine = FeedbackEngine::new(PromotionBar {
        minimum_support: 100,
        ..PromotionBar::default()
    });
    let evaluator = ThesisEvaluator::default();
    let mut evaluations = Vec::new();
    for i in 0..5 {
        let id = format!("hyp-lucky-{i}");
        let mut c = claim(&id, 100.0, 0.8);
        c.contributors = vec![format!("agent-{}", i % 3)];
        let mut lucky = outcome(&id, 110.0);
        lucky.mechanism_confirmed = Some(false);
        evaluations.push(evaluator.evaluate(&c, &lucky, end())?);
    }

    let report = engine.process(&evaluations, end())?;
    assert!(report.promoted.is_empty());
    assert!(!report.rejected.is_empty());
    assert!(!report.rejected[0].1.is_empty(), "a refusal must say why");
    Ok(())
}
