//! `qip cost-anomalies` (R12-FINOPS-10141): files in, journaled task out.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;

use qip_core::Decimal;
use qip_core::time::{Duration, Timestamp};
use qip_observability::billing::{BillingRow, SpendThreshold};

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("qip-cost-anomalies-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_command_journals_a_breach_from_files_and_reports_a_rerun_as_a_duplicate() {
    let d = dir("run");
    let at = Timestamp::from_secs(1_798_761_600);
    let labels = BTreeMap::from([("team".to_string(), "research".to_string())]);
    let rows = vec![BillingRow {
        project: "qip-dev".into(),
        service: "Compute Engine".into(),
        labels: labels.clone(),
        cost: Decimal::parse("250").unwrap(),
        currency: "USD".into(),
        usage_start: at,
        usage_end: at.saturating_add(Duration::from_hours(1)),
    }];
    let thresholds = vec![SpendThreshold {
        project: "qip-dev".into(),
        selector: labels,
        threshold: Decimal::parse("100").unwrap(),
        window: Duration::from_days(1),
    }];
    std::fs::write(d.join("rows.json"), serde_json::to_string(&rows).unwrap()).unwrap();
    std::fs::write(
        d.join("t.json"),
        serde_json::to_string(&thresholds).unwrap(),
    )
    .unwrap();
    let args: Vec<String> = [
        "--export",
        d.join("rows.json").to_str().unwrap(),
        "--thresholds",
        d.join("t.json").to_str().unwrap(),
        "--log",
        d.join("log").to_str().unwrap(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let first = qip_cli::cost_anomalies::run(&args, at).unwrap();
    assert!(
        first.contains("1 task(s) journaled, 0 duplicate(s)"),
        "{first}"
    );
    let second = qip_cli::cost_anomalies::run(&args, at).unwrap();
    assert!(
        second.contains("0 task(s) journaled, 1 duplicate(s)"),
        "{second}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_missing_export_flag_is_refused_by_name() {
    let refusal = qip_cli::cost_anomalies::run(&[], Timestamp::from_secs(0)).unwrap_err();
    assert!(refusal.to_string().contains("--export"), "{refusal}");
}
