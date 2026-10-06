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
