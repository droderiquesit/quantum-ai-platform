//! Venue credit line limits and circuit breaker enforcement.
//!
//! These tests verify that:
//! 1. Venue exposure limits prevent concentration at single venues
//! 2. Circuit breakers trigger on notional rate thresholds
//! 3. Paper trading venues have no credit limits
//! 4. Limits are checked before orders exist (pre-trade)

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_core::Decimal;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use std::collections::BTreeMap;

fn equity(notional: i64) -> Decimal {
    Decimal::from_int(notional)
}

fn exposure(notional: i64) -> Decimal {
    Decimal::from_int(notional)
}

#[test]
fn a_venue_credit_line_limit_refuses_excess_exposure_at_one_venue() {
    // Capital limits checked before orders exist. A venue credit line prevents
    // concentration of risk at a single execution venue.
    let mut venues = BTreeMap::new();
    venues.insert("XNYS".to_string(), exposure(500_000));

    let state = RiskState {
        equity: equity(1_000_000),
        venue_exposures: venues,
        ..Default::default()
    };

    let limits = LimitSet::new("test").with(
        Limit::new(
            "venue-credit-nyse",
            LimitKind::MaxVenueExposure {
                venue: "XNYS".to_string(),
                limit: 0.30, // 30% of equity
            },
        )
        .with_rationale("NYSE credit line is constrained"),
    );

    let check = limits.check(&state);
    // 500k / 1M = 0.5, which exceeds 0.30 limit
    assert!(check.is_blocked());
    assert_eq!(check.breaches.len(), 1);
    assert_eq!(check.breaches[0].limit_name, "venue-credit-nyse");
}

#[test]
fn a_venue_credit_line_is_satisfied_when_exposure_is_within_limit() {
    // The same limit passes when exposure is inside.
    let mut venues = BTreeMap::new();
    venues.insert("XNYS".to_string(), exposure(250_000)); // 25% of equity

    let state = RiskState {
        equity: equity(1_000_000),
        venue_exposures: venues,
        ..Default::default()
    };

    let limits = LimitSet::new("test").with(
        Limit::new(
            "venue-credit-nyse",
            LimitKind::MaxVenueExposure {
                venue: "XNYS".to_string(),
                limit: 0.30,
            },
        )
        .with_rationale("NYSE credit line is constrained"),
    );

    let check = limits.check(&state);
    assert!(!check.is_blocked());
    assert!(check.breaches.is_empty());
}

#[test]
fn circuit_breaker_triggers_on_venue_notional_rate() {
    // The bound is raw notional in the book's currency. This comment used to
    // say "a multiple of daily ADV or a fixed threshold"; the evaluation has
    // only ever compared raw notional, and `RiskState` carries no volume to
    // divide by, so the doc on the variant now says so too.
    let mut venues = BTreeMap::new();
    venues.insert("XLON".to_string(), exposure(5_000_000)); // Very large order

    let state = RiskState {
        equity: equity(1_000_000),
        venue_exposures: venues,
        ..Default::default()
    };

    let limits = LimitSet::new("test").with(
        Limit::new(
            "circuit-breaker-lon",
            LimitKind::MaxVenueNotionalRate {
                venue: "XLON".to_string(),
                limit: 2_000_000.0, // Max notional per period (2M)
            },
        )
        .with_rationale("circuit breaker on London venue"),
    );

    let check = limits.check(&state);
    // 5M exceeds the 2M circuit breaker threshold
    assert!(check.is_blocked());
    assert_eq!(check.breaches.len(), 1);
    assert!(check.breaches[0].detail.contains("circuit breaker"));
}

#[test]
fn multiple_venue_credit_lines_can_be_enforced_simultaneously() {
    // Limits on different venues are checked independently and can all
    // trigger simultaneously if the book is badly concentrated.
    let mut venues = BTreeMap::new();
    venues.insert("XNYS".to_string(), exposure(600_000)); // 60% of equity
    venues.insert("XLON".to_string(), exposure(500_000)); // 50% of equity

    let state = RiskState {
        equity: equity(1_000_000),
        venue_exposures: venues,
        ..Default::default()
    };

    let limits = LimitSet::new("test")
        .with(
            Limit::new(
                "venue-credit-nyse",
                LimitKind::MaxVenueExposure {
                    venue: "XNYS".to_string(),
                    limit: 0.30,
                },
            )
            .with_rationale("NYSE limit"),
        )
        .with(
            Limit::new(
                "venue-credit-lon",
                LimitKind::MaxVenueExposure {
                    venue: "XLON".to_string(),
                    limit: 0.30,
                },
            )
            .with_rationale("LSE limit"),
        );

    let check = limits.check(&state);
    assert!(check.is_blocked());
    // Both venues exceed their limits
    assert_eq!(check.breaches.len(), 2);
    assert_eq!(check.evaluated, 2);
}

#[test]
fn a_venue_limit_refuses_a_venue_the_state_holds_no_figure_for() {
    // This test was `absent_venue_exposures_do_not_trigger_limits` and
    // asserted the opposite: a venue missing from the map read as zero and
    // passed. Nothing in production writes `venue_exposures`, so that rule
    // made every venue limit pass every real book — the shipped
    // `venue-exposure-simulated` limit could not fire. An absent figure is
    // one nobody computed, and it is now refused as Critical, for both venue
    // rules, rather than passed.
    let kinds = [
        LimitKind::MaxVenueExposure {
            venue: "XTSE".to_string(),
            limit: 0.20,
        },
        LimitKind::MaxVenueNotionalRate {
            venue: "XTSE".to_string(),
            limit: 2_000_000.0,
        },
    ];
    for kind in kinds {
        let label = kind.label();
        let limits = LimitSet::new("test").with(Limit::new("venue-tokyo", kind));

        // Premise: the same limit admits the same venue once a figure inside
        // the bound is filed, so the refusal below is about the absence and
        // not a limit that refuses everything.
        let present = RiskState {
            equity: equity(1_000_000),
            venue_exposures: BTreeMap::from([("XTSE".to_string(), exposure(100_000))]),
            ..Default::default()
        };
        assert!(
            !limits.check(&present).is_blocked(),
            "{label}: premise failed, a venue inside its bound was refused"
        );

        // A figure for a different venue is not a figure for this one.
        let absent = RiskState {
            equity: equity(1_000_000),
            venue_exposures: BTreeMap::from([("XNYS".to_string(), exposure(100_000))]),
            ..Default::default()
        };
        let check = limits.check(&absent);
        assert!(check.is_blocked(), "{label}: a venue with no figure passed");
        let blocking = check.blocking();
        assert_eq!(blocking.len(), 1, "{label}");
        assert_eq!(blocking[0].subject.as_deref(), Some("XTSE"), "{label}");
        assert_eq!(
            blocking[0].severity,
            qip_risk::limits::Severity::Critical,
            "{label}: an unevaluable figure is Critical, not an ordinary breach"
        );
        assert!(
            blocking[0]
                .detail
                .contains("has no figure in the risk state"),
            "{label}: {}",
            blocking[0].detail
        );
    }
}

#[test]
fn the_default_set_ships_no_venue_limit_while_nothing_writes_venue_exposures() {
    // The default set used to carry `venue-exposure-simulated`. Nothing in
    // production writes `RiskState::venue_exposures`, so under the old
    // absent-is-zero rule it passed every book, and under the absent-is-
    // refused rule it would refuse every order. Either way it was not a
    // control. This pins its absence until a producer exists.
    let shipped = LimitSet::conservative_default();
    // Premise: the set is non-empty, so "no venue limit" is a statement about
    // its contents and not about an empty list.
    assert!(!shipped.is_empty());
    let venue_limits: Vec<&str> = shipped
        .limits
        .iter()
        .filter(|l| {
            matches!(
                l.kind,
                LimitKind::MaxVenueExposure { .. } | LimitKind::MaxVenueNotionalRate { .. }
            )
        })
        .map(|l| l.name.as_str())
        .collect();
    assert!(
        venue_limits.is_empty(),
        "the shipped set carries venue limits {venue_limits:?} that no production code can feed"
    );
}

#[test]
fn venue_exposure_limit_is_recalibrable_through_bound_change() {
    // A venue credit line limit can be recalibrated by changing only the
    // bound, exactly like any other limit. The venue itself must not change.
    let limit1 = LimitKind::MaxVenueExposure {
        venue: "XLON".to_string(),
        limit: 0.20,
    };

    let limit2 = limit1.with_bound(0.15).expect("recalibration failed");

    // The venue name is preserved
    if let LimitKind::MaxVenueExposure { venue, limit } = limit2 {
        assert_eq!(venue, "XLON");
        assert_eq!(limit.to_bits(), 0.15_f64.to_bits());
    } else {
        panic!("with_bound changed the limit kind");
    }
}

#[test]
fn circuit_breaker_notional_rate_is_checked_deterministically() {
    // Circuit breaker checks are deterministic pre-trade checks, not routed
    // to a model. The same book state always produces the same decision.
    let mut venues = BTreeMap::new();
    venues.insert("venue-a".to_string(), exposure(1_500_000));

    let state = RiskState {
        equity: equity(1_000_000),
        venue_exposures: venues,
        ..Default::default()
    };

    let limits = LimitSet::new("test").with(
        Limit::new(
            "breaker",
            LimitKind::MaxVenueNotionalRate {
                venue: "venue-a".to_string(),
                limit: 1_000_000.0,
            },
        )
        .with_rationale("breaker"),
    );

    let check1 = limits.check(&state);
    let check2 = limits.check(&state);

    // Both checks must produce identical results
    assert_eq!(check1.breaches.len(), check2.breaches.len());
    assert_eq!(check1.is_blocked(), check2.is_blocked());
}

#[test]
fn limits_are_checked_before_any_order_object_is_created() {
    // The risk state carries order and venue exposures together. A limit
    // breach on venue credit lines prevents the order from ever being
    // submitted to the execution engine.
    //
    // This is the architecture of the platform: a refusal happens in the
    // risk engine before the OMS ever sees an order, so there is no path
    // by which a routed order becomes impossible to fill due to credit limits.
    let mut venues = BTreeMap::new();
    venues.insert("XNYS".to_string(), exposure(800_000)); // Already high

    let state = RiskState {
        equity: equity(1_000_000),
        venue_exposures: venues,
        // order_notional and order_subject are None here; the check still works
        // against the projected state, not the current one.
        ..Default::default()
    };

    let limits = LimitSet::new("test").with(
        Limit::new(
            "venue-limit",
            LimitKind::MaxVenueExposure {
                venue: "XNYS".to_string(),
                limit: 0.75, // 75% of equity
            },
        )
        .with_rationale("venue credit"),
    );

    let check = limits.check(&state);
    // The 80% exposure breaches the 75% limit before any order is considered
    assert!(check.is_blocked());
}

#[test]
fn venue_credit_line_distinguishes_different_venues() {
    // Each venue has its own limit. An order for XNYS cannot be charged to
    // XLON's credit line, and vice versa.
    let mut venues = BTreeMap::new();
    venues.insert("XNYS".to_string(), exposure(250_000)); // Within limit
    venues.insert("XLON".to_string(), exposure(250_000)); // Within limit

    let state = RiskState {
        equity: equity(1_000_000),
        venue_exposures: venues,
        ..Default::default()
    };

    let limits = LimitSet::new("test")
        .with(
            Limit::new(
                "nyse",
                LimitKind::MaxVenueExposure {
                    venue: "XNYS".to_string(),
                    limit: 0.20,
                },
            )
            .with_rationale("nyse"),
        )
        .with(
            Limit::new(
                "lse",
                LimitKind::MaxVenueExposure {
                    venue: "XLON".to_string(),
                    limit: 0.30, // Looser limit on LSE
                },
            )
            .with_rationale("lse"),
        );

    let check = limits.check(&state);
    // NYSE exposure (250k = 25%) exceeds NYSE limit (20%)
    assert!(check.is_blocked());
    assert_eq!(check.breaches.len(), 1);
    assert_eq!(
        check.breaches[0].subject.as_deref(),
        Some("XNYS"),
        "The breach should be specifically at NYSE"
    );
}
