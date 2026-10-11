//! R12-DATA-2024: a source is scored on reliability from observed
//! availability, errors and latency over a window — `SourceHealth::over` —
//! and not from one HEAD probe.
//!
//! The scorer gave 0.9 to any source whose HEAD answered and 0.1 to any
//! whose HEAD did not, so a source that failed nine monitor windows in ten
//! and happened to answer the assessment's probe scored as reliable as one
//! that never failed.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

mod common;

use common::{AGENT, candidate, licensed_for, now, permissive_robots, probe_for};
use qip_contracts::governance::Usage;
use qip_core::Duration;
use qip_core::error::Result;
use qip_data_finder::health::{HealthObservation, SourceHealth};
use qip_data_finder::{DataFinder, FinderConfig};

const URL: &str = "https://example.com/v1/quotes";

fn reliability_after(history: &[HealthObservation]) -> Result<f64> {
    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 7)?);
    finder.observe_health("rel", history);
    let mut probe = probe_for(URL, "example.com", permissive_robots());
    let licence = licensed_for(&[Usage::Derive])?;
    let decisions = finder.assess(
        vec![candidate("rel", URL, licence, &["EU0001"])?],
        &mut probe,
        now(),
    )?;
    Ok(decisions[0]
        .scores()
        .expect("the source was scored")
        .reliability())
}

#[test]
fn a_source_failing_most_of_its_window_scores_low_even_when_its_probe_answers() -> Result<()> {
    let clean = reliability_after(&[])?;
    assert!(
        (clean - 0.9).abs() < 1e-3,
        "premise: a probe alone scores as before: {clean}"
    );

    let failures: Vec<HealthObservation> = (1..=9)
        .map(|day| HealthObservation::failed(now().saturating_sub(Duration::from_days(day)), 503))
        .collect();
    let flaky = reliability_after(&failures)?;
    // One success in ten: 0.1 + 0.8 * 0.1.
    assert!((flaky - 0.18).abs() < 1e-3, "{flaky}");
    Ok(())
}

#[test]
fn failures_outside_the_window_do_not_count_and_a_repeated_window_is_not_counted_twice()
-> Result<()> {
    let ancient: Vec<HealthObservation> = (40..=48)
        .map(|day| HealthObservation::failed(now().saturating_sub(Duration::from_days(day)), 503))
        .collect();
    let old = reliability_after(&ancient)?;
    assert!(
        (old - 0.9).abs() < 1e-3,
        "failures older than the window counted: {old}"
    );

    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 7)?);
    let window = [
        HealthObservation::failed(now().saturating_sub(Duration::from_days(2)), 500),
        HealthObservation::failed(now().saturating_sub(Duration::from_days(1)), 500),
    ];
    finder.observe_health("rel", &window);
    finder.observe_health("rel", &window);
    assert_eq!(finder.health_observations("rel").len(), 2);
    Ok(())
}

#[test]
fn latency_beyond_the_budget_costs_reliability_in_proportion() -> Result<()> {
    let at = now();
    let fast = SourceHealth::over(
        &[HealthObservation::served(
            at,
            Duration::from_millis(200),
            at,
        )],
        at,
        Duration::from_days(1),
    )?;
    let slow = SourceHealth::over(
        &[HealthObservation::served(at, Duration::from_secs(4), at)],
        at,
        Duration::from_days(1),
    )?;
    assert!((DataFinder::reliability_from(&fast) - 0.9).abs() < 1e-12);
    // A quarter of the budget's speed: 0.1 + 0.8 * 0.25.
    assert!((DataFinder::reliability_from(&slow) - 0.3).abs() < 1e-12);
    Ok(())
}
