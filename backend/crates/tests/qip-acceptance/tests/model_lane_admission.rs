//! MODEL-007, MODEL-021: Specialist and reasoner models are refused from hot-lane
//! packages.
//!
//! The failure this prevents: a specialist financial model or symbolic reasoner
//! finding its way into Lane 0 code, violating latency guarantees through
//! expensive inference or graph traversal. Lane admission checks register every
//! model and refuse any whose class is not allowed in the target lane.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_ai::registry::{ModelCard, ModelClass, ModelRegistry};
use qip_core::{ModelId, Timestamp};

#[test]
fn specialist_models_are_refused_from_hot_lanes() {
    let mut registry = ModelRegistry::new();
    let now = Timestamp::from_civil(2026, 10, 6);

    // Register a generic model — allowed in hot lanes
    let generic_card = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000001"),
        "hot-model",
        "1.0.0",
        "team",
        now,
    )
    .with_purpose("generic prediction")
    .with_class(ModelClass::Generic);

    // Register specialist models — refused from hot lanes
    let regime_card = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000002"),
        "regime-detector",
        "1.0.0",
        "team",
        now,
    )
    .with_purpose("regime identification")
    .with_class(ModelClass::SpecialistRegime);

    let liquidity_card = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000003"),
        "liquidity-model",
        "1.0.0",
        "team",
        now,
    )
    .with_purpose("liquidity depth prediction")
    .with_class(ModelClass::SpecialistLiquidity);

    registry.register(generic_card);
    registry.register(regime_card);
    registry.register(liquidity_card);

    // Generic model passes hot-lane admission
    assert!(
        registry.allowed_in_hot_lane("hot-model@1.0.0").is_ok(),
        "generic model must be allowed in hot lanes"
    );

    // Specialist models fail hot-lane admission
    assert!(
        registry
            .allowed_in_hot_lane("regime-detector@1.0.0")
            .is_err(),
        "specialist regime model must be refused from hot lanes"
    );
    assert!(
        registry
            .allowed_in_hot_lane("liquidity-model@1.0.0")
            .is_err(),
        "specialist liquidity model must be refused from hot lanes"
    );
}

#[test]
fn reasoner_models_are_refused_from_hot_lanes() {
    let mut registry = ModelRegistry::new();
    let now = Timestamp::from_civil(2026, 10, 6);

    // Register reasoners — refused from hot lanes
    let graph_reasoner = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000004"),
        "graph-reasoner",
        "1.0.0",
        "team",
        now,
    )
    .with_purpose("graph traversal and queries")
    .with_class(ModelClass::ReasonerGraph);

    let causal_reasoner = ModelCard::new(
        ModelId::from_string("MDL0000000000000000000005"),
        "causal-reasoner",
        "1.0.0",
        "team",
        now,
    )
    .with_purpose("causal shock propagation")
    .with_class(ModelClass::ReasonerCausal);

    registry.register(graph_reasoner);
    registry.register(causal_reasoner);

    // Both reasoners fail hot-lane admission
    assert!(
        registry
            .allowed_in_hot_lane("graph-reasoner@1.0.0")
            .is_err(),
        "graph reasoner must be refused from hot lanes"
    );
    assert!(
        registry
            .allowed_in_hot_lane("causal-reasoner@1.0.0")
            .is_err(),
        "causal reasoner must be refused from hot lanes"
    );
}

#[test]
fn all_specialist_model_classes_are_refused_from_hot_lanes() {
    let mut registry = ModelRegistry::new();
    let now = Timestamp::from_civil(2026, 10, 6);

    let specialists = vec![
        ("regime", ModelClass::SpecialistRegime),
        ("event-prob", ModelClass::SpecialistEventProb),
        ("valuation", ModelClass::SpecialistValuation),
        ("liquidity", ModelClass::SpecialistLiquidity),
        ("slippage", ModelClass::SpecialistSlippage),
        ("default", ModelClass::SpecialistDefault),
        ("volatility", ModelClass::SpecialistVolatility),
        ("demand", ModelClass::SpecialistDemand),
        ("supply", ModelClass::SpecialistSupply),
    ];

    for (i, (name, class)) in specialists.iter().enumerate() {
        let model_id_str = format!("MDL{:030x}", 10 + i);
        let card = ModelCard::new(
            ModelId::from_string(&model_id_str),
            format!("specialist-{name}").as_str(),
            "1.0.0",
            "team",
            now,
        )
        .with_class(*class);
        registry.register(card);

        let reference = format!("specialist-{name}@1.0.0");
        assert!(
            registry.allowed_in_hot_lane(&reference).is_err(),
            "specialist model {name} must be refused from hot lanes"
        );
    }
}

#[test]
fn model_class_allows_generic_in_hot_lane_only() {
    // Verify the class-level decision on each variant
    assert!(
        ModelClass::Generic.allowed_in_hot_lane(),
        "only generic class is allowed in hot lanes"
    );

    for class in [
        ModelClass::SpecialistRegime,
        ModelClass::SpecialistEventProb,
        ModelClass::SpecialistValuation,
        ModelClass::SpecialistLiquidity,
        ModelClass::SpecialistSlippage,
        ModelClass::SpecialistDefault,
        ModelClass::SpecialistVolatility,
        ModelClass::SpecialistDemand,
        ModelClass::SpecialistSupply,
        ModelClass::ReasonerGraph,
        ModelClass::ReasonerCausal,
    ] {
        assert!(
            !class.allowed_in_hot_lane(),
            "{:?} must not be allowed in hot lanes",
            class
        );
    }
}
