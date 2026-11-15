//! The cost-anomaly job (R12-FINOPS-10141): JSON export and thresholds in,
//! one journaled task out, a re-read journals nothing, bad input is refused.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;

use qip_core::Decimal;
use qip_core::time::{Duration, Timestamp};
use qip_events::log::EventLog;
use qip_kernel::cost_anomaly_job::run;
use qip_observability::billing::{BillingRow, SpendThreshold};

fn at() -> Timestamp {
    Timestamp::from_secs(1_798_761_600)
}

fn labels() -> BTreeMap<String, String> {
    BTreeMap::from([("team".to_string(), "research".to_string())])
}

fn export(cost: &str) -> String {
    serde_json::to_string(&vec![BillingRow {
        project: "qip-dev".into(),
        service: "Compute Engine".into(),
        labels: labels(),
        cost: Decimal::parse(cost).unwrap(),
        currency: "USD".into(),
        usage_start: at(),
        usage_end: at().saturating_add(Duration::from_hours(1)),
    }])
    .unwrap()
}

fn thresholds() -> String {
    serde_json::to_string(&vec![SpendThreshold {
        project: "qip-dev".into(),
        selector: labels(),
        threshold: Decimal::parse("100").unwrap(),
        window: Duration::from_days(1),
    }])
    .unwrap()
}

#[test]
fn an_export_over_a_declared_threshold_journals_one_task_and_a_reread_journals_none() {
    let mut log = EventLog::in_memory();
    let first = run(&export("250"), &thresholds(), &mut log, at()).unwrap();
    assert_eq!(first.rows_read, 1, "premise: the export was read");
    assert_eq!(first.anomalies, 1);
    assert_eq!(first.journaled.written.len(), 1);
    // The export caught up with a late row: same breach, larger figure.
    let again = run(&export("900"), &thresholds(), &mut log, at()).unwrap();
    assert_eq!(again.anomalies, 1, "premise: the breach is still detected");
    assert!(again.journaled.written.is_empty());
    assert_eq!(again.journaled.duplicates.len(), 1);
}

#[test]
fn an_export_under_the_threshold_journals_nothing() {
    let mut log = EventLog::in_memory();
    let report = run(&export("10"), &thresholds(), &mut log, at()).unwrap();
    assert_eq!(report.rows_read, 1);
    assert_eq!(report.anomalies, 0);
    assert!(report.journaled.written.is_empty());
}

#[test]
fn no_declared_thresholds_is_refused_not_read_as_all_clear() {
    let mut log = EventLog::in_memory();
    let refusal = run(&export("250"), "[]", &mut log, at()).unwrap_err();
    assert!(
        refusal.to_string().contains("no spend thresholds"),
        "{refusal}"
    );
}

#[test]
fn an_unparseable_export_is_refused_not_read_as_no_spend() {
    let mut log = EventLog::in_memory();
    let refusal = run("not json", &thresholds(), &mut log, at()).unwrap_err();
    assert!(refusal.to_string().contains("billing export"), "{refusal}");
}
