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
use qip_contracts::intent::Intent;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::PolicyPayload;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_contracts::{FeatureKey, FeatureValue, FeatureVector, Revision};
use qip_core::error::{Error, Result};
use qip_core::ids::{FillId, ObjectId, OrderId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer, PricingPolicy};
use qip_edge::dropcopy::DropCopyFill;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::feasibility::{self as edge_feasibility, Granularity, VenueModel};
use qip_edge::journal::{Decision, Journal, MemoryMirror, ship};
use qip_edge::policy::VerifiedPolicy;
use qip_edge::reservation::RegionTable;
use qip_execution_engine::broker::{SimulatedBroker, SimulationSettings};
use qip_execution_engine::feasibility::{self as central_feasibility, VenueFeasibility};
use qip_execution_engine::multileg::{LegGroup, Verdict};
use qip_execution_engine::oms::{OrderManager, RefusalReason};
use qip_execution_engine::order::{Fill, Order, OrderType, Side};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::features::{
    BookPressure, ExponentialMovingAverage, Microprice, Mid, RealisedVolatility, Spread,
};
use qip_feature_dag::state::MarketState;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_orderbook::venue::VenueState;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::autonomy::AutonomyController;
use qip_risk_engine::pretrade::{PreTradeChecker, ProposedOrder};
use qip_sequencing::arbitration::{ArbitrationEvent, LineArbiter};
use qip_sequencing::tracker::{ReorderPolicy, SequenceEvent, Sequencer};
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
//
// There is no normalisation stage here. `qip-normalization` was removed by
// ADR 0029 — nothing constructed it, so the figure this file once published
// for it was the cost of a stage no observation went through — and nothing in
// the workspace now canonicalises a venue or converts a provider's units. The
// budgets document says so rather than carrying a row for it.

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

/// The stage label of every figure row in the budgets document, lowercased.
///
/// A figure row is a Markdown table row whose first cell is a stage name:
/// the header row (`Stage`) and the alignment row (`---`) are not figures and
/// are skipped. Every table in the document is a table of figures, so any row
/// this returns is a published number somebody may design against.
fn budget_rows(budgets: &str) -> Vec<String> {
    budgets
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let cells: Vec<&str> = line.strip_prefix('|')?.split('|').collect();
            let stage = cells.first()?.trim().to_lowercase();
            if stage.is_empty() || stage == "stage" || stage.chars().all(|c| c == '-' || c == ':') {
                None
            } else {
                Some(stage)
            }
        })
        .collect()
}

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
    // and then quietly left out of the budget it was measured for. One entry
    // per `*_costs_what_the_budget_says` test above; the kernel-cycle test at
    // the end of this file asserts flatness across two working sets rather
    // than a per-operation budget, and has no row by design.
    const MEASURED_STAGES: [&str; 7] = [
        "book apply",
        "feature",
        "strategy",
        "arbitrage",
        "capital",
        "risk",
        "order construction",
    ];
    let rows = budget_rows(&budgets);
    assert!(
        !rows.is_empty(),
        "docs/performance/budgets.md has no budget rows at all"
    );
    for stage in MEASURED_STAGES {
        assert!(
            rows.iter().any(|row| row.contains(stage)),
            "docs/performance/budgets.md has no row for {stage}"
        );
    }

    // And the other direction, which the check above cannot see: every row
    // names a stage something in this file measures. Without it the document
    // could keep publishing a figure for a stage nothing times — which it did:
    // the normalisation rows outlived their measurement until ADR 0029 removed
    // the crate, and this check stayed green throughout. Each row must match
    // exactly one stage, so a row that straddles two labels cannot count as
    // covering either.
    for row in &rows {
        let matched = MEASURED_STAGES
            .iter()
            .filter(|stage| row.contains(*stage))
            .count();
        assert_eq!(
            matched, 1,
            "docs/performance/budgets.md publishes a row for \"{row}\", which matches {matched} \
             measured stages rather than one; a figure for a stage nothing times is a wish \
             dressed as a measurement"
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

#[test]
fn the_cycle_cost_stops_growing_once_the_history_working_sets_reach_their_bounds() -> Result<()> {
    // The regression this catches has shipped: the kernel's history series,
    // the liquidity topology and the prediction set all held every
    // observation since assembly, and because DISCOVER rescans every series
    // per cycle, the deployed fastbrain's cycle grew from 2.4ms at cycle 255
    // to 310ms at cycle 16,728 — six times its 50ms ceiling — and the
    // readiness probe took the node out of rotation. The property asserted
    // here is *flatness beyond the bounds*, which no slow machine can fake
    // in either direction: a platform fed several times more history than
    // the caps must hold the same working set, and pay about the same per
    // cycle, as one fed exactly at them.
    use qip_core::Context;
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    use qip_financial::universe::Universe;
    use qip_kernel::{Platform, PlatformConfig, Stage};
    use qip_market::quote::Quote;
    use qip_observability::Telemetry;

    const SYMBOLS: [&str; 5] = ["AAA", "BBB", "CCC", "DDD", "EEE"];

    fn universe() -> Result<Universe> {
        let mut universe = Universe::new();
        for symbol in SYMBOLS {
            universe.insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    qip_financial::asset_class::InstrumentType::CommonStock,
                )
                .venue("XNYS")
                .sector(qip_financial::asset_class::Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("performance", start()))
                .build(start())?,
            )?;
        }
        Ok(universe)
    }

    /// The last `keep` bars of one fixed `total`-bar path.
    ///
    /// Both platforms are fed suffixes of the *same* path so that, after
    /// retention, they hold byte-identical series — otherwise the regime
    /// fit's data-dependent EM iteration count would differ between two tapes
    /// and read here as a difference retention caused.
    fn bars_tail(symbol: &str, total: usize, keep: usize) -> Vec<SensedRecord> {
        let mut price = 100.0_f64;
        (0..total)
            .map(|index| {
                let noise = ((index as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
                let open = price;
                price *= 1.0 + noise;
                let at = start().saturating_sub(Duration::from_days((total - index) as i64));
                SensedRecord::Bar(Box::new(Bar {
                    object_id: object(symbol),
                    venue: "XNYS".to_string(),
                    interval: Interval::Day,
                    open_time: at,
                    open: Decimal::from_f64(open).expect("representable"),
                    high: Decimal::from_f64(open.max(price) * 1.002).expect("representable"),
                    low: Decimal::from_f64(open.min(price) * 0.998).expect("representable"),
                    close: Decimal::from_f64(price).expect("representable"),
                    volume: dec!("1000000"),
                    trade_count: 5_000,
                    vwap: Decimal::from_f64((open + price) / 2.0),
                    quality: qip_financial::quality::DataQuality::default(),
                }))
            })
            .skip(total - keep)
            .collect()
    }

    /// The last `keep` quotes of one fixed `total`-quote path.
    fn quotes_tail(symbol: &str, total: usize, keep: usize) -> Vec<SensedRecord> {
        (0..total)
            .map(|index| {
                let wiggle = ((index as f64 * 0.618) % 1.0 - 0.5) * 0.02;
                SensedRecord::Quote(Quote {
                    object_id: object(symbol),
                    venue: "XNYS".to_string(),
                    at: start().saturating_sub(Duration::from_secs((total - index) as i64)),
                    bid: Decimal::from_f64(99.9 + wiggle).expect("representable"),
                    ask: Decimal::from_f64(100.1 + wiggle).expect("representable"),
                    bid_size: dec!("500"),
                    ask_size: dec!("500"),
                    quality: qip_financial::quality::DataQuality::default(),
                })
            })
            .skip(total - keep)
            .collect()
    }

    fn platform_fed(bars_each: usize, quotes_each: usize) -> Result<Platform> {
        let config = PlatformConfig::default();
        let (context, _clock) = Context::deterministic(start(), config.seed);
        let mut platform = Platform::new(
            config,
            context,
            Telemetry::silent(),
            universe()?,
            risk_limits(),
        )?;
        for symbol in SYMBOLS {
            platform.observe(bars_tail(symbol, 2_416, bars_each));
            platform.observe(quotes_tail(symbol, 24_161, quotes_each));
        }
        Ok(platform)
    }

    fn cheapest_cycle(platform: &mut Platform) -> (WallDuration, usize) {
        let mut cheapest = WallDuration::MAX;
        let mut sensed = 0usize;
        for _ in 0..3 {
            let began = Instant::now();
            let report = platform.run_cycle(start());
            cheapest = cheapest.min(began.elapsed());
            sensed = report
                .stages
                .iter()
                .find(|stage| stage.stage == Stage::Sense)
                .map_or(0, |stage| stage.produced);
        }
        (cheapest, sensed)
    }

    // One platform at the bounds, one fed several times past them — the
    // second is the deployed evidence's shape (2,416 bars and 24,161 depth
    // observations per instrument at cycle 16,728).
    let mut at_bounds = platform_fed(512, 512)?;
    let mut past_bounds = platform_fed(2_416, 24_161)?;

    let (bounded, bounded_sensed) = cheapest_cycle(&mut at_bounds);
    let (grown, grown_sensed) = cheapest_cycle(&mut past_bounds);

    // The premise, exactly: retention capped the second platform's working
    // set to the first's. If the bounds are removed this fails before any
    // timing is read.
    assert!(bounded_sensed > 0, "the fixture fed the platform nothing");
    assert_eq!(
        grown_sensed, bounded_sensed,
        "a platform fed 4.7x more bars holds a larger sense working set than one fed at the \
         bounds; the history caps are not being applied at retention"
    );

    println!(
        "cycle at bounds: {bounded:?}; cycle fed 4.7x past bounds: {grown:?} \
         ({} profile, this machine, single-threaded)",
        profile()
    );
    report("kernel cycle (bounded history)", 1, bounded, 500_000.0);

    // Flatness, loosely: the two cycles walk identical working sets, so only
    // a series that escaped its bound — cost growing with what was fed rather
    // than with what is retained — can push this past double.
    let ratio = grown.as_secs_f64() / bounded.as_secs_f64().max(1e-9);
    assert!(
        ratio < 2.0,
        "a cycle over a 4.7x-larger feed costs {ratio:.1}x one at the bounds; per-cycle work \
         is growing with uptime again"
    );
    Ok(())
}

// --- the execution capabilities ---------------------------------------------
//
// `docs/ops/execution-measurements.md` is written from what this section
// prints. The traceability document scores every execution capability as
// TESTED and none as MEASURED; these are the first numbers, and they are
// in-process numbers on a shared container — a regression guard on the
// complexity class of each seam, never a deployment figure. Nothing is
// deployed (`execution_nodes = {}` in every environment), so there is no
// deployment figure to have.
//
// Same three rules as the stages above: assert a ceiling, print the number,
// name the profile. Every ceiling is one to two orders of magnitude above the
// unoptimised figure, so only a change of complexity class trips it. Each
// test asserts its premise — that the workload actually ran the number of
// items it claims — before it reads a clock, so a test that measured nothing
// cannot print a fast number for it.

const EDGE_CELL: &str = "perf-cell-1";
const EDGE_REGION: &str = "europe-west2";
const EDGE_ENVELOPE_KEY: &[u8] = b"a-performance-envelope-key";
const EDGE_POLICY_KEY: &[u8] = b"a-performance-policy-key";

/// A book quoting 99 / 101 for `symbol`, so the mid is 100 and the touch on
/// either side has a known size for the depth rule.
fn edge_book(symbol: &str) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(symbol), venue(), VenueStatus::Open);
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "500"), (BookSide::Ask, "101", "400")]
            .iter()
            .enumerate()
    {
        let when = start().saturating_add(Duration::from_millis(index as i64));
        state.apply(&MarketMessage::new(
            object(symbol),
            Origin::new(venue(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side: *side,
                price: d(price),
                quantity: d(size),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    Ok(state)
}

/// A strategy whose one rule always holds, so every pass raises the same
/// signal at the same size — the workload is then the pass, not the market.
fn firing_strategy(
    id: &str,
    symbol: &str,
    kind: SignalKind,
    size: &str,
) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(symbol), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            kind,
            Expr::Flag(true),
            Expr::Exact(d(size)),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// An envelope wide enough that the grant is never what refuses over a long
/// loop: a hundred million gross, a hundred thousand per order.
fn edge_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            EDGE_CELL,
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
    let unsigned = build("unsigned")?;
    let signature = sign_payload(EDGE_ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(
        build(&signature)?,
        EDGE_ENVELOPE_KEY,
        EDGE_CELL,
        start().saturating_add(Duration::from_secs(1)),
    )
}

/// A cell holding the ACME book and one always-firing strategy per entry,
/// each priced as given.
fn edge_cell(strategies: &[(&str, SignalKind, &str, PricingPolicy)]) -> Result<Cell> {
    let config = CellConfig::new(EDGE_CELL, EDGE_REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.track(edge_book("ACME")?);
    for (id, kind, size, pricing) in strategies {
        let (compiled, program) = firing_strategy(id, "ACME", *kind, size)?;
        cell.deploy_with_pricing(compiled, program, edge_envelope(id)?, *pricing)?;
    }
    Ok(cell)
}

/// The paper venue these passes run against.
///
/// It is the venue, so what it reports on the order-entry channel is the
/// venue's own answer: with `fills` set, every accepted order is reported
/// filled in full at its limit on the next drain, which is what lets a
/// thousand passes settle rather than pile up under `MAX_OPEN_ORDERS`.
/// Without it, an order rests until the cell withdraws it, which is the
/// expiry path. Either way it has a cancel path, so the cell lets an order
/// rest at all.
#[derive(Debug, Default)]
struct PaperVenue {
    fills: bool,
    pending: Vec<ExecutionReport>,
    resting: BTreeMap<String, Decimal>,
    accepted: usize,
    cancelled: usize,
}

impl Placer for PaperVenue {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        self.accepted += 1;
        if self.fills {
            self.pending.push(ExecutionReport {
                order_id: order_id.to_string(),
                venue: venue.clone(),
                quantity,
                price,
                at,
            });
        } else {
            self.resting.insert(order_id.to_string(), quantity);
        }
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.pending)
    }

    fn can_cancel(&self) -> bool {
        true
    }

    fn cancel(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _at: Timestamp,
    ) -> Result<Decimal> {
        let remaining = self
            .resting
            .remove(order_id)
            .ok_or_else(|| Error::not_found(format!("no order {order_id} is resting")))?;
        self.cancelled += 1;
        Ok(remaining)
    }
}

/// What a run of passes produced, and what the two timed seams cost.
#[derive(Debug, Default)]
struct PassTotals {
    orders: usize,
    contributors: usize,
    fills: usize,
    crosses: usize,
    cancelled: usize,
    refusals: usize,
    /// `Cell::work` alone: confirm, expire, evaluate, gate, net, cross, send.
    work: WallDuration,
    /// The drop copy observed and reconciled, and closed orders settled.
    reconcile: WallDuration,
}

/// Run `passes` passes `step` apart, reconciling the drop copy after each.
///
/// The drop copy is the venue's other channel; here it agrees with what the
/// order-entry channel reported, so every pass's comparison is clean and
/// every closed order is settled. A break would halt the cell, and a halted
/// cell sends nothing, so the assertion is in the loop rather than after it:
/// a hundred quiet passes after a halt would otherwise read as fast.
fn run_passes(
    cell: &mut Cell,
    gateway: &mut PaperVenue,
    passes: usize,
    step: Duration,
) -> Result<PassTotals> {
    let mut totals = PassTotals::default();
    let mut now = start().saturating_add(Duration::from_secs(2));
    for pass in 0..passes {
        let began = Instant::now();
        let report = cell.work(now, gateway)?;
        totals.work += began.elapsed();

        let began = Instant::now();
        for fill in &report.fills {
            cell.observe_drop_copy(DropCopyFill {
                order_id: fill.order_id.clone(),
                venue: fill.venue.clone(),
                quantity: fill.quantity,
                price: fill.price,
                at: now,
            });
        }
        let breaks = cell.reconcile(now);
        totals.reconcile += began.elapsed();

        assert!(
            breaks.is_empty(),
            "pass {pass}: the drop copy disagreed with the order-entry channel: {breaks:?}"
        );
        assert!(!cell.is_halted(), "pass {pass}: the cell halted");
        totals.orders += report.orders.len();
        totals.contributors += report
            .orders
            .iter()
            .map(|order| order.contributors.len())
            .sum::<usize>();
        totals.fills += report.fills.len();
        totals.crosses += report.crosses.len();
        totals.cancelled += report.cancelled.len();
        totals.refusals += report.refusals.len();
        now = now.saturating_add(step);
    }
    Ok(totals)
}

/// The risk state every central submission is judged against: ten million of
/// equity, two positions, plenty of room. Constant across the loop because
/// the manager does not move it — the kernel does, between cycles.
fn central_state() -> RiskState {
    RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        gross_exposure: dec!("4000000"),
        net_exposure: dec!("1000000"),
        position_notionals: BTreeMap::from([
            ("obj-ACME".to_string(), dec!("400000")),
            ("obj-BOREAS".to_string(), dec!("600000")),
        ]),
        ..RiskState::default()
    }
}

fn central_order(manager: &mut OrderManager, symbol: &str, quantity: Decimal) -> Order {
    let order_id = manager.next_order_id("perf");
    Order::new(
        order_id,
        object(symbol),
        Side::Buy,
        quantity,
        OrderType::Market,
        dec!("100"),
        "prop-performance",
        vec!["hyp-performance".to_string()],
        "performance",
        start(),
    )
}

#[test]
fn central_order_submission_costs_what_the_execution_measurements_say() -> Result<()> {
    // The single path to a venue on the central plane: validate, the kill
    // switch, the autonomy level, five pre-trade limits, the state machine,
    // and the simulated venue's fill. Frictionless settings so the venue
    // rejects nothing and fills everything: the workload is then the manager,
    // not a coin the simulator flips.
    const ORDERS: usize = 20_000;
    let mut manager = OrderManager::new(PreTradeChecker::new(risk_limits()));
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 0xC0DE);
    let autonomy = AutonomyController::new();
    let state = central_state();
    let axes = BTreeMap::from([("sector".to_string(), "information_technology".to_string())]);

    let mut accepted = 0usize;
    let mut filled = 0usize;
    let started = Instant::now();
    for _ in 0..ORDERS {
        let order = central_order(&mut manager, "ACME", dec!("1000"));
        let result = manager.submit(
            order,
            &mut broker,
            &autonomy,
            &state,
            axes.clone(),
            Some("broker-a".to_string()),
            start(),
        );
        if result.accepted {
            accepted += 1;
            filled += result.fills.len();
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        accepted,
        ORDERS,
        "the fixture was refused, so the submission path did not run: {:?}",
        manager.refusals().first()
    );
    assert_eq!(
        filled, ORDERS,
        "the frictionless venue did not fill every order"
    );
    assert!(
        !manager.has_live_fills(),
        "a simulated venue produced a fill marked real"
    );
    report(
        "central OMS submit (validate + 5 limits + simulated fill)",
        ORDERS,
        elapsed,
        500.0,
    );
    Ok(())
}

#[test]
fn central_instrument_feasibility_costs_what_the_execution_measurements_say() -> Result<()> {
    // The grid installed through `with_instrument_feasibility`, judged where
    // it sits in the submission path — ahead of the safety controls, so an
    // order the venue cannot express spends nothing downstream. Half the
    // orders are off-lot so the measurement covers both the refusal and the
    // admission; a fixture that was all one or the other would time only
    // the cheaper branch.
    const ORDERS: usize = 20_000;
    let grid = VenueFeasibility::new(dec!("1"), Some(dec!("0.01")), Decimal::ZERO, Decimal::ZERO)?;
    let mut manager = OrderManager::new(PreTradeChecker::new(risk_limits()))
        .with_instrument_feasibility("obj-ACME", grid);
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 0xFEA5);
    let autonomy = AutonomyController::new();
    let state = central_state();

    let mut admitted = 0usize;
    let mut refused_on_lot = 0usize;
    let started = Instant::now();
    for index in 0..ORDERS {
        let quantity = if index % 2 == 0 {
            dec!("1000")
        } else {
            dec!("1000.5")
        };
        let order = central_order(&mut manager, "ACME", quantity);
        let result = manager.submit(
            order,
            &mut broker,
            &autonomy,
            &state,
            BTreeMap::new(),
            None,
            start(),
        );
        if result.accepted {
            admitted += 1;
        } else if result
            .refusal
            .as_ref()
            .and_then(RefusalReason::feasibility_gate)
            == Some(central_feasibility::GATE_LOT)
        {
            refused_on_lot += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        refused_on_lot,
        ORDERS / 2,
        "the off-lot half was not refused under the lot rule: {:?}",
        manager.refusals().first()
    );
    assert_eq!(admitted, ORDERS / 2, "the on-lot half was not admitted");
    report(
        "central instrument feasibility (half off-lot)",
        ORDERS,
        elapsed,
        500.0,
    );
    Ok(())
}

#[test]
fn an_edge_work_pass_with_a_fill_and_its_drop_copy_costs_what_the_execution_measurements_say()
-> Result<()> {
    // One pass of the cell's loop, end to end, with one strategy that fires
    // every pass: the venue's reports confirmed, the degradation table read,
    // the strategy evaluated, the intent gated, the region hold taken, the
    // net built, the order sent, the hold committed. Then the pass's other
    // half, timed separately: the drop copy observed and reconciled against
    // the confirmed fill, and the closed order settled. The region table is
    // wired so the hold-and-commit path is inside the number.
    const PASSES: usize = 2_000;
    let opening = dec!("1000000000");
    let table = RegionTable::new(opening)?;
    let mut cell = edge_cell(&[("alpha", SignalKind::Enter, "100", PricingPolicy::Marketable)])?
        .with_region_table(table.clone());
    let mut gateway = PaperVenue {
        fills: true,
        ..PaperVenue::default()
    };

    let totals = run_passes(&mut cell, &mut gateway, PASSES, Duration::from_millis(1))?;

    assert_eq!(totals.orders, PASSES, "not every pass sent its order");
    assert_eq!(
        gateway.accepted, PASSES,
        "the venue did not see every order"
    );
    // The venue reports the fill on acceptance and the cell confirms it in
    // the same pass — so every pass confirms its own fill, and after the
    // reconcile that follows nothing is left open.
    assert_eq!(
        totals.fills, PASSES,
        "the venue's acceptance-time fills were not all confirmed"
    );
    assert!(
        cell.open_orders().is_empty(),
        "settled orders were not retired: {} still open",
        cell.open_orders().len()
    );
    assert!(
        table.committed_total().is_positive() && table.free() < opening,
        "no region hold was taken and committed on the order path"
    );
    assert_eq!(totals.refusals, 0, "a gate refused inside the loop");
    report(
        "edge work pass (1 strategy, marketable, region table)",
        PASSES,
        totals.work,
        5_000.0,
    );
    report(
        "edge drop-copy reconcile + settle (1 fill)",
        PASSES,
        totals.reconcile,
        1_000.0,
    );
    Ok(())
}

#[test]
fn netting_four_intents_into_one_order_costs_what_the_execution_measurements_say() -> Result<()> {
    // Four strategies that agree, netted into one order carrying four
    // contributors. The premise is the contributor count: a cell that sent
    // four orders would be four times the venue traffic and read here as a
    // slower pass, but the property under measurement is that it sent one.
    const PASSES: usize = 1_000;
    let mut cell = edge_cell(&[
        ("alpha", SignalKind::Enter, "100", PricingPolicy::Marketable),
        ("beta", SignalKind::Enter, "50", PricingPolicy::Marketable),
        ("gamma", SignalKind::Enter, "30", PricingPolicy::Marketable),
        ("delta", SignalKind::Enter, "20", PricingPolicy::Marketable),
    ])?;
    let mut gateway = PaperVenue {
        fills: true,
        ..PaperVenue::default()
    };

    let totals = run_passes(&mut cell, &mut gateway, PASSES, Duration::from_millis(1))?;

    assert_eq!(
        totals.orders, PASSES,
        "four agreeing intents did not net to one order"
    );
    assert_eq!(
        totals.contributors,
        4 * PASSES,
        "the net order does not carry all four contributors"
    );
    assert_eq!(totals.crosses, 0, "agreeing intents crossed");
    assert_eq!(totals.refusals, 0, "a gate refused inside the loop");
    report(
        "edge netting (4 intents -> 1 order, per pass)",
        PASSES,
        totals.work,
        10_000.0,
    );
    Ok(())
}

#[test]
fn an_internal_cross_costs_what_the_execution_measurements_say() -> Result<()> {
    // A hundred against forty, every pass: forty crosses inside the cell at
    // the mid and sixty goes to the venue. Under the per-net cap (forty of a
    // hundred and forty gross), so the cross is admitted on every pass and
    // `book_cross` moves both strategies' lots and cash each time.
    const PASSES: usize = 1_000;
    let mut cell = edge_cell(&[
        ("alpha", SignalKind::Enter, "100", PricingPolicy::Marketable),
        ("beta", SignalKind::Exit, "40", PricingPolicy::Marketable),
    ])?;
    let mut gateway = PaperVenue {
        fills: true,
        ..PaperVenue::default()
    };

    let totals = run_passes(&mut cell, &mut gateway, PASSES, Duration::from_millis(1))?;

    assert_eq!(
        totals.crosses, PASSES,
        "the offsetting portion did not cross every pass"
    );
    assert_eq!(
        totals.orders, PASSES,
        "the residual did not go to the venue every pass"
    );
    assert_eq!(totals.cancelled, 0, "a net cancelled to zero");
    assert_eq!(totals.refusals, 0, "a gate refused inside the loop");
    let alpha = cell.strategy_position(&StrategyId::new("alpha"), &venue(), &object("ACME"));
    let beta = cell.strategy_position(&StrategyId::new("beta"), &venue(), &object("ACME"));
    assert!(
        alpha.is_positive() && beta.is_negative(),
        "the cross moved no lots: alpha {alpha}, beta {beta}"
    );
    report(
        "edge internal cross (net + book_cross + residual order, per pass)",
        PASSES,
        totals.work,
        10_000.0,
    );
    Ok(())
}

#[test]
fn a_resting_orders_expiry_costs_what_the_execution_measurements_say() -> Result<()> {
    // An order rested at the mid for one second, and the next pass two
    // seconds later withdrawing it: `withdraw_expired` through the venue's
    // cancel, the region hold returned, the order closed and settled. The
    // premise is the venue's cancel count — an order the cell forgot rather
    // than withdrew would leave the venue holding it and read here as fast.
    const PASSES: usize = 1_000;
    let mut cell = edge_cell(&[(
        "alpha",
        SignalKind::Enter,
        "100",
        PricingPolicy::rest_at_mid(Duration::from_secs(1))?,
    )])?;
    let mut gateway = PaperVenue::default();

    let totals = run_passes(&mut cell, &mut gateway, PASSES, Duration::from_secs(2))?;

    assert_eq!(totals.orders, PASSES, "not every pass rested its order");
    assert_eq!(
        gateway.cancelled,
        PASSES - 1,
        "the venue was not asked to withdraw every expired order"
    );
    assert_eq!(totals.fills, 0, "a resting order filled on its own");
    assert_eq!(
        cell.open_orders().len(),
        1,
        "expired orders were not settled"
    );
    assert_eq!(totals.refusals, 0, "a gate refused inside the loop");
    report(
        "edge resting order expiry (rest, withdraw, settle, per pass)",
        PASSES,
        totals.work,
        5_000.0,
    );
    Ok(())
}

#[test]
fn the_edge_feasibility_gate_costs_what_the_execution_measurements_say() -> Result<()> {
    // The pure gate the cell judges every intent by before netting, on its
    // own: lot, tick, minimum, depth at the touch, fee floor. Half the intents
    // are off-lot, so both the refusal and the admission are inside the
    // number, and the premise counts each half under the rule that bound.
    const INTENTS: usize = 200_000;
    let model = VenueModel::new(
        VenueClass::Exchange,
        Granularity::new(dec!("1"), dec!("0.01"), Decimal::ZERO)?,
        Decimal::ZERO,
        None,
    )?;
    let intents: Vec<Intent> = (0..INTENTS)
        .map(|index| {
            Intent::new(
                StrategyId::new("alpha"),
                object("ACME"),
                venue(),
                if index % 2 == 0 {
                    dec!("10")
                } else {
                    dec!("10.5")
                },
                dec!("100"),
                start().saturating_add(Duration::from_secs(30)),
            )
        })
        .collect::<Result<_>>()?;

    let mut admitted = 0usize;
    let mut refused_on_lot = 0usize;
    let started = Instant::now();
    for intent in &intents {
        match edge_feasibility::assess(Some(&model), None, intent, Some(dec!("400"))) {
            Ok(()) => admitted += 1,
            Err(infeasible) if infeasible.gate == edge_feasibility::GATE_LOT => {
                refused_on_lot += 1;
            }
            Err(infeasible) => panic!("refused under an unexpected rule: {infeasible:?}"),
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        refused_on_lot,
        INTENTS / 2,
        "the off-lot half was not refused"
    );
    assert_eq!(admitted, INTENTS / 2, "the on-lot half was not admitted");
    report(
        "edge feasibility gate (half off-lot)",
        INTENTS,
        elapsed,
        20.0,
    );
    Ok(())
}

#[test]
fn a_region_reservation_hold_and_commit_costs_what_the_execution_measurements_say() -> Result<()> {
    // The per-region ledger on the order path: one hold taken before the
    // order exists and committed when it goes out, through the mutex every
    // cell in the region shares. The premise is the ledger's own arithmetic —
    // what was committed is exactly what left the free balance.
    const HOLDS: usize = 200_000;
    let opening = dec!("1000000000");
    let table = RegionTable::new(opening)?;
    let amount = dec!("100");

    let mut committed = 0usize;
    let started = Instant::now();
    for pass in 0..HOLDS {
        table.reserve(EDGE_CELL, "perf-hold", amount, pass as u64)?;
        if table.commit(EDGE_CELL, "perf-hold").is_some() {
            committed += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(committed, HOLDS, "a hold was taken and never committed");
    let expected = amount * Decimal::from_int(HOLDS as i64);
    assert_eq!(table.committed_total(), expected);
    assert_eq!(table.free(), opening - expected);
    assert_eq!(
        table.held_total(),
        Decimal::ZERO,
        "a hold is still standing"
    );
    report(
        "region reservation (reserve + commit)",
        HOLDS,
        elapsed,
        20.0,
    );
    Ok(())
}

#[test]
fn sequencing_a_contiguous_stream_costs_what_the_execution_measurements_say() -> Result<()> {
    // The sequencer on the feed path: every message checked against the
    // stream's position and released in order. Contiguous on purpose — the
    // common case is the one every packet pays for, and a gap's cost is a
    // property of the reorder policy rather than of the tracker.
    const MESSAGES: usize = 200_000;
    const BATCH: usize = 100;
    let stream = level_stream("ACME", MESSAGES, 0x5E0);
    let mut sequencer = Sequencer::new(ReorderPolicy::default());

    let mut released = 0usize;
    let mut events: Vec<SequenceEvent> = Vec::new();
    let started = Instant::now();
    for chunk in stream.chunks(BATCH) {
        let batch = sequencer.accept(chunk.to_vec(), start());
        released += batch.released.len();
        events.extend(batch.events);
    }
    let elapsed = started.elapsed();

    assert_eq!(
        released, MESSAGES,
        "the sequencer held back messages of a contiguous stream"
    );
    assert!(
        events
            .iter()
            .all(|event| matches!(event, SequenceEvent::StreamStarted { .. })),
        "a contiguous stream produced a gap or a duplicate: {events:?}"
    );
    assert_eq!(events.len(), 1, "the stream started more than once");
    report(
        "sequencing (contiguous, batches of 100)",
        MESSAGES,
        elapsed,
        20.0,
    );
    Ok(())
}

#[test]
fn arbitrating_two_redundant_lines_costs_what_the_execution_measurements_say() -> Result<()> {
    // Two lines carrying the same stream, the A line always first: every
    // unit is published once from A and recognised as a duplicate from B.
    // The premise is both counts, so a line the arbiter silently dropped —
    // which would halve the work — cannot read as fast.
    const MESSAGES: usize = 100_000;
    const BATCH: usize = 100;
    let stream = level_stream("ACME", MESSAGES, 0xA5B);
    // The window must be wider than a batch: line B's copy of a unit arrives
    // a whole batch after line A's, and a unit that has already left the
    // window is a `Missed`, not a duplicate.
    let mut arbiter = LineArbiter::new("feed-a", &["line-a", "line-b"], 4 * BATCH);

    let mut released = 0usize;
    let mut published = 0usize;
    let mut duplicates = 0usize;
    let started = Instant::now();
    for chunk in stream.chunks(BATCH) {
        for line in ["line-a", "line-b"] {
            let outcome = arbiter.accept(line, chunk.to_vec(), start());
            released += outcome.released.len();
            for event in &outcome.events {
                match event {
                    ArbitrationEvent::Published { .. } => published += 1,
                    ArbitrationEvent::Duplicate { .. } => duplicates += 1,
                    other => panic!("two clean lines produced {other:?}"),
                }
            }
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(released, MESSAGES, "the merged stream is not the stream");
    assert_eq!(
        published, MESSAGES,
        "not every unit was published exactly once"
    );
    assert_eq!(
        duplicates, MESSAGES,
        "the second line's copies were not all recognised"
    );
    report(
        "line arbitration (2 lines, per delivered unit)",
        2 * MESSAGES,
        elapsed,
        20.0,
    );
    Ok(())
}

#[test]
fn verifying_a_capital_envelope_costs_what_the_execution_measurements_say() -> Result<()> {
    // The signature check every grant passes before a cell will deploy on it:
    // the HMAC over the signing payload, the constant-time comparison, the
    // cell and the validity window. The admission arithmetic behind it is the
    // capital stage above; this is the trust root in front of it.
    const ENVELOPES: usize = 20_000;
    let envelope = {
        let build = |signature: &str| {
            CapitalEnvelope::new(
                StrategyId::new("alpha"),
                EDGE_CELL,
                dec!("1000000"),
                dec!("100000"),
                dec!("50000"),
                vec![venue()],
                start(),
                start().saturating_add(Duration::from_hours(1)),
                "alice@example.com",
                signature,
            )
        };
        let unsigned = build("unsigned")?;
        build(&sign_payload(
            EDGE_ENVELOPE_KEY,
            &unsigned.signing_payload(),
        ))?
    };
    let now = start().saturating_add(Duration::from_secs(1));

    let mut verified = 0usize;
    let started = Instant::now();
    for _ in 0..ENVELOPES {
        if VerifiedEnvelope::verify(envelope.clone(), EDGE_ENVELOPE_KEY, EDGE_CELL, now).is_ok() {
            verified += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        verified, ENVELOPES,
        "a correctly signed envelope was refused"
    );
    assert!(
        VerifiedEnvelope::verify(envelope, b"another-key", EDGE_CELL, now).is_err(),
        "the check under measurement accepts any key"
    );
    report("capital envelope verify (HMAC)", ENVELOPES, elapsed, 500.0);
    Ok(())
}

#[test]
fn verifying_and_applying_a_policy_payload_costs_what_the_execution_measurements_say() -> Result<()>
{
    // The centre's payload arriving at the cell: the signature over the
    // canonical serialisation, the anti-replay sequence, the halt barrier,
    // the narrowing recorded, the chain entry sealed. One per sequence,
    // strictly increasing, which is the only order the cell accepts.
    const PAYLOADS: usize = 2_000;
    let mut cell = edge_cell(&[])?;
    let issued = start().saturating_add(Duration::from_secs(2));
    let payloads: Vec<PolicyPayload> = (1..=PAYLOADS as u64)
        .map(|sequence| {
            PolicyPayload::unproduced(sequence, EDGE_CELL, issued).signed(EDGE_POLICY_KEY)
        })
        .collect::<Result<_>>()?;

    let mut applied = 0usize;
    let started = Instant::now();
    for payload in payloads {
        let verified = VerifiedPolicy::verify(payload, EDGE_POLICY_KEY, EDGE_CELL, issued)?;
        cell.apply_policy(verified, issued)?;
        applied += 1;
    }
    let elapsed = started.elapsed();

    assert_eq!(applied, PAYLOADS);
    assert_eq!(
        cell.policy_sequence(),
        Some(PAYLOADS as u64),
        "the cell did not apply every payload in sequence"
    );
    report("policy payload verify + apply", PAYLOADS, elapsed, 2_000.0);
    Ok(())
}

#[test]
fn the_journal_chain_costs_what_the_execution_measurements_say() -> Result<()> {
    // The hash chain under every decision: a record sealed onto the previous
    // digest, the whole chain re-verified, and the unshipped tail handed to a
    // mirror in one batch that names what it chains onto. Three numbers
    // because they are three different costs on three different paths — the
    // record is on the pass, the verify is what a replay pays, the ship is
    // the one call that may block.
    const RECORDS: usize = 50_000;
    let decisions: Vec<Decision> = (0..RECORDS)
        .map(|index| Decision::Refused {
            gate: "performance".to_string(),
            reason: format!("record {index} of a measured chain"),
        })
        .collect();
    let mut journal = Journal::new();

    let started = Instant::now();
    for decision in decisions {
        journal.record(decision, start());
    }
    let recording = started.elapsed();

    let started = Instant::now();
    let verified = journal.verify();
    let verifying = started.elapsed();

    let mut mirror = MemoryMirror::new();
    let started = Instant::now();
    let shipped = ship(&mut journal, &mut mirror, EDGE_CELL, Vec::new(), start())?;
    let shipping = started.elapsed();

    assert_eq!(journal.len(), RECORDS, "not every decision was recorded");
    assert_eq!(
        verified,
        Ok(()),
        "the chain the test just wrote does not verify"
    );
    assert_eq!(shipped, RECORDS, "the mirror did not receive every entry");
    mirror.verify_continuity()?;
    assert!(
        journal.unshipped().is_empty(),
        "entries were shipped and not marked"
    );
    report("journal record (chain digest)", RECORDS, recording, 200.0);
    report("journal verify (per entry)", RECORDS, verifying, 200.0);
    report(
        "journal ship to mirror (per entry)",
        RECORDS,
        shipping,
        50.0,
    );
    Ok(())
}

#[test]
fn a_two_leg_group_completing_costs_what_the_execution_measurements_say() -> Result<()> {
    // The multi-leg lifecycle: two orders assembled into a group, each leg
    // filled in full, the group assessed and settled complete. Buy and sell
    // of equal notional, so the leg risk between the fills is bounded by the
    // group's own limit and the verdict is completion rather than an unwind.
    const GROUPS: usize = 20_000;
    let fill = |order: &Order, index: usize| Fill {
        fill_id: FillId::from_string(format!("fill-{index}-{}", order.order_id.as_str())),
        order_id: order.order_id.clone(),
        at: start(),
        quantity: order.quantity,
        price: order.arrival_price,
        costs: Decimal::ZERO,
        venue: "simulated-venue".to_string(),
        simulated: true,
    };

    let mut complete = 0usize;
    let started = Instant::now();
    for index in 0..GROUPS {
        let leg = |symbol: &str, side: Side| {
            Order::new(
                OrderId::from_string(format!("leg-{index}-{symbol}")),
                object(symbol),
                side,
                dec!("100"),
                OrderType::Market,
                dec!("100"),
                "prop-performance",
                vec!["hyp-performance".to_string()],
                "performance",
                start(),
            )
        };
        let buy = leg("ACME", Side::Buy);
        let sell = leg("BOREAS", Side::Sell);
        let buy_fill = fill(&buy, index);
        let sell_fill = fill(&sell, index);
        let mut group = LegGroup::new(
            format!("group-{index}"),
            vec![buy, sell],
            start().saturating_add(Duration::from_secs(60)),
            dec!("1000000"),
        )?;
        group.record_fill(&buy_fill)?;
        group.record_fill(&sell_fill)?;
        let verdict = group.assess(start());
        group.settle(&verdict, &[], start())?;
        if verdict == Verdict::Complete {
            complete += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        complete, GROUPS,
        "a fully filled group was not judged complete"
    );
    report(
        "multi-leg group (2 legs: assemble, fill, assess, settle)",
        GROUPS,
        elapsed,
        200.0,
    );
    Ok(())
}

// --- the second honesty check ------------------------------------------------

/// The test-name cell of every figure row in the execution measurements
/// document, with its backticks stripped.
fn measurement_rows(document: &str) -> Vec<String> {
    document
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let cells: Vec<&str> = line.strip_prefix('|')?.split('|').collect();
            let capability = cells.first()?.trim().to_lowercase();
            if capability.is_empty()
                || capability == "capability"
                || capability.chars().all(|c| c == '-' || c == ':')
            {
                return None;
            }
            let test = cells.get(5)?.trim().trim_matches('`').to_string();
            Some(test)
        })
        .collect()
}

#[test]
fn the_execution_measurements_document_names_only_tests_this_file_holds_and_says_what_a_number_is_not()
 {
    // The document is the deliverable this section justifies, so it is
    // checked rather than trusted, in both directions: every row names a
    // test this file holds, and every measurement this file makes has a row.
    // A row for a test that no longer exists is a figure nothing produces —
    // exactly what the budgets document once did for a stage ADR 0029 had
    // removed — and a measurement without a row is a number nobody can find.
    let document = qip_acceptance::read("docs/ops/execution-measurements.md");
    let source = qip_acceptance::read("backend/crates/tests/qip-acceptance/tests/performance.rs");
    const SUFFIX: &str = "_costs_what_the_execution_measurements_say";

    let measuring: Vec<&str> = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("fn "))
        .filter_map(|rest| rest.split('(').next())
        .filter(|name| name.ends_with(SUFFIX))
        .collect();
    assert!(
        measuring.len() >= 10,
        "this file holds only {} execution measurements; the section was cut down",
        measuring.len()
    );

    let rows = measurement_rows(&document);
    assert!(!rows.is_empty(), "the document has no figure rows");
    for row in &rows {
        assert!(
            measuring.contains(&row.as_str()),
            "docs/ops/execution-measurements.md publishes a row for `{row}`, which this file \
             does not measure; a figure for a test nothing runs is a wish dressed as a \
             measurement"
        );
    }
    for name in &measuring {
        assert!(
            rows.iter().any(|row| row == name),
            "docs/ops/execution-measurements.md has no row for `{name}`"
        );
    }

    // And the caveats a reader has to meet before any number: the shape of
    // the machine, the profile, and the two sentences that stop an in-process
    // figure being read as a deployment figure.
    let lowered = document.to_lowercase();
    for required in [
        "4 cores",
        "release",
        "not a deployment measurement",
        "nothing is deployed",
        "2026-09-05",
    ] {
        assert!(
            lowered.contains(required),
            "docs/ops/execution-measurements.md does not say \"{required}\""
        );
    }
    for overclaim in ["tick-to-order", "wire-to-wire", "microseconds at the venue"] {
        assert!(
            !lowered.contains(overclaim),
            "docs/ops/execution-measurements.md claims \"{overclaim}\""
        );
    }
}
