//! The SIMULATE stage stresses the book it actually holds.
//!
//! Until 2026-09-08 the stage counted how many closes it had accumulated and
//! did nothing with them. `StressTester`, `standard_library`,
//! `ScenarioResult::breaches` and `RiskDecomposition::effective_bets` were
//! built, tested, and reached by nothing outside tests — machinery that
//! executed and measured nothing, which is the shape this repository names by
//! example in `MaxExpectedShortfall`.
//!
//! What is asserted here is the wire and the two honest limits on it: a
//! position the factor could not be measured for is *counted as unmodelled*
//! rather than dropped or treated as immune, and a scenario that would take
//! more than the tolerance out of the book is raised as a problem on the stage
//! rather than filed in a report nobody reads.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

// --- fixtures ---------------------------------------------------------------

/// A liquid listed name, stated rather than inherited: `LiquidityProfile` has
/// no `Default`, because the controls that read it are the ones whose job is
/// to veto trading.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    InstrumentType::CommonStock,
                    fixture_liquidity(),
                )
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .expect("valid object"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("stress-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

/// Daily bars whose returns are not all the same.
///
/// The `phase` shifts the pseudo-random walk so two instruments do not share
/// one return series: a factor estimated from identical series has zero
/// variance, every beta is refused, and a test built on that would assert the
/// refusal path while claiming to assert the model.
fn bars(symbol: &str, count: i64, phase: f64) -> Vec<Bar> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662 + phase) % 1.0 - 0.5) * 0.02;
            let open = price;
            price *= 1.0 + noise;
            let at = start().saturating_sub(Duration::from_days(count - i));
            Bar {
                object_id: object(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).expect("finite"),
                high: Decimal::from_f64(open.max(price) * 1.002).expect("finite"),
                low: Decimal::from_f64(open.min(price) * 0.998).expect("finite"),
                close: Decimal::from_f64(price).expect("finite"),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }
        })
        .collect()
}

fn observations(symbol: &str, count: i64, phase: f64) -> Vec<SensedRecord> {
    bars(symbol, count, phase)
        .into_iter()
        .map(|bar| SensedRecord::Bar(Box::new(bar)))
        .collect()
}

/// Take a position in `symbol` through the control path, so the book holds it.
///
/// The quantity is a parameter because the loss a scenario produces is a
/// fraction of *equity*, so how much of the book is at risk is the premise of
/// any assertion about a tolerance being breached.
fn buy(platform: &mut Platform, symbol: &str, quantity: Decimal, at: Timestamp) -> Result<()> {
    let order = platform.order_from(
        object(symbol),
        Side::Buy,
        quantity,
        dec!("100"),
        "prop-stress",
        vec!["hyp-stress".to_string()],
        at,
    );
    platform.submit_order(order, at)
}

// --- the wire ---------------------------------------------------------------

#[test]
fn the_simulate_stage_stresses_the_open_book_against_the_standard_library() -> Result<()> {
    let mut platform = platform()?;
    // The premise first: with no history and no position the stage has nothing
    // to stress, and says so rather than reporting a book that survives
    // everything. A test that only asserted the populated case would pass
    // against an implementation that always reported a stress result.
    let quiet = platform.run_cycle(start());
    assert!(
        platform.stress_report().is_none(),
        "an empty book must produce no stress report, not an empty one"
    );
    let simulate = quiet
        .stage(Stage::Simulate)
        .expect("the cycle runs every stage");
    assert_eq!(
        simulate.produced, 0,
        "no scenario can have been applied: {}",
        simulate.detail
    );

    platform.observe(observations("AAA", 120, 0.0));
    platform.observe(observations("BBB", 120, 0.31));
    buy(&mut platform, "AAA", dec!("100"), start())?;

    let report = platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let simulate = report
        .stage(Stage::Simulate)
        .expect("the cycle runs every stage");

    let stress = platform
        .stress_report()
        .expect("a book with a position and history is stressed");
    assert_eq!(
        stress.scenarios.len(),
        qip_simulation_engine::scenario::standard_library().len(),
        "every scenario in the library is applied, not a subset"
    );
    // The stage is billed for both halves of what it did. Counting only the
    // scenarios was a regression: a platform with a long tape and no position
    // then reported producing nothing, and a blind cycle cost the same as a
    // sighted one.
    assert!(
        simulate.produced > stress.scenarios.len(),
        "produced must count the history resampled as well as the scenarios \
         applied: {}",
        simulate.detail
    );
    assert_eq!(
        stress.modelled_positions, 1,
        "the held position carries a beta from the tape: {}",
        simulate.detail
    );

    // Worst first, and the ordering is what makes `worst()` meaningful.
    let worst = stress.worst().expect("scenarios were applied");
    assert!(
        stress
            .scenarios
            .iter()
            .all(|other| other.loss_fraction <= worst.loss_fraction),
        "the report is ordered worst loss first"
    );
    // The equity-style shocks reach the position rather than leaving it on the
    // `unmodelled` list — the whole point of the factor model behind this.
    assert!(
        worst.unmodelled.is_empty(),
        "a position with a beta is stressed, not listed unmodelled: {:?}",
        worst.unmodelled
    );
    assert!(
        worst.loss_fraction > 0.0,
        "an equity shock against a long position loses money: {}",
        worst.summarise()
    );

    // And the decomposition reports over real contributions rather than the
    // zero it returned while `factor_betas` was empty everywhere.
    let decomposition = stress
        .decomposition
        .as_ref()
        .expect("a modelled position gives the single-factor model a row");
    assert!(
        decomposition.effective_bets() > 0.0,
        "effective breadth over one real contribution is positive, not the \
         zero an empty contribution map returns"
    );
    Ok(())
}

#[test]
fn a_scenario_that_would_take_a_fifth_of_the_book_is_raised_as_a_problem() -> Result<()> {
    // `ScenarioResult::breaches` existed with no production caller: a stress
    // run could report a catastrophic loss and the stage would file it beside
    // a clean bill of health. The tolerance is not a limit — nothing is
    // refused — but a cycle in which the 2008 scenario halves the book must
    // not read like a quiet one.
    let mut platform = platform()?;
    platform.observe(observations("AAA", 120, 0.0));
    platform.observe(observations("BBB", 120, 0.31));
    // Forty thousand units of each at 100: eight million of gross against a
    // ten-million book, so the 2008 scenario's equity shock is worth about
    // a quarter of equity. A token position would lose a fraction of a
    // percent and the tolerance would never be reached — the premise of this
    // test is that the book is big enough to fail.
    buy(&mut platform, "AAA", dec!("40000"), start())?;
    buy(&mut platform, "BBB", dec!("40000"), start())?;

    let report = platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let simulate = report
        .stage(Stage::Simulate)
        .expect("the cycle runs every stage");
    let stress = platform.stress_report().expect("the book is stressed");

    // Assert the premise before the property: at least one scenario must
    // actually breach, or the absence of a problem would prove nothing.
    let breaching: Vec<&str> = stress
        .scenarios
        .iter()
        .filter(|result| result.breaches(0.20))
        .map(|result| result.scenario.as_str())
        .collect();
    assert!(
        !breaching.is_empty(),
        "the standard library must contain a scenario this book fails: {}",
        simulate.detail
    );
    for scenario in breaching {
        assert!(
            simulate
                .problems
                .iter()
                .any(|problem| problem.contains(scenario)),
            "scenario {scenario} breached and the stage said nothing: {:?}",
            simulate.problems
        );
    }
    Ok(())
}

#[test]
fn a_position_the_factor_cannot_measure_is_counted_unmodelled_and_not_dropped() -> Result<()> {
    // The failure this prevents: a book of two positions where one carries no
    // beta, reported as a loss over "the book". The loss is over one position
    // and the other was stressed by nothing, and only the count says so.
    //
    // BBB gets a tape too short to estimate a beta from — below
    // `MINIMUM_OVERLAP` — while AAA gets a long one. That is the real cause of
    // an unmodelled position in production: a newly listed name.
    let mut platform = platform()?;
    platform.observe(observations("AAA", 120, 0.0));
    platform.observe(observations("BBB", 6, 0.31));
    buy(&mut platform, "AAA", dec!("100"), start())?;
    buy(&mut platform, "BBB", dec!("100"), start())?;

    platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let stress = platform.stress_report().expect("the book is stressed");
    assert_eq!(
        stress.modelled_positions, 1,
        "only the instrument with enough overlap carries a beta"
    );
    assert_eq!(
        stress.unmodelled_positions, 1,
        "the short-history position is counted, not silently dropped"
    );
    let worst = stress.worst().expect("scenarios were applied");
    assert!(
        worst
            .unmodelled
            .iter()
            .any(|id| id.contains("BBB") || id.contains("bbb")),
        "the stress tester names the position it could not model: {:?}",
        worst.unmodelled
    );
    Ok(())
}
