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
    CallConsequence, CapitalCall, CashflowForecast, CashflowKind, Commitment, CommitmentBook,
    ForecastCashflow,
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
#[test]
fn a_scheduled_capital_call_is_never_reported_as_the_instant_capital_returns() -> Result<()> {
    // The J-curve read upside down. A capital call is the position *taking*
    // cash, and a liquidity read that accepted one as an answer to "when does
    // this become cash" would report a fund at its deepest draw as its most
    // liquid — the holding would then count toward the fraction the
    // `MinLiquidity` floor vetoes trading on.
    //
    // Premise: a distribution on the very same date *is* reported, so what the
    // call below fails to satisfy is its direction and not its date.
    let returning = CashflowForecast::new("fund-1", origin(), origin())?.with_flow(
        ForecastCashflow::new(CashflowKind::Distribution, day(400), dec!("500"), 1.0)?,
    )?;
    assert_eq!(
        returning.first_return_at(origin())?,
        Some(day(400)),
        "a distribution dated ahead is the instant capital returns"
    );

    let drawing = CashflowForecast::new("fund-1", origin(), origin())?
        .with_flow(ForecastCashflow::new(
            CashflowKind::CapitalCall,
            day(400),
            dec!("500"),
            1.0,
        )?)?
        .with_flow(ForecastCashflow::new(
            CashflowKind::Fee,
            day(500),
            dec!("10"),
            1.0,
        )?)?;
    assert_eq!(
        drawing.len(),
        2,
        "the premise is a schedule that holds flows, so the None below is about their direction"
    );
    assert_eq!(
        drawing.first_return_at(origin())?,
        None,
        "a schedule of calls and fees returns no capital at all"
    );
    Ok(())
}

#[test]
fn the_instant_capital_returns_is_the_earliest_distribution_still_ahead() -> Result<()> {
    // Two failures in one. Taking the earliest flow of any kind would answer
    // with the call on day 100; taking the earliest flow in the schedule
    // regardless of the read instant would answer with the distribution on day
    // 200, which had already settled — and a liquidity read that counts money
    // already received as money still to come reports the same cash twice.
    let forecast = CashflowForecast::new("fund-1", origin(), origin())?
        .with_flow(ForecastCashflow::new(
            CashflowKind::CapitalCall,
            day(100),
            dec!("300"),
            1.0,
        )?)?
        .with_flow(ForecastCashflow::new(
            CashflowKind::Distribution,
            day(200),
            dec!("50"),
            1.0,
        )?)?
        .with_flow(ForecastCashflow::new(
            CashflowKind::Distribution,
            day(900),
            dec!("400"),
            1.0,
        )?)?;
    // Premise: read from the origin the schedule does answer with the early
    // distribution, so the later answer below is the read instant moving and
    // not an empty schedule.
    assert_eq!(forecast.first_return_at(origin())?, Some(day(200)));

    assert_eq!(
        forecast.first_return_at(day(300))?,
        Some(day(900)),
        "a distribution already settled is behind the reader, not ahead of it"
    );
    Ok(())
}

#[test]
fn the_instant_capital_returns_cannot_be_read_before_the_schedule_was_knowable() -> Result<()> {
    // Point-in-time leakage in the liquidity read rather than in the mark. A
    // lockup restated in March, applied to a January book, makes the platform
    // look as though it had known how illiquid a position was before it was
    // told — and every liquidity figure downstream is then better or worse
    // than reality by exactly the information it should not have had.
    let forecast = CashflowForecast::new("fund-1", origin(), day(60))?.with_flow(
        ForecastCashflow::new(CashflowKind::Distribution, day(900), dec!("400"), 1.0)?,
    )?;
    // Premise: read at the instant it became knowable, the same schedule
    // answers, so the refusal below is the instant and not the schedule.
    assert_eq!(forecast.first_return_at(day(60))?, Some(day(900)));

    let refusal = forecast
        .first_return_at(day(59))
        .expect_err("a schedule cannot be read before it was knowable");
    let message = refusal.message();
    assert!(
        message.contains("cannot be read as of"),
        "the refusal must name the read instant, said: {message}"
    );
    Ok(())
}
// ---------------------------------------------------------------------------
// Capital calls — blueprint §43.2's drawdown notice.
//
// A `CashflowKind::CapitalCall` variant already existed and is a different
// object: a probability-weighted projection of a call somebody might make. The
// notice below is one that has arrived, and the failure every test here
// prevents is the platform treating the second as though it were the first —
// discounting a demand that is in writing.
// ---------------------------------------------------------------------------

/// A commitment of 1000 with nothing yet called, knowable from its origin.
fn thousand_committed() -> Result<Commitment> {
    Commitment::unscheduled("fund-1", dec!("1000"), Decimal::ZERO, origin(), origin())
}

/// A pacing model expecting one call of 100 on day 20 at even odds, so its
/// probability-weighted demand over a horizon covering day 20 is 50.
fn even_odds_pacing() -> Result<CashflowForecast> {
    CashflowForecast::new("fund-1", origin(), origin())?.with_flow(ForecastCashflow::new(
        CashflowKind::CapitalCall,
        day(20),
        dec!("100"),
        0.5,
    )?)
}

fn notice(
    reference: &str,
    amount: Decimal,
    issued: Timestamp,
    due: Timestamp,
    consequence: CallConsequence,
) -> Result<CapitalCall> {
    CapitalCall::notice("fund-1", reference, amount, issued, due, consequence)
}

#[test]
fn an_issued_notice_the_pacing_model_understates_raises_the_capital_demanded() -> Result<()> {
    // The defect, exactly: a fund sends a notice for 400 and the pacing model
    // expects 50, so a reserve taken from the model alone is short by 350 of a
    // demand already in writing.
    let horizon = Duration::from_days(30);
    let mut commitment = thousand_committed()?.with_forecast(even_odds_pacing()?)?;

    // Premise first: the model on its own answers 50, so the change below is
    // the notice and not the horizon.
    assert_eq!(
        commitment.demand_within(origin(), horizon)?,
        dec!("50"),
        "the premise failed: the pacing model was expected to weight one call of 100 at even odds"
    );

    commitment.record_call(notice(
        "nt-1",
        dec!("400"),
        origin(),
        day(20),
        CallConsequence::Interest {
            annual_rate_bps: 500,
        },
    )?)?;

    // 400, not 50 — and not 450 either. Adding the projection to the notice
    // would reserve twice for the draw the projection was predicting.
    assert_eq!(
        commitment.demand_within(origin(), horizon)?,
        dec!("400"),
        "a notice in writing for 400 must set the demand, not be averaged away by a model \
         expecting 50, and not be summed with it"
    );
    Ok(())
}

#[test]
fn a_notice_demanding_more_than_remains_unfunded_is_refused_rather_than_capped() -> Result<()> {
    // A fund cannot call more than remains promised. Capping the excess would
    // turn a corrupt record into a plausible one, which is the argument
    // `Commitment::unscheduled` already makes about the called balance.
    let full = notice(
        "nt-1",
        dec!("600"),
        origin(),
        day(20),
        CallConsequence::Acceleration,
    )?;
    // Premise: 600 against an unfunded 600 is accepted, so the refusal below
    // is about the excess and not about notices in general.
    Commitment::unscheduled("fund-1", dec!("1000"), dec!("400"), origin(), origin())?
        .record_call(full)?;

    let refusal = Commitment::unscheduled("fund-1", dec!("1000"), dec!("400"), origin(), origin())?
        .record_call(notice(
            "nt-1",
            dec!("601"),
            origin(),
            day(20),
            CallConsequence::Acceleration,
        )?)
        .expect_err("a notice for more than remains unfunded must be refused");
    let message = refusal.message();
    assert!(
        message.contains("unfunded balance of 600;"),
        "the refusal must name the balance it exceeded, said: {message}"
    );
    assert!(
        message.contains("will not be capped here"),
        "the refusal must say the excess is not silently corrected, said: {message}"
    );
    Ok(())
}

#[test]
fn a_notice_is_not_counted_as_demand_before_the_platform_was_sent_it() -> Result<()> {
    // Point-in-time leakage in its exact shape. A notice issued on day 30,
    // read at a day-10 valuation, would report a squeeze the desk did not
    // have — and every figure downstream of it is then worse than reality in
    // a way no backtest reveals.
    let horizon = Duration::from_days(60);
    let mut commitment = thousand_committed()?.with_forecast(even_odds_pacing()?)?;
    commitment.record_call(notice(
        "nt-1",
        dec!("400"),
        day(30),
        day(40),
        CallConsequence::Interest {
            annual_rate_bps: 500,
        },
    )?)?;

    // Premise: once knowable the notice does dominate, so the day-10 reading
    // below is the guard firing rather than the notice being ignored outright.
    assert_eq!(
        commitment.demand_within(day(30), horizon)?,
        dec!("400"),
        "the premise failed: a notice knowable at day 30 and due at day 40 must be demanded"
    );

    assert_eq!(
        commitment.demand_within(day(10), horizon)?,
        dec!("50"),
        "a notice issued on day 30 must not raise the demand read at day 10; the desk had not \
         been told about it"
    );
    Ok(())
}

#[test]
fn default_interest_accrues_only_once_a_notice_is_both_knowable_and_past_due() -> Result<()> {
    let call = notice(
        "nt-1",
        dec!("500"),
        origin(),
        day(10),
        CallConsequence::Interest {
            annual_rate_bps: 1000,
        },
    )?;

    // Premise: the penalty is non-zero somewhere, so the zeroes below are the
    // timing rule and not a consequence that never charges anything.
    assert_eq!(
        call.penalty_at(day(375))?,
        dec!("50"),
        "the premise failed: 500 at 1000bp for 365 days past due is 50"
    );

    assert_eq!(
        call.penalty_at(day(10))?,
        Decimal::ZERO,
        "a call read on the day it falls due is not yet late and must cost nothing"
    );
    assert_eq!(
        call.days_late_at(day(10)),
        0,
        "a call read on its due date is zero days late"
    );
    assert_eq!(
        call.days_late_at(day(375)),
        365,
        "lateness is counted in whole days from the due date"
    );
    Ok(())
}

#[test]
fn an_overdue_notice_raises_the_obligation_the_capital_engine_reserves_against() -> Result<()> {
    // The seam that reaches production: `CommitmentBook::unfunded_total` is
    // what the kernel subtracts from free capital before anything is sized.
    // Default interest is capital owed to the same counterparty on the same
    // paper, so a desk deploying against the unfunded balance alone would be
    // deploying money it had already lost.
    let mut book = CommitmentBook::new();
    book.record(thousand_committed()?)?;
    book.record_call(notice(
        "nt-1",
        dec!("500"),
        origin(),
        day(10),
        CallConsequence::Interest {
            annual_rate_bps: 1000,
        },
    )?)?;

    // Premise: while nothing is overdue the total is exactly what it always
    // was, so the figure below is the penalty and not a reserve that grew for
    // some other reason.
    assert_eq!(
        book.unfunded_total(day(10))?,
        dec!("1000"),
        "the premise failed: with nothing overdue the obligation is the unfunded balance"
    );
    assert_eq!(
        book.accrued_default_penalty(day(10))?,
        Decimal::ZERO,
        "the premise failed: nothing is overdue on the due date"
    );

    assert_eq!(
        book.accrued_default_penalty(day(375))?,
        dec!("50"),
        "a year of default interest at 1000bp on a missed call of 500 is 50"
    );
    assert_eq!(
        book.unfunded_total(day(375))?,
        dec!("1050"),
        "the obligation the capital engine reserves against must carry the penalty a missed call \
         has already cost"
    );
    Ok(())
}

#[test]
fn an_overdue_acceleration_notice_demands_the_whole_unfunded_balance_at_once() -> Result<()> {
    // The arm that costs no principal and breaks the liquidity plan anyway: a
    // penalty figure alone could not express it, which is why the consequence
    // is an enum rather than a number.
    let mut commitment = thousand_committed()?.with_forecast(even_odds_pacing()?)?;
    commitment.record_call(notice(
        "nt-1",
        dec!("100"),
        origin(),
        day(10),
        CallConsequence::Acceleration,
    )?)?;
    let horizon = Duration::from_days(5);

    // Premise: before the default only the notice's own 100 is demanded, so
    // the figure below is acceleration and not the fallback for a commitment
    // with no pacing model.
    assert_eq!(
        commitment.demand_within(day(10), horizon)?,
        dec!("100"),
        "the premise failed: a notice not yet in default demands only its own amount"
    );

    assert_eq!(
        commitment.demand_within(day(11), horizon)?,
        dec!("1000"),
        "a missed call whose consequence is acceleration makes the whole unfunded balance due now"
    );
    assert_eq!(
        commitment.accrued_default_penalty(day(11))?,
        Decimal::ZERO,
        "acceleration costs no extra principal; its effect is on when the capital is demanded"
    );
    Ok(())
}

#[test]
fn a_notice_falling_due_before_it_was_issued_is_refused() -> Result<()> {
    // Not a tight deadline — a record whose two dates came from different
    // places. Accepting it would accrue default interest from an instant
    // nobody was told about.
    // Premise: due exactly on the issue date is accepted.
    notice(
        "nt-1",
        dec!("100"),
        day(10),
        day(10),
        CallConsequence::Acceleration,
    )?;

    let refusal = notice(
        "nt-1",
        dec!("100"),
        day(10),
        day(9),
        CallConsequence::Acceleration,
    )
    .expect_err("a call due before the notice that demanded it must be refused");
    let message = refusal.message();
    assert!(
        message.contains("already late when it arrived"),
        "the refusal must name what is wrong with the record, said: {message}"
    );
    assert!(
        message.contains("correct the dates"),
        "the refusal must say what to do instead, said: {message}"
    );
    Ok(())
}

#[test]
fn settling_a_notice_moves_the_called_balance_and_retires_the_notice() -> Result<()> {
    // The one operation that reduces the unfunded balance with a record of
    // why. Before notices existed the called balance was a bare figure with
    // no provenance and nothing could say which draws made it up.
    let mut commitment = thousand_committed()?;
    commitment.record_call(notice(
        "nt-1",
        dec!("400"),
        origin(),
        day(10),
        CallConsequence::Acceleration,
    )?)?;

    // Premise: the notice is held and nothing has been called yet.
    assert_eq!(
        commitment.called(),
        Decimal::ZERO,
        "the premise failed: recording a notice must not itself move the called balance"
    );
    assert_eq!(
        commitment.calls().count(),
        1,
        "the premise failed: the notice was not recorded"
    );

    assert_eq!(
        commitment.settle_call("nt-1", day(10))?,
        dec!("600"),
        "settling a call of 400 against a commitment of 1000 leaves 600 unfunded"
    );
    assert_eq!(
        commitment.called(),
        dec!("400"),
        "settling must move the called balance by the amount demanded"
    );
    assert!(
        commitment.call("nt-1").is_none(),
        "a settled notice is no longer outstanding and must not be demanded twice"
    );
    Ok(())
}

#[test]
fn a_second_notice_under_one_reference_is_refused_rather_than_overwriting_the_first() -> Result<()>
{
    // Overwriting would move the reserve with no record of which claim won —
    // the same argument `CommitmentBook::record` makes about commitments.
    let mut commitment = thousand_committed()?;
    commitment.record_call(notice(
        "nt-1",
        dec!("100"),
        origin(),
        day(10),
        CallConsequence::Acceleration,
    )?)?;
    // Premise: a second notice under a *different* reference is accepted, so
    // the refusal below is about the reference and not about second notices.
    commitment.record_call(notice(
        "nt-2",
        dec!("200"),
        origin(),
        day(20),
        CallConsequence::Acceleration,
    )?)?;

    let refusal = commitment
        .record_call(notice(
            "nt-1",
            dec!("200"),
            origin(),
            day(20),
            CallConsequence::Acceleration,
        )?)
        .expect_err("a duplicate notice reference must be refused");
    let message = refusal.message();
    assert!(
        message.contains("already holds notice nt-1;"),
        "the refusal must name the reference already held, said: {message}"
    );
    assert!(
        message.contains("which claim won"),
        "the refusal must name the failure it prevents, said: {message}"
    );
    assert_eq!(
        commitment
            .call("nt-1")
            .map(qip_financial::cashflow::CapitalCall::amount),
        Some(dec!("100")),
        "the refused notice must not have replaced the one already held"
    );
    Ok(())
}

#[test]
fn a_notice_for_a_subject_the_book_holds_no_commitment_for_is_refused() -> Result<()> {
    // Letting a call open a commitment would put an obligation in the book
    // with no promise behind it, and nothing could reconcile it.
    let mut book = CommitmentBook::new();
    book.record(thousand_committed()?)?;
    // Premise: a notice against the commitment the book does hold is accepted.
    book.record_call(notice(
        "nt-1",
        dec!("100"),
        origin(),
        day(10),
        CallConsequence::Acceleration,
    )?)?;

    let refusal = book
        .record_call(CapitalCall::notice(
            "fund-2",
            "nt-9",
            dec!("100"),
            origin(),
            day(10),
            CallConsequence::Acceleration,
        )?)
        .expect_err("a notice against a commitment the book does not hold must be refused");
    let message = refusal.message();
    assert!(
        message.contains("no commitment is recorded for fund-2,"),
        "the refusal must name the subject it could not find, said: {message}"
    );
    assert!(
        message.contains("record the commitment first"),
        "the refusal must say what to do instead, said: {message}"
    );
    assert_eq!(
        book.len(),
        1,
        "a refused notice must not have opened a commitment"
    );
    Ok(())
}

#[test]
fn a_default_penalty_is_exact_in_decimal_and_each_consequence_treats_lateness_its_own_way()
-> Result<()> {
    // Money never crosses into `f64` here: a rate in basis points and a count
    // of days are both integers, so the figures below are exact rather than
    // nearly right. A default-interest number that disagrees with the fund's
    // own notice in the last cent is a reconciliation nobody can close.
    let interest = CallConsequence::Interest {
        annual_rate_bps: 1000,
    };
    let forfeiture = CallConsequence::Forfeiture { fraction_bps: 250 };

    assert_eq!(
        interest.penalty_on(dec!("1000"), 365)?,
        dec!("100"),
        "1000bp on 1000 for a full year is exactly 100"
    );
    assert_eq!(
        interest.penalty_on(dec!("1000"), 73)?,
        dec!("20"),
        "interest grows with lateness: a fifth of a year is a fifth of the annual charge"
    );
    assert_eq!(
        forfeiture.penalty_on(dec!("1000"), 1)?,
        dec!("25"),
        "250bp forfeited on 1000 is exactly 25"
    );
    assert_eq!(
        forfeiture.penalty_on(dec!("1000"), 365)?,
        dec!("25"),
        "a forfeiture is taken once and must not grow with lateness the way interest does"
    );
    assert_eq!(
        CallConsequence::Acceleration.penalty_on(dec!("1000"), 365)?,
        Decimal::ZERO,
        "acceleration costs no principal; the balance was already owed"
    );

    let refusal = interest
        .penalty_on(dec!("1000"), -1)
        .expect_err("a penalty asked for negative lateness must be refused");
    assert!(
        refusal.message().contains("has not yet fallen due"),
        "the refusal must say what to do instead, said: {}",
        refusal.message()
    );
    Ok(())
}
