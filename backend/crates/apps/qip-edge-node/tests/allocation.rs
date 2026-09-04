//! The region allocation, proven on the node's own seam.
//!
//! `qip-edge`'s `reservation.rs` proves that a cell *given* an allocation
//! refuses a second strategy once it is spent. What it cannot see is whether
//! any node is ever given one — and until this suite existed none was:
//! `Cell::with_region_allocation` was called by no composition root, so
//! every node this binary could build ran with each strategy bounded by its
//! own envelope and nothing bounding their sum. Each test here goes through
//! the same `RegionCapital::read` and `assemble` the binary goes through,
//! and asserts on what the assembled cell then does in a real pass.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, SystemClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge_node::allocation::{ALLOCATION_VARIABLE, RegionCapital};
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{labels, names};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::fs;
use std::path::Path;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const ENVELOPE_KEY: &[u8] = b"allocation-test-envelope-key";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

/// A strategy whose one rule always holds, so a pass raises exactly one
/// signal per strategy; what is under test is the bound, not the rule.
fn firing_strategy(id: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("10")),
            Expr::Statistic(0.5),
            10,
        ),
    );
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// A grant of a million for `strategy`: far more than one pass commits, so
/// nothing per-strategy is what stops a second strategy.
fn grant(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
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

/// The node's pieces, assembled the way `main.rs` assembles them — through
/// `RegionCapital::read` and `assemble` — with a two-sided book at the venue
/// and the named strategies deployed marketable under signed grants.
fn node_with(
    allocation: &str,
    strategies: &[&str],
) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let capital = RegionCapital::read(Some(allocation))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), capital)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    for id in strategies {
        let (compiled, program) = firing_strategy(id)?;
        node.cell
            .deploy_with_pricing(compiled, program, grant(id)?, PricingPolicy::Marketable)?;
    }
    Ok((node, gateway, feed))
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

/// What one pass of one strategy actually spends of the region.
///
/// Measured against an allocation far larger than a pass could need rather
/// than restated as a literal: the degradation floor scales every size, so a
/// literal here would be a number this file believed and the cell did not.
fn spent_by_one_strategy() -> Result<Decimal> {
    let opening = "100000000";
    let (mut node, mut gateway, mut feed) = node_with(opening, &["alpha"])?;
    let mut stats = PassStats::default();
    let outcome = run_pass(&mut node.cell, &mut gateway, &mut feed, &mut stats, t(10))?;
    let PassOutcome::Ran { report, .. } = outcome else {
        panic!("the probe node reported its pass as halted: {outcome:?}");
    };
    assert_eq!(
        report.orders.len(),
        1,
        "the probe premise failed: the pass placed no order: {:?}",
        report.refusals
    );
    let free = node
        .cell
        .region_allocation_free()
        .expect("a node assembled by this root holds an allocation");
    Ok(Decimal::parse(opening).expect("a decimal literal") - free)
}

#[test]
fn a_node_built_by_this_root_refuses_to_start_without_a_region_allocation() {
    // The good value first, so the refusals below are known to be refusals
    // of the value and not of everything: a gate that refuses a well-formed
    // amount is not a gate.
    let admitted =
        RegionCapital::read(Some(" 250000.50 ")).expect("a positive decimal is admitted");
    assert_eq!(admitted.amount(), dec!("250000.50"));

    // Absent and blank are the deployment that never set it — the state
    // every deployment of this binary was in until now — and the refusal
    // has to name the variable, or the operator is told the node is
    // misconfigured and not what to set.
    for value in [None, Some(""), Some("   ")] {
        let error = match RegionCapital::read(value) {
            Ok(capital) => panic!("{value:?} was admitted as {capital:?}"),
            Err(error) => error,
        };
        let message = error.message();
        assert!(
            message.starts_with("configuration:"),
            "the refusal of {value:?} is not a configuration error, so the node would exit as a \
             crash rather than a misdeployment: {message}"
        );
        assert!(
            message.contains(ALLOCATION_VARIABLE),
            "the refusal of {value:?} does not name the variable to set: {message}"
        );
    }

    // Unparseable and non-positive are refused rather than read as zero or
    // as "no allocation". Zero is the library's coherent "a region with no
    // capital"; on a node it is a process deciding on nothing while looking
    // healthy, and an operator who wants a cell stopped has the halt flag.
    for value in ["abc", "1e5", "1,000", "$5", "0", "0.0", "-5", "-0.25"] {
        let error = match RegionCapital::read(Some(value)) {
            Ok(capital) => panic!("{ALLOCATION_VARIABLE}={value} was admitted as {capital:?}"),
            Err(error) => error,
        };
        let message = error.message();
        assert!(
            message.starts_with("configuration:"),
            "the refusal of {value} is not a configuration error: {message}"
        );
        assert!(
            message.contains(&format!("{ALLOCATION_VARIABLE}={value}")),
            "the refusal does not echo the value it refused: {message}"
        );
    }

    // And the binary asks for it. `assemble` cannot be called without a
    // `RegionCapital`, and `RegionCapital::read` is the only way to make one,
    // so the type holds the rest; what no test can call is `main`, and the
    // one thing left to check is that `main` collects the variable in the
    // same "must be set" list as the cell id, the key and the venues — the
    // list an operator deploying a new cell reads once rather than per
    // restart. Counted as a whole `required("…")` call, not a substring, so
    // a comment naming the variable does not satisfy it.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let source =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let needle = format!("required(\"{ALLOCATION_VARIABLE}\"");
    assert_eq!(
        source.matches(&needle).count(),
        1,
        "main.rs does not require {ALLOCATION_VARIABLE} beside the other variables the node \
         refuses to start without"
    );
}

#[test]
fn a_node_built_with_the_allocation_refuses_a_second_strategy_once_it_is_spent() -> Result<()> {
    // Sized to exactly what one strategy spends, so the first strategy takes
    // the whole region and the second finds nothing. Both hold a grant of a
    // million, so nothing per-strategy is what stops the second: only the
    // bound this root now installs.
    let one = spent_by_one_strategy()?;
    assert!(
        one.is_positive(),
        "the probe premise failed: one pass spent nothing of the region ({one})"
    );
    let (mut node, mut gateway, mut feed) = node_with(&one.to_string(), &["alpha", "beta"])?;
    assert_eq!(
        node.cell.region_allocation_free(),
        Some(one),
        "the premise is a node holding exactly one strategy's worth"
    );

    let mut stats = PassStats::default();
    let outcome = run_pass(&mut node.cell, &mut gateway, &mut feed, &mut stats, t(10))?;
    let PassOutcome::Ran { report, breaks, .. } = outcome else {
        panic!("a running node reported its pass as halted: {outcome:?}");
    };
    assert!(breaks.is_empty(), "{breaks:?}");

    // Premise: both strategies fired. Without this the test would pass
    // against a node where beta never raised a signal, which is a different
    // fact entirely.
    assert_eq!(
        report.signals.len(),
        2,
        "the premise failed: both strategies must fire for the region gate to be what stops \
         the second: {:?}",
        report.refusals
    );

    // One order, from one strategy, at the venue — the venue's own count is
    // the witness the report cannot fake.
    assert_eq!(
        report.orders.len(),
        1,
        "the region allocation admitted more than it holds: {:?}",
        report.orders
    );
    assert_eq!(
        report.orders[0].contributors.len(),
        1,
        "the second strategy contributed to the net despite the region being spent: {:?}",
        report.orders[0].contributors
    );
    assert_eq!(report.orders[0].contributors[0].strategy.as_str(), "alpha");
    assert_eq!(gateway.submitted_count(), 1, "the venue saw a second order");

    let refusals = refused_under(&report, "region_reservation");
    assert_eq!(
        refusals.len(),
        1,
        "the region gate did not refuse exactly once: {:?}",
        report.refusals
    );
    assert!(
        refusals[0].contains("the 0 the region allocation has left"),
        "the refusal did not name what the allocation had left: {}",
        refusals[0]
    );
    assert_eq!(
        node.cell.region_allocation_free(),
        Some(Decimal::ZERO),
        "the region was not spent to zero by the strategy it admitted"
    );

    // And the refusal reached the registry the scrape serves, under its own
    // gate, so a deployed node whose second strategy is being turned away
    // says so in a series rather than only in its journal.
    let snapshot = node.scrape_registry().snapshot();
    assert_eq!(
        snapshot.counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", "region_reservation")
            ])
        ),
        1,
        "the region refusal was not counted on the registry the scrape serves"
    );
    Ok(())
}
