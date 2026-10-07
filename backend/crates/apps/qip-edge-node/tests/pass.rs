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
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, ManualClock, SystemClock, dec};
use qip_edge::cell::PlacedOrder;
use qip_edge::cell::WorkReport;
use qip_edge::cell::{CellConfig, PolledHalt, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::journal::Journal;
use qip_edge::policy::VerifiedPolicy;
use qip_edge::quoting::{Depletion, RateLimits};
use qip_edge::telemetry::{
    CellMetrics, EDGE_FILLS_CONFIRMED, EDGE_ORDERS_REPRICED, EDGE_SETTLEMENT_UNPROJECTED,
};
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::{FEED_VARIABLE, FeedChoice, SimulatedFeed};
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::mesh::{MeshLink, MeshSettings};
use qip_edge_node::mirror::{StoreMirror, batches};
use qip_edge_node::pass::{PassLog, PassMeter, PassOutcome, PassStats, run_pass};
use qip_edge_node::quote_limits::{QUOTE_LIMITS_VARIABLE, VenueQuoteLimits};
use qip_edge_node::reprice::{Requote, Requoter};
use qip_edge_node::share::RegionShareStatus;
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Labels, labels, names};
use qip_routing::reprice::RepricePolicy;
use qip_storage::kv::{KeyValueStore, MemoryKeyValueStore};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use qip_transport::RecordingSleeper;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    grant_for(CELL, dec!("1000000"), dec!("100000"), dec!("50000"))
}

/// A signed grant for `STRATEGY` at `cell`, with the limits the test names.
/// The signature is deterministic over the terms, so signing the same terms
/// again yields the name the centre's manifest would carry.
fn grant_for(
    cell: &str,
    gross: Decimal,
    order: Decimal,
    loss: Decimal,
) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            cell,
            gross,
            order,
            loss,
            vec![venue()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signed = build(&sign_payload(ENVELOPE_KEY, &unsigned.signing_payload()))?;
    VerifiedEnvelope::verify(signed, ENVELOPE_KEY, cell, t(1))
}

/// A verified payload for `cell` whose `capital_grants` slot names `grants`
/// — what the centre ships once it has partitioned the region's grant
/// (ADR 0039), built and signed here independently of the centre.
fn share_policy(
    cell: &str,
    sequence: u64,
    issued_at: Timestamp,
    grants: Vec<String>,
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, cell, issued_at);
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: grants,
        },
        issued_at,
    );
    VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, cell, issued_at)
}

fn refused_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        // Delimited equality, not `contains`: `region_reservation_abandoned`
        // has `region_reservation` as a prefix.
        .filter(|(recorded, _)| recorded == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

/// The node's pieces, assembled the way `main.rs` assembles them: one
/// registry, the simulated gateway, the simulated feed attached to the cell,
/// one firing strategy deployed under a signed grant with the pricing the
/// test names — and **no share applied**, so the table is at nothing, as a
/// node is until the centre's first payload reaches it.
fn unfunded_node_with_feed(
    pricing: PricingPolicy,
) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    // Far above the one strategy's grant, so the pass loop is what decides;
    // the region bound has its own suite in `allocation.rs`.
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    node.cell
        .deploy_with_pricing(compiled, program, grant()?, pricing)?;
    Ok((node, gateway, feed))
}

/// [`unfunded_node_with_feed`], then the share the centre would ship: a
/// payload naming the one grant the cell holds, applied at `t(5)` under
/// sequence 1, before any test's first pass at `t(10)`.
fn node_with_feed(
    pricing: PricingPolicy,
) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let (mut node, gateway, feed) = unfunded_node_with_feed(pricing)?;
    let named = grant()?.signature().to_string();
    node.cell
        .apply_policy(share_policy(CELL, 1, t(5), vec![named])?, t(5))?;
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

/// REFLEX-039: a requote is a cancel the venue acknowledges and an order the
/// venue accepts, and until the cell was told of either it sealed neither.
/// The chain said an order was sent at 100 and, some passes later, filled at
/// 100.50, with nothing between; and the cell's own open order went on
/// naming the limit it was first sent with, a price nothing rested at.
///
/// The witness is the hard case: the cancel races a fill. One share trades
/// before the withdrawal is acknowledged, so the venue withdraws the
/// remainder and not what was sent, and the replacement has both a new price
/// and a new size. Then the replacement is itself cancelled, at its time to
/// live, so both halves of "cancel and replace" are read back from the chain.
#[test]
fn a_requote_seals_its_cancel_acknowledgement_and_its_replacement_and_moves_the_open_order()
-> Result<()> {
    fn sealed(node: &NodeAssembly, kind: &str) -> Vec<Decision> {
        node.cell
            .journal()
            .entries()
            .iter()
            .filter(|entry| entry.decision.kind() == kind)
            .map(|entry| entry.decision.clone())
            .collect()
    }

    // A twelve-second time to live, so the order sent at `t(10)` is requoted
    // at `t(20)` and expires by `t(25)`: nothing in the pass loop answers a
    // heartbeat, and the simulated venue degrades its session after thirty.
    let (mut node, mut gateway, mut feed) =
        node_with_feed(PricingPolicy::rest_at_mid(Duration::from_secs(12))?)?;
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
        sealed(&node, "requote_withdrawn").is_empty() && sealed(&node, "order_replaced").is_empty(),
        "the premise is a chain with no requote in it"
    );

    // The race: one share trades, then the bid moves past the threshold.
    let taken = gateway.seed_aggressor(&object(), Side::Sell, dec!("100"), dec!("1"), t(15))?;
    assert!(
        taken.is_positive() && taken < resting.quantity,
        "the premise is a partial fill: {taken} of {}",
        resting.quantity
    );
    gateway.seed_touch(&object(), Side::Buy, dec!("100.5"), dec!("1"), t(16))?;
    let remainder = resting.quantity - taken;
    let replacement = format!("{}-c1", resting.order_id);

    let second = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(20),
    )?;
    let PassOutcome::Ran {
        requotes, breaks, ..
    } = second
    else {
        panic!("the node halted on the pass that should requote: {second:?}");
    };
    assert_eq!(
        requotes,
        vec![Requote::Replaced {
            order_id: resting.order_id.clone(),
            withdrawn: resting.order_id.clone(),
            replacement: replacement.clone(),
            quantity: remainder,
            price: dec!("100.5"),
        }],
        "the premise is one order withdrawn and re-sent at a new price and a new size"
    );
    assert!(breaks.is_empty(), "{breaks:?}");

    // The journal reflects each acknowledgement: the venue's of the cancel,
    // with the remainder it withdrew, and the venue's of the replacement.
    assert_eq!(
        sealed(&node, "requote_withdrawn"),
        vec![Decision::RequoteWithdrawn {
            order_id: resting.order_id.clone(),
            venue: VENUE.to_string(),
            withdrawn: resting.order_id.clone(),
            acknowledged: remainder.to_string(),
        }],
        "the cancel the venue acknowledged is not in the chain as it was acknowledged"
    );
    assert_eq!(
        sealed(&node, "order_replaced"),
        vec![Decision::OrderReplaced {
            order_id: resting.order_id.clone(),
            venue: VENUE.to_string(),
            replacement: replacement.clone(),
            quantity: remainder.to_string(),
            price: "100.5".to_string(),
        }],
        "the replacement the venue accepted is not in the chain as it was accepted"
    );
    // In the order the facts became known: the fill that raced the cancel,
    // then the withdrawal, then the replacement.
    let kinds: Vec<&str> = node
        .cell
        .journal()
        .entries()
        .iter()
        .filter(|entry| entry.at == t(20))
        .map(|entry| entry.decision.kind())
        .collect();
    let position = |kind: &str| {
        kinds
            .iter()
            .position(|recorded| *recorded == kind)
            .unwrap_or_else(|| panic!("the pass journaled no `{kind}`: {kinds:?}"))
    };
    assert!(
        position("filled") < position("requote_withdrawn")
            && position("requote_withdrawn") < position("order_replaced"),
        "the race was journaled out of order: {kinds:?}"
    );

    // Open-order state reflects them too: one intention, still open, resting
    // where the venue holds it, with the raced fill accounted exactly once.
    let open = node.cell.open_orders();
    let intention = open
        .iter()
        .find(|order| order.order_id == resting.order_id)
        .expect("the requoted intention is still open");
    assert_eq!(
        intention.price,
        dec!("100.5"),
        "the cell's open order still names a limit nothing rests at"
    );
    assert_eq!(
        intention.filled, taken,
        "the raced fill was lost or doubled"
    );
    assert_eq!(intention.remaining(), remainder);
    assert!(intention.closed.is_none());
    assert!(gateway.venue_holds_open(&replacement));
    assert!(!gateway.venue_holds_open(&resting.order_id));
    assert_eq!(
        node.cell.position(&venue(), &object()),
        venue_position(&gateway),
        "the cell's position and the venue's disagree after the race"
    );
    assert_eq!(node.cell.position(&venue(), &object()), taken);

    // And the plain cancel: at its time to live the replacement is
    // withdrawn, the venue holds nothing for the intention, and the chain
    // says what was withdrawn.
    let third = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(25),
    )?;
    assert!(matches!(third, PassOutcome::Ran { .. }), "{third:?}");
    assert!(
        !gateway.venue_holds_open(&replacement),
        "the venue still holds the order the cell cancelled"
    );
    let expired: Vec<Decision> = sealed(&node, "order_expired")
        .into_iter()
        .filter(|decision| {
            matches!(decision, Decision::OrderExpired { order_id, .. } if order_id == &resting.order_id)
        })
        .collect();
    assert_eq!(
        expired,
        vec![Decision::OrderExpired {
            order_id: resting.order_id.clone(),
            venue: VENUE.to_string(),
            withdrawn: remainder.to_string(),
        }],
        "the cancel is not in the chain with the quantity the venue withdrew"
    );
    assert_eq!(
        node.cell.position(&venue(), &object()),
        venue_position(&gateway),
        "the cell's position and the venue's disagree after the cancel"
    );
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

// --- ADR 0039: the node opens unfunded and funds only from a share ------------

const SECOND_CELL: &str = "london-2";
/// The gate literal `Cell::hold_region_capital` refuses under, matched by
/// delimited equality: `region_reservation_abandoned` carries it as a prefix.
const RESERVATION_GATE: &str = "region_reservation";

/// A node for `cell`, assembled through `assemble` as `main.rs` does, with a
/// two-sided book at the venue and the firing strategy deployed marketable
/// under a grant of exactly `gross` — and no share applied.
fn unfunded_node_for(
    cell: &str,
    gross: Decimal,
) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(cell, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    node.cell.deploy_with_pricing(
        compiled,
        program,
        grant_for(cell, gross, gross, gross)?,
        PricingPolicy::Marketable,
    )?;
    Ok((node, gateway, feed))
}

/// One pass, and the report it produced.
fn one_pass(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    stats: &mut PassStats,
    now: Timestamp,
) -> Result<WorkReport> {
    match run_pass(&mut node.cell, gateway, feed, None, stats, now)? {
        PassOutcome::Ran { report, .. } => Ok(*report),
        outcome => panic!("a running node reported its pass as halted: {outcome:?}"),
    }
}

/// What the region table charges one marketable pass of the firing strategy,
/// measured on a funded node rather than restated as a literal: the
/// degradation floor scales every size, and a literal here would be a number
/// this file believed and the cell did not.
fn spent_by_one_marketable_pass() -> Result<Decimal> {
    let (mut node, mut gateway, mut feed) = node_with_feed(PricingPolicy::Marketable)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let report = one_pass(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut PassStats::default(),
        t(10),
    )?;
    assert_eq!(
        report.orders.len(),
        1,
        "the probe premise failed: the funded pass placed no order: {:?}",
        report.refusals
    );
    let bound = node
        .cell
        .region_allocation_bound()
        .expect("a node assembled by this root holds a table");
    let free = node
        .cell
        .region_allocation_free()
        .expect("a node assembled by this root holds a table");
    let spent = bound - free;
    assert!(
        spent.is_positive(),
        "the probe premise failed: one pass charged nothing"
    );
    Ok(spent)
}

#[test]
fn an_unfunded_node_sends_nothing_until_its_first_share_arrives_and_then_sends_within_it()
-> Result<()> {
    // The deployment's first minute: the node is up, the feed runs, the
    // plan's strategy fires, and the centre has not yet shipped a payload.
    // Before ADR 0039 the node sent at once against the operator's amount,
    // which nothing had checked against the region's grant. Now it refuses,
    // and its health body says why — from the order count alone an
    // unfunded node reads exactly like a quiet market.
    let (mut node, mut gateway, mut feed) = unfunded_node_with_feed(PricingPolicy::Marketable)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    assert_eq!(
        node.cell.deployed_strategies(),
        vec![STRATEGY],
        "the premise is a node with a strategy to fire"
    );
    let before = RegionShareStatus::of(&node.cell);
    assert!(!before.funded, "an unfunded node reported itself funded");
    assert_eq!(before.bound, Some(Decimal::ZERO));
    assert_eq!(before.ceiling, Some(dec!("1000000000")));
    assert_eq!(before.sequence, None);
    let why = before
        .why
        .as_deref()
        .unwrap_or_else(|| panic!("an unfunded node's health body gives no reason"));
    assert!(
        why.contains("opened unfunded") && why.contains("policy payload"),
        "the reason does not say what the node is waiting for: {why}"
    );
    let json = before.to_json();
    assert!(
        json.contains(r#""funded":false"#) && json.contains(&format!("\"why\":\"{why}\"")),
        "the health block does not carry the state and the reason: {json}"
    );

    let mut stats = PassStats::default();
    let refused = one_pass(&mut node, &mut gateway, &mut feed, &mut stats, t(10))?;
    assert_eq!(
        refused.signals.len(),
        1,
        "the premise failed: the strategy did not fire: {:?}",
        refused.refusals
    );
    assert!(
        refused.orders.is_empty(),
        "an unfunded node sent an order: {:?}",
        refused.orders
    );
    assert_eq!(
        gateway.submitted_count(),
        0,
        "the venue saw an order from an unfunded node"
    );
    assert_eq!(
        refused_under(&refused, RESERVATION_GATE).len(),
        1,
        "the unfunded node was not refused exactly once under `{RESERVATION_GATE}`: {:?}",
        refused.refusals
    );

    // The centre's first payload names the grant the cell holds; the table
    // funds to the grant's gross, under the operator's ceiling.
    let named = grant()?.signature().to_string();
    node.cell
        .apply_policy(share_policy(CELL, 1, t(15), vec![named])?, t(15))?;
    let after = RegionShareStatus::of(&node.cell);
    assert!(after.funded, "the share did not fund the node: {after:?}");
    assert_eq!(after.bound, Some(dec!("1000000")));
    assert_eq!(after.sequence, Some(1));
    assert_eq!(
        after.why, None,
        "a funded node still gives a reason for placing nothing"
    );
    assert!(after.to_json().contains(r#""funded":true"#));

    let sent = one_pass(&mut node, &mut gateway, &mut feed, &mut stats, t(20))?;
    assert_eq!(
        sent.orders.len(),
        1,
        "the funded node did not send: {:?}",
        sent.refusals
    );
    assert_eq!(gateway.submitted_count(), 1);
    // Within it: what the table charged came out of the share, not the
    // ceiling.
    let free = node
        .cell
        .region_allocation_free()
        .expect("a node assembled by this root holds a table");
    assert!(
        free < dec!("1000000") && free.is_positive(),
        "the order was not charged against the share: free={free}"
    );
    Ok(())
}

#[test]
fn a_second_node_under_the_same_regions_grant_cannot_exceed_it_with_the_first() -> Result<()> {
    // Two nodes, two processes' worth of state — two `assemble` calls, two
    // private tables — under one region whose grant is exactly one pass's
    // worth. The centre partitions the grant before shipping: the whole of
    // it to the first cell, nothing to the second, so the second's payload
    // names no grant. Both nodes' strategies fire; only the first sends,
    // and what the two send together is at most the grant.
    let grant = spent_by_one_marketable_pass()?;
    let (mut first, mut first_gateway, mut first_feed) = unfunded_node_for(CELL, grant)?;
    let (mut second, mut second_gateway, mut second_feed) = unfunded_node_for(SECOND_CELL, grant)?;
    let first_named = grant_for(CELL, grant, grant, grant)?
        .signature()
        .to_string();
    first
        .cell
        .apply_policy(share_policy(CELL, 1, t(5), vec![first_named])?, t(5))?;
    second
        .cell
        .apply_policy(share_policy(SECOND_CELL, 1, t(5), vec![])?, t(5))?;
    assert_eq!(
        first.cell.region_allocation_bound(),
        Some(grant),
        "the premise failed: the first node was not funded to the grant"
    );
    assert_eq!(
        second.cell.region_allocation_bound(),
        Some(Decimal::ZERO),
        "the premise failed: the second node's share was not nothing"
    );

    // Contrast first: a second node whose payload names its own grant sends,
    // so what refuses below is the partition and not the amount.
    let (mut over, mut over_gateway, mut over_feed) = unfunded_node_for(SECOND_CELL, grant)?;
    let over_named = grant_for(SECOND_CELL, grant, grant, grant)?
        .signature()
        .to_string();
    over.cell
        .apply_policy(share_policy(SECOND_CELL, 1, t(5), vec![over_named])?, t(5))?;
    let over_report = one_pass(
        &mut over,
        &mut over_gateway,
        &mut over_feed,
        &mut PassStats::default(),
        t(10),
    )?;
    assert_eq!(
        over_report.orders.len(),
        1,
        "the contrast premise failed: the second node cannot send even under the whole grant: {:?}",
        over_report.refusals
    );

    let mut stats = PassStats::default();
    let first_report = one_pass(
        &mut first,
        &mut first_gateway,
        &mut first_feed,
        &mut stats,
        t(10),
    )?;
    assert_eq!(
        first_report.orders.len(),
        1,
        "the first node did not send within its share: {:?}",
        first_report.refusals
    );
    let second_report = one_pass(
        &mut second,
        &mut second_gateway,
        &mut second_feed,
        &mut stats,
        t(10),
    )?;
    assert_eq!(
        second_report.signals.len(),
        1,
        "the premise failed: the second node's strategy did not fire: {:?}",
        second_report.refusals
    );
    assert!(
        second_report.orders.is_empty(),
        "the second node sent against a grant the first node's share had exhausted: {:?}",
        second_report.orders
    );
    assert_eq!(second_gateway.submitted_count(), 0);
    assert_eq!(
        refused_under(&second_report, RESERVATION_GATE).len(),
        1,
        "the second node was not refused exactly once under `{RESERVATION_GATE}`: {:?}",
        second_report.refusals
    );
    let charged = |node: &NodeAssembly| -> Decimal {
        let bound = node
            .cell
            .region_allocation_bound()
            .expect("a node assembled by this root holds a table");
        let free = node
            .cell
            .region_allocation_free()
            .expect("a node assembled by this root holds a table");
        bound - free
    };
    assert_eq!(
        charged(&first) + charged(&second),
        grant,
        "the two nodes together committed more than the region's grant"
    );
    Ok(())
}

#[test]
fn a_replayed_lower_sequence_payload_leaves_the_nodes_table_unchanged() -> Result<()> {
    // A captured payload played again after the centre has narrowed the
    // cell. The node's mesh seam hands every verified payload to
    // `Cell::apply_policy` in arrival order, so this is what a replay
    // reaches; the table must stay where sequence 6 left it, the health
    // body must say so, and the next pass must still refuse.
    let (mut node, mut gateway, mut feed) = unfunded_node_with_feed(PricingPolicy::Marketable)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let named = grant()?.signature().to_string();
    let wide = share_policy(CELL, 5, t(5), vec![named])?;
    node.cell.apply_policy(wide.clone(), t(5))?;
    assert_eq!(
        node.cell.region_allocation_bound(),
        Some(dec!("1000000")),
        "the premise failed: sequence 5 did not fund the table"
    );
    node.cell
        .apply_policy(share_policy(CELL, 6, t(6), vec![])?, t(6))?;
    assert_eq!(
        node.cell.region_allocation_bound(),
        Some(Decimal::ZERO),
        "the premise failed: sequence 6 did not narrow the table"
    );
    let narrowed = RegionShareStatus::of(&node.cell);
    let journal_before = node.cell.journal().len();

    let replayed = node.cell.apply_policy(wide, t(7));
    assert!(replayed.is_err(), "the replayed sequence 5 was applied");
    assert_eq!(
        RegionShareStatus::of(&node.cell),
        narrowed,
        "the replayed payload changed the table"
    );
    assert_eq!(
        node.cell.journal().len(),
        journal_before,
        "the replayed payload was journaled as a decision"
    );
    assert_eq!(node.cell.region_share_sequence(), Some(6));
    assert!(!narrowed.funded);
    let why = narrowed
        .why
        .as_deref()
        .unwrap_or_else(|| panic!("a narrowed node's health body gives no reason"));
    assert!(
        why.contains("sequence 6") && why.contains("named no grant"),
        "the reason does not name the payload that narrowed the cell: {why}"
    );

    let report = one_pass(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut PassStats::default(),
        t(10),
    )?;
    assert!(
        report.orders.is_empty(),
        "the node sent after the replay: {:?}",
        report.orders
    );
    assert_eq!(
        refused_under(&report, RESERVATION_GATE).len(),
        1,
        "{:?}",
        report.refusals
    );
    Ok(())
}

// --- §36.3: reconciling against every venue before resuming ------------------

#[test]
fn a_node_that_restarted_reconciles_with_its_venue_before_it_forms_an_order() -> Result<()> {
    // §36.3's node-crash row, on the node's own loop. `qip-edge`'s suite
    // proves the cell refuses while the discipline stands; what it cannot
    // see is whether the node ever *obtains* the venue's account — and a
    // discipline nothing answers is a node that never resumes, which is the
    // failure on the other side of this control.
    //
    // The order within the pass is the property: the venue answers, and only
    // then does the cell form anything. Asserted on the chain rather than on
    // the report, because the chain is what an incident review reads.
    let (mut node, mut gateway, mut feed) = node_with_feed(PricingPolicy::Marketable)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    node.cell.require_reconciliation_before_resuming(
        "this journal store already holds 1 session(s)",
        t(9),
    )?;
    // Premise: the discipline really is standing when the pass begins.
    assert!(node.cell.awaiting_reconciliation().is_some());

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
        panic!("a running node reported its pass as halted: {outcome:?}");
    };
    assert_eq!(
        report.orders.len(),
        1,
        "the node did not resume after its venue answered: {:?}",
        report.refusals
    );
    assert!(node.cell.awaiting_reconciliation().is_none());

    let chain: Vec<&str> = node
        .cell
        .journal()
        .entries()
        .iter()
        .map(|entry| entry.decision.kind())
        .collect();
    let reconciled = chain
        .iter()
        .position(|kind| *kind == "venue_reconciled")
        .expect("the chain records the venue that answered");
    let sent = chain
        .iter()
        .position(|kind| *kind == "order_sent")
        .expect("the chain records the order");
    assert!(
        reconciled < sent,
        "the node formed an order before the venue answered: {chain:?}"
    );
    Ok(())
}

#[test]
fn a_restarted_node_whose_second_venue_cannot_answer_keeps_refusing_rather_than_resuming()
-> Result<()> {
    // Fail closed, and the honest cost of it. The node's order entry reaches
    // one venue, so a cell configured for two can never be shown the
    // second's account — and §36.3 says every venue. It stays paused and
    // says so on every pass, rather than resuming against a venue nothing
    // has reconciled with.
    let config = CellConfig::new(CELL, REGION)
        .with_venue(venue())
        .with_venue(VenueId::new("XNYS"));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let mut feed = feed;
    let (compiled, program) = firing_strategy()?;
    node.cell
        .deploy_with_pricing(compiled, program, grant()?, PricingPolicy::Marketable)?;
    let named = grant()?.signature().to_string();
    node.cell
        .apply_policy(share_policy(CELL, 1, t(5), vec![named])?, t(5))?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;

    // Premise: this node trades when nothing is holding it back, so an empty
    // order list below is the discipline and not the fixture.
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
        panic!("a running node reported its pass as halted: {outcome:?}");
    };
    assert_eq!(
        report.orders.len(),
        1,
        "the premise failed: an unarmed two-venue node placed no order: {:?}",
        report.refusals
    );

    node.cell.require_reconciliation_before_resuming(
        "this journal store already holds 1 session(s)",
        t(11),
    )?;
    let submitted = gateway.submitted_count();
    for (turn, at) in [t(12), t(13)].into_iter().enumerate() {
        let outcome = run_pass(
            &mut node.cell,
            &mut gateway,
            &mut feed,
            None,
            &mut stats,
            at,
        )?;
        let PassOutcome::Ran { report, .. } = outcome else {
            panic!("a running node reported its pass as halted: {outcome:?}");
        };
        assert!(
            report.orders.is_empty(),
            "turn {turn}: a node that cannot reconcile with every venue sent an order"
        );
        assert!(
            report
                .refusals
                .iter()
                .any(|(gate, _)| gate == "awaiting_reconciliation"),
            "turn {turn}: the pass was refused under {:?}",
            report.refusals
        );
    }
    assert_eq!(
        gateway.submitted_count(),
        submitted,
        "the venue saw an order from a node that had not reconciled"
    );
    let pending = node
        .cell
        .awaiting_reconciliation()
        .expect("the discipline still stands")
        .pending();
    assert_eq!(
        pending,
        vec!["XNYS".to_string()],
        "the venue that answered is still pending, or the one that cannot has cleared"
    );
    Ok(())
}

#[test]
fn the_account_the_node_hands_the_cell_is_the_venues_own_record_of_what_rests() -> Result<()> {
    // The account has to be the venue's answer rather than the gateway's
    // memory, or the comparison is the process checking itself. Proven by
    // resting an order, reading the account back, and finding the venue's
    // own remaining quantity in it.
    let (mut node, mut gateway, mut feed) =
        node_with_feed(PricingPolicy::rest_at_mid(Duration::from_secs(60))?)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let mut stats = PassStats::default();
    run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(10),
    )?;
    // Premise: an order really is resting at the venue, or the account below
    // would be empty for the uninteresting reason.
    let open = node.cell.open_orders();
    assert_eq!(open.len(), 1, "the pass rested no order of its own");

    let account = gateway.venue_account(t(11))?;
    assert_eq!(account.venue(), &venue());
    assert_eq!(
        account.open().len(),
        1,
        "the venue's account does not name the resting order: {:?}",
        account.open()
    );
    assert_eq!(
        account.open().get(&open[0].order_id),
        Some(&open[0].remaining()),
        "the venue's account disagrees with the cell about an order neither has touched"
    );
    // And a cell shown its own venue's account agrees with it, rather than
    // halting on a record it does stand behind.
    node.cell
        .require_reconciliation_before_resuming("a prior session was found", t(11))?;
    node.cell.observe_venue_account(account, t(11))?;
    assert!(
        !node.cell.is_halted(),
        "the venue's own account of the cell's own order read as a break"
    );
    assert!(node.cell.awaiting_reconciliation().is_none());
    Ok(())
}
// --- §29.2: the requoter comes through the message budget ------------------

/// Milliseconds rather than [`t`], because these tests are about a bucket
/// that refills once a second: two passes ten seconds apart would refill a
/// small budget to full between them and every assertion below would be
/// about a full budget wearing a depleted one's name.
fn at_ms(millis: i64) -> Timestamp {
    Timestamp::from_millis(1_760_000_000_000 + millis)
}

/// [`node_with_feed`], with the venue's message budget sized by the test.
///
/// `spendable` messages above a reserve of nothing: the reserve has its own
/// proof in `qip-edge`'s suite, and a fixture that let it move would leave
/// every refusal here attributable to either control.
fn budgeted_node(
    spendable: u32,
) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed, Requoter)> {
    let mut config = CellConfig::new(CELL, REGION).with_venue(venue());
    // Refills at one per second, and no test below spans a second; the
    // monitor's window is far wider than any test sends, so narrowing never
    // moves the floor the bands are measured against.
    config.quote_limits = RateLimits::new(spendable, 1, 0, 0, 64, 4_096)?;
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    node.cell.deploy_with_pricing(
        compiled,
        program,
        grant()?,
        PricingPolicy::rest_at_mid(Duration::from_secs(600))?,
    )?;
    let named = grant()?.signature().to_string();
    node.cell
        .apply_policy(share_policy(CELL, 1, t(5), vec![named])?, t(5))?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let requoter = requoter(&node)?;
    Ok((node, gateway, feed, requoter))
}

/// The first pass: one order resting at the mid of 100, asserted rather than
/// assumed, and the budget charged exactly one message for it.
fn rest_one_budgeted_order(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    requoter: &mut Requoter,
    stats: &mut PassStats,
    now: Timestamp,
) -> Result<PlacedOrder> {
    let PassOutcome::Ran {
        report, requotes, ..
    } = run_pass(&mut node.cell, gateway, feed, Some(requoter), stats, now)?
    else {
        panic!("a running node reported its pass as halted");
    };
    assert_eq!(report.orders.len(), 1, "the premise is one resting order");
    assert!(
        requotes.is_empty(),
        "an order was repriced on the pass that sent it: {requotes:?}"
    );
    let resting = report.orders[0].clone();
    assert_eq!(resting.price, dec!("100"), "the premise is a mid rest");
    assert_eq!(
        node.cell.quote_budget()[0].placements,
        1,
        "the premise failed: the order the pass sent was not billed to the budget"
    );
    Ok(resting)
}

/// A requote is two messages at the venue — a cancel and the replacement
/// that follows it — and until this seam existed the budget saw neither.
///
/// The failure that made it matter is the one `qip-edge`'s quoting module
/// was written to pre-empt, arriving by the door nobody watched: quote
/// traffic exceeds order traffic by one to two orders of magnitude, so a
/// session is far likelier to be cut off by its repricing than by its
/// sending. The node's requoter sent its cancel and its replacement straight
/// at the gateway, so the bucket read comfortable right up to the venue
/// dropping the session — at a moment nobody chose, with resting orders the
/// cell could then no longer withdraw.
#[test]
fn a_requote_is_charged_the_two_messages_it_sends_at_the_venue() -> Result<()> {
    let (mut node, mut gateway, mut feed, mut requoter) = budgeted_node(64)?;
    let mut stats = PassStats::default();
    let resting = rest_one_budgeted_order(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut requoter,
        &mut stats,
        at_ms(10),
    )?;
    let before = node.cell.quote_budget()[0].tokens;
    assert_eq!(
        before, 63,
        "the premise failed: the resting order cost something other than one message"
    );
    assert_eq!(
        node.cell.quote_depletion(VENUE),
        Depletion::Ample,
        "the premise failed: this test is about a budget with room, so the widened threshold \
         is not what decides anything here"
    );

    // Somebody bids 100.50: the resting buy is fifty ticks behind the touch,
    // well past the declared five.
    gateway.seed_touch(&object(), Side::Buy, dec!("100.5"), dec!("1"), at_ms(15))?;
    let PassOutcome::Ran {
        report, requotes, ..
    } = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        at_ms(20),
    )?
    else {
        panic!("the node halted on the pass that should reprice");
    };
    let Some(Requote::Replaced { order_id, .. }) = requotes.first() else {
        panic!("the premise failed: the stale order was not repriced: {requotes:?}");
    };
    assert_eq!(order_id, &resting.order_id);
    // The pass sends its own orders after the requoter has run, and those are
    // billed too. Pinned rather than netted out silently, so the arithmetic
    // below is about the requote and not about however many orders this
    // fixture's strategy happened to produce.
    let sent = report.orders.len() as u32;
    assert_eq!(
        sent, 1,
        "the premise failed: the repricing pass sent a different number of its own orders"
    );

    assert_eq!(
        before - node.cell.quote_budget()[0].tokens,
        2 + sent,
        "the requote's cancel and replacement left the process and the venue's message budget \
         did not see them, so the cell believes in headroom it has already spent"
    );
    Ok(())
}

/// §29.2's threshold-adaptation row, both halves.
///
/// The same drift against the same declared threshold: repriced on a budget
/// with room, and left resting once the budget has drained far enough to
/// widen the threshold past it. The second half alone would be satisfied by
/// a gate that refused everything, which is why the first is asserted here
/// rather than in a neighbouring test.
///
/// What the widening buys is not fewer messages for their own sake: it is
/// that the messages still in the bucket go to the orders that have drifted
/// furthest, instead of to whichever instrument happened to tick first while
/// the cell still had budget to answer it.
#[test]
fn a_stale_order_that_reprices_on_a_full_budget_rests_once_depletion_has_widened_the_threshold()
-> Result<()> {
    // Ten ticks behind the touch, against a declared threshold of five ticks
    // and fifty basis points: stale on ticks and fresh on basis points, so
    // the tick bound is the one under test and the widening of it is what
    // changes the answer.
    let drift_touch = dec!("100.1");

    // Half one: a budget with room reprices it.
    {
        let (mut node, mut gateway, mut feed, mut requoter) = budgeted_node(64)?;
        let mut stats = PassStats::default();
        rest_one_budgeted_order(
            &mut node,
            &mut gateway,
            &mut feed,
            &mut requoter,
            &mut stats,
            at_ms(10),
        )?;
        assert_eq!(node.cell.quote_depletion(VENUE), Depletion::Ample);
        gateway.seed_touch(&object(), Side::Buy, drift_touch, dec!("1"), at_ms(15))?;
        let PassOutcome::Ran { requotes, .. } = run_pass(
            &mut node.cell,
            &mut gateway,
            &mut feed,
            Some(&mut requoter),
            &mut stats,
            at_ms(20),
        )?
        else {
            panic!("the node halted on the pass that should reprice");
        };
        assert!(
            matches!(requotes.first(), Some(Requote::Replaced { .. })),
            "the premise failed: ten ticks of drift does not reprice even on a full budget, so \
             nothing below distinguishes a widened threshold from an unstale order: {requotes:?}"
        );
    }

    // Half two: the same drift, on a budget drained into the depleted band.
    let (mut node, mut gateway, mut feed, mut requoter) = budgeted_node(8)?;
    let mut stats = PassStats::default();
    let resting = rest_one_budgeted_order(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut requoter,
        &mut stats,
        at_ms(10),
    )?;
    // Drained the way the requoter drains it — by requoting — rather than by
    // reaching into the bucket. Seven spendable, less two twice, is three of
    // eight: the depleted band, whose multiple is four.
    for _ in 0..2 {
        assert!(
            node.cell.spend_requote(&venue(), at_ms(11)).is_admitted(),
            "the premise failed: the drain could not be funded"
        );
    }
    assert_eq!(
        node.cell.quote_budget()[0].tokens,
        3,
        "the premise failed: the drain did not leave three of eight messages"
    );
    assert_eq!(
        node.cell.quote_depletion(VENUE),
        Depletion::Depleted,
        "the premise failed: the drain did not reach the band this test is about"
    );
    assert!(
        node.cell.requote_fundable(&venue(), at_ms(15)),
        "the premise failed: this budget cannot fund a requote at all, so a refusal below would \
         be the funding gate rather than the widened threshold"
    );

    gateway.seed_touch(&object(), Side::Buy, drift_touch, dec!("1"), at_ms(15))?;
    let PassOutcome::Ran {
        report, requotes, ..
    } = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        at_ms(20),
    )?
    else {
        panic!("the node halted on the pass that should have held its requote");
    };
    let Some(Requote::ThresholdWidened {
        order_id,
        depletion,
        widened_ticks,
        ..
    }) = requotes.first()
    else {
        panic!(
            "a depleted budget repriced at the declared threshold, so the adaptation changes \
             nothing: {requotes:?}"
        );
    };
    assert_eq!(order_id, &resting.order_id);
    assert_eq!(*depletion, Depletion::Depleted);
    assert_eq!(
        *widened_ticks, 20,
        "the depleted band did not widen the declared five ticks by its stated multiple of four"
    );
    assert!(
        gateway.venue_holds_open(&resting.order_id),
        "the order was withdrawn although its requote was held"
    );
    assert_eq!(
        3 - node.cell.quote_budget()[0].tokens,
        report.orders.len() as u32,
        "the only messages this pass should have spent are the orders it sent itself; a held \
         requote spent something anyway"
    );
    Ok(())
}

/// A requote the venue session cannot carry withdraws nothing.
///
/// The failure prevented is the asymmetric one: a cancel that is funded and
/// a replacement that is not leaves the cell unquoted where it had merely
/// been stale. A stale quote is a price; no quote is an absence, and the
/// repricer exists to improve the first rather than to create the second.
/// So both messages are asked for together, before the repricer is consulted
/// at all.
#[test]
fn a_requote_the_budget_cannot_fund_whole_withdraws_nothing_and_says_which_control_stopped_it()
-> Result<()> {
    // Two spendable messages. The resting order costs one, leaving one — and
    // one is not a requote. The drift is fifty ticks, which clears even the
    // drawn band's widened ten, so the funding gate is the only thing that
    // can refuse here.
    let (mut node, mut gateway, mut feed, mut requoter) = budgeted_node(2)?;
    let mut stats = PassStats::default();
    let resting = rest_one_budgeted_order(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut requoter,
        &mut stats,
        at_ms(10),
    )?;
    assert_eq!(
        node.cell.quote_depletion(VENUE),
        Depletion::Drawn,
        "the premise failed: the fixture is not in the band this test reasons about"
    );
    assert!(
        !node.cell.requote_fundable(&venue(), at_ms(15)),
        "the premise failed: one message read as enough for a two-message requote"
    );

    gateway.seed_touch(&object(), Side::Buy, dec!("100.5"), dec!("1"), at_ms(15))?;
    let PassOutcome::Ran {
        report, requotes, ..
    } = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        at_ms(20),
    )?
    else {
        panic!("the node halted on the pass that should have refused its requote");
    };
    let Some(Requote::BudgetRefused { order_id, reason }) = requotes.first() else {
        panic!(
            "a requote the venue's budget cannot carry was not refused as one: {requotes:?}. A \
             `Throttled` here would be the repricer declining to chase, which is a different \
             fact and a different number to tune"
        );
    };
    assert_eq!(order_id, &resting.order_id);
    assert!(
        reason.contains("cannot fund the 2 message(s)"),
        "the refusal does not say what it could not pay for: {reason}"
    );
    assert!(
        gateway.venue_holds_open(&resting.order_id),
        "the cancel went out although the replacement could not be funded, so the cell is \
         unquoted where it was merely stale"
    );
    assert_eq!(
        gateway.working_count(),
        1 + report.orders.len(),
        "the venue holds a different number of orders than the resting one plus whatever this \
         pass sent itself, so the refused requote withdrew or added something"
    );
    Ok(())
}

#[test]
fn binding_the_simulated_feed_states_instant_settlement_for_the_venue_it_drives() -> Result<()> {
    // §56.2 rule 21 on the node's own seam. The cell's settlement gate
    // projects a cycle only against venues it holds terms for, and counts
    // the rest on a gauge; a node that never stated terms for the venue its
    // feed drives would stand on that gauge for ever, and the gate would be
    // a control the deployed shape never evaluates. The simulator books a
    // fill the instant it reports it, so `instant` is the simulator's own
    // fact — and binding the feed is where the node states it.
    //
    // Premise: an assembled node whose feed is *not* bound reports the one
    // venue as unprojected after a pass, so the zero below is the binding's
    // doing and not a gauge that reads zero on nothing.
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut unbound = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    unbound.cell.work(t(10), &mut gateway)?;
    assert_eq!(
        unbound
            .scrape_registry()
            .snapshot()
            .gauge(EDGE_SETTLEMENT_UNPROJECTED, &base()),
        Some(1.0),
        "the premise failed: a node with no feed bound does not report its venue unprojected"
    );

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
    assert!(
        matches!(outcome, PassOutcome::Ran { .. }),
        "a running node reported its pass as halted: {outcome:?}"
    );
    assert_eq!(
        node.scrape_registry()
            .snapshot()
            .gauge(EDGE_SETTLEMENT_UNPROJECTED, &base()),
        Some(0.0),
        "binding the simulated feed left its venue without settlement terms"
    );
    assert!(
        node.cell
            .config()
            .settlement
            .get(VENUE)
            .is_some_and(qip_edge::settlement::SettlementTerms::is_instant),
        "the terms the feed stated are not the simulator's instant credit"
    );
    Ok(())
}

/// A peer address nothing listens on: bind to learn a free port, then drop
/// the listener so every connect is refused, which is what a cut link to the
/// centre looks like from the cell's side.
fn severed_centre() -> Result<String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|error| qip_core::error::Error::io(error.to_string()))?;
    let address = listener
        .local_addr()
        .map_err(|error| qip_core::error::Error::io(error.to_string()))?;
    Ok(format!("http://{address}"))
}

#[test]
fn a_cell_cut_off_from_the_centre_keeps_deciding_from_local_artifacts_within_the_pass_budget()
-> Result<()> {
    // ADR 0008 and EXPAND-011: the reflex path is local. Every pass here is
    // followed by a mesh tick against a centre that refuses every
    // connection, the only route a cell has to any central endpoint, and the
    // pass must neither wait on it nor stop deciding. A generous wall-clock
    // budget is deliberate: a pass that blocked on a socket timeout would
    // cost seconds, not milliseconds, so the bound catches that failure
    // without being a flaky micro-benchmark.
    const PASS_BUDGET: std::time::Duration = std::time::Duration::from_millis(500);
    let (mut node, mut gateway, mut feed) = node_with_feed(PricingPolicy::Marketable)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let mut link = qip_edge_node::mesh::MeshLink::connect_with(
        &qip_edge_node::mesh::MeshSettings {
            cell: CELL.to_string(),
            region: REGION.to_string(),
            peer: severed_centre()?,
            seed: 3,
        },
        b"pass-test-mesh-key",
        Arc::new(qip_core::ManualClock::new(t(0))),
        Arc::new(qip_transport::RecordingSleeper::new()),
    )?;

    let mut stats = PassStats::default();
    let mut slowest = std::time::Duration::ZERO;
    let mut last_report = WorkReport::default();
    for second in 10..13 {
        let started = std::time::Instant::now();
        let outcome = run_pass(
            &mut node.cell,
            &mut gateway,
            &mut feed,
            None,
            &mut stats,
            t(second),
        )?;
        slowest = slowest.max(started.elapsed());
        let PassOutcome::Ran { report, .. } = outcome else {
            panic!("a cell cut off from the centre stopped running: {outcome:?}");
        };
        last_report = *report;
        let tick = link.exchange(&mut node.cell, &last_report, t(second));
        assert!(
            tick.poll_error.is_some(),
            "the premise is a centre that cannot be reached: {tick:?}"
        );
    }

    assert_eq!(
        stats.passes, 3,
        "a pass was skipped while the centre was gone"
    );
    assert!(
        stats.orders >= 1,
        "no decision reached the venue from local artifacts: {last_report:?}"
    );
    assert!(gateway.submitted_count() >= 1);
    assert!(!node.cell.is_halted(), "losing the centre halted the cell");
    assert!(
        slowest < PASS_BUDGET,
        "a pass took {slowest:?} with the centre unreachable"
    );
    Ok(())
}

#[test]
fn an_assembled_node_enables_order_taking_and_routing_at_its_venue_and_nothing_else() -> Result<()>
{
    use qip_execution_engine::modes::ExecutionMode;

    let (node, _gateway, _feed) = unfunded_node_with_feed(PricingPolicy::Marketable)?;
    let gate = node
        .cell
        .mode_gate()
        .ok_or_else(|| qip_core::error::Error::not_found("the mode gate on an assembled node"))?;
    // Premise: the loop covers all eight modes.
    assert_eq!(ExecutionMode::ALL.len(), 8);
    for mode in ExecutionMode::ALL {
        let enabled = matches!(mode, ExecutionMode::OrderTaking | ExecutionMode::Routing);
        assert_eq!(
            gate.admit(venue().as_str(), mode).is_ok(),
            enabled,
            "{mode:?}"
        );
    }
    Ok(())
}

// --- ARCH-010: every remote store unreachable --------------------------------

/// A journal store that can be cut off and restored: every call fails while
/// `severed` is set, which is what an unreachable disk or bucket looks like
/// to the mirror. It wraps the engine store the node really opens, so what
/// ships after the outage is read back through the operator's own path.
#[derive(Debug)]
struct SeverableStore {
    inner: Arc<dyn qip_storage::kv::KeyValueStore>,
    severed: Arc<std::sync::atomic::AtomicBool>,
}

impl SeverableStore {
    fn reachable(&self) -> Result<()> {
        if self.severed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(qip_core::error::Error::io(
                "the journal store is unreachable",
            ));
        }
        Ok(())
    }
}

impl qip_storage::kv::KeyValueStore for SeverableStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        self.reachable()?;
        self.inner.get(key)
    }
    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        self.reachable()?;
        self.inner.put(key, value)
    }
    fn delete(&self, key: &str) -> Result<bool> {
        self.reachable()?;
        self.inner.delete(key)
    }
    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        self.reachable()?;
        self.inner.keys_with_prefix(prefix)
    }
    fn len(&self) -> Result<usize> {
        self.reachable()?;
        self.inner.len()
    }
}

/// What a pass decided, in the two forms a difference would show in.
type Decided = (Vec<PlacedOrder>, Vec<(String, String)>);

/// A funded node with a two-sided touch at its venue, and three passes of
/// it. `before` runs ahead of each pass, where `main.rs`'s loop flushes the
/// journal and exchanges with the centre.
fn three_passes(
    mut before: impl FnMut(&mut NodeAssembly, &WorkReport, Timestamp),
) -> Result<(NodeAssembly, Vec<Decided>)> {
    let (mut node, mut gateway, mut feed) = node_with_feed(PricingPolicy::Marketable)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let mut stats = PassStats::default();
    let mut last_report = WorkReport::default();
    let mut decided = Vec::new();
    for second in 10..13 {
        before(&mut node, &last_report, t(second));
        let outcome = run_pass(
            &mut node.cell,
            &mut gateway,
            &mut feed,
            None,
            &mut stats,
            t(second),
        )?;
        let PassOutcome::Ran { report, .. } = outcome else {
            panic!("the pass at {second} did not run: {outcome:?}");
        };
        decided.push((report.orders.clone(), report.refusals.clone()));
        last_report = *report;
    }
    Ok((node, decided))
}

#[test]
fn a_cell_whose_journal_store_and_centre_are_both_unreachable_decides_exactly_as_before_and_ships_the_held_record_when_the_store_returns()
-> Result<()> {
    use qip_edge_node::mirror::{StoreMirror, batches};
    use std::sync::atomic::{AtomicBool, Ordering};

    // The failure this prevents: a storage or network outage becoming a
    // trading outage, or — worse — a cell that keeps trading through one and
    // loses the record of what it did. `main.rs` reports a failed flush and
    // keeps serving; until this test nothing drove a pass after one.

    // The control: the same node, the same venue, nothing remote at all.
    let (_, undisturbed) = three_passes(|_, _, _| {})?;
    assert!(
        undisturbed.iter().any(|(orders, _)| !orders.is_empty()),
        "the premise is a cell that decides something; three passes placed nothing"
    );

    let root = std::env::temp_dir().join(format!("qip-edge-pass-outage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)
        .map_err(|error| qip_core::error::Error::io(error.to_string()))?;
    let severed = Arc::new(AtomicBool::new(false));
    let store: Arc<dyn qip_storage::kv::KeyValueStore> = Arc::new(SeverableStore {
        inner: qip_storage::settings::StorageSettings::from_values(Some("engine"), root.to_str())?
            .key_value("cell-journal")?,
        severed: Arc::clone(&severed),
    });
    let mut mirror = StoreMirror::open(Arc::clone(&store), CELL, t(0))?;
    let mut link = qip_edge_node::mesh::MeshLink::connect_with(
        &qip_edge_node::mesh::MeshSettings {
            cell: CELL.to_string(),
            region: REGION.to_string(),
            peer: severed_centre()?,
            seed: 3,
        },
        b"pass-test-mesh-key",
        Arc::new(qip_core::ManualClock::new(t(0))),
        Arc::new(qip_transport::RecordingSleeper::new()),
    )?;

    // Both remote dependencies are cut before the first pass and stay cut
    // for all three: the store the journal ships to, and the centre.
    severed.store(true, Ordering::SeqCst);
    let mut failed_flushes = 0;
    let mut failed_exchanges = 0;
    let (mut node, cut_off) = three_passes(|node, last_report, now| {
        if node.cell.flush(&mut mirror, now).is_err() {
            failed_flushes += 1;
        }
        if link
            .exchange(&mut node.cell, last_report, now)
            .poll_error
            .is_some()
        {
            failed_exchanges += 1;
        }
    })?;
    assert_eq!(
        (failed_flushes, failed_exchanges),
        (3, 3),
        "the premise is that neither the store nor the centre could be reached"
    );

    assert_eq!(
        cut_off, undisturbed,
        "losing the journal store and the centre changed what the cell decided"
    );
    assert!(!node.cell.is_halted(), "the outage halted the cell");
    assert_eq!(
        mirror.shipped_entries(),
        0,
        "something shipped to a store nothing could reach"
    );
    let held = node.cell.journal().unshipped().len();
    assert_eq!(
        held,
        node.cell.journal().len(),
        "an entry left the pending set although no flush succeeded"
    );
    assert!(held > 0);

    // The store returns. Everything held ships, in one chained record a
    // reader of the store can verify from its start.
    severed.store(false, Ordering::SeqCst);
    assert_eq!(node.cell.flush(&mut mirror, t(13))?, held);
    assert!(node.cell.journal().unshipped().is_empty());
    let shipped = batches(store.as_ref())?;
    let mut tail = qip_edge::journal::Journal::GENESIS.to_string();
    let mut entries = 0;
    for batch in &shipped {
        batch.verify_against(&tail)?;
        tail = batch.tail_digest();
        entries += batch.entries.len();
    }
    assert_eq!(
        entries, held,
        "the store does not hold every decision made during the outage"
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

// --- EXEC-004: the venue's own rate and ratio, as the deployment states them

/// Three messages of burst refilling at one a second, two messages per trade
/// refused over a minute — the declaration a deployment would write, read
/// through the door `main.rs` reads it through.
const STATED_LIMITS: &str = "XLON=3:1:0:0:2:64:60000";

/// [`node_with_feed`], with the venue's limits read from a declaration, and
/// a two-sided touch at the venue: a marketable buy takes the offer and
/// fills, an order rested at the mid does not.
fn node_under_stated_limits(
    pricing: PricingPolicy,
) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let venues = [venue()];
    let config = VenueQuoteLimits::read(Some(STATED_LIMITS), &venues)?
        .apply(CellConfig::new(CELL, REGION).with_venue(venue()));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    node.cell
        .deploy_with_pricing(compiled, program, grant()?, pricing)?;
    let named = grant()?.signature().to_string();
    node.cell
        .apply_policy(share_policy(CELL, 1, t(5), vec![named])?, t(5))?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    assert_eq!(
        node.cell
            .quote_limits_at(&venue())
            .map(RateLimits::capacity),
        Some(3),
        "the premise failed: the declared limits did not reach the budget the node's cell spends"
    );
    Ok((node, gateway, feed))
}

/// Run `instants.len()` passes and return every quote-budget refusal.
fn refusals_over(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    instants: &[Timestamp],
) -> Result<Vec<String>> {
    let mut stats = PassStats::default();
    let mut refused = Vec::new();
    for now in instants {
        let PassOutcome::Ran { report, .. } =
            run_pass(&mut node.cell, gateway, feed, None, &mut stats, *now)?
        else {
            panic!("a running node reported its pass as halted");
        };
        refused.extend(
            refused_under(&report, "quote_budget")
                .into_iter()
                .map(str::to_string),
        );
    }
    Ok(refused)
}

#[test]
fn a_node_refuses_quotes_past_the_venues_stated_rate_before_the_simulated_venue_sees_them()
-> Result<()> {
    // The failure this prevents: every node ran one default ceiling of 4,096
    // messages on every venue because this binary never set a limit, so a
    // venue's own rate was found out when the venue enforced it.
    let (mut node, mut gateway, mut feed) = node_under_stated_limits(PricingPolicy::Marketable)?;
    // Five passes inside one second, against a rate of one a second.
    let instants: Vec<Timestamp> = (0..5).map(|pass| at_ms(10_000 + pass)).collect();
    let refused = refusals_over(&mut node, &mut gateway, &mut feed, &instants)?;
    assert_eq!(
        gateway.submitted_count(),
        3,
        "the venue received more than the burst it was stated to allow: {refused:?}"
    );
    assert_eq!(
        node.cell.quote_budget()[0].trades,
        3,
        "the premise failed: the orders did not fill, so the ratio may be what refused"
    );
    assert_eq!(refused.len(), 2, "the excess was not refused once each");
    assert!(
        refused
            .iter()
            .all(|reason| reason.contains("quote budget at XLON")),
        "a refusal past the rate did not name the bucket: {refused:?}"
    );
    assert_eq!(
        node.scrape_registry()
            .snapshot()
            .counter(names::EDGE_REFUSALS, &by("gate", "quote_budget")),
        2,
        "the refusals did not reach the series the scrape serves"
    );
    Ok(())
}

#[test]
fn a_node_refuses_quotes_past_the_venues_stated_message_to_trade_ratio_before_the_simulated_venue_sees_them()
-> Result<()> {
    // The ratio half. The monitor narrowed and never refused, so a stream of
    // quotes that nothing fills was sent at the sustained rate for as long as
    // it ran.
    let (mut node, mut gateway, mut feed) =
        node_under_stated_limits(PricingPolicy::rest_at_mid(Duration::from_secs(600))?)?;
    // Two seconds apart: slower than the rate, so the bucket is full on
    // every pass, and rested inside the spread, so nothing fills.
    let instants: Vec<Timestamp> = (0..5).map(|pass| t(10 + pass * 2)).collect();
    let refused = refusals_over(&mut node, &mut gateway, &mut feed, &instants)?;
    assert_eq!(
        node.cell.quote_budget()[0].trades,
        0,
        "the premise failed: the venue filled an order, so the stream is not short of trades"
    );
    assert_eq!(
        gateway.submitted_count(),
        2,
        "the venue received more than two messages against no trade at two per trade: \
         {refused:?}"
    );
    assert_eq!(refused.len(), 3, "the excess was not refused once each");
    assert!(
        refused
            .iter()
            .all(|reason| reason.contains("message-to-trade ratio at XLON")),
        "a refusal past the ratio did not name the limit: {refused:?}"
    );
    assert_eq!(
        node.scrape_registry()
            .snapshot()
            .counter(names::EDGE_REFUSALS, &by("gate", "quote_budget")),
        3,
        "the refusals did not reach the series the scrape serves"
    );
    Ok(())
}

#[test]
fn a_node_sends_a_stream_within_the_venues_stated_rate_and_ratio_in_full() -> Result<()> {
    // What distinguishes the two limits above from a node that refuses
    // everything: slower than the rate, and every message a trade.
    let (mut node, mut gateway, mut feed) = node_under_stated_limits(PricingPolicy::Marketable)?;
    // Two seconds apart, inside the simulated session's heartbeat allowance.
    let instants: Vec<Timestamp> = (0..5).map(|pass| t(10 + pass * 2)).collect();
    let refused = refusals_over(&mut node, &mut gateway, &mut feed, &instants)?;
    assert!(
        refused.is_empty(),
        "a stream inside both limits was refused: {refused:?}"
    );
    assert_eq!(
        gateway.submitted_count(),
        5,
        "a stream inside both limits did not reach the venue in full"
    );
    assert_eq!(node.cell.quote_budget()[0].trades, 5, "the premise failed");
    Ok(())
}

#[test]
fn the_binary_reads_the_quote_limits_variable_and_applies_it_to_the_cell_it_assembles() {
    // `main.rs` is a binary no test can call, so this is the narrow claim a
    // source check can honestly make: the variable is read by its constant
    // and what was read is applied to the configuration. That applying it
    // changes what the cell refuses is the three tests above.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert!(
        source.contains("std::env::var(QUOTE_LIMITS_VARIABLE)"),
        "main.rs does not read {QUOTE_LIMITS_VARIABLE}"
    );
    assert!(
        source.contains("config.quote_limits.apply(cell_config)"),
        "main.rs reads the venue limits and never applies them to the cell configuration"
    );
}

// --- EXEC-037: the order path with the journal sink and the centre gone -----

/// A journal store that can be taken away and given back.
///
/// Every operation fails while it is down, the way a store on the far side
/// of a partition fails: nothing is written and nothing is read. What it
/// held before stays held, so the record after the outage is the record
/// before it plus whatever is shipped once it returns.
#[derive(Debug, Default)]
struct PartitionedStore {
    inner: MemoryKeyValueStore,
    down: AtomicBool,
}

impl PartitionedStore {
    fn reachable(&self) -> Result<()> {
        if self.down.load(Ordering::SeqCst) {
            return Err(Error::io(
                "the journal store is unreachable; the batch stays in the cell's journal and \
                 ships on the next flush that finds the store",
            ));
        }
        Ok(())
    }
}

impl KeyValueStore for PartitionedStore {
    fn get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        self.reachable()?;
        self.inner.get(key)
    }

    fn put(&self, key: &str, value: serde_json::Value) -> Result<()> {
        self.reachable()?;
        self.inner.put(key, value)
    }

    fn delete(&self, key: &str) -> Result<bool> {
        self.reachable()?;
        self.inner.delete(key)
    }

    fn keys_with_prefix(&self, prefix: &str) -> Result<Vec<String>> {
        self.reachable()?;
        self.inner.keys_with_prefix(prefix)
    }

    fn len(&self) -> Result<usize> {
        self.reachable()?;
        self.inner.len()
    }
}

/// A link to a central plane nobody is listening as: the port was bound to
/// learn a free address and released before the link was built.
fn link_to_a_dead_centre() -> Result<MeshLink> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|error| Error::io(error.to_string()))?;
    let address = listener
        .local_addr()
        .map_err(|error| Error::io(error.to_string()))?;
    drop(listener);
    MeshLink::connect_with(
        &MeshSettings {
            cell: CELL.to_string(),
            region: REGION.to_string(),
            peer: format!("http://{address}"),
            seed: 3,
        },
        ENVELOPE_KEY,
        Arc::new(ManualClock::new(t(0))),
        Arc::new(RecordingSleeper::new()),
    )
}

/// One probe of the node as `serve` in `main.rs` runs it, with both the
/// journal store and the centre gone: the flush, then the exchange, then
/// the pass, in that order and on one thread. Asserts that the two outages
/// are real before returning what the pass did, so a caller's "the order
/// was still sent" cannot be about a turn in which nothing was down.
///
/// The third value is how many decisions the flush tried and failed to
/// ship. A flush with nothing pending never reaches the store, so it is the
/// store itself that is asked first, and the flush is held to failing
/// exactly when it had something to send.
fn turn_with_everything_else_unreachable(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    requoter: &mut Requoter,
    (store, mirror): (&PartitionedStore, &mut StoreMirror),
    link: &mut MeshLink,
    stats: &mut PassStats,
    last_report: &WorkReport,
    now: Timestamp,
) -> Result<(WorkReport, Vec<Requote>, usize)> {
    assert!(
        store.len().is_err(),
        "the premise failed: the journal store answered, so this turn is not cut off from it"
    );
    let pending = node.cell.journal().unshipped().len();
    match node.cell.flush(mirror, now) {
        Ok(shipped) => assert_eq!(
            (pending, shipped),
            (0, 0),
            "the journal shipped although its store is unreachable"
        ),
        Err(refusal) => assert!(
            pending > 0 && refusal.message().contains("unreachable"),
            "the flush failed for some other reason than the outage: {}",
            refusal.message()
        ),
    }
    assert_eq!(
        node.cell.journal().unshipped().len(),
        pending,
        "a flush the store refused dropped decisions from the cell's backlog"
    );
    let tick = link.exchange(&mut node.cell, last_report, now);
    assert!(
        tick.poll_error.is_some() && tick.policy_poll_error.is_some(),
        "the premise failed: the centre answered, so this turn is not cut off from it: {tick:?}"
    );
    assert_ne!(
        tick.delta.as_deref(),
        Some("delivered"),
        "the premise failed: the cell's state reached the centre: {tick:?}"
    );
    let outcome = run_pass(&mut node.cell, gateway, feed, Some(requoter), stats, now)?;
    let PassOutcome::Ran {
        report,
        requotes,
        breaks,
        ..
    } = outcome
    else {
        panic!("losing the journal store and the centre stopped the node's pass: {outcome:?}");
    };
    assert!(
        breaks.is_empty(),
        "the outage reconciled as a break: {breaks:?}"
    );
    Ok((*report, requotes, pending))
}

/// EXEC-037's own chaos check, on the pieces the binary is assembled from.
///
/// The failure it prevents is an order path that only works while something
/// else does: a node that stops sending, stops cancelling or stops booking
/// fills because its journal has nowhere to go or its centre has stopped
/// answering has put a store and a regional service between itself and the
/// venue, whatever the diagram says. Two earlier tests each showed half —
/// orders with nothing else constructed at all, and a cell that stays up
/// with the centre dead while placing nothing — and neither failed the sink,
/// so the journal catching up was shown by nothing.
#[test]
fn with_the_journal_store_and_the_centre_unreachable_a_node_still_sends_cancels_and_books_fills_and_the_journal_catches_up_when_the_store_returns()
-> Result<()> {
    let (mut node, mut gateway, mut feed) =
        node_with_feed(PricingPolicy::rest_at_mid(Duration::from_secs(60))?)?;
    let mut requoter = requoter(&node)?;
    let mut stats = PassStats::default();
    let store = Arc::new(PartitionedStore::default());
    let durable: Arc<dyn KeyValueStore> = Arc::clone(&store) as Arc<dyn KeyValueStore>;
    let mut mirror = StoreMirror::open(Arc::clone(&durable), CELL, t(0))?;
    let mut link = link_to_a_dead_centre()?;

    // The premise, asserted: the store works before it is taken away, and
    // holds the part of the session that preceded the outage. Without this a
    // store that never accepted anything would pass every refusal below.
    let before_outage = node.cell.flush(&mut mirror, t(6))?;
    assert!(
        before_outage > 0,
        "the premise failed: the cell had journaled nothing to ship before the outage"
    );
    assert_eq!(batches(durable.as_ref())?.len(), 1, "the premise failed");

    store.down.store(true, Ordering::SeqCst);
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;

    // An order: sent to the venue, and resting there by the venue's record.
    let (first, _, _) = turn_with_everything_else_unreachable(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut requoter,
        (&store, &mut mirror),
        &mut link,
        &mut stats,
        &WorkReport::default(),
        t(10),
    )?;
    assert_eq!(
        first.orders.len(),
        1,
        "no order was sent with the journal store and the centre unreachable: {:?}",
        first.refusals
    );
    let resting = first.orders[0].clone();
    assert!(
        gateway.venue_holds_open(&resting.order_id),
        "the order the cell reports sending is not at the venue"
    );

    // A cancel: the touch moves away, and the stale order is withdrawn and
    // re-sent at it — a cancel and a replacement, both at the venue.
    gateway.seed_touch(&object(), Side::Buy, dec!("100.5"), dec!("1"), t(15))?;
    let (second, requotes, refused_after_the_order) = turn_with_everything_else_unreachable(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut requoter,
        (&store, &mut mirror),
        &mut link,
        &mut stats,
        &first,
        t(20),
    )?;
    assert!(
        refused_after_the_order > 0,
        "the premise failed: the turn after the order had nothing to ship, so no flush has yet \
         been refused by the outage"
    );
    let Some(Requote::Replaced { replacement, .. }) = requotes.first() else {
        panic!("no cancel was sent with everything else unreachable: {requotes:?}");
    };
    assert!(
        !gateway.venue_holds_open(&resting.order_id),
        "the venue still holds the order the cell cancelled"
    );
    assert!(
        gateway.venue_holds_open(replacement),
        "the venue does not hold the replacement"
    );

    // A fill: somebody sells through the replacement, and the next turn
    // books it under the cell's own id and agrees with the venue's ledger.
    gateway.seed_aggressor(&object(), Side::Sell, dec!("100.5"), dec!("200"), t(25))?;
    let (third, _, _) = turn_with_everything_else_unreachable(
        &mut node,
        &mut gateway,
        &mut feed,
        &mut requoter,
        (&store, &mut mirror),
        &mut link,
        &mut stats,
        &second,
        t(28),
    )?;
    assert!(
        third
            .fills
            .iter()
            .any(|fill| fill.order_id == resting.order_id),
        "the venue's fill was not booked with everything else unreachable: {:?}",
        third.fills
    );
    assert_eq!(
        node.cell.position(&venue(), &object()),
        venue_position(&gateway),
        "the cell's position and the venue's disagree after the outage's fill"
    );
    assert!(
        !node.cell.is_halted(),
        "the outage halted the cell, so the journal store or the centre sits on the order path"
    );

    // Nothing reached the store while it was down, and the cell still holds
    // everything it decided: that is the backlog the catch-up has to ship.
    let backlog = node.cell.journal().unshipped().len();
    assert!(
        backlog >= 3,
        "the premise failed: an order, a cancel and a fill left fewer than three decisions \
         waiting to ship ({backlog})"
    );
    store.down.store(false, Ordering::SeqCst);
    assert_eq!(
        batches(durable.as_ref())?.len(),
        1,
        "a batch was written to a store that was unreachable"
    );

    // The store returns. One flush ships the whole backlog, and the record
    // reads as one unbroken chain from the session's start.
    let caught_up = node.cell.flush(&mut mirror, t(30))?;
    assert_eq!(
        caught_up, backlog,
        "the flush after the store returned did not ship everything decided during the outage"
    );
    assert!(
        node.cell.journal().unshipped().is_empty(),
        "decisions are still waiting after the catch-up"
    );
    let shipped = batches(durable.as_ref())?;
    assert_eq!(
        shipped.len(),
        2,
        "the catch-up is not one batch after the first"
    );
    let mut tail = Journal::GENESIS.to_string();
    let mut entries = 0;
    for batch in &shipped {
        batch.verify_against(&tail)?;
        tail = batch.tail_digest();
        entries += batch.entries.len();
    }
    assert_eq!(
        entries,
        node.cell.journal().len(),
        "the store does not hold every decision the cell made across the outage"
    );
    Ok(())
}

#[test]
fn the_binary_reports_a_failed_flush_and_keeps_serving_rather_than_stopping_on_it() {
    // `main.rs` is a binary no test can call, so this is the narrow claim a
    // source check can honestly make: the loop that runs the pass takes the
    // flush's failure as a value and logs it, and does not propagate it. It
    // cannot prove the pass still runs afterwards; the test above proves that
    // on the same three calls in the same order.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert!(
        source.contains("if let Err(error) = cell.flush(mirror, now) {"),
        "main.rs no longer takes a failed flush as a value to report; a `?` here turns a \
         storage outage into a trading outage"
    );
    assert!(
        !source.contains("cell.flush(mirror, now)?"),
        "main.rs propagates a failed flush out of the loop that runs the pass"
    );
}

// --- OBS-021: the pass loop's stderr is sampled, not per event --------------

/// Lines the sampler lets through per window in the storm below — the figure
/// `main.rs` passes to [`PassLog::new`].
const LINES_PER_WINDOW: u32 = 5;

/// Run `passes` passes inside one nine-second span with the touch walking
/// away from a resting order on every one, so every pass mints at least one
/// requote outcome, and return `(requote outcomes, lines written)`.
///
/// The span is fixed and the pass count is the variable, because that is the
/// shape of the claim: a busier market in the same wall-clock window.
fn stderr_lines_for_a_requote_storm(passes: i64) -> Result<(usize, usize)> {
    let (mut node, mut gateway, mut feed) =
        node_with_feed(PricingPolicy::rest_at_mid(Duration::from_secs(60))?)?;
    let mut requoter = requoter(&node)?;
    let mut stats = PassStats::default();
    // A wide book, so the bid has a hundred whole units to walk through
    // before it reaches the offer.
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("301"), dec!("400"), t(1))?;
    let first = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        Some(&mut requoter),
        &mut stats,
        t(10),
    )?;
    let PassOutcome::Ran { report, .. } = &first else {
        panic!("a running node reported its first pass as halted: {first:?}");
    };
    assert_eq!(report.orders.len(), 1, "the premise is one resting order");
    assert_eq!(report.orders[0].price, dec!("200"), "resting at the mid");

    let mut log = PassLog::new(LINES_PER_WINDOW, Duration::from_secs(10))?;
    let mut sink: Vec<u8> = Vec::new();
    let mut outcomes = 0usize;
    let mut written = 0usize;
    for pass in 0..passes {
        let now = at_ms(20_000 + pass * 9_000 / passes);
        // Somebody bids one whole unit above the last bid: a hundred ticks
        // past whatever the cell has resting behind it.
        let bid = dec!("201") + Decimal::from(pass);
        gateway.seed_touch(&object(), Side::Buy, bid, dec!("1"), now)?;
        let outcome = run_pass(
            &mut node.cell,
            &mut gateway,
            &mut feed,
            Some(&mut requoter),
            &mut stats,
            now,
        );
        match &outcome {
            Ok(PassOutcome::Ran { requotes, .. }) => outcomes += requotes.len(),
            other => panic!("pass {pass} of the storm did not run: {other:?}"),
        }
        written += log.write(now, &outcome, &mut sink);
    }
    let text = String::from_utf8(sink).expect("the log is text");
    assert_eq!(
        text.lines().count(),
        written,
        "the count returned is not the count of lines that reached the sink"
    );
    assert!(
        text.lines()
            .all(|line| line.starts_with("qip-edge-node: requote: ")),
        "{text}"
    );
    Ok((outcomes, written))
}

/// OBS-021's own check: the same nine seconds, eight passes and then eighty,
/// and the stderr volume is the sample rate both times.
///
/// Before `PassLog` the loop wrote one line per requote outcome, so the
/// eighty-pass window wrote ten times the eight-pass one — log ingestion
/// proportional to how busy the market was, on the one process whose busy
/// moments are the ones an operator most needs a readable log for.
#[test]
fn the_stderr_a_node_writes_in_one_window_is_the_sample_rate_whether_it_held_eight_passes_or_eighty()
-> Result<()> {
    let (few_outcomes, few_lines) = stderr_lines_for_a_requote_storm(8)?;
    let (many_outcomes, many_lines) = stderr_lines_for_a_requote_storm(80)?;
    // The premise, first: both storms offered more lines than the sampler
    // allows, and the larger offered several times the smaller. Without this
    // an idle node would pass by writing nothing.
    assert!(
        few_outcomes > LINES_PER_WINDOW as usize,
        "the small storm minted only {few_outcomes} requote outcome(s), so the bound was never \
         reached and nothing below is about sampling"
    );
    assert!(
        many_outcomes >= few_outcomes * 5,
        "the large storm minted {many_outcomes} outcome(s) against {few_outcomes}: not a \
         meaningfully busier window"
    );
    assert_eq!(
        few_lines, LINES_PER_WINDOW as usize,
        "the small storm did not fill the window's allowance"
    );
    assert_eq!(
        many_lines, few_lines,
        "ten times the passes wrote a different number of lines: stderr volume follows message \
         count, not the sample rate"
    );
    Ok(())
}

/// The other half of the wiring: the binary writes its per-pass lines through
/// [`PassLog`] and has no per-event `eprintln!` of its own beside it.
///
/// A source scan, because `main.rs` is a binary and no test can call its
/// loop. It asserts its premise first — that the scan is reading the loop —
/// and matches the two fragments that would each mint a line per event.
#[test]
fn the_node_binary_writes_its_per_pass_lines_only_through_the_sampled_pass_log() {
    let main = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("the node's main.rs is readable");
    assert!(
        main.contains("run_pass(") && main.contains("fn serve("),
        "the scan is not reading the serve loop"
    );
    assert!(
        main.contains("pass_log.write(now, &outcome, &mut std::io::stderr())"),
        "the serve loop no longer hands its pass outcome to PassLog"
    );
    for per_event in ["requote.describe()", "reconciliation break:"] {
        assert!(
            !main.contains(per_event),
            "main.rs formats `{per_event}` itself: a per-event line beside the sampled log"
        );
    }
}

// --- OBS-018: the node's four golden signals, the pass as the unit of work --

/// A pass that runs, a pass the gateway refuses, and a pass that runs again:
/// the meter counts three, times three, counts the refused one as this
/// service's failure, and reports how much of the request allowance the last
/// one consumed.
///
/// The refused pass is asked for at an instant earlier than the last, which
/// `SimulatedGateway::advance_to` refuses before anything is released — a
/// real failure of the real pass, not a closure that returns an error.
#[test]
fn a_run_of_passes_with_a_refused_one_moves_all_four_of_the_nodes_golden_signals() -> Result<()> {
    let (mut node, mut gateway, mut feed) = node_with_feed(PricingPolicy::Marketable)?;
    let registry = Arc::clone(node.scrape_registry());
    let meter = PassMeter::new(Arc::clone(&registry), std::time::Duration::from_secs(2))?;
    let mut stats = PassStats::default();

    // The premise: no pass has run, so none of the four exists.
    let before = registry.snapshot();
    assert_eq!(before.counter_total(names::SERVICE_REQUESTS), 0);
    assert_eq!(before.counter_total(names::SERVICE_ERRORS), 0);
    assert!(
        before
            .gauge(names::SERVICE_SATURATION, &Labels::new())
            .is_none()
    );

    let mut pass_at = |now: Timestamp| {
        meter.measure(|| {
            run_pass(
                &mut node.cell,
                &mut gateway,
                &mut feed,
                None,
                &mut stats,
                now,
            )
        })
    };
    let first = pass_at(t(10))?;
    assert!(
        matches!(first, PassOutcome::Ran { .. }),
        "the premise is a pass that ran: {first:?}"
    );
    let backwards = pass_at(t(5));
    let refusal = backwards.expect_err("a pass asked for before the last one ran");
    assert!(
        refusal.message().contains("passes run forward"),
        "the pass failed for another reason: {}",
        refusal.message()
    );
    let third = pass_at(t(20))?;
    assert!(matches!(third, PassOutcome::Ran { .. }), "{third:?}");

    let after = registry.snapshot();
    assert_eq!(after.counter_total(names::SERVICE_REQUESTS), 3, "traffic");
    assert_eq!(
        after.counter(names::SERVICE_ERRORS, &labels([("class", "service")])),
        1,
        "errors: the refused pass and nothing else"
    );
    let latency = after
        .histogram(names::SERVICE_LATENCY_MS, &Labels::new())
        .expect("the latency histogram");
    assert_eq!(latency.count, 3, "latency");
    let saturation = after
        .gauge(names::SERVICE_SATURATION, &Labels::new())
        .expect("the saturation gauge");
    assert!(
        saturation > 0.0 && saturation < 1.0,
        "a pass in a test took none, or all, of a two-second allowance: {saturation}"
    );
    Ok(())
}

/// A meter with no allowance to measure against is refused at start-up, so
/// the loop cannot chart a division by zero on every pass.
#[test]
fn a_pass_meter_with_no_request_allowance_is_refused_before_the_loop_starts() -> Result<()> {
    let (node, _gateway, _feed) = node_with_feed(PricingPolicy::Marketable)?;
    let refused = PassMeter::new(
        Arc::clone(node.scrape_registry()),
        std::time::Duration::ZERO,
    )
    .expect_err("a zero allowance");
    assert!(
        refused.message().starts_with("configuration:"),
        "{}",
        refused.message()
    );
    Ok(())
}

/// The binary runs its pass inside the meter. A source scan for the same
/// reason as the log's: `main.rs` is a binary and its loop cannot be called.
#[test]
fn the_node_binary_runs_its_pass_inside_the_golden_signal_meter() {
    let main = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("the node's main.rs is readable");
    assert!(
        main.contains("fn serve(") && main.contains("run_pass("),
        "the scan is not reading the serve loop"
    );
    assert!(
        main.contains("PassMeter::new(Arc::clone(context.metrics), REQUEST_TIMEOUT)?"),
        "the meter is no longer built on the scraped registry and the request allowance"
    );
    assert!(
        main.contains("pass_meter.measure(|| {\n                        run_pass("),
        "the serve loop calls run_pass outside the meter"
    );
    assert_eq!(
        main.matches("run_pass(").count(),
        1,
        "a second call to run_pass would be a pass nothing times"
    );
}
