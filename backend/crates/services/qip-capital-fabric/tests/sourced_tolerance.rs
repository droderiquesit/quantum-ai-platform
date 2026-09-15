//! §38.3's interval rate when it comes from a figure somebody published
//! rather than from a number somebody chose.
//!
//! The module these exercise exists because the tolerance formula's rate arm
//! had no feed: every basis was declared at zero, so `dust + rate x |expected|`
//! reduced to the dust floor everywhere and the record said `rate: 0` without
//! saying why. The repair that must not happen is a constant — a tolerance
//! decides whether the books balance, and a fabricated rate there is a halt
//! that reads as configured while judging real books against an invention.
//!
//! So the properties here are the ones that make a sourced rate different from
//! a chosen one: it names its publisher, it carries both instants, it governs
//! exactly the asset its issuer sets it for, and it stops governing when its
//! publisher goes quiet.

// The workspace denies `panic_in_result_fn` for production code. In a test the
// assertion is the deliverable, and `?` keeps the fixtures readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital_fabric::tolerance::{
    PolicyRateTable, RateLookup, SourcedIntervalRate, ToleranceBasis, ToleranceClass,
};
use qip_capital_fabric::wallet::Asset;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp, dec};

/// The source id the licensing catalogue admitted, spelled the way the
/// connector's manifest spells it. A rate whose provenance named something
/// else would be a rate from a source nobody evaluated.
const ECB: &str = "ecb-key-interest-rates";

/// Tuesday 15 September 2026, midnight UTC — the reference date the recorded
/// ECB message stamps its levels with.
fn applied_on() -> Timestamp {
    Timestamp::from_civil(2026, 9, 15)
}

/// Sixteen hours later, the manifest's dissemination delay.
fn knowable() -> Timestamp {
    applied_on().saturating_add(Duration::from_hours(16))
}

fn euro() -> Result<Asset> {
    Asset::new("EUR")
}

/// The deposit facility rate the ECB's own data portal served on 2026-09-15:
/// 2.25 percent per annum.
fn deposit_facility_rate() -> Result<SourcedIntervalRate> {
    SourcedIntervalRate::from_percent_per_annum(
        ToleranceClass::FiatAtBrokerOrBank,
        euro()?,
        ECB,
        dec!("2.25"),
        applied_on(),
        knowable(),
    )
}

#[test]
fn a_published_deposit_rate_becomes_one_days_accrual_and_widens_only_by_that() -> Result<()> {
    // Premise: the basis this platform shipped for a fiat book had no accrual
    // at all, so the tolerance was the dust floor and nothing more. That is
    // the figure the sourced one has to beat, and it is asserted first so a
    // mutation that stopped the rate reaching the basis would leave two equal
    // numbers rather than a passing test.
    let floor = dec!("1");
    let expected = dec!("1000000");
    let dust_only =
        ToleranceBasis::dust_only(ToleranceClass::FiatAtBrokerOrBank, floor)?.evaluate(expected)?;
    assert_eq!(dust_only.tolerance, floor);
    assert!(!dust_only.accrual_applied());

    let rate = deposit_facility_rate()?;
    // 2.25 per cent per annum on an actual/360 day count is 0.0000625 a day —
    // the ECB's own convention for euro money-market interest, not a rounder
    // number this platform preferred.
    assert_eq!(rate.rate(), dec!("0.0000625"));
    assert_eq!(rate.published_percent_per_annum(), dec!("2.25"));
    assert_eq!(rate.source_id(), ECB);

    let sourced = ToleranceBasis::from_sourced(floor, &rate)?;
    let evaluated = sourced.evaluate(expected)?;
    // One day's interest on a million euros at 2.25 per cent, and the dust
    // floor beneath it: 62.50 + 1.
    assert_eq!(evaluated.accrual, dec!("62.5"));
    assert_eq!(evaluated.tolerance, dec!("63.5"));
    assert!(evaluated.accrual_applied());
    assert_eq!(evaluated.class, ToleranceClass::FiatAtBrokerOrBank);

    // And the sentence that travels with it names the publisher, both
    // instants and the division — which is what the ECB's terms require of a
    // modified figure, and what lets a reader re-derive the number rather than
    // trust it.
    let derivation = rate.derivation();
    for fragment in [
        "2.25",
        ECB,
        "2026-09-15T00:00:00.000Z",
        "2026-09-15T16:00:00.000Z",
        "actual/360",
        "0.0000625",
    ] {
        assert!(
            derivation.contains(fragment),
            "the derivation does not state {fragment}: {derivation}"
        );
    }
    Ok(())
}

#[test]
fn a_rate_knowable_before_it_was_true_is_refused_as_point_in_time_leakage() -> Result<()> {
    // Premise: the same figure with its instants the right way round builds,
    // so the refusal below is about the stamps and not about the figure.
    deposit_facility_rate()?;

    let refused = SourcedIntervalRate::from_percent_per_annum(
        ToleranceClass::FiatAtBrokerOrBank,
        euro()?,
        ECB,
        dec!("2.25"),
        knowable(),
        applied_on(),
    )
    .expect_err("a rate knowable before it was true was admitted");
    assert!(
        refused.message().contains("point-in-time leakage"),
        "the refusal is not about the stamps: {}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_per_annum_figure_is_refused_for_every_class_the_section_does_not_measure_in_days() -> Result<()>
{
    // The failure this prevents is the one this whole lane exists to avoid: a
    // real number applied to a class it does not govern, which is a
    // fabrication with a citation attached. §38.3 gives the fiat row "one
    // day's interest accrual" and gives the other accruing rows a funding
    // interval, a mark-to-market interval and a statement cadence; an annual
    // percentage becomes a day without a second assumption and becomes any of
    // those only with one.
    //
    // Premise: the fiat row does admit it, so the refusals below are about the
    // other rows and not about the figure.
    assert_eq!(
        deposit_facility_rate()?.class(),
        ToleranceClass::FiatAtBrokerOrBank
    );
    let others: Vec<ToleranceClass> = ToleranceClass::ALL
        .into_iter()
        .filter(|class| *class != ToleranceClass::FiatAtBrokerOrBank)
        .collect();
    assert_eq!(
        others.len(),
        ToleranceClass::ALL.len() - 1,
        "the class table has changed shape and this test is filtering nothing"
    );
    for class in others {
        let refused = SourcedIntervalRate::from_percent_per_annum(
            class,
            euro()?,
            ECB,
            dec!("2.25"),
            applied_on(),
            knowable(),
        )
        .err()
        .unwrap_or_else(|| {
            panic!("a {class} tolerance was given an interval rate derived from an annual figure")
        });
        // The refusal must name the interval the section gives that class, so
        // a reader goes to §38.3's table rather than to this file.
        assert!(
            refused.message().contains(class.interval()),
            "the refusal for {class} does not name the interval the section gives it: {}",
            refused.message()
        );
    }
    Ok(())
}

#[test]
fn a_euro_rate_never_answers_for_a_book_in_another_currency() -> Result<()> {
    // The ECB sets the euro area's rates and no other issuer's. A table that
    // answered for dollars because the *class* matched would put a euro
    // deposit rate behind a halt on a dollar book — real arithmetic on a
    // number that governs nothing there, which is worse than the missing rate
    // it replaced because it comes with a citation.
    let mut table = PolicyRateTable::new();
    table.record(deposit_facility_rate()?)?;
    // Premise: the rate is held and does govern the euro book at this instant,
    // so a `NoneHeld` below is about the currency and not an empty table.
    let euro_lookup = table.governing(&euro()?, ToleranceClass::FiatAtBrokerOrBank, knowable());
    assert!(
        matches!(euro_lookup, RateLookup::Governed(_)),
        "the euro book is not governed by a euro rate: {euro_lookup:?}"
    );

    let dollars = Asset::new("USD")?;
    let dollar_lookup = table.governing(&dollars, ToleranceClass::FiatAtBrokerOrBank, knowable());
    assert_eq!(dollar_lookup, RateLookup::NoneHeld);
    assert!(dollar_lookup.rate().is_none());
    // And the record says which currency is missing, so a reader of a halt is
    // not left to infer it from a zero.
    let said = dollar_lookup.describe(&dollars, knowable());
    assert!(
        said.contains("no source in this build publishes an interval rate for USD"),
        "the record does not name the missing currency: {said}"
    );
    Ok(())
}

#[test]
fn a_rate_whose_publisher_has_gone_quiet_stops_governing_and_the_dust_floor_returns() -> Result<()>
{
    // A control whose input has gone dark while the control keeps reporting is
    // the failure mode this repository already has a name for. A key rate
    // changes only at a scheduled meeting and the daily series carries a level
    // every calendar day, so a level a fortnight old does not mean the rate
    // has not moved — it means nobody is publishing.
    let mut table = PolicyRateTable::new();
    table.record(deposit_facility_rate()?)?;
    let asset = euro()?;

    // Premise: inside the window it governs, so the refusal below is about the
    // age and not about the table.
    let fresh = knowable().saturating_add(Duration::from_days(6));
    assert!(matches!(
        table.governing(&asset, ToleranceClass::FiatAtBrokerOrBank, fresh),
        RateLookup::Governed(_)
    ));

    let stale = knowable().saturating_add(Duration::from_days(8));
    let lookup = table.governing(&asset, ToleranceClass::FiatAtBrokerOrBank, stale);
    assert!(lookup.rate().is_none(), "a stale rate still governed");
    assert!(
        matches!(lookup, RateLookup::NotCurrent { .. }),
        "a stale rate was reported as something other than not current: {lookup:?}"
    );
    assert!(
        lookup
            .describe(&asset, stale)
            .contains("is not evidence about"),
        "the record does not say the rate stopped being evidence: {}",
        lookup.describe(&asset, stale)
    );

    // Before it was knowable is the same answer, for the opposite reason: a
    // rate read at midnight on its own date is a rate read before it existed.
    let early = applied_on().saturating_add(Duration::from_hours(1));
    assert!(
        table
            .governing(&asset, ToleranceClass::FiatAtBrokerOrBank, early)
            .rate()
            .is_none(),
        "a rate was used before it became knowable"
    );
    Ok(())
}

#[test]
fn a_superseded_rate_offered_after_a_later_one_is_refused_rather_than_taken() -> Result<()> {
    // A vendor serving history, or a replay, would otherwise move the
    // tolerance backwards onto a figure that has since been replaced — a
    // control judging today's book by yesterday's rate, with nothing in the
    // record saying it happened.
    let mut table = PolicyRateTable::new();
    table.record(deposit_facility_rate()?)?;
    // Premise: the table holds the later figure, so the refusal below is about
    // the offered one being older.
    assert_eq!(table.len(), 1);

    let older = SourcedIntervalRate::from_percent_per_annum(
        ToleranceClass::FiatAtBrokerOrBank,
        euro()?,
        ECB,
        dec!("3.75"),
        applied_on().saturating_sub(Duration::from_days(30)),
        knowable().saturating_sub(Duration::from_days(30)),
    )?;
    let refused = table
        .record(older)
        .expect_err("a superseded rate was taken over a later one");
    assert!(
        refused.message().contains("superseded"),
        "the refusal is not about the figure being superseded: {}",
        refused.message()
    );
    // And the table still holds the later one, unmoved.
    let held = table
        .governing(&euro()?, ToleranceClass::FiatAtBrokerOrBank, knowable())
        .rate()
        .map(SourcedIntervalRate::published_percent_per_annum);
    assert_eq!(held, Some(dec!("2.25")));
    Ok(())
}

#[test]
fn a_negative_policy_rate_accrues_on_its_magnitude_rather_than_being_refused() -> Result<()> {
    // The ECB's deposit facility sat at -0.50 from 2019 to 2022. A tolerance
    // allows for a movement of a given size whichever way it points, so one
    // day's accrual is taken on the magnitude; signing it would produce a
    // negative interval rate, which `ToleranceBasis::new` refuses because it
    // would pull the tolerance below the dust floor an operator set — and
    // three real years of published policy would then have no usable rate.
    let negative = SourcedIntervalRate::from_percent_per_annum(
        ToleranceClass::FiatAtBrokerOrBank,
        euro()?,
        ECB,
        dec!("-0.5"),
        applied_on(),
        knowable(),
    )?;
    // Premise: the figure kept its sign in the record, so what follows is a
    // derivation and not a value quietly corrected on the way in.
    assert_eq!(negative.published_percent_per_annum(), dec!("-0.5"));
    assert!(!negative.rate().is_negative());
    assert_eq!(
        negative.rate(),
        dec!("0.5")
            .checked_div(Decimal::from_int(100))
            .and_then(|fraction| fraction.checked_div(SourcedIntervalRate::DAY_COUNT_BASIS))
            .expect("a finite quotient")
    );
    ToleranceBasis::from_sourced(dec!("1"), &negative)?;
    Ok(())
}

#[test]
fn a_sourced_rate_faces_the_same_ceiling_a_declared_one_does() -> Result<()> {
    // A gate a sourced number skipped would be a gate that guards only the
    // numbers nobody worried about. A vendor serving a mis-scaled figure — a
    // rate as basis points where the series is percent, say — must meet
    // `MAX_INTERVAL_RATE` exactly as an operator's typo does.
    //
    // Premise: the ceiling is a tenth, so a figure that derives above a tenth
    // a day is what is needed, and 2.25 per cent a year is nowhere near it.
    assert_eq!(ToleranceBasis::MAX_INTERVAL_RATE, dec!("0.1"));
    ToleranceBasis::from_sourced(dec!("1"), &deposit_facility_rate()?)?;

    let mis_scaled = SourcedIntervalRate::from_percent_per_annum(
        ToleranceClass::FiatAtBrokerOrBank,
        euro()?,
        ECB,
        // Ten thousand per cent per annum: a percent figure served as basis
        // points would look like this, and one day of it is over a quarter of
        // the balance.
        dec!("10000"),
        applied_on(),
        knowable(),
    )?;
    assert!(mis_scaled.rate() > ToleranceBasis::MAX_INTERVAL_RATE);
    let refused = ToleranceBasis::from_sourced(dec!("1"), &mis_scaled)
        .expect_err("a mis-scaled published rate built a basis");
    assert!(
        refused.message().contains("above the ceiling of"),
        "the refusal is not the interval-rate ceiling: {}",
        refused.message()
    );
    Ok(())
}
