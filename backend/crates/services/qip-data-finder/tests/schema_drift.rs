//! Schema drift: naming exactly what moved, and stopping the source.
//!
//! The case worth being precise about is the retype. A vanished field breaks
//! a parser loudly and gets fixed the same afternoon. A field that changed
//! from an integer to a string of digits keeps parsing, keeps producing
//! numbers, and the numbers are wrong — which is why this quarantines rather
//! than degrades.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{AGENT, candidate, licensed_for, now, permissive_robots, probe_for};
use qip_contracts::governance::Usage;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_data_finder::finder::{DataFinder, FinderConfig, MonitorOutcome};
use qip_data_finder::health::HealthObservation;
use qip_data_finder::schema::{DriftSeverity, FieldType, SourceSchema};

const URL: &str = "https://example.com/data/prices.json";

#[test]
fn drift_names_exactly_which_fields_appeared_vanished_and_changed_type() -> Result<()> {
    let before = SourceSchema::parse(
        r#"{"symbol":"EU0001","bid":10.25,"ask":10.27,"volume":41000,"venue":"XPAR"}"#,
    )?;
    let after = SourceSchema::parse(
        r#"{"symbol":"EU0001","bid":10.25,"ask":10.27,"volume":"41,000","currency":"EUR"}"#,
    )?;

    let drift = before.drift_to(&after);
    assert!(!drift.is_stable());
    assert_eq!(drift.appeared(), ["currency".to_string()]);
    assert_eq!(drift.vanished(), ["venue".to_string()]);
    assert_eq!(drift.retyped().len(), 1);

    let retype = &drift.retyped()[0];
    assert_eq!(retype.field, "volume");
    assert_eq!(retype.was, FieldType::Integer);
    assert_eq!(retype.now, FieldType::Text);
    assert!(
        drift
            .describe()
            .contains("`volume` changed from integer to text")
    );

    // Fields that did not move are not reported, so the record is the change
    // rather than the payload.
    assert!(!drift.describe().contains("symbol"));
    Ok(())
}

#[test]
fn a_retype_is_ranked_above_a_removal_because_it_fails_silently() -> Result<()> {
    let base = SourceSchema::parse(r#"{"a":1,"b":2}"#)?;
    let removed = SourceSchema::parse(r#"{"a":1}"#)?;
    let retyped = SourceSchema::parse(r#"{"a":"1","b":2}"#)?;
    let added = SourceSchema::parse(r#"{"a":1,"b":2,"c":3}"#)?;

    assert_eq!(base.drift_to(&base).severity(), DriftSeverity::Stable);
    assert_eq!(base.drift_to(&added).severity(), DriftSeverity::Additive);
    assert_eq!(base.drift_to(&removed).severity(), DriftSeverity::Breaking);
    assert_eq!(base.drift_to(&retyped).severity(), DriftSeverity::Silent);

    assert!(!DriftSeverity::Stable.requires_quarantine());
    assert!(!DriftSeverity::Additive.requires_quarantine());
    assert!(DriftSeverity::Breaking.requires_quarantine());
    assert!(DriftSeverity::Silent.requires_quarantine());
    Ok(())
}

#[test]
fn nested_fields_are_named_at_the_level_that_changed() -> Result<()> {
    let before = SourceSchema::parse(r#"{"quote":{"bid":1,"ask":2},"meta":{"venue":"X"}}"#)?;
    let after = SourceSchema::parse(r#"{"quote":{"bid":1.5,"ask":2},"meta":{"venue":"X"}}"#)?;

    let drift = before.drift_to(&after);
    assert_eq!(drift.retyped().len(), 1);
    assert_eq!(drift.retyped()[0].field, "quote.bid");
    assert!(
        !drift.describe().contains("meta"),
        "an untouched branch must not be reported as changed"
    );
    Ok(())
}

#[test]
fn the_fingerprint_moves_with_the_shape_and_not_with_the_values() -> Result<()> {
    let one = SourceSchema::parse(r#"{"bid":10.25,"volume":41000}"#)?;
    let same_shape = SourceSchema::parse(r#"{"bid":99.99,"volume":7}"#)?;
    let different_shape = SourceSchema::parse(r#"{"bid":10.25,"volume":"41000"}"#)?;

    assert_eq!(one.fingerprint(), same_shape.fingerprint());
    assert_ne!(one.fingerprint(), different_shape.fingerprint());

    // Field order in the payload is not part of the shape.
    let reordered = SourceSchema::parse(r#"{"volume":41000,"bid":10.25}"#)?;
    assert_eq!(one.fingerprint(), reordered.fingerprint());
    Ok(())
}

#[test]
fn a_field_that_disagrees_with_itself_inside_one_payload_is_recorded_as_mixed() -> Result<()> {
    // An array of records where one record types a field differently. A
    // parser that took the last record's shape would report no drift at all.
    let schema = SourceSchema::parse(r#"[{"px":1},{"px":"1"}]"#)?;
    assert_eq!(schema.fields().get("px"), Some(&FieldType::Mixed));
    Ok(())
}

#[test]
fn a_source_whose_schema_drifted_is_quarantined_rather_than_consumed() -> Result<()> {
    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 3)?);
    let mut probe = probe_for(URL, "example.com", permissive_robots());
    let decisions = finder.assess(
        vec![candidate(
            "drifter",
            URL,
            licensed_for(&[Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    assert!(decisions[0].is_registered());
    assert!(
        !finder
            .registered("drifter")
            .ok_or_else(|| Error::not_found("the registration"))?
            .is_quarantined()
    );

    // The source now serves `volume` as a string. Availability is perfect.
    let drifted =
        SourceSchema::parse(r#"{"symbol":"EU0001","bid":10.25,"ask":10.27,"volume":"41,000"}"#)?;
    let later = now().saturating_add(Duration::from_mins(10));
    let observations = healthy_observations(later);

    let outcome = finder.monitor(
        "drifter",
        &observations,
        Some(&drifted),
        later,
        Duration::from_hours(1),
    )?;

    let MonitorOutcome::Quarantined { drift, reason } = outcome else {
        return Err(Error::invalid(format!(
            "a drifted source must be quarantined, not reported {}",
            "healthy"
        )));
    };
    assert_eq!(drift.retyped().len(), 1);
    assert_eq!(drift.retyped()[0].field, "volume");
    assert!(reason.contains("changed meaning"));
    assert!(
        finder
            .registered("drifter")
            .ok_or_else(|| Error::not_found("the registration"))?
            .is_quarantined(),
        "the registry must stop offering a quarantined source"
    );
    Ok(())
}

#[test]
fn drift_is_checked_before_health_so_a_healthy_drifted_source_still_stops() -> Result<()> {
    // The dangerous shape: 100% availability, low latency, wrong numbers.
    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 3)?);
    let mut probe = probe_for(URL, "example.com", permissive_robots());
    finder.assess(
        vec![candidate(
            "drifter",
            URL,
            licensed_for(&[Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;

    let later = now().saturating_add(Duration::from_mins(10));
    let perfect_health = healthy_observations(later);
    let stable = SourceSchema::parse(common::QUOTE_PAYLOAD)?;

    // Unchanged shape and perfect health reports healthy...
    assert!(matches!(
        finder.monitor(
            "drifter",
            &perfect_health,
            Some(&stable),
            later,
            Duration::from_hours(1)
        )?,
        MonitorOutcome::Healthy { .. }
    ));

    // ...and the same health with a moved shape does not.
    let moved =
        SourceSchema::parse(r#"{"symbol":"EU0001","bid":"10.25","ask":10.27,"volume":41000}"#)?;
    assert!(matches!(
        finder.monitor(
            "drifter",
            &perfect_health,
            Some(&moved),
            later,
            Duration::from_hours(1)
        )?,
        MonitorOutcome::Quarantined { .. }
    ));
    Ok(())
}

#[test]
fn health_over_a_window_nobody_observed_is_an_error_rather_than_a_perfect_score() -> Result<()> {
    use qip_data_finder::health::SourceHealth;
    let error = SourceHealth::over(&[], now(), Duration::from_hours(1)).unwrap_err();
    assert!(matches!(error, Error::NotFound(_)));
    assert!(error.message().contains("not a healthy source"));
    Ok(())
}

fn healthy_observations(until: Timestamp) -> Vec<HealthObservation> {
    (1..=5)
        .map(|step| {
            let at = until.saturating_sub(Duration::from_mins(6 - step));
            HealthObservation::served(at, Duration::from_millis(30), at)
        })
        .collect()
}
