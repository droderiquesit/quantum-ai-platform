//! Generated strategies scoring against real bars, for the first time.
//!
//! Until this harness existed, every driver of the strategy foundry was a
//! test with hand-made returns: the evolution brain could search but never
//! measure. These tests close that loop — a grammar writes candidates, the
//! compiler admits them, and the backtester scores them on bar history — and
//! each asserts its own premise first, because the failure mode of a bridge
//! like this is running *something* and calling it the candidate.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_evolution::generate::StrategyGenerator;
use qip_evolution::grammar::Grammar;
use qip_evolution::palette::FeaturePalette;
use qip_financial::quality::DataQuality;
use qip_financial::universe::Universe;
use qip_market::bar::{Bar, Interval};
use qip_simulation_engine::backtest::{BacktestConfig, Backtester};
use qip_simulation_engine::clock::ExecutionAssumptions;
use qip_simulation_engine::clock::SimulationClock;
use qip_simulation_engine::harness::{CompiledHarness, WARM_UP_BARS, bar_catalogue, bar_vector};
use qip_strategy::compile::StrategyCompiler;

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn subject() -> ObjectId {
    ObjectId::from_string("EVOL")
}

fn bar(day: i64, close: f64) -> Bar {
    let open = close * 0.999;
    Bar {
        object_id: subject(),
        venue: "XSIM".to_string(),
        interval: Interval::Day,
        open_time: start().saturating_add(Duration::from_days(day)),
        open: Decimal::from_f64(open).expect("finite"),
        high: Decimal::from_f64(close.max(open) * 1.003).expect("finite"),
        low: Decimal::from_f64(close.min(open) * 0.997).expect("finite"),
        close: Decimal::from_f64(close).expect("finite"),
        volume: Decimal::from_int(10_000),
        trade_count: 500,
        vwap: None,
        quality: DataQuality::default(),
    }
}

/// A year of trending bars with a wobble, so momentum and volatility both
/// have something to say.
fn history(days: i64) -> Vec<Bar> {
    (0..days)
        .map(|day| {
            let trend = 100.0 * (1.0 + 0.001 * day as f64);
            let wobble = 1.0 + 0.01 * ((day % 7) as f64 - 3.0) / 3.0;
            bar(day, trend * wobble)
        })
        .collect()
}

/// Search until the compiler admits at least one candidate. The grammar can
/// legitimately propose refusable candidates, so a fixed count would make
/// this test flaky against the seed rather than meaningful.
fn admitted_candidate(
    seed: u64,
) -> Result<(
    qip_strategy::compile::CompiledStrategy,
    qip_strategy::program::Program,
)> {
    let on = subject();
    let catalogue = bar_catalogue(&on)?;
    let grammar = Grammar::over(FeaturePalette::from_catalogue(&catalogue, &on)?);
    let mut generator = StrategyGenerator::new(grammar, format!("harness@{seed}"), seed);
    let mut compiler = StrategyCompiler::new(bar_catalogue(&on)?);
    for _ in 0..8 {
        let run = generator.generate(8, &mut compiler);
        if let Some(candidate) = run.accepted().first() {
            return Ok((candidate.compiled().clone(), compiler.program().clone()));
        }
    }
    Err(Error::not_found(
        "eight rounds of eight candidates produced nothing the compiler admits; the \
         grammar and the bar catalogue no longer overlap",
    ))
}

#[test]
fn a_generated_strategy_scores_against_bars_and_the_same_seed_reproduces_it() -> Result<()> {
    let (compiled, program) = admitted_candidate(11)?;

    let run = |compiled: &qip_strategy::compile::CompiledStrategy,
               program: &qip_strategy::program::Program|
     -> Result<(Vec<f64>, usize, usize)> {
        let mut harness = CompiledHarness::new(compiled.clone(), program.clone(), 0.5)?;
        let mut clock = SimulationClock::new(history(120), ExecutionAssumptions::next_bar())?;
        let result = Backtester::new(BacktestConfig::default())?.run(
            &mut harness,
            &mut clock,
            &Universe::new(),
        )?;
        let trace = harness.trace();
        Ok((result.returns, trace.decisions, trace.warming_decisions))
    };

    let (returns, decisions, warming) = run(&compiled, &program)?;

    // The premise: the strategy was actually asked, warmed up, and the run
    // produced a return series. Without these, the reproducibility assertion
    // below would be comparing two empty vectors and proving nothing.
    assert!(decisions > 0, "the backtest never consulted the strategy");
    assert!(
        warming < decisions,
        "every decision was warm-up ({warming}/{decisions}); the history is too short for \
         the harness's own warm-up of {WARM_UP_BARS} bars and this test measures nothing"
    );
    assert!(!returns.is_empty(), "the run produced no return series");

    let (again, _, _) = run(&compiled, &program)?;
    assert_eq!(
        returns, again,
        "the same candidate over the same bars produced different returns; an evaluation \
         nobody can replay is evidence nobody can check"
    );
    Ok(())
}

#[test]
fn warm_up_is_cash_and_is_counted_rather_than_scored() -> Result<()> {
    let (compiled, program) = admitted_candidate(23)?;
    let mut harness = CompiledHarness::new(compiled, program, 0.5)?;

    // The premise: strictly fewer closed bars than the warm-up needs.
    let short = history(WARM_UP_BARS as i64 - 2);
    let mut clock = SimulationClock::new(short, ExecutionAssumptions::next_bar())?;
    let result = Backtester::new(BacktestConfig::default())?.run(
        &mut harness,
        &mut clock,
        &Universe::new(),
    )?;

    let trace = harness.trace();
    assert!(
        trace.decisions > 0,
        "the backtest never consulted the strategy"
    );
    assert_eq!(
        trace.warming_decisions, trace.decisions,
        "a decision before the features were knowable was scored as if they were"
    );
    assert!(
        result.fills.is_empty(),
        "warm-up traded: {} fill(s) from a strategy that could not read its inputs",
        result.fills.len()
    );
    Ok(())
}

#[test]
fn every_feature_the_grammar_may_write_is_computed_after_warm_up() -> Result<()> {
    // The one-table construction is the guarantee; this test is what notices
    // if the table and the computer ever stop being generated from it.
    let on = subject();
    let catalogue = bar_catalogue(&on)?;
    let bars = history(WARM_UP_BARS as i64 + 5);
    let mut clock = SimulationClock::new(bars, ExecutionAssumptions::next_bar())?;
    // Walk the clock, keeping the vector from the last step that offered a
    // view: past the final advance the cursor is beyond the walk and the
    // clock deliberately offers nothing.
    let mut vector = None;
    loop {
        if let Some(view) = clock.view() {
            vector = Some(bar_vector(&on, &view));
        }
        if !clock.advance() {
            break;
        }
    }
    let vector = vector.ok_or_else(|| Error::not_found("a single view in the whole walk"))?;

    let undefined = vector.undefined();
    assert!(
        undefined.is_empty(),
        "after warm-up these features are still undefined: {undefined:?}"
    );
    for key in catalogue.keys() {
        assert!(
            vector.get(key).is_some(),
            "{} is in the catalogue the grammar writes from and the vector does not carry it",
            key.canonical()
        );
    }
    Ok(())
}

#[test]
fn a_mismatched_program_is_refused_before_it_can_masquerade_as_the_candidate() -> Result<()> {
    let (compiled, _program) = admitted_candidate(31)?;
    // A fresh, empty arena: nothing the plan points at exists in it.
    let empty =
        qip_strategy::compile::StrategyCompiler::new(bar_catalogue(&subject())?).into_program();
    let error = CompiledHarness::new(compiled, empty, 0.5)
        .err()
        .ok_or_else(|| {
            Error::invalid(
                "a harness was built over an arena that does not contain the strategy; the \
                 evaluation would have run a different strategy than the evidence names",
            )
        })?;
    assert!(error.message().contains("different strategy"));
    Ok(())
}
