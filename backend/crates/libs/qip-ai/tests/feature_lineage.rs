//! A card records where each feature came from, and a promotion refuses one
//! whose features read the future or were never audited (R12-MODEL-4016).

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_ai::registry::{EvaluationRecord, FeatureOrigin, ModelCard, ModelRegistry};
use qip_core::{ModelId, Timestamp};
use std::collections::BTreeMap;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn origin(source_through: i64, decision_at: i64, audited_rows: usize) -> FeatureOrigin {
    FeatureOrigin {
        derivation: "close[t] / close[t-1] - 1".to_string(),
        source_digest: "s".repeat(64),
        code_digest: "c".repeat(64),
        source_through: at(source_through),
        decision_at: at(decision_at),
        audited_rows,
    }
}

fn card_with(lineage: &[(&str, FeatureOrigin)]) -> ModelCard {
    let mut card = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000055"),
        "lineaged",
        "1.0.0",
        "quant-research",
        at(0),
    )
    .with_features(vec!["return_1".to_string(), "momentum_5".to_string()]);
    card.evaluations.push(EvaluationRecord {
        evaluated_at: at(0),
        dataset: "held-out".into(),
        metrics: BTreeMap::from([("rmse".to_string(), 0.1)]),
        passed: true,
    });
    for (feature, origin) in lineage {
        card.feature_lineage
            .insert((*feature).to_string(), origin.clone());
    }
    card
}

fn promote(card: ModelCard) -> Result<(), String> {
    let mut registry = ModelRegistry::default();
    let reference = card.reference();
    registry.register(card);
    registry
        .promote(&reference, at(10))
        .map_err(|e| e.message().to_string())
}

/// Mutation: delete the `lineage_findings` refusal in `ModelRegistry::promote`
/// — the look-ahead card is promoted and the `expect_err` fires.
#[test]
fn a_feature_that_reads_past_its_decision_instant_blocks_the_promotion() {
    let clean = card_with(&[
        ("return_1", origin(5, 5, 48)),
        ("momentum_5", origin(5, 5, 48)),
    ]);
    assert!(
        clean.lineage_findings().is_empty(),
        "premise: clean lineage"
    );
    assert_eq!(promote(clean), Ok(()), "premise: a clean card is promoted");

    let leaking = card_with(&[
        ("return_1", origin(5, 5, 48)),
        ("momentum_5", origin(9, 5, 48)),
    ]);
    let refusal = promote(leaking).expect_err("a look-ahead feature is not promoted");
    assert!(
        refusal.contains("feature momentum_5 reads source through"),
        "{refusal}"
    );
    assert!(refusal.contains("look-ahead"), "{refusal}");
    assert!(
        !refusal.contains("feature return_1"),
        "only the leaking feature is named: {refusal}"
    );
}

/// Mutation: drop the `audited_rows == 0` branch of `FeatureOrigin::finding` —
/// the unaudited card is promoted and the `expect_err` fires.
#[test]
fn a_feature_whose_audit_examined_nothing_blocks_the_promotion() {
    let unaudited = card_with(&[
        ("return_1", origin(5, 5, 48)),
        ("momentum_5", origin(5, 5, 0)),
    ]);
    let refusal = promote(unaudited).expect_err("an unaudited feature is not promoted");
    assert!(
        refusal.contains("feature momentum_5 was not audited"),
        "{refusal}"
    );
}

#[test]
fn a_card_reports_the_features_its_lineage_leaves_out() {
    let card = card_with(&[("return_1", origin(5, 5, 48))]);
    assert_eq!(card.features_without_lineage(), vec!["momentum_5"]);
    assert!(card_with(&[]).features_without_lineage().len() == 2);
    // A card that records no lineage is not blocked by the registry, which
    // cannot tell "predates the field" from "left out"; the desk refuses it.
    assert_eq!(promote(card_with(&[])), Ok(()));
}
