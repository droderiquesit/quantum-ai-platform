//! Stress: the named ways this platform is asked to fail.
//!
//! `resilience.rs` asks whether the loop survives load, a degraded feed and an
//! operator pulling things out from under it. This file asks a narrower and
//! harsher question about the same platform: when a specific thing breaks —
//! a region, a broker, a feed, the cash, the model, the centre, the quantum
//! device — does it *refuse*, or does it guess?
//!
//! One property unifies every test here, and it is worth stating before the
//! first one:
//!
//! > **A failure must make the platform refuse, not guess.**
//!
//! So no test asserts merely that nothing panicked. Each names the specific
//! safe outcome the failure has to produce — an order not sent, a book that
//! serves no price, a scope halted, a refusal recorded with its gate and its
//! reason — and fails if the platform carried on with a number it invented.
//! "Nothing crashed" is the assertion that lets a system that silently made
//! something up pass.
//!
//! Two honesty notes, because a stress suite is the easiest place in a
//! repository to overclaim.
//!
//! * The one throughput test here **measures and prints** what it achieved, on
//!   whatever machine ran it, in whatever profile it was built in. It asserts
//!   a ceiling loose enough that only a genuine regression trips it, exactly as
//!   `qip-orderbook`'s `throughput.rs` does. Nothing in this repository has
//!   measured a million events per second, and nothing here claims to.
//! * Where a scenario cannot reach the assembled path — because a seam is not
//!   wired yet — the test says so in its comment and asserts the refusal the
//!   unwired seam produces, rather than pretending the path exists. Two such
//!   seams are documented below, at the tests that found them.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_agents::finding::{AgentFinding, Direction};
use qip_ai::registry::{EvaluationRecord, ModelCard, ModelRegistry, ModelStage};
use qip_arbitrage::graph::{ArbitrageGraph, Node, VenueFacts};
use qip_arbitrage::liquidity::{LiquiditySource, StaticLiquidity};
use qip_arbitrage::netedge::{EdgeAssumptions, NetEdgeCalculator};
use qip_arbitrage::plan::{LegPlanner, PlanSettings};
use qip_arbitrage::pricing::price_path;
use qip_arbitrage::scan::{OpportunityScanner, RejectionStage, SizePolicy};
use qip_arbitrage::search::{SearchSettings, search_candidates};
use qip_contracts::capital::{CapitalEnvelope, CapitalGrant, Utilisation};
use qip_contracts::edge::{DeductionKind, LegStep};
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::ids::{AgentRunId, HypothesisId, ModelId, ObjectId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Currency, Decimal, ManualClock, Money, dec};
use qip_edge::cell::{Cell, CellConfig, Placer};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::seam::CellLiquidity;
use qip_execution_engine::broker::{SimulatedBroker, SimulationSettings};
use qip_execution_engine::oms::{OrderManager, RefusalReason};
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::features::BookPressure;
use qip_feature_dag::state::MarketState;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::SERIES_HISTORY;
use qip_kernel::{Platform, PlatformConfig};
use qip_learning_engine::feedback::{LessonCandidate, PromotionBar};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_numerics::anneal::Qubo;
use qip_observability::Telemetry;
use qip_optimization_engine::problem::{Objective, PortfolioProblem};
use qip_optimization_engine::router::{ComputeRouter, RoutingPolicy, Solver};
use qip_quantum::provider::{HostedConfig, HostedProvider, QuantumProvider};
use qip_quantum::qaoa::QaoaSettings;
use qip_reasoning_engine::engine::{ReasoningEngine, SynthesisInput};
use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet, Stance};
use qip_reasoning_engine::hypothesis::{CausalChain, CausalStep, Claim};
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::autonomy::AutonomyController;
use qip_risk_engine::pretrade::{PreTradeChecker, PreTradeDecision, ProposedOrder};
use qip_sequencing::tracker::{ReorderPolicy, SequenceEvent, Sequencer};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_world_model::causal::Mechanism;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

// --- the instants, identities and fixtures every test shares ----------------

const CELL: &str = "london-1";
const ENVELOPE_KEY: &[u8] = b"a-stress-suite-envelope-signing-key";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn t(secs: i64) -> Timestamp {
    start().saturating_add(Duration::from_secs(secs))
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{name}"))
}

fn venue(name: &str) -> VenueId {
    VenueId::new(name)
}

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("a decimal literal in a fixture")
}

/// Which profile the binary was built in, for anything it prints.
///
/// A measured number without its profile is not a measurement: the same code
/// is several times slower unoptimised, and a reader who cannot tell which one
/// produced a figure cannot use it.
fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

fn universe(symbols: &[&str]) -> Universe {
    let mut universe = Universe::new();
    for symbol in symbols {
        universe
            .insert(
                FinancialObject::builder(object(symbol), *symbol, InstrumentType::CommonStock)
                    .venue("XNYS")
                    .sector(Sector::InformationTechnology)
                    .price(dec!("100"))
                    .provenance(Provenance::synthetic("stress", start()))
                    .build(start())
                    .expect("valid instrument"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("stress")
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
}

fn platform_over(symbols: &[&str]) -> Result<Platform> {
    let clock = Arc::new(ManualClock::new(start()));
    let config = PlatformConfig::default();
    let context = Context::new(clock, config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(symbols),
        limits(),
    )
}

/// One synthetic bar. Deterministic, so a run reproduces exactly.
fn bar(symbol: &str, at: Timestamp, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(close).expect("representable open"),
        high: Decimal::from_f64(close * 1.003).expect("representable high"),
        low: Decimal::from_f64(close * 0.997).expect("representable low"),
        close: Decimal::from_f64(close).expect("representable close"),
        volume: dec!("2500000"),
        trade_count: 8_000,
        vwap: Decimal::from_f64(close),
        quality: qip_financial::quality::DataQuality::default(),
    }))
}

/// A level-set message, the smallest thing a book learns from.
fn level(
    symbol: &str,
    at: VenueId,
    feed: &str,
    sequence: u64,
    side: BookSide,
    price: &str,
    size: &str,
    when: Timestamp,
) -> MarketMessage {
    MarketMessage::new(
        object(symbol),
        Origin::new(at, feed, 0, sequence),
        MessageBody::LevelSet {
            side,
            price: d(price),
            quantity: d(size),
            order_count: None,
        },
        when,
        when,
    )
}

/// A two-sided book at a venue, built the way a book is really built: from
/// messages, because there is deliberately no setter that bypasses the feed.
fn book_at(venue_name: &str, symbol: &str) -> qip_orderbook::venue::VenueState {
    let id = venue(venue_name);
    let mut state =
        qip_orderbook::venue::VenueState::aggregated(object(symbol), id.clone(), VenueStatus::Open);
    let levels = [
        (BookSide::Bid, "99", "500"),
        (BookSide::Bid, "98", "800"),
        (BookSide::Ask, "101", "400"),
        (BookSide::Ask, "102", "900"),
    ];
    for (index, (side, price, size)) in levels.iter().enumerate() {
        state
            .apply(&level(
                symbol,
                id.clone(),
                "feed-a",
                index as u64,
                *side,
                price,
                size,
                t(index as i64),
            ))
            .expect("a well-formed level");
    }
    state
}

/// An envelope signed the way the central allocator would sign it.
fn signed_envelope(cell: &str, gross: &str, order: &str) -> Result<CapitalEnvelope> {
    let terms = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("stress-strategy"),
            cell,
            d(gross),
            d(order),
            dec!("50000"),
            vec![venue("XLON")],
            start(),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = terms("unsigned")?;
    terms(&sign_payload(ENVELOPE_KEY, &unsigned.signing_payload()))
}

/// A cell assembled the way `qip-edge-node` assembles one.
fn cell(venues: &[&str]) -> Result<Cell> {
    let mut config = CellConfig::new(CELL, "europe-west2");
    for name in venues {
        config = config.with_venue(venue(name));
    }
    Cell::new(
        config,
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
    )
}

/// A one-rule strategy, compiled by the real compiler.
fn compiled_strategy() -> Result<(CompiledStrategy, qip_strategy::program::Program)> {
    let subject = object("ACME");
    let pressure =
        qip_contracts::FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;

    let spec = StrategySpec::new(
        StrategyId::new("stress-strategy"),
        subject,
        Duration::from_millis(250),
    )
    .with_rule(Rule::new(
        "enter",
        qip_contracts::SignalKind::Enter,
        Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
        Expr::Exact(dec!("100")),
        Expr::Statistic(0.62),
        500,
    ));
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// One crypto venue whose ETH/BTC cross is a percent from the two dollar legs
/// that imply it: a real triangular dislocation with real spreads.
///
/// `asks` is the ETHUSDT offer ladder, which is where the slippage test thins
/// the touch. Everything else is held fixed so the only thing that changes
/// between the profitable case and the refused one is the depth.
fn triangular(asks: &[(&str, &str)]) -> Result<(ArbitrageGraph, StaticLiquidity)> {
    use qip_market::book::{BookLevel, OrderBook};

    let cx = venue("CX");
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        cx.clone(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    let node = |name: &str| Node::new(ObjectId::from_string(name), cx.clone());

    graph.add_trade(
        node("USDT"),
        node("ETH"),
        d("0.000333328"),
        d("0.0004"),
        ObjectId::from_string("ETHUSDT"),
        BookSide::Ask,
        start(),
        20,
    )?;
    graph.add_trade(
        node("ETH"),
        node("BTC"),
        d("0.050505"),
        d("0.0004"),
        ObjectId::from_string("ETHBTC"),
        BookSide::Bid,
        start(),
        20,
    )?;
    graph.add_trade(
        node("BTC"),
        node("USDT"),
        d("60000.5"),
        d("0.0004"),
        ObjectId::from_string("BTCUSDT"),
        BookSide::Bid,
        start(),
        20,
    )?;

    let book = |market: &str, bids: &[(&str, &str)], asks: &[(&str, &str)]| {
        OrderBook::from_levels(
            ObjectId::from_string(market),
            "CX",
            start(),
            bids.iter()
                .map(|(p, s)| BookLevel::new(d(p), d(s)))
                .collect(),
            asks.iter()
                .map(|(p, s)| BookLevel::new(d(p), d(s)))
                .collect(),
        )
    };
    let depth = StaticLiquidity::new()
        .with_book(cx.clone(), book("ETHUSDT", &[("3000.0", "200")], asks), 20)
        .with_book(
            cx.clone(),
            book("ETHBTC", &[("0.0505", "200")], &[("0.05051", "200")]),
            20,
        )
        .with_book(
            cx,
            book("BTCUSDT", &[("60000", "10")], &[("60001", "10")]),
            20,
        );
    Ok((graph, depth))
}

/// A gateway that works for `succeed_first` orders and is then out.
///
/// The shape of a broker outage as a cell experiences it: the session was fine
/// a millisecond ago, the first leg is away, and the second call fails.
#[derive(Debug)]
struct OutageGateway {
    succeed_first: usize,
    placed: Vec<LegStep>,
}

impl OutageGateway {
    fn new(succeed_first: usize) -> Self {
        Self {
            succeed_first,
            placed: Vec::new(),
        }
    }

    /// Send one leg, or fail the way a dropped order-entry session fails.
    fn send(&mut self, step: &LegStep) -> Result<()> {
        if self.placed.len() >= self.succeed_first {
            return Err(Error::unavailable(format!(
                "the order-entry session to {} is down; no acknowledgement for {}",
                step.venue.as_str(),
                step.object_id.as_str()
            )));
        }
        self.placed.push(step.clone());
        Ok(())
    }
}

impl Placer for OutageGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        Err(Error::unavailable("the order-entry session is down"))
    }
}

// --- a very high synthetic event rate ---------------------------------------

#[test]
fn a_very_high_synthetic_event_rate_is_absorbed_without_losing_an_event() -> Result<()> {
    // Load is only interesting here if it can make the platform lie. The
    // failure this catches is a fast path that drops observations under
    // pressure and reports a count that looks fine: SENSE would say a smaller
    // number, every downstream stage would reason about a shorter series, and
    // nothing would ever say a bar went missing.
    //
    // The honest claim: this is a wall-clock measurement of one process on
    // whatever machine ran it, printed rather than asserted. The assertion is
    // a ceiling loose enough that only a genuine regression — an accidental
    // clone of the history per record, a linear scan where there was a map —
    // can trip it. It is not a latency benchmark and it is not evidence of a
    // million events per second; see `docs/performance/budgets.md`.
    const EVENTS: usize = 250_000;
    const CEILING_SECONDS: f64 = 120.0;

    let names = ["ACME", "BOREAS", "CERES", "DORIS", "ERATO"];
    let mut platform = platform_over(&names)?;

    // Built before the clock starts, so the measurement is absorption rather
    // than fixture construction.
    let batches: Vec<Vec<SensedRecord>> = names
        .iter()
        .enumerate()
        .map(|(index, symbol)| {
            (0..EVENTS / names.len())
                .map(|i| {
                    let phase = (i + index * 37) as f64;
                    let close = 100.0 + ((phase * 0.7548776662) % 1.0 - 0.5) * 4.0;
                    bar(
                        symbol,
                        start().saturating_sub(Duration::from_secs(i as i64)),
                        close,
                    )
                })
                .collect()
        })
        .collect();

    let started = Instant::now();
    let mut absorbed = 0usize;
    for batch in batches {
        absorbed += platform.observe(batch);
    }
    let elapsed = started.elapsed();

    // The property, and it is exact: every event or a stated shortfall. A
    // count that merely looks plausible is what a silent drop produces.
    assert_eq!(
        absorbed, EVENTS,
        "{} of {EVENTS} events were absorbed; the rest vanished without a word",
        absorbed
    );

    let seconds = elapsed.as_secs_f64();
    println!(
        "sense ingest: {EVENTS} events in {seconds:.3}s = {:.0} events/s \
         ({:.0} ns/event, {} profile, this machine, single-threaded)",
        EVENTS as f64 / seconds,
        seconds * 1e9 / EVENTS as f64,
        profile()
    );
    assert!(
        seconds < CEILING_SECONDS,
        "absorbing {EVENTS} events took {seconds:.3}s, past the {CEILING_SECONDS}s ceiling"
    );

    // And the platform still reports the whole of what it holds — which, since
    // the retention bound landed, is not the whole of what it absorbed. The two
    // numbers are different properties and both are asserted: everything was
    // absorbed (above, exactly), and what is *held* is the declared bound,
    // saturated. A stage that quietly truncated under load would report fewer
    // than the bound and would show up here and nowhere else.
    let report = platform.run_cycle(start());
    assert!(
        report.traversed_every_stage(),
        "a stage stopped running at rate:\n{}",
        report.summarise()
    );
    let sense = report.stage(Stage::Sense).expect("sense ran");
    let held = SERIES_HISTORY * names.len();
    assert!(
        held < EVENTS,
        "the premise: this load must exceed the bound, or saturation proves nothing"
    );
    assert_eq!(
        sense.produced,
        held,
        "sense holds {} observations; the bound implies {held} ({SERIES_HISTORY} per series \
         across {} instruments), so the working set was truncated below policy \
         rather than by it: {}",
        sense.produced,
        names.len(),
        sense.detail
    );
    Ok(())
}

// --- a region offline, and regional network loss ----------------------------

#[test]
fn a_region_that_goes_completely_offline_is_halted_by_scope_and_the_others_keep_trading()
-> Result<()> {
    // The failure mode a global platform has and a single-region one does not:
    // one region stops answering, and the response is either too small (that
    // region keeps trading on a book nobody can confirm) or too large (the
    // whole platform stops because one data centre did). The kill switch is
    // scoped precisely so neither is the only option.
    let mut autonomy = AutonomyController::new();
    autonomy.kill_switch_mut().trip_scope(
        "region:europe-west2",
        start(),
        "regional-health",
        "every cell in europe-west2 stopped reporting; its books cannot be confirmed",
    );

    let mut orders = OrderManager::new(PreTradeChecker::new(limits()));
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 7);
    let risk_state = RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        ..RiskState::default()
    };

    let order_in = |scope: &str, id: &str| {
        Order::new(
            qip_core::ids::OrderId::from_string(id),
            object("ACME"),
            Side::Buy,
            dec!("1000"),
            OrderType::Market,
            dec!("100"),
            "prop-stress",
            vec!["hyp-stress".to_string()],
            scope,
            start(),
        )
    };

    let dark = orders.submit(
        order_in("region:europe-west2", "ord-dark"),
        &mut broker,
        &autonomy,
        &risk_state,
        BTreeMap::new(),
        None,
        start(),
    );
    assert!(
        !dark.accepted,
        "an order reached a region that is not there"
    );
    assert!(
        dark.fills.is_empty(),
        "a refused order produced {} fill(s)",
        dark.fills.len()
    );
    let refusal = dark.refusal.expect("a refusal without a reason is not one");
    assert!(
        matches!(refusal, RefusalReason::Halted { .. }),
        "the region was refused for the wrong reason: {}",
        refusal.describe()
    );
    // Distinguished from a transient fault: a safety refusal must never be
    // retried automatically, and the outage is exactly when something would.
    assert!(
        refusal.is_safety_control(),
        "the regional halt was recorded as a retryable fault: {}",
        refusal.describe()
    );
    assert!(
        refusal.describe().contains("stopped reporting"),
        "the refusal did not carry why the region was halted: {}",
        refusal.describe()
    );

    // And the platform is not globally stopped: another region still trades.
    assert!(
        !autonomy.kill_switch().is_globally_tripped(),
        "one region going dark stopped the whole platform"
    );
    assert!(autonomy.may_execute("region:us-east1"));
    let live = orders.submit(
        order_in("region:us-east1", "ord-live"),
        &mut broker,
        &autonomy,
        &risk_state,
        BTreeMap::new(),
        None,
        start(),
    );
    assert!(
        live.accepted,
        "a healthy region was stopped by another region's outage: {:?}",
        live.refusal
    );
    assert_eq!(
        autonomy.kill_switch().halted_scopes(),
        vec!["region:europe-west2"]
    );
    Ok(())
}

#[test]
fn regional_network_loss_leaves_every_book_in_the_region_serving_no_price_at_all() -> Result<()> {
    // Losing the feed is not losing the book: the data structure is still
    // there, full of prices from before the loss, and they look exactly like
    // facts. The dangerous answer is the last good mid; the correct one is
    // "this source is ignorant", which is what `None` means to the pricer.
    let mut liquidity = CellLiquidity::new();
    for symbol in ["ACME", "BOREAS", "CERES"] {
        liquidity.insert(book_at("XLON", symbol));
    }
    let xlon = venue("XLON");

    // While the region is reachable the books price normally, so the assertion
    // below is about the loss rather than about an empty fixture.
    for symbol in ["ACME", "BOREAS", "CERES"] {
        assert!(
            liquidity.mid(&xlon, &object(symbol)).is_some(),
            "{symbol} had no price before the region was lost"
        );
    }

    for state in liquidity.iter_mut() {
        state.reset("the region's feed stopped arriving and the book cannot be confirmed");
    }

    for symbol in ["ACME", "BOREAS", "CERES"] {
        let subject = object(symbol);
        let state = liquidity.get(&xlon, &subject).expect("still tracked");
        assert!(state.is_stale(), "{symbol} did not go stale on feed loss");
        assert!(
            state
                .reset_reason()
                .is_some_and(|reason| reason.contains("stopped arriving")),
            "{symbol} went stale without recording why"
        );

        // Every read a router or a pricer takes, and every one of them empty.
        assert!(liquidity.mid(&xlon, &subject).is_none());
        assert!(liquidity.touch(&xlon, &subject, BookSide::Ask).is_none());
        assert!(
            liquidity
                .sweep_cost(&xlon, &subject, BookSide::Ask, dec!("100"))
                .is_none(),
            "{symbol} offered depth from before the region was lost"
        );
        assert!(
            liquidity.as_of(&xlon, &subject).is_none(),
            "{symbol} claimed to know when it last looked"
        );
        assert_eq!(liquidity.observations(&xlon, &subject), 0);
    }
    Ok(())
}

// --- a broker outage, and a half-executed arbitrage --------------------------

#[test]
fn a_broker_outage_mid_arbitrage_stops_the_plan_and_names_the_exposure_it_stranded() -> Result<()> {
    // A cycle is only flat once every leg is on. Losing the session between
    // the first leg and the second leaves a position nobody decided to take,
    // and the thing that makes that survivable is knowing — before the first
    // order is sent — exactly how much would be stranded and in what units.
    let (graph, depth) = triangular(&[("3000.1", "200")])?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    assert_eq!(candidates.len(), 1, "the fixture holds one dislocation");
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;

    let planned = LegPlanner::new(PlanSettings::with_budget(d("1000000"))).plan(&pricing)?;
    assert!(
        planned.plan.len() >= 3,
        "a triangular cycle has three legs, not {}",
        planned.plan.len()
    );

    // The plan is ordered hardest-to-undo first, and it says so. A plan whose
    // ordering has to be reverse-engineered is a plan nobody will check.
    assert!(
        !planned.rationale.is_empty(),
        "the plan gave no account of its own ordering"
    );

    // Now the outage, between leg one and leg two.
    let mut gateway = OutageGateway::new(1);
    let mut sent = 0usize;
    let mut stopped_at: Option<(usize, String)> = None;
    for (index, step) in planned.plan.steps().iter().enumerate() {
        match gateway.send(step) {
            Ok(()) => sent += 1,
            Err(error) => {
                stopped_at = Some((index, error.message().to_string()));
                break;
            }
        }
    }

    let (index, reason) = stopped_at.expect("the outage must stop the plan");
    assert_eq!(sent, 1, "the plan kept sending into a dead session");
    assert_eq!(index, 1, "the outage was noticed at the wrong leg");
    assert!(
        reason.contains("order-entry session"),
        "the stop did not say what failed: {reason}"
    );
    // Not retried, and not continued: the remaining legs are never sent. What
    // makes that safe is that the exposure it leaves was bounded in advance.
    assert_eq!(
        gateway.placed.len(),
        1,
        "more than one leg reached a venue that was down"
    );

    // The residual the planner computed before anything was sent is the number
    // an operator has to act on, and it is stated per unit of account rather
    // than as one sum across incomparable ones.
    assert!(
        planned.residual_risk.is_positive(),
        "a plan that can strand a position reported no residual"
    );
    assert!(
        !planned.residual_by_quote.is_empty(),
        "the residual was reported as a number with no units"
    );
    let summed = planned
        .residual_by_quote
        .iter()
        .map(|(_, amount)| *amount)
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(
        summed, planned.residual_risk,
        "the per-currency breakdown does not add up to the residual it explains"
    );
    for (quote, amount) in &planned.residual_by_quote {
        assert!(
            !quote.as_str().is_empty() && amount.is_positive(),
            "a residual component named no instrument"
        );
    }
    Ok(())
}

#[test]
fn an_arbitrage_leg_the_book_cannot_fill_is_refused_rather_than_part_executed() -> Result<()> {
    // The partial fill is the same failure as the outage arriving early: the
    // legs no longer net out and what is left is a position. So the pipeline
    // refuses the whole path rather than sizing down into it, and records that
    // depth is what stopped it.
    let (graph, depth) = triangular(&[("3000.1", "200")])?;

    // Asked at a size the books cannot cover on every leg at once.
    let report = OpportunityScanner::new(
        SearchSettings::default(),
        EdgeAssumptions::default(),
        PlanSettings::with_budget(d("100000000")),
    )
    .scan(&graph, &depth, &SizePolicy::uniform(d("10000000")), start());

    assert!(
        report.opportunities.is_empty(),
        "{} opportunity(ies) survived a book that cannot fill them",
        report.opportunities.len()
    );
    let refused = report.rejected_at(RejectionStage::Depth);
    assert_eq!(
        refused.len(),
        1,
        "the path was not refused for the depth it lacked: {:?}",
        report.rejections
    );
    assert!(
        !refused[0].detail.trim().is_empty(),
        "the depth refusal carries no reason"
    );

    // And the pricing itself says which legs could not be filled, rather than
    // reporting a smaller opportunity.
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000000"))?;
    assert!(
        !pricing.is_fully_available(),
        "a path larger than the book reported itself fully available"
    );
    Ok(())
}

#[test]
fn slippage_ten_times_wider_turns_a_profitable_path_into_a_recorded_refusal() -> Result<()> {
    // A gap is a market where the touch is suddenly a token and the real size
    // is several levels back. Nothing about the quoted rates changes, which is
    // why a system that prices on them keeps finding the same opportunity all
    // the way down. The pricer walks the book, so the same graph produces a
    // profit before the gap and a refusal after it.
    let calculator = NetEdgeCalculator::new(EdgeAssumptions::default());
    let slippage_on = |asks: &[(&str, &str)]| -> Result<(Decimal, Decimal)> {
        let (graph, depth) = triangular(asks)?;
        let candidates = search_candidates(&graph, &SearchSettings::default());
        let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
        let edge = calculator.calculate(&pricing, start())?;
        let slippage = edge
            .deductions()
            .iter()
            .find(|deduction| deduction.kind == DeductionKind::Slippage)
            .map(|deduction| deduction.amount)
            .unwrap_or(Decimal::ZERO);
        Ok((slippage, pricing.end_quantity))
    };

    // Before: a deep touch that covers the whole size.
    let (calm_slippage, calm_end) = slippage_on(&[("3000.1", "200")])?;
    assert!(
        calm_end > d("10000"),
        "the fixture must pay before the gap: {calm_end} back on 10000"
    );

    // After: a token at the touch and the size ten percent worse behind it.
    let (gapped_slippage, gapped_end) = slippage_on(&[("3000.1", "0.01"), ("3300", "500")])?;

    assert!(
        gapped_slippage > calm_slippage * Decimal::from_int(10),
        "the gap did not multiply the slippage deduction: {calm_slippage} became {gapped_slippage}"
    );
    assert!(
        gapped_end < d("10000"),
        "the path still claimed to pay through a tenfold gap: {gapped_end} back on 10000"
    );

    // And the scanner refuses it rather than sizing into it.
    let (graph, depth) = triangular(&[("3000.1", "0.01"), ("3300", "500")])?;
    let report = OpportunityScanner::new(
        SearchSettings::default(),
        EdgeAssumptions::default(),
        PlanSettings::with_budget(d("1000000")),
    )
    .scan(&graph, &depth, &SizePolicy::uniform(d("10000")), start());
    assert!(
        report.opportunities.is_empty(),
        "a path that loses money through the gap was still offered"
    );
    assert!(
        !report.rejections.is_empty(),
        "the path vanished without a recorded refusal"
    );
    for rejection in &report.rejections {
        assert!(
            !rejection.detail.trim().is_empty(),
            "a refusal at {:?} carries no reason",
            rejection.stage
        );
    }
    Ok(())
}

// --- stale data, a duplicate feed, a corrupt sequence ------------------------

#[test]
fn stale_data_is_refused_as_a_price_rather_than_used_as_one() -> Result<()> {
    // Staleness is the most dangerous kind of wrong, because a stale price is
    // a well-formed number that every downstream check accepts. The book has
    // to be the thing that refuses, since by the time a strategy sees a mid it
    // has no way to tell how old it is.
    let mut state = book_at("XLON", "ACME");
    assert!(
        state.mid().is_some(),
        "the fixture must price before it is stale"
    );
    assert!(state.prices_are_usable());

    state.reset("the last update is older than this cell tolerates");

    assert!(state.is_stale());
    assert!(
        !state.prices_are_usable(),
        "a stale book called its prices usable"
    );
    assert!(state.mid().is_none(), "a stale book served a mid");
    assert!(state.best_bid().is_none() && state.best_ask().is_none());
    assert!(
        state
            .sweep_cost(BookSide::Ask, dec!("100"))
            .is_none_or(|sweep| !sweep.filled.is_positive()),
        "a stale book offered a fill"
    );

    // The cell's own router gate reads the same book, and the refusal it would
    // record names staleness rather than a price.
    let mut liquidity = CellLiquidity::new();
    liquidity.insert(state);
    let held = liquidity
        .get(&venue("XLON"), &object("ACME"))
        .expect("tracked");
    assert!(
        held.reset_reason()
            .is_some_and(|reason| reason.contains("older than")),
        "the stale book did not say why it stopped being usable"
    );
    Ok(())
}

#[test]
fn a_duplicated_market_feed_is_recognised_rather_than_applied_twice() -> Result<()> {
    // Redundant A/B lines are normal, and both carry the same message. Applied
    // twice, a level set is idempotent and a trade print is not: session
    // volume doubles, and every feature computed from it is wrong in a way no
    // downstream check can see. The sequencer is what has to notice.
    let mut sequencer = Sequencer::new(ReorderPolicy::default());
    let xlon = venue("XLON");

    let line = |sequence: u64| {
        level(
            "ACME",
            xlon.clone(),
            "feed-a",
            sequence,
            BookSide::Bid,
            "99",
            "500",
            t(sequence as i64),
        )
    };
    let primary: Vec<MarketMessage> = (0..8).map(line).collect();
    let redundant = primary.clone();

    let first = sequencer.accept(primary, t(10));
    assert_eq!(first.released.len(), 8, "the primary line was not released");

    let second = sequencer.accept(redundant, t(11));
    assert!(
        second.released.is_empty(),
        "the redundant line was released again: {} message(s)",
        second.released.len()
    );
    assert!(
        second
            .events
            .iter()
            .any(|event| matches!(event, SequenceEvent::Duplicate { .. })),
        "a duplicate was dropped without being recorded as one: {:?}",
        second.events
    );

    // And the book applied it once, which is the number that would be wrong.
    let mut state = qip_orderbook::venue::VenueState::aggregated(
        object("ACME"),
        xlon.clone(),
        VenueStatus::Open,
    );
    for message in first.released.iter().chain(second.released.iter()) {
        state.apply(message)?;
    }
    assert_eq!(
        state.applied(),
        8,
        "the book absorbed the redundant line as new messages"
    );
    Ok(())
}

#[test]
fn a_corrupt_sequence_invalidates_the_book_rather_than_being_absorbed() -> Result<()> {
    // A hole in the sequence means the book is missing changes it will never
    // see. Carrying on produces a book that is wrong in an unknown direction —
    // the one state from which a confident price is worse than no price.
    let mut sequencer = Sequencer::new(ReorderPolicy::new(4, Duration::from_secs(1)));
    let xlon = venue("XLON");
    let line = |sequence: u64, price: &str| {
        level(
            "ACME",
            xlon.clone(),
            "feed-a",
            sequence,
            BookSide::Bid,
            price,
            "500",
            t(sequence as i64),
        )
    };

    let opening = sequencer.accept(vec![line(0, "99"), line(1, "99")], t(2));
    assert_eq!(opening.released.len(), 2);

    // Sequences 2..=40 never arrive; 41 does. The tracker holds, then gives up.
    let after_gap = sequencer.accept(vec![line(41, "150")], t(3));
    assert!(
        after_gap.released.is_empty(),
        "a message from beyond a hole was released as if the hole were not there"
    );
    assert!(
        after_gap
            .events
            .iter()
            .any(|event| matches!(event, SequenceEvent::GapOpened { .. })),
        "the hole was not recorded: {:?}",
        after_gap.events
    );

    // Past the reorder window the gap is abandoned, which is the event that
    // invalidates the book rather than merely annotating it.
    let abandoned = sequencer.poll(t(3600));
    assert!(
        abandoned
            .events
            .iter()
            .any(|event| matches!(event, SequenceEvent::GapAbandoned { .. })),
        "the hole was held open forever instead of being abandoned: {:?}",
        abandoned.events
    );

    let mut state = qip_orderbook::venue::VenueState::aggregated(
        object("ACME"),
        xlon.clone(),
        VenueStatus::Open,
    );
    for message in &opening.released {
        state.apply(message)?;
    }
    state.reset("a sequence gap was abandoned; the book is missing changes it will never see");

    assert!(state.is_stale());
    assert!(
        state.mid().is_none(),
        "a book missing an unknown number of changes still served a price"
    );
    assert!(
        state
            .reset_reason()
            .is_some_and(|reason| reason.contains("never see")),
        "the invalidation did not record what caused it"
    );
    Ok(())
}

// --- cash and FX -------------------------------------------------------------

#[test]
fn cash_below_its_buffer_refuses_the_order_instead_of_borrowing_silently() -> Result<()> {
    // The failure this stops is the quietest one in the file: an order that
    // fits every exposure limit and simply cannot be paid for. Without a cash
    // floor it goes through, the shortfall becomes leverage nobody chose, and
    // the first anyone hears of it is a margin call.
    let checker = PreTradeChecker::new(
        LimitSet::new("stress-cash").with(
            Limit::new("cash-buffer", LimitKind::MinCashBuffer { limit: 0.10 })
                .with_rationale("a tenth of equity stays in cash so a settlement never fails"),
        ),
    );
    let state = RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        ..RiskState::default()
    };
    let order = ProposedOrder {
        object_id: object("ACME"),
        quantity: dec!("95000"),
        reference_price: dec!("100"),
        axes: BTreeMap::new(),
        counterparty: None,
        scope: "stress".to_string(),
    };

    let result = checker.check(&order, &state, start())?;
    assert!(
        !result.is_approved(),
        "an order that leaves 5% of equity in cash passed a 10% floor"
    );
    let PreTradeDecision::Rejected { reasons } = &result.decision else {
        panic!("expected a refusal, got {}", result.decision.describe());
    };
    assert!(
        reasons.iter().any(|reason| reason.contains("cash-buffer")),
        "the refusal did not name the limit that stopped it: {reasons:?}"
    );
    assert_eq!(
        result.decision.permitted_quantity(order.quantity),
        Decimal::ZERO,
        "a refused order was still given a size"
    );

    // The same check is what the OMS runs, so the refusal reaches the venue
    // path rather than living only in the risk crate.
    let mut orders = OrderManager::new(PreTradeChecker::new(
        LimitSet::new("stress-cash").with(
            Limit::new("cash-buffer", LimitKind::MinCashBuffer { limit: 0.10 })
                .with_rationale("a tenth of equity stays in cash so a settlement never fails"),
        ),
    ));
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 11);
    let submitted = orders.submit(
        Order::new(
            qip_core::ids::OrderId::from_string("ord-cash"),
            object("ACME"),
            Side::Buy,
            dec!("95000"),
            OrderType::Market,
            dec!("100"),
            "prop-stress",
            vec!["hyp-stress".to_string()],
            "stress",
            start(),
        ),
        &mut broker,
        &AutonomyController::new(),
        &state,
        BTreeMap::new(),
        None,
        start(),
    );
    assert!(
        !submitted.accepted,
        "an unaffordable order reached the broker"
    );
    let refusal = submitted.refusal.expect("a refusal with a reason");
    assert!(
        matches!(refusal, RefusalReason::RiskRejected { .. }) && refusal.is_safety_control(),
        "the shortfall was recorded as something retryable: {}",
        refusal.describe()
    );
    Ok(())
}

#[test]
fn an_amount_in_a_currency_nobody_can_convert_is_refused_rather_than_summed_at_par() -> Result<()> {
    // There is no ambient FX service, and that is the design: `Money::convert`
    // takes the rate as an argument, so a missing rate is a value that does not
    // exist rather than a silent 1.0. What has to be true on top of that is
    // that the arithmetic refuses to add across currencies, because the failure
    // here — a book summed at par when the rate was unavailable — produces a
    // total that looks entirely reasonable and is wrong by the exchange rate.
    let dollars = Money::new(dec!("1000000"), Currency::USD);
    let pounds = Money::new(dec!("800000"), Currency::GBP);

    let refusal = dollars
        .checked_add(pounds)
        .expect_err("two currencies were added without a rate");
    assert!(
        refusal.message().contains("USD") && refusal.message().contains("GBP"),
        "the refusal did not name both currencies: {}",
        refusal.message()
    );
    assert!(dollars.checked_sub(pounds).is_err());

    // With a rate stated by the caller it converts, and the result carries the
    // currency it was converted into rather than the one it came from.
    let converted = pounds.convert(Currency::USD, d("1.27"));
    assert_eq!(converted.currency, Currency::USD);
    assert_eq!(converted.amount, dec!("1016000"));
    assert_eq!(dollars.checked_add(converted)?.amount, dec!("2016000"));

    // The same rule holds where it matters most: a cycle whose legs are priced
    // in different instruments reports its stranded exposure per unit of
    // account, so a leg-risk budget is never compared against a sum across
    // incomparable units.
    let (graph, depth) = triangular(&[("3000.1", "200")])?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    let planned = LegPlanner::new(PlanSettings::with_budget(d("1000000"))).plan(&pricing)?;
    assert!(
        planned
            .residual_by_quote
            .iter()
            .all(|(quote, _)| !quote.as_str().is_empty()),
        "a residual component was reported without the unit it is in"
    );
    Ok(())
}

// --- a contradictory recommendation ------------------------------------------

#[test]
fn a_recommendation_its_own_organisation_contradicts_does_not_clear_the_action_bar() -> Result<()> {
    // Two models, one bullish and one bearish on the same instrument, is the
    // normal state of a research organisation rather than a malfunction. The
    // failure is a synthesis that averages them into a confident number: the
    // disagreement is the finding, and it has to survive into the decision.
    let mut engine = ReasoningEngine::new(ReviewPolicy::default());

    let finding = |id: &str, direction: Direction, conviction: f64| {
        AgentFinding::new(
            AgentRunId::from_string(format!("run-{id}")),
            id,
            start(),
            start(),
            "ACME's guidance does not reflect its funding costs",
        )
        .with_direction(direction, conviction)
    };

    let outcome = engine.reason(SynthesisInput {
        hypothesis_id: HypothesisId::from_string("hyp-contradiction"),
        opportunity_id: None,
        as_of: start(),
        now: start(),
        class: "funding-cost-pass-through".to_string(),
        claim: Claim::Overvalued,
        statement: "ACME is overvalued against its floating-rate funding".to_string(),
        subjects: vec![object("ACME")],
        chain: CausalChain::new(vec![CausalStep::new(
            "policy-rate",
            "obj-ACME",
            Mechanism::CreditConditions,
            "gross margin compresses in the next quarterly report",
            Duration::from_days(20),
            0.75,
        )]),
        findings: vec![
            finding("credit-analyst", Direction::Positive, 0.8),
            finding("equity-analyst", Direction::Negative, 0.8),
        ],
        direct_evidence: EvidenceSet::from_items(vec![
            Evidence::new(
                qip_core::ids::EvidenceId::from_string("ev-filing"),
                EvidenceKind::Filing,
                Stance::Supports,
                "the filing discloses floating-rate debt",
                "rec-filing",
                "sec-edgar",
                start(),
                start(),
            )
            .with_reliability(0.9)
            .with_diagnosticity(0.6),
        ]),
        prior: 0.25,
        falsifiers: vec!["the next quarterly report shows flat gross margin".to_string()],
        leading_alternative: "the market has already priced the funding structure".to_string(),
        horizon: Duration::from_days(60),
        market_priced_in: None,
        models: vec!["credit-model@1".to_string(), "equity-model@1".to_string()],
    })?;

    // The dissent is carried, by name, rather than being netted away.
    assert!(
        !outcome.dissenters.is_empty(),
        "an agent that argued the opposite way left no trace in the outcome"
    );

    // And the outcome cannot be acted on — with a reason a person can read,
    // which is the whole point: "did not meet the bar" is not actionable.
    let refusal = engine
        .clears_action_bar(&outcome)
        .expect_err("a contradicted thesis cleared the bar");
    assert!(
        refusal.len() > 10,
        "the refusal said nothing useful: {refusal:?}"
    );
    assert!(
        outcome.narrate().contains("Dissent from"),
        "the decision record does not mention the disagreement: {}",
        outcome.narrate()
    );
    Ok(())
}

// --- the centre unreachable from a cell --------------------------------------

#[test]
fn a_cell_cut_off_from_the_centre_spends_only_its_grant_and_then_stops() -> Result<()> {
    // A cell decides alone, so "the global brain is unreachable" is not an
    // outage for it — it is the normal case, and the reason it is safe is that
    // the cell never decides how much it may risk. The bound is the envelope,
    // and the backstop on a cell nobody can reach is the expiry, checked
    // against the cell's own clock. A recall is a request; expiry is not.
    let envelope = VerifiedEnvelope::verify(
        signed_envelope(CELL, "1000", "400")?,
        ENVELOPE_KEY,
        CELL,
        t(10),
    )?;
    let xlon = venue("XLON");
    let fresh = Utilisation::default();

    // Inside the window and inside the grant: the cell acts without asking.
    assert!(matches!(
        envelope.admit(&xlon, d("300"), &fresh, t(10)),
        CapitalGrant::Full
    ));

    // Still cut off, and now past the grant it holds: reduced, then refused.
    let mostly_used = Utilisation {
        gross_committed: d("800"),
        realised_loss: Decimal::ZERO,
        orders_sent: 2,
    };
    assert!(matches!(
        envelope.admit(&xlon, d("400"), &mostly_used, t(20)),
        CapitalGrant::Reduced(_)
    ));
    let exhausted = Utilisation {
        gross_committed: d("1000"),
        realised_loss: Decimal::ZERO,
        orders_sent: 3,
    };
    assert!(
        envelope
            .admit(&xlon, d("10"), &exhausted, t(30))
            .is_refused(),
        "a cell out of contact kept spending past its grant"
    );

    // And past expiry it stops entirely, whatever headroom is left. This is
    // the property that bounds a cell the centre can no longer talk to.
    assert!(!envelope.is_live(t(3600)));
    match envelope.admit(&xlon, d("1"), &fresh, t(3600)) {
        CapitalGrant::Refused(reason) => assert!(
            reason.contains("expired"),
            "the expiry refusal did not say it had expired: {reason}"
        ),
        other => panic!("an expired grant admitted capital: {other:?}"),
    }

    // A venue outside the grant is refused with headroom to spare, because the
    // two bounds are independent and an order must clear both.
    assert!(
        envelope
            .admit(&venue("XNYS"), d("1"), &fresh, t(10))
            .is_refused()
    );
    Ok(())
}
#[test]
fn a_cell_missing_what_its_strategy_needs_refuses_rather_than_guessing() -> Result<()> {
    // This test used to record a seam: `Cell::deploy` took a compiled strategy
    // and an envelope, and *not* the `Program` the strategy's plan indexes
    // into, so a cell could accept a strategy it was structurally unable to
    // evaluate and then refuse on every pass. The seam is now closed —
    // `deploy` takes the program and establishes at deployment every reason it
    // can establish without the market — so what remains here is the half that
    // genuinely depends on the market, plus a check that the closed half stays
    // closed.
    //
    // The point of the test is unchanged and is *which way it fails*. A vector
    // without the feature could have been read as a zero, and a plan node the
    // arena does not hold could have been read as whatever sits at that index.
    // Both would have produced an order. Instead each refuses, names the gate
    // it refused at, and sends nothing.
    let subject = object("ACME");
    let (strategy, program) = compiled_strategy()?;
    let grant = || -> Result<VerifiedEnvelope> {
        VerifiedEnvelope::verify(
            signed_envelope(CELL, "1000000", "100000")?,
            ENVELOPE_KEY,
            CELL,
            t(10),
        )
    };

    // 1. The arena does not hold the plan. Refused by the deployment, not by a
    //    market that happened to move first.
    let mut mismatched = cell(&["XLON"])?;
    let error = mismatched
        .deploy(
            strategy.clone(),
            qip_strategy::program::Program::default(),
            grant()?,
        )
        .expect_err("a plan was deployed against an arena that does not hold it");
    assert!(
        error.message().contains("do not belong together"),
        "the refusal did not name the mismatch: {error}"
    );
    assert!(
        mismatched.deployed_strategies().is_empty(),
        "a refused deployment was still recorded"
    );

    // 2. The program is right and the feature graph does not carry what the
    //    strategy reads. This one *cannot* be settled at deployment — a
    //    feature can be registered and still undefined for want of a quote —
    //    so it is a per-pass judgement, and the runtime distinguishes "the
    //    vector does not have it" from "it is undefined", because conflating
    //    them hides a strategy pointed at the wrong graph.
    let mut bare = cell(&["XLON"])?;
    bare.track(book_at("XLON", "ACME"));
    bare.deploy(strategy.clone(), program.clone(), grant()?)?;
    assert_eq!(bare.deployed_strategies(), vec!["stress-strategy"]);

    let mut gateway = OutageGateway::new(0);
    let report = bare.work(t(20), &mut gateway)?;
    assert!(
        report.orders.is_empty() && report.signals.is_empty(),
        "a cell whose graph lacks the feature still produced {} signal(s) and {} order(s)",
        report.signals.len(),
        report.orders.len()
    );
    let (gate, reason) = report
        .refusals
        .first()
        .expect("the cell did nothing and could not say why");
    assert_eq!(gate, "strategy_runtime");
    assert!(
        reason.contains("does not carry"),
        "the refusal did not name the missing input: {reason}"
    );

    // 3. Give it the feature, computed by the real engine from a real book,
    //    and the cell now decides: signal, gates, order. That is the seam
    //    closing — the same fixture that used to prove a cell could not
    //    evaluate its own strategy now proves it can.
    //
    //    `BookPressure` is a signed imbalance, `(bid - ask) / (bid + ask)`, so
    //    a merely two-sided book sits near zero and fires nothing. Nine
    //    hundred bid against three hundred offered is 0.5 — above the rule's
    //    threshold, and a shape a real book takes.
    let mut features = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    features.register(Box::new(BookPressure::new(subject.clone(), 5)))?;
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "900"), (BookSide::Ask, "101", "300")]
            .iter()
            .enumerate()
    {
        features.ingest(&level(
            "ACME",
            venue("XLON"),
            "feed-a",
            index as u64,
            *side,
            price,
            size,
            t(20),
        ))?;
    }
    assert!(
        features
            .evaluate(t(20))?
            .get(&BookPressure::key(&subject, 5))
            .is_some_and(|value| value.is_defined()),
        "the fixture must supply a defined feature, or step 3 repeats step 2"
    );

    let mut config = CellConfig::new(CELL, "europe-west2");
    config = config.with_venue(venue("XLON"));
    let mut fed = Cell::new(config, features)?;
    fed.track(book_at("XLON", "ACME"));
    fed.deploy(strategy, program, grant()?)?;

    // The gateway is deliberately one that accepts, because the assertion is
    // that the cell reached it at all.
    let mut accepting = RecordingGateway::default();
    let report = fed.work(t(20), &mut accepting)?;
    assert!(
        report.refusals.is_empty(),
        "a fully-equipped cell still refused: {:?}",
        report.refusals
    );
    assert_eq!(
        report.signals.len(),
        1,
        "the strategy did not fire on a book its rule matches"
    );
    let order = report
        .orders
        .first()
        .expect("a signal cleared every gate and no order was sent");
    assert!(
        order.simulated,
        "a paper cell reported a live order; the gateway's own answer is what sets this"
    );
    assert_eq!(order.venue, venue("XLON"));
    assert!(order.quantity.is_positive());
    assert_eq!(
        accepting.placed.len(),
        1,
        "the gateway saw a different count"
    );

    // The decision is recorded like a decision, so "why did this cell trade"
    // is answerable from the journal without re-running it — and the chain
    // still verifies, which is what makes the record evidence.
    assert!(
        fed.journal()
            .entries()
            .iter()
            .any(|entry| matches!(&entry.decision, Decision::OrderSent { .. })),
        "the order never reached the journal"
    );
    fed.journal()
        .verify()
        .map_err(|sequence| Error::invalid(format!("the journal chain broke at {sequence}")))?;
    Ok(())
}

/// A gateway that accepts everything and remembers what it was handed.
#[derive(Debug, Default)]
struct RecordingGateway {
    placed: Vec<(String, VenueId, Decimal, Decimal)>,
}

impl Placer for RecordingGateway {
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
        _at: Timestamp,
    ) -> Result<()> {
        self.placed
            .push((order_id.to_string(), venue.clone(), quantity, price));
        Ok(())
    }
}

// --- quantum and training ----------------------------------------------------

#[test]
fn quantum_being_unavailable_produces_a_classical_answer_that_says_why() -> Result<()> {
    // This build has no vendor SDK, no credential and no egress path. The
    // failure to avoid is an adapter that pretends: a simulated result
    // presented as hardware, or a fallback so quiet that nobody notices the
    // device was never reached. The port reports itself unavailable, names
    // what is missing, and the router still produces a decision.
    let hosted = HostedProvider::new(HostedConfig {
        vendor: "ibm-quantum".to_string(),
        backend: "ibm-heron".to_string(),
        credential_env: "QIP_QUANTUM_TOKEN".to_string(),
        endpoint: "https://example.invalid/api".to_string(),
        max_qubits: 64,
        cost_per_job_micros: 1_000,
    });

    assert!(!hosted.is_available());
    let requirement = hosted.requirement();
    assert!(
        requirement.contains("QIP_QUANTUM_TOKEN"),
        "the port did not name the credential it needs: {requirement}"
    );
    let refusal = hosted
        .solve_qubo(&Qubo::new(4), &QaoaSettings::default())
        .expect_err("an unreachable device returned an answer");
    assert_eq!(refusal.code(), "unavailable", "{}", refusal.message());

    // Even with a credential the transport is absent, so availability cannot
    // be talked into being true by configuration alone.
    let with_token = HostedProvider::with_credential(hosted.config().clone(), true);
    assert!(!with_token.is_available());
    assert!(with_token.requirement().contains("transport"));

    // And the decision still happens, classically, with the reason recorded.
    let assets: Vec<String> = (0..12).map(|i| format!("obj-A{i}")).collect();
    let covariance: Vec<Vec<f64>> = (0..12)
        .map(|i| {
            (0..12)
                .map(|j| if i == j { 0.04 } else { 0.04 * 0.3 })
                .collect()
        })
        .collect();
    let problem = PortfolioProblem::new(assets, covariance)?
        .with_objective(Objective::MeanVariance)
        .with_expected_returns((0..12).map(|i| 0.05 + 0.01 * i as f64).collect())
        .with_risk_aversion(5.0)
        .with_bounds(vec![0.0; 12], vec![0.3; 12])
        .fully_invested(1.0)
        .with_cardinality(4);

    let decision = ComputeRouter::classical(3)
        .with_policy(RoutingPolicy {
            minimum_assets_for_quantum: 4,
            exact_enumeration_limit: 8,
            ..RoutingPolicy::default()
        })
        .with_quantum(Arc::new(hosted))
        .solve(&problem)?;

    assert_ne!(
        decision.chosen,
        Solver::Quantum,
        "an unreachable device won the routing decision"
    );
    assert!(
        decision.weights.iter().any(|weight| weight.abs() > 1e-9),
        "the fallback produced no portfolio at all"
    );
    assert!(
        !decision.quantum_note.trim().is_empty(),
        "the decision did not record what happened to the quantum attempt"
    );
    assert!(
        decision.measured_quantum_advantage().is_none(),
        "an advantage was reported over a device that was never reached"
    );
    Ok(())
}

#[test]
fn training_that_misses_its_bar_promotes_nothing_and_names_the_shortfall() -> Result<()> {
    // "Training failed" is rarely an exception. It is a fit that ran, produced
    // numbers, and is not good enough — and the dangerous outcome is that it
    // ships anyway because nothing refused it. Two gates have to hold: the
    // lesson bar, and the model registry.
    let thin = LessonCandidate {
        applies_when: "the funding-cost detector fires on a floating-rate issuer".to_string(),
        statement: "the move persists for at least ten sessions".to_string(),
        supporting: vec!["eval-1".to_string(), "eval-2".to_string()],
        contradicting: vec!["eval-3".to_string()],
        distinct_contributors: 1,
        proposed_at: start(),
    };
    let shortfall = PromotionBar::default()
        .assess(&thin)
        .expect_err("a lesson from three episodes was promoted");
    assert!(
        shortfall.contains("supporting episode"),
        "the bar did not say what was short: {shortfall}"
    );

    // The registry is the second gate: a model whose evaluation did not pass
    // cannot be promoted, and one that is not in production cannot be used for
    // a decision however good its numbers look.
    let mut registry = ModelRegistry::new();
    let card = ModelCard::new(
        ModelId::from_string("mdl-stress"),
        "funding-cost",
        "3",
        "research@example.com",
        start(),
    )
    .with_purpose("estimates margin compression from floating-rate funding");
    let reference = card.reference();
    registry.register(card);
    registry.record_evaluation(
        &reference,
        EvaluationRecord {
            evaluated_at: start(),
            dataset: "holdout-2024".to_string(),
            metrics: BTreeMap::from([("brier_score".to_string(), 0.31)]),
            passed: false,
        },
    )?;

    let refusal = registry
        .promote(&reference, start())
        .expect_err("a model with a failed evaluation was promoted");
    assert!(
        refusal.message().contains("passing evaluation"),
        "the refusal did not say what was missing: {}",
        refusal.message()
    );
    assert_eq!(
        registry.get(&reference).map(|card| card.stage),
        Some(ModelStage::Development),
        "a refused promotion still moved the model's stage"
    );
    let unusable = registry
        .require_for_decision(&reference, start())
        .expect_err("a model that never passed was used for a decision");
    assert!(
        !unusable.message().is_empty(),
        "the model was refused without a reason"
    );

    // A passing evaluation promotes it, so the gate is a gate rather than a
    // wall — a control that refuses everything is not evidence of anything.
    registry.record_evaluation(
        &reference,
        EvaluationRecord {
            evaluated_at: start(),
            dataset: "holdout-2024".to_string(),
            metrics: BTreeMap::from([("brier_score".to_string(), 0.14)]),
            passed: true,
        },
    )?;
    registry.promote(&reference, start())?;
    registry.require_for_decision(&reference, start())?;

    // And drift past the threshold takes it back out of service, naming the
    // number, without anyone having to notice first.
    registry.record_drift(&reference, 0.9)?;
    let drifted = registry
        .require_for_decision(&reference, start())
        .expect_err("a drifted model stayed in service");
    assert!(
        drifted.message().contains("drift"),
        "the model was withdrawn without saying why: {}",
        drifted.message()
    );
    assert_eq!(registry.ineligible(start()).len(), 1);
    Ok(())
}
