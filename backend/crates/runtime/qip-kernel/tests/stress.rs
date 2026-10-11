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
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

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

// --- through the graph --------------------------------------------------------

/// Bars for `symbol` whose log returns follow `returns` exactly, starting
/// from a price of 100 and stepping one day per observation, oldest first.
/// The same fixture `causal_precedence.rs` drives the UNDERSTAND writer with.
fn bars_from_returns(symbol: &str, returns: &[f64], count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let open = price;
            price *= returns[i].exp();
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).expect("a price"),
                high: Decimal::from_f64(open.max(price) * 1.002).expect("a price"),
                low: Decimal::from_f64(open.min(price) * 0.998).expect("a price"),
                close: Decimal::from_f64(price).expect("a price"),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

/// A deterministic, bounded pseudo-random sequence in roughly `[-scale,
/// scale]`; deterministic means a failure reproduces byte-for-byte.
fn noise(seed: u64, count: usize, scale: f64) -> Vec<f64> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let unit = (state % 1_000_000) as f64 / 1_000_000.0;
            (unit - 0.5) * 2.0 * scale
        })
        .collect()
}

#[test]
fn decayed_calibration_shrinks_allocation_through_the_gate() -> Result<()> {
    // RISK-036 / R12-RISK-4111, and R12-RISK-4117..4124's "failure to survive
    // changes capital". This test used to run two cycles, assert one
    // position, and end in a page of comments promising the mechanism
    // existed; it had no assertion that could fail. It now drives both
    // triggers into the survival kernel the DECIDE stage sizes through, from
    // a stress report the SIMULATE stage really produced, and asserts the
    // released budget and the gate's gross ceiling move down and only down.
    use qip_cost_router::MarketRegime;
    use qip_kernel::confidence::{CALIBRATION_FLOOR, ThesisView};
    use qip_kernel::survival::{LedgerFigures, SurvivalGovernor};

    // A book big enough to fail the library: eight million of gross on ten.
    let mut platform = platform()?;
    platform.observe(observations("AAA", 120, 0.0));
    platform.observe(observations("BBB", 120, 0.31));
    buy(&mut platform, "AAA", dec!("40000"), start())?;
    buy(&mut platform, "BBB", dec!("40000"), start())?;
    platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let stress = platform
        .stress_report()
        .expect("the book is stressed")
        .clone();
    let (worst, loss) =
        qip_kernel::survival::worst_world(&stress).expect("the stress applied worlds");
    assert!(
        loss > 0.20,
        "premise: the book must fail a world at the 20% tolerance, or a reserve of zero proves \
         nothing; worst was {worst} at {loss}"
    );
    // Both generated tail worlds were applied beside the library.
    for name in [
        qip_kernel::tail::COUNTERPARTY_FAILURE,
        qip_kernel::tail::MODEL_FAILURE,
    ] {
        assert!(
            stress
                .tail_worlds
                .iter()
                .any(|world| world.scenario == name),
            "the {name} world was not applied: {:?}",
            stress.tail_worlds
        );
    }

    let figures = LedgerFigures {
        equity: dec!("10000000"),
        held: Decimal::ZERO,
        obligations: Decimal::ZERO,
        deployable: dec!("10000000"),
        gross: dec!("8000000"),
    };
    let views = [ThesisView {
        hypothesis_id: "hyp-stress".to_string(),
        object_id: "obj-AAA".to_string(),
        regime: MarketRegime::Trending,
    }];
    let at = start().saturating_add(Duration::from_days(1));

    // Removing the trigger leaves the size unchanged: with no stress the
    // whole deployable balance is released.
    let mut governor = SurvivalGovernor::default();
    let unstressed = governor.size(figures, None, 0.20, &views, at)?;
    assert_eq!(unstressed.released, figures.deployable);

    // The world the book fails withholds the excess as a reserve.
    let stressed = governor.size(figures, Some(&stress), 0.20, &views, at)?;
    let tail = stressed
        .tail
        .as_ref()
        .expect("a stressed book has a tail response");
    assert!(!tail.survives());
    assert!(stressed.survival.stressed_liquidity_reserve.is_positive());
    assert!(
        stressed.released < unstressed.released,
        "a book that fails its worst world must size smaller: {} against {}",
        stressed.released,
        unstressed.released
    );
    assert!(stressed.gross_ceiling < unstressed.gross_ceiling);

    // Decayed calibration narrows it again, by exactly the schedule's floor.
    governor
        .signals_mut()
        .observe_calibration(0.45, 60, [MarketRegime::Trending]);
    let decayed = governor.size(figures, Some(&stress), 0.20, &views, at)?;
    assert_eq!(
        decayed.released,
        stressed.released * CALIBRATION_FLOOR,
        "the calibration contraction compounds on the survival release"
    );
    // And nothing enlarges a size: every release is at most the ledger's.
    for budget in [&unstressed, &stressed, &decayed] {
        assert!(budget.released <= figures.deployable);
        assert!(budget.confidence.multiplier <= Decimal::ONE);
    }
    Ok(())
}

#[test]
fn all_eight_named_scenarios_are_enumerated_and_paper_capital_invariants_hold() -> Result<()> {
    // RISK-037: Eight named scenarios with capital invariants per scenario.
    //
    // The test asserts:
    // 1. Exactly eight scenarios are in the standard library
    // 2. Each scenario has a name and description (enumerated)
    // 3. For each scenario: paper book survives (loss < equity), no negative loss
    // 4. Survivability holds across all positions in the book
    let scenarios = qip_simulation_engine::scenario::standard_library();

    // Premise: eight scenarios exist
    assert_eq!(
        scenarios.len(),
        8,
        "standard library must enumerate eight named scenarios, not {}: {:?}",
        scenarios.len(),
        scenarios.iter().map(|s| &s.name).collect::<Vec<_>>()
    );

    // Validate each scenario has provenance
    for scenario in &scenarios {
        assert!(!scenario.name.trim().is_empty(), "scenario has no name");
        assert!(
            scenario.description.len() >= 20,
            "scenario {} lacks stated provenance (description too short)",
            scenario.name
        );
        scenario.validate().expect("scenario is valid");
    }

    // Run a paper book through each scenario and assert capital invariants
    let mut platform = platform()?;
    platform.observe(observations("AAA", 120, 0.0));
    platform.observe(observations("BBB", 120, 0.31));

    // Build a representative position
    buy(&mut platform, "AAA", dec!("10000"), start())?;
    buy(&mut platform, "BBB", dec!("5000"), start())?;

    platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let stress_report = platform
        .stress_report()
        .expect("paper book with position is stressed");

    // Each scenario: book equity must never go negative, loss must be bounded
    for result in &stress_report.scenarios {
        // The equity invariant: paper book capital cannot go negative
        assert!(
            result.equity_after >= 0.0,
            "scenario {} violated paper-trading invariant: equity went negative ({})",
            result.scenario,
            result.equity_after
        );

        // The loss fraction is the equity identity, not a number in a loose
        // band. This said `<= 1.5` — a scenario could report losing half as
        // much again as the book held and pass — and the equity assertion
        // above already bounds a real loss at one whole book.
        let identity =
            ((result.equity_before - result.equity_after) / result.equity_before).max(0.0);
        assert!(
            (result.loss_fraction - identity).abs() < 1e-9,
            "scenario {} reports a loss fraction of {} where its equity moved by {}",
            result.scenario,
            result.loss_fraction,
            identity
        );
        assert!(
            (0.0..=1.0).contains(&result.loss_fraction),
            "scenario {} loss fraction {} is outside a book that cannot go negative",
            result.scenario,
            result.loss_fraction
        );

        // Liquidation cost must be non-negative
        assert!(
            result.liquidation_cost >= 0.0,
            "scenario {} liquidation cost is negative",
            result.scenario
        );
    }

    // Survivability per scenario: at least in most scenarios, the book survives
    let surviving = stress_report
        .scenarios
        .iter()
        .filter(|r| r.loss_fraction < 0.5) // less than 50% loss
        .count();
    assert!(
        surviving >= 5,
        "paper book must survive at least 5 of 8 scenarios moderately (loss < 50%); only {} do",
        surviving
    );

    Ok(())
}

#[test]
fn a_driver_the_understand_stage_established_from_the_tape_is_stressed_through_the_graph()
-> Result<()> {
    // The whole wire, production writer to production reader: UNDERSTAND
    // establishes obj-AAA -> obj-BBB from a real lagged pair on the tape,
    // and SIMULATE, on the next cycle, walks that edge from its driver into
    // the held position. Nothing here seats an edge by hand — a stress path
    // proven only against a hand-claimed graph would say nothing about
    // whether the graph the platform actually builds ever reaches it.
    let count = 120;
    let cause_returns = noise(11, count, 0.02);
    let mut effect_returns = vec![0.0; count];
    let effect_noise = noise(22, count, 0.004);
    for t in 1..count {
        effect_returns[t] = 0.8 * cause_returns[t - 1] + effect_noise[t];
    }
    let mut platform = platform()?;
    platform.observe(bars_from_returns("AAA", &cause_returns, count));
    platform.observe(bars_from_returns("BBB", &effect_returns, count));
    buy(&mut platform, "BBB", dec!("100"), start())?;

    // First cycle: the writer runs. Premise, asserted rather than assumed:
    // the graph now holds the edge the stress will walk.
    platform.run_cycle(start());
    let edges = platform.world().causal().edges().to_vec();
    assert!(
        edges
            .iter()
            .any(|edge| edge.cause == "obj-AAA" && edge.effect == "obj-BBB"),
        "premise: UNDERSTAND did not establish obj-AAA -> obj-BBB: {edges:?}"
    );

    // Second cycle: the reader runs against the graph as it now stands.
    let report = platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let simulate = report
        .stage(Stage::Simulate)
        .expect("the cycle runs every stage");
    let stress = platform.stress_report().expect("the book is stressed");
    assert!(
        stress.causal_drivers >= 1,
        "no driver was walked from the graph the platform built: {}",
        simulate.detail
    );
    let through_aaa = stress
        .causal
        .iter()
        .find(|result| result.scenario == "causal:obj-AAA")
        .unwrap_or_else(|| {
            panic!(
                "the established driver was not stressed through the graph: {}",
                simulate.detail
            )
        });
    let hit = through_aaa
        .positions
        .iter()
        .find(|impact| impact.object_id == "obj-BBB")
        .expect("the walk from obj-AAA reached the held obj-BBB");
    assert!(
        hit.profit_and_loss < 0.0,
        "the worse of the two signs is kept, so the reached position loses: {hit:?}"
    );
    assert!(
        simulate.produced > stress.scenarios.len() + usize::from(stress.adversarial.is_some()),
        "the causal scenarios applied are billed on the stage: {}",
        simulate.detail
    );
    Ok(())
}
