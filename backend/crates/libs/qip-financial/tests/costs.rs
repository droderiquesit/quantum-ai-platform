//! The transaction cost model: the square-root impact law and its inverse.
//!
//! `TransactionCostModel::breakeven_participation` inverts the same law
//! `TransactionCostModel::impact_bps` prices, and the two must agree: a
//! participation rate the breakeven names as "where impact eats the alpha"
//! has to be a participation rate at which `impact_bps` actually reports that
//! much impact. Before this file existed, nothing in the crate exercised
//! `costs.rs` at all — `qip-capital`'s capacity sizing was the only caller of
//! `breakeven_participation`, and a defect here would have surfaced first as
//! a mis-sized position rather than a failing test naming the arithmetic.

// Exact float comparison is deliberate below: these assert that a refused or
// capped case yields exactly zero or exactly the cap, not merely something
// close to it.
#![allow(clippy::float_cmp)]

use qip_core::testing::approx_eq;
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};

/// A model with round numbers, so the arithmetic below is checkable by hand:
/// commission 1bp, no tax, half-spread 2.5bp, impact coefficient 40bp.
fn model() -> TransactionCostModel {
    TransactionCostModel::default()
}

#[test]
fn impact_follows_the_square_root_law_below_the_cap() {
    let m = model();
    // Premise: at 25% participation the naive square root is exactly 0.5,
    // comfortably below the 4.0 cap, so this checks the uncapped formula.
    let expected = 40.0 * 0.25_f64.sqrt();
    assert!(approx_eq(m.impact_bps(0.25), expected, 1e-9));
    assert!(approx_eq(expected, 20.0, 1e-9));
}

#[test]
fn impact_is_capped_at_four_times_participation() {
    let m = model();
    // Participation of 400% (four full days of volume in one) and anything
    // beyond it must report the same, capped, impact — not a climbing one.
    let at_cap = m.impact_bps(4.0);
    assert!(approx_eq(at_cap, 80.0, 1e-9), "impact at the cap: {at_cap}");
    assert!(approx_eq(m.impact_bps(9.0), at_cap, 1e-9));
    assert!(approx_eq(m.impact_bps(1_000.0), at_cap, 1e-9));
}

#[test]
fn zero_and_negative_participation_carry_no_impact() {
    let m = model();
    assert_eq!(m.impact_bps(0.0), 0.0);
    assert_eq!(m.impact_bps(-0.5), 0.0);
    assert_eq!(m.impact_bps(f64::NAN), 0.0);
    assert_eq!(m.impact_bps(f64::INFINITY), 0.0);
}

#[test]
fn breakeven_participation_is_where_impact_actually_reaches_the_budget() {
    let m = model();
    // Premise: this alpha budget, after subtracting the linear costs, is
    // small enough that the naive inverse-square lands below the 4.0 cap
    // (budget = 30 - 1 - 0 - 2.5 = 26.5; sqrt-domain check: 26.5 < 80).
    let alpha_bps = 30.0;
    let budget = alpha_bps - m.commission_bps - m.tax_bps - m.half_spread_bps;
    assert!(
        budget < 2.0 * m.impact_coefficient_bps,
        "fixture must stay under the cap boundary: budget {budget}"
    );

    let breakeven = m.breakeven_participation(alpha_bps);
    // The defining property: feeding the reported breakeven back into the
    // impact function must reproduce the budget it claims to exhaust.
    assert!(
        approx_eq(m.impact_bps(breakeven), budget, 1e-6),
        "impact at the reported breakeven ({}) is {}, not the budget {budget}",
        breakeven,
        m.impact_bps(breakeven)
    );
}

/// The mutation this guards: dropping the cap check from
/// `breakeven_participation` makes it return `(budget/coeff)^2` unconditionally,
/// which for a large alpha names a participation several multiples of a full
/// day's volume — a figure `impact_bps` itself never prices, because it caps
/// modelled impact at participation 4.0. This is exactly the inconsistency
/// `qip-capital`'s capacity sizing would have inherited silently.
#[test]
fn breakeven_participation_never_exceeds_the_caps_own_domain() {
    let m = model();
    // A large alpha budget: 100.0 - 1.0 - 0.0 - 2.5 = 96.5 exhausts more bps
    // than the model can ever report as impact (the cap tops out at 80bp),
    // so the true breakeven is "as fast as the model prices at all": 4.0.
    let alpha_bps = 1_000.0;
    let budget = alpha_bps - m.commission_bps - m.tax_bps - m.half_spread_bps;
    assert!(
        budget >= 2.0 * m.impact_coefficient_bps,
        "fixture must exceed the cap boundary to exercise it: budget {budget}"
    );

    let breakeven = m.breakeven_participation(alpha_bps);
    assert!(
        breakeven <= 4.0,
        "breakeven {breakeven} exceeds the participation domain impact_bps ever prices"
    );
    assert!(approx_eq(breakeven, 4.0, 1e-12));
    // And the impact at that reported breakeven must be the capped value,
    // not a number that pretends to consume a budget the model cannot reach.
    assert!(approx_eq(
        m.impact_bps(breakeven),
        2.0 * m.impact_coefficient_bps,
        1e-9
    ));
}

#[test]
fn a_budget_that_cannot_clear_linear_costs_has_no_breakeven() {
    let m = model();
    // Alpha smaller than commission + tax + half-spread: there is no
    // participation, however small, at which trading is worthwhile.
    assert_eq!(m.breakeven_participation(0.0), 0.0);
    assert_eq!(m.breakeven_participation(m.commission_bps), 0.0);
}

#[test]
fn a_zero_impact_coefficient_has_no_breakeven_regardless_of_alpha() {
    // Negotiated instruments price no square-root impact at all; dividing by
    // a zero coefficient must be refused rather than producing infinity.
    let m = TransactionCostModel::negotiated();
    assert_eq!(m.impact_coefficient_bps, 0.0);
    assert_eq!(m.breakeven_participation(1_000.0), 0.0);
}

#[test]
fn total_bps_is_the_sum_of_every_component_at_the_given_participation() {
    let m = model();
    let participation = 0.5;
    let expected = m.commission_bps + m.tax_bps + m.half_spread_bps + m.impact_bps(participation);
    assert!(approx_eq(m.total_bps(participation), expected, 1e-12));
}

#[test]
fn estimate_scales_with_notional_and_includes_the_fixed_fee() {
    let m = TransactionCostModel {
        fixed_fee: qip_core::dec!("5"),
        ..TransactionCostModel::default()
    };
    let notional = qip_core::dec!("1000000");
    let participation = 0.1;
    let cost = m.estimate(notional, participation);

    // Premise: the total-bps figure is non-zero, so the estimate below is
    // actually checking a computed cost rather than a coincidental zero.
    let total_bps = m.total_bps(participation);
    assert!(total_bps > 0.0);

    let expected_bps_cost = notional.apply_bps(total_bps - 0.0);
    // total_bps already sums commission+tax+half_spread+impact; apply_bps of
    // that on the notional plus the fixed fee is the same total the estimate
    // computes component-by-component.
    let expected = expected_bps_cost + m.fixed_fee;
    assert!(approx_eq(cost.to_f64(), expected.to_f64(), 1e-6));

    // A trade with zero notional still pays the fixed fee.
    let zero_notional_cost = m.estimate(qip_core::Decimal::ZERO, participation);
    assert_eq!(zero_notional_cost, m.fixed_fee);
}

#[test]
fn estimate_uses_the_magnitude_of_a_negative_notional() {
    // A sell is represented with a negative notional in some callers; the
    // cost of trading it is the same as the cost of the equivalent buy.
    let m = model();
    let buy = m.estimate(qip_core::dec!("100000"), 0.1);
    let sell = m.estimate(qip_core::dec!("-100000"), 0.1);
    assert_eq!(buy, sell);
}

#[test]
fn days_to_exit_scales_inversely_with_the_permitted_participation_rate() {
    let liquidity = LiquidityProfile {
        average_daily_volume: qip_core::Decimal::from_int(1_000_000),
        max_participation_rate: 0.1,
        ..LiquidityProfile::default()
    };
    let quantity = qip_core::Decimal::from_int(500_000);
    let days = liquidity
        .days_to_exit(quantity)
        .expect("a listed instrument reports a volume-based estimate");
    // 500,000 units at 10% of 1,000,000 ADV per day = 100,000/day = 5 days.
    assert!(approx_eq(days, 5.0, 1e-9), "days: {days}");

    // A negative (short) quantity exits over the same horizon as the long.
    let short_days = liquidity.days_to_exit(-quantity).expect("short quantity");
    assert!(approx_eq(short_days, days, 1e-9));

    // A negotiated instrument has no volume-based estimate at all.
    let negotiated = LiquidityProfile::illiquid(30.0);
    assert!(negotiated.days_to_exit(quantity).is_none());

    // Zero participation policy (nobody may trade this) has no estimate either.
    let frozen = LiquidityProfile {
        average_daily_volume: qip_core::Decimal::from_int(1_000_000),
        max_participation_rate: 0.0,
        ..LiquidityProfile::default()
    };
    assert!(frozen.days_to_exit(quantity).is_none());
}
