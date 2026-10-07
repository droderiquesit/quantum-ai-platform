//! WORLD-004's worked chain, weather to shipping to commodity basis to FX,
//! planted as directed, evidenced edges and then walked.
//!
//! What this is not: the requirement's verification is a *replay* of a
//! history containing the chain into a global causal model. This builds the
//! chain by hand in one `WorldModel`. No history is replayed, and no
//! production pass writes these edges; the platform's only production writer
//! of causal edges is the temporal-precedence pass. What it does prove is
//! that the chain is representable with one mechanism per hop whose
//! documented direction matches the edge, that each edge keeps the evidence
//! it was claimed on, and that a shock at the weather end reaches FX along
//! exactly those hops.

use qip_core::error::Result;
use qip_core::time::Duration;
use qip_core::time::Timestamp;
use qip_world_model::causal::{CausalEdge, Mechanism};
use qip_world_model::world::WorldModel;

const WEATHER: &str = "weather-index-atlantic";
const SHIPPING: &str = "shipping-cost-index";
const BASIS: &str = "wheat-basis-black-sea";
const FX: &str = "exporter-currency-vs-usd";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// The three hops, as `(cause, effect, mechanism, evidence)`.
const CHAIN: [(&str, &str, Mechanism, &str); 3] = [
    // A storm track closes routes and raises freight.
    (
        WEATHER,
        SHIPPING,
        Mechanism::Weather,
        "source:noaa-hurricane-track",
    ),
    // Freight is part of the delivered price, so it moves the basis.
    (
        SHIPPING,
        BASIS,
        Mechanism::Logistics,
        "source:freight-basis-analysis",
    ),
    // The exporter's currency moves with the price of what it exports.
    // Not CurrencyTranslation, which runs from a currency to translated
    // revenue: the first form of this test used it here, backwards.
    (
        BASIS,
        FX,
        Mechanism::TermsOfTrade,
        "research:commodity-currency-study",
    ),
];

fn planted() -> Result<WorldModel> {
    let mut model = WorldModel::new();
    for (cause, effect, mechanism, evidence) in CHAIN {
        let edge = CausalEdge::new(
            cause,
            effect,
            mechanism,
            0.6,
            Duration::from_days(2),
            start(),
        )?
        .with_confidence(0.7)?
        .with_evidence(vec![evidence.to_string()]);
        model.claim_causal(edge)?;
    }
    Ok(model)
}

#[test]
fn a_weather_shock_reaches_fx_through_shipping_and_commodity_basis_on_evidenced_directed_edges() {
    // The first form of this test claimed the chain "is traversable from
    // weather to FX" and asserted only that each edge it inserted read back
    // with the mechanism it was built with, which holds for any mechanism and
    // any graph. This walks the graph.
    let model = planted().expect("the planted chain is well formed");
    let known_at = start();
    let edges = model.causal().edges();
    assert_eq!(
        edges.len(),
        CHAIN.len(),
        "the premise failed: the model does not hold the planted edges: {edges:?}"
    );

    // Each directed edge is held, under its mechanism, citing its evidence.
    for (cause, effect, mechanism, evidence) in CHAIN {
        let edge = edges
            .iter()
            .find(|edge| edge.cause == cause && edge.effect == effect)
            .unwrap_or_else(|| panic!("no edge {cause} -> {effect}"));
        assert_eq!(edge.mechanism, mechanism, "{cause} -> {effect}");
        assert_eq!(
            edge.evidence,
            vec![evidence.to_string()],
            "{cause} -> {effect} does not cite the evidence it was claimed on"
        );
        assert!(
            !edges
                .iter()
                .any(|reverse| reverse.cause == effect && reverse.effect == cause),
            "{effect} -> {cause} is held as well, so the chain has no direction"
        );
    }

    // Forward: a rise in the weather index reaches FX at third order, through
    // exactly the planted nodes and mechanisms, and in the same direction.
    let propagated = model.propagate(WEATHER, 1.0, CHAIN.len(), known_at, known_at);
    let fx = propagated
        .effects
        .iter()
        .find(|effect| effect.target == FX)
        .unwrap_or_else(|| {
            panic!(
                "a weather shock does not reach {FX}: {:?}",
                propagated.effects
            )
        });
    assert_eq!(fx.order, 3, "FX was not reached at third order");
    assert_eq!(fx.path, vec![WEATHER, SHIPPING, BASIS, FX]);
    assert_eq!(
        fx.chain,
        vec![
            Mechanism::Weather,
            Mechanism::Logistics,
            Mechanism::TermsOfTrade
        ]
    );
    assert!(
        fx.magnitude > 0.0,
        "every hop is same-signed, so FX should move with the weather index: {}",
        fx.magnitude
    );

    // Backward: each link names its one cause, from FX to weather.
    for (cause, effect, _, _) in CHAIN {
        let explanations = model.causal().explanations(effect, known_at);
        assert_eq!(
            explanations
                .iter()
                .map(|edge| edge.cause.as_str())
                .collect::<Vec<_>>(),
            vec![cause],
            "{effect} is not explained by {cause} alone"
        );
    }
}

#[test]
fn the_new_weather_mechanism_is_properly_defined() {
    assert_eq!(Mechanism::Weather.as_str(), "weather");
}

#[test]
fn the_new_logistics_mechanism_is_properly_defined() {
    assert_eq!(Mechanism::Logistics.as_str(), "logistics");
}

#[test]
fn the_new_geopolitical_mechanism_is_properly_defined() {
    assert_eq!(Mechanism::Geopolitical.as_str(), "geopolitical");
}

#[test]
fn the_terms_of_trade_mechanism_is_properly_defined() {
    assert_eq!(Mechanism::TermsOfTrade.as_str(), "terms_of_trade");
}
