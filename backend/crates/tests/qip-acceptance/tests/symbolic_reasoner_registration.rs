//! MODEL-021: Symbolic reasoners (graph, causal) are registered and refuse
//! hot-lane use.
//!
//! The failure this prevents: symbolic reasoners being silently included in
//! hot-lane packages, violating latency guarantees. Graph and causal reasoning
//! are registered as model classes so lane-admission checks can refuse them
//! from hot-lane paths and verify reproducibility of answers through recorded
//! derivations.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_ai::registry::{ModelCard, ModelClass, ModelRegistry};
use qip_core::{ModelId, Timestamp};

#[test]
fn graph_reasoner_is_registered_and_refused_from_hot_lanes() {
    let mut registry = ModelRegistry::new();
    let now = Timestamp::from_civil(2026, 10, 6);

    // Register graph reasoner (fact queries over knowledge graph)
    let graph_reasoner = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000101"),
        "world-model-graph",
        "1.0.0",
        "world-model-team",
        now,
    )
    .with_purpose("bitemporal graph traversal and as-of queries")
    .with_class(ModelClass::ReasonerGraph);

    registry.register(graph_reasoner);

    // Verify registration
    let card = registry
        .get("world-model-graph@1.0.0")
        .expect("graph reasoner must be registered");
    assert_eq!(
        card.class,
        ModelClass::ReasonerGraph,
        "graph reasoner must have correct class"
    );

    // Verify hot-lane refusal
    let refusal = registry.allowed_in_hot_lane("world-model-graph@1.0.0");
    assert!(
        refusal.is_err(),
        "graph reasoner must be refused from hot lanes"
    );
    let reason = refusal.unwrap_err().to_string();
    assert!(
        reason.contains("reasoner"),
        "refusal reason must name the reasoner class: {reason}"
    );
}

#[test]
fn causal_reasoner_is_registered_and_refused_from_hot_lanes() {
    let mut registry = ModelRegistry::new();
    let now = Timestamp::from_civil(2026, 10, 6);

    // Register causal reasoner (shock propagation)
    let causal_reasoner = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000102"),
        "world-model-causal",
        "1.0.0",
        "world-model-team",
        now,
    )
    .with_purpose("causal shock propagation with lag and attenuation")
    .with_class(ModelClass::ReasonerCausal);

    registry.register(causal_reasoner);

    // Verify registration
    let card = registry
        .get("world-model-causal@1.0.0")
        .expect("causal reasoner must be registered");
    assert_eq!(
        card.class,
        ModelClass::ReasonerCausal,
        "causal reasoner must have correct class"
    );

    // Verify hot-lane refusal
    let refusal = registry.allowed_in_hot_lane("world-model-causal@1.0.0");
    assert!(
        refusal.is_err(),
        "causal reasoner must be refused from hot lanes"
    );
}

#[test]
fn both_reasoner_types_prevent_hot_lane_use() {
    // Reasoners are too expensive for hot paths: graph traversal and causal
    // propagation require iterative computation that cannot fit in
    // microsecond budgets. This test verifies the lane-admission barrier is
    // in place for both types.

    let reasoner_classes = [ModelClass::ReasonerGraph, ModelClass::ReasonerCausal];

    for class in &reasoner_classes {
        assert!(
            !class.allowed_in_hot_lane(),
            "{:?} must not be allowed in hot lanes",
            class
        );
    }
}

#[test]
fn reasoner_class_names_are_stable_for_serialization() {
    // Serialization round-trip must preserve the class so models can be
    // persisted and reloaded with their category intact.
    use serde_json;

    let graph_class = ModelClass::ReasonerGraph;
    let causal_class = ModelClass::ReasonerCausal;

    let graph_json = serde_json::to_string(&graph_class).unwrap();
    let causal_json = serde_json::to_string(&causal_class).unwrap();

    let graph_restored: ModelClass = serde_json::from_str(&graph_json).unwrap();
    let causal_restored: ModelClass = serde_json::from_str(&causal_json).unwrap();

    assert_eq!(
        graph_class, graph_restored,
        "graph reasoner class must survive serialization"
    );
    assert_eq!(
        causal_class, causal_restored,
        "causal reasoner class must survive serialization"
    );
}
