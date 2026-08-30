//! Signals, factors and the strategy SDK.

use qip_core::testing::approx_eq;
use qip_core::{Currency, Decimal, ObjectId, PortfolioId, Timestamp, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::constraints::{RegulatoryConstraints, TradingRestriction};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_portfolio::portfolio::Portfolio;
use qip_quant::factor::{FactorDefinition, FactorLibrary};
use qip_quant::signal::{
    Horizon, Signal, SignalKind, SignalSet, carry, mean_reversion, momentum, realised_volatility,
};
use qip_quant::strategy::{SignalWeightedStrategy, Strategy, StrategyContext, TargetWeights};
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("OBJ{symbol:0>23}"))
}

// --- signal computations ----------------------------------------------------

#[test]
fn momentum_is_volatility_scaled_so_instruments_compare() {
    // Two instruments with the same total return but very different volatility
    // must not produce the same momentum score.
    let smooth: Vec<f64> = (0..61)
        .map(|i| 100.0 * (1.0 + 0.002 * f64::from(i)))
        .collect();
    let choppy: Vec<f64> = (0..61)
        .map(|i| 100.0 * (1.0 + 0.002 * f64::from(i)) * if i % 2 == 0 { 1.03 } else { 0.97 })
        .collect();

    let smooth_score = momentum(&smooth, 60).unwrap();
    let choppy_score = momentum(&choppy, 60).unwrap();
    assert!(
        smooth_score > choppy_score,
        "the same move earned with less risk should score higher: {smooth_score} vs {choppy_score}"
    );
    assert!(momentum(&smooth, 100).is_none(), "not enough history");
    assert!(
        momentum(&vec![100.0; 61], 60).is_none(),
        "a flat series has no scale"
    );
}

#[test]
fn mean_reversion_points_against_the_recent_move() {
    let mut prices: Vec<f64> = (0..40).map(|_| 100.0).collect();
    // Add variation so the standard deviation is non-zero, then spike.
    for (index, price) in prices.iter_mut().enumerate() {
        *price += (index % 3) as f64 * 0.5;
    }
    prices.push(115.0);
    let score = mean_reversion(&prices, 30).unwrap();
    assert!(
        score < 0.0,
        "a price far above its mean should score negative: {score}"
    );

    prices.pop();
    prices.push(85.0);
    assert!(mean_reversion(&prices, 30).unwrap() > 0.0);
}

#[test]
fn realised_volatility_and_carry_behave() {
    let mut prices = vec![100.0];
    for i in 1..40 {
        prices.push(100.0 * if i % 2 == 0 { 1.01 } else { 0.99 });
    }
    let volatility = realised_volatility(&prices, 30, 252.0).unwrap();
    assert!(
        volatility > 0.05 && volatility < 1.0,
        "annualised vol {volatility}"
    );

    // Carry is a yield pickup per unit of risk.
    assert!(approx_eq(carry(0.06, 0.04, 0.10).unwrap(), 0.2, 1e-12));
    assert!(carry(0.06, 0.04, 0.0).is_none(), "no risk means no ratio");
}

// --- signals ----------------------------------------------------------------

#[test]
fn confidence_grows_with_sample_size_and_saturates() {
    let tiny = Signal::new(object("A"), SignalKind::Momentum, 2.0, now(), 5);
    let large = Signal::new(object("A"), SignalKind::Momentum, 2.0, now(), 500);
    assert!(large.confidence > tiny.confidence);
    assert!(
        large.confidence < 1.0,
        "confidence must never reach certainty"
    );
    assert!(
        tiny.confidence < 0.2,
        "five observations is barely a signal"
    );
    assert!(!tiny.is_actionable(0.5), "too few observations to act on");
    assert!(large.is_actionable(0.5));
}

#[test]
fn a_signal_declares_the_horizon_it_speaks_to() {
    // Mixing horizons unlabelled produces a portfolio sized for neither.
    assert_eq!(SignalKind::OrderFlow.natural_horizon(), Horizon::Intraday);
    assert_eq!(SignalKind::Value.natural_horizon(), Horizon::LongTerm);
    assert!(Horizon::Intraday.is_fast());
    assert!(!Horizon::LongTerm.is_fast());
    assert!(Horizon::LongTerm.typical_holding_days() > Horizon::ShortTerm.typical_holding_days());
}

#[test]
fn a_signal_that_cannot_cover_its_costs_is_not_traded() {
    // The check that stops a real but tiny edge being traded away in spread.
    let signal =
        Signal::new(object("A"), SignalKind::MeanReversion, 1.0, now(), 200).with_confidence(0.6);
    assert!(
        !signal.survives_costs(8.0, 10.0),
        "a 4.8bp expected move cannot pay 10bp"
    );
    assert!(signal.survives_costs(50.0, 10.0));
}

#[test]
fn a_composite_only_blends_signals_sharing_a_horizon() {
    // Blending a long-term value score with an intraday flow reading produces
    // a number that means nothing.
    let mut signals = SignalSet::new();
    signals.push(Signal::new(
        object("A"),
        SignalKind::Momentum,
        2.0,
        now(),
        200,
    ));
    signals.push(Signal::new(
        object("A"),
        SignalKind::OrderFlow,
        -3.0,
        now(),
        200,
    ));

    let medium = signals.composite(Horizon::MediumTerm);
    let intraday = signals.composite(Horizon::Intraday);
    assert!(
        medium[object("A").as_str()] > 0.0,
        "the momentum signal alone"
    );
    assert!(
        intraday[object("A").as_str()] < 0.0,
        "the flow signal alone"
    );
}

#[test]
fn a_composite_weights_by_confidence() {
    let mut signals = SignalSet::new();
    signals
        .push(Signal::new(object("A"), SignalKind::Momentum, 3.0, now(), 500).with_confidence(0.9));
    signals.push(
        Signal::new(object("A"), SignalKind::EarningsRevision, -1.0, now(), 500)
            .with_confidence(0.1),
    );
    let composite = signals.composite(Horizon::MediumTerm)[object("A").as_str()];
    let expected = (3.0 * 0.9 - 1.0 * 0.1) / (0.9 + 0.1);
    assert!(
        approx_eq(composite, expected, 1e-12),
        "composite {composite}"
    );
    assert!(composite > 2.0, "the confident signal should dominate");
}

#[test]
fn standardisation_is_robust_to_an_outlier() {
    // With classical z-scores a single extreme value compresses every other
    // score toward zero, and the cross-section stops discriminating.
    let mut signals = SignalSet::new();
    for (index, score) in [1.0, 1.2, 0.8, 1.1, 50.0].iter().enumerate() {
        signals.push(Signal::new(
            object(&format!("A{index}")),
            SignalKind::Momentum,
            *score,
            now(),
            200,
        ));
    }
    let standardised = signals.standardised();
    let scores: Vec<f64> = standardised.iter().map(|s| s.score).collect();
    // The four clustered values should still be distinguishable from each other.
    let spread = scores[..4]
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max)
        - scores[..4].iter().cloned().fold(f64::INFINITY, f64::min);
    assert!(spread > 0.5, "the cluster collapsed: {scores:?}");
    assert!(scores.iter().all(|s| s.abs() <= 3.0), "scores are clamped");
}

#[test]
fn the_strongest_signals_are_ranked_by_effective_score() {
    let mut signals = SignalSet::new();
    signals
        .push(Signal::new(object("A"), SignalKind::Momentum, 1.0, now(), 500).with_confidence(0.9));
    signals
        .push(Signal::new(object("B"), SignalKind::Momentum, 3.0, now(), 500).with_confidence(0.1));
    let strongest = signals.strongest(1);
    assert_eq!(
        strongest[0].object_id,
        object("A"),
        "confidence-weighted, not raw"
    );
    assert_eq!(signals.for_object(&object("A")).len(), 1);
    assert_eq!(signals.of_kind(SignalKind::Momentum).len(), 2);
}

// --- factors ----------------------------------------------------------------

#[test]
fn the_factor_library_records_what_goes_wrong_with_each_factor() {
    // These are not footnotes: momentum's crash risk is why a portfolio built
    // on it needs a risk overlay, and the adversarial reviewer needs to know.
    let library = FactorLibrary::standard();
    assert!(library.len() >= 6);
    for name in library.names() {
        let definition = library.get(name).unwrap();
        assert!(
            !definition.known_weaknesses.is_empty(),
            "{name} must record how it fails"
        );
    }

    let weaknesses = library.weaknesses_of(&["momentum".to_string(), "carry".to_string()]);
    assert!(weaknesses.iter().any(|(_, w)| w.contains("crash")));
    assert!(weaknesses.iter().any(|(_, w)| w.contains("tail")));
}

#[test]
fn scoring_flips_the_sign_where_lower_is_better() {
    let library = FactorLibrary::standard();
    // Low volatility is scored inverted: a lower raw value is a better score.
    let values = vec![
        ("A".to_string(), 0.10),
        ("B".to_string(), 0.40),
        ("C".to_string(), 0.25),
    ];
    let scores = library.score_cross_section("low_volatility", &values);
    let a = scores.iter().find(|s| s.object_id == "A").unwrap();
    let b = scores.iter().find(|s| s.object_id == "B").unwrap();
    assert!(a.score > b.score, "the calmer name should score higher");
    assert!(a.percentile > b.percentile);
    assert!(approx_eq(a.raw, 0.10, 1e-12), "the raw value is preserved");
}

#[test]
fn scoring_an_empty_cross_section_is_safe() {
    assert!(
        FactorLibrary::standard()
            .score_cross_section("momentum", &[])
            .is_empty()
    );
}

#[test]
fn a_custom_factor_can_be_defined() {
    let mut library = FactorLibrary::new();
    library.define(
        FactorDefinition::new("custom", "a bespoke measure")
            .with_inputs(vec!["x".into()])
            .with_weakness("untested outside the fitted sample"),
    );
    assert_eq!(library.len(), 1);
    assert!(library.get("custom").is_some());
}

// --- target weights ---------------------------------------------------------

#[test]
fn weights_report_gross_and_net_separately() {
    let mut targets = TargetWeights::new();
    targets.set(&object("A"), 0.4);
    targets.set(&object("B"), -0.3);
    assert!(approx_eq(targets.net(), 0.1, 1e-12));
    assert!(approx_eq(targets.gross(), 0.7, 1e-12));

    let scaled = targets.scaled_to_gross(1.4);
    assert!(approx_eq(scaled.gross(), 1.4, 1e-12));
    assert!(approx_eq(scaled.get(&object("A")), 0.8, 1e-12));
}

#[test]
fn tiny_positions_are_pruned() {
    // A 0.1% target costs the same in operational overhead as a 5% one.
    let mut targets = TargetWeights::new();
    targets.set(&object("A"), 0.30);
    targets.set(&object("B"), 0.001);
    let pruned = targets.pruned(0.005);
    assert_eq!(pruned.len(), 1);
    assert!(
        !pruned.weights.contains_key(object("B").as_str()),
        "a sub-minimum weight must be dropped, not shrunk"
    );
}

#[test]
fn turnover_is_the_fraction_of_the_book_that_must_trade() {
    let mut targets = TargetWeights::new();
    targets.set(&object("A"), 0.5);
    targets.set(&object("B"), 0.5);

    let current = BTreeMap::from([
        (object("A").as_str().to_string(), 0.5),
        (object("C").as_str().to_string(), 0.5),
    ]);
    // B is bought entirely and C sold entirely: half the book trades.
    assert!(approx_eq(targets.turnover_from(&current), 0.5, 1e-12));
    // No change means no turnover.
    let unchanged = BTreeMap::from([
        (object("A").as_str().to_string(), 0.5),
        (object("B").as_str().to_string(), 0.5),
    ]);
    assert!(approx_eq(targets.turnover_from(&unchanged), 0.0, 1e-12));
}

// --- the strategy SDK -------------------------------------------------------

fn equity(symbol: &str, price: &str) -> FinancialObject {
    FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(Decimal::parse(price).unwrap())
        .provenance(Provenance::synthetic("test", now()))
        .build(now())
        .expect("valid object")
}

#[test]
fn the_strategy_weights_by_signal_strength_and_respects_its_caps() {
    let mut universe = Universe::new();
    let mut prices = BTreeMap::new();
    let mut signals = SignalSet::new();

    for (symbol, score) in [("AAA", 3.0), ("BBB", 1.5), ("CCC", 0.5), ("DDD", -2.0)] {
        let instrument = equity(symbol, "100");
        prices.insert(instrument.object_id.as_str().to_string(), dec!("100"));
        universe.insert(instrument).unwrap();
        signals.push(Signal::new(
            object(symbol),
            SignalKind::Momentum,
            score,
            now(),
            300,
        ));
    }

    let portfolio = Portfolio::new(
        PortfolioId::from_string("PRT0000000000000000000001"),
        "test",
        Currency::USD,
        Decimal::from_int(1_000_000),
        now(),
    );
    let features = BTreeMap::new();
    let strategy_context = StrategyContext {
        as_of: now(),
        universe: &universe,
        portfolio: &portfolio,
        prices: &prices,
        signals: &signals,
        features: &features,
    };

    let strategy = SignalWeightedStrategy::default();
    let decision = strategy.decide(&strategy_context).unwrap();

    assert!(!decision.targets.is_empty());
    // Long only by default: the negative-score name is excluded.
    assert!(
        !decision
            .targets
            .weights
            .contains_key(object("DDD").as_str()),
        "long only by default: a negative-score name is excluded outright"
    );
    // Three names against a 10% cap cannot reach 90% gross, and the cap wins:
    // a concentration limit is not negotiable against an exposure target.
    assert!(
        decision.targets.weights.values().all(|w| *w <= 0.10 + 1e-9),
        "cap breached: {:?}",
        decision.targets.weights
    );
    assert!(decision.targets.gross() <= 0.30 + 1e-9);
    assert!(
        decision.rationale.contains("under-invested"),
        "{}",
        decision.rationale
    );
    assert!(decision.intended_factors.contains(&"momentum".to_string()));
}

#[test]
fn ranking_survives_where_the_cap_does_not_bind() {
    // Capping and then scaling would flatten the ranking; redistributing the
    // excess keeps the ordering wherever the cap leaves room.
    let mut universe = Universe::new();
    let mut prices = BTreeMap::new();
    let mut signals = SignalSet::new();

    let scores = [3.0, 2.0, 1.5, 1.2, 1.0, 0.9, 0.8, 0.7, 0.6, 0.5, 0.45, 0.4];
    for (index, score) in scores.iter().enumerate() {
        let symbol = format!("S{index:02}");
        let instrument = equity(&symbol, "100");
        prices.insert(instrument.object_id.as_str().to_string(), dec!("100"));
        universe.insert(instrument).unwrap();
        signals.push(Signal::new(
            object(&symbol),
            SignalKind::Momentum,
            *score,
            now(),
            300,
        ));
    }

    let portfolio = Portfolio::new(
        PortfolioId::from_string("PRT0000000000000000000001"),
        "test",
        Currency::USD,
        Decimal::from_int(1_000_000),
        now(),
    );
    let features = BTreeMap::new();
    let decision = SignalWeightedStrategy::default()
        .decide(&StrategyContext {
            as_of: now(),
            universe: &universe,
            portfolio: &portfolio,
            prices: &prices,
            signals: &signals,
            features: &features,
        })
        .unwrap();

    assert!(
        approx_eq(decision.targets.gross(), 0.9, 1e-6),
        "gross {}",
        decision.targets.gross()
    );
    assert!(decision.targets.weights.values().all(|w| *w <= 0.10 + 1e-9));
    // The top name is at the cap; the ordering below it survives.
    let mid = decision.targets.get(&object("S05"));
    let low = decision.targets.get(&object("S11"));
    assert!(mid > low, "ranking lost below the cap: {mid} against {low}");
}

#[test]
fn the_strategy_holds_cash_when_nothing_clears_the_bar() {
    let mut universe = Universe::new();
    let mut prices = BTreeMap::new();
    let mut signals = SignalSet::new();
    let instrument = equity("AAA", "100");
    prices.insert(instrument.object_id.as_str().to_string(), dec!("100"));
    universe.insert(instrument).unwrap();
    signals.push(Signal::new(
        object("AAA"),
        SignalKind::Momentum,
        0.05,
        now(),
        300,
    ));

    let portfolio = Portfolio::new(
        PortfolioId::from_string("PRT0000000000000000000001"),
        "test",
        Currency::USD,
        Decimal::from_int(1_000_000),
        now(),
    );
    let features = BTreeMap::new();
    let decision = SignalWeightedStrategy::default()
        .decide(&StrategyContext {
            as_of: now(),
            universe: &universe,
            portfolio: &portfolio,
            prices: &prices,
            signals: &signals,
            features: &features,
        })
        .unwrap();

    assert!(decision.targets.is_empty());
    assert!(decision.rationale.contains("cash"));
}

#[test]
fn a_restricted_instrument_is_never_targeted() {
    let mut universe = Universe::new();
    let mut prices = BTreeMap::new();
    let mut signals = SignalSet::new();

    let mut restricted = equity("BAD", "100");
    restricted.regulatory =
        RegulatoryConstraints::unrestricted().with_restriction(TradingRestriction::Sanctioned {
            authority: "OFAC".into(),
        });
    prices.insert(restricted.object_id.as_str().to_string(), dec!("100"));
    universe.insert(restricted).unwrap();

    let clean = equity("GOOD", "100");
    prices.insert(clean.object_id.as_str().to_string(), dec!("100"));
    universe.insert(clean).unwrap();

    for symbol in ["BAD", "GOOD"] {
        signals.push(Signal::new(
            object(symbol),
            SignalKind::Momentum,
            3.0,
            now(),
            300,
        ));
    }

    let portfolio = Portfolio::new(
        PortfolioId::from_string("PRT0000000000000000000001"),
        "test",
        Currency::USD,
        Decimal::from_int(1_000_000),
        now(),
    );
    let features = BTreeMap::new();
    let decision = SignalWeightedStrategy::default()
        .decide(&StrategyContext {
            as_of: now(),
            universe: &universe,
            portfolio: &portfolio,
            prices: &prices,
            signals: &signals,
            features: &features,
        })
        .unwrap();

    assert!(
        !decision
            .targets
            .weights
            .contains_key(object("BAD").as_str()),
        "a sanctioned name must never be held"
    );
    assert!(decision.targets.get(&object("GOOD")) > 0.0);
}

#[test]
fn an_unpriced_instrument_is_not_tradable() {
    let mut universe = Universe::new();
    universe.insert(equity("AAA", "100")).unwrap();
    let prices = BTreeMap::new();
    let signals = SignalSet::new();
    let portfolio = Portfolio::new(
        PortfolioId::from_string("PRT0000000000000000000001"),
        "test",
        Currency::USD,
        Decimal::from_int(1_000_000),
        now(),
    );
    let features = BTreeMap::new();
    let strategy_context = StrategyContext {
        as_of: now(),
        universe: &universe,
        portfolio: &portfolio,
        prices: &prices,
        signals: &signals,
        features: &features,
    };
    assert!(
        strategy_context.tradable().is_empty(),
        "no price means not tradable"
    );
}
