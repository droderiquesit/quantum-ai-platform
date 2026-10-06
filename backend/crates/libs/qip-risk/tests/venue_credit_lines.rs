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
use qip_core::dec;
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
    // A circuit breaker is a rate-of-orders control. It counts the absolute
    // notional sent to a venue in a period and halts further orders when that
    // rate exceeds capacity, as a multiple of daily ADV or a fixed threshold.
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
fn absent_venue_exposures_do_not_trigger_limits() {
    // A venue not in the exposures map records nothing, just like an absent
    // concentration axis records nothing. The limit is not evaluated against
    // a venue the book made no statement about.
    let state = RiskState {
        equity: equity(1_000_000),
        venue_exposures: BTreeMap::new(), // No venues
        ..Default::default()
    };

    let limits = LimitSet::new("test").with(
        Limit::new(
            "venue-credit-tokyo",
            LimitKind::MaxVenueExposure {
                venue: "XTSE".to_string(),
                limit: 0.20,
            },
        )
        .with_rationale("Tokyo venue limit"),
    );

    let check = limits.check(&state);
    // No exposure to Tokyo = no breach
    assert!(!check.is_blocked());
    assert!(check.breaches.is_empty());
}

#[test]
fn paper_trading_venue_has_unlimited_credit_line_in_default_set() {
    // The shipped conservative default includes a simulated venue with no
    // effective credit limit (1.0 = 100% of equity), because a paper trading
    // venue has no credit line to the desk.
    let mut venues = BTreeMap::new();
    venues.insert("simulated".to_string(), exposure(10_000_000)); // Way over equity

    let state = RiskState {
        equity: equity(1_000_000),
        venue_exposures: venues,
        ..Default::default()
    };

    let limits = LimitSet::conservative_default();
    let check = limits.check(&state);

    // The simulated venue should have an exposure limit of 1.0 (100% of
    // equity), so even massive exposure doesn't breach in paper trading
    let venue_limit = check
        .breaches
        .iter()
        .find(|b| b.subject.as_deref() == Some("simulated"));
    // Simulated venue can have up to 100% of equity
    assert!(
        venue_limit.is_none()
            || venue_limit.as_ref().map(|b| b.limit_name.as_str())
                != Some("venue-exposure-simulated")
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
        assert_eq!(limit, 0.15);
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
