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

fn private_object_observed_at(
    symbol: &str,
    observed: Timestamp,
    details: PrivateAssetDetails,
) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(format!("obj-{symbol}")),
        symbol,
        InstrumentType::PrivateEquityFund,
    )
    .venue("OTC")
    .price(dec!("1"))
    .extension(Extension::PrivateAsset(details))
    .provenance(Provenance::new("administrator", observed, observed))
    .build(observed)
}

#[test]
fn a_mark_decays_with_the_age_of_the_record_and_not_with_the_instant_it_was_assembled() -> Result<()>
{
    // The defect this prevents, which shipped: `mark_object` struck every mark
    // as of the caller's instant, so the decay clock measured how long the
    // process had been up rather than how old the evidence was. A fastbrain up
    // 180 days sized a last-round mark at 0.20 and one restarted that morning
    // sized the byte-identical record at 0.40 — a sizing input that could not
    // be reproduced from the event log, and a restart that refreshed a report
    // from 2010. Both marks below are read at the same instant; only the
    // record's own observation instant differs.
    let read_at = day(4000);
    let fresh = private_object_observed_at(
        "FRESH",
        day(3999),
        details(dec!("1000"), dec!("800"), Decimal::ZERO),
    )?;
    let ancient = private_object_observed_at(
        "OLD",
        origin(),
        details(dec!("1000"), dec!("800"), Decimal::ZERO),
    )?;

    let fresh_mark = IlliquidValuator::mark_object(&fresh, origin(), read_at)?
        .expect("a private record with a residual value is markable");
    let ancient_mark = IlliquidValuator::mark_object(&ancient, origin(), read_at)?
        .expect("a private record with a residual value is markable");

    // Premise: the two marks are the same method at the same struck
    // confidence, so every difference below is age and nothing else.
    assert_eq!(fresh_mark.method(), ValuationMethod::LastRound);
    assert_eq!(ancient_mark.method(), ValuationMethod::LastRound);
    let base = ValuationMethod::LastRound.base_confidence();
    assert!((fresh_mark.struck_confidence() - base).abs() < 1e-12);
    assert!((ancient_mark.struck_confidence() - base).abs() < 1e-12);

    assert_eq!(
        fresh_mark.as_of(),
        day(3999),
        "a mark is struck as of the instant its evidence was observed"
    );
    assert_eq!(
        ancient_mark.as_of(),
        origin(),
        "a ten-year-old report is struck as of ten years ago, not as of the sweep"
    );

    let fresh_confidence = fresh_mark.confidence_at(read_at)?;
    let ancient_confidence = ancient_mark.confidence_at(read_at)?;
    assert!(
        fresh_confidence > 0.39,
        "a report a day old keeps almost all of its weight, was {fresh_confidence}"
    );
    assert!(
        ancient_confidence < 0.001,
        "a report twenty-two half-lives old must be worth almost nothing, was \
         {ancient_confidence}"
    );

    assert!(
        !fresh_mark.is_stale(read_at),
        "a report a day old is inside its review interval"
    );
    assert!(
        ancient_mark.is_stale(read_at),
        "a report ten years past its annual review is stale, and the staleness control exists to \
         say so"
    );
    Ok(())
}

#[test]
fn a_record_whose_evidence_postdates_the_platforms_copy_of_it_is_refused_rather_than_dated()
-> Result<()> {
    // Two instants that disagree about when a fact existed leave no honest
    // origin for the decay clock, and picking the later would let a vendor
    // field make an old report size as though it were current.
    let observed = day(10);
    let object = FinancialObject::builder(
        ObjectId::from_string("obj-AHEAD"),
        "AHEAD",
        InstrumentType::PrivateEquityFund,
    )
    .venue("OTC")
    .price(dec!("1"))
    .extension(Extension::PrivateAsset(details(
        dec!("1000"),
        dec!("800"),
        Decimal::ZERO,
    )))
    .provenance(Provenance::new("administrator", observed, observed))
    .build(day(5))?;

    // Premise: the same record with the two stamps in order is marked, so the
    // refusal is about their order and not about the record.
    let ordered = private_object_observed_at(
        "ORDERED",
        observed,
        details(dec!("1000"), dec!("800"), Decimal::ZERO),
    )?;
    assert!(
        IlliquidValuator::mark_object(&ordered, origin(), day(20))?.is_some(),
        "a record observed at or before the platform recorded it is markable"
    );

    let refusal = IlliquidValuator::mark_object(&object, origin(), day(20))
        .expect_err("evidence dated after the platform's own copy has no honest observation date");
    let message = refusal.message();
    assert!(
        message.contains("recorded before it happened"),
        "the refusal must name the contradiction, said: {message}"
    );
    assert!(
        message.contains("will not pick one of the two instants for you"),
        "the refusal must say the plane does not choose between them, said: {message}"
    );
    Ok(())
}

#[test]
fn a_mark_cannot_be_read_before_the_record_it_rests_on_was_knowable() -> Result<()> {
    // Bitemporality: `known_at <= as_of`. A mark readable before it was
    // knowable is a point-in-time leak, and a backtest resting on one is
    // better than reality by exactly the information it should not have had.
    let object = private_object_observed_at(
        "LATER",
        day(100),
        details(dec!("1000"), dec!("800"), Decimal::ZERO),
    )?;
    // Premise: read at the knowability instant itself, the mark exists.
    assert!(
        IlliquidValuator::mark_object(&object, origin(), day(100))?.is_some(),
        "a record is markable as of the instant it became knowable"
    );

    let refusal = IlliquidValuator::mark_object(&object, origin(), day(99))
        .expect_err("a record not yet knowable must not be marked");
    assert!(
        refusal.message().contains("point-in-time leak"),
        "the refusal must name the leak, said: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_vintage_year_outside_the_representable_range_is_refused_rather_than_overflowing() -> Result<()>
{
    // `Timestamp` is an i64 nanosecond count and tops out near 2262.
    // `Timestamp::from_civil(2300, 1, 1)` multiplies unchecked, so a catalogue
    // record stating 2300 — or a typed 20204 — aborted the debug build inside
    // `Platform::new` and wrapped to a negative instant in the release build,
    // where it became the commitment origin and the discounting origin.
    let mut sound = details(dec!("1000"), dec!("800"), Decimal::ZERO);
    sound.vintage_year = 2262;
    // Premise: the last representable vintage is admitted, so the range check
    // is a range check and not a blanket refusal.
    assert_eq!(
        sound.clone().checked()?.vintage_origin()?.to_date_string(),
        "2262-01-01"
    );

    for year in [2263_u32, 2300, 20204, u32::MAX] {
        let mut broken = details(dec!("1000"), dec!("800"), Decimal::ZERO);
        broken.vintage_year = year;
        let refusal = broken
            .checked()
            .expect_err("a vintage year that is not an instant must be refused");
        assert!(
            refusal
                .message()
                .contains(&format!("vintage year of {year}")),
            "the refusal must name the value it read, said: {}",
            refusal.message()
        );
        assert!(
            refusal.message().contains("between 1678 and 2262"),
            "the refusal must say what to supply instead, said: {}",
            refusal.message()
        );
    }
    Ok(())
}

#[test]
fn a_catalogue_file_cannot_smuggle_an_unrepresentable_vintage_year_past_deserialisation()
-> Result<()> {
    // The record arrives as vendor data, so the constructor is only half the
    // guard; `serde(try_from)` is the other half.
    let wire = |year: u32| {
        format!(
            r#"{{"vintage_year":{year},"committed_capital":"1000","called_capital":"800",
                 "distributed_capital":"0","residual_value":"1000","stage":"buyout",
                 "lockup_years":7.0,"capital_call_notice_days":10}}"#
        )
    };
    // Premise: a sound record deserialises, so the failure below is the year.
    let sound: PrivateAssetDetails = serde_json::from_str(&wire(2020))
        .map_err(|e| qip_core::error::Error::invalid(e.to_string()))?;
    assert_eq!(sound.vintage_year, 2020);

    let refused = serde_json::from_str::<PrivateAssetDetails>(&wire(2300))
        .expect_err("a file stating an unrepresentable vintage must be refused at load");
    assert!(
        refused.to_string().contains("vintage year of 2300"),
        "the refusal must survive into the deserialisation error, said: {refused}"
    );
    Ok(())
}

#[test]
fn a_lockup_that_is_not_a_finite_term_is_refused_rather_than_saturating() -> Result<()> {
    // `(lockup_years * 365.0) as i64` turns NaN into a lockup of zero and
    // infinity into `i64::MAX` days, which `Duration::from_days` then
    // multiplied unchecked. Neither number was on any record.
    let mut sound = details(dec!("1000"), dec!("800"), Decimal::ZERO);
    sound.lockup_years = 7.0;
    // Premise: an ordinary term is admitted and is seven years of days.
    assert_eq!(sound.lockup()?, Duration::from_days(2555));

    for term in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0, 1.0e18] {
        let mut broken = details(dec!("1000"), dec!("800"), Decimal::ZERO);
        broken.lockup_years = term;
        let refusal = broken
            .checked()
            .expect_err("a lockup that is not a finite non-negative term must be refused");
        assert!(
            refusal
                .message()
                .contains("supply a finite term between 0 and 100"),
            "the refusal must say what a term may be, said: {}",
            refusal.message()
        );
    }
    Ok(())
}
