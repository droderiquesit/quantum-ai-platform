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
        assert_eq!(
            days_from_civil(y, m, d),
            days,
            "days_from_civil({y},{m},{d})"
        );
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
                return Err(format!(
                    "{days} -> {y}-{m}-{d} -> {}",
                    days_from_civil(y, m, d)
                ));
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
fn a_bare_date_is_the_start_of_that_day_in_utc() {
    // The one short form that survives, and it survives because it is not a
    // guess. A date names a day and no clock reading, so there is no zone to
    // invent; the type has to give the day an instant and midnight UTC is the
    // convention `to_date_string`, `civil_date` and `from_civil` already keep.
    // Several vendors publish date-only fields with nothing more precise
    // behind them — the ECB, Frankfurter and the New York Fed connectors all
    // pass one straight through.
    assert_eq!(
        Timestamp::parse_rfc3339("2026-08-22").unwrap(),
        Timestamp::from_civil(2026, 8, 22)
    );
    assert_eq!(
        Timestamp::parse_rfc3339("2026-08-22").unwrap().to_rfc3339(),
        "2026-08-22T00:00:00.000Z"
    );
}

#[test]
fn a_time_with_no_zone_designator_is_refused_rather_than_read_as_utc() {
    // Until 2026-09-19 `2026-08-22T10:00:00` parsed as 10:00 UTC, and this
    // test asserted that it did. It was the last guess left in this function
    // after the offset-dropping defect was fixed — the one that read
    // `2026-09-19T05:21:00-05:00` five hours early — and it is the same defect
    // from the other side: that one discarded an offset the text carried, this
    // one invented an offset the text withheld. Both produce a well-formed
    // instant that nothing downstream can tell from a correct one, and a
    // record stamped before it was knowable is point-in-time leakage.
    //
    // It was kept on the belief that the platform's own literals depended on
    // it. The line above was the only naked local time in the workspace.
    //
    // `qip-api`'s statement and ledger views parse an `as_of` out of an HTTP
    // query, so this is the difference between a browser in New York being
    // told what the book held at 05:21 local and being told what it held five
    // hours earlier.
    for text in [
        "2026-08-22T10:00:00",
        "2026-08-22T10:00:00.250",
        "2026-08-22 10:00:00",
        "2026-08-22T10:00",
    ] {
        assert!(
            Timestamp::parse_rfc3339(text).is_none(),
            "{text:?} names a clock reading without saying whose clock, and was accepted"
        );
    }

    // The designator in every spelling the platform meets, so the refusal
    // above cannot be satisfied by refusing everything. This half is the one
    // that distinguishes a working gate from one that says no to all input.
    for (text, expected) in [
        ("2026-08-22T10:00:00Z", "2026-08-22T10:00:00.000Z"),
        ("2026-08-22T10:00:00z", "2026-08-22T10:00:00.000Z"),
        ("2026-08-22T10:00:00+00:00", "2026-08-22T10:00:00.000Z"),
        ("2026-08-22T05:00:00-05:00", "2026-08-22T10:00:00.000Z"),
        ("2026-08-22T15:30:00+05:30", "2026-08-22T10:00:00.000Z"),
        ("2026-08-22T05:00:00-0500", "2026-08-22T10:00:00.000Z"),
    ] {
        assert_eq!(
            Timestamp::parse_rfc3339(text)
                .unwrap_or_else(|| panic!("{text:?} carries a designator and was refused"))
                .to_rfc3339(),
            expected,
            "{text:?}"
        );
    }
}

#[test]
fn two_contradictory_zone_claims_on_one_instant_are_refused() {
    // `Z` and a numeric offset say different things about the same reading and
    // neither is the more likely right. A parser that honoured one of them
    // would be choosing, which is the behaviour this function has just stopped
    // doing. `...00ZZ` is here for the same reason: the earlier code trimmed
    // every trailing `Z`, so a string nobody could have meant parsed happily.
    for text in [
        "2026-08-22T10:00:00+05:00Z",
        "2026-08-22T10:00:00-05:00Z",
        "2026-08-22T10:00:00ZZ",
        "2026-08-22Z",
    ] {
        assert!(
            Timestamp::parse_rfc3339(text).is_none(),
            "should reject {text:?}"
        );
    }
}

#[test]
fn rfc3339_rejects_malformed_input() {
    for text in [
        "",
        "not-a-date",
        "2026-13-01",
        "2026-00-10",
        "2026-01-99",
        "2026/01/01",
    ] {
        assert!(
            Timestamp::parse_rfc3339(text).is_none(),
            "should reject {text:?}"
        );
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
        if back == expected {
            Ok(())
        } else {
            Err(format!("{t:?} -> {text} -> {back:?}"))
        }
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
    assert_eq!(
        clock.now(),
        start.saturating_add(Duration::from_hours(2)),
        "must not rewind"
    );
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

// ---------------------------------------------------------------------------
// UTC offsets. Until 2026-09-19 `parse_rfc3339` split the time at the first
// `+` or `-` and read what preceded it as UTC, so every instant below parsed
// to its own wall-clock reading and the zone was lost before the value had a
// type. Nothing downstream could detect it — the result was a well-formed
// instant, simply the wrong one — and a reading stamped earlier than the
// instant it became knowable is the point-in-time leakage the domain rules
// put first. This is not hypothetical: one connector in the tree carries a
// guard written specifically against it, and no other caller had one.
// ---------------------------------------------------------------------------

#[test]
fn a_western_offset_names_an_instant_later_than_its_own_wall_clock() {
    let parsed = Timestamp::parse_rfc3339("2026-09-19T05:21:00-05:00").expect("a valid instant");
    // Premise: the naive reading this used to return is a *different* instant.
    // Without asserting that first, the equality below could pass on a parser
    // that ignored the offset and a renderer that reprinted what it was given.
    let naive = Timestamp::parse_rfc3339("2026-09-19T05:21:00Z").expect("a valid instant");
    assert_ne!(
        parsed, naive,
        "a -05:00 stamp must not parse to the same instant as the same digits in UTC"
    );
    assert_eq!(parsed.to_rfc3339(), "2026-09-19T10:21:00.000Z");
    // The direction as arithmetic rather than as a string: a zone five hours
    // behind UTC means the UTC instant is five hours later.
    assert_eq!(parsed.since(naive), Duration::from_hours(5));
}

#[test]
fn an_eastern_offset_names_an_instant_earlier_than_its_own_wall_clock() {
    let parsed = Timestamp::parse_rfc3339("2026-09-19T05:21:00+05:00").expect("a valid instant");
    let naive = Timestamp::parse_rfc3339("2026-09-19T05:21:00Z").expect("a valid instant");
    assert_ne!(parsed, naive, "a +05:00 stamp is not the same instant as Z");
    assert_eq!(parsed.to_rfc3339(), "2026-09-19T00:21:00.000Z");
    assert_eq!(parsed.since(naive), Duration::from_hours(-5));
}

#[test]
fn a_half_hour_offset_is_applied_to_the_minute_and_may_cross_the_day() {
    // +05:30 exists and is common. It is the case a parser handling only
    // whole hours gets wrong by thirty minutes rather than not at all, and it
    // moves this instant into the previous civil day, which an implementation
    // correcting within the day would fumble.
    let parsed = Timestamp::parse_rfc3339("2026-09-19T05:21:00+05:30").expect("a valid instant");
    let naive = Timestamp::parse_rfc3339("2026-09-19T05:21:00Z").expect("a valid instant");
    assert_ne!(parsed, naive, "a +05:30 stamp is not the same instant as Z");
    assert_eq!(parsed.to_rfc3339(), "2026-09-18T23:51:00.000Z");
    assert_eq!(parsed.civil_date(), (2026, 9, 18), "it crossed the day");
    assert_eq!(
        parsed.since(naive),
        Duration::from_hours(-5) - Duration::from_mins(30)
    );
}

#[test]
fn a_western_offset_may_carry_an_instant_into_the_following_day() {
    let parsed = Timestamp::parse_rfc3339("2026-09-19T21:00:00-05:00").expect("a valid instant");
    assert_eq!(parsed.to_rfc3339(), "2026-09-20T02:00:00.000Z");
    assert_eq!(parsed.civil_date(), (2026, 9, 20));
}

#[test]
fn every_spelling_of_a_zero_offset_is_the_same_instant_as_z() {
    let zulu = Timestamp::parse_rfc3339("2026-09-19T05:00:00Z").expect("a valid instant");
    for text in [
        "2026-09-19T05:00:00+00:00",
        "2026-09-19T05:00:00-00:00",
        "2026-09-19T05:00:00+0000",
        "2026-09-19T05:00:00-0000",
    ] {
        assert_eq!(
            Timestamp::parse_rfc3339(text).expect("a valid instant"),
            zulu,
            "{text} names the same instant as Z"
        );
    }
}

#[test]
fn an_offset_is_applied_without_losing_the_fractional_second() {
    // The fraction is parsed from the same slice the offset was cut out of,
    // so an implementation slicing at the wrong index drops it in silence.
    let parsed = Timestamp::parse_rfc3339("2026-09-19T05:21:00.250-05:00").expect("an instant");
    assert_eq!(parsed.to_rfc3339(), "2026-09-19T10:21:00.250Z");
}

#[test]
fn an_offset_the_parser_cannot_read_is_refused_rather_than_taken_as_zero() {
    // Premise: the same instant with a well-formed offset is admitted, so
    // these are refusals of the offset and not a blanket refusal of the shape.
    assert!(
        Timestamp::parse_rfc3339("2026-09-19T05:21:00+05:00").is_some(),
        "the premise: a well-formed offset is admitted"
    );
    for text in [
        "2026-09-19T05:21:00+5:00",  // a one-digit hour
        "2026-09-19T05:21:00+25:00", // no zone stands 25 hours from UTC
        "2026-09-19T05:21:00+00:60", // sixty minutes is the next hour
        "2026-09-19T05:21:00-00:99",
        "2026-09-19T05:21:00+1",
        "2026-09-19T05:21:00+",
        "2026-09-19T05:21:00-",
        "2026-09-19T05:21:00+0:500",
        "2026-09-19T05:21:00+ab:cd",
        "2026-09-19T05:21:00+05:0",
        "2026-09-19T05:21:00+050",
        "2026-09-19T05:21:00+050000",
    ] {
        assert!(
            Timestamp::parse_rfc3339(text).is_none(),
            "{text} carries an offset nothing can read, and reading it as zero \
             is the defect this whole block exists to prevent"
        );
    }
}

#[test]
fn two_stamps_naming_one_instant_in_different_zones_are_equal() {
    // The property a bitemporal store actually rests on: identity of the
    // instant, independent of the zone a publisher happened to print it in.
    // A backtest keyed on one of these and a live reading keyed on another
    // must agree about ordering, and a parser dropping the offset made them
    // disagree by five hours while every one of them looked well formed.
    let new_york = Timestamp::parse_rfc3339("2026-09-19T05:21:00-05:00").expect("an instant");
    let kolkata = Timestamp::parse_rfc3339("2026-09-19T15:51:00+05:30").expect("an instant");
    let utc = Timestamp::parse_rfc3339("2026-09-19T10:21:00Z").expect("an instant");
    assert_eq!(new_york, utc);
    assert_eq!(kolkata, utc);
    assert_eq!(new_york, kolkata);
}
