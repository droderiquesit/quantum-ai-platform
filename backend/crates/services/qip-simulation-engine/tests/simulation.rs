//! Tests for the SIMULATE stage.
//!
//! The central test is `a_perfect_foresight_strategy_cannot_be_expressed`: the
//! whole design rests on look-ahead being unreachable rather than merely
//! discouraged, and a claim like that is worth demonstrating.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::{approx_eq, is_exactly_zero};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_market::bar::{Bar, Interval};
use qip_simulation_engine::backtest::{
    BacktestConfig, BacktestStrategy, Backtester, SimulatedFill,
};
use qip_simulation_engine::clock::{ExecutionAssumptions, PointInTimeView, SimulationClock};
use qip_simulation_engine::costs::{CostModel, Unfillable};
use qip_simulation_engine::montecarlo::{Generator, MonteCarlo};
use qip_simulation_engine::scenario::{
    FactorExposure, FactorShock, Scenario, StressTester, standard_library,
};
use qip_simulation_engine::validation::{
    PurgedSplit, WalkForward, assess_overfitting, deflated_sharpe, sharpe_standard_error,
};
use std::collections::BTreeMap;

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn equity(symbol: &str) -> FinancialObject {
    FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())
        .expect("valid object")
}

fn universe_of(symbols: &[&str]) -> Universe {
    let mut universe = Universe::new();
    for symbol in symbols {
        universe.insert(equity(symbol)).unwrap();
    }
    universe
}

fn bar(symbol: &str, day: i64, close: f64, volume: f64) -> Bar {
    let open_time = start().saturating_add(Duration::from_days(day));
    let open = close * 0.999;
    Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time,
        open: Decimal::from_f64(open).unwrap(),
        high: Decimal::from_f64(close.max(open) * 1.003).unwrap(),
        low: Decimal::from_f64(close.min(open) * 0.997).unwrap(),
        close: Decimal::from_f64(close).unwrap(),
        volume: Decimal::from_f64(volume).unwrap(),
        trade_count: 1_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }
}

/// A series that rises for the first half and falls for the second, so a
/// perfect-foresight strategy would be spectacularly profitable and an honest
/// one would not.
fn v_shaped(symbol: &str, days: usize) -> Vec<Bar> {
    (0..days)
        .map(|i| {
            let half = days / 2;
            let close = if i < half {
                100.0 + i as f64
            } else {
                100.0 + half as f64 - (i - half) as f64
            };
            bar(symbol, i as i64, close, 1_000_000.0)
        })
        .collect()
}

// --- the leakage guard ------------------------------------------------------

#[test]
fn a_view_shows_only_bars_that_had_closed() -> Result<()> {
    let clock = SimulationClock::new(v_shaped("AAA", 10), ExecutionAssumptions::next_bar())?;
    let view = clock.view().expect("a view at the first step");
    // The first step is the close of day zero, so exactly one bar is visible.
    assert_eq!(view.bars(&object("AAA")).len(), 1);
    assert_eq!(view.as_of(), clock.now().unwrap());
    Ok(())
}

#[test]
fn a_bar_does_not_exist_until_its_session_ends() -> Result<()> {
    // A daily bar stamped with today's date is not available at today's open;
    // treating it as available is a full day of look-ahead.
    let bars = v_shaped("AAA", 5);
    let clock = SimulationClock::new(bars.clone(), ExecutionAssumptions::next_bar())?;
    let view = clock.view().unwrap();
    let visible = view.bars(&object("AAA"));
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].close_time(), view.as_of());
    assert!(
        visible[0].open_time < view.as_of(),
        "the visible bar opened before the instant it became knowable"
    );
    Ok(())
}

#[test]
fn a_perfect_foresight_strategy_cannot_be_expressed() -> Result<()> {
    // The design claim, demonstrated. A strategy that wants tomorrow's price
    // has no method to ask for it: `bars` filters on the view's own instant,
    // and the view cannot outlive that instant because it borrows the clock.
    struct WouldCheat {
        best_seen: usize,
    }
    impl BacktestStrategy for WouldCheat {
        fn name(&self) -> &str {
            "would-cheat"
        }
        fn target_weights(&mut self, view: &PointInTimeView<'_>) -> BTreeMap<String, f64> {
            // Everything reachable is history. There is no accessor that
            // returns a bar with a close time after `view.as_of()`.
            let bars = view.bars(&object("AAA"));
            self.best_seen = self.best_seen.max(bars.len());
            for bar in bars {
                assert!(
                    bar.close_time() <= view.as_of(),
                    "a bar from the future reached the strategy"
                );
            }
            BTreeMap::new()
        }
    }

    let mut clock = SimulationClock::new(v_shaped("AAA", 40), ExecutionAssumptions::next_bar())?;
    let mut strategy = WouldCheat { best_seen: 0 };
    let result = Backtester::new(BacktestConfig::default())?.run(
        &mut strategy,
        &mut clock,
        &universe_of(&["AAA"]),
    )?;
    assert_eq!(
        strategy.best_seen, 40,
        "the strategy saw all the history it should"
    );
    assert!(result.guarded_reads > 0, "reads must go through the guard");
    Ok(())
}

#[test]
fn a_decision_executes_after_the_lag_not_at_the_bar_that_produced_it() -> Result<()> {
    let clock = SimulationClock::new(v_shaped("AAA", 10), ExecutionAssumptions::next_bar())?;
    let now = clock.now().unwrap();
    let executable = clock.executable_at().expect("an executable instant");
    assert!(
        executable > now,
        "a next-bar assumption must not fill at the bar that produced the signal"
    );
    Ok(())
}

#[test]
fn a_decision_at_the_last_bar_cannot_be_executed_inside_the_simulation() -> Result<()> {
    // Otherwise every backtest gets one free trade with perfect hindsight.
    let mut clock = SimulationClock::new(v_shaped("AAA", 5), ExecutionAssumptions::next_bar())?;
    while clock.advance() {}
    assert!(clock.executable_at().is_none());
    Ok(())
}

#[test]
fn optimistic_assumptions_are_declared_as_such() {
    assert!(ExecutionAssumptions::instantaneous().is_optimistic());
    assert!(!ExecutionAssumptions::next_bar().is_optimistic());
    assert!(
        ExecutionAssumptions::instantaneous()
            .describe()
            .contains("overstate")
    );
}

#[test]
fn a_clock_refuses_an_incoherent_bar() {
    let mut broken = bar("AAA", 0, 100.0, 1_000.0);
    broken.low = dec!("200");
    let error = SimulationClock::new(vec![broken], ExecutionAssumptions::next_bar()).unwrap_err();
    assert!(error.message().contains("incoherent"));
}

#[test]
fn unsorted_history_is_sorted_rather_than_silently_truncated() -> Result<()> {
    // The point-in-time filter uses a binary search, which would cut at the
    // first out-of-order bar and hide most of the history.
    let mut bars = v_shaped("AAA", 20);
    bars.reverse();
    let clock = SimulationClock::new(bars, ExecutionAssumptions::next_bar())?;
    assert_eq!(clock.step_count(), 20);
    Ok(())
}

// --- costs ------------------------------------------------------------------

#[test]
fn impact_grows_with_the_square_root_of_participation() -> Result<()> {
    let model = CostModel::default();
    let price = dec!("100");
    let small = model
        .cost_of(dec!("10000"), price, 1_000_000.0, 0.02)
        .unwrap();
    let large = model
        .cost_of(dec!("40000"), price, 1_000_000.0, 0.02)
        .unwrap();
    // Four times the size at four times the participation is twice the impact
    // per unit, so four times the notional gives eight times the total impact.
    let ratio = large.impact / small.impact;
    assert!(
        approx_eq(ratio, 8.0, 1e-9),
        "impact scaled by {ratio} rather than 8"
    );
    Ok(())
}

#[test]
fn an_order_beyond_the_calibrated_range_is_refused_rather_than_priced() {
    // Returning a big number would look like an answer. It would not be one.
    let model = CostModel::default();
    let error = model
        .cost_of(dec!("400000"), dec!("100"), 1_000_000.0, 0.02)
        .unwrap_err();
    match error {
        Unfillable::ExceedsParticipation { requested, limit } => {
            assert!(approx_eq(requested, 0.4, 1e-9));
            assert!(approx_eq(limit, 0.2, 1e-9));
        }
        other => panic!("expected a participation refusal, got {other:?}"),
    }
}

#[test]
fn a_frictionless_model_says_what_it_is() {
    let model = CostModel::frictionless();
    assert!(model.is_frictionless());
    assert!(model.describe().contains("not an achievable return"));
}

#[test]
fn borrow_accrues_only_on_a_short() {
    let model = CostModel::default();
    assert!(is_exactly_zero(model.borrow_cost(1_000_000.0, 30.0)));
    let short = model.borrow_cost(-1_000_000.0, 365.0);
    assert!(approx_eq(short, 5_000.0, 1e-6));
}

// --- the backtester ---------------------------------------------------------

/// Buys and holds a fixed weight.
struct BuyAndHold {
    weight: f64,
    symbol: &'static str,
    rebalanced: bool,
}

impl BacktestStrategy for BuyAndHold {
    fn name(&self) -> &str {
        "buy-and-hold"
    }
    fn should_rebalance(&self, _view: &PointInTimeView<'_>) -> bool {
        !self.rebalanced
    }
    fn target_weights(&mut self, view: &PointInTimeView<'_>) -> BTreeMap<String, f64> {
        if view.bars(&object(self.symbol)).len() < 2 {
            return BTreeMap::new();
        }
        self.rebalanced = true;
        BTreeMap::from([(object(self.symbol).as_str().to_string(), self.weight)])
    }
}

#[test]
fn a_backtest_produces_an_equity_curve_and_pays_its_costs() -> Result<()> {
    let mut clock = SimulationClock::new(v_shaped("AAA", 60), ExecutionAssumptions::next_bar())?;
    let mut strategy = BuyAndHold {
        weight: 0.5,
        symbol: "AAA",
        rebalanced: false,
    };
    let result = Backtester::new(BacktestConfig::default())?.run(
        &mut strategy,
        &mut clock,
        &universe_of(&["AAA"]),
    )?;

    assert_eq!(result.equity_curve.len(), 60);
    assert_eq!(result.returns.len(), 59);
    assert_eq!(result.fills.len(), 1, "buy and hold trades once");
    assert!(result.total_costs() > 0.0, "a fill must cost something");
    assert!(result.cost_drag() > 0.0);
    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    Ok(())
}

#[test]
fn a_frictionless_backtest_reports_the_caveat_that_makes_it_readable() -> Result<()> {
    let mut clock =
        SimulationClock::new(v_shaped("AAA", 30), ExecutionAssumptions::instantaneous())?;
    let mut strategy = BuyAndHold {
        weight: 0.5,
        symbol: "AAA",
        rebalanced: false,
    };
    let config = BacktestConfig {
        costs: CostModel::frictionless(),
        assumptions: ExecutionAssumptions::instantaneous(),
        ..BacktestConfig::default()
    };
    let result = Backtester::new(config)?.run(&mut strategy, &mut clock, &universe_of(&["AAA"]))?;

    let caveats = result.caveats();
    assert!(
        caveats.iter().any(|c| c.contains("costs are zero")),
        "{caveats:?}"
    );
    assert!(
        caveats.iter().any(|c| c.contains("overstate")),
        "{caveats:?}"
    );
    assert!(is_exactly_zero(result.total_costs()));
    Ok(())
}

#[test]
fn costs_reduce_the_return_by_exactly_what_was_paid() -> Result<()> {
    // The accounting identity between the two runs: same prices, same trade,
    // and the difference in final equity is the costs.
    let universe = universe_of(&["AAA"]);
    let run = |costs: CostModel| -> Result<(f64, f64)> {
        let mut clock =
            SimulationClock::new(v_shaped("AAA", 40), ExecutionAssumptions::next_bar())?;
        let mut strategy = BuyAndHold {
            weight: 0.5,
            symbol: "AAA",
            rebalanced: false,
        };
        let config = BacktestConfig {
            costs,
            ..BacktestConfig::default()
        };
        let result = Backtester::new(config)?.run(&mut strategy, &mut clock, &universe)?;
        Ok((result.final_equity().to_f64(), result.total_costs()))
    };

    let (free_equity, free_costs) = run(CostModel::frictionless())?;
    let (paid_equity, paid_costs) = run(CostModel::default())?;
    assert!(is_exactly_zero(free_costs));
    assert!(paid_costs > 0.0);
    assert!(
        approx_eq(free_equity - paid_equity, paid_costs, 1.0),
        "the equity gap was {} against {paid_costs} of costs",
        free_equity - paid_equity
    );
    Ok(())
}

#[test]
fn an_instrument_outside_the_universe_is_rejected_rather_than_guessed() -> Result<()> {
    struct WantsUnknown;
    impl BacktestStrategy for WantsUnknown {
        fn name(&self) -> &str {
            "wants-unknown"
        }
        fn target_weights(&mut self, _view: &PointInTimeView<'_>) -> BTreeMap<String, f64> {
            BTreeMap::from([(object("AAA").as_str().to_string(), 0.5)])
        }
    }

    let mut clock = SimulationClock::new(v_shaped("AAA", 20), ExecutionAssumptions::next_bar())?;
    let result = Backtester::new(BacktestConfig::default())?.run(
        &mut WantsUnknown,
        &mut clock,
        &Universe::new(),
    )?;
    assert!(!result.rejected.is_empty());
    assert!(
        result.rejected[0].reason.contains("not in the universe"),
        "{:?}",
        result.rejected[0]
    );
    assert!(result.fills.is_empty(), "nothing may be traded on a guess");
    Ok(())
}

#[test]
fn an_order_too_large_for_the_market_is_rejected_and_reported() -> Result<()> {
    struct Whale;
    impl BacktestStrategy for Whale {
        fn name(&self) -> &str {
            "whale"
        }
        fn target_weights(&mut self, view: &PointInTimeView<'_>) -> BTreeMap<String, f64> {
            if view.bars(&object("AAA")).len() < 2 {
                return BTreeMap::new();
            }
            BTreeMap::from([(object("AAA").as_str().to_string(), 1.0)])
        }
    }

    // A thin market: a hundred shares a day against a ten-million book.
    let bars: Vec<Bar> = (0..30)
        .map(|i| bar("AAA", i, 100.0 + i as f64 * 0.1, 100.0))
        .collect();
    let mut clock = SimulationClock::new(bars, ExecutionAssumptions::next_bar())?;
    let result = Backtester::new(BacktestConfig::default())?.run(
        &mut Whale,
        &mut clock,
        &universe_of(&["AAA"]),
    )?;

    assert!(!result.rejected.is_empty());
    assert!(
        result.rejected[0].reason.contains("daily volume"),
        "{:?}",
        result.rejected[0]
    );
    assert!(
        result
            .caveats()
            .iter()
            .any(|c| c.contains("could not be filled")),
        "an unimplementable strategy must say so"
    );
    Ok(())
}

#[test]
fn the_same_inputs_produce_the_same_backtest() -> Result<()> {
    let universe = universe_of(&["AAA"]);
    let run = || -> Result<Vec<SimulatedFill>> {
        let mut clock =
            SimulationClock::new(v_shaped("AAA", 50), ExecutionAssumptions::next_bar())?;
        let mut strategy = BuyAndHold {
            weight: 0.4,
            symbol: "AAA",
            rebalanced: false,
        };
        Ok(Backtester::new(BacktestConfig::default())?
            .run(&mut strategy, &mut clock, &universe)?
            .fills)
    };
    assert_eq!(run()?, run()?);
    Ok(())
}

// --- Monte Carlo ------------------------------------------------------------

fn history(rng: &mut Xoshiro256, n: usize) -> Vec<f64> {
    (0..n).map(|_| rng.normal_with(0.0004, 0.012)).collect()
}

#[test]
fn a_monte_carlo_reports_the_precision_of_its_own_estimates() -> Result<()> {
    let mut rng = Xoshiro256::seeded(7);
    let data = history(&mut rng, 500);
    let distribution = MonteCarlo::new(Generator::Bootstrap { mean_block: 10.0 }, 2_000)?
        .run(&data, 21, &mut rng)?;

    assert_eq!(distribution.paths, 2_000);
    assert!(distribution.mean_standard_error > 0.0);
    assert!(
        distribution.mean_standard_error < distribution.stddev,
        "the standard error of the mean must be smaller than the spread of outcomes"
    );
    assert!(distribution.p05 < distribution.median);
    assert!(distribution.median < distribution.p95);
    assert!(
        distribution.expected_shortfall_95 <= distribution.p05,
        "the mean of the worst 5% cannot be better than where the tail starts"
    );
    Ok(())
}

#[test]
fn more_paths_narrow_the_standard_error_by_the_square_root() -> Result<()> {
    let mut rng = Xoshiro256::seeded(11);
    let data = history(&mut rng, 400);
    let few = MonteCarlo::new(Generator::Bootstrap { mean_block: 8.0 }, 1_000)?.run(
        &data,
        10,
        &mut Xoshiro256::seeded(1),
    )?;
    let many = MonteCarlo::new(Generator::Bootstrap { mean_block: 8.0 }, 16_000)?.run(
        &data,
        10,
        &mut Xoshiro256::seeded(1),
    )?;

    let ratio = few.mean_standard_error / many.mean_standard_error;
    assert!(
        (3.0..=5.0).contains(&ratio),
        "sixteen times the paths should cut the error about fourfold, got {ratio}"
    );
    Ok(())
}

#[test]
fn a_bootstrap_preserves_the_fat_tails_a_normal_draw_would_lose() -> Result<()> {
    // The reason bootstrapping is the default: resampling keeps whatever the
    // history had, and financial history has more tail than a normal.
    let mut rng = Xoshiro256::seeded(3);
    let mut data: Vec<f64> = (0..600).map(|_| rng.normal_with(0.0, 0.008)).collect();
    // A handful of crisis days, as any real series has.
    for i in (0..600).step_by(97) {
        data[i] = -0.09;
    }

    let bootstrap = MonteCarlo::new(Generator::Bootstrap { mean_block: 1.0 }, 8_000)?.run(
        &data,
        1,
        &mut Xoshiro256::seeded(5),
    )?;
    assert!(
        bootstrap.excess_kurtosis > 3.0,
        "the resampled distribution lost the tail: excess kurtosis {}",
        bootstrap.excess_kurtosis
    );
    Ok(())
}

#[test]
fn antithetic_pairing_reduces_simulation_noise() -> Result<()> {
    // The variance reduction is the entire justification for the extra
    // machinery, so it is worth checking it happens.
    let mut rng = Xoshiro256::seeded(13);
    let data = history(&mut rng, 500);
    let plain = MonteCarlo::new(
        Generator::Parametric {
            degrees_of_freedom: 5.0,
        },
        4_000,
    )?
    .run(&data, 20, &mut Xoshiro256::seeded(21))?;
    let paired = MonteCarlo::new(
        Generator::Antithetic {
            degrees_of_freedom: 5.0,
        },
        4_000,
    )?
    .run(&data, 20, &mut Xoshiro256::seeded(21))?;

    assert!(
        paired.mean_standard_error < plain.mean_standard_error,
        "antithetic pairing gave {} against {}",
        paired.mean_standard_error,
        plain.mean_standard_error
    );
    Ok(())
}

#[test]
fn a_monte_carlo_with_too_few_paths_is_refused() {
    assert!(MonteCarlo::new(Generator::Bootstrap { mean_block: 5.0 }, 50).is_err());
    assert!(
        MonteCarlo::new(
            Generator::Parametric {
                degrees_of_freedom: 2.0
            },
            1_000
        )
        .is_err(),
        "a Student-t with two degrees of freedom has infinite variance"
    );
}

#[test]
fn the_distribution_says_how_many_paths_would_reach_a_target_precision() -> Result<()> {
    let mut rng = Xoshiro256::seeded(17);
    let data = history(&mut rng, 300);
    let distribution = MonteCarlo::new(Generator::Bootstrap { mean_block: 6.0 }, 1_000)?
        .run(&data, 10, &mut rng)?;
    let needed = distribution.paths_for_precision(distribution.mean_standard_error / 2.0);
    assert!(
        needed > distribution.paths * 3,
        "halving the error needs about four times the paths, suggested {needed}"
    );
    Ok(())
}

// --- validation -------------------------------------------------------------

#[test]
fn a_sharpe_ratio_is_deflated_for_the_number_of_strategies_tried() -> Result<()> {
    let mut rng = Xoshiro256::seeded(23);
    // A genuinely mediocre strategy.
    let returns: Vec<f64> = (0..500).map(|_| rng.normal_with(0.0003, 0.01)).collect();

    let single = deflated_sharpe(&returns, 1, 252.0)?;
    let searched = deflated_sharpe(&returns, 1_000, 252.0)?;

    assert!(
        searched.expected_maximum > single.expected_maximum,
        "trying a thousand strategies must raise the bar"
    );
    assert!(
        searched.probability < single.probability,
        "the same result is less credible after a thousand trials: {} vs {}",
        searched.probability,
        single.probability
    );
    Ok(())
}

/// The standard error is written once, and this pins what it says. Both
/// halves matter: the fixture catches the formula itself changing, which
/// nothing else can now that the holdout band takes its figure from here
/// rather than restating it; the second half catches `deflated_sharpe`
/// quietly growing a private copy again, by rebuilding its probability from
/// the exposed error and the fields it reports.
#[test]
fn the_sharpe_standard_error_is_the_stated_formula_and_is_what_deflation_divides_by() -> Result<()>
{
    // By hand, for SR = 0.5, γ₃ = −1, γ₄ = 4, n = 101:
    //   1 − γ₃·SR         = 1 − (−1)(0.5)      = 1.5
    //   ¼·γ₄·SR²          = 0.25 · 4 · 0.25    = 0.25
    //   variance          = (1.5 + 0.25) / 100 = 0.0175
    //   standard error    = √0.0175            = 0.132 287 565 553 229 5…
    let error = sharpe_standard_error(0.5, -1.0, 4.0, 101)?;
    assert!(
        (error - 0.0175_f64.sqrt()).abs() < 1e-15,
        "standard error {error} is not √0.0175"
    );
    assert!((error - 0.132_287_565_553_229_5).abs() < 1e-12, "{error}");
    // Normal returns (no skew, no excess kurtosis) reduce to √(1/(n−1)).
    let plain = sharpe_standard_error(0.5, 0.0, 0.0, 101)?;
    assert!((plain - 0.1).abs() < 1e-15, "{plain}");
    assert!(error > plain, "negative skew and fat tails widen the error");
    let refused = sharpe_standard_error(0.5, 0.0, 0.0, 1).expect_err("one observation");
    assert_eq!(refused.code(), "invalid");

    // Premise: a searched, non-normal series, so every term is live.
    let mut rng = Xoshiro256::seeded(29);
    let returns: Vec<f64> = (0..400)
        .map(|_| {
            let r = rng.normal_with(0.0008, 0.01);
            if r < -0.02 { r * 3.0 } else { r }
        })
        .collect();
    let deflated = deflated_sharpe(&returns, 50, 252.0)?;
    assert!(deflated.skewness < -0.1, "{deflated:?}");
    assert!(deflated.excess_kurtosis > 0.5, "{deflated:?}");
    assert!(deflated.expected_maximum > 0.0, "{deflated:?}");

    // The probability `deflated_sharpe` reports is Φ of the periodic gap
    // over the exposed error, and nothing else.
    let scale = 252.0_f64.sqrt();
    let periodic = deflated.observed / scale;
    let exposed = sharpe_standard_error(
        periodic,
        deflated.skewness,
        deflated.excess_kurtosis,
        deflated.observations,
    )?;
    let rebuilt = qip_numerics::distributions::normal_cdf(
        (periodic - deflated.expected_maximum / scale) / exposed,
    );
    assert!(
        (rebuilt - deflated.probability).abs() < 1e-12,
        "deflation divides by a different error than it exposes: {rebuilt} vs {}",
        deflated.probability
    );
    Ok(())
}

#[test]
fn negative_skew_raises_the_bar_a_sharpe_ratio_must_clear() -> Result<()> {
    // A strategy that sells tail risk shows a flattering Sharpe until the tail
    // arrives. Two series are compared at *identical* Sharpe ratios so the only
    // thing that differs is the shape — a fixture where the skewed series also
    // had a higher Sharpe would prove nothing about the correction.
    // Both fixtures clear the selection threshold, which is the only regime
    // where "more uncertainty means less confidence" holds — below it a wider
    // distribution puts more mass above a threshold the estimate never reaches,
    // and the probability rises. See `clears_selection_threshold`.
    let mut rng = Xoshiro256::seeded(29);
    let symmetric: Vec<f64> = (0..400).map(|_| rng.normal_with(0.0020, 0.008)).collect();

    // Tail-selling: small steady gains punctuated by rare large losses.
    let mut skewed: Vec<f64> = (0..400).map(|_| rng.normal_with(0.0020, 0.004)).collect();
    for i in (0..400).step_by(80) {
        skewed[i] -= 0.04;
    }
    // Rescale so the two series have the same periodic Sharpe ratio exactly.
    let target = mean_of(&symmetric) / stddev_of(&symmetric);
    let current = mean_of(&skewed) / stddev_of(&skewed);
    let shift = (target - current) * stddev_of(&skewed);
    for value in skewed.iter_mut() {
        *value += shift;
    }

    let plain = deflated_sharpe(&symmetric, 100, 252.0)?;
    let tail_selling = deflated_sharpe(&skewed, 100, 252.0)?;
    assert!(
        plain.clears_selection_threshold(),
        "the fixture must be above the selection threshold for the comparison to mean anything"
    );
    assert!(tail_selling.clears_selection_threshold());
    assert!(
        approx_eq(plain.observed, tail_selling.observed, 1e-6),
        "the fixtures must have the same Sharpe: {} vs {}",
        plain.observed,
        tail_selling.observed
    );
    assert!(
        tail_selling.skewness < plain.skewness - 1.0,
        "the fixture must actually be negatively skewed: {} vs {}",
        tail_selling.skewness,
        plain.skewness
    );
    assert!(
        tail_selling.probability < plain.probability,
        "at equal Sharpe, negative skew must reduce confidence: {} vs {}",
        tail_selling.probability,
        plain.probability
    );
    Ok(())
}

fn mean_of(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn stddev_of(values: &[f64]) -> f64 {
    let mean = mean_of(values);
    (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (values.len() - 1) as f64).sqrt()
}

#[test]
fn a_result_below_the_selection_threshold_is_reported_as_failing_it() -> Result<()> {
    // The counter-intuitive regime, pinned so it cannot change silently: below
    // the threshold, extra uncertainty raises the probability rather than
    // lowering it, because a wider distribution puts more mass above a bar the
    // point estimate never reaches. `clears_selection_threshold` is what tells
    // a caller which regime the number came from.
    let mut rng = Xoshiro256::seeded(41);
    let mediocre: Vec<f64> = (0..400).map(|_| rng.normal_with(0.0002, 0.01)).collect();
    let result = deflated_sharpe(&mediocre, 1_000, 252.0)?;

    assert!(
        !result.clears_selection_threshold(),
        "the fixture must sit below the threshold: {:.3} against {:.3}",
        result.observed,
        result.expected_maximum
    );
    assert!(
        !result.is_credible(),
        "a result the search alone explains must not read as credible"
    );
    Ok(())
}

#[test]
fn purged_folds_remove_the_observations_that_would_leak() -> Result<()> {
    // A strategy holding for ten days leaks ten days of the test set into the
    // training set at every boundary unless they are purged.
    let split = PurgedSplit::new(5, 10, 5)?;
    let folds = split.split(500)?;
    assert_eq!(folds.len(), 5);

    for (i, fold) in folds.iter().enumerate() {
        assert!(fold.is_valid());
        // No index appears in both.
        for index in &fold.test {
            assert!(!fold.train.contains(index), "fold {i} leaked index {index}");
        }
        // The purged and embargoed observations are gone, not merely marked.
        let expected = 500 - fold.test.len() - fold.purged - fold.embargoed;
        assert_eq!(fold.train.len(), expected, "fold {i}");
    }
    // The first fold has nothing before it to purge; the rest do.
    assert_eq!(folds[0].purged, 0);
    assert_eq!(folds[1].purged, 10);
    Ok(())
}

#[test]
fn a_walk_forward_test_window_is_always_after_its_training_window() -> Result<()> {
    for anchored in [true, false] {
        let walk = WalkForward::new(100, 20, anchored, 5)?;
        for fold in walk.split(400)? {
            let last_train = *fold.train.iter().max().unwrap();
            let first_test = *fold.test.iter().min().unwrap();
            assert!(
                last_train < first_test,
                "training ran to {last_train} against a test starting at {first_test}"
            );
            assert!(first_test - last_train > 5, "the embargo was not respected");
        }
    }
    Ok(())
}

#[test]
fn an_anchored_window_grows_and_a_rolling_one_does_not() -> Result<()> {
    let anchored = WalkForward::new(100, 50, true, 0)?.split(400)?;
    let rolling = WalkForward::new(100, 50, false, 0)?.split(400)?;

    assert!(
        anchored[anchored.len() - 1].train.len() > anchored[0].train.len(),
        "an anchored window must accumulate history"
    );
    for fold in &rolling {
        assert_eq!(fold.train.len(), 100, "a rolling window is fixed length");
    }
    Ok(())
}

#[test]
fn a_strategy_whose_edge_vanishes_out_of_sample_is_identified() -> Result<()> {
    let mut rng = Xoshiro256::seeded(31);
    // In sample it looks excellent; out of sample it is noise.
    let in_sample: Vec<Vec<f64>> = (0..5)
        .map(|_| (0..100).map(|_| rng.normal_with(0.002, 0.01)).collect())
        .collect();
    let out_of_sample: Vec<Vec<f64>> = (0..5)
        .map(|_| (0..100).map(|_| rng.normal_with(-0.0001, 0.01)).collect())
        .collect();

    let report = assess_overfitting(&in_sample, &out_of_sample)?;
    assert!(report.looks_overfitted(), "{}", report.summarise());
    assert!(report.degradation < 0.3);
    Ok(())
}

#[test]
fn a_strategy_that_holds_up_is_not_flagged() -> Result<()> {
    let mut rng = Xoshiro256::seeded(37);
    let in_sample: Vec<Vec<f64>> = (0..5)
        .map(|_| (0..200).map(|_| rng.normal_with(0.0012, 0.01)).collect())
        .collect();
    let out_of_sample: Vec<Vec<f64>> = (0..5)
        .map(|_| (0..200).map(|_| rng.normal_with(0.0010, 0.01)).collect())
        .collect();

    let report = assess_overfitting(&in_sample, &out_of_sample)?;
    assert!(!report.looks_overfitted(), "{}", report.summarise());
    Ok(())
}

// --- scenarios and stress ---------------------------------------------------

fn exposures() -> Vec<FactorExposure> {
    vec![
        FactorExposure {
            object_id: "obj-EQ".to_string(),
            notional: dec!("6000000"),
            betas: BTreeMap::from([("equity".to_string(), 1.1)]),
        },
        FactorExposure {
            object_id: "obj-BOND".to_string(),
            notional: dec!("3000000"),
            betas: BTreeMap::from([("rates".to_string(), 6.5), ("credit".to_string(), 4.0)]),
        },
        FactorExposure {
            object_id: "obj-EXOTIC".to_string(),
            notional: dec!("1000000"),
            betas: BTreeMap::new(),
        },
    ]
}

#[test]
fn the_standard_library_is_documented_and_valid() {
    let library = standard_library();
    assert!(library.len() >= 6);
    for scenario in &library {
        scenario
            .validate()
            .unwrap_or_else(|e| panic!("{} is invalid: {}", scenario.name, e.message()));
        assert!(
            scenario.description.len() > 40,
            "{} has no real provenance",
            scenario.name
        );
    }
    assert!(library.iter().any(|s| s.historical));
    assert!(
        library.iter().any(|s| !s.historical),
        "a library of only past crises cannot ask about the next one"
    );
}

#[test]
fn a_scenario_without_stated_provenance_is_refused() {
    let vague = Scenario {
        name: "bad-day".to_string(),
        description: "things fall".to_string(),
        shocks: vec![FactorShock::new("equity", -0.2, 1.0)],
        stressed_correlation: Some(0.9),
        liquidity_multiplier: 2.0,
        historical: false,
    };
    assert!(
        vague
            .validate()
            .unwrap_err()
            .message()
            .contains("provenance")
    );
}

#[test]
fn a_scenario_that_improves_liquidity_is_refused() {
    let wrong = Scenario {
        name: "easy-crisis".to_string(),
        description: "a crisis in which it somehow becomes cheaper to trade".to_string(),
        shocks: vec![FactorShock::new("equity", -0.2, 1.0)],
        stressed_correlation: None,
        liquidity_multiplier: 0.5,
        historical: false,
    };
    assert!(wrong.validate().is_err());
}

#[test]
fn a_stress_test_reports_what_it_could_not_model() -> Result<()> {
    // A position with no recorded exposure shows zero loss because nothing was
    // measured. Reporting that as safety is how a stress test understates.
    let tester = StressTester::new(5.0);
    let scenario = standard_library()
        .into_iter()
        .find(|s| s.name == "credit-crisis-2008")
        .unwrap();
    let result = tester.apply(&scenario, &exposures(), 10_000_000.0, start())?;

    assert_eq!(result.unmodelled, vec!["obj-EXOTIC".to_string()]);
    assert!(
        result.summarise().contains("unmodelled"),
        "{}",
        result.summarise()
    );
    assert!(result.loss_fraction > 0.0);
    Ok(())
}

#[test]
fn a_rates_shock_hurts_a_long_bond_position() -> Result<()> {
    // The sign convention that is easy to get backwards: yields up, prices
    // down, so a positive rates shock is a loss for a long duration position.
    let tester = StressTester::new(5.0);
    let inflation = standard_library()
        .into_iter()
        .find(|s| s.name == "inflation-shock-2022")
        .unwrap();
    let result = tester.apply(&inflation, &exposures(), 10_000_000.0, start())?;

    let bond = result
        .positions
        .iter()
        .find(|p| p.object_id == "obj-BOND")
        .unwrap();
    assert!(
        bond.profit_and_loss < 0.0,
        "a rate rise must hurt a long bond: {}",
        bond.profit_and_loss
    );
    let rates_contribution = bond.attribution.get("rates").copied().unwrap_or(0.0);
    assert!(rates_contribution < 0.0);
    Ok(())
}

#[test]
fn the_liquidation_cost_scales_with_the_scenarios_own_liquidity_assumption() -> Result<()> {
    let tester = StressTester::new(5.0);
    let library = standard_library();
    let freeze = library
        .iter()
        .find(|s| s.name == "liquidity-freeze")
        .unwrap();
    let crash = library
        .iter()
        .find(|s| s.name == "equity-crash-1987")
        .unwrap();

    let frozen = tester.apply(freeze, &exposures(), 10_000_000.0, start())?;
    let crashed = tester.apply(crash, &exposures(), 10_000_000.0, start())?;

    assert!(
        frozen.liquidation_cost > crashed.liquidation_cost,
        "a liquidity freeze must cost more to exit than a price crash"
    );
    Ok(())
}

#[test]
fn scenarios_are_ranked_worst_first() -> Result<()> {
    let tester = StressTester::new(5.0);
    let results = tester.apply_all(&standard_library(), &exposures(), 10_000_000.0, start())?;
    for pair in results.windows(2) {
        assert!(
            pair[0].loss_fraction >= pair[1].loss_fraction,
            "{} ranked above {}",
            pair[1].scenario,
            pair[0].scenario
        );
    }
    // The worst case for this book — long equity, long duration — is the one
    // where both fall together.
    assert!(results[0].loss_fraction > 0.1);
    Ok(())
}

#[test]
fn a_stress_test_on_a_book_with_no_equity_is_refused() {
    let tester = StressTester::new(5.0);
    let scenario = standard_library().into_iter().next().unwrap();
    assert!(tester.apply(&scenario, &exposures(), 0.0, start()).is_err());
}

/// Rebalances on every bar, so it always has a decision in flight.
struct AlwaysRebalancing {
    symbol: &'static str,
    weight: f64,
}

impl BacktestStrategy for AlwaysRebalancing {
    fn name(&self) -> &str {
        "always-rebalancing"
    }

    fn should_rebalance(&self, _view: &PointInTimeView<'_>) -> bool {
        true
    }

    fn target_weights(&mut self, _view: &PointInTimeView<'_>) -> BTreeMap<String, f64> {
        BTreeMap::from([(object(self.symbol).as_str().to_string(), self.weight)])
    }
}

#[test]
fn an_order_replaced_before_it_came_due_is_counted_rather_than_dropped_in_silence() -> Result<()> {
    // Only one decision is held at a time, so a strategy rebalancing faster
    // than its own decision lag overwrites its pending order every step and
    // never trades. Keeping the latest signal is defensible; being silent
    // about it was not.
    //
    // This was expensive in practice. The evolution loop asked for a one-day
    // lag on one-minute bars, and the result -- two thousand rebalances, zero
    // fills, a perfectly flat equity curve -- was indistinguishable from a
    // strategy that had simply chosen not to trade. The holdout gate scored
    // that flat line as a Sharpe ratio.
    let days = 40;
    let mut clock = SimulationClock::new(
        v_shaped("AAA", days),
        // Ten days of lag against daily bars: every decision is replaced nine
        // times before the one that survives comes due.
        ExecutionAssumptions::intraday(Duration::from_days(10)),
    )?;
    let mut strategy = AlwaysRebalancing {
        symbol: "AAA",
        weight: 0.5,
    };
    let result = Backtester::new(BacktestConfig::default())?.run(
        &mut strategy,
        &mut clock,
        &universe_of(&["AAA"]),
    )?;

    // The premise: the strategy really did keep asking. Without this, a
    // supersession count of zero would be correct rather than a missing signal.
    assert!(
        result.rebalance_count > 1,
        "the strategy rebalanced {} time(s), so nothing could be superseded",
        result.rebalance_count
    );
    assert!(
        result.superseded > 0,
        "{} rebalance(s) against a ten-day lag on daily bars superseded nothing",
        result.rebalance_count
    );

    // And the same strategy at a one-bar lag supersedes nothing, because each
    // order comes due before the next decision is taken. Without this half, the
    // counter could be incrementing on every rebalance and still pass above.
    let mut clock = SimulationClock::new(v_shaped("AAA", days), ExecutionAssumptions::next_bar())?;
    let mut strategy = AlwaysRebalancing {
        symbol: "AAA",
        weight: 0.5,
    };
    let prompt = Backtester::new(BacktestConfig::default())?.run(
        &mut strategy,
        &mut clock,
        &universe_of(&["AAA"]),
    )?;
    assert!(prompt.rebalance_count > 1);
    assert_eq!(
        prompt.superseded, 0,
        "an order that comes due before the next decision was counted as superseded"
    );
    Ok(())
}
