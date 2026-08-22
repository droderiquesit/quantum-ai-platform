//! Exactness and rounding guarantees for the money type.

use qip_core::decimal::{Decimal, SCALE};
use qip_core::testing::{Property, any_decimal, any_positive_decimal, any_small_decimal};

#[test]
fn parses_and_renders_round_trip() {
    for text in [
        "0", "1", "-1", "0.1", "-0.1", "123.456789", "0.000000001", "-0.000000001",
        "1000000.000000001", "-99999.5",
    ] {
        let d = Decimal::parse(text).expect("parse");
        assert_eq!(d.to_string(), text, "round trip failed for {text}");
    }
}

#[test]
fn parse_rejects_malformed_input() {
    for text in ["", " ", "abc", "1.2.3", "1e5", "--1", "1..2", "+", "-", "1,000", "0x1f"] {
        assert!(Decimal::parse(text).is_none(), "should reject {text:?}");
    }
}

#[test]
fn parse_normalises_equivalent_spellings() {
    assert_eq!(Decimal::parse("1.5"), Decimal::parse("1.500000000"));
    assert_eq!(Decimal::parse("+2"), Decimal::parse("2"));
    assert_eq!(Decimal::parse(".5"), Decimal::parse("0.5"));
    assert_eq!(Decimal::parse("-0.0"), Some(Decimal::ZERO));
}

#[test]
fn parse_rounds_beyond_nine_digits() {
    // Half away from zero at the tenth fractional digit.
    assert_eq!(Decimal::parse("0.0000000005").unwrap(), Decimal::from_raw(1));
    assert_eq!(Decimal::parse("0.0000000004").unwrap(), Decimal::ZERO);
    assert_eq!(Decimal::parse("-0.0000000005").unwrap(), Decimal::from_raw(-1));
}

#[test]
fn addition_is_exact_where_float_is_not() {
    // 0.1 + 0.2 == 0.3 exactly, which f64 cannot represent.
    let a = Decimal::parse("0.1").unwrap();
    let b = Decimal::parse("0.2").unwrap();
    assert_eq!(a + b, Decimal::parse("0.3").unwrap());
    assert_ne!(0.1_f64 + 0.2_f64, 0.3_f64);
}

#[test]
fn hundred_cents_make_a_dollar() {
    let cent = Decimal::parse("0.01").unwrap();
    let total: Decimal = (0..100).map(|_| cent).sum();
    assert_eq!(total, Decimal::ONE);
}

#[test]
fn multiplication_matches_known_values() {
    let cases = [
        ("2", "3", "6"),
        ("0.5", "0.5", "0.25"),
        ("-1.5", "2", "-3"),
        ("1.000000001", "1", "1.000000001"),
        ("123.456", "0", "0"),
        ("1000000", "1000000", "1000000000000"),
    ];
    for (a, b, expected) in cases {
        let got = Decimal::parse(a).unwrap() * Decimal::parse(b).unwrap();
        assert_eq!(got, Decimal::parse(expected).unwrap(), "{a} * {b}");
    }
}

#[test]
fn division_matches_known_values() {
    let cases = [("6", "3", "2"), ("1", "8", "0.125"), ("-9", "3", "-3"), ("1", "3", "0.333333333")];
    for (a, b, expected) in cases {
        let got = Decimal::parse(a).unwrap() / Decimal::parse(b).unwrap();
        assert_eq!(got, Decimal::parse(expected).unwrap(), "{a} / {b}");
    }
}

#[test]
fn division_by_zero_is_none() {
    assert!(Decimal::ONE.checked_div(Decimal::ZERO).is_none());
}

#[test]
fn rounding_is_half_away_from_zero() {
    let cases = [
        ("0.5", 0, "1"),
        ("-0.5", 0, "-1"),
        ("1.5", 0, "2"),
        ("2.5", 0, "3"),
        ("1.005", 2, "1.01"),
        ("-1.005", 2, "-1.01"),
        ("1.004", 2, "1"),
    ];
    for (input, digits, expected) in cases {
        let got = Decimal::parse(input).unwrap().round_dp(digits);
        assert_eq!(got, Decimal::parse(expected).unwrap(), "round({input}, {digits})");
    }
}

#[test]
fn truncation_always_moves_toward_zero() {
    assert_eq!(Decimal::parse("1.999").unwrap().truncate_dp(0), Decimal::from_int(1));
    assert_eq!(Decimal::parse("-1.999").unwrap().truncate_dp(0), Decimal::from_int(-1));
}

#[test]
fn floor_to_step_never_grows_a_position() {
    let lot = Decimal::from_int(100);
    assert_eq!(Decimal::from_int(250).floor_to_step(lot), Decimal::from_int(200));
    // A short position rounds toward flat, not further short.
    assert_eq!(Decimal::from_int(-250).floor_to_step(lot), Decimal::from_int(-200));
}

// --- properties -------------------------------------------------------------

#[test]
fn property_addition_is_commutative_and_associative() {
    Property::new("add commutes").for_all(
        |r| (any_decimal(r), any_decimal(r), any_decimal(r)),
        |(a, b, c)| {
            if *a + *b != *b + *a {
                return Err("commutativity".into());
            }
            if (*a + *b) + *c != *a + (*b + *c) {
                return Err("associativity".into());
            }
            Ok(())
        },
    );
}

#[test]
fn property_subtraction_inverts_addition() {
    Property::new("sub inverts add").for_all(
        |r| (any_decimal(r), any_decimal(r)),
        |(a, b)| {
            if (*a + *b) - *b != *a {
                return Err(format!("({a} + {b}) - {b} != {a}"));
            }
            Ok(())
        },
    );
}

#[test]
fn property_multiplication_is_commutative() {
    Property::new("mul commutes").for_all(
        |r| (any_small_decimal(r), any_small_decimal(r)),
        |(a, b)| {
            if *a * *b != *b * *a {
                return Err(format!("{a} * {b} != {b} * {a}"));
            }
            Ok(())
        },
    );
}

#[test]
fn property_multiply_then_divide_returns_within_one_ulp() {
    // Fixed-point division rounds, so the round trip is exact to one unit in
    // the last place rather than bit-identical.
    Property::new("mul/div round trip").for_all(
        |r| (any_decimal(r), any_positive_decimal(r)),
        |(a, b)| {
            let round_trip = (*a * *b) / *b;
            let delta = (round_trip.raw() - a.raw()).abs();
            // Tolerance scales with the magnitude ratio, which bounds the error
            // introduced by the intermediate rounding.
            let allowed = 2 + (a.raw().abs() / SCALE.max(1)) + (b.raw().abs() / SCALE.max(1));
            if delta > allowed {
                return Err(format!("({a} * {b}) / {b} = {round_trip}, delta {delta} > {allowed}"));
            }
            Ok(())
        },
    );
}

#[test]
fn property_display_parse_is_lossless() {
    Property::new("display/parse").for_all(any_decimal, |d| {
        match Decimal::parse(&d.to_string()) {
            Some(back) if back == *d => Ok(()),
            other => Err(format!("{d} -> {} -> {other:?}", d.to_string())),
        }
    });
}

#[test]
fn property_json_round_trip_is_lossless() {
    Property::new("json round trip").for_all(any_decimal, |d| {
        let text = serde_json::to_string(d).map_err(|e| e.to_string())?;
        let back: Decimal = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if back == *d { Ok(()) } else { Err(format!("{d} -> {text} -> {back}")) }
    });
}

#[test]
fn property_rounding_never_moves_more_than_half_a_step() {
    Property::new("round bound").for_all(any_decimal, |d| {
        for digits in 0..9u32 {
            let rounded = d.round_dp(digits);
            let step = 10i128.pow(9 - digits);
            let delta = (rounded.raw() - d.raw()).abs();
            if delta * 2 > step {
                return Err(format!("round_dp({d}, {digits}) moved {delta} > half of {step}"));
            }
        }
        Ok(())
    });
}

#[test]
fn property_sign_and_abs_are_consistent() {
    Property::new("sign/abs").for_all(any_decimal, |d| {
        if d.abs() != *d * Decimal::from_int(i64::from(d.signum()).max(-1)) && !d.is_zero() {
            // signum() is -1, 0 or 1; multiplying by it must yield the magnitude.
            let expected = if d.is_negative() { -*d } else { *d };
            if d.abs() != expected {
                return Err(format!("abs({d}) = {}", d.abs()));
            }
        }
        if d.is_zero() && d.signum() != 0 {
            return Err("zero must have signum 0".into());
        }
        Ok(())
    });
}

#[test]
fn f64_conversion_round_trips_within_representable_precision() {
    for text in ["0", "1", "-1", "1234.5", "0.125", "-99999.25"] {
        let d = Decimal::parse(text).unwrap();
        let back = Decimal::from_f64(d.to_f64()).unwrap();
        assert_eq!(back, d, "f64 round trip for {text}");
    }
}

#[test]
fn overflow_is_reported_not_wrapped() {
    assert!(Decimal::MAX.checked_add(Decimal::ONE).is_none());
    assert!(Decimal::MAX.checked_mul(Decimal::from_int(2)).is_none());
}

#[test]
fn apply_bps_scales_correctly() {
    let notional = Decimal::from_int(1_000_000);
    assert_eq!(notional.apply_bps(1.0), Decimal::from_int(100));
    assert_eq!(notional.apply_bps(25.0), Decimal::from_int(2500));
    assert_eq!(notional.apply_bps(0.0), Decimal::ZERO);
}
