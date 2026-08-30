//! What each stage of the hot path actually costs, measured.
//!
//! `docs/performance/budgets.md` is written from the numbers this file prints.
//! That is the whole point of it existing: a budget nobody measured is a wish,
//! and a document full of wishes is worse than an empty one because somebody
//! will design against it.
//!
//! Three rules, taken from `qip-orderbook`'s `throughput.rs`, which is how
//! this repository already measures things honestly.
//!
//! * **Assert a ceiling, print the number.** Every assertion here is loose
//!   enough that only a real regression — an accidental clone per message, a
//!   linear scan where there was a lookup, a recomputation of a whole graph
//!   per tick — can trip it. A tight threshold on shared hardware fails for
//!   reasons that have nothing to do with the code, and a threshold loose
//!   enough not to would catch nothing, so the *measurement* is the output and
//!   the assertion is only a floor under it.
//! * **Say which profile.** `cargo test` builds unoptimised and is several
//!   times slower than `--release`. A figure quoted without its profile is not
//!   a figure. Every line printed here names the profile it came from.
//! * **Measure a stage, not a system.** Each test times one stage in
//!   isolation, on this machine, in one thread, with the fixture built before
//!   the clock starts. None of this is end-to-end latency, and none of it is
//!   evidence about a deployed system: there is no network here, no venue, no
//!   colocation, and no I/O. See the caveats in the budgets document.
//!
//! What is deliberately **not** measured, and therefore not claimed anywhere:
//! wire-to-wire latency, tick-to-order latency, cross-region latency, and
//! anything involving a real venue. This build has no venue transport at all.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::{CapitalEnvelope, Utilisation};
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_contracts::{FeatureKey, FeatureValue, FeatureVector, Revision};
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_execution_engine::oms::OrderManager;
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::features::{
    BookPressure, ExponentialMovingAverage, Microprice, Mid, RealisedVolatility, Spread,
};
use qip_feature_dag::state::MarketState;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_normalization::normalizer::{Normalizer, SymbolMapping, UnitConversion};
use qip_orderbook::venue::VenueState;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::pretrade::{PreTradeChecker, ProposedOrder};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;
use qip_strategy::runtime::StrategyRuntime;
use std::collections::BTreeMap;
use std::time::{Duration as WallDuration, Instant};

// --- measurement ------------------------------------------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{name}"))
}

fn venue() -> VenueId {
    VenueId::new("XLON")
}

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("a decimal literal in a fixture")
}

fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Print what a stage measured, and assert only that it was not absurd.
///
/// `ceiling` is per-operation, in microseconds, and is deliberately one to two
/// orders of magnitude above what an unoptimised build takes. It exists to
/// catch a change in complexity class, not to police a few percent.
fn report(label: &str, operations: usize, elapsed: WallDuration, ceiling_micros: f64) {
    let seconds = elapsed.as_secs_f64();
    let per_operation_micros = seconds * 1e6 / operations as f64;
    println!(
        "{label}: {operations} ops in {seconds:.3}s = {:.0} ops/s \
         ({per_operation_micros:.3} us/op, {} profile, this machine, single-threaded)",
        operations as f64 / seconds,
        profile()
    );
    assert!(
        per_operation_micros < ceiling_micros,
        "{label} took {per_operation_micros:.3} us/op, past the {ceiling_micros:.0} us ceiling; \
         this is a change in complexity rather than a slow machine"
    );
}

// --- fixtures ---------------------------------------------------------------

/// `count` level-set messages walking a book around a hundred.
///
/// Built before any clock starts, so the fixture's own cost is never inside a
/// measurement.
fn level_stream(symbol: &str, count: usize, seed: u64) -> Vec<MarketMessage> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..count)
        .map(|index| {
            let side = if index % 2 == 0 {
                BookSide::Bid
            } else {
                BookSide::Ask
            };
            let offset = rng.below(9) as i64 - 4;
            let price = if side == BookSide::Bid {
                99 + offset.min(0)
            } else {
                101 + offset.max(0)
            };
            let at = start().saturating_add(Duration::from_millis(index as i64));
            MarketMessage::new(
                object(symbol),
                Origin::new(venue(), "feed-a", 0, index as u64),
                MessageBody::LevelSet {
                    side,
                    price: Decimal::from_int(price),
                    quantity: Decimal::from_int(100 + (index % 7) as i64 * 50),
                    order_count: None,
                },
                at,
                at,
            )
        })
        .collect()
}

/// `count` bars from a provider that writes the venue and the units its own
/// way, so normalisation has real work to do rather than a pass-through.
fn provider_bars(count: usize) -> Vec<SensedRecord> {
    (0..count)
        .map(|index| {
            let close = 100.0 + ((index as f64 * 0.7548776662) % 1.0 - 0.5) * 2.0;
            let at = start().saturating_sub(Duration::from_secs(index as i64));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object("ACME"),
                // The provider's own venue code, not the canonical one.
                venue: "LSE".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(close).expect("representable"),
                high: Decimal::from_f64(close * 1.003).expect("representable"),
                low: Decimal::from_f64(close * 0.997).expect("representable"),
                close: Decimal::from_f64(close).expect("representable"),
                volume: dec!("2500000"),
                trade_count: 8_000,
                vwap: Decimal::from_f64(close),
                quality: qip_financial::quality::DataQuality::default(),
            }))
        })
        .collect()
}

fn normalizer() -> Normalizer {
    let mut normalizer = Normalizer::with_standard_venues();
    normalizer.add_symbol_mapping(SymbolMapping {
        provider: "acme-data".to_string(),
        provider_symbol: "ACME.L".to_string(),
        object_id: object("ACME"),
        canonical_symbol: "ACME".to_string(),
        canonical_venue: "XLON".to_string(),
    });
    normalizer.add_conversion(UnitConversion::pence_to_pounds("acme-data", object("ACME")));
    normalizer
}

/// A feature graph of the size a cell really runs: several instruments, and
/// the microstructure features a strategy actually reads.
fn feature_engine(symbols: &[&str]) -> Result<FeatureEngine> {
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    for symbol in symbols {
        let subject = object(symbol);
        engine.register(Box::new(Mid::new(subject.clone())))?;
        engine.register(Box::new(Spread::new(subject.clone())))?;
        engine.register(Box::new(Microprice::new(subject.clone())))?;
        engine.register(Box::new(BookPressure::new(subject.clone(), 5)))?;
        engine.register(Box::new(RealisedVolatility::new(subject.clone(), 20)))?;
        engine.register(Box::new(ExponentialMovingAverage::new(subject, 20)))?;
    }
    Ok(engine)
}

fn pressure_key(symbol: &str) -> FeatureKey {
    FeatureKey::new("book_pressure", object(symbol)).with("levels", 5)
}

/// A three-rule strategy over four features, compiled by the real compiler.
fn compiled(symbol: &str) -> Result<(CompiledStrategy, Program)> {
    let subject = object(symbol);
    let pressure = pressure_key(symbol);
    let volatility = FeatureKey::new("realised_volatility", subject.clone()).with("window", 20);
    let mid = FeatureKey::new("mid", subject.clone());
    let spread = FeatureKey::new("spread", subject.clone());

    let mut catalogue = FeatureCatalogue::new();
    for (key, value_type) in [
        (pressure.clone(), Type::Statistic),
        (volatility.clone(), Type::Statistic),
        (mid.clone(), Type::Exact),
        (spread.clone(), Type::Exact),
    ] {
        catalogue.declare(key, value_type)?;
    }

    let spec = StrategySpec::new(
        StrategyId::new("performance-strategy"),
        subject,
        Duration::from_millis(250),
    )
    .with_rule(Rule::new(
        "enter",
        SignalKind::Enter,
        Expr::feature(pressure.clone())
            .greater_than(Expr::Statistic(0.4))
            .and(Expr::feature(volatility.clone()).less_than(Expr::Statistic(0.9)))
            .and(Expr::feature(spread.clone()).at_most(Expr::feature(mid.clone()))),
        Expr::Exact(dec!("100")),
        Expr::Statistic(0.62),
        500,
    ))
    .with_rule(Rule::new(
        "exit",
        SignalKind::Exit,
        Expr::feature(pressure)
            .less_than(Expr::Statistic(-0.4))
            .and(Expr::feature(volatility).greater_than(Expr::Statistic(0.2))),
        Expr::Exact(dec!("100")),
        Expr::Statistic(0.55),
        500,
    ));

    let mut compiler = StrategyCompiler::new(catalogue);
    let strategy = compiler.compile(&spec)?;
    Ok((strategy, compiler.into_program()))
}

fn vector_for(symbol: &str, as_of: Timestamp, pressure: f64) -> FeatureVector {
    let subject = object(symbol);
    let mut vector = FeatureVector::new(as_of);
    vector.insert(
        pressure_key(symbol),
        FeatureValue::Statistic(pressure),
        Revision::new(1),
    );
    vector.insert(
        FeatureKey::new("realised_volatility", subject.clone()).with("window", 20),
        FeatureValue::Statistic(0.35),
        Revision::new(2),
    );
    vector.insert(
        FeatureKey::new("mid", subject.clone()),
        FeatureValue::Exact(dec!("100")),
        Revision::new(3),
    );
    vector.insert(
        FeatureKey::new("spread", subject),
        FeatureValue::Exact(dec!("0.02")),
        Revision::new(4),
    );
    vector
}

fn signed_envelope() -> Result<CapitalEnvelope> {
    let terms = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("performance-strategy"),
            "performance-1",
            dec!("100000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
            start(),
            start().saturating_add(Duration::from_hours(1)),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = terms("unsigned")?;
    terms(&sign_payload(
        b"a-performance-key",
        &unsigned.signing_payload(),
    ))
}

fn risk_limits() -> LimitSet {
    LimitSet::new("performance")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at twice equity"),
        )
        .with(
            Limit::new("max-net-exposure", LimitKind::MaxNetExposure { limit: 1.0 })
                .with_rationale("the book is not net long more than its equity"),
        )
        .with(
            Limit::new("cash-buffer", LimitKind::MinCashBuffer { limit: 0.05 })
                .with_rationale("a twentieth of equity stays in cash so a settlement never fails"),
        )
        .with(
            Limit::new(
                "max-order-notional",
                LimitKind::MaxOrderNotional {
                    limit: dec!("5000000"),
                },
            )
            .with_rationale("no single order moves more than five million at once"),
        )
}

// --- the stages -------------------------------------------------------------

#[test]
fn event_normalisation_costs_what_the_budget_says() -> Result<()> {
    // The first thing every observation goes through: venue canonicalisation,
    // unit conversion, the price-continuity guard and a future-timestamp
    // clamp. All four run per record, so this is a per-event cost on the
    // widest path in the platform.
    const RECORDS: usize = 100_000;
    let normalizer = normalizer();
    let records = provider_bars(RECORDS);

    let started = Instant::now();
    let (out, summary) = normalizer.normalise("acme-data", records, start());
    let elapsed = started.elapsed();

    assert_eq!(out.len(), RECORDS, "normalisation lost records");
    assert_eq!(summary.processed, RECORDS as u64);
    assert!(
        summary.venues_canonicalised > 0 && summary.units_converted > 0,
        "the fixture gave normalisation nothing to do: {summary:?}"
    );
    report("normalise (bar)", RECORDS, elapsed, 50.0);
    Ok(())
}

#[test]
fn book_apply_costs_what_the_budget_says() -> Result<()> {
    // The hottest call in the platform: one venue message folded into one
    // book. Everything downstream is per-decision; this is per-packet.
    const MESSAGES: usize = 200_000;
    let stream = level_stream("ACME", MESSAGES, 0xB0_0C);
    let mut state = VenueState::aggregated(object("ACME"), venue(), VenueStatus::Open);

    let started = Instant::now();
    for message in &stream {
        state.apply(message)?;
    }
    let elapsed = started.elapsed();

    assert_eq!(state.applied(), MESSAGES as u64);
    assert!(state.mid().is_some(), "the book did not end up priceable");
    report("book apply (L2 level set)", MESSAGES, elapsed, 20.0);
    Ok(())
}

#[test]
fn feature_evaluation_costs_what_the_budget_says() -> Result<()> {
    // The measurement that matters is the incremental one: a message dirties
    // the nodes it can affect, and an evaluation recomputes exactly those. So
    // the loop is ingest-then-evaluate, per message, which is what a cell
    // actually does — not a batch evaluation amortised over a stream.
    const MESSAGES: usize = 20_000;
    let symbols = ["ACME", "BOREAS", "CERES", "DORIS"];
    let mut engine = feature_engine(&symbols)?;
    let streams: Vec<Vec<MarketMessage>> = symbols
        .iter()
        .enumerate()
        .map(|(index, symbol)| {
            level_stream(symbol, MESSAGES / symbols.len(), 0xFEA7 + index as u64)
        })
        .collect();

    let mut computed = 0usize;
    let started = Instant::now();
    for (index, stream) in streams.iter().enumerate() {
        for message in stream {
            engine.ingest(message)?;
            let vector = engine.evaluate(
                start().saturating_add(Duration::from_millis((index * MESSAGES + computed) as i64)),
            )?;
            computed += vector.len().min(1);
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(computed, MESSAGES, "an evaluation returned an empty vector");
    let vector = engine.evaluate(start().saturating_add(Duration::from_secs(1)))?;
    assert_eq!(
        vector.len(),
        symbols.len() * 6,
        "the graph is not the size the fixture registered"
    );
    report("feature ingest + evaluate", MESSAGES, elapsed, 500.0);
    Ok(())
}

#[test]
fn strategy_evaluation_costs_what_the_budget_says() -> Result<()> {
    // Evaluation cost does not depend on the market: every node the strategy
    // reaches is computed whichever way its conditions go. That is why this
    // number is worth budgeting at all — it is a property of the strategy
    // rather than of the news, so a measurement today bounds tomorrow.
    const RUNS: usize = 200_000;
    let (strategy, program) = compiled("ACME")?;
    let mut runtime = StrategyRuntime::new(program)?;

    // Two vectors, one that fires and one that does not, alternated so the
    // measurement is not of a single branch.
    let firing = vector_for("ACME", start(), 0.8);
    let quiet = vector_for("ACME", start(), 0.0);
    let mut signals = 0usize;

    let started = Instant::now();
    for index in 0..RUNS {
        let vector = if index % 2 == 0 { &firing } else { &quiet };
        if runtime.run(&strategy, vector, start())?.is_some() {
            signals += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        signals,
        RUNS / 2,
        "the fixture did not exercise both branches"
    );
    report(
        &format!("strategy run ({} nodes)", strategy.cost()),
        RUNS,
        elapsed,
        50.0,
    );
    Ok(())
}

#[test]
fn arbitrage_detection_costs_what_the_budget_says() -> Result<()> {
    // Search, exact confirmation, book walk, nine deductions and a leg plan:
    // the whole narrowing, per scan. Budgeted as one number because the stages
    // are not separable in practice — a scan that stops early is a scan that
    // found nothing, which is the common case and the cheap one.
    use qip_arbitrage::graph::{ArbitrageGraph, Node, VenueFacts};
    use qip_arbitrage::liquidity::StaticLiquidity;
    use qip_arbitrage::netedge::EdgeAssumptions;
    use qip_arbitrage::plan::PlanSettings;
    use qip_arbitrage::scan::{OpportunityScanner, SizePolicy};
    use qip_arbitrage::search::SearchSettings;
    use qip_market::book::{BookLevel, OrderBook};

    const SCANS: usize = 2_000;
    let cx = VenueId::new("CX");
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        cx.clone(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    let node = |name: &str| Node::new(ObjectId::from_string(name), cx.clone());
    for (from, to, rate, market, side) in [
        ("USDT", "ETH", "0.000333328", "ETHUSDT", BookSide::Ask),
        ("ETH", "BTC", "0.050505", "ETHBTC", BookSide::Bid),
        ("BTC", "USDT", "60000.5", "BTCUSDT", BookSide::Bid),
    ] {
        graph.add_trade(
            node(from),
            node(to),
            d(rate),
            d("0.0004"),
            ObjectId::from_string(market),
            side,
            start(),
            20,
        )?;
    }

    let book = |market: &str, bid: &str, ask: &str| {
        OrderBook::from_levels(
            ObjectId::from_string(market),
            "CX",
            start(),
            vec![BookLevel::new(d(bid), d("200"))],
            vec![BookLevel::new(d(ask), d("200"))],
        )
    };
    let depth = StaticLiquidity::new()
        .with_book(cx.clone(), book("ETHUSDT", "3000.0", "3000.1"), 20)
        .with_book(cx.clone(), book("ETHBTC", "0.0505", "0.05051"), 20)
        .with_book(cx, book("BTCUSDT", "60000", "60001"), 20);

    let scanner = OpportunityScanner::new(
        SearchSettings::default(),
        EdgeAssumptions::default(),
        PlanSettings::with_budget(d("1000000")),
    );
    let policy = SizePolicy::uniform(d("10000"));

    let mut found = 0usize;
    let started = Instant::now();
    for _ in 0..SCANS {
        let report = scanner.scan(&graph, &depth, &policy, start());
        found += report.opportunities.len();
    }
    let elapsed = started.elapsed();

    assert_eq!(
        found, SCANS,
        "the fixture stopped finding its dislocation partway through"
    );
    report(
        "arbitrage scan (3-node graph, 3 edges)",
        SCANS,
        elapsed,
        5_000.0,
    );
    Ok(())
}

#[test]
fn the_capital_decision_costs_what_the_budget_says() -> Result<()> {
    // The gate between a signal and an order on the cell's hot path: expiry,
    // venue scope, loss limit, headroom and the per-order cap, in that order.
    // It runs per candidate order, inside the decision, so it is budgeted with
    // the decision rather than with the allocator that produced the grant.
    const DECISIONS: usize = 500_000;
    let envelope = VerifiedEnvelope::verify(
        signed_envelope()?,
        b"a-performance-key",
        "performance-1",
        start(),
    )?;
    let xlon = venue();
    let mut utilisation = Utilisation::default();
    let mut granted = 0usize;

    let started = Instant::now();
    for index in 0..DECISIONS {
        let notional = Decimal::from_int(1_000 + (index % 97) as i64 * 100);
        if !envelope
            .admit(&xlon, notional, &utilisation, start())
            .is_refused()
        {
            granted += 1;
        }
        // A little utilisation so the headroom arithmetic is not constant.
        utilisation.orders_sent += 1;
    }
    let elapsed = started.elapsed();

    assert_eq!(granted, DECISIONS, "the fixture ran out of headroom");
    report("capital admit", DECISIONS, elapsed, 20.0);
    Ok(())
}

#[test]
fn the_risk_decision_costs_what_the_budget_says() -> Result<()> {
    // Five limits projected against the state the order would produce. This is
    // the check between a proposal and a venue, and it runs once per order, so
    // its cost is on the decision path rather than the packet path.
    const CHECKS: usize = 100_000;
    let checker = PreTradeChecker::new(risk_limits());
    let state = RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        gross_exposure: dec!("4000000"),
        net_exposure: dec!("1000000"),
        position_notionals: BTreeMap::from([
            ("obj-ACME".to_string(), dec!("400000")),
            ("obj-BOREAS".to_string(), dec!("600000")),
        ]),
        ..RiskState::default()
    };
    let order = ProposedOrder {
        object_id: object("ACME"),
        quantity: dec!("1000"),
        reference_price: dec!("100"),
        axes: BTreeMap::from([("sector".to_string(), "information_technology".to_string())]),
        counterparty: Some("broker-a".to_string()),
        scope: "performance".to_string(),
    };

    let mut approved = 0usize;
    let started = Instant::now();
    for _ in 0..CHECKS {
        if checker.check(&order, &state, start())?.is_approved() {
            approved += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(approved, CHECKS, "the fixture was refused, so nothing ran");
    report("pre-trade check (5 limits)", CHECKS, elapsed, 100.0);
    Ok(())
}

#[test]
fn order_construction_costs_what_the_budget_says() -> Result<()> {
    // Building the order and validating it: the identifier, the lineage back
    // to a proposal and its hypotheses, and the five refusals that make an
    // untraceable order impossible to construct.
    const ORDERS: usize = 200_000;
    let mut manager = OrderManager::new(PreTradeChecker::new(risk_limits()));
    let hypotheses = vec!["hyp-performance".to_string()];

    let mut built = 0usize;
    let started = Instant::now();
    for _ in 0..ORDERS {
        let order_id = manager.next_order_id("ord");
        let order = Order::new(
            order_id,
            object("ACME"),
            Side::Buy,
            dec!("1000"),
            OrderType::Market,
            dec!("100"),
            "prop-performance",
            hypotheses.clone(),
            "performance",
            start(),
        );
        order.validate()?;
        built += 1;
    }
    let elapsed = started.elapsed();

    assert_eq!(built, ORDERS);
    report("order construct + validate", ORDERS, elapsed, 20.0);
    Ok(())
}

// --- the honesty check ------------------------------------------------------

#[test]
fn the_budgets_document_says_what_is_measured_and_what_is_not() {
    // The document is the deliverable these tests exist to justify, so it is
    // checked here rather than trusted. Two claims have to survive contact
    // with a reader: that no end-to-end latency has been measured, and that
    // microsecond-class figures apply only to colocated paths this build does
    // not have. A budgets document that quietly dropped either would read as a
    // performance guarantee, which is the specific overclaim to avoid.
    let budgets = qip_acceptance::read("docs/performance/budgets.md");
    let lowered = budgets.to_lowercase();

    for required in [
        "no end-to-end latency has been measured",
        "colocated",
        "unmeasured",
        "debug",
        "release",
    ] {
        assert!(
            lowered.contains(required),
            "docs/performance/budgets.md does not say \"{required}\""
        );
    }

    // Every stage these tests measure has a row, so a stage cannot be measured
    // and then quietly left out of the budget it was measured for.
    for stage in [
        "normalis",
        "book apply",
        "feature",
        "strategy",
        "arbitrage",
        "capital",
        "risk",
        "order construction",
    ] {
        assert!(
            lowered.contains(stage),
            "docs/performance/budgets.md has no row for {stage}"
        );
    }

    // And nothing in it claims a number for a path nobody timed.
    for overclaim in [
        "sub-microsecond end-to-end",
        "wire-to-wire latency of",
        "one million events per second",
    ] {
        assert!(
            !lowered.contains(overclaim),
            "docs/performance/budgets.md claims \"{overclaim}\""
        );
    }
}
