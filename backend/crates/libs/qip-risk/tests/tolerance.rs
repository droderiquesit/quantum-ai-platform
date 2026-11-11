//! The limit in force is the operator's ceiling narrowed by a risk tolerance
//! (ADR 0405). These tests hold the arithmetic; the pre-trade refusal it
//! produces is proven in `qip-risk-engine/tests/tolerance.rs`.

// Exact comparison is the point here: a bound that is a ceiling times a
// fraction is either that number or it is a different limit.
#![allow(clippy::float_cmp)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::Decimal;
use qip_core::dec;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState, Severity};

/// One of every kind. [`position`] is an exhaustive match with no wildcard,
/// so a new kind does not compile until it has a position, and the coverage
/// assertion in the test below then fails until the kind is in this list.
fn every_kind() -> Vec<LimitKind> {
    vec![
        LimitKind::MaxOrderNotional {
            limit: dec!("10000"),
        },
        LimitKind::MaxPositionNotional {
            limit: dec!("250000.5"),
        },
        LimitKind::MaxPositionWeight { limit: 0.1 },
        LimitKind::MaxLeverage { limit: 1.5 },
        LimitKind::MaxNetExposure { limit: 1.0 },
        LimitKind::MaxConcentration {
            axis: "sector".into(),
            limit: 0.3,
        },
        LimitKind::MaxAxisWeight {
            axis: "country".into(),
            limit: 0.7,
        },
        LimitKind::MaxBucketExposure {
            axis: "sector".into(),
            bucket: "energy".into(),
            limit: 0.25,
        },
        LimitKind::MaxVolatility { limit: 0.18 },
        LimitKind::MaxValueAtRisk {
            confidence: 0.99,
            limit: 0.03,
        },
        LimitKind::MaxExpectedShortfall {
            confidence: 0.975,
            limit: 0.05,
        },
        LimitKind::MaxDrawdown { limit: 0.12 },
        LimitKind::MaxDailyLoss { limit: 0.02 },
        LimitKind::MinLiquidity {
            days: 5.0,
            fraction: 0.6,
        },
        LimitKind::MaxDaysToLiquidate { limit: 10.0 },
        LimitKind::MaxCounterpartyExposure { limit: 0.2 },
        LimitKind::MinCashBuffer { limit: 0.02 },
        LimitKind::MaxVenueExposure {
            venue: "sim".into(),
            limit: 0.4,
        },
        LimitKind::MaxVenueNotionalRate {
            venue: "sim".into(),
            limit: 0.05,
        },
        LimitKind::MaxSurvivalGross { limit: 1.0 },
        LimitKind::MaxMarginUtilisation { limit: 0.8 },
        LimitKind::MinFundingBuffer { limit: 0.1 },
    ]
}

const KINDS: usize = 22;

fn position(kind: &LimitKind) -> usize {
    match kind {
        LimitKind::MaxOrderNotional { .. } => 0,
        LimitKind::MaxPositionNotional { .. } => 1,
        LimitKind::MaxPositionWeight { .. } => 2,
        LimitKind::MaxLeverage { .. } => 3,
        LimitKind::MaxNetExposure { .. } => 4,
        LimitKind::MaxConcentration { .. } => 5,
        LimitKind::MaxAxisWeight { .. } => 6,
        LimitKind::MaxBucketExposure { .. } => 7,
        LimitKind::MaxVolatility { .. } => 8,
        LimitKind::MaxValueAtRisk { .. } => 9,
        LimitKind::MaxExpectedShortfall { .. } => 10,
        LimitKind::MaxDrawdown { .. } => 11,
        LimitKind::MaxDailyLoss { .. } => 12,
        LimitKind::MinLiquidity { .. } => 13,
        LimitKind::MaxDaysToLiquidate { .. } => 14,
        LimitKind::MaxCounterpartyExposure { .. } => 15,
        LimitKind::MinCashBuffer { .. } => 16,
        LimitKind::MaxVenueExposure { .. } => 17,
        LimitKind::MaxVenueNotionalRate { .. } => 18,
        LimitKind::MaxSurvivalGross { .. } => 19,
        LimitKind::MaxMarginUtilisation { .. } => 20,
        LimitKind::MinFundingBuffer { .. } => 21,
    }
}

fn tolerances() -> Vec<Decimal> {
    vec![
        Decimal::ZERO,
        dec!("0.000000001"),
        dec!("0.2"),
        dec!("0.333333333"),
        dec!("0.5"),
        dec!("0.999999999"),
        Decimal::ONE,
    ]
}

// The property the whole change exists to hold: a tolerance can tighten a
// limit and can never loosen one. A maximum that came back above its ceiling,
// or a minimum that came back below the operator's floor, would be the
// tolerance granting risk the operator's file refuses.
#[test]
fn for_every_limit_kind_the_narrowed_bound_is_never_looser_than_the_ceiling() {
    let kinds = every_kind();
    let mut covered: Vec<usize> = kinds.iter().map(position).collect();
    covered.sort_unstable();
    assert_eq!(
        covered,
        (0..KINDS).collect::<Vec<_>>(),
        "every LimitKind must appear exactly once in every_kind()"
    );
    // The premise of the two arms below: both directions are present.
    assert!(kinds.iter().any(LimitKind::is_minimum));
    assert!(kinds.iter().any(|kind| !kind.is_minimum()));

    for kind in &kinds {
        let ceiling = Limit::new(kind.label(), kind.clone());
        for tolerance in tolerances() {
            let in_force = ceiling
                .narrowed_by(tolerance)
                .unwrap_or_else(|error| panic!("{} at {tolerance}: {error}", kind.label()));
            let bound = in_force.kind.bound();
            if kind.is_minimum() {
                assert_eq!(
                    in_force,
                    ceiling,
                    "{} at {tolerance}: a minimum stays exactly where the operator put it",
                    kind.label()
                );
                continue;
            }
            assert!(
                bound <= kind.bound(),
                "{} at {tolerance}: in force {bound} is above the ceiling {}",
                kind.label(),
                kind.bound()
            );
            assert!(bound >= 0.0, "{} at {tolerance}: {bound}", kind.label());
            if tolerance == Decimal::ONE {
                assert_eq!(in_force, ceiling, "{}", kind.label());
            } else {
                // Strictly tighter, so the assertion above cannot pass on a
                // narrowing that did nothing.
                assert!(
                    bound < kind.bound(),
                    "{} at {tolerance}: in force {bound} did not narrow {}",
                    kind.label(),
                    kind.bound()
                );
                let narrowing = in_force
                    .narrowing
                    .unwrap_or_else(|| panic!("{}: a narrowed limit says so", kind.label()));
                assert_eq!(narrowing.ceiling, kind.bound());
                assert_eq!(narrowing.tolerance, tolerance);
            }
            if tolerance.is_zero() {
                assert_eq!(bound, 0.0, "{}", kind.label());
            }
        }
    }
}

// "Do not change what any existing limit set allows when the tolerance is 1."
// Equality of the whole set, not of its verdicts on a few states: a set that
// is equal cannot differ on any state.
#[test]
fn a_tolerance_of_one_gives_back_the_shipped_limit_set_itself() {
    let ceiling = LimitSet::conservative_default();
    assert!(!ceiling.is_empty());
    let in_force = ceiling
        .narrowed_by(Decimal::ONE)
        .expect("one is a tolerance");
    assert_eq!(in_force, ceiling);
    assert_eq!(
        serde_json::to_string(&in_force).expect("serialises"),
        serde_json::to_string(&ceiling).expect("serialises"),
        "at tolerance one the set is journalled byte for byte as it was"
    );
}

// Principle 2. A tolerance of 1.2 clamped to 1 would run the desk at the
// ceiling while its mandate says something nobody can act on; -0.1 clamped to
// 0 would halt it with no error anywhere.
#[test]
fn a_tolerance_outside_zero_to_one_is_refused_and_not_clamped() {
    let ceiling = LimitSet::conservative_default();
    for tolerance in [
        dec!("-0.1"),
        dec!("-0.000000001"),
        dec!("1.000000001"),
        dec!("20"),
    ] {
        let error = ceiling
            .narrowed_by(tolerance)
            .expect_err("a tolerance outside [0, 1] narrows nothing");
        assert!(
            error.message().contains("must be a fraction in [0, 1]")
                && error.message().contains("correct the risk_tolerance"),
            "{tolerance}: {}",
            error.message()
        );
    }
    // The premise: the two ends of the range are admitted by the same call.
    assert!(ceiling.narrowed_by(Decimal::ZERO).is_ok());
    assert!(ceiling.narrowed_by(Decimal::ONE).is_ok());
}

fn order_of(notional: Decimal) -> RiskState {
    RiskState {
        equity: dec!("1000000"),
        cash: dec!("1000000"),
        order_notional: Some(notional),
        order_subject: Some("AAA".into()),
        ..RiskState::default()
    }
}

fn order_cap() -> LimitSet {
    LimitSet::new("order-cap").with(Limit::new(
        "order-notional",
        LimitKind::MaxOrderNotional {
            limit: dec!("10000"),
        },
    ))
}

// Clause 5. An order of 7,000 refused under a file that says 10,000 is
// unexplainable unless the breach itself says the limit in force was 5,000
// and why.
#[test]
fn a_breach_of_a_narrowed_limit_names_the_ceiling_the_tolerance_and_the_limit_in_force() {
    let ceiling = order_cap();
    let in_force = ceiling.narrowed_by(dec!("0.5")).expect("a valid tolerance");
    let state = order_of(dec!("7000"));

    assert!(
        ceiling.check(&state).breaches.is_empty(),
        "the premise: 7,000 is inside the operator's 10,000"
    );
    let check = in_force.check(&state);
    assert!(check.is_blocked());
    let breach = check.blocking()[0];
    assert_eq!(breach.bound, 5000.0);
    let narrowing = breach.narrowing.expect("the breach carries the narrowing");
    assert_eq!(narrowing.ceiling, 10000.0);
    assert_eq!(narrowing.tolerance, dec!("0.5"));
    for needle in [
        "the limit in force, 5000,",
        "ceiling of 10000 ",
        "risk tolerance of 0.5 ",
    ] {
        assert!(
            breach.detail.contains(needle) && check.reason().contains(needle),
            "{needle:?} missing from {:?}",
            check.reason()
        );
    }
}

// ADR 0405 D4. Had the ladder stayed on the ceiling, 5,500 against a limit in
// force of 5,000 would sit under the ceiling's warning line of 8,500 and be
// reported as nothing at all.
#[test]
fn the_severity_ladder_follows_the_limit_in_force_and_not_the_ceiling() {
    let ceiling = order_cap();
    let in_force = ceiling.narrowed_by(dec!("0.5")).expect("a valid tolerance");
    let severity = |set: &LimitSet, notional: Decimal| {
        set.check(&order_of(notional))
            .breaches
            .first()
            .map(|breach| breach.severity)
    };
    // Warning from 0.85 × 5,000, breach above 5,000, critical at 1.25 × 5,000.
    assert_eq!(severity(&in_force, dec!("4000")), None);
    assert_eq!(severity(&in_force, dec!("4500")), Some(Severity::Warning));
    assert_eq!(severity(&in_force, dec!("5500")), Some(Severity::Breach));
    assert_eq!(severity(&in_force, dec!("6250")), Some(Severity::Critical));
    // The premise: the ceiling alone says nothing about any of them.
    for notional in [dec!("4000"), dec!("4500"), dec!("5500"), dec!("6250")] {
        assert_eq!(severity(&ceiling, notional), None, "{notional}");
    }
}

// Narrowing twice would square the tolerance and record the first result as
// the operator's ceiling.
#[test]
fn a_set_already_in_force_is_not_narrowed_a_second_time() {
    let in_force = order_cap()
        .narrowed_by(dec!("0.5"))
        .expect("a valid tolerance");
    let error = in_force
        .narrowed_by(dec!("0.5"))
        .expect_err("a narrowed set is not a ceiling");
    assert!(
        error
            .message()
            .contains("already narrowed from a ceiling of 10000"),
        "{}",
        error.message()
    );
}

// Half of a negative maximum is a larger number, so a tolerance would loosen
// it. No shipped kind has one; this is the refusal that keeps that true.
#[test]
fn a_maximum_with_a_negative_or_non_finite_ceiling_is_refused_rather_than_loosened() {
    for limit in [-0.05, f64::NAN, f64::INFINITY] {
        let error = LimitKind::MaxDailyLoss { limit }
            .narrowed_by(dec!("0.5"))
            .expect_err("such a ceiling cannot be narrowed");
        assert!(
            error
                .message()
                .contains("cannot be narrowed by a risk tolerance"),
            "{limit}: {}",
            error.message()
        );
    }
    let error = LimitKind::MaxOrderNotional { limit: dec!("-1") }
        .narrowed_by(dec!("0.5"))
        .expect_err("a negative notional ceiling cannot be narrowed");
    assert!(
        error
            .message()
            .contains("cannot be narrowed by a risk tolerance")
    );
    // The premise: the same kind at an ordinary bound narrows.
    assert_eq!(
        LimitKind::MaxDailyLoss { limit: 0.02 }
            .narrowed_by(dec!("0.5"))
            .expect("narrows")
            .bound(),
        0.01
    );
}
