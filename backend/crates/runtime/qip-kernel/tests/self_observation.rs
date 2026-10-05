//! The platform watching itself: OBS-003 (level shifts in its own telemetry)
//! and OBS-028 (error budgets deciding whether a release may proceed).

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, ObjectId, dec};
use qip_financial::universe::Universe;
use qip_kernel::central::CellReport;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::{CYCLE_DURATION_SERIES, Platform};
use qip_mesh::delta::DeltaOrder;
use qip_observability::Telemetry;
use qip_observability::aiops::ReleaseDecision;
use qip_observability::metrics::{labels, names};
use qip_risk::limits::LimitSet;

const CELL: &str = "cell-lon-1";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::new("self-observation-test"),
    )
}

/// Gross 120 over a net of 100: 1.2, under §49.1's 1.5 netting floor.
fn thin_report() -> CellReport {
    let contributors = ["100", "20", "-20"]
        .iter()
        .enumerate()
        .map(|(index, size)| qip_contracts::intent::Contributor {
            strategy: StrategyId::new(format!("contributor-{index}")),
            signed_size: qip_core::Decimal::parse(size).unwrap_or(qip_core::Decimal::ZERO),
            inputs: Vec::new(),
        })
        .collect();
    CellReport::new(CELL, start()).with_orders(vec![DeltaOrder {
        order_id: "ord-thin".to_string(),
        strategy: StrategyId::new("objectives-strategy"),
        object_id: ObjectId::from_string("AAA"),
        venue: VenueId::new("XNYS"),
        side: BookSide::Bid,
        quantity: dec!("100"),
        price: dec!("100"),
        simulated: true,
        contributors,
    }])
}

/// A release is refused only on a spent budget that has been measured often
/// enough to page, and it is refused for the plane that owns the objective.
#[test]
fn a_plane_whose_error_budget_is_spent_refuses_a_release_and_other_planes_proceed() -> Result<()> {
    let mut platform = platform()?;
    assert_eq!(
        platform.release_decision("edge", start()),
        ReleaseDecision::Proceed,
        "premise: a platform that has measured nothing blocks nothing"
    );
    platform.ingest_cell_report(thin_report(), start())?;
    assert_eq!(
        platform.release_decision("edge", start()),
        ReleaseDecision::Proceed,
        "premise: one miss is real but is not yet a page-worthy spend"
    );
    for _ in 0..19 {
        platform.ingest_cell_report(thin_report(), start())?;
    }
    match platform.release_decision("edge", start()) {
        ReleaseDecision::Refused { reason } => assert!(
            reason.contains("netting-ratio"),
            "the refusal names the objective whose budget is gone: {reason}"
        ),
        ReleaseDecision::Proceed => panic!("twenty missed observations spend the budget"),
    }
    assert_eq!(
        platform.release_decision("api", start()),
        ReleaseDecision::Proceed,
        "another plane's budget is not this one's"
    );

    // The same decision reaches the LEARN record every cycle it holds.
    let report = platform.run_cycle(start());
    let problems = report.problems();
    assert!(
        problems
            .iter()
            .any(|(_, p)| p.contains("release to `edge` is refused")),
        "LEARN must say which release is blocked: {problems:?}"
    );
    Ok(())
}

/// A shift crossing no configured threshold is found in the platform's own
/// series, counted, and reported once.
#[test]
fn a_step_in_the_platforms_own_series_is_counted_and_reported_once() -> Result<()> {
    let mut platform = platform()?;
    let count = |p: &Platform| {
        p.telemetry()
            .metrics
            .snapshot()
            .counter(names::TELEMETRY_ANOMALIES, &labels([("series", "probe")]))
    };
    assert_eq!(count(&platform), 0, "premise: nothing has been found yet");
    for i in 0..16 {
        let quiet =
            platform.watch_own_series("probe", Timestamp::from_secs(i), 10.0 + (i % 2) as f64);
        assert_eq!(quiet, None, "a flat series raises nothing");
    }
    let mut found = Vec::new();
    for i in 16..40 {
        if let Some(p) =
            platform.watch_own_series("probe", Timestamp::from_secs(i), 40.0 + (i % 2) as f64)
        {
            found.push(p);
        }
    }
    assert_eq!(found.len(), 1, "one step is one report: {found:?}");
    assert!(
        found[0].contains("`probe`"),
        "the report names the series: {}",
        found[0]
    );
    assert_eq!(count(&platform), 1);
    Ok(())
}

/// The cycle feeds the detector its own duration, so the wiring is not only
/// reachable by a caller who remembers to call it.
#[test]
fn a_cycle_feeds_its_own_duration_to_the_self_watch() -> Result<()> {
    let mut platform = platform()?;
    assert_eq!(
        platform.watched_points(CYCLE_DURATION_SERIES),
        0,
        "premise: empty before a cycle"
    );
    platform.run_cycle(start());
    assert_eq!(platform.watched_points(CYCLE_DURATION_SERIES), 1);
    Ok(())
}
