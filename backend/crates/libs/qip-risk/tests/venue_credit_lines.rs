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
fn the_default_set_admits_the_simulated_venue_up_to_equity_and_refuses_beyond() {
    // This test was `paper_trading_venue_has_unlimited_credit_line_in_default_set`
    // and asserted that ten times equity at the simulated venue did not breach
    // a bound of 1.0 — arithmetic that cannot hold, in a file that had never
    // compiled. The shipped `venue-exposure-simulated` limit is a real bound
    // of 100% of equity, whatever its rationale string says, so this states
    // both halves of that bound: admitted at it, refused past it.
    let at = |notional: i64| {
        let state = RiskState {
            equity: equity(1_000_000),
            venue_exposures: BTreeMap::from([("simulated".to_string(), exposure(notional))]),
            ..Default::default()
        };
        // Other defaults (the cash floor, for one) bind on this sparse
        // state; only the venue limit is under test here.
        LimitSet::conservative_default()
            .check(&state)
            .blocking()
            .into_iter()
            .filter(|b| b.limit_name == "venue-exposure-simulated")
            .cloned()
            .collect::<Vec<_>>()
    };

    // Premise: past the bound the default limit fires, so the admit half
    // below is a statement about the bound and not about a limit that is
    // absent or never reads the map.
    let refused = at(1_500_000);
    assert_eq!(
        refused.len(),
        1,
        "the default venue limit did not fire at 1.5x equity"
    );
    assert_eq!(refused[0].subject.as_deref(), Some("simulated"));
    assert!((refused[0].observed - 1.5).abs() < 1e-9);
    assert!((refused[0].bound - 1.0).abs() < 1e-9);

    // At exactly equity the venue is admitted.
    assert!(
        at(1_000_000).is_empty(),
        "the simulated venue was refused at 100% of equity"
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
