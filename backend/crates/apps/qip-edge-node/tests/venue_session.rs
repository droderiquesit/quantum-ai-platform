//! The simulated venue's session, kept answering by the pass.
//!
//! Until 2026-11-23 the node heartbeated its simulated venue once, when
//! `NodeGateway::open` built it, and never again. The venue allows thirty
//! seconds between heartbeats, so from thirty seconds after start-up
//! `ConnectionState::ready_ticket` refused every placement and every held
//! release ("has not answered a heartbeat for …, longer than the 30s the
//! session allows") for the life of the process, and every pass-loop suite
//! in this crate kept its timeline inside the window rather than meet it.
//! `SimulatedGateway::advance_to` now heartbeats the session on every pass,
//! through the venue's own heartbeat.
//!
//! The gateway here is opened as `main.rs` opens it — `NodeGateway::open`
//! on the simulated choice at the instant the node starts, narrowed with
//! `simulated_for_feed` — and the passes are `run_pass`, the function
//! `DecisionLoop::turn` runs. One difference from the binary, named so
//! nobody reads past it: `main.rs` installs no listing permissions, so a
//! deployed node lists nothing at its simulated venue; these tests record a
//! register so that there is a book to place against.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::BookSide;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, SystemClock, dec};
use qip_edge::cell::{CellConfig, GATE_RELEASE_REFUSED, Placer, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_edge::telemetry::EDGE_VENUE_ACK_LATENCY_MILLIS;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::{NodeGateway, SimulatedGateway};
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::venue::VenueChoice;
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Labels, labels, names};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use std::collections::BTreeSet;
use std::sync::Arc;

const CELL: &str = "session-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const ENVELOPE_KEY: &[u8] = b"venue-session-test-key";
/// The seed `VenueChoice::Simulated` carries.
const SEED: u64 = 7;
/// The order the tests hold at the venue for a later release.
const HELD: &str = "held-at-venue";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-SESSION")
}

fn at_venue() -> Labels {
    labels([("cell", CELL), ("region", REGION), ("venue", VENUE)])
}

/// The heartbeat allowance the gateway's venue is built with, read from the
/// settings `SimulatedGateway::new` uses rather than written here.
fn allowance() -> Duration {
    SimulatedGateway::default_settings().heartbeat_interval
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
    // Bounded as the centre's issuer bounds every grant (R12-CAPITAL-4088).
    let unsigned = build("")?;
    let capital = unsigned.gross_limit();
    let unsigned = unsigned.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        2_500,
        Decimal::ONE,
        capital,
    )?)?;
    let loss_limit = unsigned.loss_limit();
    let unsigned = unsigned.with_drawdown(qip_contracts::DrawdownBounds::bounded(
        loss_limit, None, None,
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

/// The gateway's register: create, list and seed the one instrument.
fn permit(gateway: &mut SimulatedGateway) -> Result<()> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for action in [
        CreationAction::Create,
        CreationAction::List,
        CreationAction::Seed,
    ] {
        let request = CreationRequest::new(action, venue.as_str(), object().as_str(), "paper");
        for authority in CreationAuthority::ALL {
            permissions.record(authority, &request, "test fixture")?;
        }
    }
    gateway.permit_listings(permissions, "paper")
}

/// The node, its gateway opened as `main.rs` opens it at `t(0)`, and the
/// simulated feed; one strategy that fires on every pass, sent marketable,
/// funded by the share the centre would ship at `t(5)`.
fn node() -> Result<(NodeAssembly, NodeGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = NodeGateway::open(&VenueChoice::Simulated { seed: SEED }, venue(), t(0))?;
    permit(gateway.simulated_for_feed()?)?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;

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
    node.cell.deploy_with_pricing(
        compiled,
        compiler.into_program(),
        grant()?,
        PricingPolicy::Marketable,
    )?;
    let named = grant()?.signature().to_string();
    node.cell.apply_policy(share_policy(vec![named])?, t(5))?;
    Ok((node, gateway, feed))
}

/// Every reason the venue gave for refusing an order the cell sent.
fn rejected(node: &NodeAssembly) -> Vec<String> {
    node.cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::OrderRejected { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

/// Every reason the cell journaled for a held order the gateway refused at
/// its release.
fn refused_at_release(node: &NodeAssembly) -> Vec<String> {
    node.cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::Refused { gate, reason } if gate == GATE_RELEASE_REFUSED => {
                Some(reason.clone())
            }
            _ => None,
        })
        .collect()
}

/// Hold a buy of one at 99 at the venue for release at `release`: below the
/// ask, so once released it rests rather than fills, and the venue's own book
/// is the witness that it arrived.
fn hold(gateway: &mut SimulatedGateway, release: Timestamp) -> Result<()> {
    gateway.place(
        HELD,
        &object(),
        &venue(),
        BookSide::Ask,
        dec!("1"),
        dec!("99"),
        release,
    )?;
    assert_eq!(gateway.held_count(), 1, "premise: the order is held");
    Ok(())
}

/// R12-OBS-9145 and R12-EXEC-8068 on a node running as long as a deployed
/// one does: passes one second apart, as `main.rs` turns, from `t(10)` to
/// `t(130)` — four times the venue's allowance — each sending a marketable
/// order, and an order held at the venue for release at `t(100)`.
///
/// Mutation: delete the heartbeat call in `SimulatedGateway::advance_to`
/// and the pass at `t(31)` fails, "XLON is degraded and cannot accept an
/// order".
#[test]
fn a_node_turning_long_past_the_venues_heartbeat_allowance_still_places_and_releases() -> Result<()>
{
    assert_eq!(
        allowance(),
        Duration::from_secs(30),
        "premise: the timeline below is written against a thirty-second allowance"
    );
    let (mut node, mut node_gateway, mut feed) = node()?;
    let gateway = node_gateway.simulated_for_feed()?;
    // Depth for every pass's order of ten, so no pass refuses for want of a
    // book and each one that sends reaches the venue.
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("5000"), t(1))?;
    hold(gateway, t(100))?;

    let mut stats = PassStats::default();
    let mut sent_on: BTreeSet<i64> = BTreeSet::new();
    for second in 10..=130 {
        let before = gateway.submitted_count();
        let outcome = run_pass(
            &mut node.cell,
            gateway,
            &mut feed,
            None,
            &mut stats,
            t(second),
        )
        .unwrap_or_else(|error| panic!("the pass at t({second}) failed: {}", error.message()));
        let PassOutcome::Ran { breaks, .. } = outcome else {
            panic!("the pass at t({second}) found the cell halted: {outcome:?}");
        };
        assert!(breaks.is_empty(), "t({second}): {breaks:?}");
        if gateway.submitted_count() > before {
            sent_on.insert(second);
        }
    }

    // The premise, then the property: the first pass is inside the window
    // the session had before this change, and is not what is under test.
    assert!(
        sent_on.contains(&10),
        "premise: the first pass sent an order"
    );
    for second in [31, 45, 75, 120, 130] {
        assert!(
            sent_on.contains(&second),
            "the pass at t({second}), {second} s after the venue session started, sent \
             nothing to the venue; sent on {sent_on:?}"
        );
    }
    assert_eq!(
        rejected(&node),
        Vec::<String>::new(),
        "the venue refused an order the cell sent"
    );

    // The held order: released on the pass at t(100), seventy seconds past
    // the allowance, and resting at the venue.
    assert_eq!(gateway.held_count(), 0, "the held order was never released");
    assert!(
        refused_at_release(&node).is_empty(),
        "the held order was refused at its release: {:?}",
        refused_at_release(&node)
    );
    assert!(
        gateway.venue_holds_open(HELD),
        "the released order is not resting at the venue"
    );
    Ok(())
}

/// The heartbeat must not paper over a venue that stopped answering. The
/// venue is silenced after the pass at `t(10)`; it is still taken at its
/// word at `t(20)`, inside the allowance, and from `t(41)` — thirty-one
/// seconds after its last answer — the cell's order is refused naming the
/// gap, and so is the release of an order held for `t(45)`.
///
/// Mutation: in `SimulatedGateway::heartbeat`, call
/// `self.exchange.start_heartbeat()` before heartbeating, so the pass
/// revives the venue it should find silent, and the pass at `t(41)` sends.
#[test]
fn a_venue_that_stops_answering_still_degrades_and_refuses_orders_and_releases() -> Result<()> {
    let (mut node, mut node_gateway, mut feed) = node()?;
    let gateway = node_gateway.simulated_for_feed()?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("5000"), t(1))?;
    let mut stats = PassStats::default();

    run_pass(&mut node.cell, gateway, &mut feed, None, &mut stats, t(10))?;
    assert_eq!(
        gateway.submitted_count(),
        1,
        "premise: an answering venue took the first pass's order"
    );
    hold(gateway, t(45))?;
    gateway.stop_heartbeat();

    run_pass(&mut node.cell, gateway, &mut feed, None, &mut stats, t(20))?;
    assert_eq!(
        gateway.submitted_count(),
        2,
        "premise: ten seconds of silence is inside the allowance, so the venue still took an order"
    );
    assert!(rejected(&node).is_empty(), "premise: nothing refused yet");

    // The pass's own result is not asserted: an order the venue refuses
    // leaves the pass as an error, and what matters is what reached the
    // venue and what the cell journaled.
    let refusal = run_pass(&mut node.cell, gateway, &mut feed, None, &mut stats, t(41));
    assert_eq!(
        gateway.submitted_count(),
        2,
        "a venue silent for thirty-one seconds took an order: {refusal:?}"
    );
    let reasons = rejected(&node);
    assert_eq!(reasons.len(), 1, "the cell journaled {reasons:?}");
    assert!(
        reasons[0].contains("XLON is degraded and cannot accept an order")
            && reasons[0].contains("no heartbeat for 31.000s"),
        "the refusal does not name the silence: {}",
        reasons[0]
    );

    let _ = run_pass(&mut node.cell, gateway, &mut feed, None, &mut stats, t(45));
    assert_eq!(gateway.held_count(), 0, "the held order is still held");
    assert!(
        !gateway.venue_holds_open(HELD),
        "a silent venue took the held order"
    );
    let refused = refused_at_release(&node);
    assert_eq!(refused.len(), 1, "the release refusal: {refused:?}");
    assert!(
        refused[0].contains(HELD) && refused[0].contains("is degraded and cannot accept an order"),
        "the release refusal does not name the silence: {}",
        refused[0]
    );
    Ok(())
}

/// R12-OBS-9145 and R12-OBS-9159 rest on the cell's per-order
/// acknowledgement latency. Sixty-one passes heartbeat the venue sixty-one
/// times while the cell sends a handful of orders, some of them past the
/// allowance, and none of the heartbeats is an order: the venue's own count
/// of orders it took, the cell's count of orders it placed and the
/// acknowledgement series agree, each sample the venue's four-millisecond
/// round trip.
///
/// The venue's `VenueObservation` tally is a different count and is not
/// asserted here: it counts every answer, heartbeats included, as its
/// documentation and `qip-acceptance`'s `decentralised_venue.rs` state, and
/// nothing in this node reads it.
///
/// Mutation: in `qip-brokers`' `SimulatedExchange::heartbeat`, add one to
/// `submitted`, the venue's count of orders it took, and the venue's count
/// reads sixty-two more than the cell placed.
#[test]
fn heartbeats_on_every_pass_are_not_counted_as_orders_the_venue_acknowledged() -> Result<()> {
    let (mut node, mut node_gateway, mut feed) = node()?;
    let gateway = node_gateway.simulated_for_feed()?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    let mut stats = PassStats::default();
    let mut passes = 0u32;
    let mut inside_allowance = 0;
    for second in 10..=70 {
        // Ten offered at three instants only, so most passes have no ask to
        // take and send nothing: a few orders against sixty-one heartbeats,
        // and one more from bring-up.
        if [10, 40, 65].contains(&second) {
            gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("10"), t(second))?;
        }
        run_pass(
            &mut node.cell,
            gateway,
            &mut feed,
            None,
            &mut stats,
            t(second),
        )?;
        passes += 1;
        if second == 30 {
            inside_allowance = gateway.submitted_count();
        }
    }
    assert_eq!(
        passes, 61,
        "premise: sixty-one passes, so sixty-one heartbeats"
    );

    let snapshot = node.scrape_registry().snapshot();
    let placed = snapshot.counter(names::EDGE_ORDERS_PLACED, &at_venue());
    assert!(
        placed >= 2 && placed < u64::from(passes),
        "premise: the cell placed a few orders, fewer than the passes that heartbeated; it \
         placed {placed}"
    );
    assert!(
        gateway.submitted_count() > inside_allowance,
        "premise: an order reached the venue after the allowance, so the heartbeats were \
         answered"
    );
    assert!(
        stats.fills > 0 && gateway.working_count() == 0,
        "premise: every order the venue took was answered with its fill, none left resting \
         unanswered; {} fills, {} resting",
        stats.fills,
        gateway.working_count()
    );
    assert_eq!(
        gateway.submitted_count(),
        placed,
        "the venue counted {} orders taken for {placed} placed",
        gateway.submitted_count()
    );
    let ack = snapshot
        .histogram(EDGE_VENUE_ACK_LATENCY_MILLIS, &at_venue())
        .expect("the venue answered orders and no acknowledgement latency was recorded");
    assert_eq!(
        ack.count, placed,
        "{} acknowledgement latencies recorded for {placed} orders",
        ack.count
    );
    let round_trip = SimulatedGateway::default_settings().latency;
    #[allow(clippy::cast_precision_loss)]
    let millis = round_trip.as_nanos() as f64 / 1_000_000.0;
    assert!(
        (ack.min - millis).abs() < f64::EPSILON && (ack.max - millis).abs() < f64::EPSILON,
        "the acknowledgement latency ran {} to {} ms against the venue's {millis} ms round trip",
        ack.min,
        ack.max
    );
    Ok(())
}

/// ADR 0084 §3 and the replay: a heartbeat draws nothing from the venue's
/// seed, so a venue that answered a hundred heartbeats before an order
/// refuses exactly the orders one that answered a single heartbeat does.
/// Were it otherwise, the rejections a replay reproduces would depend on how
/// many passes ran before them rather than on the orders.
///
/// Mutation: build `SimulatedGateway::with_rejection_probability` on
/// `ExchangeSettings::default()` (a three-millisecond jitter) rather than
/// `orderly()`, so each heartbeat's round trip is drawn from the seed, and
/// the two venues refuse different orders.
#[test]
fn a_heartbeat_draws_nothing_from_the_seed_so_rejections_do_not_depend_on_how_many_passes_ran()
-> Result<()> {
    let refused_after = |heartbeats: i64| -> Result<Vec<String>> {
        let mut gateway = SimulatedGateway::with_rejection_probability(venue(), 11, 0.5, t(0))?;
        permit(&mut gateway)?;
        gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("100000"), t(0))?;
        for second in (100 - heartbeats + 1)..=100 {
            gateway.advance_to(t(second))?;
        }
        let mut refused = Vec::new();
        for index in 0..40 {
            let order_id = format!("order-{index}");
            if gateway
                .place(
                    &order_id,
                    &object(),
                    &venue(),
                    BookSide::Ask,
                    dec!("1"),
                    dec!("101"),
                    t(100),
                )
                .is_err()
            {
                refused.push(order_id);
            }
        }
        Ok(refused)
    };
    let quiet = refused_after(1)?;
    let busy = refused_after(100)?;
    assert!(
        !quiet.is_empty() && quiet.len() < 40,
        "premise: an even coin refuses some of forty orders and not all; it refused {}",
        quiet.len()
    );
    assert_eq!(
        busy, quiet,
        "a hundred heartbeats changed which orders the venue refused"
    );
    Ok(())
}
