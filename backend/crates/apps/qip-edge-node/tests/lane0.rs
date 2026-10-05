//! Every Lane 0 function, in one process, on one venue event (REFLEX-004).
//!
//! `pass.rs` proves the node reaches `Cell::work`, and it does so with a
//! strategy whose rule is `Flag(true)`: no feature is read and no model is
//! evaluated, so two of the stages the blueprint places inside the node were
//! never on the path any node test drove. They were not on the path a
//! deployed node drove either. `main.rs` builds its feature engine empty,
//! because it cannot know at start-up which instruments a plan will name,
//! and nothing registered into it afterwards — a plan reading a computed
//! feature compiled against the installer's catalogue, deployed, and then
//! read a value nothing computed, on every pass, without a refusal anywhere.
//!
//! This suite assembles the node as `main.rs` does — the engine empty — and
//! hands it its package the way the mesh does: a plan file named by digest,
//! a grant, and one signed payload naming the plan, the promoted model and
//! the grant. Then it moves the simulated venue's book and follows the event
//! through decode, sequence, book, features, the inline model, the strategy
//! predicate, the feasibility gate, netting, venue selection, the order and
//! the venue adapter, to a paper fill. Nothing here opens a socket; the
//! architecture suite's
//! `every_lane_0_stage_is_linked_into_the_node_and_none_is_reached_through_a_transport`
//! is what holds that true of the crates rather than of this test.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, ModelManifest, PlanDigest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{Duration, SystemClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::strategies::{StrategyInstaller, StrategyPlan};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::features::BookPressure;
use qip_feature_dag::state::MarketState;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::model::DistilledModel;
use qip_strategy::program::Op;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

const CELL: &str = "lane0-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "pressure-entry";
const MODEL: &str = "lane0-pressure";
const KEY: &[u8] = b"lane0-test-key";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-LANE0")
}

/// The distilled model the plan carries inline: book pressure less 0.3.
///
/// Positive only when the bid side holds well over half the visible depth,
/// so the same deployed strategy is quiet on one book and enters on
/// another, and the only thing that differs between the two is what this
/// model returned over a feature the engine computed.
fn model() -> Result<DistilledModel> {
    DistilledModel::linear(MODEL, -0.3, vec![1.0])
}

/// Enter ten when the model, read over five-level book pressure, is positive.
fn spec() -> Result<StrategySpec> {
    let pressure = BookPressure::key(&object(), 5);
    let condition = Expr::Model {
        model: model()?,
        inputs: vec![Expr::feature(pressure)],
    }
    .greater_than(Expr::Statistic(0.0));
    Ok(
        StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30)).with_rule(
            Rule::new(
                "pressure",
                SignalKind::Enter,
                condition,
                Expr::Exact(dec!("10")),
                Expr::Statistic(0.6),
                10,
            ),
        ),
    )
}

/// The plan as a file on the node, and its digest.
fn plan_file() -> Result<(PathBuf, String)> {
    let dir = std::env::temp_dir().join(format!("qip-edge-node-lane0-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    let bytes = serde_json::to_vec(&serde_json::json!({ "strategies": [spec()?] }))
        .expect("a plan serialises");
    let path = dir.join("plan.json");
    fs::write(&path, &bytes).expect("the plan is written");
    Ok((path, StrategyPlan::digest_of(&bytes)))
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
    let signed = build(&sign_payload(KEY, &unsigned.signing_payload()))?;
    VerifiedEnvelope::verify(signed, KEY, CELL, t(1))
}

/// The package's signed half: the plan by digest, the model the centre
/// promoted at its digest, and the grant the region's share is drawn from.
fn package(plan_digest: &str, grant_name: &str) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.compiled_plan = Slot::produced(
        PlanDigest {
            digest: plan_digest.to_string(),
            strategies: 1,
        },
        t(5),
    );
    payload.trained_models = Slot::produced(
        ModelManifest {
            models: BTreeMap::from([(MODEL.to_string(), model()?.digest())]),
        },
        t(5),
    );
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![grant_name.to_string()],
        },
        t(5),
    );
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, t(5))
}

fn pass(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    stats: &mut PassStats,
    now: Timestamp,
) -> Result<WorkReport> {
    match run_pass(&mut node.cell, gateway, feed, None, stats, now)? {
        PassOutcome::Ran { report, breaks, .. } => {
            assert!(
                breaks.is_empty(),
                "the pass reconciled as a break: {breaks:?}"
            );
            Ok(*report)
        }
        halted @ PassOutcome::Halted { .. } => {
            panic!("a running node reported its pass as halted: {halted:?}")
        }
    }
}

/// The kinds the cell journaled at one instant, in the order it sealed them.
fn journaled_at(node: &NodeAssembly, at: Timestamp) -> Vec<&'static str> {
    node.cell
        .journal()
        .entries()
        .iter()
        .filter(|entry| entry.at == at)
        .map(|entry| entry.decision.kind())
        .collect()
}

#[test]
fn one_node_process_takes_a_venue_event_through_every_lane_0_stage_to_a_paper_order() -> Result<()>
{
    // The node as `main.rs` assembles it: an engine nothing is registered in.
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let mut feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let pressure = BookPressure::key(&object(), 5);
    assert!(
        node.cell.features().graph().is_empty(),
        "the premise is the engine every node starts with: empty"
    );

    // Premise: the plan really carries the model inline. Over a literal
    // input the compiler folds a model to a constant, and a test of "reflex
    // inference" would then be a test of a literal.
    let (compiled, program) = StrategyInstaller::compile(&spec()?)?;
    assert!(
        program
            .reachable_from(compiled.plan())
            .iter()
            .filter_map(|node| program.node(*node))
            .any(|node| matches!(&node.op, Op::Model { .. })),
        "the premise failed: the compiled plan carries no model"
    );

    // The package, the way the mesh hands it over.
    let (path, digest) = plan_file()?;
    let grant = grant()?;
    let grant_name = grant.signature().to_string();
    let mut installer = StrategyInstaller::new(Some(path), Some(PricingPolicy::Marketable));
    installer.offer(grant)?;
    node.cell
        .apply_policy(package(&digest, &grant_name)?, t(5))?;
    let installed = installer.install(&mut node.cell, t(5));
    assert_eq!(
        installed.deployed,
        vec![STRATEGY.to_string()],
        "the package's strategy was not deployed: {}",
        installed.describe()
    );
    assert!(
        node.cell.features().graph().is_defined(&pressure),
        "the strategy was deployed and the feature it reads is still registered nowhere"
    );

    let mut stats = PassStats::default();

    // No book: the feature is undefined, the model has nothing to read.
    let empty = pass(&mut node, &mut gateway, &mut feed, &mut stats, t(10))?;
    assert!(empty.signals.is_empty(), "{:?}", empty.signals);

    // A book the model says no to: 500 bid against 400 offered is a
    // pressure of about 0.11, and 0.11 - 0.3 is negative.
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(11))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(11))?;
    let declined = pass(&mut node, &mut gateway, &mut feed, &mut stats, t(12))?;
    assert!(
        node.cell
            .features()
            .value(&pressure)
            .is_some_and(|value| value.is_defined()),
        "the premise is a book the engine computed the feature from"
    );
    assert!(
        declined.signals.is_empty(),
        "the premise is a book the model declines; a strategy that fires on any book proves \
         nothing about the model: {:?}",
        declined.signals
    );
    assert_eq!(
        gateway.submitted_count(),
        0,
        "the premise is a venue that has been sent nothing"
    );

    // The venue event: a thousand more bid at 99. Pressure is now about
    // 0.58, the model is positive, and nothing else about the node changed.
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("1000"), t(13))?;
    let entered = pass(&mut node, &mut gateway, &mut feed, &mut stats, t(14))?;

    // Features and inference: the signal exists, and names the revision of
    // the feature the model read.
    assert_eq!(
        entered.signals.len(),
        1,
        "the event did not reach a signal: refusals {:?}",
        entered.refusals
    );
    assert!(
        entered.signals[0]
            .inputs
            .iter()
            .any(|(name, _)| *name == pressure.canonical()),
        "the signal does not carry the feature the model read: {:?}",
        entered.signals[0].inputs
    );

    // Risk, netting, routing, order generation and the venue adapter: one
    // paper order, at the simulated venue, filled against the ask the feed
    // published.
    assert_eq!(
        entered.orders.len(),
        1,
        "the signal did not reach an order: refusals {:?}",
        entered.refusals
    );
    let order = &entered.orders[0];
    assert_eq!(order.venue, venue());
    assert!(order.simulated, "the order was not a paper order");
    // Local risk, visibly: the package carries no causal graph and no belief
    // state, so the cell narrows what the strategy asked for. The order is
    // the strategy's ten at the cell's own sizing multiplier, not the ten.
    let multiplier = node.cell.narrowing(t(14)).sizing_multiplier();
    assert!(
        multiplier.is_positive() && multiplier < dec!("1"),
        "the premise is a package the cell narrows under: {multiplier}"
    );
    let sized = dec!("10") * multiplier;
    assert_eq!(
        order.quantity, sized,
        "the order was not sized by the cell's own narrowing"
    );
    assert_eq!(
        gateway.submitted_count(),
        1,
        "the venue adapter saw no order"
    );
    assert_eq!(
        node.cell.position(&venue(), &object()),
        sized,
        "the venue's fill was not booked by the cell"
    );

    // And the journal holds the pass in the order the stages ran: the frame
    // decoded, the signal, the venue chosen, the order sent, the fill.
    let kinds = journaled_at(&node, t(14));
    let position = |kind: &str| {
        kinds
            .iter()
            .position(|recorded| *recorded == kind)
            .unwrap_or_else(|| panic!("the pass journaled no `{kind}`: {kinds:?}"))
    };
    assert!(
        position("ingested") < position("signal_raised")
            && position("signal_raised") < position("venue_chosen")
            && position("venue_chosen") < position("order_sent")
            && position("order_sent") < position("filled"),
        "the stages were journaled out of order: {kinds:?}"
    );
    Ok(())
}
