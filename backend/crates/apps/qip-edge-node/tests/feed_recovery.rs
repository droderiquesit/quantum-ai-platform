//! The node's half of what a cell does about a venue it can no longer use.
//!
//! `qip-edge`'s `venue_failure` suite proves the cell refuses orders for a
//! silent feed, discards a book behind a sequence gap, asks for a snapshot
//! and quarantines a venue that rejects orders. None of that reaches a
//! deployed process unless this binary's own pieces do their part: the
//! simulated feed has to say it is alive when nothing moved, it has to
//! answer the cell's snapshot request, and `assemble` has to arm the
//! quarantine. Each test here drives the assembled node through `run_pass`
//! and breaks if the wiring is removed.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{SystemClock, dec};
use qip_edge::cell::{CellConfig, GATE_SILENT_FEED, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::{FeedTick, SimulatedFeed};
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_routing::health::HealthPolicy;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const ENVELOPE_KEY: &[u8] = b"feed-recovery-test-envelope-key";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
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

/// The node as `main.rs` assembles it — one registry, the simulated gateway,
/// the simulated feed attached — with one always-firing marketable strategy
/// and the share the centre would ship applied at `t(5)`.
fn node() -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let gateway = SimulatedGateway::new(venue(), 7, t(0))?;
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

    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![grant()?.signature().to_string()],
        },
        t(5),
    );
    node.cell.apply_policy(
        VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, CELL, t(5))?,
        t(5),
    )?;
    Ok((node, gateway, feed))
}

fn pass(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    now: Timestamp,
) -> Result<(FeedTick, Box<WorkReport>)> {
    let mut stats = PassStats::default();
    match run_pass(&mut node.cell, gateway, feed, None, &mut stats, now)? {
        PassOutcome::Ran { feed, report, .. } => Ok((feed, report)),
        halted @ PassOutcome::Halted { .. } => panic!("the node is halted: {halted:?}"),
    }
}

fn refused_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(recorded, _)| recorded == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

#[test]
fn a_simulated_venue_that_did_not_move_is_still_heard_so_a_quiet_market_is_not_refused_as_a_lost_feed()
-> Result<()> {
    // The failure this prevents: the simulated feed publishes differences,
    // so a venue whose book did not change publishes nothing — exactly what
    // a dead feed publishes. A cell that judged silence from bytes alone
    // would refuse every order at a quiet venue after five seconds.
    let (mut node, mut gateway, mut feed) = node()?;
    // A bid and no offer: the strategy's marketable buy has nothing to take,
    // so no order goes out and the venue's book does not move between passes.
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;

    let (tick, report) = pass(&mut node, &mut gateway, &mut feed, t(10))?;
    assert_eq!(
        tick.messages, 1,
        "the premise failed: the first pass did not publish the bid"
    );
    assert!(report.orders.is_empty(), "{:?}", report.orders);

    // A minute later, far past the cell's five-second limit, and the feed
    // has nothing to publish.
    let (tick, report) = pass(&mut node, &mut gateway, &mut feed, t(70))?;
    assert_eq!(
        tick.messages, 0,
        "the premise failed: something moved, so this pass proves nothing about a quiet feed"
    );
    assert!(
        refused_under(&report, GATE_SILENT_FEED).is_empty(),
        "a quiet venue the node had just read was refused as a silent feed: {:?}",
        report.refusals
    );
    // And the pass did reach the routing gates — it refused for the reason
    // that is actually true, the missing offer — so the absence above is
    // not a pass that never routed.
    assert_eq!(
        refused_under(&report, "pricing").len(),
        1,
        "{:?}",
        report.refusals
    );
    Ok(())
}

#[test]
fn a_book_the_cell_discarded_is_rebuilt_from_the_venues_own_depth_on_the_next_pass_and_the_node_trades_again()
-> Result<()> {
    // The failure this prevents: a cell that discards a book raises a
    // standing snapshot request, and a feed that never reads it leaves the
    // venue unusable for the life of the process. This feed publishes
    // differences against what it remembers publishing, so without an
    // answer the discarded book would never be refilled at all.
    let (mut node, mut gateway, mut feed) = node()?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;

    // Premise: the node trades, and an ordinary pass rebuilds nothing.
    let (tick, report) = pass(&mut node, &mut gateway, &mut feed, t(10))?;
    assert_eq!(report.orders.len(), 1, "{:?}", report.refusals);
    assert_eq!(tick.resynchronised, 0);

    // The book is discarded, as an abandoned gap would leave it. The
    // in-process feed cannot lose a frame, so the state is installed
    // directly; the request it raises is the cell's own.
    let mut discarded = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    discarded.reset("a sequence gap was abandoned");
    node.cell.track(discarded);
    assert_eq!(
        node.cell.snapshot_requests().len(),
        1,
        "the premise failed: the discarded book raised no snapshot request"
    );

    let (tick, report) = pass(&mut node, &mut gateway, &mut feed, t(11))?;
    assert_eq!(
        tick.resynchronised, 1,
        "the feed did not answer the cell's snapshot request"
    );
    assert!(node.cell.snapshot_requests().is_empty());
    assert!(
        refused_under(&report, "stale_book").is_empty(),
        "{:?}",
        report.refusals
    );
    assert_eq!(
        report.orders.len(),
        1,
        "the node did not trade again on the rebuilt book: {:?}",
        report.refusals
    );
    // Rebuilt from the venue's depth as it stood: the offer the first pass
    // took ten from, not the four hundred it was seeded with.
    assert_eq!(report.orders[0].price, dec!("101"));
    let rebuilt: Vec<usize> = node
        .cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::BookResynchronised {
                venue: at,
                object: instrument,
                levels,
                ..
            } if at == VENUE && instrument == object().as_str() => Some(*levels),
            _ => None,
        })
        .collect();
    assert_eq!(
        rebuilt,
        vec![2],
        "the rebuild is not in the chain as one entry of two levels"
    );
    Ok(())
}

#[test]
fn the_assembled_node_arms_the_venue_quarantine_with_the_routing_crates_own_policy() -> Result<()> {
    // The failure this prevents: `qip_routing::health` built, tested, and
    // held by no deployed cell — the state it was in until `assemble` armed
    // it. A caller's own configuration does not arm it; the node does.
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    assert_eq!(
        config.venue_health, None,
        "the premise failed: the configuration arrived already armed"
    );
    let (node, _, _) = node()?;
    assert_eq!(
        node.cell.config().venue_health,
        Some(HealthPolicy::default()),
        "the node assembled a cell with no venue quarantine"
    );
    Ok(())
}
