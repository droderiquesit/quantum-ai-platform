//! The features half of the hot path, proven against the node's own seam.
//!
//! `StrategyCompiler::new(FeatureCatalogue::new())` in `strategies.rs`
//! compiled every plan against an empty vocabulary, and `main.rs` built its
//! `FeatureEngine` with no suite registered at all. Neither emptiness ever
//! failed a test, because every fixture plan this crate had used a literal
//! condition (`Expr::Flag(true)`) that names no feature — so a plan naming a
//! real, computed feature such as `microprice` was refused at compile with
//! "not registered in the feature graph", and had it somehow been accepted
//! the engine had nothing registered to compute the value from. This suite
//! drives the committed fixture plan — which does name `microprice` — through
//! the same installer and the same pass loop the binary runs, and proves the
//! catalogue `qip_edge_node::standard_engine_and_catalogue` hands the
//! compiler is provably the suite it registers into the paired engine.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{PlanDigest, PolicyPayload, Slot};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{SystemClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::strategies::{StrategyInstaller, StrategyPlan};
use qip_edge_node::{NodeAssembly, assemble, standard_engine_and_catalogue};
use qip_execution_engine::order::Side;
use qip_feature_dag::definition::ValueKind;
use qip_feature_dag::features::Microprice;
use qip_feature_dag::state::DEFAULT_MAX_STALENESS;
use qip_strategy::ir::Type;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CELL: &str = "features-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "microprice-guard";
const KEY: &[u8] = b"features-test-envelope-key";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

/// Where the committed fixture lives, read from disk rather than rebuilt in
/// memory: what is under test is that *this reviewed file* deploys, not a
/// copy the test happens to construct the same way.
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/slice-strategy-plan.json")
}

/// A signed grant for [`STRATEGY`] at [`CELL`], independent of the plan.
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

/// A verified payload naming the plan by digest and strategy count, the way
/// the mesh's downlink verifies one. No `capital_grants` slot: what is under
/// test is whether the strategy compiles and raises a signal, which — per
/// `Cell::work` — happens before any capital gate is consulted.
fn policy(
    sequence: u64,
    issued_at: Timestamp,
    digest: &str,
    strategies: u64,
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    payload.compiled_plan = Slot::produced(
        PlanDigest {
            digest: digest.to_string(),
            strategies,
        },
        issued_at,
    );
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, issued_at)
}

/// A node assembled with the standard suite registered for `subject` —
/// [`standard_engine_and_catalogue`]'s engine half — so the cell can actually
/// compute what a strategy naming that suite's features reads.
fn node_with_feed(subject: &ObjectId) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let (engine, _catalogue) = standard_engine_and_catalogue(subject, DEFAULT_MAX_STALENESS)?;
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, engine, Arc::new(SystemClock), allocation, None)?;
    let gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    Ok((node, gateway, feed))
}

#[test]
fn a_strategy_reading_microprice_compiles_and_fires_in_the_assembled_node() -> Result<()> {
    let bytes = fs::read(fixture_path())
        .unwrap_or_else(|error| panic!("the committed fixture plan is unreadable: {error}"));
    let plan = StrategyPlan::from_bytes(&bytes)?;

    // Premise: the fixture actually names one strategy whose rule reads
    // microprice. If it did not, every assertion below would hold of a plan
    // that never touched the bug this suite exists to catch.
    assert_eq!(
        plan.strategies.len(),
        1,
        "the fixture should name exactly one strategy"
    );
    let spec = plan.strategies[0].clone();
    assert_eq!(spec.id, StrategyId::new(STRATEGY));
    let microprice_key = Microprice::key(&spec.subject);
    assert!(
        spec.rules
            .iter()
            .any(|rule| rule.condition.features().contains(&microprice_key)),
        "the fixture's rule does not read microprice; this test would prove nothing about the \
         defect it targets"
    );

    let (mut node, mut gateway, mut feed) = node_with_feed(&spec.subject)?;
    let digest = StrategyPlan::digest_of(&bytes);
    let mut installer =
        StrategyInstaller::new(Some(fixture_path()), Some(PricingPolicy::Marketable));
    installer.offer(grant()?)?;
    node.cell.apply_policy(
        policy(1, t(10), &digest, plan.strategies.len() as u64)?,
        t(10),
    )?;

    let outcome = installer.install(&mut node.cell, t(10));
    assert_eq!(
        outcome.deployed,
        vec![STRATEGY.to_string()],
        "the fixture plan's strategy was not deployed: {}",
        outcome.describe()
    );
    assert!(outcome.refused.is_empty(), "{}", outcome.describe());

    // Premise: before any book exists, microprice is undefined and the
    // strategy raises nothing — so the signal the next pass raises is
    // provably caused by the book, not a strategy that fires regardless.
    let mut stats = PassStats::default();
    let quiet = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(11),
    )?;
    let PassOutcome::Ran { report: quiet, .. } = quiet else {
        panic!("a running node reported its pass as halted");
    };
    assert!(
        quiet.signals.is_empty(),
        "the premise is a cell that raises nothing before it has ever seen a book: {:?}",
        quiet.signals
    );

    gateway.seed_touch(&spec.subject, Side::Buy, dec!("99"), dec!("500"), t(12))?;
    gateway.seed_touch(&spec.subject, Side::Sell, dec!("101"), dec!("400"), t(12))?;

    let fired = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut stats,
        t(13),
    )?;
    let PassOutcome::Ran { report: fired, .. } = fired else {
        panic!("a running node reported its pass as halted");
    };
    assert_eq!(
        fired.signals.len(),
        1,
        "a strategy naming microprice, deployed against a book that now has one, should fire \
         exactly once: {:?}",
        fired.signals
    );
    let signal = &fired.signals[0];
    assert_eq!(signal.strategy, StrategyId::new(STRATEGY));
    assert!(
        signal
            .inputs
            .iter()
            .any(|(name, _)| *name == microprice_key.canonical()),
        "the fired signal does not carry the microprice revision it should have been computed \
         from: {:?}",
        signal.inputs
    );
    Ok(())
}

/// The type [`ValueKind`] declares mapped to the type [`FeatureCatalogue`]
/// checks a strategy against — written independently of
/// `standard_engine_and_catalogue`'s own mapping, so this test does not pass
/// merely because it shares a typo with the code it checks.
fn expected_type(kind: ValueKind) -> Type {
    match kind {
        ValueKind::Exact => Type::Exact,
        ValueKind::Statistic => Type::Statistic,
        ValueKind::Count => Type::Count,
        ValueKind::Flag => Type::Flag,
    }
}

#[test]
fn the_catalogue_the_compiler_sees_is_the_suite_the_engine_computes() -> Result<()> {
    let subject = ObjectId::from_string("obj-SLICE13-CATALOGUE");
    let (engine, catalogue) = standard_engine_and_catalogue(&subject, DEFAULT_MAX_STALENESS)?;

    let catalogue_keys: BTreeSet<String> =
        catalogue.keys().iter().map(|key| key.canonical()).collect();
    // Premise: the pair is not two empty vocabularies agreeing vacuously —
    // an empty catalogue would trivially equal an empty engine and this test
    // would guard nothing.
    assert!(
        !catalogue_keys.is_empty(),
        "the premise is a non-empty catalogue for a real instrument"
    );

    let engine_keys: BTreeSet<String> = engine
        .graph()
        .keys()
        .into_iter()
        .map(|key| key.canonical())
        .collect();
    assert_eq!(
        catalogue_keys, engine_keys,
        "the catalogue the compiler checks a strategy against names a different vocabulary than \
         the engine that would actually compute it"
    );

    for key in catalogue.keys() {
        let declared = catalogue.type_of(key).unwrap_or_else(|| {
            panic!("{} came from the catalogue's own key list", key.canonical())
        });
        let computed = engine.graph().value_kind(key).unwrap_or_else(|| {
            panic!(
                "{} is registered in the engine by the same loop",
                key.canonical()
            )
        });
        assert_eq!(
            declared,
            expected_type(computed),
            "feature {} is declared {declared} in the catalogue and computes {computed:?} in the \
             engine",
            key.canonical()
        );
    }
    Ok(())
}
