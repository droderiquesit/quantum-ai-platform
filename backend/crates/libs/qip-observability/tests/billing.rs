//! Cost anomalies over the billing export (R12-FINOPS-10141): detection
//! against a reference sum written here, and the fingerprint's stability.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;

use qip_core::Decimal;
use qip_core::time::{Duration, Timestamp};
use qip_observability::billing::{BillingRow, SpendThreshold, detect, fingerprint};

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
fn spend_over_a_threshold_in_a_window_raises_one_anomaly_carrying_project_labels_amount_and_threshold()
 {
    // Reference: the selected rows' sum per day, computed here by hand.
    let anomalies = detect(&export(), &[research_daily("100.00")]).unwrap();
    assert_eq!(anomalies.len(), 1, "{anomalies:?}");
    let a = &anomalies[0];
    assert_eq!(a.project, "qip-dev");
    assert_eq!(a.labels, labels(&[("team", "research")]));
    assert_eq!(a.amount, Decimal::parse("105.50").unwrap());
    assert_eq!(a.threshold, Decimal::parse("100.00").unwrap());
    assert_eq!(a.rows, 2);
    assert_eq!((a.window_start, a.window_end), (day(2), day(3)));
    assert_eq!(a.currency, "USD");

    // At the exact figure, spend has not exceeded it.
    let at = detect(&export(), &[research_daily("105.50")]).unwrap();
    assert!(at.is_empty(), "{at:?}");
}

#[test]
fn the_fingerprint_names_the_breach_and_not_its_size() {
    let first = detect(&export(), &[research_daily("100.00")]).unwrap();
    let mut revised = export();
    revised.push(row(
        "qip-dev",
        "research",
        "12.00",
        day(2).saturating_add(Duration::from_hours(22)),
    ));
    let second = detect(&revised, &[research_daily("100.00")]).unwrap();
    // Premise: the late row changed the amount.
    assert_ne!(first[0].amount, second[0].amount);
    assert_eq!(first[0].fingerprint, second[0].fingerprint);
    // Every part the key is made of moves it.
    let base = || {
        fingerprint(
            "qip-dev",
            &labels(&[("team", "research")]),
            Decimal::parse("100.00").unwrap(),
            "USD",
            day(2),
            day(3),
        )
    };
    assert_eq!(base(), first[0].fingerprint);
    let others = [
        fingerprint(
            "qip-stage",
            &labels(&[("team", "research")]),
            Decimal::parse("100.00").unwrap(),
            "USD",
            day(2),
            day(3),
        ),
        fingerprint(
            "qip-dev",
            &labels(&[("team", "platform")]),
            Decimal::parse("100.00").unwrap(),
            "USD",
            day(2),
            day(3),
        ),
        fingerprint(
            "qip-dev",
            &labels(&[("team", "research")]),
            Decimal::parse("99.00").unwrap(),
            "USD",
            day(2),
            day(3),
        ),
        fingerprint(
            "qip-dev",
            &labels(&[("team", "research")]),
            Decimal::parse("100.00").unwrap(),
            "EUR",
            day(2),
            day(3),
        ),
        fingerprint(
            "qip-dev",
            &labels(&[("team", "research")]),
            Decimal::parse("100.00").unwrap(),
            "USD",
            day(1),
            day(2),
        ),
    ];
    for other in others {
        assert_ne!(other, base());
    }
}

#[test]
fn rows_the_threshold_cannot_be_judged_against_are_refused_by_name() {
    let mut mixed = export();
    let mut euro = row("qip-dev", "research", "10.00", day(2));
    euro.currency = "EUR".to_string();
    mixed.push(euro);
    let refusal = detect(&mixed, &[research_daily("100.00")]).unwrap_err();
    assert!(refusal.message().contains("EUR"), "{}", refusal.message());

    let mut backwards = row("qip-dev", "research", "1.00", day(2));
    backwards.usage_end = day(1);
    assert!(detect(&[backwards], &[research_daily("100.00")]).is_err());

    for bad in [research_daily("0"), {
        let mut t = research_daily("100.00");
        t.window = Duration::from_secs(0);
        t
    }] {
        assert!(detect(&export(), &[bad]).is_err());
    }
}
