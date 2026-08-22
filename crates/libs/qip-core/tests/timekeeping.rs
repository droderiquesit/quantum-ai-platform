//! Calendar arithmetic and deterministic clocks.

use qip_core::testing::{Property, any_timestamp};
use qip_core::time::{
    Clock, Duration, ManualClock, MonotonicClock, SystemClock, Timestamp, civil_from_days,
    days_from_civil,
};

#[test]
fn epoch_is_the_unix_epoch() {
    assert_eq!(Timestamp::EPOCH.to_rfc3339(), "1970-01-01T00:00:00.000Z");
    assert_eq!(Timestamp::EPOCH.civil_date(), (1970, 1, 1));
    // 1970-01-01 was a Thursday; Monday is 0.
    assert_eq!(Timestamp::EPOCH.weekday(), 3);
}

#[test]
fn known_dates_convert_correctly() {
    let cases = [
        (1970, 1, 1, 0i64),
        (1970, 1, 2, 1),
        (1969, 12, 31, -1),
        (2000, 3, 1, 11017),
        (2024, 2, 29, 19782), // leap day
        (2026, 8, 22, 20687),
    ];
    for (y, m, d, days) in cases {
        assert_eq!(days_from_civil(y, m, d), days, "days_from_civil({y},{m},{d})");
        assert_eq!(civil_from_days(days), (y, m, d), "civil_from_days({days})");
    }
}

#[test]
fn property_civil_date_round_trips() {
    Property::new("civil round trip").cases(4000).for_all(
        |r| {
            use qip_core::rng::Rng;
            r.below(80_000) as i64 - 40_000
        },
        |days| {
            let (y, m, d) = civil_from_days(*days);
            if days_from_civil(y, m, d) != *days {
                return Err(format!("{days} -> {y}-{m}-{d} -> {}", days_from_civil(y, m, d)));
            }
            if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
                return Err(format!("out of range: {y}-{m}-{d}"));
            }
            Ok(())
        },
    );
}

#[test]
fn rfc3339_round_trips() {
    for text in [
        "2026-08-22T14:30:00.000Z",
        "1970-01-01T00:00:00.000Z",
        "2099-12-31T23:59:59.999Z",
        "2024-02-29T12:00:00.000Z",
    ] {
        let t = Timestamp::parse_rfc3339(text).expect("parse");
        assert_eq!(t.to_rfc3339(), text);
    }
}

#[test]
fn rfc3339_accepts_shorter_forms() {
    assert_eq!(
        Timestamp::parse_rfc3339("2026-08-22").unwrap(),
        Timestamp::from_civil(2026, 8, 22)
    );
    assert_eq!(
        Timestamp::parse_rfc3339("2026-08-22T10:00:00").unwrap().to_rfc3339(),
        "2026-08-22T10:00:00.000Z"
    );
}

#[test]
fn rfc3339_rejects_malformed_input() {
    for text in ["", "not-a-date", "2026-13-01", "2026-00-10", "2026-01-99", "2026/01/01"] {
        assert!(Timestamp::parse_rfc3339(text).is_none(), "should reject {text:?}");
    }
}

#[test]
fn property_timestamp_serde_round_trips_to_the_millisecond() {
    Property::new("timestamp serde").for_all(any_timestamp, |t| {
        let text = serde_json::to_string(t).map_err(|e| e.to_string())?;
        let back: Timestamp = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        // RFC 3339 rendering keeps milliseconds, so sub-millisecond detail is
        // deliberately dropped on the wire.
        let expected = Timestamp::from_millis(t.as_millis());
        if back == expected { Ok(()) } else { Err(format!("{t:?} -> {text} -> {back:?}")) }
    });
}

#[test]
fn floor_to_bucket_is_idempotent_and_monotone() {
    let minute = Duration::from_mins(1);
    let t = Timestamp::parse_rfc3339("2026-08-22T14:30:45.500Z").unwrap();
    let floored = t.floor_to(minute);
    assert_eq!(floored.to_rfc3339(), "2026-08-22T14:30:00.000Z");
    assert_eq!(floored.floor_to(minute), floored, "idempotent");
    assert!(floored <= t);
}

#[test]
fn start_of_day_truncates() {
    let t = Timestamp::parse_rfc3339("2026-08-22T23:59:59.999Z").unwrap();
    assert_eq!(t.start_of_day().to_rfc3339(), "2026-08-22T00:00:00.000Z");
}

#[test]
fn negative_timestamps_floor_toward_the_past() {
    // Pre-epoch instants must not round the wrong way, which naive integer
    // division would do.
    let t = Timestamp::from_nanos(-1);
    assert_eq!(t.civil_date(), (1969, 12, 31));
    assert_eq!(t.start_of_day().civil_date(), (1969, 12, 31));
}

#[test]
fn manual_clock_only_moves_forward() {
    let clock = ManualClock::new(Timestamp::from_civil(2026, 1, 1));
    let start = clock.now();
    clock.advance(Duration::from_hours(2));
    assert_eq!(clock.now().since(start), Duration::from_hours(2));
    clock.set(Timestamp::from_civil(2020, 1, 1));
    assert_eq!(clock.now(), start.saturating_add(Duration::from_hours(2)), "must not rewind");
}

#[test]
fn monotonic_clock_never_repeats() {
    let clock = MonotonicClock::new(Timestamp::EPOCH);
    let a = clock.now();
    let b = clock.now();
    let c = clock.now();
    assert!(a < b && b < c);
}

#[test]
fn system_clock_is_after_the_epoch() {
    assert!(SystemClock.now() > Timestamp::from_civil(2020, 1, 1));
}

#[test]
fn duration_annualisation_uses_a_365_day_year() {
    assert!((Duration::from_days(365).as_years_f64() - 1.0).abs() < 1e-12);
    assert!((Duration::from_days(730).as_years_f64() - 2.0).abs() < 1e-12);
}
