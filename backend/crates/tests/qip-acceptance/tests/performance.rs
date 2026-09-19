//! What each stage of the hot path actually costs, measured.
//!
//! `docs/performance/budgets.md` is written from the numbers this file prints.
//! That is the whole point of it existing: a budget nobody measured is a wish,
//! and a document full of wishes is worse than an empty one because somebody
//! will design against it.
//!
//! Three rules, taken from `qip-orderbook`'s `throughput.rs`, which is how
//! this repository already measures things honestly.
//!
//! * **Assert a ceiling, print the number.** Every assertion here is loose
//!   enough that only a real regression — an accidental clone per message, a
//!   linear scan where there was a lookup, a recomputation of a whole graph
//!   per tick — can trip it. A tight threshold on shared hardware fails for
//!   reasons that have nothing to do with the code, and a threshold loose
//!   enough not to would catch nothing, so the *measurement* is the output and
//!   the assertion is only a floor under it.
//! * **Say which profile.** `cargo test` builds unoptimised and is several
//!   times slower than `--release`. A figure quoted without its profile is not
//!   a figure. Every line printed here names the profile it came from.
//! * **Measure a stage, not a system.** Each test times one stage in
//!   isolation, on this machine, in one thread, with the fixture built before
//!   the clock starts. None of this is end-to-end latency, and none of it is
//!   evidence about a deployed system: there is no network here, no venue, no
//!   colocation, and no I/O. See the caveats in the budgets document.
//!
//! What is deliberately **not** measured, and therefore not claimed anywhere:
//! wire-to-wire latency, tick-to-order latency, cross-region latency, and
//! anything involving a real venue. This build has no venue transport at all.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::{CapitalEnvelope, Utilisation};
use qip_contracts::intent::Intent;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::PolicyPayload;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_contracts::{FeatureKey, FeatureValue, FeatureVector, Revision};
use qip_core::error::{Error, Result};
use qip_core::ids::{FillId, ObjectId, OrderId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_edge::cell::{
    Cell, CellConfig, ExecutionReport, Placer, PolledHalt, PricingPolicy, WorkReport,
};
use qip_edge::dropcopy::DropCopyFill;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::feasibility::{self as edge_feasibility, Granularity, VenueModel};
use qip_edge::journal::{Decision, Journal, MemoryMirror, ship};
use qip_edge::policy::VerifiedPolicy;
use qip_edge::reservation::RegionTable;
use qip_execution_engine::broker::{SimulatedBroker, SimulationSettings};
use qip_execution_engine::feasibility::{self as central_feasibility, VenueFeasibility};
use qip_execution_engine::multileg::{LegGroup, Verdict};
use qip_execution_engine::oms::{OrderManager, RefusalReason};
use qip_execution_engine::order::{Fill, Order, OrderType, Side};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::features::{
    BookPressure, ExponentialMovingAverage, Microprice, Mid, RealisedVolatility, Spread,
};
use qip_feature_dag::state::MarketState;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_orderbook::venue::VenueState;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::autonomy::AutonomyController;
use qip_risk_engine::pretrade::{PreTradeChecker, ProposedOrder};
use qip_sequencing::arbitration::{ArbitrationEvent, LineArbiter};
use qip_sequencing::tracker::{ReorderPolicy, SequenceEvent, Sequencer};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;
use qip_strategy::runtime::StrategyRuntime;
use std::collections::BTreeMap;
use std::time::{Duration as WallDuration, Instant};

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

// --- measurement ------------------------------------------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{name}"))
}

fn venue() -> VenueId {
    VenueId::new("XLON")
}

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("a decimal literal in a fixture")
}

fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// How much work [`machine_probe_nanos`] does. Sized so the probe costs a few
/// milliseconds unloaded: long enough that one descheduling does not dominate
/// it, short enough that running it beside every measurement in this file is
/// not itself the cost of the suite.
const PROBE_OPERATIONS: u64 = 4_000;

/// Time a fixed, unchanging workload, so that a measurement printed beside it
/// can be told apart from a machine that was merely busy.
///
/// **Nothing under test touches this function, and no threshold in this file
/// is derived from it.** That second half is deliberate and was arrived at the
/// hard way. The first attempt at this divided each measurement by the probe's
/// ratio against a baked-in reference figure for an unloaded machine, and the
/// figure was wrong by a factor of ten on its first run — which silently
/// multiplied every ceiling in the file by ten. A calibration constant that is
/// too low turns twenty-seven controls into controls that cannot fire, and
/// does it without a single failing test to say so. So the probe informs the
/// reader and nothing else: the ceilings are asserted against raw wall clock,
/// exactly as they were.
fn machine_probe_nanos() -> u128 {
    let started = Instant::now();
    let mut map: BTreeMap<u64, u64> = BTreeMap::new();
    let mut acc: u64 = 0;
    for i in 0..PROBE_OPERATIONS {
        let key = i.wrapping_mul(2_654_435_761) % 4_096;
        map.insert(key, i);
        acc = acc.wrapping_add(*map.get(&key).unwrap_or(&0));
        if map.len() > 2_048 {
            map.clear();
        }
    }
    std::hint::black_box((acc, &map));
    started.elapsed().as_nanos()
}

/// Print what a stage measured, and assert only that it was not absurd.
///
/// `ceiling` is per-operation, in microseconds, and is deliberately one to two
/// orders of magnitude above what an unoptimised build takes. It exists to
/// catch a change in complexity class, not to police a few percent.
///
/// Every line also carries `probe=`, the cost of the fixed workload in
/// [`machine_probe_nanos`] measured immediately after the stage. That field
/// exists because this suite spent a session misattributing its own failures.
/// The message here used to end "this is a change in complexity rather than a
/// slow machine", and in a container running several cargo builds at once that
/// sentence was false: the same suite measured 5.113 us/op alone and 27.701
/// us/op under load against a 20 us ceiling, four separate lanes went looking
/// for a regression that did not exist, and one of them watched the complexity
/// property *hold* inside the failing run — the stage fed 4.7x past its bounds
/// cost less per operation than at bounds. The claim was true about the code
/// and the number it was made about was measuring the machine. A failure
/// message naming the wrong cause is worse than one naming none, because it is
/// acted on.
///
/// A wall-clock ceiling cannot distinguish the two cases on its own, and this
/// function does not pretend otherwise. What it does is hand the reader a
/// controlled experiment: `probe=` is the same work every time, so re-running
/// the suite alone and comparing the two `probe=` fields says whether the
/// machine changed. If the probe fell by roughly the factor the measurement
/// fell by, it was contention. If the probe barely moved and the measurement
/// did, it is the code. [`report_scaling`] asserts the complexity property
/// directly and needs no such experiment.
fn report(label: &str, operations: usize, elapsed: WallDuration, ceiling_micros: f64) {
    let seconds = elapsed.as_secs_f64();
    let per_operation_micros = seconds * 1e6 / operations as f64;
    let probe = machine_probe_nanos();
    println!(
        "{label}: {operations} ops in {seconds:.3}s = {:.0} ops/s \
         ({per_operation_micros:.3} us/op, probe={probe}ns over {PROBE_OPERATIONS} fixed ops, \
         {} profile, this machine, single-threaded)",
        operations as f64 / seconds,
        profile()
    );
    assert!(
        per_operation_micros < ceiling_micros,
        "{label} took {per_operation_micros:.3} us/op, past the {ceiling_micros:.0} us \
         ceiling. This is wall clock, so it is not yet evidence of anything about the code: \
         a busy machine reads identically to a change in complexity, and this suite has \
         already sent four readers after a regression that did not exist. The probe beside \
         this measurement — a fixed workload of {PROBE_OPERATIONS} operations that no change \
         under test can affect — cost {probe}ns. Re-run this suite alone and compare that \
         field. If it falls by about the factor this measurement falls by, the machine was \
         the cause; if it barely moves while this measurement does, the change is real. \
         `book_apply_costs_the_same_per_message_however_many_it_is_fed` asserts the \
         complexity property without the experiment, and is the shape the rest of this file \
         should grow toward"
    );
}

/// Assert that per-operation cost does not grow when the input does.
///
/// This is the property the ceilings in this file are a proxy for, and unlike
/// a wall-clock ceiling it is load-invariant: both halves run on the same
/// machine, in the same process, under whatever load is present, so the load
/// divides out of the ratio instead of being subtracted from the margin.
///
/// `tolerance` is how much worse the larger run's per-operation cost may be.
/// It is not one: a larger run pays more cache pressure at the same complexity
/// class, and the distinction being drawn here is between constant-ish and
/// linear-in-N per operation, which is a factor of the size ratio and not a
/// few percent.
fn report_scaling(
    label: &str,
    small: (usize, WallDuration),
    large: (usize, WallDuration),
    tolerance: f64,
) {
    let (small_ops, small_elapsed) = small;
    let (large_ops, large_elapsed) = large;
    assert!(
        large_ops > small_ops,
        "{label}: the larger run must feed more operations than the smaller one, \
         or this measures nothing"
    );
    let small_micros = small_elapsed.as_secs_f64() * 1e6 / small_ops as f64;
    let large_micros = large_elapsed.as_secs_f64() * 1e6 / large_ops as f64;
    let growth = large_micros / small_micros;
    println!(
        "{label}: {small_ops} ops at {small_micros:.3} us/op vs {large_ops} ops at \
         {large_micros:.3} us/op = {growth:.2}x per-operation growth over a \
         {:.1}x larger input ({} profile, this machine, single-threaded)",
        large_ops as f64 / small_ops as f64,
        profile()
    );
    assert!(
        growth < tolerance,
        "{label}: feeding {:.1}x the input made each operation {growth:.2}x more \
         expensive, past the {tolerance:.1}x tolerance. Per-operation cost that rises \
         with the size of the input is a complexity class change — a linear scan where \
         there was a lookup, a whole-structure recomputation per item — and this \
         assertion is load-invariant, so a busy machine is not the explanation: both \
         halves were measured on it",
        large_ops as f64 / small_ops as f64
    );
}

// --- which ceilings stay, and why -------------------------------------------
//
// [`report_scaling`] is the better assertion wherever the property under test
// is "per-operation cost does not grow with the input", because it is
// load-invariant and a wall-clock ceiling is not. It is **not** a better
// assertion everywhere, and converting the rest mechanically would produce
// tests that pass forever while claiming to guard something. Ten of the
// twenty-seven ceilings in this file now have a scaling companion beside them;
// the seventeen that do not are listed here with the reason, because a reader
// who finds a raw ceiling is entitled to know whether it survived a decision
// or was overlooked.
//
// **Stateless per-operation work.** Nothing accumulates between iterations, so
// feeding the loop more iterations measures the same thing again and a growth
// ratio near 1.0 is arithmetic rather than evidence:
//
// * `capital admit` — [`CapitalEnvelope::admit`] reads the grant and a
//   `Utilisation` of scalar counters. Nothing it touches is sized by how many
//   decisions came before.
// * `order construct + validate` — a fresh `Order` and five refusals over its
//   own fields. `OrderManager::next_order_id` increments a `u64`.
// * `edge feasibility gate` — `qip_edge::feasibility::assess` is a pure
//   function of one intent and one venue model.
// * `capital envelope verify (HMAC)` — one HMAC over a fixed-size payload and
//   a constant-time comparison, by construction the same work every time.
// * `multi-leg group` — every group is assembled, filled, assessed and settled
//   from nothing. The axis that would say something here is legs per group,
//   not groups.
// * `pre-trade check (5 limits)` — the limit set is fixed and the risk state
//   is not mutated by the check. The axis that would say something is limits
//   per set.
//
// **A ceiling that is the point, not a proxy.** Here the absolute number is
// the deliverable and a ratio would answer a question nobody asked:
//
// * both `edge halt wire` figures — how long the platform keeps trading after
//   somebody has told it to stop. A risk desk asks for that in milliseconds.
//   "It stops in the same time per halt however many halts you send" is true
//   of a wire that takes a minute.
// * `arbitrage scan` — the scan is a bounded search whose cost is a property
//   of the graph, and its cheap case is the one that found nothing. Per-scan
//   cost is meant to move with the graph; flatness is not the property.
// * `strategy run` — cost is meant to be linear in the program's node count
//   and independent of the market, which is what makes it budgetable at all.
//   The node count is printed in the label for exactly that reason.
// * `kernel cycle (bounded history)` — the test around it already asserts the
//   load-invariant half directly, as a ratio between a platform at its
//   retention bounds and one fed 4.7x past them.
//
// **Covered by a scaling assertion at the same seam.** Adding a second ratio
// over the same accumulating state would be a duplicate, not a guard:
//
// * `central instrument feasibility` — the same `OrderManager` store as
//   `central OMS submit scaling`, with a grid lookup in front.
// * `edge netting`, `edge internal cross`, `edge resting order expiry` — the
//   same `Cell` pass loop as `edge work pass scaling`, exercising different
//   branches inside it.
//
// **Measured, printed, and deliberately not asserted.** `journal ship to
// mirror` has a ratio beside it in
// `the_journal_chain_costs_the_same_per_entry_however_long_the_chain_gets` and
// no assertion on it: three consecutive full-suite runs on an idle machine
// printed 1.40x, 1.63x and 2.02x for identical code, because the call is a few
// milliseconds dominated by the mirror allocating its copy of the tail. The
// reasoning is written out where the measurement is taken.
//
// **Measured only with one hold standing.** `region reservation` takes a hold
// and commits it in the same breath, so the ledger never holds more than one
// and repeating the pair more times grows nothing. The honest axis is the
// number of *concurrent* holds a region's cells have outstanding, which this
// fixture does not build; the ceiling stays and this is named rather than
// papered over.

// --- fixtures ---------------------------------------------------------------

/// `count` level-set messages walking a book around a hundred.
///
/// Built before any clock starts, so the fixture's own cost is never inside a
/// measurement.
fn level_stream(symbol: &str, count: usize, seed: u64) -> Vec<MarketMessage> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..count)
        .map(|index| {
            let side = if index % 2 == 0 {
                BookSide::Bid
            } else {
                BookSide::Ask
            };
            let offset = rng.below(9) as i64 - 4;
            let price = if side == BookSide::Bid {
                99 + offset.min(0)
            } else {
                101 + offset.max(0)
            };
            let at = start().saturating_add(Duration::from_millis(index as i64));
            MarketMessage::new(
                object(symbol),
                Origin::new(venue(), "feed-a", 0, index as u64),
                MessageBody::LevelSet {
                    side,
                    price: Decimal::from_int(price),
                    quantity: Decimal::from_int(100 + (index % 7) as i64 * 50),
                    order_count: None,
                },
                at,
                at,
            )
        })
        .collect()
}

/// A feature graph of the size a cell really runs: several instruments, and
/// the microstructure features a strategy actually reads.
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

fn pressure_key(symbol: &str) -> FeatureKey {
    FeatureKey::new("book_pressure", object(symbol)).with("levels", 5)
}

/// A three-rule strategy over four features, compiled by the real compiler.
fn compiled(symbol: &str) -> Result<(CompiledStrategy, Program)> {
    let subject = object(symbol);
    let pressure = pressure_key(symbol);
    let volatility = FeatureKey::new("realised_volatility", subject.clone()).with("window", 20);
    let mid = FeatureKey::new("mid", subject.clone());
    let spread = FeatureKey::new("spread", subject.clone());

    let mut catalogue = FeatureCatalogue::new();
    for (key, value_type) in [
        (pressure.clone(), Type::Statistic),
        (volatility.clone(), Type::Statistic),
        (mid.clone(), Type::Exact),
        (spread.clone(), Type::Exact),
    ] {
        catalogue.declare(key, value_type)?;
    }

    let spec = StrategySpec::new(
        StrategyId::new("performance-strategy"),
        subject,
        Duration::from_millis(250),
    )
    .with_rule(Rule::new(
        "enter",
        SignalKind::Enter,
        Expr::feature(pressure.clone())
            .greater_than(Expr::Statistic(0.4))
            .and(Expr::feature(volatility.clone()).less_than(Expr::Statistic(0.9)))
            .and(Expr::feature(spread.clone()).at_most(Expr::feature(mid.clone()))),
        Expr::Exact(dec!("100")),
        Expr::Statistic(0.62),
        500,
    ))
    .with_rule(Rule::new(
        "exit",
        SignalKind::Exit,
        Expr::feature(pressure)
            .less_than(Expr::Statistic(-0.4))
            .and(Expr::feature(volatility).greater_than(Expr::Statistic(0.2))),
        Expr::Exact(dec!("100")),
        Expr::Statistic(0.55),
        500,
    ));

    let mut compiler = StrategyCompiler::new(catalogue);
    let strategy = compiler.compile(&spec)?;
    Ok((strategy, compiler.into_program()))
}

fn vector_for(symbol: &str, as_of: Timestamp, pressure: f64) -> FeatureVector {
    let subject = object(symbol);
    let mut vector = FeatureVector::new(as_of);
    vector.insert(
        pressure_key(symbol),
        FeatureValue::Statistic(pressure),
        Revision::new(1),
    );
    vector.insert(
        FeatureKey::new("realised_volatility", subject.clone()).with("window", 20),
        FeatureValue::Statistic(0.35),
        Revision::new(2),
    );
    vector.insert(
        FeatureKey::new("mid", subject.clone()),
        FeatureValue::Exact(dec!("100")),
        Revision::new(3),
    );
    vector.insert(
        FeatureKey::new("spread", subject),
        FeatureValue::Exact(dec!("0.02")),
        Revision::new(4),
    );
    vector
}

fn signed_envelope() -> Result<CapitalEnvelope> {
    let terms = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("performance-strategy"),
            "performance-1",
            dec!("100000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
            start(),
            start().saturating_add(Duration::from_hours(1)),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = terms("unsigned")?;
    terms(&sign_payload(
        b"a-performance-key",
        &unsigned.signing_payload(),
    ))
}

fn risk_limits() -> LimitSet {
    LimitSet::new("performance")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at twice equity"),
        )
        .with(
            Limit::new("max-net-exposure", LimitKind::MaxNetExposure { limit: 1.0 })
                .with_rationale("the book is not net long more than its equity"),
        )
        .with(
            Limit::new("cash-buffer", LimitKind::MinCashBuffer { limit: 0.05 })
                .with_rationale("a twentieth of equity stays in cash so a settlement never fails"),
        )
        .with(
            Limit::new(
                "max-order-notional",
                LimitKind::MaxOrderNotional {
                    limit: dec!("5000000"),
                },
            )
            .with_rationale("no single order moves more than five million at once"),
        )
}

// --- the stages -------------------------------------------------------------
//
// There is no normalisation stage here. `qip-normalization` was removed by
// ADR 0029 — nothing constructed it, so the figure this file once published
// for it was the cost of a stage no observation went through — and nothing in
// the workspace now canonicalises a venue or converts a provider's units. The
// budgets document says so rather than carrying a row for it.

#[test]
fn book_apply_costs_what_the_budget_says() -> Result<()> {
    // The hottest call in the platform: one venue message folded into one
    // book. Everything downstream is per-decision; this is per-packet.
    const MESSAGES: usize = 200_000;
    let stream = level_stream("ACME", MESSAGES, 0xB0_0C);
    let mut state = VenueState::aggregated(object("ACME"), venue(), VenueStatus::Open);

    let started = Instant::now();
    for message in &stream {
        state.apply(message)?;
    }
    let elapsed = started.elapsed();

    assert_eq!(state.applied(), MESSAGES as u64);
    assert!(state.mid().is_some(), "the book did not end up priceable");
    report("book apply (L2 level set)", MESSAGES, elapsed, 20.0);
    Ok(())
}

#[test]
fn book_apply_costs_the_same_per_message_however_many_it_is_fed() -> Result<()> {
    // The property the ceiling above is a proxy for, asserted directly. A book
    // that folds one message in constant time is the whole design; one that
    // rescans its levels per message is linear in the book and would show up
    // here as per-operation cost rising with the input.
    //
    // This is the load-invariant half of this suite. The ceiling above is a
    // wall-clock figure and a wall clock on a shared machine measures the
    // machine; this ratio measures both halves on the same machine moments
    // apart, so contention cancels. It is here because a lane watched this
    // exact stage trip its ceiling under a parallel build while the scaling
    // property visibly held — fed 4.7x past bounds it cost *less* per message
    // — which is the clearest possible statement that the ceiling and the
    // property are not the same assertion.
    const SMALL: usize = 50_000;
    const LARGE: usize = 250_000;

    let small_stream = level_stream("ACME", SMALL, 0xB0_0C);
    let large_stream = level_stream("ACME", LARGE, 0xB0_0C);

    let mut small_state = VenueState::aggregated(object("ACME"), venue(), VenueStatus::Open);
    let started = Instant::now();
    for message in &small_stream {
        small_state.apply(message)?;
    }
    let small_elapsed = started.elapsed();

    let mut large_state = VenueState::aggregated(object("ACME"), venue(), VenueStatus::Open);
    let started = Instant::now();
    for message in &large_stream {
        large_state.apply(message)?;
    }
    let large_elapsed = started.elapsed();

    // Premise: both runs really folded every message they were given. A run
    // that silently stopped early would make the larger one look cheap per
    // operation and pass this for the wrong reason.
    assert_eq!(small_state.applied(), SMALL as u64);
    assert_eq!(large_state.applied(), LARGE as u64);

    report_scaling(
        "book apply scaling",
        (SMALL, small_elapsed),
        (LARGE, large_elapsed),
        2.0,
    );
    Ok(())
}

#[test]
fn feature_evaluation_costs_what_the_budget_says() -> Result<()> {
    // The measurement that matters is the incremental one: a message dirties
    // the nodes it can affect, and an evaluation recomputes exactly those. So
    // the loop is ingest-then-evaluate, per message, which is what a cell
    // actually does — not a batch evaluation amortised over a stream.
    const MESSAGES: usize = 20_000;
    let symbols = ["ACME", "BOREAS", "CERES", "DORIS"];
    let mut engine = feature_engine(&symbols)?;
    let streams: Vec<Vec<MarketMessage>> = symbols
        .iter()
        .enumerate()
        .map(|(index, symbol)| {
            level_stream(symbol, MESSAGES / symbols.len(), 0xFEA7 + index as u64)
        })
        .collect();

    let mut computed = 0usize;
    let started = Instant::now();
    for (index, stream) in streams.iter().enumerate() {
        for message in stream {
            engine.ingest(message)?;
            let vector = engine.evaluate(
                start().saturating_add(Duration::from_millis((index * MESSAGES + computed) as i64)),
            )?;
            computed += vector.len().min(1);
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(computed, MESSAGES, "an evaluation returned an empty vector");
    let vector = engine.evaluate(start().saturating_add(Duration::from_secs(1)))?;
    assert_eq!(
        vector.len(),
        symbols.len() * 6,
        "the graph is not the size the fixture registered"
    );
    report("feature ingest + evaluate", MESSAGES, elapsed, 500.0);
    Ok(())
}

#[test]
fn feature_evaluation_costs_the_same_per_message_however_many_the_engine_has_already_seen()
-> Result<()> {
    // The property the ceiling above is a proxy for, on the axis that can
    // actually run away: uptime.
    //
    // The *graph's* size is deliberately not the axis. `FeatureEngine::evaluate`
    // walks its whole topological order every call and recomputes the nodes a
    // message dirtied, so per-message cost is linear in the number of
    // registered features by construction, and a scaling assertion over symbol
    // count would fail on a design decision rather than on a regression. What
    // must not grow is the history behind each feature: the realised-volatility
    // and moving-average windows, and the book `MarketState` keeps. That is a
    // regression this platform has already shipped once on another path — the
    // kernel's history series held every observation since assembly and the
    // deployed cycle went from 2.4ms at cycle 255 to 310ms at cycle 16,728 —
    // and here it would read as per-message cost rising with the number of
    // messages already ingested.
    //
    // Load-invariant, unlike the ceiling above: both halves are measured on
    // this machine moments apart, so contention divides out of the ratio
    // instead of being subtracted from the margin.
    const SMALL: usize = 5_000;
    const LARGE: usize = 25_000;
    let symbols = ["ACME", "BOREAS", "CERES", "DORIS"];

    let feed = |total: usize| -> Result<(WallDuration, usize)> {
        let mut engine = feature_engine(&symbols)?;
        let streams: Vec<Vec<MarketMessage>> = symbols
            .iter()
            .enumerate()
            .map(|(index, symbol)| {
                level_stream(symbol, total / symbols.len(), 0xFEA7 + index as u64)
            })
            .collect();
        let mut computed = 0usize;
        let started = Instant::now();
        for (index, stream) in streams.iter().enumerate() {
            for message in stream {
                engine.ingest(message)?;
                let vector = engine
                    .evaluate(start().saturating_add(Duration::from_millis(
                        (index * total + computed) as i64,
                    )))?;
                computed += vector.len().min(1);
            }
        }
        Ok((started.elapsed(), computed))
    };

    let (small_elapsed, small_computed) = feed(SMALL)?;
    let (large_elapsed, large_computed) = feed(LARGE)?;

    // Premise: both runs really evaluated every message they were given. A run
    // whose evaluations returned nothing would make the larger one look cheap
    // per message and pass this for the wrong reason.
    assert_eq!(small_computed, SMALL, "an evaluation returned no vector");
    assert_eq!(large_computed, LARGE, "an evaluation returned no vector");

    report_scaling(
        "feature ingest + evaluate scaling",
        (SMALL, small_elapsed),
        (LARGE, large_elapsed),
        2.0,
    );
    Ok(())
}

#[test]
fn strategy_evaluation_costs_what_the_budget_says() -> Result<()> {
    // Evaluation cost does not depend on the market: every node the strategy
    // reaches is computed whichever way its conditions go. That is why this
    // number is worth budgeting at all — it is a property of the strategy
    // rather than of the news, so a measurement today bounds tomorrow.
    const RUNS: usize = 200_000;
    let (strategy, program) = compiled("ACME")?;
    let mut runtime = StrategyRuntime::new(program)?;

    // Two vectors, one that fires and one that does not, alternated so the
    // measurement is not of a single branch.
    let firing = vector_for("ACME", start(), 0.8);
    let quiet = vector_for("ACME", start(), 0.0);
    let mut signals = 0usize;

    let started = Instant::now();
    for index in 0..RUNS {
        let vector = if index % 2 == 0 { &firing } else { &quiet };
        if runtime.run(&strategy, vector, start())?.is_some() {
            signals += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        signals,
        RUNS / 2,
        "the fixture did not exercise both branches"
    );
    report(
        &format!("strategy run ({} nodes)", strategy.cost()),
        RUNS,
        elapsed,
        50.0,
    );
    Ok(())
}

#[test]
fn arbitrage_detection_costs_what_the_budget_says() -> Result<()> {
    // Search, exact confirmation, book walk, nine deductions and a leg plan:
    // the whole narrowing, per scan. Budgeted as one number because the stages
    // are not separable in practice — a scan that stops early is a scan that
    // found nothing, which is the common case and the cheap one.
    use qip_arbitrage::graph::{ArbitrageGraph, Node, VenueFacts};
    use qip_arbitrage::liquidity::StaticLiquidity;
    use qip_arbitrage::netedge::EdgeAssumptions;
    use qip_arbitrage::plan::PlanSettings;
    use qip_arbitrage::scan::{OpportunityScanner, SizePolicy};
    use qip_arbitrage::search::SearchSettings;
    use qip_market::book::{BookLevel, OrderBook};

    const SCANS: usize = 2_000;
    let cx = VenueId::new("CX");
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        cx.clone(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    let node = |name: &str| Node::new(ObjectId::from_string(name), cx.clone());
    for (from, to, rate, market, side) in [
        ("USDT", "ETH", "0.000333328", "ETHUSDT", BookSide::Ask),
        ("ETH", "BTC", "0.050505", "ETHBTC", BookSide::Bid),
        ("BTC", "USDT", "60000.5", "BTCUSDT", BookSide::Bid),
    ] {
        graph.add_trade(
            node(from),
            node(to),
            d(rate),
            d("0.0004"),
            ObjectId::from_string(market),
            side,
            start(),
            20,
        )?;
    }

    let book = |market: &str, bid: &str, ask: &str| {
        OrderBook::from_levels(
            ObjectId::from_string(market),
            "CX",
            start(),
            vec![BookLevel::new(d(bid), d("200"))],
            vec![BookLevel::new(d(ask), d("200"))],
        )
    };
    let depth = StaticLiquidity::new()
        .with_book(cx.clone(), book("ETHUSDT", "3000.0", "3000.1"), 20)
        .with_book(cx.clone(), book("ETHBTC", "0.0505", "0.05051"), 20)
        .with_book(cx, book("BTCUSDT", "60000", "60001"), 20);

    let scanner = OpportunityScanner::new(
        SearchSettings::default(),
        EdgeAssumptions::default(),
        PlanSettings::with_budget(d("1000000")),
    );
    let policy = SizePolicy::uniform(d("10000"));

    let mut found = 0usize;
    let started = Instant::now();
    for _ in 0..SCANS {
        let report = scanner.scan(&graph, &depth, &policy, start());
        found += report.opportunities.len();
    }
    let elapsed = started.elapsed();

    assert_eq!(
        found, SCANS,
        "the fixture stopped finding its dislocation partway through"
    );
    report(
        "arbitrage scan (3-node graph, 3 edges)",
        SCANS,
        elapsed,
        5_000.0,
    );
    Ok(())
}

#[test]
fn the_capital_decision_costs_what_the_budget_says() -> Result<()> {
    // The gate between a signal and an order on the cell's hot path: expiry,
    // venue scope, loss limit, headroom and the per-order cap, in that order.
    // It runs per candidate order, inside the decision, so it is budgeted with
    // the decision rather than with the allocator that produced the grant.
    const DECISIONS: usize = 500_000;
    let envelope = VerifiedEnvelope::verify(
        signed_envelope()?,
        b"a-performance-key",
        "performance-1",
        start(),
    )?;
    let xlon = venue();
    let mut utilisation = Utilisation::default();
    let mut granted = 0usize;

    let started = Instant::now();
    for index in 0..DECISIONS {
        let notional = Decimal::from_int(1_000 + (index % 97) as i64 * 100);
        if !envelope
            .admit(&xlon, notional, &utilisation, start())
            .is_refused()
        {
            granted += 1;
        }
        // A little utilisation so the headroom arithmetic is not constant.
        utilisation.orders_sent += 1;
    }
    let elapsed = started.elapsed();

    assert_eq!(granted, DECISIONS, "the fixture ran out of headroom");
    report("capital admit", DECISIONS, elapsed, 20.0);
    Ok(())
}

#[test]
fn the_risk_decision_costs_what_the_budget_says() -> Result<()> {
    // Five limits projected against the state the order would produce. This is
    // the check between a proposal and a venue, and it runs once per order, so
    // its cost is on the decision path rather than the packet path.
    const CHECKS: usize = 100_000;
    let checker = PreTradeChecker::new(risk_limits());
    let state = RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        gross_exposure: dec!("4000000"),
        net_exposure: dec!("1000000"),
        position_notionals: BTreeMap::from([
            ("obj-ACME".to_string(), dec!("400000")),
            ("obj-BOREAS".to_string(), dec!("600000")),
        ]),
        ..RiskState::default()
    };
    let order = ProposedOrder {
        object_id: object("ACME"),
        quantity: dec!("1000"),
        reference_price: dec!("100"),
        // The counterparty is one more exposure axis rather than a field of
        // its own — see `ProposedOrder::axes` — so the projection walks two
        // buckets here, which is what the budget below is measured against.
        axes: BTreeMap::from([
            ("sector".to_string(), "information_technology".to_string()),
            (
                qip_risk::limits::COUNTERPARTY_AXIS.to_string(),
                "broker-a".to_string(),
            ),
        ]),
        scope: "performance".to_string(),
    };

    let mut approved = 0usize;
    let started = Instant::now();
    for _ in 0..CHECKS {
        if checker.check(&order, &state, start())?.is_approved() {
            approved += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(approved, CHECKS, "the fixture was refused, so nothing ran");
    report("pre-trade check (5 limits)", CHECKS, elapsed, 100.0);
    Ok(())
}

#[test]
fn order_construction_costs_what_the_budget_says() -> Result<()> {
    // Building the order and validating it: the identifier, the lineage back
    // to a proposal and its hypotheses, and the five refusals that make an
    // untraceable order impossible to construct.
    const ORDERS: usize = 200_000;
    let mut manager = OrderManager::new(PreTradeChecker::new(risk_limits()));
    let hypotheses = vec!["hyp-performance".to_string()];

    let mut built = 0usize;
    let started = Instant::now();
    for _ in 0..ORDERS {
        let order_id = manager.next_order_id("ord");
        let order = Order::new(
            order_id,
            object("ACME"),
            Side::Buy,
            dec!("1000"),
            OrderType::Market,
            dec!("100"),
            "prop-performance",
            hypotheses.clone(),
            "performance",
            start(),
        );
        order.validate()?;
        built += 1;
    }
    let elapsed = started.elapsed();

    assert_eq!(built, ORDERS);
    report("order construct + validate", ORDERS, elapsed, 20.0);
    Ok(())
}

// --- the honesty check ------------------------------------------------------

/// The stage label of every figure row in the budgets document, lowercased.
///
/// A figure row is a Markdown table row whose first cell is a stage name:
/// the header row (`Stage`) and the alignment row (`---`) are not figures and
/// are skipped. Every table in the document is a table of figures, so any row
/// this returns is a published number somebody may design against.
fn budget_rows(budgets: &str) -> Vec<String> {
    budgets
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let cells: Vec<&str> = line.strip_prefix('|')?.split('|').collect();
            let stage = cells.first()?.trim().to_lowercase();
            if stage.is_empty() || stage == "stage" || stage.chars().all(|c| c == '-' || c == ':') {
                None
            } else {
                Some(stage)
            }
        })
        .collect()
}

#[test]
fn the_budgets_document_says_what_is_measured_and_what_is_not() {
    // The document is the deliverable these tests exist to justify, so it is
    // checked here rather than trusted. Two claims have to survive contact
    // with a reader: that no end-to-end latency has been measured, and that
    // microsecond-class figures apply only to colocated paths this build does
    // not have. A budgets document that quietly dropped either would read as a
    // performance guarantee, which is the specific overclaim to avoid.
    let budgets = qip_acceptance::read("docs/performance/budgets.md");
    let lowered = budgets.to_lowercase();

    for required in [
        "no end-to-end latency has been measured",
        "colocated",
        "unmeasured",
        "debug",
        "release",
    ] {
        assert!(
            lowered.contains(required),
            "docs/performance/budgets.md does not say \"{required}\""
        );
    }

    // Every stage these tests measure has a row, so a stage cannot be measured
    // and then quietly left out of the budget it was measured for. One entry
    // per `*_costs_what_the_budget_says` test above; the kernel-cycle test at
    // the end of this file asserts flatness across two working sets rather
    // than a per-operation budget, and has no row by design.
    const MEASURED_STAGES: [&str; 7] = [
        "book apply",
        "feature",
        "strategy",
        "arbitrage",
        "capital",
        "risk",
        "order construction",
    ];
    let rows = budget_rows(&budgets);
    assert!(
        !rows.is_empty(),
        "docs/performance/budgets.md has no budget rows at all"
    );
    for stage in MEASURED_STAGES {
        assert!(
            rows.iter().any(|row| row.contains(stage)),
            "docs/performance/budgets.md has no row for {stage}"
        );
    }

    // And the other direction, which the check above cannot see: every row
    // names a stage something in this file measures. Without it the document
    // could keep publishing a figure for a stage nothing times — which it did:
    // the normalisation rows outlived their measurement until ADR 0029 removed
    // the crate, and this check stayed green throughout. Each row must match
    // exactly one stage, so a row that straddles two labels cannot count as
    // covering either.
    for row in &rows {
        let matched = MEASURED_STAGES
            .iter()
            .filter(|stage| row.contains(*stage))
            .count();
        assert_eq!(
            matched, 1,
            "docs/performance/budgets.md publishes a row for \"{row}\", which matches {matched} \
             measured stages rather than one; a figure for a stage nothing times is a wish \
             dressed as a measurement"
        );
    }

    // And nothing in it claims a number for a path nobody timed.
    for overclaim in [
        "sub-microsecond end-to-end",
        "wire-to-wire latency of",
        "one million events per second",
    ] {
        assert!(
            !lowered.contains(overclaim),
            "docs/performance/budgets.md claims \"{overclaim}\""
        );
    }
}

#[test]
fn the_cycle_cost_stops_growing_once_the_history_working_sets_reach_their_bounds() -> Result<()> {
    // The regression this catches has shipped: the kernel's history series,
    // the liquidity topology and the prediction set all held every
    // observation since assembly, and because DISCOVER rescans every series
    // per cycle, the deployed fastbrain's cycle grew from 2.4ms at cycle 255
    // to 310ms at cycle 16,728 — six times its 50ms ceiling — and the
    // readiness probe took the node out of rotation. The property asserted
    // here is *flatness beyond the bounds*, which no slow machine can fake
    // in either direction: a platform fed several times more history than
    // the caps must hold the same working set, and pay about the same per
    // cycle, as one fed exactly at them.
    use qip_core::Context;
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    use qip_financial::universe::Universe;
    use qip_kernel::{Platform, PlatformConfig, Stage};
    use qip_market::quote::Quote;
    use qip_observability::Telemetry;

    const SYMBOLS: [&str; 5] = ["AAA", "BBB", "CCC", "DDD", "EEE"];

    fn universe() -> Result<Universe> {
        let mut universe = Universe::new();
        for symbol in SYMBOLS {
            universe.insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    qip_financial::asset_class::InstrumentType::CommonStock,
                    fixture_liquidity(),
                )
                .venue("XNYS")
                .sector(qip_financial::asset_class::Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("performance", start()))
                .build(start())?,
            )?;
        }
        Ok(universe)
    }

    /// The last `keep` bars of one fixed `total`-bar path.
    ///
    /// Both platforms are fed suffixes of the *same* path so that, after
    /// retention, they hold byte-identical series — otherwise the regime
    /// fit's data-dependent EM iteration count would differ between two tapes
    /// and read here as a difference retention caused.
    fn bars_tail(symbol: &str, total: usize, keep: usize) -> Vec<SensedRecord> {
        let mut price = 100.0_f64;
        (0..total)
            .map(|index| {
                let noise = ((index as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
                let open = price;
                price *= 1.0 + noise;
                let at = start().saturating_sub(Duration::from_days((total - index) as i64));
                SensedRecord::Bar(Box::new(Bar {
                    object_id: object(symbol),
                    venue: "XNYS".to_string(),
                    interval: Interval::Day,
                    open_time: at,
                    open: Decimal::from_f64(open).expect("representable"),
                    high: Decimal::from_f64(open.max(price) * 1.002).expect("representable"),
                    low: Decimal::from_f64(open.min(price) * 0.998).expect("representable"),
                    close: Decimal::from_f64(price).expect("representable"),
                    volume: dec!("1000000"),
                    trade_count: 5_000,
                    vwap: Decimal::from_f64((open + price) / 2.0),
                    quality: qip_financial::quality::DataQuality::default(),
                }))
            })
            .skip(total - keep)
            .collect()
    }

    /// The last `keep` quotes of one fixed `total`-quote path.
    fn quotes_tail(symbol: &str, total: usize, keep: usize) -> Vec<SensedRecord> {
        (0..total)
            .map(|index| {
                let wiggle = ((index as f64 * 0.618) % 1.0 - 0.5) * 0.02;
                SensedRecord::Quote(Quote {
                    object_id: object(symbol),
                    venue: "XNYS".to_string(),
                    at: start().saturating_sub(Duration::from_secs((total - index) as i64)),
                    bid: Decimal::from_f64(99.9 + wiggle).expect("representable"),
                    ask: Decimal::from_f64(100.1 + wiggle).expect("representable"),
                    bid_size: dec!("500"),
                    ask_size: dec!("500"),
                    quality: qip_financial::quality::DataQuality::default(),
                })
            })
            .skip(total - keep)
            .collect()
    }

    fn platform_fed(bars_each: usize, quotes_each: usize) -> Result<Platform> {
        let config = PlatformConfig::default();
        let (context, _clock) = Context::deterministic(start(), config.seed);
        let mut platform = Platform::new(
            config,
            context,
            Telemetry::silent(),
            universe()?,
            risk_limits(),
        )?;
        for symbol in SYMBOLS {
            platform.observe(bars_tail(symbol, 2_416, bars_each));
            platform.observe(quotes_tail(symbol, 24_161, quotes_each));
        }
        Ok(platform)
    }

    fn cheapest_cycle(platform: &mut Platform) -> (WallDuration, usize) {
        let mut cheapest = WallDuration::MAX;
        let mut sensed = 0usize;
        for _ in 0..3 {
            let began = Instant::now();
            let report = platform.run_cycle(start());
            cheapest = cheapest.min(began.elapsed());
            sensed = report
                .stages
                .iter()
                .find(|stage| stage.stage == Stage::Sense)
                .map_or(0, |stage| stage.produced);
        }
        (cheapest, sensed)
    }

    // One platform at the bounds, one fed several times past them — the
    // second is the deployed evidence's shape (2,416 bars and 24,161 depth
    // observations per instrument at cycle 16,728).
    let mut at_bounds = platform_fed(512, 512)?;
    let mut past_bounds = platform_fed(2_416, 24_161)?;

    let (bounded, bounded_sensed) = cheapest_cycle(&mut at_bounds);
    let (grown, grown_sensed) = cheapest_cycle(&mut past_bounds);

    // The premise, exactly: retention capped the second platform's working
    // set to the first's. If the bounds are removed this fails before any
    // timing is read.
    assert!(bounded_sensed > 0, "the fixture fed the platform nothing");
    assert_eq!(
        grown_sensed, bounded_sensed,
        "a platform fed 4.7x more bars holds a larger sense working set than one fed at the \
         bounds; the history caps are not being applied at retention"
    );

    println!(
        "cycle at bounds: {bounded:?}; cycle fed 4.7x past bounds: {grown:?} \
         ({} profile, this machine, single-threaded)",
        profile()
    );
    report("kernel cycle (bounded history)", 1, bounded, 500_000.0);

    // Flatness, loosely: the two cycles walk identical working sets, so only
    // a series that escaped its bound — cost growing with what was fed rather
    // than with what is retained — can push this past double.
    let ratio = grown.as_secs_f64() / bounded.as_secs_f64().max(1e-9);
    assert!(
        ratio < 2.0,
        "a cycle over a 4.7x-larger feed costs {ratio:.1}x one at the bounds; per-cycle work \
         is growing with uptime again"
    );
    Ok(())
}

// --- the execution capabilities ---------------------------------------------
//
// `docs/ops/execution-measurements.md` is written from what this section
// prints. The traceability document scores every execution capability as
// TESTED and none as MEASURED; these are the first numbers, and they are
// in-process numbers on a shared container — a regression guard on the
// complexity class of each seam, never a deployment figure. Nothing is
// deployed (`execution_nodes = {}` in every environment), so there is no
// deployment figure to have.
//
// Same three rules as the stages above: assert a ceiling, print the number,
// name the profile. Every ceiling is one to two orders of magnitude above the
// unoptimised figure, so only a change of complexity class trips it. Each
// test asserts its premise — that the workload actually ran the number of
// items it claims — before it reads a clock, so a test that measured nothing
// cannot print a fast number for it.

const EDGE_CELL: &str = "perf-cell-1";
const EDGE_REGION: &str = "europe-west2";
const EDGE_ENVELOPE_KEY: &[u8] = b"a-performance-envelope-key";
const EDGE_POLICY_KEY: &[u8] = b"a-performance-policy-key";

/// A book quoting 99 / 101 for `symbol`, so the mid is 100 and the touch on
/// either side has a known size for the depth rule.
fn edge_book(symbol: &str) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(symbol), venue(), VenueStatus::Open);
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "500"), (BookSide::Ask, "101", "400")]
            .iter()
            .enumerate()
    {
        let when = start().saturating_add(Duration::from_millis(index as i64));
        state.apply(&MarketMessage::new(
            object(symbol),
            Origin::new(venue(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side: *side,
                price: d(price),
                quantity: d(size),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    Ok(state)
}

/// A strategy whose one rule always holds, so every pass raises the same
/// signal at the same size — the workload is then the pass, not the market.
fn firing_strategy(
    id: &str,
    symbol: &str,
    kind: SignalKind,
    size: &str,
) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(symbol), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            kind,
            Expr::Flag(true),
            Expr::Exact(d(size)),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// An envelope wide enough that the grant is never what refuses over a long
/// loop: a hundred million gross, a hundred thousand per order.
fn edge_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            EDGE_CELL,
            dec!("100000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
            start(),
            start().saturating_add(Duration::from_hours(1)),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(EDGE_ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(
        build(&signature)?,
        EDGE_ENVELOPE_KEY,
        EDGE_CELL,
        start().saturating_add(Duration::from_secs(1)),
    )
}

/// A cell holding the ACME book and one always-firing strategy per entry,
/// each priced as given.
fn edge_cell(strategies: &[(&str, SignalKind, &str, PricingPolicy)]) -> Result<Cell> {
    let config = CellConfig::new(EDGE_CELL, EDGE_REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.track(edge_book("ACME")?);
    for (id, kind, size, pricing) in strategies {
        let (compiled, program) = firing_strategy(id, "ACME", *kind, size)?;
        cell.deploy_with_pricing(compiled, program, edge_envelope(id)?, *pricing)?;
    }
    Ok(cell)
}

/// The paper venue these passes run against.
///
/// It is the venue, so what it reports on the order-entry channel is the
/// venue's own answer: with `fills` set, every accepted order is reported
/// filled in full at its limit on the next drain, which is what lets a
/// thousand passes settle rather than pile up under `MAX_OPEN_ORDERS`.
/// Without it, an order rests until the cell withdraws it, which is the
/// expiry path. Either way it has a cancel path, so the cell lets an order
/// rest at all.
#[derive(Debug, Default)]
struct PaperVenue {
    fills: bool,
    pending: Vec<ExecutionReport>,
    resting: BTreeMap<String, Decimal>,
    accepted: usize,
    cancelled: usize,
}

impl Placer for PaperVenue {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        self.accepted += 1;
        if self.fills {
            self.pending.push(ExecutionReport {
                order_id: order_id.to_string(),
                venue: venue.clone(),
                quantity,
                price,
                at,
            });
        } else {
            self.resting.insert(order_id.to_string(), quantity);
        }
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.pending)
    }

    fn can_cancel(&self) -> bool {
        true
    }

    fn cancel(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _at: Timestamp,
    ) -> Result<Decimal> {
        let remaining = self
            .resting
            .remove(order_id)
            .ok_or_else(|| Error::not_found(format!("no order {order_id} is resting")))?;
        self.cancelled += 1;
        Ok(remaining)
    }
}

/// What a run of passes produced, and what the two timed seams cost.
#[derive(Debug, Default)]
struct PassTotals {
    orders: usize,
    contributors: usize,
    fills: usize,
    crosses: usize,
    cancelled: usize,
    refusals: usize,
    /// `Cell::work` alone: confirm, expire, evaluate, gate, net, cross, send.
    work: WallDuration,
    /// The drop copy observed and reconciled, and closed orders settled.
    reconcile: WallDuration,
}

/// Run `passes` passes `step` apart, reconciling the drop copy after each.
///
/// The drop copy is the venue's other channel; here it agrees with what the
/// order-entry channel reported, so every pass's comparison is clean and
/// every closed order is settled. A break would halt the cell, and a halted
/// cell sends nothing, so the assertion is in the loop rather than after it:
/// a hundred quiet passes after a halt would otherwise read as fast.
fn run_passes(
    cell: &mut Cell,
    gateway: &mut PaperVenue,
    passes: usize,
    step: Duration,
) -> Result<PassTotals> {
    let mut totals = PassTotals::default();
    let mut now = start().saturating_add(Duration::from_secs(2));
    for pass in 0..passes {
        let began = Instant::now();
        let report = cell.work(now, gateway)?;
        totals.work += began.elapsed();

        let began = Instant::now();
        for fill in &report.fills {
            cell.observe_drop_copy(DropCopyFill {
                order_id: fill.order_id.clone(),
                venue: fill.venue.clone(),
                quantity: fill.quantity,
                price: fill.price,
                at: now,
            });
        }
        let breaks = cell.reconcile(now);
        totals.reconcile += began.elapsed();

        assert!(
            breaks.is_empty(),
            "pass {pass}: the drop copy disagreed with the order-entry channel: {breaks:?}"
        );
        assert!(!cell.is_halted(), "pass {pass}: the cell halted");
        totals.orders += report.orders.len();
        totals.contributors += report
            .orders
            .iter()
            .map(|order| order.contributors.len())
            .sum::<usize>();
        totals.fills += report.fills.len();
        totals.crosses += report.crosses.len();
        totals.cancelled += report.cancelled.len();
        totals.refusals += report.refusals.len();
        now = now.saturating_add(step);
    }
    Ok(totals)
}

/// The risk state every central submission is judged against: ten million of
/// equity, two positions, plenty of room. Constant across the loop because
/// the manager does not move it — the kernel does, between cycles.
fn central_state() -> RiskState {
    RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        gross_exposure: dec!("4000000"),
        net_exposure: dec!("1000000"),
        position_notionals: BTreeMap::from([
            ("obj-ACME".to_string(), dec!("400000")),
            ("obj-BOREAS".to_string(), dec!("600000")),
        ]),
        ..RiskState::default()
    }
}

fn central_order(manager: &mut OrderManager, symbol: &str, quantity: Decimal) -> Order {
    let order_id = manager.next_order_id("perf");
    Order::new(
        order_id,
        object(symbol),
        Side::Buy,
        quantity,
        OrderType::Market,
        dec!("100"),
        "prop-performance",
        vec!["hyp-performance".to_string()],
        "performance",
        start(),
    )
}

#[test]
fn central_order_submission_costs_what_the_execution_measurements_say() -> Result<()> {
    // The single path to a venue on the central plane: validate, the kill
    // switch, the autonomy level, five pre-trade limits, the state machine,
    // and the simulated venue's fill. Frictionless settings so the venue
    // rejects nothing and fills everything: the workload is then the manager,
    // not a coin the simulator flips.
    const ORDERS: usize = 20_000;
    let mut manager = OrderManager::new(PreTradeChecker::new(risk_limits()));
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 0xC0DE);
    let autonomy = AutonomyController::new();
    let state = central_state();
    let axes = BTreeMap::from([("sector".to_string(), "information_technology".to_string())]);

    let mut accepted = 0usize;
    let mut filled = 0usize;
    let started = Instant::now();
    for _ in 0..ORDERS {
        let order = central_order(&mut manager, "ACME", dec!("1000"));
        let result = manager.submit(
            order,
            &mut broker,
            &autonomy,
            &state,
            axes.clone(),
            Some("broker-a".to_string()),
            start(),
        );
        if result.accepted {
            accepted += 1;
            filled += result.fills.len();
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        accepted,
        ORDERS,
        "the fixture was refused, so the submission path did not run: {:?}",
        manager.refusals().first()
    );
    assert_eq!(
        filled, ORDERS,
        "the frictionless venue did not fill every order"
    );
    assert!(
        !manager.has_live_fills(),
        "a simulated venue produced a fill marked real"
    );
    report(
        "central OMS submit (validate + 5 limits + simulated fill)",
        ORDERS,
        elapsed,
        500.0,
    );
    Ok(())
}

#[test]
fn central_order_submission_costs_the_same_per_order_however_many_the_manager_already_holds()
-> Result<()> {
    // The property the ceiling above is a proxy for, asserted directly on the
    // axis that grows: `OrderManager` keeps every order it has taken in a
    // `BTreeMap` and every refusal in a `Vec`, and a desk session is one
    // manager for the whole session. A submission path that consulted its own
    // history — a scan for a duplicate, a re-projection of every open order
    // into the risk state — would cost more per order the longer the session
    // ran, and the deployed symptom is the one the kernel already produced
    // once: fine on the bench, degrading with uptime, taken out of rotation by
    // its own readiness probe hours in.
    //
    // Load-invariant where the ceiling is not: both halves run on this machine
    // moments apart, so contention divides out of the ratio. The ceiling above
    // stays because it is also the published figure in
    // `docs/ops/execution-measurements.md`; this asserts the complexity class
    // the figure is only a proxy for.
    const SMALL: usize = 5_000;
    const LARGE: usize = 25_000;

    let submit_all = |orders: usize| -> Result<(WallDuration, usize, usize)> {
        let mut manager = OrderManager::new(PreTradeChecker::new(risk_limits()));
        let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 0xC0DE);
        let autonomy = AutonomyController::new();
        let state = central_state();
        let axes = BTreeMap::from([("sector".to_string(), "information_technology".to_string())]);

        let mut accepted = 0usize;
        let mut filled = 0usize;
        let started = Instant::now();
        for _ in 0..orders {
            let order = central_order(&mut manager, "ACME", dec!("1000"));
            let result = manager.submit(
                order,
                &mut broker,
                &autonomy,
                &state,
                axes.clone(),
                Some("broker-a".to_string()),
                start(),
            );
            if result.accepted {
                accepted += 1;
                filled += result.fills.len();
            }
        }
        Ok((started.elapsed(), accepted, filled))
    };

    let (small_elapsed, small_accepted, small_filled) = submit_all(SMALL)?;
    let (large_elapsed, large_accepted, large_filled) = submit_all(LARGE)?;

    // Premise: both runs really submitted and really filled. A run refused at
    // the first gate costs nothing per order and would pass this trivially.
    assert_eq!(small_accepted, SMALL, "the small run was refused");
    assert_eq!(large_accepted, LARGE, "the large run was refused");
    assert_eq!(small_filled, SMALL, "the small run did not fill");
    assert_eq!(large_filled, LARGE, "the large run did not fill");

    report_scaling(
        "central OMS submit scaling",
        (SMALL, small_elapsed),
        (LARGE, large_elapsed),
        2.0,
    );
    Ok(())
}

#[test]
fn central_instrument_feasibility_costs_what_the_execution_measurements_say() -> Result<()> {
    // The grid installed through `with_instrument_feasibility`, judged where
    // it sits in the submission path — ahead of the safety controls, so an
    // order the venue cannot express spends nothing downstream. Half the
    // orders are off-lot so the measurement covers both the refusal and the
    // admission; a fixture that was all one or the other would time only
    // the cheaper branch.
    const ORDERS: usize = 20_000;
    let grid = VenueFeasibility::new(dec!("1"), Some(dec!("0.01")), Decimal::ZERO, Decimal::ZERO)?;
    let mut manager = OrderManager::new(PreTradeChecker::new(risk_limits()))
        .with_instrument_feasibility("obj-ACME", grid);
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 0xFEA5);
    let autonomy = AutonomyController::new();
    let state = central_state();

    let mut admitted = 0usize;
    let mut refused_on_lot = 0usize;
    let started = Instant::now();
    for index in 0..ORDERS {
        let quantity = if index % 2 == 0 {
            dec!("1000")
        } else {
            dec!("1000.5")
        };
        let order = central_order(&mut manager, "ACME", quantity);
        let result = manager.submit(
            order,
            &mut broker,
            &autonomy,
            &state,
            BTreeMap::new(),
            None,
            start(),
        );
        if result.accepted {
            admitted += 1;
        } else if result
            .refusal
            .as_ref()
            .and_then(RefusalReason::feasibility_gate)
            == Some(central_feasibility::GATE_LOT)
        {
            refused_on_lot += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        refused_on_lot,
        ORDERS / 2,
        "the off-lot half was not refused under the lot rule: {:?}",
        manager.refusals().first()
    );
    assert_eq!(admitted, ORDERS / 2, "the on-lot half was not admitted");
    report(
        "central instrument feasibility (half off-lot)",
        ORDERS,
        elapsed,
        500.0,
    );
    Ok(())
}

#[test]
fn an_edge_work_pass_with_a_fill_and_its_drop_copy_costs_what_the_execution_measurements_say()
-> Result<()> {
    // One pass of the cell's loop, end to end, with one strategy that fires
    // every pass: the venue's reports confirmed, the degradation table read,
    // the strategy evaluated, the intent gated, the region hold taken, the
    // net built, the order sent, the hold committed. Then the pass's other
    // half, timed separately: the drop copy observed and reconciled against
    // the confirmed fill, and the closed order settled. The region table is
    // wired so the hold-and-commit path is inside the number.
    const PASSES: usize = 2_000;
    let opening = dec!("1000000000");
    let table = RegionTable::new(opening)?;
    let mut cell = edge_cell(&[("alpha", SignalKind::Enter, "100", PricingPolicy::Marketable)])?
        .with_region_table(table.clone());
    let mut gateway = PaperVenue {
        fills: true,
        ..PaperVenue::default()
    };

    let totals = run_passes(&mut cell, &mut gateway, PASSES, Duration::from_millis(1))?;

    assert_eq!(totals.orders, PASSES, "not every pass sent its order");
    assert_eq!(
        gateway.accepted, PASSES,
        "the venue did not see every order"
    );
    // The venue reports the fill on acceptance and the cell confirms it in
    // the same pass — so every pass confirms its own fill, and after the
    // reconcile that follows nothing is left open.
    assert_eq!(
        totals.fills, PASSES,
        "the venue's acceptance-time fills were not all confirmed"
    );
    assert!(
        cell.open_orders().is_empty(),
        "settled orders were not retired: {} still open",
        cell.open_orders().len()
    );
    assert!(
        table.committed_total().is_positive() && table.free() < opening,
        "no region hold was taken and committed on the order path"
    );
    assert_eq!(totals.refusals, 0, "a gate refused inside the loop");
    report(
        "edge work pass (1 strategy, marketable, region table)",
        PASSES,
        totals.work,
        5_000.0,
    );
    report(
        "edge drop-copy reconcile + settle (1 fill)",
        PASSES,
        totals.reconcile,
        1_000.0,
    );
    Ok(())
}

#[test]
fn an_edge_work_pass_costs_the_same_per_pass_however_many_passes_the_cell_has_already_run()
-> Result<()> {
    // The property both ceilings above are a proxy for. A cell is the one
    // process here that is meant to run for weeks without the centre — ADR
    // 0008, a cell that cannot reach the centre keeps working — so its pass
    // cost is the number that must not depend on how long it has been up. The
    // seams that grow across a session are the hash-chained journal, the
    // per-strategy positions and the settled-order history; a pass that
    // rescanned any of them would be flat on the bench and ruinous on day
    // three, which is exactly how the kernel's history regression reached a
    // deployed node.
    //
    // Both halves run on this machine moments apart, so a busy machine cannot
    // explain a failure here the way it can explain the ceilings above.
    const SMALL: usize = 500;
    const LARGE: usize = 2_500;

    let passes = |count: usize| -> Result<PassTotals> {
        let opening = dec!("1000000000");
        let table = RegionTable::new(opening)?;
        let mut cell =
            edge_cell(&[("alpha", SignalKind::Enter, "100", PricingPolicy::Marketable)])?
                .with_region_table(table);
        let mut gateway = PaperVenue {
            fills: true,
            ..PaperVenue::default()
        };
        run_passes(&mut cell, &mut gateway, count, Duration::from_millis(1))
    };

    let small = passes(SMALL)?;
    let large = passes(LARGE)?;

    // Premise: every pass in both runs sent its order and confirmed its fill.
    // A cell that quietly stopped trading runs cheap passes, and a cheap pass
    // measured against a working one is the failure mode this assertion would
    // otherwise be blind to.
    assert_eq!(
        small.orders, SMALL,
        "the short run did not place every pass"
    );
    assert_eq!(large.orders, LARGE, "the long run did not place every pass");
    assert_eq!(small.fills, SMALL, "the short run confirmed no fill");
    assert_eq!(large.fills, LARGE, "the long run confirmed no fill");
    assert_eq!(small.refusals, 0, "a gate refused inside the short run");
    assert_eq!(large.refusals, 0, "a gate refused inside the long run");

    report_scaling(
        "edge work pass scaling",
        (SMALL, small.work),
        (LARGE, large.work),
        2.0,
    );
    report_scaling(
        "edge drop-copy reconcile + settle scaling",
        (SMALL, small.reconcile),
        (LARGE, large.reconcile),
        2.0,
    );
    Ok(())
}

#[test]
fn netting_four_intents_into_one_order_costs_what_the_execution_measurements_say() -> Result<()> {
    // Four strategies that agree, netted into one order carrying four
    // contributors. The premise is the contributor count: a cell that sent
    // four orders would be four times the venue traffic and read here as a
    // slower pass, but the property under measurement is that it sent one.
    const PASSES: usize = 1_000;
    let mut cell = edge_cell(&[
        ("alpha", SignalKind::Enter, "100", PricingPolicy::Marketable),
        ("beta", SignalKind::Enter, "50", PricingPolicy::Marketable),
        ("gamma", SignalKind::Enter, "30", PricingPolicy::Marketable),
        ("delta", SignalKind::Enter, "20", PricingPolicy::Marketable),
    ])?;
    let mut gateway = PaperVenue {
        fills: true,
        ..PaperVenue::default()
    };

    let totals = run_passes(&mut cell, &mut gateway, PASSES, Duration::from_millis(1))?;

    assert_eq!(
        totals.orders, PASSES,
        "four agreeing intents did not net to one order"
    );
    assert_eq!(
        totals.contributors,
        4 * PASSES,
        "the net order does not carry all four contributors"
    );
    assert_eq!(totals.crosses, 0, "agreeing intents crossed");
    assert_eq!(totals.refusals, 0, "a gate refused inside the loop");
    report(
        "edge netting (4 intents -> 1 order, per pass)",
        PASSES,
        totals.work,
        10_000.0,
    );
    Ok(())
}

#[test]
fn an_internal_cross_costs_what_the_execution_measurements_say() -> Result<()> {
    // A hundred against forty, every pass: forty crosses inside the cell at
    // the mid and sixty goes to the venue. Under the per-net cap (forty of a
    // hundred and forty gross), so the cross is admitted on every pass and
    // `book_cross` moves both strategies' lots and cash each time.
    const PASSES: usize = 1_000;
    let mut cell = edge_cell(&[
        ("alpha", SignalKind::Enter, "100", PricingPolicy::Marketable),
        ("beta", SignalKind::Exit, "40", PricingPolicy::Marketable),
    ])?;
    let mut gateway = PaperVenue {
        fills: true,
        ..PaperVenue::default()
    };

    let totals = run_passes(&mut cell, &mut gateway, PASSES, Duration::from_millis(1))?;

    assert_eq!(
        totals.crosses, PASSES,
        "the offsetting portion did not cross every pass"
    );
    assert_eq!(
        totals.orders, PASSES,
        "the residual did not go to the venue every pass"
    );
    assert_eq!(totals.cancelled, 0, "a net cancelled to zero");
    assert_eq!(totals.refusals, 0, "a gate refused inside the loop");
    let alpha = cell.strategy_position(&StrategyId::new("alpha"), &venue(), &object("ACME"));
    let beta = cell.strategy_position(&StrategyId::new("beta"), &venue(), &object("ACME"));
    assert!(
        alpha.is_positive() && beta.is_negative(),
        "the cross moved no lots: alpha {alpha}, beta {beta}"
    );
    report(
        "edge internal cross (net + book_cross + residual order, per pass)",
        PASSES,
        totals.work,
        10_000.0,
    );
    Ok(())
}

#[test]
fn a_resting_orders_expiry_costs_what_the_execution_measurements_say() -> Result<()> {
    // An order rested at the mid for one second, and the next pass two
    // seconds later withdrawing it: `withdraw_expired` through the venue's
    // cancel, the region hold returned, the order closed and settled. The
    // premise is the venue's cancel count — an order the cell forgot rather
    // than withdrew would leave the venue holding it and read here as fast.
    const PASSES: usize = 1_000;
    let mut cell = edge_cell(&[(
        "alpha",
        SignalKind::Enter,
        "100",
        PricingPolicy::rest_at_mid(Duration::from_secs(1))?,
    )])?;
    let mut gateway = PaperVenue::default();

    let totals = run_passes(&mut cell, &mut gateway, PASSES, Duration::from_secs(2))?;

    assert_eq!(totals.orders, PASSES, "not every pass rested its order");
    assert_eq!(
        gateway.cancelled,
        PASSES - 1,
        "the venue was not asked to withdraw every expired order"
    );
    assert_eq!(totals.fills, 0, "a resting order filled on its own");
    assert_eq!(
        cell.open_orders().len(),
        1,
        "expired orders were not settled"
    );
    assert_eq!(totals.refusals, 0, "a gate refused inside the loop");
    report(
        "edge resting order expiry (rest, withdraw, settle, per pass)",
        PASSES,
        totals.work,
        5_000.0,
    );
    Ok(())
}

#[test]
fn the_edge_feasibility_gate_costs_what_the_execution_measurements_say() -> Result<()> {
    // The pure gate the cell judges every intent by before netting, on its
    // own: lot, tick, minimum, depth at the touch, fee floor. Half the intents
    // are off-lot, so both the refusal and the admission are inside the
    // number, and the premise counts each half under the rule that bound.
    const INTENTS: usize = 200_000;
    let model = VenueModel::new(
        VenueClass::Exchange,
        Granularity::new(dec!("1"), dec!("0.01"), Decimal::ZERO)?,
        Decimal::ZERO,
        None,
    )?;
    let intents: Vec<Intent> = (0..INTENTS)
        .map(|index| {
            Intent::new(
                StrategyId::new("alpha"),
                object("ACME"),
                venue(),
                if index % 2 == 0 {
                    dec!("10")
                } else {
                    dec!("10.5")
                },
                dec!("100"),
                start().saturating_add(Duration::from_secs(30)),
            )
        })
        .collect::<Result<_>>()?;

    let mut admitted = 0usize;
    let mut refused_on_lot = 0usize;
    let started = Instant::now();
    for intent in &intents {
        match edge_feasibility::assess(Some(&model), None, intent, Some(dec!("400"))) {
            Ok(()) => admitted += 1,
            Err(infeasible) if infeasible.gate == edge_feasibility::GATE_LOT => {
                refused_on_lot += 1;
            }
            Err(infeasible) => panic!("refused under an unexpected rule: {infeasible:?}"),
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        refused_on_lot,
        INTENTS / 2,
        "the off-lot half was not refused"
    );
    assert_eq!(admitted, INTENTS / 2, "the on-lot half was not admitted");
    report(
        "edge feasibility gate (half off-lot)",
        INTENTS,
        elapsed,
        20.0,
    );
    Ok(())
}

#[test]
fn a_region_reservation_hold_and_commit_costs_what_the_execution_measurements_say() -> Result<()> {
    // The per-region ledger on the order path: one hold taken before the
    // order exists and committed when it goes out, through the mutex every
    // cell in the region shares. The premise is the ledger's own arithmetic —
    // what was committed is exactly what left the free balance.
    const HOLDS: usize = 200_000;
    let opening = dec!("1000000000");
    let table = RegionTable::new(opening)?;
    let amount = dec!("100");

    let mut committed = 0usize;
    let started = Instant::now();
    for pass in 0..HOLDS {
        table.reserve(EDGE_CELL, "perf-hold", amount, pass as u64)?;
        if table.commit(EDGE_CELL, "perf-hold").is_some() {
            committed += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(committed, HOLDS, "a hold was taken and never committed");
    let expected = amount * Decimal::from_int(HOLDS as i64);
    assert_eq!(table.committed_total(), expected);
    assert_eq!(table.free(), opening - expected);
    assert_eq!(
        table.held_total(),
        Decimal::ZERO,
        "a hold is still standing"
    );
    report(
        "region reservation (reserve + commit)",
        HOLDS,
        elapsed,
        20.0,
    );
    Ok(())
}

#[test]
fn sequencing_a_contiguous_stream_costs_what_the_execution_measurements_say() -> Result<()> {
    // The sequencer on the feed path: every message checked against the
    // stream's position and released in order. Contiguous on purpose — the
    // common case is the one every packet pays for, and a gap's cost is a
    // property of the reorder policy rather than of the tracker.
    const MESSAGES: usize = 200_000;
    const BATCH: usize = 100;
    let stream = level_stream("ACME", MESSAGES, 0x5E0);
    let mut sequencer = Sequencer::new(ReorderPolicy::default());

    let mut released = 0usize;
    let mut events: Vec<SequenceEvent> = Vec::new();
    let started = Instant::now();
    for chunk in stream.chunks(BATCH) {
        let batch = sequencer.accept(chunk.to_vec(), start());
        released += batch.released.len();
        events.extend(batch.events);
    }
    let elapsed = started.elapsed();

    assert_eq!(
        released, MESSAGES,
        "the sequencer held back messages of a contiguous stream"
    );
    assert!(
        events
            .iter()
            .all(|event| matches!(event, SequenceEvent::StreamStarted { .. })),
        "a contiguous stream produced a gap or a duplicate: {events:?}"
    );
    assert_eq!(events.len(), 1, "the stream started more than once");
    report(
        "sequencing (contiguous, batches of 100)",
        MESSAGES,
        elapsed,
        20.0,
    );
    Ok(())
}

#[test]
fn sequencing_costs_the_same_per_message_however_long_the_stream_has_been_running() -> Result<()> {
    // The property the ceiling above is a proxy for, and the one the data
    // domain's rule states outright: bounded retention, always. A sequencer
    // holds per-stream position and a reorder buffer, and both are on the feed
    // path of a process that never restarts on purpose. If either grew with
    // the number of messages already seen — a buffer nothing drains, a seen-set
    // that only ever gains members — per-message cost would rise with uptime,
    // and a contiguous stream is precisely the case where nothing should ever
    // be retained at all.
    //
    // Load-invariant: both halves on this machine, moments apart.
    const SMALL: usize = 50_000;
    const LARGE: usize = 250_000;
    const BATCH: usize = 100;

    let run = |messages: usize| -> (WallDuration, usize, Vec<SequenceEvent>) {
        let stream = level_stream("ACME", messages, 0x5E0);
        let mut sequencer = Sequencer::new(ReorderPolicy::default());
        let mut released = 0usize;
        let mut events: Vec<SequenceEvent> = Vec::new();
        let started = Instant::now();
        for chunk in stream.chunks(BATCH) {
            let batch = sequencer.accept(chunk.to_vec(), start());
            released += batch.released.len();
            events.extend(batch.events);
        }
        (started.elapsed(), released, events)
    };

    let (small_elapsed, small_released, small_events) = run(SMALL);
    let (large_elapsed, large_released, large_events) = run(LARGE);

    // Premise: both runs released every message. A sequencer holding messages
    // back does less work per message, not more, and would pass this while
    // being broken.
    assert_eq!(small_released, SMALL, "the short stream was held back");
    assert_eq!(large_released, LARGE, "the long stream was held back");
    assert_eq!(small_events.len(), 1, "the short stream was not contiguous");
    assert_eq!(large_events.len(), 1, "the long stream was not contiguous");

    report_scaling(
        "sequencing scaling",
        (SMALL, small_elapsed),
        (LARGE, large_elapsed),
        2.0,
    );
    Ok(())
}

#[test]
fn arbitrating_two_redundant_lines_costs_what_the_execution_measurements_say() -> Result<()> {
    // Two lines carrying the same stream, the A line always first: every
    // unit is published once from A and recognised as a duplicate from B.
    // The premise is both counts, so a line the arbiter silently dropped —
    // which would halve the work — cannot read as fast.
    const MESSAGES: usize = 100_000;
    const BATCH: usize = 100;
    let stream = level_stream("ACME", MESSAGES, 0xA5B);
    // The window must be wider than a batch: line B's copy of a unit arrives
    // a whole batch after line A's, and a unit that has already left the
    // window is a `Missed`, not a duplicate.
    let mut arbiter = LineArbiter::new("feed-a", &["line-a", "line-b"], 4 * BATCH);

    let mut released = 0usize;
    let mut published = 0usize;
    let mut duplicates = 0usize;
    let started = Instant::now();
    for chunk in stream.chunks(BATCH) {
        for line in ["line-a", "line-b"] {
            let outcome = arbiter.accept(line, chunk.to_vec(), start());
            released += outcome.released.len();
            for event in &outcome.events {
                match event {
                    ArbitrationEvent::Published { .. } => published += 1,
                    ArbitrationEvent::Duplicate { .. } => duplicates += 1,
                    other => panic!("two clean lines produced {other:?}"),
                }
            }
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(released, MESSAGES, "the merged stream is not the stream");
    assert_eq!(
        published, MESSAGES,
        "not every unit was published exactly once"
    );
    assert_eq!(
        duplicates, MESSAGES,
        "the second line's copies were not all recognised"
    );
    report(
        "line arbitration (2 lines, per delivered unit)",
        2 * MESSAGES,
        elapsed,
        20.0,
    );
    Ok(())
}

#[test]
fn line_arbitration_costs_the_same_per_unit_however_long_the_lines_have_been_running() -> Result<()>
{
    // The arbiter's window is the bounded-retention claim in the one place a
    // duplicate suppressor is most tempted to break it: to recognise line B's
    // copy of a unit it must remember line A's, and the cheap way to be always
    // right is to remember every unit forever. That is an unbounded working set
    // on the feed path — prohibited outright by the data domain's rule — and it
    // does not fail, it degrades, which is why a ceiling on a fixed message
    // count cannot see it. Feeding five times the stream and asserting the
    // per-unit cost did not rise is the assertion that can.
    const SMALL: usize = 25_000;
    const LARGE: usize = 125_000;
    const BATCH: usize = 100;

    let run = |messages: usize| -> (WallDuration, usize, usize, usize) {
        let stream = level_stream("ACME", messages, 0xA5B);
        // Wider than a batch, for the reason the ceiling test above gives: B's
        // copy arrives a whole batch after A's, and a unit that has left the
        // window is a `Missed` rather than a duplicate.
        let mut arbiter = LineArbiter::new("feed-a", &["line-a", "line-b"], 4 * BATCH);
        let mut released = 0usize;
        let mut published = 0usize;
        let mut duplicates = 0usize;
        let started = Instant::now();
        for chunk in stream.chunks(BATCH) {
            for line in ["line-a", "line-b"] {
                let outcome = arbiter.accept(line, chunk.to_vec(), start());
                released += outcome.released.len();
                for event in &outcome.events {
                    match event {
                        ArbitrationEvent::Published { .. } => published += 1,
                        ArbitrationEvent::Duplicate { .. } => duplicates += 1,
                        other => panic!("two clean lines produced {other:?}"),
                    }
                }
            }
        }
        (started.elapsed(), released, published, duplicates)
    };

    let (small_elapsed, small_released, small_published, small_duplicates) = run(SMALL);
    let (large_elapsed, large_released, large_published, large_duplicates) = run(LARGE);

    // Premise: both runs published every unit once and recognised the second
    // line's copy of every one. An arbiter that dropped a line does half the
    // work and would read here as flat.
    assert_eq!(small_released, SMALL, "the short merge lost units");
    assert_eq!(large_released, LARGE, "the long merge lost units");
    assert_eq!(small_published, SMALL, "the short run republished");
    assert_eq!(large_published, LARGE, "the long run republished");
    assert_eq!(
        small_duplicates, SMALL,
        "the short run missed line B's copies"
    );
    assert_eq!(
        large_duplicates, LARGE,
        "the long run missed line B's copies"
    );

    report_scaling(
        "line arbitration scaling",
        (2 * SMALL, small_elapsed),
        (2 * LARGE, large_elapsed),
        2.0,
    );
    Ok(())
}

#[test]
fn verifying_a_capital_envelope_costs_what_the_execution_measurements_say() -> Result<()> {
    // The signature check every grant passes before a cell will deploy on it:
    // the HMAC over the signing payload, the constant-time comparison, the
    // cell and the validity window. The admission arithmetic behind it is the
    // capital stage above; this is the trust root in front of it.
    const ENVELOPES: usize = 20_000;
    let envelope = {
        let build = |signature: &str| {
            CapitalEnvelope::new(
                StrategyId::new("alpha"),
                EDGE_CELL,
                dec!("1000000"),
                dec!("100000"),
                dec!("50000"),
                vec![venue()],
                start(),
                start().saturating_add(Duration::from_hours(1)),
                "alice@example.com",
                signature,
            )
        };
        let unsigned = build("unsigned")?;
        build(&sign_payload(
            EDGE_ENVELOPE_KEY,
            &unsigned.signing_payload(),
        ))?
    };
    let now = start().saturating_add(Duration::from_secs(1));

    let mut verified = 0usize;
    let started = Instant::now();
    for _ in 0..ENVELOPES {
        if VerifiedEnvelope::verify(envelope.clone(), EDGE_ENVELOPE_KEY, EDGE_CELL, now).is_ok() {
            verified += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        verified, ENVELOPES,
        "a correctly signed envelope was refused"
    );
    assert!(
        VerifiedEnvelope::verify(envelope, b"another-key", EDGE_CELL, now).is_err(),
        "the check under measurement accepts any key"
    );
    report("capital envelope verify (HMAC)", ENVELOPES, elapsed, 500.0);
    Ok(())
}

#[test]
fn verifying_and_applying_a_policy_payload_costs_what_the_execution_measurements_say() -> Result<()>
{
    // The centre's payload arriving at the cell: the signature over the
    // canonical serialisation, the anti-replay sequence, the halt barrier,
    // the narrowing recorded, the chain entry sealed. One per sequence,
    // strictly increasing, which is the only order the cell accepts.
    const PAYLOADS: usize = 2_000;
    let mut cell = edge_cell(&[])?;
    let issued = start().saturating_add(Duration::from_secs(2));
    let payloads: Vec<PolicyPayload> = (1..=PAYLOADS as u64)
        .map(|sequence| {
            PolicyPayload::unproduced(sequence, EDGE_CELL, issued).signed(EDGE_POLICY_KEY)
        })
        .collect::<Result<_>>()?;

    let mut applied = 0usize;
    let started = Instant::now();
    for payload in payloads {
        let verified = VerifiedPolicy::verify(payload, EDGE_POLICY_KEY, EDGE_CELL, issued)?;
        cell.apply_policy(verified, issued)?;
        applied += 1;
    }
    let elapsed = started.elapsed();

    assert_eq!(applied, PAYLOADS);
    assert_eq!(
        cell.policy_sequence(),
        Some(PAYLOADS as u64),
        "the cell did not apply every payload in sequence"
    );
    report("policy payload verify + apply", PAYLOADS, elapsed, 2_000.0);
    Ok(())
}

#[test]
fn applying_a_policy_payload_costs_the_same_per_payload_however_many_the_cell_has_applied()
-> Result<()> {
    // The centre publishes to a cell for as long as the cell lives, so the
    // anti-replay sequence only ever goes up and the narrowing chain only ever
    // gets longer. The property is that applying the ten-thousandth payload
    // costs what the first did. A cell that rescanned its applied history to
    // decide whether a sequence was a replay — the obvious wrong way to make
    // replay protection airtight — would pass a ceiling measured over two
    // thousand payloads and starve a cell that had been up for a week.
    //
    // Both halves run on this machine moments apart, so this fires on the
    // complexity class and not on the machine.
    const SMALL: usize = 500;
    const LARGE: usize = 2_500;

    let apply_all = |count: usize| -> Result<(WallDuration, usize, Option<u64>)> {
        let mut cell = edge_cell(&[])?;
        let issued = start().saturating_add(Duration::from_secs(2));
        let payloads: Vec<PolicyPayload> = (1..=count as u64)
            .map(|sequence| {
                PolicyPayload::unproduced(sequence, EDGE_CELL, issued).signed(EDGE_POLICY_KEY)
            })
            .collect::<Result<_>>()?;

        let mut applied = 0usize;
        let started = Instant::now();
        for payload in payloads {
            let verified = VerifiedPolicy::verify(payload, EDGE_POLICY_KEY, EDGE_CELL, issued)?;
            cell.apply_policy(verified, issued)?;
            applied += 1;
        }
        Ok((started.elapsed(), applied, cell.policy_sequence()))
    };

    let (small_elapsed, small_applied, small_sequence) = apply_all(SMALL)?;
    let (large_elapsed, large_applied, large_sequence) = apply_all(LARGE)?;

    // Premise: every payload was verified and applied in sequence. A cell that
    // refused them all does no work and would read as perfectly flat.
    assert_eq!(small_applied, SMALL);
    assert_eq!(large_applied, LARGE);
    assert_eq!(
        small_sequence,
        Some(SMALL as u64),
        "the short run did not apply every payload"
    );
    assert_eq!(
        large_sequence,
        Some(LARGE as u64),
        "the long run did not apply every payload"
    );

    report_scaling(
        "policy payload verify + apply scaling",
        (SMALL, small_elapsed),
        (LARGE, large_elapsed),
        2.0,
    );
    Ok(())
}

#[test]
fn the_journal_chain_costs_what_the_execution_measurements_say() -> Result<()> {
    // The hash chain under every decision: a record sealed onto the previous
    // digest, the whole chain re-verified, and the unshipped tail handed to a
    // mirror in one batch that names what it chains onto. Three numbers
    // because they are three different costs on three different paths — the
    // record is on the pass, the verify is what a replay pays, the ship is
    // the one call that may block.
    const RECORDS: usize = 50_000;
    let decisions: Vec<Decision> = (0..RECORDS)
        .map(|index| Decision::Refused {
            gate: "performance".to_string(),
            reason: format!("record {index} of a measured chain"),
        })
        .collect();
    let mut journal = Journal::new();

    let started = Instant::now();
    for decision in decisions {
        journal.record(decision, start());
    }
    let recording = started.elapsed();

    let started = Instant::now();
    let verified = journal.verify();
    let verifying = started.elapsed();

    let mut mirror = MemoryMirror::new();
    let started = Instant::now();
    let shipped = ship(&mut journal, &mut mirror, EDGE_CELL, Vec::new(), start())?;
    let shipping = started.elapsed();

    assert_eq!(journal.len(), RECORDS, "not every decision was recorded");
    assert_eq!(
        verified,
        Ok(()),
        "the chain the test just wrote does not verify"
    );
    assert_eq!(shipped, RECORDS, "the mirror did not receive every entry");
    mirror.verify_continuity()?;
    assert!(
        journal.unshipped().is_empty(),
        "entries were shipped and not marked"
    );
    report("journal record (chain digest)", RECORDS, recording, 200.0);
    report("journal verify (per entry)", RECORDS, verifying, 200.0);
    report(
        "journal ship to mirror (per entry)",
        RECORDS,
        shipping,
        50.0,
    );
    Ok(())
}

#[test]
fn the_journal_chain_costs_the_same_per_entry_however_long_the_chain_gets() -> Result<()> {
    // The three ceilings above, as the property they stand for. A hash chain
    // is the one structure where the linear implementation is both obvious and
    // catastrophic: seal an entry by hashing what came before it and recording
    // is O(chain); re-derive the tail digest on every ship and shipping is too.
    // Both are correct, both pass every functional test in the workspace, and
    // both make a cell that has been up a day cost a hundred times what the
    // measurement said. Nothing in this repository can replay a decision
    // without this chain, so it is not a structure that may quietly degrade.
    //
    // Two assertions, not three, and the third is the interesting one.
    //
    // The record is on the pass and the verify is what a replay pays; both
    // ratios are stable and both are asserted. The ship is measured and
    // deliberately **not** asserted: at these sizes it is a few milliseconds
    // whose cost is dominated by the mirror allocating its own copy of the
    // tail, not by anything algorithmic, and across three consecutive
    // full-suite runs on an idle machine it printed 1.40x, 1.63x and 2.02x
    // for the same code. An assertion whose noise is the size of the effect
    // it claims to detect is not a loose control, it is a control that fires
    // on the wrong thing — and widening the tolerance until the noise fits
    // under it is how this file nearly ended up with twenty-seven ceilings
    // that could not fire at all. So the number is printed for a reader and
    // the claim is not made. Making it assertable needs a fixture that ships
    // a fixed batch out of chains of two different lengths, which is a
    // different fixture from this one.
    const SHORT: usize = 10_000;
    const LONG: usize = 50_000;

    struct ChainTiming {
        recording: WallDuration,
        verifying: WallDuration,
        shipping: WallDuration,
        recorded: usize,
        shipped: usize,
        verified: bool,
    }

    let chain = |records: usize| -> Result<ChainTiming> {
        let decisions: Vec<Decision> = (0..records)
            .map(|index| Decision::Refused {
                gate: "performance".to_string(),
                reason: format!("record {index} of a measured chain"),
            })
            .collect();
        let mut journal = Journal::new();

        let started = Instant::now();
        for decision in decisions {
            journal.record(decision, start());
        }
        let recording = started.elapsed();

        let started = Instant::now();
        let verified = journal.verify();
        let verifying = started.elapsed();

        let mut mirror = MemoryMirror::new();
        let started = Instant::now();
        let shipped = ship(&mut journal, &mut mirror, EDGE_CELL, Vec::new(), start())?;
        let shipping = started.elapsed();

        Ok(ChainTiming {
            recording,
            verifying,
            shipping,
            recorded: journal.len(),
            shipped,
            verified: verified == Ok(()),
        })
    };

    let short = chain(SHORT)?;
    let long = chain(LONG)?;

    // Premise: both chains were really written, really verified and really
    // shipped. A chain that stopped early is cheap per entry in exactly the
    // direction that would make this pass.
    assert_eq!(short.recorded, SHORT, "the short chain lost entries");
    assert_eq!(long.recorded, LONG, "the long chain lost entries");
    assert!(short.verified, "the short chain does not verify");
    assert!(long.verified, "the long chain does not verify");
    assert_eq!(short.shipped, SHORT, "the short chain was not all shipped");
    assert_eq!(long.shipped, LONG, "the long chain was not all shipped");

    report_scaling(
        "journal record scaling",
        (SHORT, short.recording),
        (LONG, long.recording),
        2.0,
    );
    report_scaling(
        "journal verify scaling",
        (SHORT, short.verifying),
        (LONG, long.verifying),
        2.0,
    );
    println!(
        "journal ship scaling (printed, not asserted — see the comment above): \
         {SHORT} ops at {:.3} us/op vs {LONG} ops at {:.3} us/op ({} profile, this machine, \
         single-threaded)",
        short.shipping.as_secs_f64() * 1e6 / SHORT as f64,
        long.shipping.as_secs_f64() * 1e6 / LONG as f64,
        profile()
    );
    Ok(())
}

#[test]
fn a_two_leg_group_completing_costs_what_the_execution_measurements_say() -> Result<()> {
    // The multi-leg lifecycle: two orders assembled into a group, each leg
    // filled in full, the group assessed and settled complete. Buy and sell
    // of equal notional, so the leg risk between the fills is bounded by the
    // group's own limit and the verdict is completion rather than an unwind.
    const GROUPS: usize = 20_000;
    let fill = |order: &Order, index: usize| Fill {
        fill_id: FillId::from_string(format!("fill-{index}-{}", order.order_id.as_str())),
        order_id: order.order_id.clone(),
        at: start(),
        quantity: order.quantity,
        price: order.arrival_price,
        costs: Decimal::ZERO,
        venue: "simulated-venue".to_string(),
        simulated: true,
    };

    let mut complete = 0usize;
    let started = Instant::now();
    for index in 0..GROUPS {
        let leg = |symbol: &str, side: Side| {
            Order::new(
                OrderId::from_string(format!("leg-{index}-{symbol}")),
                object(symbol),
                side,
                dec!("100"),
                OrderType::Market,
                dec!("100"),
                "prop-performance",
                vec!["hyp-performance".to_string()],
                "performance",
                start(),
            )
        };
        let buy = leg("ACME", Side::Buy);
        let sell = leg("BOREAS", Side::Sell);
        let buy_fill = fill(&buy, index);
        let sell_fill = fill(&sell, index);
        let mut group = LegGroup::new(
            format!("group-{index}"),
            vec![buy, sell],
            start().saturating_add(Duration::from_secs(60)),
            dec!("1000000"),
        )?;
        group.record_fill(&buy_fill)?;
        group.record_fill(&sell_fill)?;
        let verdict = group.assess(start());
        group.settle(&verdict, &[], start())?;
        if verdict == Verdict::Complete {
            complete += 1;
        }
    }
    let elapsed = started.elapsed();

    assert_eq!(
        complete, GROUPS,
        "a fully filled group was not judged complete"
    );
    report(
        "multi-leg group (2 legs: assemble, fill, assess, settle)",
        GROUPS,
        elapsed,
        200.0,
    );
    Ok(())
}

// --- the second honesty check ------------------------------------------------

/// The test-name cell of every figure row in the execution measurements
/// document, with its backticks stripped.
fn measurement_rows(document: &str) -> Vec<String> {
    document
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let cells: Vec<&str> = line.strip_prefix('|')?.split('|').collect();
            let capability = cells.first()?.trim().to_lowercase();
            if capability.is_empty()
                || capability == "capability"
                || capability.chars().all(|c| c == '-' || c == ':')
            {
                return None;
            }
            let test = cells.get(5)?.trim().trim_matches('`').to_string();
            Some(test)
        })
        .collect()
}

#[test]
fn the_execution_measurements_document_names_only_tests_this_file_holds_and_says_what_a_number_is_not()
 {
    // The document is the deliverable this section justifies, so it is
    // checked rather than trusted, in both directions: every row names a
    // test this file holds, and every measurement this file makes has a row.
    // A row for a test that no longer exists is a figure nothing produces —
    // exactly what the budgets document once did for a stage ADR 0029 had
    // removed — and a measurement without a row is a number nobody can find.
    let document = qip_acceptance::read("docs/ops/execution-measurements.md");
    let source = qip_acceptance::read("backend/crates/tests/qip-acceptance/tests/performance.rs");
    const SUFFIX: &str = "_costs_what_the_execution_measurements_say";

    let measuring: Vec<&str> = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("fn "))
        .filter_map(|rest| rest.split('(').next())
        .filter(|name| name.ends_with(SUFFIX))
        .collect();
    // Fifteen as of 2026-09-05, one per execution capability the traceability
    // document names. Raised from ten when the halt wire was measured: a
    // floor left below the count it was written from stops being a guard
    // against the section being cut down, which is the only thing it is for.
    assert!(
        measuring.len() >= 15,
        "this file holds only {} execution measurements; the section was cut down",
        measuring.len()
    );

    let rows = measurement_rows(&document);
    assert!(!rows.is_empty(), "the document has no figure rows");
    for row in &rows {
        assert!(
            measuring.contains(&row.as_str()),
            "docs/ops/execution-measurements.md publishes a row for `{row}`, which this file \
             does not measure; a figure for a test nothing runs is a wish dressed as a \
             measurement"
        );
    }
    for name in &measuring {
        assert!(
            rows.iter().any(|row| row == name),
            "docs/ops/execution-measurements.md has no row for `{name}`"
        );
    }

    // And the caveats a reader has to meet before any number: the shape of
    // the machine, the profile, and the two sentences that stop an in-process
    // figure being read as a deployment figure. `taken on the debug profile`
    // is pinned because one row is not release and a table whose heading no
    // longer names a profile has to say so somewhere a reader will reach: a
    // debug figure read as a release one overstates the cost of stopping a
    // cell by an unmeasured factor, in the safe direction, which is exactly
    // the kind of error nobody goes looking for.
    let lowered = document.to_lowercase();
    for required in [
        "4 cores",
        "release",
        "taken on the debug profile",
        "not a deployment measurement",
        "nothing is deployed",
        "2026-09-05",
    ] {
        assert!(
            lowered.contains(required),
            "docs/ops/execution-measurements.md does not say \"{required}\""
        );
    }
    for overclaim in ["tick-to-order", "wire-to-wire", "microseconds at the venue"] {
        assert!(
            !lowered.contains(overclaim),
            "docs/ops/execution-measurements.md claims \"{overclaim}\""
        );
    }
}

// --- the fifteenth capability: the halt wire ---------------------------------
//
// Fourteen of the fifteen execution capabilities had a number above; this is
// the fifteenth, and it is the one whose absence mattered most. Every row up
// to here says what the platform costs while it is working. This one says how
// long it keeps working after somebody has told it to stop, which is the only
// figure on the page a risk desk is entitled to ask for by name.
//
// §46.2 asks for two halt paths that share no failure — "Spanner flag polled
// and Pub/Sub broadcast. Either halts trading" — and both are measured, for
// the same reason both exist: a wire measured on the day the other one worked
// is not a kill switch. In this workspace they are the centre's trip carried
// in the signed policy payload, and a flag file on the node's own filesystem.

/// What an operator writes into the polled halt flag to engage it.
///
/// The reason is echoed into the cell's halt state, and the tests below assert
/// the *whole* reason rather than a fragment of it: an unreadable flag halts
/// too, fail-closed, so a measurement that only checked "the cell is halted"
/// would be satisfied by a write that never landed.
const HALT_FLAG_CONTENT: &[u8] = b"engaged: a measurement drill\n";

/// The halt state that content produces, in full.
const HALT_FLAG_REASON: &str = "polled halt: is engaged: a measurement drill";

/// One pass that must place an order, its fill settled through the drop copy.
///
/// This is the premise of both halt measurements. "The wire stopped a cell
/// that was placing" is a fact about the wire only if the cell was placing,
/// and a halt timed against a cell that had quietly stopped trading for some
/// unrelated reason is a stopwatch on nothing. Settling each fill keeps the
/// loop honest for a second reason: unsettled orders accumulate against
/// `MAX_OPEN_ORDERS`, and a capacity refusal partway through would end the
/// placing the premise asserts.
fn placing_pass(cell: &mut Cell, gateway: &mut PaperVenue, now: Timestamp) -> Result<usize> {
    let report = cell.work(now, gateway)?;
    assert!(
        !report.halted,
        "the premise is a running cell, and this one was already halted before the wire under \
         measurement engaged it"
    );
    for fill in &report.fills {
        cell.observe_drop_copy(DropCopyFill {
            order_id: fill.order_id.clone(),
            venue: fill.venue.clone(),
            quantity: fill.quantity,
            price: fill.price,
            at: now,
        });
    }
    let breaks = cell.reconcile(now);
    assert!(
        breaks.is_empty(),
        "the drop copy disagreed with the order-entry channel: {breaks:?}"
    );
    Ok(report.orders.len())
}

/// What a halted pass has to look like, and which wire it has to name.
///
/// The gate is compared as a whole token and never as a substring. The two
/// gates are `policy_halt` and `polled_halt`; they differ by two characters,
/// both contain `halt`, and an operator who reads the wrong one knocks on the
/// wrong door — the broadcast halt is released by a newer signed payload from
/// the centre, the polled halt by deleting a file on the node. So the pass is
/// asserted to name the wire that fired *and* not to name the other one.
fn assert_the_pass_was_refused_by(report: &WorkReport, gate: &str, other: &str) {
    assert!(
        report.halted,
        "the pass after the halt reports a running cell"
    );
    assert!(
        report.orders.is_empty(),
        "a halted cell sent {} orders",
        report.orders.len()
    );
    let gates: Vec<&str> = report
        .refusals
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        gates.contains(&gate),
        "the halted pass does not refuse under `{gate}`, so nothing tells an operator which wire \
         stopped the cell: {gates:?}"
    );
    assert!(
        !gates.contains(&other),
        "the halted pass refuses under `{other}`, which is the other wire entirely: {gates:?}"
    );
}

#[test]
fn each_halt_wire_stopping_the_next_pass_costs_what_the_execution_measurements_say() -> Result<()> {
    // Timed, for each wire: from the instant the halt exists — the signed
    // payload handed to the cell, or the flag written on the node — to the
    // instant the next pass refuses to trade. One always-firing strategy over
    // a fixed two-level book, so the workload either side of the halt is the
    // same deterministic pass every iteration.
    //
    // Two things this number is not. It is not a deployed latency: nothing is
    // deployed, and `qip-edge-node` has no scheduler, so it reads the flag
    // once per liveness probe and the wait for the next read dominates the
    // whole figure in any real deployment. And the polled half is the only
    // timed path in this file with a syscall on it — one `write` and one
    // `read` against the container's page cache, which is not a Secret
    // Manager mount.
    const HALTS: usize = 500;
    let base = start().saturating_add(Duration::from_secs(2));
    // Three instants per iteration: place, halt, release. Milliseconds apart,
    // the same regime as the work-pass measurement above, so the book never
    // ages out from under the strategy and the envelope never expires.
    let at = |millis: usize| base.saturating_add(Duration::from_millis(millis as i64));

    // --- wire one: the centre's trip, carried in the policy payload ---------
    let mut cell = edge_cell(&[("alpha", SignalKind::Enter, "100", PricingPolicy::Marketable)])?;
    let mut gateway = PaperVenue {
        fills: true,
        ..PaperVenue::default()
    };
    // Signed before the clock starts. What a cell pays is the verification and
    // the application; the signing happened at the centre, on another machine
    // in any deployment that exists. Each halt is followed by a release with a
    // newer sequence issued after the barrier the halt raised, because that is
    // the only thing that can release it.
    let mut commands: Vec<(PolicyPayload, PolicyPayload)> = Vec::with_capacity(HALTS);
    for index in 0..HALTS {
        let mut halt =
            PolicyPayload::unproduced(2 * index as u64 + 1, EDGE_CELL, at(3 * index + 1));
        halt.halted = true;
        let release = PolicyPayload::unproduced(2 * index as u64 + 2, EDGE_CELL, at(3 * index + 2));
        commands.push((
            halt.signed(EDGE_POLICY_KEY)?,
            release.signed(EDGE_POLICY_KEY)?,
        ));
    }

    let mut placed = 0usize;
    let mut stopped = 0usize;
    let mut central = WallDuration::ZERO;
    for (index, (halt, release)) in commands.into_iter().enumerate() {
        placed += placing_pass(&mut cell, &mut gateway, at(3 * index))?;

        let halt_at = at(3 * index + 1);
        let began = Instant::now();
        let verified = VerifiedPolicy::verify(halt, EDGE_POLICY_KEY, EDGE_CELL, halt_at)?;
        cell.apply_policy(verified, halt_at)?;
        let report = cell.work(halt_at, &mut gateway)?;
        central += began.elapsed();

        assert_the_pass_was_refused_by(&report, "policy_halt", "polled_halt");
        stopped += 1;

        let release_at = at(3 * index + 2);
        let verified = VerifiedPolicy::verify(release, EDGE_POLICY_KEY, EDGE_CELL, release_at)?;
        cell.apply_policy(verified, release_at)?;
        assert!(
            !cell.is_halted(),
            "the release did not restore the cell, so every iteration after this one would time a \
             halt on a cell that was already stopped"
        );
    }
    assert_eq!(
        placed, HALTS,
        "the cell did not place an order on every pass before a halt"
    );
    assert_eq!(
        gateway.accepted, HALTS,
        "the venue did not receive an order for every pass before a halt"
    );
    assert_eq!(stopped, HALTS, "not every halt stopped the next pass");
    report(
        "edge halt wire, central policy payload (verify, apply, refuse next pass)",
        HALTS,
        central,
        5_000.0,
    );

    // --- wire two: the flag polled off the node's own filesystem ------------
    // Its own directory, because what a *missing* flag means depends on
    // whether the directory carrying it is still there: an absent file is the
    // off state, a gone mount is a wire whose state is unknown and halts.
    let directory =
        std::env::temp_dir().join(format!("qip-halt-wire-{}-{}", std::process::id(), line!()));
    std::fs::create_dir_all(&directory)
        .map_err(|error| Error::io(format!("cannot create {}: {error}", directory.display())))?;
    let flag = directory.join("halt");

    let mut cell = edge_cell(&[("alpha", SignalKind::Enter, "100", PricingPolicy::Marketable)])?;
    let mut gateway = PaperVenue {
        fills: true,
        ..PaperVenue::default()
    };
    let mut placed = 0usize;
    let mut stopped = 0usize;
    let mut polled = WallDuration::ZERO;
    for index in 0..HALTS {
        placed += placing_pass(&mut cell, &mut gateway, at(3 * index))?;

        let halt_at = at(3 * index + 1);
        let began = Instant::now();
        std::fs::write(&flag, HALT_FLAG_CONTENT)
            .map_err(|error| Error::io(format!("cannot write {}: {error}", flag.display())))?;
        // What `qip_edge_node::halt::HaltFlag::read` does with a flag that is
        // there. The node is an application crate this suite cannot link — the
        // dependency direction forbids it — so the read is reproduced here
        // rather than called, and only its present-file arm is inside the
        // number.
        let reading = match std::fs::read(&flag) {
            Ok(bytes) => PolledHalt::from_content(&bytes),
            Err(error) => PolledHalt::Unreadable(format!("cannot read the flag: {error}")),
        };
        cell.apply_polled_halt(reading, halt_at);
        let report = cell.work(halt_at, &mut gateway)?;
        polled += began.elapsed();

        assert_the_pass_was_refused_by(&report, "polled_halt", "policy_halt");
        // The whole reason, not a fragment of it: an unreadable flag halts as
        // well, so a measurement that asked only whether the cell was stopped
        // would be satisfied by a write that never landed and a read that
        // failed — the fail-closed path, timing the wrong thing and passing.
        assert_eq!(
            cell.polled_halt(),
            Some(HALT_FLAG_REASON),
            "the cell is halted by something other than the flag that was written"
        );
        stopped += 1;

        // Released outside the clock by removing the flag, which is the shape
        // the deployment uses: create to halt, delete to release.
        std::fs::remove_file(&flag)
            .map_err(|error| Error::io(format!("cannot remove {}: {error}", flag.display())))?;
        let reading = match std::fs::read(&flag) {
            Ok(bytes) => PolledHalt::from_content(&bytes),
            Err(_) => PolledHalt::Absent,
        };
        cell.apply_polled_halt(reading, at(3 * index + 2));
        assert!(
            !cell.is_halted(),
            "deleting the flag did not release the cell, so every iteration after this one would \
             time a halt on a cell that was already stopped"
        );
    }
    assert_eq!(
        placed, HALTS,
        "the cell did not place an order on every pass before a halt"
    );
    assert_eq!(
        gateway.accepted, HALTS,
        "the venue did not receive an order for every pass before a halt"
    );
    assert_eq!(stopped, HALTS, "not every flag stopped the next pass");
    report(
        "edge halt wire, polled flag (write, read, apply, refuse next pass)",
        HALTS,
        polled,
        5_000.0,
    );

    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}
