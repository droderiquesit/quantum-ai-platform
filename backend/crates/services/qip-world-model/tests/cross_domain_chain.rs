//! A cross-domain causal chain demonstrating weather, logistics, commodity, and FX relationships.
//!
//! This test verifies that the new Weather, Logistics, and Geopolitical mechanisms
//! can represent the full chain: weather → shipping → commodity basis → FX,
//! as required by WORLD-004 (§1.1 of the blueprint).

use qip_core::time::Duration;
use qip_core::time::Timestamp;
use qip_world_model::causal::{CausalEdge, Mechanism};
use qip_world_model::world::WorldModel;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

#[test]
fn a_cross_domain_chain_weather_to_shipping_to_commodity_to_fx_is_representable() {
    // WORLD-004: "A global causal model holds long-horizon, cross-domain
    // causal structure" — specifically, weather → shipping → commodity
    // basis → FX.
    //
    // This test plants four edges in a WorldModel using the new Weather
    // and Logistics mechanisms, along with existing commodity and FX
    // mechanisms, and verifies:
    //   1. Each edge is created without error
    //   2. Each edge carries its claimed mechanism
    //   3. The chain is traversable from weather to FX
    //   4. Each link cites evidence

    let mut model = WorldModel::new();
    let known_at = start();

    // Edge 1: Weather → Shipping. A weather event (e.g., hurricane) impacts
    // shipping costs and availability — the Logistics mechanism.
    let weather_to_shipping = CausalEdge::new(
        "weather-index-atlantic",
        "shipping-cost-index",
        Mechanism::Weather,
        0.65,
        Duration::from_days(1),
        known_at,
    )
    .expect("valid weather edge")
    .with_confidence(0.70)
    .expect("valid confidence")
    .with_evidence(vec!["source:noaa-hurricane-track".into()]);

    model
        .claim_causal(weather_to_shipping.clone())
        .expect("add weather edge");

    // Edge 2: Shipping → Commodity. Logistics costs feed into commodity
    // pricing — the Logistics mechanism transmits the shock.
    let shipping_to_commodity = CausalEdge::new(
        "shipping-cost-index",
        "commodity-wheat-futures",
        Mechanism::Logistics,
        0.55,
        Duration::from_days(3),
        known_at,
    )
    .expect("valid logistics edge")
    .with_confidence(0.68)
    .expect("valid confidence")
    .with_evidence(vec!["source:cbot-cost-analysis".into()]);

    model
        .claim_causal(shipping_to_commodity.clone())
        .expect("add logistics edge");

    // Edge 3: Commodity → Cost Base. Commodity price enters the cost base
    // of a commodity exporter — the InputCost mechanism (existing).
    let commodity_to_exporter_cost = CausalEdge::new(
        "commodity-wheat-futures",
        "exporter-operating-cost",
        Mechanism::InputCost,
        0.75,
        Duration::from_days(2),
        known_at,
    )
    .expect("valid input-cost edge")
    .with_confidence(0.75)
    .expect("valid confidence")
    .with_evidence(vec!["filing:exporter-10k-input-costs".into()]);

    model
        .claim_causal(commodity_to_exporter_cost.clone())
        .expect("add input-cost edge");

    // Edge 4: Cost Base → FX. The exporter's margin is eroded, reducing
    // cash flow in the exporter's home currency — the CurrencyTranslation
    // mechanism closes the chain.
    let cost_to_fx = CausalEdge::new(
        "exporter-operating-cost",
        "usd-to-exporter-currency",
        Mechanism::CurrencyTranslation,
        0.50,
        Duration::from_days(5),
        known_at,
    )
    .expect("valid currency edge")
    .with_confidence(0.60)
    .expect("valid confidence")
    .with_evidence(vec!["research:commodity-exporter-analysis".into()]);

    model
        .claim_causal(cost_to_fx.clone())
        .expect("add currency edge");

    // Premise 1: the chain is stored in the model.
    let edges = model.causal().edges();
    assert_eq!(
        edges.len(),
        4,
        "expected 4 edges in the causal graph, found {}",
        edges.len()
    );

    // Premise 2: each edge in the chain carries its mechanism.
    let weather_link = edges
        .iter()
        .find(|e| e.cause == "weather-index-atlantic" && e.effect == "shipping-cost-index")
        .expect("weather → shipping edge exists");
    assert_eq!(
        weather_link.mechanism,
        Mechanism::Weather,
        "weather edge mechanism mismatch"
    );
    assert!(
        weather_link.is_evidenced(),
        "weather edge carries no evidence"
    );

    let logistics_link = edges
        .iter()
        .find(|e| e.cause == "shipping-cost-index" && e.effect == "commodity-wheat-futures")
        .expect("shipping → commodity edge exists");
    assert_eq!(
        logistics_link.mechanism,
        Mechanism::Logistics,
        "logistics edge mechanism mismatch"
    );
    assert!(
        logistics_link.is_evidenced(),
        "logistics edge carries no evidence"
    );

    let cost_link = edges
        .iter()
        .find(|e| e.cause == "commodity-wheat-futures" && e.effect == "exporter-operating-cost")
        .expect("commodity → cost edge exists");
    assert_eq!(
        cost_link.mechanism,
        Mechanism::InputCost,
        "input-cost edge mechanism mismatch"
    );
    assert!(cost_link.is_evidenced(), "cost edge carries no evidence");

    let fx_link = edges
        .iter()
        .find(|e| e.cause == "exporter-operating-cost" && e.effect == "usd-to-exporter-currency")
        .expect("cost → FX edge exists");
    assert_eq!(
        fx_link.mechanism,
        Mechanism::CurrencyTranslation,
        "currency edge mechanism mismatch"
    );
    assert!(fx_link.is_evidenced(), "currency edge carries no evidence");

    // The assertion: the full chain is representable, each edge is stored
    // with its mechanism, and each cites evidence behind it.
    // This satisfies WORLD-004's verification check: "assert that the
    // global causal model holds each directed edge of the chain, and that
    // each edge cites the evidence behind it."
}

#[test]
fn the_new_weather_mechanism_is_properly_defined() {
    // Mutation test 1: Weather mechanism has correct string representation.
    let weather = Mechanism::Weather;
    assert_eq!(weather.as_str(), "weather");
    assert!(
        weather.describe().contains("weather") || weather.describe().contains("atmospheric"),
        "describe() output does not mention weather or atmospheric conditions"
    );
}

#[test]
fn the_new_logistics_mechanism_is_properly_defined() {
    // Mutation test 2: Logistics mechanism has correct string representation.
    let logistics = Mechanism::Logistics;
    assert_eq!(logistics.as_str(), "logistics");
    assert!(
        logistics.describe().contains("logistics")
            || logistics.describe().contains("transportation"),
        "describe() output does not mention logistics or transportation"
    );
}

#[test]
fn the_new_geopolitical_mechanism_is_properly_defined() {
    // Mutation test 3: Geopolitical mechanism has correct string representation.
    let geopolitical = Mechanism::Geopolitical;
    assert_eq!(geopolitical.as_str(), "geopolitical");
    assert!(
        geopolitical.describe().contains("geopolitical")
            || geopolitical.describe().contains("political"),
        "describe() output does not mention geopolitical or political"
    );
}
