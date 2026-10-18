//! R12-WORLD-1016: the platform's models represent uncertainty, and where
//! sources disagree they record the disagreement instead of overwriting one
//! side. One test per model kind, each reading the field a consumer reads.
//!
//! The kinds and what holds their uncertainty: the world model's `Fact`
//! (a confidence refused outside `[0, 1]`) and its `Contradiction` record
//! (both beliefs kept); the reasoning engine's `ContradictionResolution`
//! (evidence disagreement with a named resolution); the episodic memory's
//! `ClaimRecord` (the confidence the claim was made at); and the self-model's
//! `CapabilityEstimate` (refuses to estimate from a thin sample rather than
//! reporting a guess). The episodic and self models hold no contradiction
//! record of their own; the register row says so.

#![allow(clippy::panic_in_result_fn)]

use qip_ai::memory::episode::ClaimRecord;
use qip_core::error::{Error, Result};
use qip_core::ids::EvidenceId;
use qip_core::{Duration, Timestamp};
use qip_learning_engine::self_model::{CapabilityEstimate, MINIMUM_SAMPLE, ScoredOutcome};
use qip_reasoning_engine::contradiction::{ContradictionResolution, ResolutionMethod};
use qip_world_model::graph::{Fact, KnowledgeGraph};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use std::collections::BTreeSet;

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn says(weight: f64, source: &str, learned_days_ago: i64) -> Result<Fact> {
    Fact::new(
        Relationship::new("a", "b", RelationshipKind::Issues, weight, source),
        now().saturating_sub(Duration::from_days(100)),
        now().saturating_sub(Duration::from_days(learned_days_ago)),
        0.8,
    )
}

#[test]
fn a_world_fact_carries_a_confidence_and_refuses_one_that_is_not_a_probability() -> Result<()> {
    let fact = says(0.5, "filings", 10)?;
    assert!(fact.confidence > 0.0 && fact.confidence < 1.0);
    for bad in [2.0, -0.1, f64::NAN] {
        let refused = Fact::new(
            Relationship::new("a", "b", RelationshipKind::Issues, 0.5, "filings"),
            now(),
            now(),
            bad,
        );
        assert!(refused.is_err(), "a confidence of {bad} was stored");
    }
    Ok(())
}

#[test]
fn two_sources_contradicting_one_world_fact_are_both_kept_and_linked_by_a_record() -> Result<()> {
    let mut graph = KnowledgeGraph::new();
    graph.assert_fact(says(0.30, "filings", 10)?);
    assert!(
        graph.contradictions_at(now()).is_empty(),
        "premise: one statement is no contradiction"
    );
    graph.assert_fact(says(0.80, "newswire", 5)?);
    let recorded = graph.contradictions_at(now());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].held.source, "filings");
    assert_eq!(recorded[0].arrived.source, "newswire");
    assert!(recorded[0].held.confidence > 0.0 && recorded[0].arrived.confidence > 0.0);
    assert_eq!(
        graph.facts_at(now(), now()).len(),
        2,
        "a belief was dropped"
    );
    Ok(())
}

#[test]
fn contradicting_evidence_is_recorded_with_how_it_was_resolved() -> Result<()> {
    let ids: BTreeSet<EvidenceId> = ["ev-1", "ev-2"].map(EvidenceId::from_string).into();
    let resolved = ContradictionResolution::new(
        now(),
        ids.clone(),
        ResolutionMethod::InterpretationDifference,
        BTreeSet::new(),
        now(),
        "the two sources define the measure differently",
    )
    .map_err(Error::invalid)?;
    assert_eq!(resolved.contradicting_ids, ids);
    assert!(!resolved.is_unresolved());
    // One piece of evidence cannot contradict itself.
    let single: BTreeSet<EvidenceId> = [EvidenceId::from_string("ev-1")].into();
    assert!(
        ContradictionResolution::new(
            now(),
            single,
            ResolutionMethod::Unresolved,
            BTreeSet::new(),
            now(),
            "alone"
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn an_episodes_claim_carries_the_confidence_it_was_made_at() -> Result<()> {
    let claim = ClaimRecord {
        class: "momentum".into(),
        claim: "continuation".into(),
        direction: 1.0,
        confidence: 0.62,
    };
    let encoded = serde_json::to_value(&claim).map_err(|e| Error::invalid(e.to_string()))?;
    assert_eq!(encoded["confidence"], serde_json::json!(0.62));
    Ok(())
}

#[test]
fn a_capability_is_estimated_only_from_enough_graded_outcomes_and_never_as_certain() -> Result<()> {
    let mut estimate = CapabilityEstimate::new();
    assert!(
        estimate.estimate().is_err(),
        "premise: an empty record is not an estimate"
    );
    let outcome = |i: usize| {
        ScoredOutcome::new(
            format!("hyp-{i}"),
            0.7,
            true,
            now().saturating_add(Duration::from_secs(i as i64)),
        )
    };
    // One short of the minimum: the record is non-empty, so the refusal is
    // the sample-size rule and not the empty-window fallback.
    for i in 0..MINIMUM_SAMPLE - 1 {
        estimate.record(outcome(i)?);
    }
    assert_eq!(estimate.sample_count(), MINIMUM_SAMPLE - 1, "premise");
    assert!(estimate.estimate().is_err(), "a thin sample was estimated");
    estimate.record(outcome(MINIMUM_SAMPLE)?);
    let capability = estimate.estimate()?;
    assert_eq!(capability.sample_count, MINIMUM_SAMPLE);
    assert!(
        (capability.hit_rate - 1.0).abs() < 1e-12 && capability.accuracy < 1.0,
        "a perfect record is still not reported as certainty: {capability:?}"
    );
    Ok(())
}
