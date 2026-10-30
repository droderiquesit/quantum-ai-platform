//! A model is accepted only in the regimes it was validated in, and only on
//! inputs inside the range it was fitted on (R12-MODEL-4020, R12-RISK-4099).

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_ai::registry::{EvaluationRecord, ModelCard, ModelRegistry, ModelStage};
use qip_core::{ModelId, Timestamp};
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

const CALM: &str = "bull/low";
const STRESSED: &str = "bear/high";

fn production_card() -> ModelCard {
    let mut card = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000077"),
        "scoped",
        "1.0.0",
        "quant-research",
        now(),
    )
    .with_features(vec!["momentum".to_string(), "volatility".to_string()]);
    card.evaluations.push(EvaluationRecord {
        evaluated_at: now(),
        dataset: "held-out".into(),
        metrics: BTreeMap::from([("rmse".to_string(), 0.1)]),
        passed: true,
    });
    card.stage = ModelStage::Production;
    card
}

fn inputs(momentum: f64, volatility: f64) -> BTreeMap<String, f64> {
    BTreeMap::from([
        ("momentum".to_string(), momentum),
        ("volatility".to_string(), volatility),
    ])
}

fn bounded() -> ModelCard {
    production_card()
        .with_validated_regime(CALM)
        .with_training_bounds(BTreeMap::from([
            ("momentum".to_string(), (-0.05, 0.05)),
            ("volatility".to_string(), (0.0, 0.4)),
        ]))
        .expect("a finite range on consumed features")
}

/// Mutation: make `regime_gap` return `None` — the calm-only card is admitted
/// in the stressed regime and the second assertion fails.
#[test]
fn a_model_validated_in_one_regime_is_refused_in_another() {
    let card = bounded();
    let ok = inputs(0.01, 0.2);
    // Premise: the same card, same inputs, in the regime it was scored in.
    assert_eq!(card.decision_eligibility_in(now(), CALM, &ok), Ok(()));
    let refusal = card
        .decision_eligibility_in(now(), STRESSED, &ok)
        .expect_err("never scored in the stressed regime");
    assert!(refusal.contains("validated only in bull/low"), "{refusal}");
    assert!(refusal.contains("never scored in bear/high"), "{refusal}");
}

/// Mutation: replace `value < low || value > high` with `false` — the
/// extrapolating input is admitted and the first refusal assertion fails.
#[test]
fn an_input_outside_the_range_the_model_was_fitted_on_is_refused() {
    let card = bounded();
    assert_eq!(
        card.decision_eligibility_in(now(), CALM, &inputs(0.05, 0.4)),
        Ok(()),
        "premise: the edges of the fitted range are inside it"
    );
    let high = card
        .decision_eligibility_in(now(), CALM, &inputs(0.06, 0.2))
        .expect_err("momentum 0.06 is past the fitted 0.05");
    assert!(high.contains("feature momentum is 0.06"), "{high}");
    let low = card
        .decision_eligibility_in(now(), CALM, &inputs(0.0, -0.1))
        .expect_err("negative volatility was never seen");
    assert!(low.contains("feature volatility is -0.1"), "{low}");
}

/// Mutation: delete the `is_finite` branch — NaN compares false against both
/// bounds and is admitted, so the NaN assertion fails.
#[test]
fn a_non_finite_or_missing_input_is_refused_rather_than_passed_through() {
    let card = bounded();
    let nan = card
        .decision_eligibility_in(now(), CALM, &inputs(f64::NAN, 0.2))
        .expect_err("NaN fails every comparison, which is why it needs its own refusal");
    assert!(nan.contains("not a finite number"), "{nan}");
    let missing = card
        .decision_eligibility_in(
            now(),
            CALM,
            &BTreeMap::from([("momentum".to_string(), 0.0)]),
        )
        .expect_err("volatility is bounded and not supplied");
    assert!(missing.contains("no input to check"), "{missing}");
}

#[test]
fn a_card_recording_no_regime_or_bounds_is_not_restricted_by_them() {
    let card = production_card();
    assert!(card.validated_regimes.is_empty() && card.training_bounds.is_empty());
    assert_eq!(
        card.decision_eligibility_in(now(), STRESSED, &BTreeMap::new()),
        Ok(())
    );
}

#[test]
fn bounds_on_a_feature_the_card_does_not_consume_or_an_inverted_range_are_refused() {
    let unknown = production_card()
        .with_training_bounds(BTreeMap::from([("spread".to_string(), (0.0, 1.0))]));
    assert!(unknown.is_err());
    let inverted = production_card()
        .with_training_bounds(BTreeMap::from([("momentum".to_string(), (1.0, 0.0))]));
    assert!(inverted.is_err());
}

/// Mutation: make `ineligible_in_regime` ignore `regime_gap` — the calm-only
/// production card is not listed in the stressed regime and the listing is
/// empty. And with inputs supplied, an extrapolating input is listed in the
/// regime the card was validated in.
#[test]
fn the_registry_lists_a_production_model_whose_skill_was_measured_elsewhere() {
    let mut registry = ModelRegistry::default();
    registry
        .register(bounded())
        .expect("a registry below its cap admits a card");
    assert!(
        registry.ineligible(now()).is_empty(),
        "premise: eligible on its own terms"
    );
    assert!(
        registry
            .ineligible_in_regime(now(), CALM, |_| None)
            .is_empty()
    );
    let listed = registry.ineligible_in_regime(now(), STRESSED, |_| None);
    assert_eq!(listed.len(), 1);
    assert!(listed[0].1.contains("never scored in bear/high"));
    let wild = inputs(0.5, 0.2);
    let listed = registry.ineligible_in_regime(now(), CALM, |_| Some(&wild));
    assert_eq!(listed.len(), 1);
    assert!(
        listed[0].1.contains("feature momentum is 0.5"),
        "{listed:?}"
    );
    assert!(
        registry
            .require_for_decision_in("scoped@1.0.0", STRESSED, &inputs(0.0, 0.1), now())
            .is_err()
    );
    assert!(
        registry
            .require_for_decision_in("scoped@1.0.0", CALM, &inputs(0.0, 0.1), now())
            .is_ok()
    );
}
