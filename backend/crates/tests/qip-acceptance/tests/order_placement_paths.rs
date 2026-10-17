//! Every place an order reaches a placer or a broker in production code,
//! driven with an order the gate must refuse, and the call sites themselves
//! pinned so a new one cannot appear unclassified (W1.9, R12-RISK-4093).
//!
//! `order_route_risk_gate.rs` finds routes by the *shape* of the call
//! (`.submit(&order`, `place(` with `quantity` and `price` in its arguments).
//! A call written another way — a receiver split across lines, a method
//! reached through a new name, `submit_order` on the platform — is invisible
//! to it, and it classifies by enclosing function, so a second call inside an
//! already-listed function is invisible too. This file is the other half:
//!
//! * **Behavioural.** Each route that can be reached from a running process
//!   is driven with an order its gate refuses, and the sink — a placer, a
//!   broker, a venue's listed book — is asked whether it heard of the order.
//!   Each test first proves its own premise: the same route under a generous
//!   gate does reach the sink, so a refusal below is the gate and not the
//!   fixture.
//! * **Structural.** Every non-test occurrence of an order-sending method name
//!   anywhere under `backend/crates` is counted per file and must equal an
//!   allow-list entry that names the gate it passes through (or says it is
//!   not an order at all). A new call site, or a second one in a file already
//!   listed, fails here until somebody classifies it.
//!
//! The paper-trading boundary is not weakened by anything here: the tests
//! only ever hand orders to simulated sinks.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_arbitrage::{
    ArbitrageGraph, EdgeAssumptions, Node, OpportunityScanner, PlanSettings, SearchSettings,
    SizePolicy, VenueFacts,
};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, SystemClock, dec};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_edge::risk_gate::{CellRiskGate, GATE_RISK};
use qip_edge::telemetry::CellMetrics;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::reprice::{Requote, Requoter};
use qip_edge_node::risk::CellRiskLimits;
use qip_edge_node::{NodeAssembly, assemble_with_risk};
use qip_execution_engine::broker::{Broker, ListingVenue, SimulatedBroker, SimulationSettings};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::market_creation::{MarketFactory, PricingRule};
use qip_execution_engine::oms::{OrderManager, RefusalReason};
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_orderbook::venue::VenueState;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::autonomy::AutonomyController;
use qip_risk_engine::pretrade::PreTradeChecker;
use qip_routing::reprice::RepricePolicy;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

// =========================================================================
// Behavioural: the cell's own placer seam (`Cell::send`)
// =========================================================================

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const ENVELOPE_KEY: &[u8] = b"placement-paths-envelope-key";

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn firing_strategy() -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("10")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn grant() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("")?.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        qip_capital::DEFAULT_MAX_PARTICIPATION_BP,
        qip_capital::DEFAULT_MAX_LEVERAGE,
        dec!("1000000"),
    )?)?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), ENVELOPE_KEY, CELL, t(1))
}

fn share_policy(grants: Vec<String>) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: grants,
        },
        t(5),
    );
    VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, CELL, t(5))
}

/// A node assembled the way `main.rs` assembles it, under `limits`, with one
/// firing strategy resting at the mid.
fn node(limits: &CellRiskLimits) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble_with_risk(
        config,
        features,
        Arc::new(SystemClock),
        allocation,
        None,
        limits,
    )?;
    let gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    node.cell.deploy_with_pricing(
        compiled,
        program,
        grant()?,
        PricingPolicy::rest_at_mid(Duration::from_secs(60))?,
    )?;
    let named = grant()?.signature().to_string();
    node.cell.apply_policy(share_policy(vec![named])?, t(5))?;
    Ok((node, gateway, feed))
}

fn limits_with_order_notional(bound: Decimal) -> Result<CellRiskLimits> {
    let mut set = CellRiskLimits::shipped_set();
    set.name = "placement-paths".to_string();
    set = set.with(
        Limit::new(
            "order-notional",
            LimitKind::MaxOrderNotional { limit: bound },
        )
        .with_rationale("the bound this suite breaches on purpose"),
    );
    let text = serde_json::to_string(&set).expect("a limit set serialises");
    CellRiskLimits::from_document(&text, "placement-paths-limits.json")
}

fn seed_book(gateway: &mut SimulatedGateway) -> Result<()> {
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    Ok(())
}

fn risk_refusals(cell: &Cell) -> Vec<String> {
    cell.journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::Refused { gate, reason } if gate == GATE_RISK => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

/// Run the node's first pass under the shipped limits and return the order
/// quantity it sends, asserting the premise that the venue heard of it.
fn resting_quantity_under_the_shipped_limits() -> Result<Decimal> {
    let (mut node, mut gateway, mut feed) = node(&CellRiskLimits::shipped())?;
    seed_book(&mut gateway)?;
    let mut stats = PassStats::default();
    let outcome = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
    let PassOutcome::Ran { report, .. } = outcome else {
        panic!("premise: a running node reported its pass as halted");
    };
    assert_eq!(report.orders.len(), 1, "premise: one order is sent");
    assert_eq!(gateway.submitted_count(), 1, "premise: the venue heard it");
    assert!(report.orders[0].quantity.is_positive());
    Ok(report.orders[0].quantity)
}

/// The net path: a strategy's order through `Cell::work` into the venue the
/// binary holds. Mutation: delete the `self.risk_admit(None, ...)?` line in
/// `Cell::send` and the venue's count is one, not zero.
#[test]
fn the_net_path_hands_the_venue_nothing_when_the_cell_gate_refuses() -> Result<()> {
    let quantity = resting_quantity_under_the_shipped_limits()?;
    let (mut node, mut gateway, mut feed) =
        node(&limits_with_order_notional(quantity * dec!("50"))?)?;
    seed_book(&mut gateway)?;
    let mut stats = PassStats::default();
    let outcome = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    );
    assert_eq!(
        gateway.submitted_count(),
        0,
        "an order the cell's risk gate refuses reached the venue"
    );
    assert!(outcome.is_err(), "a refused order fails the pass");
    assert_eq!(risk_refusals(&node.cell).len(), 1);
    Ok(())
}

/// The repricer's replacement. Mutation: delete the `cell.admit_replacement(`
/// block from `Requoter::replace` and the venue receives the replacement.
#[test]
fn the_repricer_withdraws_but_places_nothing_when_the_replacement_breaches() -> Result<()> {
    let quantity = resting_quantity_under_the_shipped_limits()?;
    // Between the resting order (100 a unit) and its replacement (100.5).
    let (mut node, mut gateway, mut feed) =
        node(&limits_with_order_notional(quantity * dec!("100.25"))?)?;
    let mut requoter = Requoter::new(
        RepricePolicy::new(dec!("0.01"), 5, 50.0),
        CellMetrics::new(Arc::clone(node.scrape_registry()), CELL, REGION),
    )?;
    seed_book(&mut gateway)?;
    let mut stats = PassStats::default();
    let first = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(10),
    )?;
    let PassOutcome::Ran { report, .. } = first else {
        panic!("premise: a running node reported its pass as halted");
    };
    assert_eq!(report.orders.len(), 1, "premise: one order rests in bound");
    let resting = report.orders[0].clone();
    assert!(gateway.venue_holds_open(&resting.order_id), "premise");
    assert_eq!(gateway.submitted_count(), 1, "premise");

    gateway.seed_touch(&object(), Side::Buy, dec!("100.5"), dec!("1"), t(15))?;
    let second = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(20),
    );
    // The strategy's own order on this pass breaches at the new mid too and
    // fails the pass at `Cell::send`; whatever it did, no replacement went.
    let sent_by_work = match &second {
        Ok(PassOutcome::Ran {
            report, requotes, ..
        }) => {
            assert!(
                requotes
                    .iter()
                    .any(|requote| matches!(requote, Requote::RiskRefused { .. })),
                "the requote outcome does not name the risk gate: {requotes:?}"
            );
            report.orders.len() as u64
        }
        Ok(PassOutcome::Halted { .. }) => panic!("the node halted: {second:?}"),
        Err(_) => 0,
    };
    assert!(!gateway.venue_holds_open(&resting.order_id), "withdrawn");
    assert_eq!(
        gateway.submitted_count(),
        1 + sent_by_work,
        "the venue received a replacement the cell's risk gate refuses"
    );
    Ok(())
}

// --- the arbitrage desk's legs ---------------------------------------------

const ARB_VENUE: &str = "CX";
const DESK: &str = "arb-desk";

fn arb_venue() -> VenueId {
    VenueId::new(ARB_VENUE)
}

fn arb_object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

fn arb_book(market: &str, bid: (&str, &str), ask: (&str, &str)) -> Result<VenueState> {
    let mut state = VenueState::aggregated(arb_object(market), arb_venue(), VenueStatus::Open);
    for (index, (side, (price, size))) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            arb_object(market),
            Origin::new(arb_venue(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side,
                price: Decimal::parse(price).expect("a decimal literal"),
                quantity: Decimal::parse(size).expect("a decimal literal"),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    Ok(state)
}

fn arb_desk() -> Result<ArbitrageDesk> {
    let fee = dec!("0.0004");
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        arb_venue(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    let node = |name: &str| Node::new(arb_object(name), arb_venue());
    for (from, to, market, side) in [
        ("USDT", "ETH", "ETHUSDT", BookSide::Ask),
        ("ETH", "BTC", "ETHBTC", BookSide::Bid),
        ("BTC", "USDT", "BTCUSDT", BookSide::Bid),
    ] {
        graph.add_trade(
            node(from),
            node(to),
            Decimal::ONE,
            fee,
            arb_object(market),
            side,
            t(0),
            0,
        )?;
    }
    let sizes = SizePolicy::uniform(dec!("10000"))
        .with(arb_object("ETH"), dec!("3.3"))
        .with(arb_object("BTC"), dec!("0.16"));
    let scanner = OpportunityScanner::new(
        SearchSettings::default(),
        EdgeAssumptions::default(),
        PlanSettings::with_budget(dec!("50000")),
    );
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(DESK),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![arb_venue()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    let envelope = VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))?;
    ArbitrageDesk::new(
        StrategyId::new(DESK),
        scanner,
        graph,
        sizes,
        envelope,
        4,
        Duration::from_secs(30),
    )
}

/// A cell holding the ethereum triangle's books and the desk, behind a gate
/// whose per-order notional ceiling is `ceiling`.
fn arbitrage_cell(ceiling: Decimal) -> Result<Cell> {
    let gate = CellRiskGate::new(
        LimitSet::new("cell").with(Limit::new(
            "order-notional",
            LimitKind::MaxOrderNotional { limit: ceiling },
        )),
        dec!("1000000000"),
    )?;
    let config = CellConfig::new(CELL, REGION).with_venue(arb_venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?
        .with_risk_gate(gate)
        .with_arbitrage(arb_desk()?)?;
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.belief_priors = Slot::produced(
        qip_contracts::policy::BeliefPriors {
            priors: BTreeMap::new(),
        },
        t(5),
    );
    payload.causal_digest = Slot::produced(
        qip_contracts::policy::CausalDigest {
            active_edges: Vec::new(),
        },
        t(5),
    );
    payload.episodic_digest = Slot::produced(
        qip_contracts::policy::EpisodicDigest {
            digest: "d".to_string(),
            episodes: 0,
        },
        t(5),
    );
    cell.apply_policy(
        VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, CELL, t(5))?,
        t(5),
    )?;
    for state in [
        arb_book("ETHUSDT", ("3000", "200"), ("3000.1", "200"))?,
        arb_book("ETHBTC", ("0.0505", "200"), ("0.05051", "200"))?,
        arb_book("BTCUSDT", ("60000", "10"), ("60001", "10"))?,
    ] {
        cell.track(state);
    }
    Ok(cell)
}

/// A placer that counts every order it is handed, through either method.
#[derive(Debug, Default)]
struct CountingPlacer {
    placed: usize,
}

impl Placer for CountingPlacer {
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
        self.placed += 1;
        Ok(())
    }
}

/// The desk's legs leave through `Cell::send` like every other order.
/// Mutation: delete the `self.risk_admit(None, ...)?` line in `Cell::send`
/// and the placer is handed all three legs.
#[test]
fn an_arbitrage_leg_the_cell_gate_refuses_never_reaches_the_placer() -> Result<()> {
    // Premise: under a generous ceiling the cycle's three legs all go out.
    let mut open = arbitrage_cell(dec!("1000000000"))?;
    let mut open_placer = CountingPlacer::default();
    open.work(t(10), &mut open_placer)?;
    assert_eq!(
        open_placer.placed, 3,
        "premise: the desk sends its three legs"
    );
    assert!(risk_refusals(&open).is_empty(), "premise");

    // Each leg is about ten thousand of notional; a ceiling of 100 refuses the
    // first, and a cycle short a leg is broken, not shrunk.
    let mut tight = arbitrage_cell(dec!("100"))?;
    let mut tight_placer = CountingPlacer::default();
    let _ = tight.work(t(10), &mut tight_placer);
    assert_eq!(
        tight_placer.placed, 0,
        "a leg the risk gate refuses reached the placer"
    );
    assert!(
        !risk_refusals(&tight).is_empty(),
        "no refusal was journaled under {GATE_RISK}"
    );
    Ok(())
}

// =========================================================================
// Behavioural: the desk's order manager and the platform that holds it
// =========================================================================

fn desk_order(quantity: &str, id: &str) -> Order {
    Order::new(
        OrderId::from_string(id),
        object(),
        Side::Buy,
        Decimal::parse(quantity).expect("a decimal literal"),
        OrderType::Market,
        dec!("100"),
        "prop-1",
        vec!["hyp-1".to_string()],
        "momentum",
        t(0),
    )
}

/// `OrderManager::submit`: a limit breach and a tripped kill switch each stop
/// the order before the broker. Mutation: skip `.check_with_working(` in
/// `OrderManager::submit`, and the breach reaches the broker (count two).
#[test]
fn the_order_manager_never_calls_the_broker_for_a_breach_or_a_halt() {
    let limits = LimitSet::new("desk").with(
        Limit::new(
            "order-notional",
            LimitKind::MaxOrderNotional {
                limit: dec!("1000"),
            },
        )
        .with_rationale("the bound this suite breaches on purpose"),
    );
    let mut manager = OrderManager::new(PreTradeChecker::new(limits));
    let mut broker = SimulatedBroker::new(
        SimulationSettings::default()
            .with_rejection_probability(0.0)
            .and_then(|s| s.with_immediate_fill_fraction(Decimal::ONE))
            .expect("0.0 and 1.0 are inside the permitted ranges"),
        3,
    );
    let mut autonomy = AutonomyController::new();
    let state = RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        ..RiskState::default()
    };
    let inside = manager.submit(
        desk_order("5", "ord-inside"),
        &mut broker,
        &autonomy,
        &state,
        BTreeMap::new(),
        None,
        t(0),
    );
    assert!(inside.accepted, "premise: {:?}", inside.refusal);
    assert_eq!(
        broker.submitted_count(),
        1,
        "premise: the broker was called"
    );

    let breach = manager.submit(
        desk_order("50", "ord-breach"),
        &mut broker,
        &autonomy,
        &state,
        BTreeMap::new(),
        None,
        t(0),
    );
    assert!(matches!(
        breach.refusal,
        Some(RefusalReason::RiskRejected { .. })
    ));
    assert_eq!(
        broker.submitted_count(),
        1,
        "a breaching desk order reached the broker"
    );

    autonomy
        .kill_switch_mut()
        .trip_global(t(0), "acceptance", "an operator halted the desk");
    let halted = manager.submit(
        desk_order("5", "ord-halted"),
        &mut broker,
        &autonomy,
        &state,
        BTreeMap::new(),
        None,
        t(0),
    );
    assert!(!halted.accepted);
    assert_eq!(
        broker.submitted_count(),
        1,
        "an order submitted under a tripped kill switch reached the broker"
    );
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(t(0)));
    let context = Context::new(clock, config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .expect("the platform assembles")
}

fn platform_order(platform: &mut Platform, quantity: &str, id: &str) -> Order {
    let mut order = platform.order_from(
        object(),
        Side::Buy,
        Decimal::parse(quantity).expect("a decimal literal"),
        dec!("100"),
        "prop-paths",
        vec!["hyp-paths".to_string()],
        t(0),
    );
    order.order_id = OrderId::from_string(id);
    order
}

/// `Platform::submit_order`, the kernel's door to the broker it owns (ACT
/// stage, qip-api's order route and the CLI all come through it). The broker
/// is private to the platform, so the sink is read through the order manager:
/// an order the broker was handed is recorded as open or filled; a refusal is
/// recorded as a refusal and nothing else.
///
/// Mutation: make `OrderManager::submit` skip `.check_with_working(` and the
/// oversized order is accepted, so the `is_err` and the open-or-filled
/// assertions both fail.
#[test]
fn the_platform_door_to_its_broker_refuses_an_order_past_the_notional_cap() -> Result<()> {
    let mut platform = platform();
    // Premise: a small order is accepted and the order manager records it
    // having gone to the broker.
    let small = platform_order(&mut platform, "10", "ord-small");
    platform.submit_order(small, t(0))?;
    let recorded: Vec<&Order> = platform.orders().orders().collect();
    assert_eq!(recorded.len(), 1, "premise: the small order was recorded");
    let sent_before = platform.orders().fills().len() + platform.orders().open_orders().len();
    assert!(sent_before > 0, "premise: the broker was handed the order");

    // 3,000 at 100 is 300,000 against the shipped 250,000 order cap.
    let big = platform_order(&mut platform, "3000", "ord-big");
    let refusals_before = platform.orders().refusals().len();
    let error = platform.submit_order(big, t(0));
    assert!(error.is_err(), "an order past the cap was accepted");
    assert_eq!(
        platform.orders().refusals().len(),
        refusals_before + 1,
        "the refusal was not recorded"
    );
    assert!(matches!(
        platform
            .orders()
            .refusals()
            .last()
            .and_then(|r| r.refusal.as_ref()),
        Some(RefusalReason::RiskRejected { .. })
    ));
    assert_eq!(
        platform.orders().fills().len() + platform.orders().open_orders().len(),
        sent_before,
        "a refused order was recorded as sent to the broker"
    );
    Ok(())
}

// --- the market factory's ladder ---------------------------------------------

/// A ladder is orders at a venue; `MarketFactory::seed` rests one only on a
/// `LadderAdmission`, whose one mint is `OrderManager::admit_ladder`.
/// Mutation: make `admit_ladder` skip its check loop and a breaching ladder
/// is admitted, failing the `expect_err`.
#[test]
fn a_ladder_past_the_desks_limits_cannot_be_admitted_so_cannot_be_seeded() -> Result<()> {
    const PRODUCT: &str = "TEST/USD";
    let rule = PricingRule::new(
        dec!("100"),
        dec!("0.5"),
        dec!("0.25"),
        dec!("10"),
        3,
        dec!("0.05"),
    )?;
    let mut permissions = CreationPermissions::new();
    for action in CreationAction::ALL {
        for authority in CreationAuthority::ALL {
            permissions.record(
                authority,
                &CreationRequest::new(action, "simulated-venue", PRODUCT, "GB"),
                "recorded evidence",
            )?;
        }
    }
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 1);
    let mut factory = MarketFactory::for_venue(&broker)?;
    factory.create(
        &permissions.admit(&CreationRequest::new(
            CreationAction::Create,
            "simulated-venue",
            PRODUCT,
            "GB",
        ))?,
        rule.clone(),
        t(0),
    )?;
    let venue = broker.listing_venue().expect("a listing venue");
    factory.list(
        &permissions.admit(&CreationRequest::new(
            CreationAction::List,
            "simulated-venue",
            PRODUCT,
            "GB",
        ))?,
        venue,
        t(0),
    )?;
    assert!(
        broker.listed_book(PRODUCT).is_some(),
        "premise: the empty book is listed"
    );

    let mut orders = OrderManager::new(PreTradeChecker::new(LimitSet::new("governance").with(
        Limit::new("leverage", LimitKind::MaxLeverage { limit: 2.0 }),
    )));
    let state = |equity: &str| RiskState {
        equity: Decimal::parse(equity).expect("a decimal literal"),
        cash: Decimal::parse(equity).expect("a decimal literal"),
        ..RiskState::default()
    };
    // Premise: the same ladder over a million of equity is admitted.
    orders.admit_ladder(
        PRODUCT,
        "simulated-venue",
        &rule,
        "market-factory",
        &AutonomyController::new(),
        &state("1000000"),
        t(0),
    )?;
    // Over a hundred of equity it is some twenty times levered and is not.
    let refused = orders
        .admit_ladder(
            PRODUCT,
            "simulated-venue",
            &rule,
            "market-factory",
            &AutonomyController::new(),
            &state("100"),
            t(0),
        )
        .expect_err("a ladder far past the leverage limit was admitted");
    assert!(
        refused.message().contains("nothing was rested"),
        "{}",
        refused.message()
    );
    assert_eq!(
        broker
            .listed_book(PRODUCT)
            .map(qip_market::book::OrderBook::total_depth),
        Some(Decimal::ZERO),
        "the venue's book holds depth nobody admitted"
    );
    Ok(())
}

// =========================================================================
// Structural: every non-test call site of an order-sending method
// =========================================================================

/// What a call site is, and so why it may exist.
#[derive(Clone, Copy, Debug)]
enum Class {
    /// An order reaches a sink here and `gate` stands before it in the same
    /// function (checked in the source); `proof` names the behavioural test
    /// above that drives the route with an order the gate refuses.
    Gated {
        gate: &'static str,
        proof: &'static str,
    },
    /// A forwarding layer that adds no route: it calls on behalf of a caller
    /// that is itself a row (or a gated seam), and `to` says which.
    Forwards { to: &'static str },
    /// Not an order at all (a name collision), and why.
    NotAnOrder { why: &'static str },
}

const NET: &str = "the_net_path_hands_the_venue_nothing_when_the_cell_gate_refuses";
const ARB: &str = "an_arbitrage_leg_the_cell_gate_refuses_never_reaches_the_placer";
const REPRICE: &str = "the_repricer_withdraws_but_places_nothing_when_the_replacement_breaches";
const DESK_TEST: &str = "the_order_manager_never_calls_the_broker_for_a_breach_or_a_halt";
const LADDER: &str = "a_ladder_past_the_desks_limits_cannot_be_admitted_so_cannot_be_seeded";
const SEAM: &str = "Cell::send / Cell::admit_replacement (the cell's Risk Gate)";

/// `(file under backend/crates, token, enclosing fn, exact non-test count, class)`.
///
/// The count is exact on purpose: a second occurrence in a file already
/// listed is a new route until somebody reads it, and a smaller count means
/// a row describes code that is gone.
const SITES: &[(&str, &str, &str, usize, Class)] = &[
    // --- the cell's placer seam ---------------------------------------------
    (
        "edge/qip-edge/src/cell.rs",
        ".try_place(",
        "send",
        1,
        Class::Gated {
            gate: "self.risk_admit(",
            proof: NET,
        },
    ),
    (
        "edge/qip-edge/src/cell.rs",
        ".place(",
        "try_place",
        1,
        Class::Forwards {
            to: "the default `Placer::try_place`, called only from Cell::send",
        },
    ),
    // --- the node's venue sinks: reached through the cell's seam only ---------
    (
        "apps/qip-edge-node/src/gateway.rs",
        ".place(",
        "place",
        2,
        Class::Forwards { to: SEAM },
    ),
    (
        "apps/qip-edge-node/src/gateway.rs",
        ".try_place(",
        "try_place",
        2,
        Class::Forwards { to: SEAM },
    ),
    (
        "apps/qip-edge-node/src/gateway.rs",
        ".submit(",
        "submit_now",
        1,
        Class::Forwards { to: SEAM },
    ),
    (
        "apps/qip-edge-node/src/gateway.rs",
        ".submit(",
        "place",
        1,
        Class::Forwards { to: SEAM },
    ),
    (
        "apps/qip-edge-node/src/reprice.rs",
        ".place(",
        "replace",
        1,
        Class::Gated {
            gate: "cell.admit_replacement(",
            proof: REPRICE,
        },
    ),
    (
        "apps/qip-edge-node/src/reprice.rs",
        ".place(",
        "place",
        1,
        Class::Forwards { to: SEAM },
    ),
    (
        "apps/qip-edge-node/src/reprice.rs",
        ".try_place(",
        "try_place",
        1,
        Class::Forwards { to: SEAM },
    ),
    // --- the desk's order manager and the doors to it ------------------------
    (
        "services/qip-execution-engine/src/oms.rs",
        ".submit(",
        "submit",
        1,
        Class::Gated {
            gate: ".check_with_working(",
            proof: DESK_TEST,
        },
    ),
    (
        "runtime/qip-kernel/src/platform.rs",
        ".submit(",
        "submit_order",
        1,
        Class::Forwards {
            to: "OrderManager::submit",
        },
    ),
    (
        "apps/qip-cli/src/demo/mod.rs",
        ".submit(",
        "cycle",
        1,
        Class::Forwards {
            to: "OrderManager::submit",
        },
    ),
    // `Platform::submit_order` reached from the ACT stage.
    (
        "runtime/qip-kernel/src/platform.rs",
        ".submit_order(",
        "stage_act_orders",
        1,
        Class::Forwards {
            to: "OrderManager::submit via Platform::submit_order",
        },
    ),
    // The venue adapters' `Broker::submit` bodies. `Broker::submit` has one
    // non-test caller outside the node's gateway (oms.rs above).
    (
        "services/qip-brokers/src/adapter.rs",
        ".submit_order(",
        "submit_declared",
        1,
        Class::Forwards {
            to: "Broker::submit, called by OrderManager::submit",
        },
    ),
    (
        "services/qip-brokers/src/exchange.rs",
        ".submit_order(",
        "submit",
        1,
        Class::Forwards {
            to: "Broker::submit, called by OrderManager::submit",
        },
    ),
    (
        "services/qip-brokers/src/rest.rs",
        ".submit_order(",
        "submit",
        1,
        Class::Forwards {
            to: "Broker::submit, called by OrderManager::submit",
        },
    ),
    // --- the market factory's ladders ----------------------------------------
    (
        "services/qip-execution-engine/src/market_creation.rs",
        ".list_book(",
        "seed",
        1,
        Class::Gated {
            gate: "ladder: &LadderAdmission",
            proof: LADDER,
        },
    ),
    (
        "services/qip-execution-engine/src/market_creation.rs",
        ".list_book(",
        "list",
        1,
        Class::NotAnOrder {
            why: "lists an EMPTY OrderBook::new(...): nothing rests",
        },
    ),
    // --- name collisions ------------------------------------------------------
    (
        "services/qip-agency/src/ambient_intake.rs",
        ".submit(",
        "admit",
        1,
        Class::NotAnOrder {
            why: "AmbientIntake::submit is the ambient door: it takes a Promoted \
                  opportunity and, through the action guard, appends a draft to an \
                  internal outbox (outbox.rs, the only Adapter). No venue, broker or \
                  order is reachable from it (ADR 0114 D8)",
        },
    ),
    (
        "libs/qip-financial/src/commerce.rs",
        ".place(",
        "purchase",
        1,
        Class::NotAnOrder {
            why: "a simulated merchant's purchase record, not a venue order",
        },
    ),
    (
        "services/qip-capital/src/bank.rs",
        ".place(",
        "route_idle_cash",
        1,
        Class::NotAnOrder {
            why: "places a maturity rung on a cash ladder",
        },
    ),
    (
        "services/qip-market-ingestion/src/tape.rs",
        ".place(",
        "*",
        4,
        Class::NotAnOrder {
            why: "places a reading on a bitemporal timeline",
        },
    ),
    (
        "runtime/qip-kernel/src/model_tournament.rs",
        ".submit(",
        "*",
        1,
        Class::NotAnOrder {
            why: "submits a forecast to a scoring tournament",
        },
    ),
    (
        "services/qip-optimization-engine/src/router.rs",
        ".submit(",
        "*",
        1,
        Class::NotAnOrder {
            why: "submits a quantum optimisation job to the provider gateway",
        },
    ),
    (
        "services/qip-training/src/job.rs",
        ".submit(",
        "*",
        1,
        Class::NotAnOrder {
            why: "submits a training job to a compute provider",
        },
    ),
];

const TOKENS: &[&str] = &[
    ".try_place(",
    ".place(",
    ".submit(",
    ".submit_order(",
    ".list_book(",
];

/// One non-test occurrence: where, and the text of its function so far.
#[derive(Debug)]
struct Occurrence {
    path: String,
    token: &'static str,
    line: usize,
    function: String,
    preamble: String,
}

fn crates_root() -> PathBuf {
    qip_acceptance::repository_root().join("backend/crates")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every production source file: `src/` of every crate under the production
/// groups, never a crate's `tests/`, `benches/` or `examples/`.
fn production_files() -> Vec<PathBuf> {
    let root = crates_root();
    let mut files = Vec::new();
    for group in [
        "libs", "services", "runtime", "apps", "edge", "agents", "quant",
    ] {
        let Ok(members) = std::fs::read_dir(root.join(group)) else {
            continue;
        };
        for member in members.flatten() {
            rust_files(&member.path().join("src"), &mut files);
        }
    }
    files.sort();
    files
}

/// `(line number, code)` with every `#[cfg(test)]` item and every comment
/// removed. A `#[cfg(test)] mod` is skipped to its matching brace rather than
/// to end of file: `platform.rs` opens its first test module a quarter of the
/// way down, and a cut at the first marker would hide production code that
/// follows it. (Braces inside string literals are not special-cased; a test
/// module whose strings unbalance them would make the scan fail loudly on a
/// count, not pass quietly.)
fn production_lines(source: &str) -> Vec<(usize, String)> {
    let mut kept = Vec::new();
    let mut depth_to_close: Option<i64> = None;
    let mut pending_skip = false;
    for (index, raw) in source.lines().enumerate() {
        let code = raw.split("//").next().unwrap_or("").to_string();
        let trimmed = code.trim();
        if let Some(depth) = depth_to_close.as_mut() {
            *depth += code.matches('{').count() as i64 - code.matches('}').count() as i64;
            if *depth <= 0 {
                depth_to_close = None;
            }
            continue;
        }
        if trimmed.starts_with("#[cfg(test)]") {
            pending_skip = true;
            continue;
        }
        if pending_skip {
            if trimmed.starts_with("#[") || trimmed.is_empty() {
                continue;
            }
            pending_skip = false;
            let opened = code.matches('{').count() as i64;
            let closed = code.matches('}').count() as i64;
            if opened > closed {
                depth_to_close = Some(opened - closed);
            }
            continue;
        }
        kept.push((index + 1, code));
    }
    kept
}

fn declared_fn(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = ["pub(crate) fn ", "pub fn ", "fn "]
        .iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix))?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn relative(file: &Path) -> String {
    file.strip_prefix(crates_root())
        .expect("under backend/crates")
        .to_string_lossy()
        .replace('\\', "/")
}

fn occurrences() -> Vec<Occurrence> {
    let mut found = Vec::new();
    for file in production_files() {
        let text = std::fs::read_to_string(&file).expect("a source file is readable");
        let lines = production_lines(&text);
        for (position, (line, code)) in lines.iter().enumerate() {
            for token in TOKENS {
                for _ in 0..code.matches(token).count() {
                    let start = (0..=position)
                        .rev()
                        .find(|i| declared_fn(&lines[*i].1).is_some())
                        .unwrap_or(0);
                    found.push(Occurrence {
                        path: relative(&file),
                        token,
                        line: *line,
                        function: declared_fn(&lines[start].1).unwrap_or_else(|| "?".to_string()),
                        preamble: lines[start..=position]
                            .iter()
                            .map(|(_, c)| c.as_str())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    });
                }
            }
        }
    }
    found
}

fn row_for<'a>(
    site: &Occurrence,
) -> Option<&'a (&'static str, &'static str, &'static str, usize, Class)> {
    SITES.iter().find(|(path, token, function, _, _)| {
        *path == site.path
            && *token == site.token
            && (*function == site.function || *function == "*")
    })
}

/// A new call site, or a second one in a function already listed, fails here
/// until somebody classifies it; and a row whose code is gone fails too.
#[test]
fn every_non_test_call_site_of_an_order_sending_method_is_classified_exactly() {
    let found = occurrences();
    // Premise: the scan sees the seams this file exists for.
    for (path, function) in [
        ("edge/qip-edge/src/cell.rs", "send"),
        ("services/qip-execution-engine/src/oms.rs", "submit"),
        ("apps/qip-edge-node/src/reprice.rs", "replace"),
    ] {
        assert!(
            found
                .iter()
                .any(|site| site.path == path && site.function == function),
            "premise: the scan did not find {path}::{function}"
        );
    }
    let unclassified: Vec<String> = found
        .iter()
        .filter(|site| row_for(site).is_none())
        .map(|site| {
            format!(
                "{}:{} `{}` in fn {}",
                site.path, site.line, site.token, site.function
            )
        })
        .collect();
    assert!(
        unclassified.is_empty(),
        "an order-sending call appears at a site no row classifies; route it through \
         `Cell::send`, `Cell::admit_replacement` or `OrderManager::submit`, or add a row naming \
         its gate and a behavioural test above: {unclassified:?}"
    );
    for (path, token, function, count, _) in SITES {
        let actual = found
            .iter()
            .filter(|site| {
                site.path == *path
                    && site.token == *token
                    && (*function == "*" || site.function == *function)
            })
            .count();
        assert_eq!(
            actual, *count,
            "{path} `{token}` in fn {function}: the row says {count}, the source has {actual}"
        );
    }
}

/// A `Gated` row's gate stands in the same function before the call, and its
/// proof names a test that exists in this file.
#[test]
fn every_gated_site_runs_its_gate_before_the_call_and_names_a_test_that_drives_it() {
    let found = occurrences();
    let source = std::fs::read_to_string(
        qip_acceptance::repository_root()
            .join("backend/crates/tests/qip-acceptance/tests/order_placement_paths.rs"),
    )
    .expect("this file is readable");
    let mut gated = 0;
    for site in &found {
        let Some((_, _, _, _, Class::Gated { gate, proof })) = row_for(site) else {
            continue;
        };
        gated += 1;
        assert!(
            site.preamble.contains(gate),
            "{}:{} hands an order onward in fn {} without `{gate}` before it",
            site.path,
            site.line,
            site.function
        );
        assert!(
            source.contains(&format!("fn {proof}(")),
            "the proof `{proof}` for {}:{} is not a test in this file",
            site.path,
            site.line
        );
    }
    assert!(
        gated >= 4,
        "premise: four gated seams were examined, {gated} were"
    );
    // The arbitrage legs share `Cell::send`'s row; their driver must exist too.
    assert!(
        source.contains(&format!("fn {ARB}(")),
        "the arbitrage-leg test named by ARB is not in this file"
    );
    // Forwarding and name-collision rows must say why.
    for (path, token, _, _, class) in SITES {
        match class {
            Class::Forwards { to } => assert!(!to.is_empty(), "{path} {token}"),
            Class::NotAnOrder { why } => assert!(!why.is_empty(), "{path} {token}"),
            Class::Gated { .. } => {}
        }
    }
}

/// The venue sinks themselves. A new `impl Placer`, `impl Broker` or
/// `impl ListingVenue` in production code is a new place an order can end up;
/// it must be added here so somebody asks which seam reaches it.
#[test]
fn the_set_of_production_placers_and_brokers_is_exactly_the_reviewed_one() {
    let mut found: BTreeSet<(String, String)> = BTreeSet::new();
    for file in production_files() {
        let text = std::fs::read_to_string(&file).expect("a source file is readable");
        for (_, code) in production_lines(&text) {
            for marker in [
                "impl Placer for ",
                "impl Broker for ",
                "impl ListingVenue for ",
            ] {
                if let Some((_, after)) = code.split_once(marker) {
                    let name: String = after
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    found.insert((relative(&file), format!("{marker}{name}")));
                }
            }
        }
    }
    let reviewed: BTreeSet<(String, String)> = [
        // The node's sinks, reached through `Cell::send` / `admit_replacement`.
        (
            "apps/qip-edge-node/src/gateway.rs",
            "impl Placer for SimulatedGateway",
        ),
        (
            "apps/qip-edge-node/src/gateway.rs",
            "impl Placer for RestGateway",
        ),
        (
            "apps/qip-edge-node/src/gateway.rs",
            "impl Placer for NodeGateway",
        ),
        (
            "apps/qip-edge-node/src/reprice.rs",
            "impl Placer for RequotingPlacer",
        ),
        // The desk's brokers, reached through `OrderManager::submit`.
        (
            "services/qip-execution-engine/src/broker.rs",
            "impl Broker for SimulatedBroker",
        ),
        (
            "services/qip-execution-engine/src/broker.rs",
            "impl Broker for LiveBroker",
        ),
        // Rests a book in the simulator's own map; `MarketFactory::seed` is
        // the caller and takes a `LadderAdmission`.
        (
            "services/qip-execution-engine/src/broker.rs",
            "impl ListingVenue for SimulatedBroker",
        ),
        (
            "services/qip-brokers/src/exchange.rs",
            "impl Broker for SimulatedExchange",
        ),
        (
            "services/qip-brokers/src/rest.rs",
            "impl Broker for RestOrderEntryAdapter",
        ),
    ]
    .into_iter()
    .map(|(p, i)| (p.to_string(), i.to_string()))
    .collect();
    assert!(!found.is_empty(), "premise: the scan found implementors");
    let new: Vec<_> = found.difference(&reviewed).collect();
    let gone: Vec<_> = reviewed.difference(&found).collect();
    assert!(
        new.is_empty(),
        "a production type now receives orders and no row says which gated seam reaches it: {new:?}"
    );
    assert!(
        gone.is_empty(),
        "reviewed implementors no longer exist: {gone:?}"
    );
}

/// `Cell::risk_admit` admits when no gate is installed, so a cell is gated
/// only if its constructor installed one. The only production constructors
/// are the node's (gated) and the CLI demonstration's (which never runs a
/// pass: `Cell::work` has one production caller, in the node).
///
/// Mutation: delete `.with_risk_gate(risk_gate)` from `qip-edge-node`'s
/// `assemble_with_risk`, or add a second `Cell::new(` anywhere in production.
#[test]
fn every_production_cell_is_built_gated_or_never_run() {
    let mut constructors: Vec<(String, usize, String)> = Vec::new();
    let mut runs: Vec<String> = Vec::new();
    for file in production_files() {
        let path = relative(&file);
        if path == "edge/qip-edge/src/cell.rs" {
            continue; // the type's own module; its constructor is the definition
        }
        let text = std::fs::read_to_string(&file).expect("a source file is readable");
        let lines = production_lines(&text);
        for (position, (line, code)) in lines.iter().enumerate() {
            if code.contains("Cell::new(")
                && !code.contains("std::cell::Cell::new(")
                && !code.contains("fn new(")
            {
                let window: String = lines[position..lines.len().min(position + 6)]
                    .iter()
                    .map(|(_, c)| c.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                constructors.push((path.clone(), *line, window));
            }
            if code.contains("cell.work(") {
                runs.push(format!("{path}:{line}"));
            }
        }
    }
    assert_eq!(
        runs.len(),
        1,
        "`Cell::work` has more than one caller: {runs:?}"
    );
    assert!(
        runs[0].starts_with("apps/qip-edge-node/src/pass.rs:"),
        "{runs:?}"
    );
    let paths: BTreeSet<&str> = constructors.iter().map(|(p, _, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        BTreeSet::from([
            "apps/qip-cli/src/demo/mod.rs",
            "apps/qip-edge-node/src/lib.rs"
        ]),
        "a new production file builds a Cell: {constructors:?}"
    );
    for (path, line, window) in &constructors {
        if path == "apps/qip-edge-node/src/lib.rs" {
            assert!(
                window.contains(".with_risk_gate("),
                "{path}:{line} builds the node's cell without the risk gate:\n{window}"
            );
        }
    }
}
