//! Tests for the predictive capital fabric.
//!
//! The properties here are the ones that go wrong quietly and expensively: a
//! budget exceeded by a rounding step, a transfer justified on an optimistic
//! reading of both sides of its own inequality, a wide forecast acted on as
//! though it were a narrow one, a Friday instruction believed to be a Friday
//! balance, and a symmetric penalty that under-positions every lane by the same
//! invisible amount. Each is asserted over a swept range rather than at a single
//! point, because every one of them passes a spot check.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital::allocation::{
    AllocationLimits, AllocationPlan, CapitalAllocator, DrawdownSchedule, StrategyProposal,
};
use qip_capital::capacity::CapacityModel;
use qip_capital_fabric::evaluate::{RealisedDemand, evaluate};
use qip_capital_fabric::forecast::{
    DemandForecast, DemandForecaster, DemandKind, DemandObservation, Interval,
};
use qip_capital_fabric::location::{CapitalLocation, Region};
use qip_capital_fabric::plan::{
    LocationBalance, PrePositioningPlan, PrePositioningPlanner, PrePositioningRequest,
    RefusalReason,
};
use qip_capital_fabric::settlement::{SettlementCalendar, SettlementConvention};
use qip_capital_fabric::transfer::{FundingCurve, FxRates, ShortfallAsymmetry, TransferCostModel};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Currency, Decimal, Duration, Timestamp, dec};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};

// --- fixtures ---------------------------------------------------------------

/// Thursday 7 March 2024, 09:00 UTC — a settlement day, inside every cut-off.
fn thursday() -> Timestamp {
    Timestamp::from_civil(2024, 3, 7).saturating_add(Duration::from_hours(9))
}

/// Friday 8 March 2024, 18:00 UTC — a settlement day, past every cut-off.
fn friday_evening() -> Timestamp {
    Timestamp::from_civil(2024, 3, 8).saturating_add(Duration::from_hours(18))
}

/// Friday 8 March 2024, 15:00 UTC — a settlement day, inside every cut-off.
fn friday_afternoon() -> Timestamp {
    Timestamp::from_civil(2024, 3, 8).saturating_add(Duration::from_hours(15))
}

/// Saturday 9 March 2024, 12:00 UTC — nothing settles.
fn saturday_noon() -> Timestamp {
    Timestamp::from_civil(2024, 3, 9).saturating_add(Duration::from_hours(12))
}

/// Monday 11 March 2024, 09:00 UTC.
fn monday_morning() -> Timestamp {
    Timestamp::from_civil(2024, 3, 11).saturating_add(Duration::from_hours(9))
}

fn treasury() -> CapitalLocation {
    CapitalLocation::new(Region::new("namr"), Currency::USD, VenueId::new("TREASURY"))
}

fn at_venue(venue: &str) -> CapitalLocation {
    CapitalLocation::new(Region::new("emea"), Currency::USD, VenueId::new(venue))
}

fn cost_model(wire_fee: Decimal) -> Result<TransferCostModel> {
    TransferCostModel::new(
        TransactionCostModel::listed(1.0),
        LiquidityProfile::listed(Decimal::from_int(5_000_000_000), 1.0),
        FundingCurve::flat(400.0)?,
        wire_fee,
        300.0,
    )
}

fn planner_with(
    total: Decimal,
    per_venue: Decimal,
    wire_fee: Decimal,
    convention: SettlementConvention,
) -> Result<PrePositioningPlanner> {
    Ok(PrePositioningPlanner::new(
        CapitalAllocator::new(
            AllocationLimits::new(total, total, total, per_venue)?,
            DrawdownSchedule::default(),
        ),
        cost_model(wire_fee)?,
        SettlementCalendar::weekday(convention)?,
    ))
}

fn planner() -> Result<PrePositioningPlanner> {
    planner_with(
        dec!("100000000"),
        dec!("100000000"),
        dec!("25"),
        SettlementConvention::T1,
    )
}

/// A live allocation with nothing yet committed.
fn idle_allocation(planner: &PrePositioningPlanner, at: Timestamp) -> Result<AllocationPlan> {
    planner.allocator().allocate(&[], 0.0, at)
}

/// A real `qip-capital` proposal, so a live plan actually consumes venue
/// headroom rather than being hand-written.
fn proposal(name: &str, venue: &str, sharpe: f64, daily_volume: i64) -> Result<StrategyProposal> {
    Ok(StrategyProposal {
        strategy: StrategyId::new(name),
        cell: "cell-lon-1".to_string(),
        venue: VenueId::new(venue),
        expected_sharpe: sharpe,
        sharpe_standard_error: 0.1,
        capacity: CapacityModel::new(
            LiquidityProfile::listed(Decimal::from_int(daily_volume), 5.0),
            TransactionCostModel::listed(5.0),
            60.0,
            dec!("100"),
            0.4,
        )?,
        capacity_uncertainty: 0.05,
    })
}

fn margin_forecast(
    location: &CapitalLocation,
    lower: Decimal,
    point: Decimal,
    upper: Decimal,
    at: Timestamp,
    horizon: Duration,
) -> Result<DemandForecast> {
    DemandForecast::new(
        location.clone(),
        DemandKind::Margin,
        at,
        horizon,
        Interval::new(lower, point, upper, 0.80)?,
        60,
    )
}

/// Pull a figure back out of a refusal message, so the assertion is about the
/// number the operator will read rather than about a field nobody sees.
fn figure_after(text: &str, marker: &str) -> Option<Decimal> {
    let rest = text.split_once(marker)?.1;
    let token: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    Decimal::parse(&token)
}

// --- the budget invariant ---------------------------------------------------

#[test]
fn a_plan_never_commits_more_than_the_budget_it_was_given() -> Result<()> {
    // Swept over randomly generated books rather than asserted once: the failure
    // this guards against is a fractional overshoot that only shows up when the
    // amounts do not divide evenly, which a hand-written fixture will not
    // produce. The counters below assert the sweep actually reached both the
    // budget and the venue limit, since a property test where a limit is never
    // hit proves nothing about that limit.
    let mut rng = Xoshiro256::seeded(0x0CA9_17A1);
    let planner = planner_with(
        dec!("40000000"),
        dec!("9000000"),
        dec!("25"),
        SettlementConvention::T1,
    )?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;

    let mut budget_refusals = 0usize;
    let mut venue_refusals = 0usize;
    let mut plans_with_moves = 0usize;

    for case in 0..200u64 {
        let mobile = Decimal::from_int(1_000_000 + (rng.next_u64() % 60_000_000) as i64);
        let mut request =
            PrePositioningRequest::new(treasury(), mobile, FxRates::new(Currency::USD))?;
        let lanes = 1 + (rng.next_u64() % 6) as usize;
        for lane in 0..lanes {
            let location = at_venue(&format!("VENUE-{}", lane % 3));
            let point = Decimal::from_int(1_000_000 + (rng.next_u64() % 20_000_000) as i64);
            let half = Decimal::from_int((rng.next_u64() % 8_000_000) as i64);
            let interval =
                Interval::new((point - half).max(Decimal::ZERO), point, point + half, 0.80)?;
            let kinds = [
                DemandKind::Cash,
                DemandKind::Collateral,
                DemandKind::Margin,
                DemandKind::Inventory,
            ];
            request = request.with_forecast(DemandForecast::new(
                location.clone(),
                kinds[(rng.next_u64() % 4) as usize],
                now,
                Duration::from_hours(24 + (rng.next_u64() % 120) as i64),
                interval,
                40,
            )?);
            if rng.bernoulli(0.4) {
                request = request.with_balance(LocationBalance::new(
                    location,
                    DemandKind::Margin,
                    Decimal::from_int((rng.next_u64() % 4_000_000) as i64),
                )?);
            }
        }

        let plan = planner.plan(&request, &live, now)?;

        assert!(
            plan.is_within_budget(),
            "case {case}: committed {} against a {} budget",
            plan.committed(),
            plan.budget
        );
        assert!(
            plan.committed() <= planner.allocator().limits().total_budget,
            "case {case}: committed above the allocator's total budget"
        );
        // Exact, not approximate. The sum of the moves plus what was left must
        // reconstruct the budget to the last unit of the ninth decimal place.
        assert_eq!(
            plan.committed() + plan.unspent(),
            plan.budget,
            "case {case}: the budget did not reconstruct exactly"
        );
        for lane in 0..3 {
            let venue = VenueId::new(format!("VENUE-{lane}"));
            assert!(
                plan.for_venue(&venue) <= planner.allocator().limits().venue_limit(&venue),
                "case {case}: venue {lane} exceeded its allocation limit"
            );
        }

        if !plan.moves.is_empty() {
            plans_with_moves += 1;
        }
        budget_refusals += plan
            .refusals_because(RefusalReason::BudgetExhausted)
            .count();
        venue_refusals += plan.refusals_because(RefusalReason::VenueLimit).count();
    }

    assert!(plans_with_moves > 0, "the sweep never produced a transfer");
    assert!(budget_refusals > 0, "the sweep never exhausted a budget");
    assert!(venue_refusals > 0, "the sweep never reached a venue limit");
    Ok(())
}

// --- the hurdle, and what a refusal has to say ------------------------------

#[test]
fn a_transfer_costing_more_than_its_lower_bound_benefit_is_refused_naming_both_figures()
-> Result<()> {
    // A wire fee large enough to swamp a modest lane. Nothing else about the
    // lane is unusual, which is the point: this is the ordinary case where the
    // arithmetic simply does not work.
    let planner = planner_with(
        dec!("100000000"),
        dec!("100000000"),
        dec!("400000"),
        SettlementConvention::T1,
    )?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;
    let venue = at_venue("XLON");

    let request =
        PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?
            .with_forecast(margin_forecast(
                &venue,
                dec!("900000"),
                dec!("1000000"),
                dec!("1100000"),
                now,
                Duration::from_days(4),
            )?);

    let plan = planner.plan(&request, &live, now)?;
    assert!(plan.moves.is_empty(), "an unprofitable transfer was made");

    let refusal = plan
        .refusals_because(RefusalReason::CostExceedsBenefit)
        .next()
        .ok_or_else(|| qip_core::error::Error::not_found("no cost refusal was recorded"))?;

    let benefit = figure_after(&refusal.detail, "a benefit of ")
        .ok_or_else(|| qip_core::error::Error::schema("the refusal did not name the benefit"))?;
    let transfer = figure_after(&refusal.detail, "a transfer cost of ")
        .ok_or_else(|| qip_core::error::Error::schema("the refusal did not name the cost"))?;
    let funding = figure_after(&refusal.detail, "a funding cost of ")
        .ok_or_else(|| qip_core::error::Error::schema("the refusal did not name the funding"))?;

    // Independently recomputed: the benefit is the shortfall penalty over the
    // lag reactive funding would have left, on the confident gap alone.
    let calendar = SettlementCalendar::weekday(SettlementConvention::T1)?;
    let needed_by = now.saturating_add(Duration::from_days(4));
    let reactive_lag = calendar.quote(needed_by)?.available_at.since(needed_by);
    let expected = ShortfallAsymmetry::for_kind(DemandKind::Margin)?
        .shortfall_penalty(dec!("900000"), reactive_lag);
    assert_eq!(
        benefit, expected,
        "the refusal named a benefit the planner did not compute"
    );
    assert!(
        benefit <= transfer + funding,
        "the refusal claims a benefit of {benefit} that already exceeds its {transfer} \
         transfer and {funding} funding cost"
    );
    assert!(
        refusal.detail.contains("lower bound"),
        "the refusal does not say which bound the benefit came from"
    );
    Ok(())
}

#[test]
fn the_benefit_is_taken_at_the_lower_bound_and_the_cost_at_the_upper() -> Result<()> {
    // The same lane, priced by two models differing only in how much they widen
    // the uncertain cost components. The one that widens more must charge more,
    // and the move it produces must be worth strictly less.
    let now = thursday();
    let venue = at_venue("XLON");
    let request =
        PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?
            .with_forecast(margin_forecast(
                &venue,
                dec!("9000000"),
                dec!("10000000"),
                dec!("11000000"),
                now,
                Duration::from_days(4),
            )?);

    let mut net_values = Vec::new();
    for uncertainty in [0.0_f64, 0.5, 2.0] {
        let planner = PrePositioningPlanner::new(
            CapitalAllocator::new(
                AllocationLimits::new(
                    dec!("100000000"),
                    dec!("100000000"),
                    dec!("100000000"),
                    dec!("100000000"),
                )?,
                DrawdownSchedule::default(),
            ),
            cost_model(dec!("25"))?.with_cost_uncertainty(uncertainty)?,
            SettlementCalendar::weekday(SettlementConvention::T1)?,
        );
        let live = idle_allocation(&planner, now)?;
        let plan = planner.plan(&request, &live, now)?;
        let move_ = plan
            .moves
            .first()
            .ok_or_else(|| qip_core::error::Error::not_found("no transfer at all"))?;
        // The benefit never moves: it is anchored to the interval's lower bound,
        // not to the cost model.
        assert!(move_.benefit_lower_bound.is_positive());
        assert!(move_.cost_upper_bound >= move_.cost.total);
        net_values.push(move_.net_value);
    }

    assert!(
        net_values.windows(2).all(|w| w[0] > w[1]),
        "widening the cost band did not reduce the net value: {net_values:?}"
    );
    Ok(())
}

// --- uncertainty must reduce conviction -------------------------------------

#[test]
fn a_wider_forecast_interval_pre_positions_less_than_a_narrow_one() -> Result<()> {
    let planner = planner()?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;
    let venue = at_venue("XLON");
    let point = dec!("10000000");

    // The point estimate is identical in every case; only the band widens.
    let mut positioned = Vec::new();
    for half in [
        dec!("1000000"),
        dec!("3000000"),
        dec!("6000000"),
        dec!("9000000"),
        dec!("10000000"),
    ] {
        let request =
            PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?
                .with_forecast(margin_forecast(
                    &venue,
                    point - half,
                    point,
                    point + half,
                    now,
                    Duration::from_days(4),
                )?);
        let plan = planner.plan(&request, &live, now)?;
        positioned.push(plan.moved_into(&venue, DemandKind::Margin));
    }

    assert!(
        positioned.windows(2).all(|w| w[0] > w[1]),
        "widening the interval did not reduce pre-positioning: {positioned:?}"
    );
    assert!(
        positioned.first().is_some_and(|first| first.is_positive()),
        "the narrowest interval did not produce a transfer, so the sweep proves nothing"
    );
    assert!(
        positioned.last().is_some_and(|last| last.is_zero()),
        "an interval reaching down to zero still pre-positioned"
    );
    Ok(())
}

#[test]
fn a_forecast_reaching_further_ahead_carries_a_wider_interval() -> Result<()> {
    // The leverage term: predicting past the end of the sample costs certainty,
    // and the further past it the more. The dispersion floor is switched off so
    // the sweep measures the fitted band rather than the floor, and the sample
    // carries noise so there is a residual for the leverage to multiply.
    let forecaster = DemandForecaster::new().with_dispersion_floor(0.0)?;
    let mut rng = Xoshiro256::seeded(0x11_5701);
    let start = thursday();
    let history: Vec<DemandObservation> = (0..12)
        .map(|day| {
            let noise = (rng.next_f64() - 0.5) * 160_000.0;
            DemandObservation::new(
                start.saturating_add(Duration::from_days(day)),
                Decimal::from_int(1_000_000 + day * 20_000 + noise as i64),
            )
        })
        .collect();
    let as_of = start.saturating_add(Duration::from_days(11));

    let mut widths = Vec::new();
    for days in [1_i64, 5, 15, 40] {
        let forecast = forecaster.forecast(
            at_venue("XLON"),
            DemandKind::Cash,
            &history,
            as_of,
            Duration::from_days(days),
        )?;
        assert!(forecast.interval().width().is_positive());
        widths.push(forecast.interval().width());
    }
    assert!(
        widths.windows(2).all(|w| w[0] < w[1]),
        "reaching further ahead did not widen the interval: {widths:?}"
    );
    Ok(())
}

#[test]
fn a_history_with_no_variation_still_produces_a_band_rather_than_a_point() -> Result<()> {
    // The exact-fit trap. Four identical readings regress with zero residual,
    // and a forecaster that reported that band honestly would hand the planner a
    // point estimate wearing an interval's clothes.
    let forecaster = DemandForecaster::new();
    let start = thursday();
    let history: Vec<DemandObservation> = (0..8)
        .map(|day| {
            DemandObservation::new(
                start.saturating_add(Duration::from_days(day)),
                dec!("5000000"),
            )
        })
        .collect();
    let forecast = forecaster.forecast(
        at_venue("XLON"),
        DemandKind::Cash,
        &history,
        start.saturating_add(Duration::from_days(7)),
        Duration::from_days(2),
    )?;
    assert!(
        forecast.interval().width().is_positive(),
        "a flat history produced a zero-width interval"
    );
    assert!(forecast.interval().lower() < forecast.interval().point());
    Ok(())
}

#[test]
fn an_interval_refuses_to_be_built_out_of_order_or_without_coverage() -> Result<()> {
    assert!(Interval::new(dec!("10"), dec!("5"), dec!("20"), 0.8).is_err());
    assert!(Interval::new(dec!("1"), dec!("5"), dec!("2"), 0.8).is_err());
    assert!(Interval::new(dec!("1"), dec!("5"), dec!("20"), 1.0).is_err());
    assert!(Interval::new(dec!("1"), dec!("5"), dec!("20"), 0.0).is_err());
    assert!(Interval::new(dec!("1"), dec!("5"), dec!("20"), 0.8).is_ok());
    Ok(())
}

// --- settlement -------------------------------------------------------------

#[test]
fn a_weekend_plan_that_assumes_same_day_availability_is_refused() -> Result<()> {
    // A collateral requirement with Saturday value, at a venue that margins
    // through the weekend against settlement rails that do not. The two
    // instructions below are three hours apart on the same Friday, and that is
    // the whole difference between the capital being there and not.
    let planner = planner_with(
        dec!("100000000"),
        dec!("100000000"),
        dec!("25"),
        SettlementConvention::T0,
    )?;
    let venue = at_venue("XLON");
    let needed_by = saturday_noon();

    let request_at = |now: Timestamp| -> Result<PrePositioningRequest> {
        Ok(
            PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?
                .with_forecast(margin_forecast(
                    &venue,
                    dec!("9000000"),
                    dec!("10000000"),
                    dec!("11000000"),
                    now,
                    needed_by.since(now),
                )?),
        )
    };

    // Even a same-day calendar does not deliver same day across a weekend.
    let late = friday_evening();
    let quote = planner.calendar().quote(late)?;
    assert!(!quote.made_cutoff, "{}", quote.describe());
    assert_eq!(quote.available_at.weekday(), 0, "{}", quote.describe());
    assert!(quote.days_in_flight_stat > 2.0, "{}", quote.describe());
    assert!(!quote.arrives_by(needed_by));

    let refused = planner.plan(&request_at(late)?, &idle_allocation(&planner, late)?, late)?;
    assert!(
        refused.moves.is_empty(),
        "capital was committed across a weekend it could not cross"
    );
    let refusal = refused
        .refusals_because(RefusalReason::SettlesTooLate)
        .next()
        .ok_or_else(|| qip_core::error::Error::not_found("no settlement refusal"))?;
    assert!(refusal.detail.contains("after"), "{}", refusal.detail);
    assert!(
        refusal.detail.contains("day(s) after it is needed"),
        "the refusal does not say how late the capital would be: {}",
        refusal.detail
    );

    // The same lane instructed three hours earlier, inside the cut-off, settles
    // on Friday and is there for the weekend.
    let early = friday_afternoon();
    let accepted = planner.plan(
        &request_at(early)?,
        &idle_allocation(&planner, early)?,
        early,
    )?;
    assert_eq!(
        accepted.moves.len(),
        1,
        "an instruction inside the cut-off was refused too, so the refusal is not about \
         settlement: {:?}",
        accepted.refusals
    );
    let move_ = accepted
        .moves
        .first()
        .ok_or_else(|| qip_core::error::Error::not_found("no move"))?;
    assert!(move_.settlement.made_cutoff);
    assert!(move_.settlement.arrives_by(needed_by));
    Ok(())
}

#[test]
fn settlement_counts_settlement_days_and_skips_the_ones_that_do_not_settle() -> Result<()> {
    let t2 = SettlementCalendar::weekday(SettlementConvention::T2)?;
    // Thursday T+2 is Monday, not Saturday.
    let quote = t2.quote(thursday())?;
    assert!(quote.made_cutoff);
    assert_eq!(quote.available_at.weekday(), 0, "{}", quote.describe());
    assert!(quote.days_in_flight_stat > 3.0);

    // Friday evening on a T+1 calendar rolls to a Monday value date and settles
    // on Tuesday: two boundaries in one instruction.
    let t1 = SettlementCalendar::weekday(SettlementConvention::T1)?;
    let late = t1.quote(friday_evening())?;
    assert!(!late.made_cutoff);
    assert_eq!(late.value_date.weekday(), 0);
    assert_eq!(late.available_at.weekday(), 1);

    // A holiday on the Monday pushes both a day further.
    let with_holiday = t1.clone().with_holiday(monday_morning());
    let over_holiday = with_holiday.quote(friday_evening())?;
    assert_eq!(over_holiday.value_date.weekday(), 1);
    assert_eq!(over_holiday.available_at.weekday(), 2);

    // Saturdays do not settle, and neither does the holiday.
    assert!(!t1.is_settlement_day(Timestamp::from_civil(2024, 3, 9)));
    assert!(t1.is_settlement_day(monday_morning()));
    assert!(!with_holiday.is_settlement_day(monday_morning()));
    Ok(())
}

#[test]
fn a_settlement_calendar_that_pays_out_before_its_own_cut_off_is_refused() -> Result<()> {
    let weekdays = qip_financial::calendar::MarketHours::weekday_session("SETTLEMENT", 0, 1440);
    assert!(
        SettlementCalendar::new(weekdays.clone(), SettlementConvention::T0, 16 * 60, 9 * 60)
            .is_err(),
        "a calendar settling before it accepts instructions was allowed"
    );
    assert!(SettlementCalendar::new(weekdays, SettlementConvention::T0, 16 * 60, 17 * 60).is_ok());
    Ok(())
}

// --- the asymmetry ----------------------------------------------------------

#[test]
fn a_shortfall_is_penalised_harder_than_an_equivalent_surplus() -> Result<()> {
    let mut rng = Xoshiro256::seeded(0x000A_55E7);
    for kind in [
        DemandKind::Cash,
        DemandKind::Collateral,
        DemandKind::FxFunding,
        DemandKind::Inventory,
        DemandKind::Margin,
    ] {
        let asymmetry = ShortfallAsymmetry::for_kind(kind)?;
        assert!(asymmetry.multiple() > 1.0);
        assert!(asymmetry.critical_fractile() > 0.5);
        for _ in 0..200 {
            let gap = Decimal::from_int(1 + (rng.next_u64() % 50_000_000) as i64);
            let over = Duration::from_hours(1 + (rng.next_u64() % 400) as i64);
            let short = asymmetry.shortfall_penalty(gap, over);
            let long = asymmetry.surplus_penalty(gap, over);
            assert!(
                short > long,
                "{}: a {gap} shortfall cost {short} against {long} for the same surplus",
                kind.as_str()
            );
            assert_eq!(asymmetry.penalty(-gap, over), short);
            assert_eq!(asymmetry.penalty(gap, over), long);
        }
        // Contractual demands are penalised harder still, because a shortfall
        // there is somebody else choosing what to sell.
        if kind.is_contractual() {
            assert!(
                asymmetry.multiple() >= ShortfallAsymmetry::for_kind(DemandKind::Cash)?.multiple()
            );
        }
    }
    Ok(())
}

#[test]
fn a_symmetric_penalty_cannot_be_configured() -> Result<()> {
    assert!(ShortfallAsymmetry::new(400.0, 400.0).is_err());
    assert!(ShortfallAsymmetry::new(300.0, 400.0).is_err());
    assert!(ShortfallAsymmetry::new(400.0, -1.0).is_err());
    let error = ShortfallAsymmetry::new(400.0, 400.0)
        .err()
        .map(|e| e.message().to_string())
        .unwrap_or_default();
    assert!(
        error.contains("under-positions"),
        "the refusal does not say why symmetry is wrong: {error}"
    );
    assert!(ShortfallAsymmetry::new(1200.0, 400.0).is_ok());
    Ok(())
}

#[test]
fn the_asymmetry_positions_above_the_demand_it_is_confident_about() -> Result<()> {
    let planner = planner()?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;
    let venue = at_venue("XLON");
    let request =
        PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?
            .with_forecast(margin_forecast(
                &venue,
                dec!("9000000"),
                dec!("10000000"),
                dec!("11000000"),
                now,
                Duration::from_days(4),
            )?);
    let plan = planner.plan(&request, &live, now)?;
    let positioned = plan.moved_into(&venue, DemandKind::Margin);
    assert!(
        positioned > dec!("9000000"),
        "the buffer the asymmetry buys was not applied: {positioned}"
    );
    // But never so far as the upper bound, which is the part of the forecast the
    // planner does not trust.
    assert!(
        positioned < dec!("11000000"),
        "the planner reached into the wide part of the interval: {positioned}"
    );
    Ok(())
}

// --- composing qip-capital's limits -----------------------------------------

#[test]
fn a_transfer_breaching_the_allocators_venue_limit_is_refused_not_clipped() -> Result<()> {
    // The venue already carries a live strategy allocation from `qip-capital`;
    // the fabric must fit inside what is left, and where it cannot, it must
    // decline rather than send a smaller wire nobody decided on.
    let planner = planner_with(
        dec!("100000000"),
        dec!("20000000"),
        dec!("25"),
        SettlementConvention::T1,
    )?;
    let now = thursday();
    let live = planner.allocator().allocate(
        &[proposal("momentum-v3", "XLON", 2.4, 4_000_000)?],
        0.0,
        now,
    )?;
    let venue = at_venue("XLON");
    let already = live.for_venue(&VenueId::new("XLON"));
    assert!(
        already.is_positive(),
        "the live allocation did not commit anything at the venue, so the test is vacuous"
    );

    let headroom = planner
        .allocator()
        .limits()
        .venue_limit(&VenueId::new("XLON"))
        - already;
    // Ask for more than the headroom, comfortably.
    let ask = headroom + dec!("5000000");
    let request =
        PrePositioningRequest::new(treasury(), dec!("90000000"), FxRates::new(Currency::USD))?
            .with_forecast(margin_forecast(
                &venue,
                ask,
                ask + dec!("1000000"),
                ask + dec!("2000000"),
                now,
                Duration::from_days(4),
            )?);

    let plan = planner.plan(&request, &live, now)?;
    assert!(
        plan.moved_into(&venue, DemandKind::Margin).is_zero(),
        "the transfer was silently clipped to fit rather than refused"
    );
    let refusal = plan
        .refusals_because(RefusalReason::VenueLimit)
        .next()
        .ok_or_else(|| qip_core::error::Error::not_found("no venue-limit refusal"))?;
    assert!(
        refusal.detail.contains(&already.to_string()),
        "the refusal does not name what the venue already carries: {}",
        refusal.detail
    );
    assert!(
        refusal.detail.contains(
            &planner
                .allocator()
                .limits()
                .venue_limit(&VenueId::new("XLON"))
                .to_string()
        ),
        "the refusal does not name the limit: {}",
        refusal.detail
    );
    Ok(())
}

#[test]
fn the_drawdown_response_shuts_the_fabric_down_with_the_rest_of_the_book() -> Result<()> {
    // The fabric spends the allocator's headroom, so a drawdown deep enough to
    // take the budget to zero stops pre-positioning too. A layer that kept
    // moving capital around a book being taken off would be routing around the
    // risk response.
    let planner = planner()?;
    let now = thursday();
    let venue = at_venue("XLON");
    let request =
        PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?
            .with_forecast(margin_forecast(
                &venue,
                dec!("9000000"),
                dec!("10000000"),
                dec!("11000000"),
                now,
                Duration::from_days(4),
            )?);

    let calm = planner.plan(
        &request,
        &planner.allocator().allocate(&[], 0.02, now)?,
        now,
    )?;
    assert_eq!(calm.moves.len(), 1);

    let deep = planner.allocator().allocate(&[], 0.30, now)?;
    assert!(deep.budget.is_zero(), "the fixture drawdown did not bite");
    let stopped = planner.plan(&request, &deep, now)?;
    assert!(stopped.moves.is_empty());
    assert!(stopped.budget.is_zero());
    assert!(stopped.is_within_budget());
    Ok(())
}

// --- scoring after the fact -------------------------------------------------

/// Two lanes: one that really needs capital, one whose forecast is wrong.
fn scoring_request(
    ghost_lower: Decimal,
    ghost_point: Decimal,
    at: Timestamp,
) -> Result<PrePositioningRequest> {
    Ok(
        PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?
            .with_forecast(margin_forecast(
                &at_venue("XLON"),
                dec!("9000000"),
                dec!("10000000"),
                dec!("11000000"),
                at,
                Duration::from_days(4),
            )?)
            .with_forecast(margin_forecast(
                &at_venue("XETR"),
                ghost_lower,
                ghost_point,
                ghost_point + dec!("1000000"),
                at,
                Duration::from_days(4),
            )?),
    )
}

#[test]
fn evaluate_scores_a_good_plan_above_a_bad_one_on_the_same_realised_demand() -> Result<()> {
    let planner = planner()?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;

    // What actually happened: London needed what was forecast, Frankfurt needed
    // nothing at all.
    let realised = RealisedDemand::new()
        .with(at_venue("XLON"), DemandKind::Margin, dec!("10000000"))
        .with(at_venue("XETR"), DemandKind::Margin, Decimal::ZERO);

    // The good plan declined the Frankfurt lane: its lower bound was zero.
    let good = planner.plan(
        &scoring_request(Decimal::ZERO, dec!("500000"), now)?,
        &live,
        now,
    )?;
    // The bad plan believed a confident Frankfurt demand that never arrived.
    let bad = planner.plan(
        &scoring_request(dec!("8000000"), dec!("9000000"), now)?,
        &live,
        now,
    )?;

    assert_eq!(
        good.moves.len(),
        1,
        "the good plan moved the wrong number of times"
    );
    assert_eq!(
        bad.moves.len(),
        2,
        "the bad plan did not take the ghost lane"
    );

    let good_score = evaluate(&good, &realised)?;
    let bad_score = evaluate(&bad, &realised)?;

    assert!(
        good_score.net_value > bad_score.net_value,
        "the bad plan scored at least as well: {} against {}",
        good_score.net_value,
        bad_score.net_value
    );
    assert!(
        good_score.net_value.is_positive(),
        "{}",
        good_score.describe()
    );
    assert!(bad_score.idle_surplus > good_score.idle_surplus);
    assert!(good_score.coverage_ratio_stat >= bad_score.coverage_ratio_stat);
    // The interval calibration check: Frankfurt's realised demand fell outside
    // the band the bad plan believed, and the score says so.
    assert!(bad_score.interval_hit_rate_stat < 1.0);
    Ok(())
}

#[test]
fn a_plan_that_pre_positioned_nothing_is_scored_rather_than_skipped() -> Result<()> {
    let planner = planner()?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;
    // No mobile capital at all, so every lane is refused for want of headroom.
    let request =
        PrePositioningRequest::new(treasury(), Decimal::ZERO, FxRates::new(Currency::USD))?
            .with_forecast(margin_forecast(
                &at_venue("XLON"),
                dec!("9000000"),
                dec!("10000000"),
                dec!("11000000"),
                now,
                Duration::from_days(4),
            )?);

    let plan = planner.plan(&request, &live, now)?;
    assert!(plan.moves.is_empty());
    assert_eq!(plan.lanes.len(), 1, "the lane was not recorded");

    let realised =
        RealisedDemand::new().with(at_venue("XLON"), DemandKind::Margin, dec!("10000000"));
    let score = evaluate(&plan, &realised)?;

    assert_eq!(
        score.lanes.len(),
        1,
        "the empty plan produced no lane outcome"
    );
    assert!(
        score.shortfall.is_positive(),
        "the shortfall went unrecorded"
    );
    assert!(score.positioned.is_zero());
    // Doing nothing scores exactly zero: it is the baseline, neither punished
    // nor credited, and it is a number rather than an absence.
    assert_eq!(score.net_value, Decimal::ZERO);
    assert!(!score.beat_doing_nothing());
    assert!(score.coverage_ratio_stat < 1.0);
    Ok(())
}

#[test]
fn scoring_reports_the_forecast_error_a_forecaster_could_be_improved_on() -> Result<()> {
    let planner = planner()?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;
    let venue = at_venue("XLON");
    let request =
        PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?
            .with_forecast(margin_forecast(
                &venue,
                dec!("9000000"),
                dec!("10000000"),
                dec!("11000000"),
                now,
                Duration::from_days(4),
            )?);
    let plan = planner.plan(&request, &live, now)?;

    // Demand came in above the point estimate: the forecaster ran low.
    let under = RealisedDemand::new().with(venue.clone(), DemandKind::Margin, dec!("10500000"));
    let under_score = evaluate(&plan, &under)?;
    assert!(
        under_score.bias_stat > 0.0,
        "an under-forecast read as unbiased"
    );

    // And above it: the bias flips sign rather than being reported as magnitude.
    let over = RealisedDemand::new().with(venue, DemandKind::Margin, dec!("9500000"));
    let over_score = evaluate(&plan, &over)?;
    assert!(over_score.bias_stat < 0.0);
    assert!(under_score.mean_absolute_error_stat > 0.0);
    assert!(over_score.mean_absolute_error_stat > 0.0);
    Ok(())
}

// --- determinism ------------------------------------------------------------

#[test]
fn the_same_inputs_and_seed_reproduce_the_same_plan_and_the_same_score() -> Result<()> {
    let planner = planner()?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;

    let build = |order_reversed: bool| -> Result<PrePositioningPlan> {
        let mut forecasts = vec![
            margin_forecast(
                &at_venue("XLON"),
                dec!("9000000"),
                dec!("10000000"),
                dec!("11000000"),
                now,
                Duration::from_days(4),
            )?,
            margin_forecast(
                &at_venue("XETR"),
                dec!("4000000"),
                dec!("5000000"),
                dec!("6000000"),
                now,
                Duration::from_days(4),
            )?,
            margin_forecast(
                &at_venue("XPAR"),
                dec!("2000000"),
                dec!("3000000"),
                dec!("4000000"),
                now,
                Duration::from_days(4),
            )?,
        ];
        if order_reversed {
            forecasts.reverse();
        }
        let mut request =
            PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?;
        for forecast in forecasts {
            request = request.with_forecast(forecast);
        }
        planner.plan(&request, &live, now)
    };

    let first = build(false)?;
    let second = build(false)?;
    assert_eq!(first, second, "two identical runs produced different plans");
    // And the order the forecasts arrived in does not change the answer, which
    // is what makes a plan diffable against the previous one.
    assert_eq!(first, build(true)?, "input order changed the plan");

    // The one stochastic operation in the crate: same seed, same world.
    let forecasts: Vec<DemandForecast> = vec![
        margin_forecast(
            &at_venue("XLON"),
            dec!("9000000"),
            dec!("10000000"),
            dec!("11000000"),
            now,
            Duration::from_days(4),
        )?,
        margin_forecast(
            &at_venue("XETR"),
            dec!("4000000"),
            dec!("5000000"),
            dec!("6000000"),
            now,
            Duration::from_days(4),
        )?,
    ];
    let mut rng_a = Xoshiro256::seeded(0xFEED_BEEF);
    let mut rng_b = Xoshiro256::seeded(0xFEED_BEEF);
    let mut rng_c = Xoshiro256::seeded(0x0BAD_C0DE);
    let world_a = RealisedDemand::sample(&forecasts, &mut rng_a);
    let world_b = RealisedDemand::sample(&forecasts, &mut rng_b);
    let world_c = RealisedDemand::sample(&forecasts, &mut rng_c);
    assert_eq!(
        world_a, world_b,
        "a fixed seed produced two different worlds"
    );
    assert_ne!(world_a, world_c, "two seeds produced the same world");

    assert_eq!(evaluate(&first, &world_a)?, evaluate(&second, &world_b)?);
    Ok(())
}

#[test]
fn a_backtest_over_many_sampled_worlds_reproduces_exactly() -> Result<()> {
    // The shape a backtest actually takes: plan once, score against many drawn
    // worlds, average. Two runs of the whole loop must agree to the last unit,
    // or nothing measured across two versions of the forecaster is comparable.
    let planner = planner()?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;
    let forecasts: Vec<DemandForecast> = ["XLON", "XETR", "XPAR"]
        .iter()
        .map(|venue| {
            margin_forecast(
                &at_venue(venue),
                dec!("6000000"),
                dec!("8000000"),
                dec!("10000000"),
                now,
                Duration::from_days(4),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let mut request =
        PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?;
    for forecast in forecasts.clone() {
        request = request.with_forecast(forecast);
    }
    let plan = planner.plan(&request, &live, now)?;
    assert!(!plan.moves.is_empty());

    let run = |seed: u64| -> Result<Decimal> {
        let mut rng = Xoshiro256::seeded(seed);
        let mut total = Decimal::ZERO;
        for _ in 0..64 {
            let world = RealisedDemand::sample(&forecasts, &mut rng);
            total += evaluate(&plan, &world)?.net_value;
        }
        Ok(total)
    };
    assert_eq!(run(0x51EED)?, run(0x51EED)?);
    assert_ne!(run(0x51EED)?, run(0x51EEE)?);
    Ok(())
}

// --- the optimisation report ------------------------------------------------

#[test]
fn the_greedy_allocation_is_reported_against_its_own_relaxation_bound() -> Result<()> {
    // The bound is an upper bound on what any plan could have achieved against
    // the same budget and the same venue limits, so the plan can never beat it.
    let planner = planner_with(
        dec!("100000000"),
        dec!("100000000"),
        dec!("25"),
        SettlementConvention::T1,
    )?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;
    let mut request =
        PrePositioningRequest::new(treasury(), dec!("12000000"), FxRates::new(Currency::USD))?;
    for (index, venue) in ["XLON", "XETR", "XPAR", "XAMS"].iter().enumerate() {
        let point = Decimal::from_int(4_000_000 + index as i64 * 1_500_000);
        request = request.with_forecast(margin_forecast(
            &at_venue(venue),
            point - dec!("500000"),
            point,
            point + dec!("500000"),
            now,
            Duration::from_days(4 + index as i64),
        )?);
    }
    let plan = planner.plan(&request, &live, now)?;
    let bound = plan
        .relaxation_bound_stat
        .ok_or_else(|| qip_core::error::Error::not_found("no relaxation bound was reported"))?;
    assert!(
        plan.expected_net_value().to_f64() <= bound + 1e-6,
        "the plan claims {} against an upper bound of {bound}",
        plan.expected_net_value()
    );
    assert!(bound > 0.0);
    assert!(!plan.describe().is_empty());
    Ok(())
}

// --- currency ---------------------------------------------------------------

#[test]
fn an_unpriced_currency_is_refused_rather_than_assumed_to_be_at_parity() -> Result<()> {
    let rates = FxRates::new(Currency::USD).with_rate(Currency::JPY, dec!("0.0067"))?;
    assert_eq!(rates.to_base(dec!("100"), Currency::USD)?, dec!("100"));
    assert_eq!(rates.to_base(dec!("100"), Currency::JPY)?, dec!("0.67"));
    assert!(rates.to_base(dec!("100"), Currency::GBP).is_err());
    assert!(
        rates
            .convert(dec!("100"), Currency::USD, Currency::GBP)
            .is_err()
    );
    assert!(rates.with_rate(Currency::GBP, Decimal::ZERO).is_err());
    Ok(())
}

#[test]
fn a_cross_currency_transfer_pays_the_impact_of_being_a_large_one() -> Result<()> {
    // Square-root impact, reused from `qip-financial`: doubling the size more
    // than doubles the conversion cost, so a transfer that is a meaningful share
    // of a thin market pays for being one.
    let model = cost_model(dec!("25"))?;
    let calendar = SettlementCalendar::weekday(SettlementConvention::T1)?;
    let quote = calendar.quote(thursday())?;
    let from = treasury();
    let to = CapitalLocation::new(Region::new("apac"), Currency::JPY, VenueId::new("XTKS"));

    let small = model.price(
        dec!("100000000"),
        &from,
        &to,
        &quote,
        Duration::from_days(2),
    )?;
    let large = model.price(
        dec!("400000000"),
        &from,
        &to,
        &quote,
        Duration::from_days(2),
    )?;
    assert!(large.fx_conversion > small.fx_conversion * dec!("4"));
    assert!(large.upper > large.total);
    assert!(!large.describe().is_empty());

    // Within a currency there is no conversion leg at all.
    let domestic = model.price(
        dec!("100000000"),
        &from,
        &at_venue("XLON"),
        &quote,
        Duration::from_days(2),
    )?;
    assert!(domestic.fx_conversion.is_zero());
    assert!(
        model
            .price(dec!("-1"), &from, &to, &quote, Duration::ZERO)
            .is_err()
    );
    Ok(())
}

#[test]
fn a_lane_already_covered_by_what_is_on_hand_is_not_topped_up_on_a_point_estimate() -> Result<()> {
    let planner = planner()?;
    let now = thursday();
    let live = idle_allocation(&planner, now)?;
    let venue = at_venue("XLON");
    // On hand covers the lower bound but not the point estimate. Moving on the
    // difference would be pre-positioning on a number the forecaster cannot
    // defend.
    let request =
        PrePositioningRequest::new(treasury(), dec!("50000000"), FxRates::new(Currency::USD))?
            .with_balance(LocationBalance::new(
                venue.clone(),
                DemandKind::Margin,
                dec!("9500000"),
            )?)
            .with_forecast(margin_forecast(
                &venue,
                dec!("9000000"),
                dec!("10000000"),
                dec!("11000000"),
                now,
                Duration::from_days(4),
            )?);

    let plan = planner.plan(&request, &live, now)?;
    assert!(plan.moves.is_empty());
    let refusal = plan
        .refusals_because(RefusalReason::NoConfidentDemand)
        .next()
        .ok_or_else(|| qip_core::error::Error::not_found("no confidence refusal"))?;
    assert!(
        refusal.detail.contains("point estimate"),
        "{}",
        refusal.detail
    );
    assert!(!refusal.describe().is_empty());
    Ok(())
}

#[test]
fn a_forecaster_with_no_history_refuses_rather_than_defaulting() -> Result<()> {
    let forecaster = DemandForecaster::new();
    assert!(
        forecaster
            .forecast(
                at_venue("XLON"),
                DemandKind::Cash,
                &[],
                thursday(),
                Duration::from_days(2),
            )
            .is_err()
    );
    // And a forecast that reaches nowhere is not a forecast.
    let history = [DemandObservation::new(thursday(), dec!("1000"))];
    assert!(
        forecaster
            .forecast(
                at_venue("XLON"),
                DemandKind::Cash,
                &history,
                thursday(),
                Duration::ZERO,
            )
            .is_err()
    );
    assert!(forecaster.with_confidence(1.5).is_err());
    assert!(forecaster.with_confidence(0.95).is_ok());
    Ok(())
}

#[test]
fn a_forecaster_refuses_history_that_reaches_past_its_own_as_of_instant() -> Result<()> {
    // Point-in-time leakage: fitting a forecast made "as of" some instant on an
    // observation dated after that instant is how a backtest built from a
    // full-run history reports a forecaster that "knew" a shortfall was coming
    // for a lane, when the production loop — which only ever appends an
    // observation once a fill has actually happened — never has one to leak.
    let forecaster = DemandForecaster::new();
    let as_of = thursday();
    let past = vec![
        DemandObservation::new(
            as_of.saturating_sub(Duration::from_days(2)),
            dec!("1000000"),
        ),
        DemandObservation::new(
            as_of.saturating_sub(Duration::from_days(1)),
            dec!("1100000"),
        ),
        DemandObservation::new(as_of, dec!("1200000")),
    ];

    // Assert the premise first: a history entirely at or before `as_of` fits
    // cleanly, so the refusal below is caused by the future-dated entry added
    // to it and not by something else about the shape of the history.
    assert!(
        forecaster
            .forecast(
                at_venue("XLON"),
                DemandKind::Cash,
                &past,
                as_of,
                Duration::from_days(3),
            )
            .is_ok(),
        "a history entirely at or before as_of was refused for the wrong reason"
    );

    let mut leaking = past;
    leaking.push(DemandObservation::new(
        as_of.saturating_add(Duration::from_days(1)),
        dec!("5000000"),
    ));

    let err = forecaster
        .forecast(
            at_venue("XLON"),
            DemandKind::Cash,
            &leaking,
            as_of,
            Duration::from_days(3),
        )
        .expect_err(
            "a forecast fitted on an observation from after its own as-of instant was accepted",
        );
    assert!(
        err.to_string()
            .contains("after the forecast's as-of instant"),
        "the refusal did not name the leakage as the reason: {err}"
    );
    Ok(())
}
