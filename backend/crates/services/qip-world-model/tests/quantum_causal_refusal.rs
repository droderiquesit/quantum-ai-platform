//! QUANT-029: No quantum result is treated as causal proof.
//!
//! A quantum or quantum-assisted result is never recorded or used as proof of
//! a causal effect or structure. Causal claims come only from classical
//! identification methods (experiments, defensible observational methods, or
//! temporal precedence via Granger).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_core::{Duration, Timestamp};
use qip_world_model::causal::{CausalEdge, Mechanism};
use qip_world_model::world::WorldModel;
use std::collections::BTreeSet;

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

#[test]
fn world_model_claim_causal_refuses_edges_with_quantum_sourced_evidence() {
    // QUANT-029 verification: WorldModel::claim_causal refuses an edge whose
    // evidence basis is a quantum result.
    let mut model = WorldModel::new();

    // A quantum-sourced evidence ID is prefixed with "quantum:".
    let quantum_edge = CausalEdge {
        cause: "price_a".to_string(),
        effect: "price_b".to_string(),
        mechanism: Mechanism::Sentiment,
        strength: 0.5,
        lag: Duration::from_hours(1),
        confidence: 0.7,
        evidence: vec!["quantum:qaoa-result-001".to_string()],
        recorded_at: now(),
        decayed_at: None,
        adjusted_for: BTreeSet::new(),
        suspected_confounders: BTreeSet::new(),
        holds_in: BTreeSet::new(),
        fails_in: BTreeSet::new(),
        failure_run: None,
        retired: None,
        evidence_unretrievable: false,
    };

    let result = model.claim_causal(quantum_edge);
    assert!(
        result.is_err(),
        "a claim with quantum-sourced evidence must be refused"
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("quantum result is treated as causal proof"),
        "error message must name the prohibition"
    );
}

#[test]
fn world_model_claim_causal_accepts_edges_with_classical_evidence() {
    // QUANT-029 verification: WorldModel::claim_causal still accepts edges
    // established by classical methods (Granger temporal precedence, filings,
    // research, etc.).
    let mut model = WorldModel::new();

    // A Granger-identified edge with temporal-precedence mechanism.
    let granger_edge = CausalEdge {
        cause: "price_a".to_string(),
        effect: "price_b".to_string(),
        mechanism: Mechanism::TemporalPrecedence,
        strength: 0.4,
        lag: Duration::from_hours(2),
        confidence: 0.6,
        evidence: vec!["granger:lagged-precedence-test".to_string()],
        recorded_at: now(),
        decayed_at: None,
        adjusted_for: BTreeSet::new(),
        suspected_confounders: BTreeSet::new(),
        holds_in: BTreeSet::new(),
        fails_in: BTreeSet::new(),
        failure_run: None,
        retired: None,
        evidence_unretrievable: false,
    };

    let result = model.claim_causal(granger_edge);
    assert!(
        result.is_ok(),
        "a claim with classical (Granger) evidence must be accepted"
    );
}

#[test]
fn world_model_claim_causal_accepts_mixed_evidence_without_quantum_sources() {
    // QUANT-029 verification: an edge with multiple classical evidence sources
    // (filing, research, etc.) is accepted, even when no quantum evidence is
    // present.
    let mut model = WorldModel::new();

    let multi_evidence_edge = CausalEdge {
        cause: "commodity_x".to_string(),
        effect: "firm_y_cost".to_string(),
        mechanism: Mechanism::InputCost,
        strength: 0.85,
        lag: Duration::from_days(1),
        confidence: 0.9,
        evidence: vec![
            "filing:annual-report-2025".to_string(),
            "research:supply-chain-analysis".to_string(),
        ],
        recorded_at: now(),
        decayed_at: None,
        adjusted_for: BTreeSet::new(),
        suspected_confounders: BTreeSet::new(),
        holds_in: BTreeSet::new(),
        fails_in: BTreeSet::new(),
        failure_run: None,
        retired: None,
        evidence_unretrievable: false,
    };

    let result = model.claim_causal(multi_evidence_edge);
    assert!(
        result.is_ok(),
        "a claim with multiple classical evidence sources must be accepted"
    );
}

#[test]
fn world_model_claim_causal_rejects_any_quantum_sourced_evidence_in_mixed_list() {
    // QUANT-029 verification: if a quantum-sourced evidence ID is in the list
    // alongside classical evidence, the entire edge is refused. Quantum cannot
    // be mixed in.
    let mut model = WorldModel::new();

    let mixed_quantum_edge = CausalEdge {
        cause: "supply_disruption".to_string(),
        effect: "price_impact".to_string(),
        mechanism: Mechanism::SupplyChain,
        strength: 0.7,
        lag: Duration::from_hours(6),
        confidence: 0.8,
        evidence: vec![
            "filing:supplier-guidance".to_string(),
            "quantum:variational-circuit-002".to_string(),
        ],
        recorded_at: now(),
        decayed_at: None,
        adjusted_for: BTreeSet::new(),
        suspected_confounders: BTreeSet::new(),
        holds_in: BTreeSet::new(),
        fails_in: BTreeSet::new(),
        failure_run: None,
        retired: None,
        evidence_unretrievable: false,
    };

    let result = model.claim_causal(mixed_quantum_edge);
    assert!(
        result.is_err(),
        "a claim with any quantum-sourced evidence, even mixed, must be refused"
    );
}

/// R12-QUANT-4044: a causal claim cannot cite a quantum branch-selection
/// result, on every insertion path, not only `WorldModel::claim_causal`.
/// `CausalEdge::validate` refuses it. `CausalGraph::add`, infallible for its
/// callers, does not absorb it and records the refusal. `reestimate` refuses
/// a supporting claim that cites one, so a quantum id cannot reach an
/// existing edge's evidence either. A classical citation passes all three,
/// which is the premise that the quantum id and not the call is refused.
///
/// Mutation: remove the check in `CausalGraph::reestimate` -- the quantum
/// citation then joins the edge's evidence.
#[test]
fn a_causal_edge_citing_quantum_evidence_is_refused_on_every_insertion_path() {
    use qip_world_model::causal::{CausalGraph, SupportingClaim};
    let edge = |evidence: &str| {
        let mut e = CausalEdge::new(
            "price_a",
            "price_b",
            Mechanism::Sentiment,
            0.5,
            Duration::from_hours(1),
            now(),
        )
        .unwrap();
        e.evidence = vec![evidence.to_string()];
        e
    };
    assert!(edge("filing:10-k-2026").validate().is_ok(), "premise");
    let refused = edge("quantum:branch-selection-7").validate().unwrap_err();
    assert!(refused.to_string().contains("causal proof"), "{refused}");

    let mut graph = CausalGraph::new();
    graph.add(edge("quantum:branch-selection-7"));
    assert_eq!(
        graph.len(),
        0,
        "add absorbed an edge citing a quantum result"
    );
    assert_eq!(graph.refused().len(), 1);
    assert!(graph.refused()[0].contains("quantum:branch-selection-7"));
    graph.add(edge("filing:10-k-2026"));
    assert_eq!(graph.len(), 1, "premise: a classical edge is absorbed");

    let claim = |evidence: &str| {
        let mut c = SupportingClaim::new("price_a", "price_b", Mechanism::Sentiment, 0.6, now());
        c.evidence = vec![evidence.to_string()];
        c
    };
    assert!(
        graph
            .reestimate(
                [claim("observation:tape-1")],
                Duration::from_days(90),
                now()
            )
            .is_ok(),
        "premise: a classical claim re-estimates"
    );
    let refused = graph
        .reestimate(
            [claim("quantum:branch-selection-8")],
            Duration::from_days(90),
            now(),
        )
        .unwrap_err();
    assert!(refused.to_string().contains("causal proof"), "{refused}");
    assert!(
        graph
            .edges()
            .iter()
            .all(|e| e.evidence.iter().all(|id| !id.starts_with("quantum:"))),
        "a quantum citation reached an edge's evidence"
    );
}
