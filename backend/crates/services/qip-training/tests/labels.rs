//! Each outcome label equals a value worked out by hand, at each horizon.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_training::labels::{LabelBuilder, MidSeries, Print, Side};

fn t(s: i64) -> Timestamp {
    Timestamp::from_secs(1_700_000_000 + s)
}
fn secs(s: i64) -> Duration {
    Duration::from_secs(s)
}
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

/// Mids at whole seconds 0..=5: 100, 101, 102, 100, 99, 103.
fn series() -> Result<MidSeries> {
    MidSeries::new(
        [100.0, 101.0, 102.0, 100.0, 99.0, 103.0]
            .iter()
            .enumerate()
            .map(|(i, p)| (t(i as i64), *p))
            .collect(),
    )
}

#[test]
fn future_return_at_each_horizon_is_the_hand_computed_ratio() -> Result<()> {
    let s = series()?;
    let b = LabelBuilder::new(&s);
    let got = b.at_horizons(&[secs(2), secs(4), secs(5)], |b, h| {
        b.future_return(t(0), h)
    })?;
    assert_eq!(got.len(), 3, "premise: one label per horizon");
    assert!(near(got[&secs(2).as_nanos()], 0.02));
    assert!(near(got[&secs(4).as_nanos()], -0.01));
    assert!(near(got[&secs(5).as_nanos()], 0.03));
    Ok(())
}

#[test]
fn realized_volatility_is_the_root_of_summed_squared_log_returns() -> Result<()> {
    let s = series()?;
    let b = LabelBuilder::new(&s);
    let expect =
        (f64::ln(1.01).powi(2) + f64::ln(102.0 / 101.0).powi(2) + f64::ln(100.0 / 102.0).powi(2))
            .sqrt();
    assert!(near(b.realized_volatility(t(0), secs(3))?, expect));
    // One tick only: a single squared log return.
    assert!(near(
        b.realized_volatility(t(0), secs(1))?,
        f64::ln(1.01).abs()
    ));
    Ok(())
}

#[test]
fn adverse_selection_is_signed_against_the_side_that_filled() -> Result<()> {
    let s = series()?;
    let b = LabelBuilder::new(&s);
    // Bought at 101.5 at t=1; the mid at t=4 is 99: the price fell after buying.
    assert!(near(
        b.adverse_selection(Side::Buy, 101.5, t(1), secs(3))?,
        (101.5 - 99.0) / 101.5
    ));
    // Sold at 101.5: the same fall is in the seller's favour.
    assert!(near(
        b.adverse_selection(Side::Sell, 101.5, t(1), secs(3))?,
        -(101.5 - 99.0) / 101.5
    ));
    Ok(())
}

#[test]
fn market_impact_is_positive_when_the_price_moves_in_the_traded_direction() -> Result<()> {
    let s = series()?;
    let b = LabelBuilder::new(&s);
    // Buy at t=0, mid 100 -> 102 at t=2: +2%.
    assert!(near(b.market_impact(Side::Buy, t(0), secs(2))?, 0.02));
    assert!(near(b.market_impact(Side::Sell, t(0), secs(2))?, -0.02));
    Ok(())
}

#[test]
fn a_resting_order_is_filled_only_by_a_print_through_its_limit_inside_the_horizon() -> Result<()> {
    let s = series()?;
    let b = LabelBuilder::new(&s);
    let prints = [
        Print {
            at: t(2),
            price: 100.0,
        },
        Print {
            at: t(4),
            price: 99.4,
        },
    ];
    // Buy limit 99.5: the 99.4 print at t=4 trades through it, the 100.0 does not.
    assert!(near(
        b.fill_outcome(Side::Buy, 99.5, t(0), secs(3), &prints)?,
        0.0
    ));
    assert!(near(
        b.fill_outcome(Side::Buy, 99.5, t(0), secs(5), &prints)?,
        1.0
    ));
    // A print at the decision instant itself is not in (t, t+h].
    let at_t = [Print {
        at: t(0),
        price: 50.0,
    }];
    assert!(near(
        b.fill_outcome(Side::Buy, 99.5, t(0), secs(5), &at_t)?,
        0.0
    ));
    Ok(())
}

#[test]
fn opportunity_decay_is_the_share_of_the_initial_edge_still_available() -> Result<()> {
    let s = series()?;
    let b = LabelBuilder::new(&s);
    // Buy toward 103 from mid 100: edge 3. At t=2 mid is 102: 1 left.
    assert!(near(
        b.opportunity_decay(Side::Buy, 103.0, t(0), secs(2))?,
        1.0 / 3.0
    ));
    assert!(near(
        b.opportunity_decay(Side::Buy, 103.0, t(0), secs(5))?,
        0.0
    ));
    // The price moved away from the target at t=4 (99): more edge, not less.
    assert!(near(
        b.opportunity_decay(Side::Buy, 103.0, t(0), secs(4))?,
        4.0 / 3.0
    ));
    assert!(
        b.opportunity_decay(Side::Buy, 100.0, t(0), secs(2))
            .is_err()
    );
    Ok(())
}

#[test]
fn a_horizon_the_series_does_not_cover_is_refused_not_shortened() -> Result<()> {
    let s = series()?;
    let b = LabelBuilder::new(&s);
    assert!(
        b.future_return(t(0), secs(5)).is_ok(),
        "premise: the full window is fine"
    );
    assert!(b.future_return(t(0), secs(6)).is_err());
    assert!(b.realized_volatility(t(3), secs(3)).is_err());
    assert!(b.future_return(t(0), secs(0)).is_err());
    assert!(MidSeries::new(vec![(t(1), 100.0), (t(1), 101.0)]).is_err());
    Ok(())
}
