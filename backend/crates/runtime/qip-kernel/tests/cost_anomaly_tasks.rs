//! The cost-anomaly task adapter (R12-FINOPS-10141): a billing-export
//! fixture in, one hash-chained task out, a second report of it refused.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;

use qip_core::Decimal;
use qip_core::time::{Duration, Timestamp};
use qip_events::log::EventLog;
use qip_events::topic::Topic;
use qip_kernel::cost_anomaly_tasks::{CostAnomalyTask, journal};
use qip_observability::billing::{BillingRow, SpendThreshold, detect};

fn day(n: i64) -> Timestamp {
    Timestamp::from_secs(1_798_761_600 + n * 86_400) // 2027-01-01T00:00:00Z
}

fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn row(project: &str, team: &str, cost: &str, at: Timestamp) -> BillingRow {
    BillingRow {
        project: project.to_string(),
        service: "Compute Engine".to_string(),
        labels: labels(&[("team", team), ("env", "dev")]),
        cost: Decimal::parse(cost).unwrap(),
        currency: "USD".to_string(),
        usage_start: at,
        usage_end: at.saturating_add(Duration::from_hours(1)),
    }
}

fn research_daily(threshold: &str) -> SpendThreshold {
    SpendThreshold {
        project: "qip-dev".to_string(),
        selector: labels(&[("team", "research")]),
        threshold: Decimal::parse(threshold).unwrap(),
        window: Duration::from_days(1),
    }
}

/// The fixture: day 0 under the ceiling, day 1 over it only once a second
/// team's row (which the selector excludes) is wrongly counted, day 2 over it
/// on research's own rows, and another project's spend that must not count.
fn export() -> Vec<BillingRow> {
    vec![
        row("qip-dev", "research", "40.00", day(0)),
        row("qip-dev", "research", "55.00", day(1)),
        row("qip-dev", "platform", "500.00", day(1)),
        row("qip-dev", "research", "70.00", day(2)),
        row(
            "qip-dev",
            "research",
            "35.50",
            day(2).saturating_add(Duration::from_hours(20)),
        ),
        row("qip-stage", "research", "900.00", day(2)),
    ]
}

#[test]
fn a_journaled_anomaly_is_one_chained_task_and_a_second_report_of_it_is_refused() {
    let mut log = EventLog::in_memory();
    let anomalies = detect(&export(), &[research_daily("100.00")]).unwrap();
    let first = journal(&mut log, &anomalies, day(3)).unwrap();
    assert_eq!(first.written.len(), 1);
    assert!(first.duplicates.is_empty());

    // The export caught up: same breach, larger amount. No second task.
    let mut revised = export();
    revised.push(row(
        "qip-dev",
        "research",
        "12.00",
        day(2).saturating_add(Duration::from_hours(22)),
    ));
    let again = detect(&revised, &[research_daily("100.00")]).unwrap();
    let second = journal(&mut log, &again, day(4)).unwrap();
    assert!(second.written.is_empty(), "{second:?}");
    assert_eq!(second.duplicates, vec![anomalies[0].fingerprint.clone()]);

    let tasks = log.by_topic(Topic::SystemAlert);
    assert_eq!(tasks.len(), 1);
    let task: CostAnomalyTask = serde_json::from_value(tasks[0].payload.clone()).unwrap();
    assert_eq!(task.anomaly, anomalies[0]);
    assert!(task.ask.contains("qip-dev"), "{}", task.ask);
    assert_eq!(log.verify_chain(), Ok(()));
}
