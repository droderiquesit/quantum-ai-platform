//! Model classes beyond the specialists: foundation, policy, ensemble and
//! reflex models are recorded with class-specific decision eligibility
//! (R12-EXPAND-6060).

use qip_ai::registry::{
    EvaluationRecord, MIN_SHADOW_ROUNDS, ModelCard, ModelClass, ModelRegistry, ModelStage,
    ResourceBudget, ShadowRecord,
};
use qip_core::{ModelId, Timestamp};
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// A production card with a passing, current evaluation: eligible as
/// `Generic`, so any refusal below is the class rule's.
fn eligible(class: ModelClass) -> ModelCard {
    let mut card = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000099"),
        "student",
        "1.0.0",
        "quant-research",
        now(),
    )
    .with_class(class);
    card.evaluations.push(EvaluationRecord {
        evaluated_at: now(),
        dataset: "held-out".into(),
        metrics: BTreeMap::from([("rmse".to_string(), 0.1)]),
        passed: true,
    });
    card.stage = ModelStage::Production;
    card
}

/// Mutation: make `ModelClass::class_eligibility` return `Ok(())` for every
/// class — the reflex card without a budget is admitted and the first
/// refusal assertion fails.
#[test]
fn a_reflex_class_card_without_a_resource_budget_is_refused() {
    // Premise: the same card as a generic model is eligible.
    assert_eq!(
        eligible(ModelClass::Generic).decision_eligibility(now()),
        Ok(())
    );

    let unbudgeted = eligible(ModelClass::Reflex);
    let reason = unbudgeted
        .decision_eligibility(now())
        .expect_err("a reflex model with no resource budget was decision-eligible");
    assert!(reason.contains("resource budget"), "{reason}");

    let budgeted = eligible(ModelClass::Reflex).with_resource_budget(ResourceBudget {
        max_inference_micros: 50,
        max_memory_bytes: 64 * 1024,
    });
    assert_eq!(budgeted.decision_eligibility(now()), Ok(()));
    assert!(
        ModelClass::Reflex.allowed_in_hot_lane(),
        "a reflex student is what the hot lane runs"
    );
}

/// Mutation: drop the `Policy | Ensemble` arm of `class_eligibility` — the
/// unbenchmarked policy is admitted. Mutation: drop the `Foundation` arm — the
/// foundation model is admitted.
#[test]
fn foundation_models_never_decide_and_policies_and_ensembles_need_a_named_benchmark() {
    assert!(
        eligible(ModelClass::Foundation)
            .decision_eligibility(now())
            .is_err()
    );
    assert!(!ModelClass::Foundation.allowed_in_hot_lane());
    for class in [ModelClass::Policy, ModelClass::Ensemble] {
        assert!(
            eligible(class).decision_eligibility(now()).is_err(),
            "{} with no benchmark was eligible",
            class.as_str()
        );
        let mut benchmarked = eligible(class);
        benchmarked.benchmarks.push("best-single-member".into());
        assert_eq!(
            benchmarked.decision_eligibility(now()),
            Ok(()),
            "{}",
            class.as_str()
        );
        assert!(!class.allowed_in_hot_lane());
    }
    // Every class names itself distinctly, so a card's class reads back.
    let names: std::collections::BTreeSet<_> = ModelClass::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(names.len(), ModelClass::ALL.len());
    assert!(
        ["foundation", "policy", "ensemble", "reflex"]
            .iter()
            .all(|n| names.contains(n))
    );
}

/// ADR 0198, R12-EXPAND-6070: `ModelRegistry::promote` is the one door into
/// production for a model the platform fitted for itself, and `register`
/// honours the same rule, so a card built at `Production` cannot walk around
/// the shadow stage. The restore path registers journaled cards, which
/// carry the completed shadow they were promoted out of, and those are
/// admitted. Mutation: delete the `self_fitted && stage == Production` check
/// in `register` — the unshadowed card is registered and the first `is_err`
/// fails.
#[test]
fn a_self_fitted_card_registered_at_production_without_a_completed_shadow_is_refused() {
    let mut card = eligible(ModelClass::Generic);
    card.self_fitted = true;
    assert_eq!(
        card.stage,
        ModelStage::Production,
        "premise: built at production"
    );
    assert!(card.shadow.is_none(), "premise: no shadow stage");
    let mut registry = ModelRegistry::new();
    assert!(registry.register(card.clone()).is_err());
    assert_eq!(registry.len(), 0, "nothing was registered");

    let mut short = card.clone();
    short.shadow = Some(ShadowRecord {
        since: now(),
        rounds_won: MIN_SHADOW_ROUNDS - 1,
        last_round: now(),
        evidence: vec!["round".to_string()],
    });
    assert!(
        registry.register(short).is_err(),
        "an unfinished shadow is not a completed one"
    );

    let mut completed = card.clone();
    completed.shadow = Some(ShadowRecord {
        since: now(),
        rounds_won: MIN_SHADOW_ROUNDS,
        last_round: now(),
        evidence: vec!["round".to_string(); MIN_SHADOW_ROUNDS as usize],
    });
    registry.register(completed).unwrap();
    assert_eq!(
        registry.len(),
        1,
        "a restored, shadow-completed card is admitted"
    );

    let mut pipeline = eligible(ModelClass::Generic);
    pipeline.self_fitted = false;
    pipeline.version = "1.0.1".to_string();
    registry.register(pipeline).unwrap();
    assert_eq!(
        registry.len(),
        2,
        "a pipeline-trained card is governed elsewhere"
    );
}

/// ADR 0436 (R12-MODEL-4022): a trial round is evidence only if it is new,
/// says what it was scored on, and comes before acceptance. Each refusal
/// below leaves the trial as it was; the admitted rounds count up and name
/// both venues, so the door is not one that refuses everything.
///
/// Mutation: drop the blank-evidence refusal in `record_trial_round` — the
/// blank round is admitted and the first `is_err` assertion fails. Mutation:
/// compare `at < trial.last_round` instead of `<=` — the repeated round is
/// admitted and the third assertion fails. Mutation: drop the stage refusal —
/// the production card's round is admitted and the last assertion fails.
#[test]
fn a_trial_round_is_refused_when_blank_repeated_or_recorded_after_acceptance() {
    use qip_ai::registry::{MIN_TRIAL_ROUNDS, TrialVenue};
    let mut card = eligible(ModelClass::Generic);
    card.stage = ModelStage::Validation;
    let reference = card.reference();
    let mut registry = ModelRegistry::new();
    assert!(
        registry.register(card).is_ok(),
        "premise: the card registers"
    );
    let at = |secs: i64| now().saturating_add(qip_core::Duration::from_secs(secs));

    assert!(
        registry
            .record_trial_round(&reference, TrialVenue::Paper, "  ", at(1))
            .is_err()
    );
    assert_eq!(
        registry.record_trial_round(&reference, TrialVenue::Paper, "paper fills", at(1)),
        Ok(1)
    );
    assert!(
        registry
            .record_trial_round(&reference, TrialVenue::Paper, "the same instant", at(1))
            .is_err(),
        "a round at the same instant counted twice"
    );
    for round in 2..=i64::from(MIN_TRIAL_ROUNDS) {
        assert!(
            registry
                .record_trial_round(&reference, TrialVenue::Simulation, "replayed", at(round))
                .is_ok()
        );
    }
    let trial = registry
        .get(&reference)
        .and_then(|card| card.trial.clone())
        .expect("the trial is on the card");
    assert!(trial.completed(), "{trial:?}");
    assert_eq!(trial.rounds, MIN_TRIAL_ROUNDS);
    assert_eq!(trial.venues.len(), 2, "{trial:?}");
    assert_eq!(
        registry.get(&reference).map(|card| card.stage),
        Some(ModelStage::Validation),
        "a trial round moved the card's stage"
    );

    let accepted = eligible(ModelClass::Generic);
    let accepted_reference = format!("{}-accepted", accepted.name);
    let mut accepted = accepted;
    accepted.name = accepted_reference.clone();
    let accepted_reference = accepted.reference();
    assert!(registry.register(accepted).is_ok(), "premise: registers");
    assert!(
        registry
            .record_trial_round(&accepted_reference, TrialVenue::Paper, "late", at(100))
            .is_err(),
        "a round recorded after acceptance was admitted"
    );
}
