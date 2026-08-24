//! Cross-cell insight leaves the central plane through the gate, or not at all.
//!
//! `qip-confidential` was built with fifty-four tests and wired to nothing —
//! a gate nobody stood at, which is this codebase's recurring trap. These
//! tests drive real cell reports through the platform's own ingestion and
//! read the aggregate back through `Platform::insights_mut`, so the seam
//! under test is the one a console would actually use.

#![allow(clippy::panic_in_result_fn)]
// Exact float equality is the property under test, not an oversight: the same
// seed must reproduce the released value bit for bit, or no incident that
// involved an insight can be replayed. An epsilon here would pass a release
// that drifted, which is the failure the assertion exists to catch.
#![allow(clippy::float_cmp)]

use qip_capital::exposure::CellPosition;
use qip_confidential::budget::Epsilon;
use qip_contracts::Utilisation;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Context, Currency, Decimal, Timestamp, dec};
use qip_financial::asset_class::Sector;
use qip_financial::universe::Universe;
use qip_kernel::central::CellReport;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

const CELLS: [&str; 5] = [
    "cell-dallas-1",
    "cell-london-1",
    "cell-tokyo-1",
    "cell-frankfurt-1",
    "cell-singapore-1",
];

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::new("insights-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        ),
    )
}

fn position(cell: &str, quantity: Decimal) -> CellPosition {
    CellPosition {
        cell: cell.to_string(),
        strategy: StrategyId::new(format!("momentum-{cell}")),
        instrument: "AAA".to_string(),
        sector: Sector::InformationTechnology,
        venue: VenueId::new("XNAS"),
        currency: Currency::USD,
        quantity,
        price: dec!("100"),
    }
}

fn ingest(platform: &mut Platform, cells: &[&str]) -> Result<()> {
    for (index, cell) in cells.iter().enumerate() {
        let report = CellReport::new(*cell, start())
            .with_positions(vec![position(
                cell,
                dec!("10") * Decimal::from((index + 1) as i64),
            )])
            .with_utilisation(vec![(
                StrategyId::new(format!("momentum-{cell}")),
                Utilisation {
                    gross_committed: dec!("1000"),
                    realised_loss: dec!("25"),
                    orders_sent: 4,
                },
            )]);
        platform.ingest_cell_report(report, start())?;
    }
    Ok(())
}

/// The policy bound: the largest gross notional a cell may hold, as a number
/// the platform issues rather than observes.
fn bound() -> Decimal {
    dec!("1000000")
}

#[test]
fn an_insight_over_too_few_cells_is_refused_and_zero_is_not_substituted() -> Result<()> {
    let mut platform = platform()?;
    ingest(&mut platform, &CELLS[..2])?;

    let (insights, plane) = platform.insights_mut();
    let refusal = insights
        .mean_gross_notional(plane, bound(), Epsilon::new(0.25)?)
        .expect_err("an aggregate over two cells was released");
    assert!(
        refusal.message().contains('2'),
        "the refusal does not name the contributor count: {}",
        refusal.message()
    );

    // The premise, proven rather than assumed: the same query over five cells
    // is answerable, so the refusal above was the threshold and not a broken
    // pipeline.
    ingest(&mut platform, &CELLS[2..])?;
    let (insights, plane) = platform.insights_mut();
    let release = insights.mean_gross_notional(plane, bound(), Epsilon::new(0.25)?)?;
    assert!(release.value().is_finite());
    Ok(())
}

#[test]
fn the_same_seed_reproduces_a_release_exactly_and_a_repeat_spends_nothing() -> Result<()> {
    // Two platforms, identical config, identical reports: the released number
    // must be identical, or no incident involving an insight can be replayed.
    let mut first = platform()?;
    let mut second = platform()?;
    ingest(&mut first, &CELLS)?;
    ingest(&mut second, &CELLS)?;

    let (insights, plane) = first.insights_mut();
    let a = insights.mean_gross_notional(plane, bound(), Epsilon::new(0.25)?)?;
    let (insights, plane) = second.insights_mut();
    let b = insights.mean_gross_notional(plane, bound(), Epsilon::new(0.25)?)?;
    assert_eq!(
        a.value(),
        b.value(),
        "the same seed produced different releases"
    );

    // A repeat of the identical query is the cached release, not a fresh
    // draw: asking twice must not buy a second sample of the noise, because
    // averaging samples is how the noise is removed.
    let (insights, plane) = first.insights_mut();
    let again = insights.mean_gross_notional(plane, bound(), Epsilon::new(0.25)?)?;
    assert_eq!(a.value(), again.value(), "a repeat drew fresh noise");
    assert_eq!(
        insights.records().len(),
        1,
        "a repeat was recorded as a second release"
    );
    Ok(())
}

#[test]
fn a_second_statistic_is_a_second_release_and_the_record_keeps_both() -> Result<()> {
    let mut platform = platform()?;
    ingest(&mut platform, &CELLS)?;

    let (insights, plane) = platform.insights_mut();
    insights.mean_gross_notional(plane, bound(), Epsilon::new(0.25)?)?;
    insights.total_realised_loss(plane, dec!("100000"), Epsilon::new(0.25)?)?;

    assert_eq!(
        insights.records().len(),
        2,
        "two distinct queries must be two records; an audit that shows one is hiding one"
    );
    Ok(())
}
