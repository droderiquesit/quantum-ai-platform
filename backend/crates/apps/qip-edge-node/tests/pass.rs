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
use qip_edge::cell::PlacedOrder;
use qip_edge::cell::{CellConfig, PolledHalt, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::telemetry::{CellMetrics, EDGE_FILLS_CONFIRMED, EDGE_ORDERS_REPRICED};
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::{FEED_VARIABLE, FeedChoice, SimulatedFeed};
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::reprice::{Requote, Requoter};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Labels, labels, names};
use qip_routing::reprice::RepricePolicy;
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
    // Far above the one strategy's grant, so the pass loop is what decides;
    // the region bound has its own suite in `allocation.rs`.
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation)?;
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
    let outcome = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
    let PassOutcome::Ran {
        feed: tick,
        report,
        breaks,
        ..
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

    let first = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
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

    let second = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(20),
    )?;
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

    let first = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
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
    let second = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(20),
    )?;
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
    let outcome = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
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
    let outcome = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
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

/// A fill counted twice is a fill the centre attributes twice.
///
/// A partial fill leaves its order open, so the fill stays in the cell's
/// cumulative record; when that order later reaches its time to live, the
/// node used to match it by expired order id and count it again — in
/// `stats.fills` and in `report.fills`, which `main.rs` publishes to the
/// centre. The venue counter is the independent claim about the same fact
/// and is recorded once per confirmation, so the two disagreeing is the
/// defect. Nothing here has ever been seen in production, because no node
/// is deployed; it is reachable on the first one that is.
#[test]
fn a_partial_fill_on_an_order_that_later_expires_is_counted_once() -> Result<()> {
    // A five-second time to live and a timeline inside ten seconds, because
    // nothing in the pass loop answers a heartbeat and the simulated venue
    // degrades its session after thirty (`ExchangeSettings::orderly`). The
    // shape the defect needs is what matters and not the interval: the fill
    // must land on one pass and the expiry on a *later* one, so that the old
    // fill is still in the cumulative record when the withdrawal runs.
    let (mut node, mut gateway, mut feed) =
        node_with_feed(PricingPolicy::rest_at_mid(Duration::from_secs(5))?)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let mut stats = PassStats::default();

    let first = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(4),
    )?;
    let PassOutcome::Ran { report, .. } = first else {
        panic!("a running node reported its pass as halted: {first:?}");
    };
    assert_eq!(report.orders.len(), 1, "the premise is one resting order");
    let resting = report.orders[0].clone();

    // Somebody else takes part of it, so the order stays open with a fill
    // against it — the premise the whole test rests on.
    let taken = gateway.seed_aggressor(&object(), Side::Sell, dec!("100"), dec!("1"), t(6))?;
    assert!(
        taken > Decimal::ZERO && taken < resting.quantity,
        "the premise is a partial fill: {taken} of {}",
        resting.quantity
    );

    let second = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(8),
    )?;
    let PassOutcome::Ran { report, breaks, .. } = second else {
        panic!("the node halted on the pass after a partial fill: {second:?}");
    };
    assert!(breaks.is_empty(), "{breaks:?}");
    assert_eq!(
        report
            .fills
            .iter()
            .filter(|fill| fill.order_id == resting.order_id)
            .count(),
        1,
        "the premise is the partial fill confirmed exactly once: {:?}",
        report.fills
    );
    assert!(
        node.cell
            .open_orders()
            .iter()
            .any(|order| order.order_id == resting.order_id),
        "the premise is that a partly filled order is still open"
    );
    let after_fill = stats.fills;
    assert_eq!(after_fill, 1, "the premise is one fill counted so far");

    // Past the time to live of the order that filled, so the withdrawal runs
    // on a turn where the old fill is still in the cumulative record.
    let third = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(12),
    )?;
    let PassOutcome::Ran { report, breaks, .. } = third else {
        panic!("the node halted on the expiry pass: {third:?}");
    };
    assert!(breaks.is_empty(), "{breaks:?}");
    assert!(
        stats.expired >= 1,
        "the premise is that the resting order reached its time to live"
    );
    assert!(
        !report
            .fills
            .iter()
            .any(|fill| fill.order_id == resting.order_id),
        "the expiry pass re-published a fill from an earlier pass: {:?}",
        report.fills
    );
    assert_eq!(
        stats.fills, after_fill,
        "withdrawing an order counted its earlier fill again"
    );
    let snapshot = node.scrape_registry().snapshot();
    assert_eq!(
        snapshot.counter(EDGE_FILLS_CONFIRMED, &by("venue", VENUE)),
        stats.fills,
        "the node's fill count and the venue counter disagree about the same fact"
    );
    Ok(())
}

// --- the requote seam ------------------------------------------------------

/// A requoter on the node's registry: tick 0.01, stale at five ticks or
/// fifty basis points behind the touch, whichever binds first.
fn requoter(node: &NodeAssembly) -> Result<Requoter> {
    Requoter::new(
        RepricePolicy::new(dec!("0.01"), 5, 50.0),
        CellMetrics::new(Arc::clone(node.scrape_registry()), CELL, REGION),
    )
}

/// One pass against a 99/101 book, returning the single order it rested at
/// the mid — the premise every requote test starts from, asserted rather
/// than assumed.
fn rest_one_order(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    requoter: &mut Requoter,
    stats: &mut PassStats,
) -> Result<PlacedOrder> {
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let first = run_pass(&mut node.cell, gateway, feed, Some(requoter), stats, t(10))?;
    let PassOutcome::Ran {
        report,
        requotes,
        breaks,
        ..
    } = first
    else {
        panic!("a running node reported its pass as halted: {first:?}");
    };
    assert_eq!(report.orders.len(), 1, "the premise is one resting order");
    assert!(breaks.is_empty(), "{breaks:?}");
    assert!(
        requotes.is_empty(),
        "an order was repriced on the pass that sent it: {requotes:?}"
    );
    let resting = report.orders[0].clone();
    assert_eq!(
        resting.price,
        dec!("100"),
        "the premise is an order resting at the mid"
    );
    assert!(
        gateway.venue_holds_open(&resting.order_id),
        "the premise is an order the venue holds open"
    );
    assert_eq!(
        gateway.working_count(),
        1,
        "the premise is exactly one order followed at the venue"
    );
    Ok(resting)
}

/// The one thing this mechanism exists to guarantee: a stale resting order
/// is withdrawn, its withdrawal acknowledged, and only then its remainder
/// re-sent at the touch under a fresh id — so the venue never holds two
/// orders for one intention, and the cell's record keeps one id for it.
/// The fill on the replacement then reaches the cell as a fill on the order
/// the cell sent, on both channels, and reconciles clean. `reprice.rs`
/// proves the repricer refuses the race in isolation; this proves the node
/// carries the instruction to the venue in the right order.
#[test]
fn a_node_pass_reprices_a_stale_resting_child_after_draining_gateway_events_first() -> Result<()> {
    let (mut node, mut gateway, mut feed) =
        node_with_feed(PricingPolicy::rest_at_mid(Duration::from_secs(60))?)?;
    let mut requoter = requoter(&node)?;
    let mut stats = PassStats::default();
    let resting = rest_one_order(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut requoter,
        &mut stats,
    )?;
    assert!(
        node.cell.fills().is_empty(),
        "the premise is a resting order with no fill pending: {:?}",
        node.cell.fills()
    );
    assert_eq!(
        node.scrape_registry()
            .snapshot()
            .counter(EDGE_ORDERS_REPRICED, &by("venue", VENUE)),
        0,
        "the premise is a node that has repriced nothing"
    );

    // Somebody bids 100.50 above the cell's 100: the resting buy is now 50
    // ticks (about 50 bps) behind the touch, past the five-tick threshold.
    gateway.seed_touch(&object(), Side::Buy, dec!("100.5"), dec!("1"), t(15))?;

    let second = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(20),
    )?;
    let PassOutcome::Ran {
        report,
        requotes,
        breaks,
        ..
    } = second
    else {
        panic!("the node halted on the pass that should reprice: {second:?}");
    };
    assert_eq!(
        requotes.len(),
        1,
        "one stale order, one requote outcome: {requotes:?}"
    );
    let Requote::Replaced {
        order_id,
        withdrawn,
        replacement,
        quantity,
        price,
    } = &requotes[0]
    else {
        panic!(
            "the stale order was not cancelled and replaced: {:?}",
            requotes[0]
        );
    };
    assert_eq!(order_id, &resting.order_id);
    assert_eq!(
        withdrawn, &resting.order_id,
        "the original was not the order withdrawn"
    );
    assert_ne!(
        replacement, &resting.order_id,
        "the replacement reused the cancelled order's id, which an honouring venue dedupes away"
    );
    assert_eq!(
        *quantity, resting.quantity,
        "nothing filled, so the whole remainder is re-sent"
    );
    assert_eq!(
        *price,
        dec!("100.5"),
        "the replacement does not rest at the touch"
    );

    // One cancel, then one new order, never two live — by the venue's own
    // record, which is the one witness the node's bookkeeping cannot fake.
    assert!(
        !gateway.venue_holds_open(&resting.order_id),
        "the venue still holds the stale order open beside its replacement"
    );
    assert!(
        gateway.venue_holds_open(replacement),
        "the venue does not hold the replacement open"
    );
    assert_eq!(
        gateway.working_count(),
        1 + report.orders.len(),
        "the venue follows more orders than the replacement and what this pass sent"
    );
    assert_eq!(
        gateway.submitted_count(),
        2 + report.orders.len() as u64,
        "the venue saw more or fewer submissions than the original, its replacement and this \
         pass's own"
    );

    // The cell's record keeps one id per intention: the original is still
    // its one open order for that intention, and the replacement's id never
    // reaches it.
    let open = node.cell.open_orders();
    assert_eq!(
        open.iter()
            .filter(|order| order.order_id == resting.order_id)
            .count(),
        1,
        "the cell no longer holds the repriced intention open: {open:?}"
    );
    assert!(
        !open.iter().any(|order| &order.order_id == replacement),
        "the replacement's venue id leaked into the cell's record: {open:?}"
    );
    assert!(
        breaks.is_empty(),
        "a requote reconciled as a break: {breaks:?}"
    );
    assert!(!node.cell.is_halted(), "a requote halted the cell");
    assert_eq!(stats.repriced, 1);
    assert_eq!(
        node.scrape_registry()
            .snapshot()
            .counter(EDGE_ORDERS_REPRICED, &by("venue", VENUE)),
        1,
        "the requote did not move the series the scrape serves"
    );

    // And the fill on the replacement reaches the cell as a fill on the
    // order it sent, on both channels: somebody sells through everything
    // resting at 100.50 or better, and the next pass confirms it under the
    // cell's id, matches it against the drop copy, and keeps trading.
    gateway.seed_aggressor(&object(), Side::Sell, dec!("100.5"), dec!("200"), t(25))?;
    let third = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(30),
    )?;
    let PassOutcome::Ran { report, breaks, .. } = third else {
        panic!("the node halted on the pass after the replacement filled: {third:?}");
    };
    let on_intention: Vec<_> = report
        .fills
        .iter()
        .filter(|fill| fill.order_id == resting.order_id)
        .collect();
    assert_eq!(
        on_intention.len(),
        1,
        "the replacement's fill was not confirmed under the cell's own id: {:?}",
        report.fills
    );
    assert_eq!(on_intention[0].quantity, resting.quantity);
    assert_eq!(
        on_intention[0].price,
        dec!("100.5"),
        "the fill was not at the replacement's price"
    );
    assert!(
        !report
            .fills
            .iter()
            .any(|fill| &fill.order_id == replacement),
        "a fill reached the cell under the replacement's venue id: {:?}",
        report.fills
    );
    assert_eq!(
        node.cell.position(&venue(), &object()),
        venue_position(&gateway),
        "the cell's position and the venue's disagree after a repriced fill"
    );
    assert!(
        breaks.is_empty(),
        "a fill on a replacement reconciled as a break: {breaks:?}"
    );
    assert!(!node.cell.is_halted());
    Ok(())
}

/// A fill the venue reported since the last pass must be booked before the
/// order is judged stale, or the replacement carries a quantity that no
/// longer exists. The repricer's own header names this as the one ordering
/// the caller owes it; this proves the node honours it, with the partial
/// fill as the witness: the replacement must be for six, not ten.
#[test]
fn a_fill_that_arrived_this_pass_is_booked_before_staleness_is_judged() -> Result<()> {
    let (mut node, mut gateway, mut feed) =
        node_with_feed(PricingPolicy::rest_at_mid(Duration::from_secs(60))?)?;
    let mut requoter = requoter(&node)?;
    let mut stats = PassStats::default();
    let resting = rest_one_order(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut requoter,
        &mut stats,
    )?;

    // Between passes: somebody takes one share of the resting order, and
    // then the bid moves past the threshold. Both facts are waiting for the
    // next pass.
    let taken = gateway.seed_aggressor(&object(), Side::Sell, dec!("100"), dec!("1"), t(15))?;
    assert!(
        taken.is_positive() && taken < resting.quantity,
        "the premise is a partial fill: {taken} of {}",
        resting.quantity
    );
    gateway.seed_touch(&object(), Side::Buy, dec!("100.5"), dec!("1"), t(16))?;
    let remainder = resting.quantity - taken;

    let second = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(20),
    )?;
    let PassOutcome::Ran {
        report,
        requotes,
        breaks,
        ..
    } = second
    else {
        panic!("the node halted on the pass after a partial fill: {second:?}");
    };
    let booked: Vec<_> = report
        .fills
        .iter()
        .filter(|fill| fill.order_id == resting.order_id)
        .collect();
    assert_eq!(
        booked.len(),
        1,
        "the partial fill was not confirmed on this pass: {:?}",
        report.fills
    );
    assert_eq!(booked[0].quantity, taken);
    assert_eq!(
        requotes,
        vec![Requote::Replaced {
            order_id: resting.order_id.clone(),
            withdrawn: resting.order_id.clone(),
            replacement: format!("{}-c1", resting.order_id),
            quantity: remainder,
            price: dec!("100.5"),
        }],
        "the replacement does not carry the remainder after the fill this pass booked"
    );
    let open = node.cell.open_orders();
    let intention = open
        .iter()
        .find(|order| order.order_id == resting.order_id)
        .expect("the partly filled intention is still open");
    assert_eq!(intention.filled, taken, "the cell's record lost the fill");
    assert!(intention.closed.is_none());
    assert_eq!(
        node.cell.position(&venue(), &object()),
        venue_position(&gateway),
        "the cell's position and the venue's disagree"
    );
    assert!(breaks.is_empty(), "{breaks:?}");
    assert!(!node.cell.is_halted());
    assert_eq!(stats.fills, 1);
    assert_eq!(stats.repriced, 1);
    Ok(())
}

/// Inside the declared thresholds nothing moves: an order two ticks behind
/// the touch rests where it is, the venue holds the same order open, and the
/// series does not move. Without this the requoter would be a chaser that
/// pays a cancel round trip on every breath of the book — the failure the
/// thresholds and budgets exist to prevent.
#[test]
fn a_fresh_resting_child_is_not_repriced() -> Result<()> {
    let (mut node, mut gateway, mut feed) =
        node_with_feed(PricingPolicy::rest_at_mid(Duration::from_secs(60))?)?;
    let mut requoter = requoter(&node)?;
    let mut stats = PassStats::default();
    let resting = rest_one_order(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut requoter,
        &mut stats,
    )?;

    // Two ticks behind — about two basis points — against a threshold of
    // five ticks or fifty.
    gateway.seed_touch(&object(), Side::Buy, dec!("100.02"), dec!("1"), t(15))?;
    let second = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(20),
    )?;
    let PassOutcome::Ran {
        report, requotes, ..
    } = second
    else {
        panic!("{second:?}");
    };
    // The premise: the cell's own book shows the order behind the touch,
    // so a repricer that ignored its thresholds would have moved it.
    let best_bid = node
        .cell
        .liquidity()
        .get(&venue(), &object())
        .and_then(qip_orderbook::venue::VenueState::best_bid)
        .map(|level| level.price);
    assert_eq!(
        best_bid,
        Some(dec!("100.02")),
        "the premise is a touch that moved above the resting order"
    );
    assert!(
        requotes.is_empty(),
        "an order inside the drift thresholds was touched: {requotes:?}"
    );
    assert!(
        gateway.venue_holds_open(&resting.order_id),
        "the venue no longer holds the fresh order"
    );
    assert_eq!(
        gateway.submitted_count(),
        1 + report.orders.len() as u64,
        "something beyond the original and this pass's own orders was submitted"
    );
    assert_eq!(stats.repriced, 0);
    assert_eq!(
        node.scrape_registry()
            .snapshot()
            .counter(EDGE_ORDERS_REPRICED, &by("venue", VENUE)),
        0,
        "the requote series moved for an order that was not repriced"
    );
    Ok(())
}
