//! Model classes beyond the specialists: foundation, policy, ensemble and
//! reflex models are recorded with class-specific decision eligibility
//! (R12-EXPAND-6060).

use qip_ai::registry::{EvaluationRecord, ModelCard, ModelClass, ModelStage, ResourceBudget};
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
