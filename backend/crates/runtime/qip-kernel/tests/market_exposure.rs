//! The book's basis and gap exposure are measured and recorded every cycle
//! (R12-RISK-4095, ADR 0690).
//!
//! The row names price, volatility, gap, basis, Greeks, factor and
//! causal-driver exposure. Basis and gap had a per-candidate input and a
//! stress world and no figure on the cycle a reader holds; these tests drive
//! a whole `Platform::run_cycle` and read `CycleReport::risk_discovery`, the
//! same surface the journal seals, so a figure that exists only in a helper
//! no cycle calls cannot satisfy them.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::float_cmp)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::universe::Universe;
use qip_kernel::config::{PlatformConfig, paper_hedge_candidates};
use qip_kernel::hedge::HOLDING_DAYS;
use qip_kernel::platform::Platform;
use qip_kernel::survival::SurvivalPolicy;
use qip_market::bar::{Bar, Interval};
use qip_market::quote::{Trade, TradeCondition};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::hedge_search::BASIS_QUANTILE;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

const COMMITTED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/universe.json"
);
const EXPOSED: &str = "OBJ00000000000000000NWSC";
const SUBJECT: &str = "OBJ00000000000000000VNTG";

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 10, 1)
}

fn catalogue() -> Universe {
    let text = std::fs::read_to_string(COMMITTED).expect("the committed catalogue is readable");
    qip_financial::catalogue::load(&text, start())
        .expect("the committed catalogue loads")
        .universe
}

fn leverage_only() -> LimitSet {
    LimitSet::new("hedge-candidates-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 3.0 })
            .with_rationale("gross exposure is capped at 3x equity"),
    )
}

fn platform(universe: Universe) -> Result<Platform> {
    let candidates = paper_hedge_candidates(&universe);
    let config = PlatformConfig {
        initial_equity: dec!("1000000"),
        review: ReviewPolicy {
            minimum_surviving_confidence: 0.10,
            ..ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    }
    .with_hedge_candidates(candidates);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        leverage_only(),
    )?;
    platform.set_survival_policy(SurvivalPolicy::shipped());
    Ok(platform)
}

fn bar(object: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: ObjectId::from_string(object),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).expect("a price"),
        high: Decimal::from_f64(open.max(close) * 1.002).expect("a price"),
        low: Decimal::from_f64(open.min(close) * 0.998).expect("a price"),
        close: Decimal::from_f64(close).expect("a price"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: Default::default(),
    }))
}

/// A quiet series ending in `drop` on its last bar.
fn tape(object: &str, phase: f64, drop: f64) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..120)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662 + phase) % 1.0 - 0.5) * 0.008;
            let last = if i == 119 { drop } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + last;
            let at = start().saturating_sub(Duration::from_days(120 - i));
            bar(object, at, open, price)
        })
        .collect()
}

/// A print in every catalogue name, which is the only thing that gives a
/// candidate a reference price to be sized against.
fn print_every_name(platform: &mut Platform, universe: &Universe, at: Timestamp) {
    let prints: Vec<SensedRecord> = universe
        .iter()
        .map(|record| {
            SensedRecord::Trade(Trade {
                object_id: record.object_id.clone(),
                venue: record.venue.clone(),
                at,
                capture_time: at,
                sequence: None,
                price: dec!("100"),
                size: Decimal::from_int(100),
                aggressor: None,
                condition: TradeCondition::Regular,
                trade_id: None,
                quality: Default::default(),
            })
        })
        .collect();
    let count = prints.len();
    assert_eq!(
        platform.observe(prints),
        count,
        "every print must reach the snapshot"
    );
}

fn finding<'a>(
    report: &'a qip_kernel::cycle::CycleReport,
    exposure: &str,
) -> &'a qip_kernel::cycle::ExposureFinding {
    report
        .risk_discovery
        .market_exposure
        .iter()
        .find(|found| found.exposure == exposure)
        .unwrap_or_else(|| panic!("the cycle reported no {exposure} finding"))
}

fn book_with_a_short_leg() -> Result<(Platform, Universe)> {
    let universe = catalogue();
    let mut platform = platform(catalogue())?;
    platform.observe(tape(EXPOSED, 0.0, -0.09));
    platform.observe(tape(SUBJECT, 0.0, -0.09));
    // Long 720,000 of one name and short 100,000 of another: the short is a
    // held leg carried against a net-long book, which is what the basis
    // figure prices.
    // A short needs a stated borrow; shorting is refused without one.
    platform.observe_borrow(SUBJECT, dec!("100000"), start(), start())?;
    for (object, side, quantity, id) in [
        (EXPOSED, Side::Buy, dec!("7200"), "prop-long"),
        (SUBJECT, Side::Sell, dec!("1000"), "prop-short"),
    ] {
        let order = platform.order_from(
            ObjectId::from_string(object),
            side,
            quantity,
            dec!("100"),
            id,
            vec![format!("hyp-{id}")],
            start(),
        );
        platform.submit_order(order, start())?;
    }
    print_every_name(&mut platform, &universe, start());
    Ok((platform, universe))
}

#[test]
fn a_cycle_reports_the_basis_of_the_hedge_leg_the_book_holds_and_no_bound_decides_it() -> Result<()>
{
    let (mut platform, universe) = book_with_a_short_leg()?;
    let declared = paper_hedge_candidates(&universe);
    let basis_volatility = declared
        .iter()
        .find(|candidate| candidate.instrument.as_str() == SUBJECT)
        .expect("premise: the short name is a declared hedge candidate")
        .basis_volatility;

    let report = platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let basis = finding(&report, "basis");

    // Premise first: a figure was measured, not skipped.
    let figure = basis
        .figure
        .unwrap_or_else(|| panic!("basis was not measured: {}", basis.detail));
    // The held leg's notional is whatever the book holds in the short name,
    // read back from the platform rather than assumed from the order.
    let held = platform
        .positions_at_cost()
        .get(SUBJECT)
        .map(|(quantity, price)| (*quantity * *price).abs().to_f64())
        .expect("premise: the book holds the short name");
    assert!(held > 0.0, "premise: a short leg is held");
    let expected = held * basis_volatility * HOLDING_DAYS.sqrt() * BASIS_QUANTILE;
    assert!(
        (figure - expected).abs() <= expected * 1e-9,
        "basis {figure} against {expected} for a {held} leg"
    );
    // No owner has stated a bound, so the finding says so rather than
    // passing.
    assert_eq!(basis.verdict, "unbounded");
    assert_eq!(basis.bound, None);
    Ok(())
}

#[test]
fn a_book_holding_no_leg_against_itself_measures_a_zero_basis_not_an_absent_one() -> Result<()> {
    let universe = catalogue();
    let mut platform = platform(catalogue())?;
    platform.observe(tape(EXPOSED, 0.0, -0.09));
    let order = platform.order_from(
        ObjectId::from_string(EXPOSED),
        Side::Buy,
        dec!("7200"),
        dec!("100"),
        "prop-long-only",
        vec!["hyp-long-only".to_string()],
        start(),
    );
    platform.submit_order(order, start())?;
    print_every_name(&mut platform, &universe, start());

    let report = platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let basis = finding(&report, "basis");
    assert_eq!(basis.figure, Some(0.0), "{}", basis.detail);
    assert_eq!(basis.verdict, "unbounded");
    Ok(())
}

#[test]
fn a_cycle_reports_the_gap_of_the_worst_tail_world_as_a_loss_on_the_books_equity() -> Result<()> {
    let (mut platform, _universe) = book_with_a_short_leg()?;
    let report = platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let gap = finding(&report, "gap");

    let stress = platform
        .stress_report()
        .expect("premise: the cycle's SIMULATE stage produced a stress report");
    let worst = stress
        .tail_worlds
        .iter()
        .map(|world| world.loss_fraction)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        !stress.tail_worlds.is_empty() && worst > 0.0,
        "premise: a tail world took something from the book"
    );
    let figure = gap
        .figure
        .unwrap_or_else(|| panic!("gap was not measured: {}", gap.detail));
    let expected = worst * platform.equity().to_f64();
    assert!(
        (figure - expected).abs() <= expected * 1e-9,
        "gap {figure} against {expected}"
    );
    assert_eq!(gap.verdict, "unbounded");
    Ok(())
}

#[test]
fn the_exposure_findings_are_sealed_on_the_journal_entry_the_cycle_replays_from() -> Result<()> {
    let (mut platform, _universe) = book_with_a_short_leg()?;
    let report = platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    assert_eq!(
        report.risk_discovery.market_exposure.len(),
        2,
        "premise: basis and gap were both reported"
    );
    let entries = platform.journal_entries()?;
    let entry = entries.last().expect("the cycle sealed a journal entry");
    assert_eq!(
        entry.risk_discovery.as_ref().map(|d| &d.market_exposure),
        Some(&report.risk_discovery.market_exposure)
    );
    Ok(())
}
