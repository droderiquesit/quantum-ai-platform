//! M6 critical path: latency of the classical baseline, its validator, a
//! feature read, and the cost router's placement of a decision.
//!
//! **Assert a ceiling, print the number.** Thresholds sit one to two orders of
//! magnitude above the debug-profile observation, to catch a complexity-class
//! regression rather than a percentage slowdown. See `performance.rs` for the
//! full reasoning, and for the measurements this file deliberately does not
//! repeat.
//!
//! **What this file used to hold, and why it no longer does.** It was written
//! against APIs that do not exist — `FeatureEngine::update`/`get`,
//! `Conviction::High`, a `SolverEffort::ensemble_size`, a two-argument
//! `Qubo::new`, `Router::select` taking a policy — and so never compiled, while
//! being reported as passing. The tests below are the ones whose property is
//! real and has an implementation, rewritten against it. Six were removed:
//!
//! - `quantum_solver_scaling_with_problem_size` asserted 12 and 20 variables
//!   cost within 5x per solve, on the premise that the larger used steepest
//!   descent. The default classical solver is exact up to 20 variables, so the
//!   two sizes differ by 2^8 by design; the property is false, not untested.
//! - `feature_dag_update_latency` duplicated
//!   `performance.rs::feature_evaluation_costs_what_the_budget_says`, which
//!   measures the same 4 x 6 graph through the real ingest-then-evaluate path.
//! - `feature_dag_scaling_with_graph_size` asserted cost flat across 2 and 6
//!   instruments. `performance.rs` documents why graph size is deliberately
//!   not the axis — evaluation is linear in registered features by
//!   construction — and guards the axis that can run away (uptime) in
//!   `feature_evaluation_costs_the_same_per_message_however_many_the_engine_has_already_seen`.
//! - `policy_gate_routing_scaling_with_context_variability` looked for a
//!   linear scan over context parameters. `Router` holds only a
//!   `RoutingPolicy`; there is no collection for routing to scan, so the test
//!   could not fail for the reason it named.
//! - `critical_path_end_to_end_latency_target` summed six hard-coded budget
//!   constants and asserted the sum was under 100ms. It measured nothing. A
//!   measured SENSE-to-DECIDE latency against the 100ms target does not exist
//!   anywhere in the workspace and is reported as missing work.
//! - `m6_optimization_targets_identified` printed a list and asserted nothing.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap

use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::{Origin, VenueId};
use qip_contracts::{FeatureKey, FeatureValue};
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_cost_router::{
    Conditions, DecisionContext, Determinism, Horizon, MarketRegime, Region, Router, Routing,
    RoutingPolicy, VolatilityRegime,
};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::features::{
    BookPressure, ExponentialMovingAverage, Microprice, Mid, RealisedVolatility, Spread,
};
use qip_feature_dag::state::MarketState;
use qip_financial::asset_class::AssetClass;
use qip_numerics::anneal::Qubo;
use qip_quantum::benchmark::ClassicalValidator;
use qip_quantum::solver::{ClassicalSolver, QuboSolver, SolverEffort};
use std::time::{Duration as WallDuration, Instant};

// ============================================================================
// UTILITIES
// ============================================================================

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{name}"))
}

fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Print the per-operation cost and assert it is under `ceiling_micros`.
fn report_latency(label: &str, operations: usize, elapsed: WallDuration, ceiling_micros: f64) {
    let seconds = elapsed.as_secs_f64();
    let per_operation_micros = seconds * 1e6 / operations as f64;

    println!(
        "{label}: {operations} ops in {seconds:.3}s = {per_operation_micros:.3} us/op ({} profile)",
        profile()
    );

    assert!(
        per_operation_micros < ceiling_micros,
        "{label} took {per_operation_micros:.3} us/op, past the {ceiling_micros:.0} us ceiling. \
         This is wall clock on {} profile, so a busy machine reads identically to a complexity \
         class change. Re-run this suite alone; if it barely moves, the change is real",
        profile()
    );
}

// ============================================================================
// THE CLASSICAL BASELINE (ADR 0006)
// ============================================================================

/// A 20-variable QUBO: assets that prefer to be held or not, with nearby
/// assets strongly correlated and distant ones weakly.
fn benchmark_qubo() -> Qubo {
    let n = 20;
    let mut qubo = Qubo::new(n);
    for i in 0..n {
        qubo.add_linear(i, if i % 3 == 0 { 0.5 } else { -0.3 });
    }
    for i in 0..n {
        for k in (i + 1)..n {
            qubo.add(i, k, if k - i <= 2 { 0.7 } else { 0.2 });
        }
    }
    qubo
}

#[test]
fn the_classical_baseline_solves_a_twenty_variable_problem_within_its_ceiling() -> Result<()> {
    // The baseline every quantum run is compared against is
    // `ClassicalSolver::new`, which is exact up to twenty variables. It runs
    // every time a quantum path does, so a complexity regression here is a
    // regression on every quantum decision.
    const SOLVES: usize = 2;

    let problem = benchmark_qubo();
    let solver = ClassicalSolver::new(0x1234);
    assert!(
        solver.is_exact_for(problem.n),
        "premise: the default baseline is exact at this size, which is what is being timed"
    );
    let effort = SolverEffort::default();

    let started = Instant::now();
    for _ in 0..SOLVES {
        let candidate = solver.solve(&problem, &effort)?;
        assert_eq!(candidate.assignment.len(), problem.n);
    }
    let elapsed = started.elapsed();

    report_latency(
        "classical baseline (exact, 20 vars)",
        SOLVES,
        elapsed,
        CEILING_BASELINE_MICROS,
    );
    Ok(())
}

#[test]
fn validating_a_solver_answer_costs_microseconds() -> Result<()> {
    // The classical validator re-evaluates every answer before anyone acts on
    // it; the claim is discarded and the recomputation is what flows on. It
    // runs on the critical path with no retry. This used to time
    // `candidate.clone()` with a comment saying the real call "would be" made.
    const VALIDATIONS: usize = 5_000;

    let problem = benchmark_qubo();
    let candidate = ClassicalSolver::new(0x5678).solve(&problem, &SolverEffort::default())?;
    let validator = ClassicalValidator::default();

    // Premise: an honest answer validates, so the loop times the full check
    // rather than an early refusal.
    let validated = validator.validate(&problem, &candidate)?;
    assert!((validated.objective() - candidate.claimed_objective).abs() < 1e-6);

    let started = Instant::now();
    for _ in 0..VALIDATIONS {
        validator.validate(&problem, &candidate)?;
    }
    let elapsed = started.elapsed();

    report_latency(
        "classical validation (20 vars)",
        VALIDATIONS,
        elapsed,
        CEILING_VALIDATION_MICROS,
    );
    Ok(())
}

// ============================================================================
// A FEATURE READ
// ============================================================================

/// The cell's graph: six features per instrument, four instruments.
fn feature_engine(symbols: &[&str]) -> Result<FeatureEngine> {
    let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    for symbol in symbols {
        let subject = object(symbol);
        engine.register(Box::new(Mid::new(subject.clone())))?;
        engine.register(Box::new(Spread::new(subject.clone())))?;
        engine.register(Box::new(Microprice::new(subject.clone())))?;
        engine.register(Box::new(BookPressure::new(subject.clone(), 5)))?;
        engine.register(Box::new(RealisedVolatility::new(subject.clone(), 20)))?;
        engine.register(Box::new(ExponentialMovingAverage::new(subject, 20)))?;
    }
    Ok(engine)
}

fn level(symbol: &str, side: BookSide, price: i64, sequence: u64) -> MarketMessage {
    let at = start().saturating_add(Duration::from_millis(sequence as i64));
    MarketMessage::new(
        object(symbol),
        Origin::new(VenueId::new("XLON"), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: Decimal::from_int(price),
            quantity: Decimal::from_int(100),
            order_count: None,
        },
        at,
        at,
    )
}

#[test]
fn reading_an_evaluated_feature_costs_microseconds() -> Result<()> {
    // A strategy reads features far more often than a message arrives. The
    // read must be a lookup, not a recomputation: `FeatureEngine::value`
    // promises "without evaluating", and this holds it to that at a cost.
    const QUERIES: usize = 50_000;

    let symbols = ["ACME", "BOREAS", "CERES", "DORIS"];
    let mut engine = feature_engine(&symbols)?;
    engine.ingest(&level("ACME", BookSide::Bid, 99, 0))?;
    engine.ingest(&level("ACME", BookSide::Ask, 101, 1))?;
    engine.evaluate(start().saturating_add(Duration::from_millis(2)))?;

    // Premise: the key is registered and holds a computed mid, so the loop
    // times a hit and not a miss returning `None` on the first comparison.
    let key = FeatureKey::new("mid", object("ACME"));
    let held = engine.value(&key).ok_or_else(|| {
        Error::invalid("premise: the mid feature is unregistered or holds no value")
    })?;
    assert_eq!(held, FeatureValue::Exact(dec!("100")));

    let started = Instant::now();
    let mut hits = 0usize;
    for _ in 0..QUERIES {
        hits += usize::from(engine.value(&key).is_some());
    }
    let elapsed = started.elapsed();
    assert_eq!(hits, QUERIES);

    report_latency(
        "feature read (24-node graph)",
        QUERIES,
        elapsed,
        CEILING_FEATURE_READ_MICROS,
    );
    Ok(())
}

// ============================================================================
// THE COST ROUTER
// ============================================================================

fn decision(determinism: Determinism) -> DecisionContext {
    DecisionContext::new(
        "size the order",
        dec!("50000"),
        Duration::from_secs(1),
        0.6,
        determinism,
        Conditions::new(
            AssetClass::Equity,
            Region::new("us-east"),
            MarketRegime::Trending,
            VolatilityRegime::Normal,
            Horizon::Intraday,
        ),
    )
}

#[test]
fn placing_a_determinism_required_decision_costs_microseconds() -> Result<()> {
    // Pre-trade risk checks, limit arithmetic and the order path are placed
    // here. The arm must be cheap because it is on every order.
    const DECISIONS: usize = 5_000;

    let router = Router::new(RoutingPolicy::default())?;
    let context = decision(Determinism::Required);

    // Premise: the arm being timed is the deterministic one.
    assert!(
        matches!(router.select(&context)?, Routing::Deterministic(_)),
        "a Required decision was not routed deterministically"
    );

    let started = Instant::now();
    for _ in 0..DECISIONS {
        router.select(&context)?;
    }
    let elapsed = started.elapsed();

    report_latency(
        "cost router: deterministic arm",
        DECISIONS,
        elapsed,
        CEILING_ROUTER_MICROS,
    );
    Ok(())
}

#[test]
fn placing_a_judged_decision_costs_microseconds() -> Result<()> {
    // The arm that walks the ladder of rungs and may choose a model. It is the
    // more expensive arm by construction and the one a regression would hide
    // in.
    const DECISIONS: usize = 5_000;

    let router = Router::new(RoutingPolicy::default())?;
    let context = decision(Determinism::NotRequired);

    assert!(
        matches!(router.select(&context)?, Routing::Judged(_)),
        "a NotRequired decision was not judged"
    );

    let started = Instant::now();
    for _ in 0..DECISIONS {
        router.select(&context)?;
    }
    let elapsed = started.elapsed();

    report_latency(
        "cost router: judged arm",
        DECISIONS,
        elapsed,
        CEILING_ROUTER_MICROS,
    );
    Ok(())
}

// ============================================================================
// CEILINGS
// ============================================================================
//
// Set from three debug-profile runs on the development machine (2026-10-06),
// per operation:
//
// - exact baseline, 20 vars:  1.69s, 1.87s, 1.75s  -> ceiling 30s (~17x)
// - validation, 20 vars:      1.99us, 1.72us, 1.38us -> ceiling 200us (~100x)
// - feature read:             0.22us, 0.35us, 0.21us -> ceiling 50us (~150x)
// - router, judged arm:       1.49us, 1.57us, 1.44us -> ceiling 200us (~130x)
//   (deterministic arm:       0.57us, 0.69us, 0.80us, same ceiling)
//
// The baseline's margin is narrower than the others' because two orders of
// magnitude above 1.7s is a ceiling nothing would reach. An exact search is
// exponential by design; what this catches is a regression to a worse class,
// which costs a factor of n or more, not a constant. Note the scale: the
// budget this file used to "assert" gave the whole REASON stage 120us.

const CEILING_BASELINE_MICROS: f64 = 30_000_000.0;
const CEILING_VALIDATION_MICROS: f64 = 200.0;
const CEILING_FEATURE_READ_MICROS: f64 = 50.0;
const CEILING_ROUTER_MICROS: f64 = 200.0;
