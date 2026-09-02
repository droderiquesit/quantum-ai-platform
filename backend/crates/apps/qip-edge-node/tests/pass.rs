//! The pass loop, proven on the node's own seam.
//!
//! `qip-edge`'s telemetry suite proves that a cell given a registry records
//! each pass-time fact. What it cannot see is whether the *node* ever
//! reaches `Cell::work` — and until this suite existed it did not, so every
//! pass-time series was recorded by code nothing in production ran. Each
//! test here drives the assembled node's cell through `run_pass` against
//! the simulated gateway and feed the binary holds, and asserts on the
//! registry the scrape serves rather than on the report, because the
//! series is what a deployed process shows and the report is what a test
//! can see.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, SystemClock, dec};
use qip_edge::cell::{CellConfig, PolledHalt, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::telemetry::EDGE_FILLS_CONFIRMED;
use qip_edge_node::feed::{FEED_VARIABLE, FeedChoice, SimulatedFeed};
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Labels, labels, names};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const ENVELOPE_KEY: &[u8] = b"pass-test-envelope-key";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn by(key: &str, value: &str) -> Labels {
    labels([("cell", CELL), ("region", REGION), (key, value)])
}

fn base() -> Labels {
    labels([("cell", CELL), ("region", REGION)])
}

/// A strategy whose one rule always holds, so a pass raises exactly one
/// signal; what is under test is the node's loop, not the strategy.
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
    let unsigned = build("unsigned")?;
    let signed = build(&sign_payload(ENVELOPE_KEY, &unsigned.signing_payload()))?;
    VerifiedEnvelope::verify(signed, ENVELOPE_KEY, CELL, t(1))
}

/// The node's pieces, assembled the way `main.rs` assembles them: one
/// registry, the simulated gateway, the simulated feed attached to the cell,
/// and one firing strategy deployed under a signed grant with the pricing
/// the test names.
fn node_with_feed(
    pricing: PricingPolicy,
) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut node = assemble(config, features, Arc::new(SystemClock))?;
    let gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    node.cell
        .deploy_with_pricing(compiled, program, grant()?, pricing)?;
    Ok((node, gateway, feed))
}

#[test]
fn a_node_with_the_simulated_feed_runs_a_pass_and_the_pass_time_series_move() -> Result<()> {
    // Rest-at-mid, so the order the pass sends rests against the two-sided
    // book: the resting half of the proof. The marketable half is below.
    let (mut node, mut gateway, mut feed) =
        node_with_feed(PricingPolicy::rest_at_mid(Duration::from_secs(60))?)?;
    // Depth resting at the venue, on both sides, so the venue's own book is
    // what the cell will price off: a mid of 100 between 99 and 101.
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;

    let before = node.scrape_registry().snapshot();
    assert_eq!(
        before.counter(names::EDGE_WORK_PASSES, &base()),
        0,
        "the premise is a node that has not yet run a pass"
    );
    assert!(!node.cell.is_halted(), "the premise is a running cell");

    let mut stats = PassStats::default();
    let outcome = run_pass(&mut node.cell, &mut gateway, &mut feed, &mut stats, t(10))?;
    let PassOutcome::Ran {
        feed: tick,
        report,
        breaks,
    } = outcome
    else {
        panic!("a running node reported its pass as halted: {outcome:?}");
    };

    // The feed reached the cell's book: what the cell holds for the
    // instrument is the venue's depth, sequenced and applied through
    // `on_bytes`, not a price the test wrote.
    assert_eq!(tick.instruments, 1);
    assert_eq!(tick.messages, 2, "two levels rested and two were published");
    let mid = node
        .cell
        .liquidity()
        .get(&venue(), &object())
        .and_then(qip_orderbook::venue::VenueState::mid);
    assert_eq!(
        mid,
        Some(Decimal::from_int(100)),
        "the cell's book does not carry the simulator's depth"
    );

    // The pass ran and acted: the strategy fired, an order reached the venue.
    assert!(
        report.refusals.is_empty(),
        "a fully-fed pass refused: {:?}",
        report.refusals
    );
    assert_eq!(report.orders.len(), 1, "the firing strategy sent no order");
    assert_eq!(gateway.submitted_count(), 1, "the venue saw no order");
    assert_eq!(stats.passes, 1);
    assert_eq!(stats.orders, 1);

    // The order rests at the mid, so against the venue's own two-sided book
    // it sits between the two seeded levels. A resting order is not a fill: the cell holds it open
    // at its full size, books no position, confirms nothing, and the
    // reconciler — comparing confirmed fills with the venue's account, both
    // empty — finds no disagreement. Until the cell stopped recording a fill
    // on acceptance this exact pass halted the node, which is what a
    // deployed node did on the first strategy that fired.
    assert_eq!(
        gateway.resting_count(),
        3,
        "the premise is an order resting beside the two seeded levels"
    );
    assert_eq!(
        node.cell.position(&venue(), &object()),
        Decimal::ZERO,
        "a resting order was booked as a position"
    );
    assert!(
        node.cell.fills().is_empty(),
        "a resting order was confirmed as a fill: {:?}",
        node.cell.fills()
    );
    let open = node.cell.open_orders();
    assert_eq!(open.len(), 1, "the resting order is not held open");
    assert_eq!(open[0].filled, Decimal::ZERO);
    assert!(
        breaks.is_empty(),
        "a resting order the venue has not filled reconciled as a break: {breaks:?}"
    );
    assert!(!node.cell.is_halted(), "a resting order halted the cell");
    assert_eq!(stats.breaks, 0);
    assert_eq!(
        stats.fills, 0,
        "a fill was counted before the venue reported one"
    );

    // And the series a scrape serves moved — the whole reason the node has
    // a pass loop. Both the pass counter and a pass-time fact underneath it.
    let after = node.scrape_registry().snapshot();
    assert_eq!(
        after.counter(names::EDGE_WORK_PASSES, &base()),
        1,
        "the pass counter did not move on the registry the scrape serves"
    );
    assert_eq!(
        after.counter(names::EDGE_ORDERS_PLACED, &by("venue", VENUE)),
        1,
        "the order the venue accepted was not counted"
    );
    assert_eq!(
        after.counter(names::EDGE_SIGNALS_RAISED, &by("kind", "enter")),
        1,
        "the signal the strategy raised was not counted"
    );
    assert_eq!(
        after.counter(names::EDGE_RECONCILIATION_BREAKS, &base()),
        0,
        "a resting order was counted as a break"
    );
    assert_eq!(
        after.counter(EDGE_FILLS_CONFIRMED, &by("venue", VENUE)),
        0,
        "a fill was charted before the venue reported one"
    );
    assert_eq!(
        after.gauge(names::EDGE_HALTED, &by("source", "kill_switch")),
        Some(0.0),
        "the kill switch charts as tripped on a pass that broke nothing"
    );
    Ok(())
}

/// The venue's own position in the fixture instrument, from its ledger.
fn venue_position(gateway: &SimulatedGateway) -> Decimal {
    gateway
        .positions()
        .into_iter()
        .find(|position| position.object_id == object())
        .map_or(Decimal::ZERO, |position| position.quantity)
}

#[test]
fn a_resting_order_the_venue_fills_on_a_later_pass_is_confirmed_and_the_node_keeps_trading()
-> Result<()> {
    // The node's whole reason to exist: pass after pass against a venue,
    // with what it holds agreeing with what the venue holds. A resting
    // order from one pass, filled by somebody else's flow in between, must
    // be confirmed on the next pass through the order-entry channel, match
    // the venue's clearing account through the drop copy, and leave the
    // node running. Until fills were venue facts this node halted on the
    // pass that sent the order, and no later pass ever ran.
    let (mut node, mut gateway, mut feed) =
        node_with_feed(PricingPolicy::rest_at_mid(Duration::from_secs(60))?)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let mut stats = PassStats::default();

    let first = run_pass(&mut node.cell, &mut gateway, &mut feed, &mut stats, t(10))?;
    let PassOutcome::Ran { report, breaks, .. } = first else {
        panic!("a running node reported its pass as halted: {first:?}");
    };
    assert_eq!(report.orders.len(), 1, "the premise is one resting order");
    assert!(
        report.fills.is_empty(),
        "the premise is that nothing filled yet"
    );
    assert!(breaks.is_empty(), "{breaks:?}");
    let resting = report.orders[0].clone();
    assert_eq!(
        resting.price,
        dec!("100"),
        "the premise is an order resting at the mid"
    );
    assert_eq!(
        venue_position(&gateway),
        Decimal::ZERO,
        "the premise is a venue holding nothing for the cell yet"
    );

    // Somebody else sells into the cell's resting buy, between passes.
    let taken = gateway.seed_aggressor(&object(), Side::Sell, dec!("100"), dec!("400"), t(15))?;
    assert_eq!(
        taken, resting.quantity,
        "the flow did not fill the resting order"
    );
    assert_eq!(
        venue_position(&gateway),
        resting.quantity,
        "the venue's own account does not show the fill"
    );

    let second = run_pass(&mut node.cell, &mut gateway, &mut feed, &mut stats, t(20))?;
    let PassOutcome::Ran { report, breaks, .. } = second else {
        panic!("the node halted on the pass after a fill: {second:?}");
    };
    let confirmed: Vec<_> = report
        .fills
        .iter()
        .filter(|fill| fill.order_id == resting.order_id)
        .collect();
    assert_eq!(
        confirmed.len(),
        1,
        "the fill the venue reported on the resting order was not confirmed on the next pass: {:?}",
        report.fills
    );
    assert_eq!(confirmed[0].quantity, resting.quantity);
    assert_eq!(
        confirmed[0].price,
        dec!("100"),
        "a maker fills at its own price"
    );
    assert_eq!(
        node.cell.position(&venue(), &object()),
        venue_position(&gateway),
        "the cell's position and the venue's disagree"
    );
    assert!(
        breaks.is_empty(),
        "a fill both channels reported reconciled as a break: {breaks:?}"
    );
    assert!(
        !node.cell.is_halted(),
        "the node halted after a confirmed fill"
    );
    // The pass after the fill still traded: a second resting order went
    // out on the same side, and the settled first one is gone.
    assert_eq!(
        report.orders.len(),
        1,
        "the node stopped sending after its first fill"
    );
    assert!(
        node.cell
            .open_orders()
            .iter()
            .all(|order| order.order_id != resting.order_id),
        "a filled and agreed order was not settled"
    );
    assert_eq!(stats.passes, 2);
    assert_eq!(stats.fills, 1);
    assert_eq!(stats.breaks, 0);

    let snapshot = node.scrape_registry().snapshot();
    assert_eq!(
        snapshot.counter(names::EDGE_ORDERS_PLACED, &by("venue", VENUE)),
        2,
        "two passes, two orders placed"
    );
    assert_eq!(
        snapshot.counter(EDGE_FILLS_CONFIRMED, &by("venue", VENUE)),
        1,
        "the confirmed fill did not move the fill series the scrape serves"
    );
    assert_eq!(
        snapshot.gauge(names::EDGE_HALTED, &by("source", "kill_switch")),
        Some(0.0)
    );
    Ok(())
}

#[test]
fn a_marketable_order_fills_on_the_pass_it_is_sent_once_the_venue_has_a_touch_to_take() -> Result<()>
{
    // The other pricing. With nothing on the offer the first pass refuses
    // rather than rests; once an offer exists the next pass takes it, and
    // the fill is confirmed, matched and counted on that same pass.
    let (mut node, mut gateway, mut feed) = node_with_feed(PricingPolicy::Marketable)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    let mut stats = PassStats::default();

    let first = run_pass(&mut node.cell, &mut gateway, &mut feed, &mut stats, t(10))?;
    let PassOutcome::Ran { report, .. } = first else {
        panic!("{first:?}");
    };
    assert_eq!(
        report.signals.len(),
        1,
        "the premise is a strategy that fires"
    );
    assert!(
        report.orders.is_empty(),
        "an order was sent with nothing to take: {:?}",
        report.orders
    );
    assert!(!node.cell.is_halted());

    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(15))?;
    let second = run_pass(&mut node.cell, &mut gateway, &mut feed, &mut stats, t(20))?;
    let PassOutcome::Ran { report, breaks, .. } = second else {
        panic!("{second:?}");
    };
    assert_eq!(
        report.orders.len(),
        1,
        "no order was sent against the new offer: {:?}",
        report.refusals
    );
    assert_eq!(
        report.orders[0].price,
        dec!("101"),
        "a marketable buy was not sent at the ask"
    );
    assert_eq!(
        report.fills.len(),
        1,
        "the fill on acceptance was not confirmed on the pass"
    );
    assert_eq!(report.fills[0].quantity, report.orders[0].quantity);
    assert_eq!(
        node.cell.position(&venue(), &object()),
        venue_position(&gateway),
        "the cell's position and the venue's disagree"
    );
    assert!(breaks.is_empty(), "{breaks:?}");
    assert!(!node.cell.is_halted());
    assert_eq!(stats.fills, 1);

    let snapshot = node.scrape_registry().snapshot();
    assert_eq!(
        snapshot.counter(names::EDGE_ORDERS_PLACED, &by("venue", VENUE)),
        1
    );
    assert_eq!(
        snapshot.counter(EDGE_FILLS_CONFIRMED, &by("venue", VENUE)),
        1
    );
    Ok(())
}

#[test]
fn a_pass_with_nothing_listed_at_the_venue_refuses_under_the_venue_selection_gate() -> Result<()> {
    // The venue lists nothing, so the feed publishes nothing and the cell
    // holds no book for the instrument the strategy names. The pass must
    // run, the strategy must fire, and the gate that refuses must be the
    // first one that reads the book — venue selection, which finds no venue
    // holding one — counted on the scrape's registry under its own label.
    let (mut node, mut gateway, mut feed) = node_with_feed(PricingPolicy::Marketable)?;
    let mut stats = PassStats::default();
    let outcome = run_pass(&mut node.cell, &mut gateway, &mut feed, &mut stats, t(10))?;
    let PassOutcome::Ran {
        feed: tick, report, ..
    } = outcome
    else {
        panic!("a running node reported its pass as halted: {outcome:?}");
    };
    assert_eq!(
        tick.instruments, 0,
        "the premise is a venue listing nothing"
    );
    assert_eq!(
        report.signals.len(),
        1,
        "the premise is a strategy that fires"
    );
    assert!(
        report
            .refusals
            .iter()
            .any(|(gate, _)| gate == "venue_selection"),
        "the pass did not refuse under the venue-selection gate: {:?}",
        report.refusals
    );
    assert_eq!(gateway.submitted_count(), 0);

    let snapshot = node.scrape_registry().snapshot();
    assert_eq!(snapshot.counter(names::EDGE_WORK_PASSES, &base()), 1);
    assert_eq!(
        snapshot.counter(names::EDGE_REFUSALS, &by("gate", "venue_selection")),
        1,
        "the refusal was not counted under its gate on the registry the scrape serves"
    );
    Ok(())
}

#[test]
fn a_halted_node_runs_no_pass() -> Result<()> {
    // §46.2's second wire, engaged before the loop turns. The node must feed
    // its books and stop there: no pass counted, no signal, no order by any
    // path — the venue's own submitted count is the witness the registry
    // cannot fake.
    let (mut node, mut gateway, mut feed) = node_with_feed(PricingPolicy::Marketable)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    node.cell
        .apply_polled_halt(PolledHalt::Engaged("drill".to_string()), t(5));
    assert!(node.cell.is_halted(), "the premise is a halted cell");

    let mut stats = PassStats::default();
    let outcome = run_pass(&mut node.cell, &mut gateway, &mut feed, &mut stats, t(10))?;
    let PassOutcome::Halted { feed: tick, .. } = outcome else {
        panic!("a halted node ran a pass: {outcome:?}");
    };
    // The books still absorbed the venue's depth: a cell that stops seeing
    // the market cannot tell whether it is safe to resume.
    assert_eq!(tick.messages, 2, "a halted node stopped feeding its books");

    assert_eq!(stats.passes, 0);
    assert_eq!(stats.halted, 1);
    assert_eq!(gateway.submitted_count(), 0, "a halted node sent an order");
    let snapshot = node.scrape_registry().snapshot();
    assert_eq!(
        snapshot.counter(names::EDGE_WORK_PASSES, &base()),
        0,
        "a halted node counted a pass"
    );
    assert_eq!(
        snapshot.counter(names::EDGE_ORDERS_PLACED, &by("venue", VENUE)),
        0
    );
    assert_eq!(
        snapshot.gauge(names::EDGE_HALTED, &by("source", "polled")),
        Some(1.0),
        "the halt that stopped the pass is not on the registry"
    );
    Ok(())
}

#[test]
fn a_venue_feed_other_than_the_simulator_is_refused_at_start_naming_adr_0003() {
    // Unset is a node with no feed; `simulated` is the simulator; anything
    // else stops the process. The refusal must name the decision it would
    // take, because an operator who typed `live` needs to be told that is
    // not a value, and must not be told it is a typo.
    assert_eq!(FeedChoice::read(None).expect("unset is allowed"), None);
    assert_eq!(FeedChoice::read(Some("  ")).expect("blank is unset"), None);
    assert_eq!(
        FeedChoice::read(Some("simulated")).expect("the simulator is the one value"),
        Some(FeedChoice::Simulated)
    );
    for value in ["live", "Simulated", "rest", "multicast", "simulated,live"] {
        let error = match FeedChoice::read(Some(value)) {
            Ok(choice) => panic!("{FEED_VARIABLE}={value} was accepted as {choice:?}"),
            Err(error) => error,
        };
        let message = error.message();
        assert!(
            message.starts_with("configuration:"),
            "the refusal is not a configuration error, so the node would exit as a crash \
             rather than a misdeployment: {message}"
        );
        assert!(
            message.contains("ADR 0003"),
            "the refusal of {value} does not name the decision a live feed needs: {message}"
        );
        assert!(
            message.contains(value),
            "the refusal does not echo the value: {message}"
        );
    }
}
