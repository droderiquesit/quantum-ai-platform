//! Illiquid valuation: a mark with a method and a confidence, or no mark.
//!
//! The failure this guards is a single one and it is the worst available to
//! this plane: a number nobody could observe, returned as a valuation. Such a
//! number is indistinguishable downstream from an observed one, and it will
//! support leverage on its own authority. Every test below is a way that could
//! happen.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion aborting a `Result`-returning function is a bug. In a test the
// assertion is the deliverable and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_financial::asset_class::InstrumentType;
use qip_financial::cashflow::{CashflowForecast, CashflowKind, ForecastCashflow};
use qip_financial::extensions::{Extension, PrivateAssetDetails};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::valuation::{IlliquidValuator, ValuationInput, ValuationMethod};

fn origin() -> Timestamp {
    Timestamp::from_civil(2020, 1, 1)
}

fn day(n: i64) -> Timestamp {
    origin().saturating_add(Duration::from_days(n))
}

fn details(residual: Decimal, called: Decimal, distributed: Decimal) -> PrivateAssetDetails {
    PrivateAssetDetails {
        vintage_year: 2020,
        committed_capital: dec!("1000"),
        called_capital: called,
        distributed_capital: distributed,
        residual_value: residual,
        stage: "buyout".to_string(),
        lockup_years: 7.0,
        capital_call_notice_days: 10,
    }
}

fn private_object(symbol: &str, details: PrivateAssetDetails) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(format!("obj-{symbol}")),
        symbol,
        InstrumentType::PrivateEquityFund,
    )
    .venue("OTC")
    .price(dec!("1"))
    .extension(Extension::PrivateAsset(details))
    .provenance(Provenance::new("vendor", origin(), origin()))
    .build(day(365))
}

#[test]
fn an_asset_with_no_observable_comparable_gets_no_comparables_mark() -> Result<()> {
    // Premise: with one comparable the mark is produced, so the refusal below
    // is about the absence of evidence and not about the method being unusable.
    let observed = IlliquidValuator::from_comparables(
        "obj-venture",
        vec![ValuationInput::new("peer-a", dec!("100"), 1.0, origin())?],
        day(1),
    )?;
    assert_eq!(observed.value(), dec!("100"));

    let refusal = IlliquidValuator::from_comparables("obj-venture", Vec::new(), day(1))
        .expect_err("no comparable means no comparables mark");
    let message = refusal.message();
    assert!(
        message.contains("no observable comparable transaction"),
        "the refusal must name the missing evidence, said: {message}"
    );
    assert!(
        message.contains("this plane does not invent a mark"),
        "the refusal must say why there is no fallback, said: {message}"
    );
    Ok(())
}

#[test]
fn a_private_asset_with_no_residual_and_no_net_cost_is_refused_rather_than_marked_at_zero()
-> Result<()> {
    // Premise: the same record carrying a net cost does get a mark, so the
    // refusal below is about having nothing to observe.
    let at_cost = IlliquidValuator::mark_private_asset(
        "obj-venture",
        &details(Decimal::ZERO, dec!("400"), dec!("100")),
        origin(),
        origin(),
        None,
        day(1),
    )?;
    assert_eq!(at_cost.method(), ValuationMethod::Cost);
    assert_eq!(at_cost.value(), dec!("300"));

    let refusal = IlliquidValuator::mark_private_asset(
        "obj-venture",
        &details(Decimal::ZERO, dec!("400"), dec!("400")),
        origin(),
        origin(),
        None,
        day(1),
    )
    .expect_err("a record with nothing observable must not be marked");
    let message = refusal.message();
    assert!(
        message.contains("nothing observable to mark it from"),
        "the refusal must name the gap, said: {message}"
    );
    assert!(
        message.contains("refuses to invent a mark"),
        "the refusal must say the plane will not fabricate, said: {message}"
    );
    Ok(())
}

#[test]
fn an_input_that_became_knowable_after_the_valuation_instant_is_refused() -> Result<()> {
    // The point-in-time leak in its purest form: a February round used to mark
    // a January book. The mark would be right, and the platform could not have
    // known it.
    // Premise: the same round dated on the valuation instant is accepted.
    let honest = IlliquidValuator::from_last_round("obj-venture", dec!("500"), day(10), day(10))?;
    assert_eq!(honest.value(), dec!("500"));

    let refusal = IlliquidValuator::from_last_round("obj-venture", dec!("500"), day(40), day(10))
        .expect_err("a round dated after the valuation instant must be refused");
    let message = refusal.message();
    assert!(
        message.contains("after the valuation instant"),
        "the refusal must name the ordering, said: {message}"
    );
    assert!(
        message.contains("point-in-time leak"),
        "the refusal must name the defect class, said: {message}"
    );
    Ok(())
}

#[test]
fn a_mark_loses_exactly_half_its_confidence_over_one_half_life() -> Result<()> {
    let mark = IlliquidValuator::from_last_round("obj-venture", dec!("500"), origin(), origin())?;
    let struck = mark.struck_confidence();
    // Premise: the mark starts at the last-round base confidence, so the decay
    // below is measured against a known starting point rather than whatever
    // the constructor happened to store.
    assert!(
        (struck - ValuationMethod::LastRound.base_confidence()).abs() < 1e-12,
        "a last-round mark starts at the table's base confidence, was {struck}"
    );

    let half_life = ValuationMethod::LastRound.decay_half_life();
    let after_one = mark.confidence_at(origin().saturating_add(half_life))?;
    assert!(
        (after_one - struck / 2.0).abs() < 1e-9,
        "one half-life must halve the confidence: {after_one} against {struck}"
    );
    let after_two = mark
        .confidence_at(origin().saturating_add(Duration::from_nanos(half_life.as_nanos() * 2)))?;
    assert!(
        (after_two - struck / 4.0).abs() < 1e-9,
        "two half-lives must quarter it: {after_two} against {struck}"
    );
    Ok(())
}

#[test]
fn an_uncertain_mark_supports_less_notional_than_a_certain_one() -> Result<()> {
    // §16.3: confidence enters position sizing, so an uncertain mark cannot
    // silently support leverage. This is the arithmetic that makes it true.
    let cost = IlliquidValuator::at_cost("obj-a", dec!("1000"), origin(), origin())?;
    let quote = IlliquidValuator::from_quote("obj-b", dec!("1000"), origin(), origin())?;
    // Premise: both marks are the same money on the same day, so the only
    // difference in what they support is the method's confidence.
    assert_eq!(cost.value(), quote.value());

    let supported_by_cost = cost.supportable_value(origin())?;
    let supported_by_quote = quote.supportable_value(origin())?;
    assert!(
        supported_by_cost < supported_by_quote,
        "a cost mark must support strictly less than a quote: {supported_by_cost} against \
         {supported_by_quote}"
    );
    assert_eq!(
        cost.haircut(dec!("1000"), origin())?,
        dec!("1000") - supported_by_cost,
        "the haircut is the part of the notional the mark does not stand behind"
    );
    Ok(())
}

#[test]
fn a_marks_confidence_cannot_be_read_before_it_was_struck() -> Result<()> {
    let mark = IlliquidValuator::at_cost("obj-a", dec!("1000"), origin(), day(10))?;
    // Premise: read on the day it was struck, it answers.
    assert!(mark.confidence_at(day(10))? > 0.0);

    let refusal = mark
        .confidence_at(day(9))
        .expect_err("a mark carries no weight before it exists");
    assert!(
        refusal
            .message()
            .contains("carries no weight before it exists"),
        "the refusal must say so, said: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_mark_is_stale_only_after_its_own_review_interval_has_passed() -> Result<()> {
    let quote = IlliquidValuator::from_quote("obj-a", dec!("100"), origin(), origin())?;
    let cost = IlliquidValuator::at_cost("obj-b", dec!("100"), origin(), origin())?;
    // Premise: the two methods declare different review intervals, so this
    // measures the mark's own interval and not one shared constant.
    assert_ne!(quote.next_review(), cost.next_review());

    assert!(!quote.is_stale(origin()), "a mark is fresh when struck");
    assert!(
        quote.is_stale(day(2)),
        "a quote must be stale two days after it was struck"
    );
    assert!(
        !cost.is_stale(day(2)),
        "a cost mark reviewed annually is not stale after two days"
    );
    Ok(())
}

#[test]
fn a_stream_whose_calls_exceed_its_distributions_is_a_liability_not_a_mark() -> Result<()> {
    // Premise: the distribution-heavy stream does produce a mark, so the
    // refusal below is about the sign of the present value.
    let positive = CashflowForecast::new("obj-fund", origin(), origin())?.with_flow(
        ForecastCashflow::new(CashflowKind::Distribution, day(365), dec!("1000"), 1.0)?,
    )?;
    let mark = IlliquidValuator::from_discounted_cashflow("obj-fund", &positive, 0.1, origin())?;
    assert_eq!(mark.method(), ValuationMethod::DiscountedCashflow);
    assert!(
        mark.value() < dec!("1000"),
        "a year of discounting must bite"
    );

    let negative = CashflowForecast::new("obj-fund", origin(), origin())?.with_flow(
        ForecastCashflow::new(CashflowKind::CapitalCall, day(365), dec!("1000"), 1.0)?,
    )?;
    let refusal = IlliquidValuator::from_discounted_cashflow("obj-fund", &negative, 0.1, origin())
        .expect_err("a net liability must not be marked as an asset");
    assert!(
        refusal
            .message()
            .contains("record it as a commitment instead"),
        "the refusal must say where it belongs, said: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_forecast_for_another_subject_cannot_mark_this_one() -> Result<()> {
    let forecast = CashflowForecast::new("obj-other", origin(), origin())?.with_flow(
        ForecastCashflow::new(CashflowKind::Distribution, day(365), dec!("1000"), 1.0)?,
    )?;
    // Premise: the same forecast marks its own subject.
    IlliquidValuator::from_discounted_cashflow("obj-other", &forecast, 0.1, origin())?;

    let refusal = IlliquidValuator::from_discounted_cashflow("obj-fund", &forecast, 0.1, origin())
        .expect_err("a forecast must not mark a subject it does not describe");
    assert!(
        refusal
            .message()
            .contains("discount the subject's own schedule"),
        "the refusal must say what to do instead, said: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_private_record_with_a_required_yield_is_discounted_rather_than_taken_at_face_value()
-> Result<()> {
    // The ladder in `mark_private_asset`: a residual value plus a rate plus a
    // lockup still running is a one-flow discounted cashflow, and every input
    // is on the record.
    let record = details(dec!("1000"), dec!("800"), Decimal::ZERO);
    let discounted = IlliquidValuator::mark_private_asset(
        "obj-fund",
        &record,
        origin(),
        origin(),
        Some(0.12),
        day(1),
    )?;
    assert_eq!(discounted.method(), ValuationMethod::DiscountedCashflow);
    assert!(
        discounted.value() < dec!("1000"),
        "seven years of discounting at 12% must be well below face, was {}",
        discounted.value()
    );

    // Premise: strip only the rate and the same record falls to a last-round
    // mark at face value, so the difference above is the discounting.
    let face = IlliquidValuator::mark_private_asset(
        "obj-fund",
        &record,
        origin(),
        origin(),
        None,
        day(1),
    )?;
    assert_eq!(face.method(), ValuationMethod::LastRound);
    assert_eq!(face.value(), dec!("1000"));
    Ok(())
}

#[test]
fn marking_an_object_that_is_not_a_private_asset_reports_not_mine_rather_than_a_refusal()
-> Result<()> {
    // "Not my instrument" and "your instrument cannot be marked" are different
    // facts, and a sweep over a universe must be able to tell them apart.
    let equity = FinancialObject::builder(
        ObjectId::from_string("obj-aaa"),
        "AAA",
        InstrumentType::CommonStock,
    )
    .venue("XNYS")
    .price(dec!("100"))
    .provenance(Provenance::new("vendor", origin(), origin()))
    .build(day(365))?;
    assert!(
        IlliquidValuator::mark_object(&equity, origin(), day(400))?.is_none(),
        "a listed equity is not this engine's business"
    );

    // Premise: a private object of the same shape does produce a mark, so the
    // `None` above is about the extension and not about the sweep failing.
    let fund = private_object("FUND", details(dec!("1000"), dec!("800"), Decimal::ZERO))?;
    let mark = IlliquidValuator::mark_object(&fund, origin(), day(400))?
        .expect("a private record with a residual value is markable");
    assert_eq!(mark.asset(), "obj-FUND");
    assert_eq!(mark.method(), ValuationMethod::LastRound);

    let barren = private_object("BARE", details(Decimal::ZERO, dec!("400"), dec!("400")))?;
    assert!(
        IlliquidValuator::mark_object(&barren, origin(), day(400)).is_err(),
        "a private record with nothing observable must refuse, not report None"
    );
    Ok(())
}

#[test]
fn a_comparables_mark_is_no_more_confident_than_its_weakest_comparable() -> Result<()> {
    let strong = IlliquidValuator::from_comparables(
        "obj-a",
        vec![
            ValuationInput::new("peer-a", dec!("100"), 1.0, origin())?,
            ValuationInput::new("peer-b", dec!("300"), 1.0, origin())?,
        ],
        day(1),
    )?;
    let weak = IlliquidValuator::from_comparables(
        "obj-a",
        vec![
            ValuationInput::new("peer-a", dec!("100"), 1.0, origin())?,
            ValuationInput::new("peer-b", dec!("300"), 0.25, origin())?,
        ],
        day(1),
    )?;
    // Premise: both produce the same mark from the same two observations, so
    // the confidence is the only thing that differs.
    assert_eq!(strong.value(), dec!("200"));
    assert_eq!(weak.value(), strong.value());
    assert!(
        weak.struck_confidence() < strong.struck_confidence(),
        "a doubtful comparable must drag the mark down: {} against {}",
        weak.struck_confidence(),
        strong.struck_confidence()
    );
    Ok(())
}

#[test]
fn the_confidence_table_ranks_the_methods_strictly() -> Result<()> {
    // A fallback that costs nothing will be taken. Premise: the ladder is
    // read in the blueprint's own order and every step must be a real step.
    let ladder = [
        ValuationMethod::Quoted,
        ValuationMethod::Matrix,
        ValuationMethod::Comparables,
        ValuationMethod::DiscountedCashflow,
        ValuationMethod::Model,
        ValuationMethod::LastRound,
        ValuationMethod::Cost,
    ];
    for pair in ladder.windows(2) {
        assert!(
            pair[0].base_confidence() > pair[1].base_confidence(),
            "{} must be strictly more confident than {}",
            pair[0].label(),
            pair[1].label()
        );
    }
    Ok(())
}
