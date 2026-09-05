//! Cashflow forecasting, commitments and capital calls.
//!
//! The failures these prevent are the ones that make a private book look
//! solvent when it is not: a call schedule read at an instant before it was
//! published, a flow dated before the fund existed, a schedule that promises
//! more calls than were ever committed, and a present value that quietly
//! dropped the flows already settled.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion aborting a `Result`-returning function is a bug. In a test the
// assertion is the deliverable and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp, dec};
use qip_financial::cashflow::{
    CashflowForecast, CashflowKind, Commitment, CommitmentBook, ForecastCashflow,
};
use qip_financial::extensions::PrivateAssetDetails;

fn origin() -> Timestamp {
    Timestamp::from_civil(2020, 1, 1)
}

fn day(n: i64) -> Timestamp {
    origin().saturating_add(Duration::from_days(n))
}

fn private_asset(committed: Decimal, called: Decimal) -> PrivateAssetDetails {
    PrivateAssetDetails {
        vintage_year: 2020,
        committed_capital: committed,
        called_capital: called,
        distributed_capital: Decimal::ZERO,
        residual_value: Decimal::ZERO,
        stage: "buyout".to_string(),
        lockup_years: 7.0,
        capital_call_notice_days: 10,
    }
}

#[test]
fn a_flow_dated_before_the_subject_existed_is_refused_by_name() -> Result<()> {
    // Premise: the same flow one day *after* the origin is accepted, so the
    // refusal below is about the date and not about the flow.
    let after = ForecastCashflow::new(CashflowKind::CapitalCall, day(1), dec!("100"), 1.0)?;
    CashflowForecast::new("fund-1", origin(), origin())?.with_flow(after)?;

    let before = ForecastCashflow::new(CashflowKind::CapitalCall, day(-1), dec!("100"), 1.0)?;
    let refusal = CashflowForecast::new("fund-1", origin(), origin())?
        .with_flow(before)
        .expect_err("a call dated before the fund existed must be refused");
    let message = refusal.message();
    assert!(
        message.contains("before the subject's origin"),
        "the refusal must name the origin violation, said: {message}"
    );
    assert!(
        message.contains("cannot precede the thing that produces it"),
        "the refusal must say what to do instead, said: {message}"
    );
    Ok(())
}

#[test]
fn a_forecast_cannot_be_read_at_an_instant_before_it_became_knowable() -> Result<()> {
    // The point-in-time defect in its exact shape: a schedule published on
    // day 30, applied to a day-10 valuation, would let the platform reserve
    // against a demand nobody had told it about.
    let forecast = CashflowForecast::new("fund-1", origin(), day(30))?.with_flow(
        ForecastCashflow::new(CashflowKind::CapitalCall, day(60), dec!("100"), 1.0)?,
    )?;

    // Premise: read at or after the knowable instant, the same call is
    // visible, so the refusal below is about the instant and nothing else.
    assert_eq!(
        forecast.expected_demand_within(day(30), Duration::from_days(90))?,
        dec!("100"),
        "the call must be visible once the forecast is knowable"
    );

    let refusal = forecast
        .expected_demand_within(day(10), Duration::from_days(90))
        .expect_err("a forecast must refuse an instant before it was knowable");
    assert!(
        refusal.message().contains("became knowable at"),
        "the refusal must name the knowable instant, said: {}",
        refusal.message()
    );
    assert!(
        forecast.j_curve_trough(day(10)).is_err(),
        "every reader of the schedule must apply the same guard"
    );
    assert!(
        forecast.present_value(day(10), 0.1).is_err(),
        "the present value must apply the same guard"
    );
    Ok(())
}

#[test]
fn a_present_value_refuses_a_settled_flow_rather_than_dropping_it() -> Result<()> {
    let forecast = CashflowForecast::new("fund-1", origin(), origin())?
        .with_flow(ForecastCashflow::new(
            CashflowKind::Distribution,
            day(10),
            dec!("100"),
            1.0,
        )?)
        .and_then(|f| {
            f.with_flow(ForecastCashflow::new(
                CashflowKind::Coupon,
                day(400),
                dec!("50"),
                1.0,
            )?)
        })?;
    // Premise: both flows are ahead at the origin, so the forecast discounts
    // without complaint there.
    assert!(
        forecast.present_value(origin(), 0.05)?.is_positive(),
        "both flows are ahead at the origin, so this must discount"
    );

    let refusal = forecast
        .present_value(day(20), 0.05)
        .expect_err("a flow due before the valuation instant must be refused");
    assert!(
        refusal.message().contains("before the valuation instant"),
        "the refusal must name the settled flow, said: {}",
        refusal.message()
    );
    assert!(
        refusal.message().contains("remaining_at"),
        "the refusal must name the way to exclude settled flows, said: {}",
        refusal.message()
    );

    // And saying so out loud works: the remaining stream discounts.
    let remaining = forecast.remaining_at(day(20))?;
    assert_eq!(remaining.len(), 1, "only the day-400 coupon is still ahead");
    assert!(remaining.present_value(day(20), 0.05)?.is_positive());
    Ok(())
}

#[test]
fn a_discount_rate_that_would_invert_the_sign_of_a_flow_is_refused() -> Result<()> {
    let forecast = CashflowForecast::new("fund-1", origin(), origin())?.with_flow(
        ForecastCashflow::new(CashflowKind::Distribution, day(365), dec!("100"), 1.0)?,
    )?;
    // Premise: a rate just above -100% is accepted, so the boundary below is
    // the refusal and not a blanket rejection of negative rates.
    assert!(
        forecast.present_value(origin(), -0.5)?.is_positive(),
        "a negative rate above -100% still discounts to a positive value"
    );

    for rate in [-1.0, -1.5, f64::NAN, f64::NEG_INFINITY] {
        let refusal = forecast
            .present_value(origin(), rate)
            .expect_err("a rate at or below -100% must be refused");
        assert!(
            refusal.message().contains("supply a rate above -1.0"),
            "the refusal for {rate} must say what to supply, said: {}",
            refusal.message()
        );
    }
    Ok(())
}

#[test]
fn the_j_curve_trough_is_the_deepest_cumulative_point_not_the_last_one() -> Result<()> {
    // Fees and calls first, value later — the shape §16.4 asks to be modelled
    // explicitly so an early mark is not misread as a loss.
    let forecast = CashflowForecast::new("fund-1", origin(), origin())?
        .with_flow(ForecastCashflow::new(
            CashflowKind::CapitalCall,
            day(30),
            dec!("400"),
            1.0,
        )?)
        .and_then(|f| {
            f.with_flow(ForecastCashflow::new(
                CashflowKind::CapitalCall,
                day(365),
                dec!("400"),
                1.0,
            )?)
        })
        .and_then(|f| {
            f.with_flow(ForecastCashflow::new(
                CashflowKind::Distribution,
                day(1000),
                dec!("1500"),
                1.0,
            )?)
        })?;
    // Premise: the stream ends well above water, so a trough read off the
    // final cumulative figure would report nothing at all.
    assert_eq!(
        forecast.expected_between(origin(), origin(), day(2000))?,
        dec!("700"),
        "the stream must end positive for this test to mean anything"
    );

    let (at, depth) = forecast
        .j_curve_trough(origin())?
        .expect("a stream that goes 800 underwater has a trough");
    assert_eq!(at, day(365), "the trough is the second call, not the first");
    assert_eq!(depth, dec!("-800"));
    Ok(())
}

#[test]
fn a_probability_weighted_call_reserves_less_than_a_certain_one() -> Result<()> {
    let certain = CashflowForecast::new("fund-1", origin(), origin())?.with_flow(
        ForecastCashflow::new(CashflowKind::CapitalCall, day(30), dec!("1000"), 1.0)?,
    )?;
    let likely = CashflowForecast::new("fund-1", origin(), origin())?.with_flow(
        ForecastCashflow::new(CashflowKind::CapitalCall, day(30), dec!("1000"), 0.25)?,
    )?;
    let horizon = Duration::from_days(90);
    // Premise: both schedules name the same call on the same day, so the only
    // difference between the two figures is the probability.
    assert_eq!(
        certain.expected_demand_within(origin(), horizon)?,
        dec!("1000")
    );
    assert_eq!(
        likely.expected_demand_within(origin(), horizon)?,
        dec!("250")
    );
    Ok(())
}

#[test]
fn a_distribution_does_not_offset_a_call_due_sooner() -> Result<()> {
    // Capital returning in three years cannot pay a call due next month, and
    // a reserve computed as a net figure would say otherwise.
    let forecast = CashflowForecast::new("fund-1", origin(), origin())?
        .with_flow(ForecastCashflow::new(
            CashflowKind::CapitalCall,
            day(30),
            dec!("1000"),
            1.0,
        )?)
        .and_then(|f| {
            f.with_flow(ForecastCashflow::new(
                CashflowKind::Distribution,
                day(60),
                dec!("5000"),
                1.0,
            )?)
        })?;
    let horizon = Duration::from_days(90);
    // Premise: the net figure over the same window is strongly positive, so
    // a netting implementation would report no demand at all.
    assert_eq!(
        forecast.expected_between(origin(), origin(), day(90))?,
        dec!("4000"),
        "the window nets to a large inflow, which is what must not be used"
    );
    assert_eq!(
        forecast.expected_demand_within(origin(), horizon)?,
        dec!("1000"),
        "the reserve is the call, undiminished by a later distribution"
    );
    Ok(())
}

#[test]
fn a_second_flow_on_the_same_day_of_the_same_kind_is_refused_rather_than_summed() -> Result<()> {
    let first = ForecastCashflow::new(CashflowKind::CapitalCall, day(30), dec!("100"), 1.0)?;
    let second = ForecastCashflow::new(CashflowKind::CapitalCall, day(30), dec!("250"), 1.0)?;
    // Premise: a different kind on the same day is accepted, so the collision
    // is on (date, kind) and not on the date alone.
    let base = CashflowForecast::new("fund-1", origin(), origin())?.with_flow(first)?;
    let mixed = base.clone().with_flow(ForecastCashflow::new(
        CashflowKind::Fee,
        day(30),
        dec!("10"),
        1.0,
    )?)?;
    assert_eq!(mixed.len(), 2);

    let refusal = base
        .with_flow(second)
        .expect_err("a colliding flow must be refused");
    assert!(
        refusal.message().contains("combine the two into one entry"),
        "the refusal must say what to do instead, said: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_commitment_refuses_more_called_than_was_ever_promised() -> Result<()> {
    // Premise: called exactly equal to committed is a legitimate, fully drawn
    // fund and is accepted, so the refusal below is about the excess.
    let drawn = Commitment::unscheduled("fund-1", dec!("1000"), dec!("1000"), origin(), origin())?;
    assert_eq!(drawn.unfunded(), Decimal::ZERO);

    let refusal = Commitment::unscheduled("fund-1", dec!("1000"), dec!("1001"), origin(), origin())
        .expect_err("called capital above the commitment must be refused");
    assert!(
        refusal
            .message()
            .contains("cannot call more than was promised"),
        "the refusal must name the contract, said: {}",
        refusal.message()
    );
    assert!(
        refusal.message().contains("will not be capped here"),
        "the refusal must say it is not clamping, said: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_schedule_calling_more_than_remains_unfunded_is_refused() -> Result<()> {
    let commitment =
        Commitment::unscheduled("fund-1", dec!("1000"), dec!("600"), origin(), origin())?;
    // Premise: 400 remains unfunded, and a schedule of exactly 400 attaches.
    assert_eq!(commitment.unfunded(), dec!("400"));
    let fits = CashflowForecast::new("fund-1", origin(), origin())?.with_flow(
        ForecastCashflow::new(CashflowKind::CapitalCall, day(30), dec!("400"), 1.0)?,
    )?;
    commitment.clone().with_forecast(fits)?;

    let overruns = CashflowForecast::new("fund-1", origin(), origin())?
        .with_flow(ForecastCashflow::new(
            CashflowKind::CapitalCall,
            day(30),
            dec!("400"),
            1.0,
        )?)
        .and_then(|f| {
            f.with_flow(ForecastCashflow::new(
                CashflowKind::Fee,
                day(31),
                dec!("1"),
                1.0,
            )?)
        })?;
    let refusal = commitment
        .with_forecast(overruns)
        .expect_err("a schedule exceeding the unfunded balance must be refused");
    assert!(
        refusal.message().contains("against an unfunded balance of"),
        "the refusal must name both figures, said: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_commitment_with_no_pacing_model_reserves_the_whole_unfunded_balance() -> Result<()> {
    // The conservative fallback: a commitment nobody has modelled the timing
    // of could be called in full tomorrow.
    let bare = Commitment::unscheduled("fund-1", dec!("1000"), dec!("250"), origin(), origin())?;
    assert_eq!(
        bare.demand_within(origin(), Duration::from_days(1))?,
        dec!("750"),
        "with no schedule, the whole obligation is the near-term demand"
    );

    // Premise: the same commitment with a schedule reports the schedule's
    // near-term figure instead, so the fallback above is a fallback and not
    // the only behaviour.
    let scheduled =
        bare.with_forecast(
            CashflowForecast::new("fund-1", origin(), origin())?.with_flow(
                ForecastCashflow::new(CashflowKind::CapitalCall, day(400), dec!("750"), 1.0)?,
            )?,
        )?;
    assert_eq!(
        scheduled.demand_within(origin(), Duration::from_days(1))?,
        Decimal::ZERO,
        "a call four hundred days out is not demand within a day"
    );
    assert_eq!(
        scheduled.unfunded(),
        dec!("750"),
        "the hard obligation is unchanged by the schedule"
    );
    Ok(())
}

#[test]
fn a_book_refuses_to_total_an_obligation_it_could_not_yet_have_known_about() -> Result<()> {
    let mut book = CommitmentBook::new();
    book.record(Commitment::unscheduled(
        "fund-1",
        dec!("1000"),
        dec!("0"),
        origin(),
        day(30),
    )?)?;
    // Premise: on day 30 the book totals normally.
    assert_eq!(book.unfunded_total(day(30))?, dec!("1000"));

    let refusal = book
        .unfunded_total(day(29))
        .expect_err("a book must refuse to total an obligation not yet knowable");
    assert!(
        refusal
            .message()
            .contains("cannot reserve against an obligation it had not been told about"),
        "the refusal must name the reason, said: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_second_commitment_for_the_same_subject_is_refused_rather_than_replacing_the_first()
-> Result<()> {
    let mut book = CommitmentBook::new();
    book.record(Commitment::unscheduled(
        "fund-1",
        dec!("1000"),
        dec!("0"),
        origin(),
        origin(),
    )?)?;
    // Premise: a different subject records fine, so the refusal is about the
    // collision and not about the book being closed.
    book.record(Commitment::unscheduled(
        "fund-2",
        dec!("500"),
        dec!("0"),
        origin(),
        origin(),
    )?)?;
    assert_eq!(book.len(), 2);

    let refusal = book
        .record(Commitment::unscheduled(
            "fund-1",
            dec!("9999"),
            dec!("0"),
            origin(),
            origin(),
        )?)
        .expect_err("a duplicate subject must be refused");
    assert!(
        refusal
            .message()
            .contains("already has a commitment recorded"),
        "the refusal must name the collision, said: {}",
        refusal.message()
    );
    assert_eq!(
        book.unfunded_total(origin())?,
        dec!("1500"),
        "the refused record must not have moved the total"
    );
    Ok(())
}

#[test]
fn a_fully_drawn_private_asset_records_no_commitment_at_all() -> Result<()> {
    // A row that can never reserve anything is the control that reads as
    // protection and is not. Premise: the partially drawn record does produce
    // one, so the `None` below is about the balance.
    let partial = Commitment::from_private_asset(
        "fund-1",
        &private_asset(dec!("1000"), dec!("400")),
        origin(),
        origin(),
    )?
    .expect("an undrawn balance obliges the book");
    assert_eq!(partial.unfunded(), dec!("600"));

    let drawn = Commitment::from_private_asset(
        "fund-2",
        &private_asset(dec!("1000"), dec!("1000")),
        origin(),
        origin(),
    )?;
    assert!(
        drawn.is_none(),
        "a fully drawn fund obliges the book to nothing further"
    );
    Ok(())
}
