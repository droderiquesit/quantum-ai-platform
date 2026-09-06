//! The valuation plane as the kernel actually reads it.
//!
//! Two engines meet the cycle here, and both are on the sizing path rather
//! than beside it:
//!
//! * An unfunded private commitment is a hard reservation. `construct_from`
//!   sizes against [`Platform::deployable_capital`], which is free capital
//!   less every unfunded commitment the universe carries. Before this
//!   existed, a book holding a nine-figure undrawn commitment deployed as
//!   though the commitment were somebody else's problem, and the first
//!   capital call would have forfeited the position.
//! * A private asset the valuation plane cannot mark may not be sized into.
//!   `construct_from` reads [`Platform::sizing_confidence`] for every thesis,
//!   which refuses an unmarkable instrument and narrows a marked one by the
//!   mark's decayed confidence. The failure this prevents is the one the
//!   valuation plane exists to prevent: a number nobody observed, believed
//!   downstream because it arrived in the shape of a price.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion aborting a `Result`-returning function is a bug. In a test the
// assertion is the deliverable and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, Duration, ObjectId, dec};
use qip_financial::asset_class::InstrumentType;
use qip_financial::extensions::{Extension, PrivateAssetDetails};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_financial::valuation::ValuationMethod;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 3, 1)
}

fn limits() -> LimitSet {
    LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn details(
    committed: Decimal,
    called: Decimal,
    distributed: Decimal,
    residual: Decimal,
) -> PrivateAssetDetails {
    PrivateAssetDetails {
        vintage_year: 2024,
        committed_capital: committed,
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
    .geography("US")
    .price(dec!("1"))
    .extension(Extension::PrivateAsset(details))
    .provenance(Provenance::new("administrator", start(), start()))
    .build(start())
}

fn equity(symbol: &str) -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string(format!("obj-{symbol}")),
        symbol,
        InstrumentType::CommonStock,
    )
    .venue("XNYS")
    .geography("US")
    .price(dec!("100"))
    .provenance(Provenance::new("vendor", start(), start()))
    .build(start())
}

fn platform_over(universe: Universe, equity_capital: Decimal) -> Result<Platform> {
    let config = PlatformConfig {
        initial_equity: equity_capital,
        ..PlatformConfig::default()
    };
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits())
}

#[test]
fn an_undrawn_commitment_is_taken_off_the_capital_the_platform_will_deploy() -> Result<()> {
    // Premise: an identical platform holding only the listed equity deploys
    // its whole equity, so the reduction below is the commitment and not some
    // other hold.
    let mut listed = Universe::new();
    listed.insert(equity("AAA")?)?;
    let mut bare = platform_over(listed, dec!("1000000"))?;
    // One cycle, so the two halves of this test are compared over books that
    // have had the same thing happen to them. It is no longer what makes the
    // number readable: `deployable_capital` anchors the reservation ledger
    // itself now, and before it did, a caller who read the budget first got
    // zero from a book of a million.
    let _ = bare.run_cycle(start());
    let unencumbered = bare.deployable_capital(start())?;
    assert_eq!(
        unencumbered,
        dec!("1000000"),
        "a book with no commitments deploys its free capital in full"
    );

    let mut committed = Universe::new();
    committed.insert(equity("AAA")?)?;
    committed.insert(private_object(
        "FUND",
        details(
            dec!("400000"),
            dec!("150000"),
            Decimal::ZERO,
            dec!("160000"),
        ),
    )?)?;
    let mut encumbered = platform_over(committed, dec!("1000000"))?;
    let _ = encumbered.run_cycle(start());
    assert_eq!(
        encumbered.commitments().len(),
        1,
        "the universe's one private record must reach the commitment book"
    );
    assert_eq!(
        encumbered.commitments().unfunded_total(start())?,
        dec!("250000"),
        "400,000 committed against 150,000 called leaves 250,000 undrawn"
    );
    assert_eq!(
        encumbered.deployable_capital(start())?,
        dec!("750000"),
        "the undrawn commitment must come off what the platform will deploy"
    );
    Ok(())
}

#[test]
fn a_commitment_larger_than_the_free_capital_stops_sizing_rather_than_sizing_at_zero() -> Result<()>
{
    let stretched_universe = || -> Result<Universe> {
        let mut universe = Universe::new();
        universe.insert(equity("AAA")?)?;
        universe.insert(private_object(
            "FUND",
            details(dec!("900000"), Decimal::ZERO, Decimal::ZERO, dec!("10000")),
        )?)?;
        Ok(universe)
    };
    // Premise: with a million of equity the same commitment leaves capital
    // free, so the refusal below is about the ratio and not about the
    // commitment existing at all.
    let mut solvent = platform_over(stretched_universe()?, dec!("1000000"))?;
    let _ = solvent.run_cycle(start());
    assert_eq!(solvent.deployable_capital(start())?, dec!("100000"));

    let mut stretched = platform_over(stretched_universe()?, dec!("500000"))?;
    let _ = stretched.run_cycle(start());
    let refusal = stretched
        .deployable_capital(start())
        .expect_err("obligations above free capital must stop the construction");
    let message = refusal.message();
    assert!(
        message.contains("meet or exceed the free capital"),
        "the refusal must name the comparison, said: {message}"
    );
    assert!(
        message.contains("forfeits the position"),
        "the refusal must name the consequence of a missed call, said: {message}"
    );
    Ok(())
}

#[test]
fn a_private_asset_with_nothing_observable_cannot_be_sized_and_is_named_at_assembly() -> Result<()>
{
    let mut universe = Universe::new();
    universe.insert(equity("AAA")?)?;
    // Fully called, fully distributed, no residual: the administrator's
    // record holds nothing anybody observed a value from.
    universe.insert(private_object(
        "BARE",
        details(
            dec!("100000"),
            dec!("100000"),
            dec!("100000"),
            Decimal::ZERO,
        ),
    )?)?;
    let platform = platform_over(universe, dec!("1000000"))?;

    // Premise: the listed equity is markable and sizes at full confidence, so
    // the refusal below is about the private record and not a blanket stop.
    assert_eq!(
        platform.sizing_confidence("obj-AAA", start())?,
        Decimal::ONE,
        "a listed equity needs no haircut from the valuation plane"
    );

    let refusal = platform
        .sizing_confidence("obj-BARE", start())
        .expect_err("an unmarkable private asset must not be sized");
    let message = refusal.message();
    assert!(
        message.contains("holds no defensible mark"),
        "the refusal must say there is no mark, said: {message}"
    );
    assert!(
        message.contains("refuses to invent a mark"),
        "the refusal must carry the valuation plane's own reason, said: {message}"
    );

    assert_eq!(
        platform.illiquid_unmarkable().len(),
        1,
        "exactly the one barren record is unmarkable"
    );
    let named: Vec<&String> = platform
        .universe_not_decision_grade()
        .iter()
        .filter(|(id, reason)| id == "obj-BARE" && reason.starts_with("no defensible mark"))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        named.len(),
        1,
        "the unmarkable instrument must be named at assembly, not after its first trade"
    );
    Ok(())
}

#[test]
fn an_uncertain_mark_narrows_sizing_and_a_confident_one_does_not() -> Result<()> {
    let mut universe = Universe::new();
    // A residual the administrator reported: a last-round mark.
    universe.insert(private_object(
        "REPORTED",
        details(
            dec!("400000"),
            dec!("400000"),
            Decimal::ZERO,
            dec!("500000"),
        ),
    )?)?;
    // No residual, capital called and not returned: a cost mark, the lowest
    // confidence in the §16.3 table.
    universe.insert(private_object(
        "ATCOST",
        details(
            dec!("400000"),
            dec!("400000"),
            dec!("100000"),
            Decimal::ZERO,
        ),
    )?)?;
    let platform = platform_over(universe, dec!("1000000"))?;

    let reported = platform
        .illiquid_mark("obj-REPORTED")
        .expect("a reported residual is markable");
    let at_cost = platform
        .illiquid_mark("obj-ATCOST")
        .expect("called capital not yet returned is markable at cost");
    // Premise: the two records are marked by different methods, so the
    // narrowing below is the §16.3 table and not one shared constant.
    assert_eq!(reported.method(), ValuationMethod::LastRound);
    assert_eq!(at_cost.method(), ValuationMethod::Cost);

    let reported_narrowing = platform.sizing_confidence("obj-REPORTED", start())?;
    let cost_narrowing = platform.sizing_confidence("obj-ATCOST", start())?;
    assert!(
        reported_narrowing.is_positive() && reported_narrowing < Decimal::ONE,
        "a private mark must narrow sizing rather than pass it through: {reported_narrowing}"
    );
    assert!(
        cost_narrowing < reported_narrowing,
        "a cost mark must narrow harder than a reported round: {cost_narrowing} against \
         {reported_narrowing}"
    );
    Ok(())
}

#[test]
fn a_mark_past_its_review_date_stops_sizing_rather_than_decaying_forever() -> Result<()> {
    let mut universe = Universe::new();
    universe.insert(private_object(
        "REPORTED",
        details(
            dec!("400000"),
            dec!("400000"),
            Decimal::ZERO,
            dec!("500000"),
        ),
    )?)?;
    let platform = platform_over(universe, dec!("1000000"))?;
    let mark = platform
        .illiquid_mark("obj-REPORTED")
        .expect("a reported residual is markable");
    let review = mark.next_review();
    // Premise: on the review date the mark still sizes, so the refusal a
    // moment later is the boundary and not a mark that never worked.
    assert!(
        platform
            .sizing_confidence("obj-REPORTED", review)?
            .is_positive()
    );

    let refusal = platform
        .sizing_confidence(
            "obj-REPORTED",
            review.saturating_add(Duration::from_days(1)),
        )
        .expect_err("a mark past its review date must not be sized against");
    assert!(
        refusal
            .message()
            .contains("refresh the mark before sizing into it"),
        "the refusal must say what to do instead, said: {}",
        refusal.message()
    );
    Ok(())
}

/// A private object whose administrator record was last updated at `updated`,
/// with the vintage year overridden.
fn private_object_at(
    symbol: &str,
    updated: Timestamp,
    mut record: PrivateAssetDetails,
    vintage_year: u32,
) -> Result<FinancialObject> {
    record.vintage_year = vintage_year;
    FinancialObject::builder(
        ObjectId::from_string(format!("obj-{symbol}")),
        symbol,
        InstrumentType::PrivateEquityFund,
    )
    .venue("OTC")
    .geography("US")
    .price(dec!("1"))
    .extension(Extension::PrivateAsset(record))
    .provenance(Provenance::new("administrator", updated, updated))
    .build(updated)
}

#[test]
fn a_vintage_year_that_is_not_an_instant_stops_assembly_instead_of_the_process() -> Result<()> {
    // `private_asset_origin` read `Timestamp::from_civil(vintage_year as i32,
    // 1, 1)`, which multiplies unchecked. A catalogue record stating 2300 — or
    // a typed 20204 — aborted `Platform::new` on an arithmetic overflow in a
    // debug build, and in a release build wrapped to a negative instant that
    // became the commitment origin, the discounting origin and the knowability
    // stamp. It is deserialised vendor data reaching a `Result`-returning
    // function in a workspace that denies `panic_in_result_fn`.
    let sound = details(
        dec!("400000"),
        dec!("150000"),
        Decimal::ZERO,
        dec!("160000"),
    );
    // Premise: the identical record with a representable vintage assembles, so
    // the refusal below is the year and nothing else about the record.
    let mut admitted = Universe::new();
    admitted.insert(private_object_at("FUND", start(), sound.clone(), 2024)?)?;
    assert!(
        platform_over(admitted, dec!("1000000"))?
            .illiquid_mark("obj-FUND")
            .is_some(),
        "a record with a representable vintage year is assembled and marked"
    );

    let mut broken = Universe::new();
    broken.insert(private_object_at("FUND", start(), sound, 20204)?)?;
    let refusal = platform_over(broken, dec!("1000000"))
        .expect_err("a vintage year that is not an instant must stop assembly");
    assert!(
        refusal.message().contains("vintage year of 20204"),
        "the refusal must name the value it read, said: {}",
        refusal.message()
    );
    assert!(
        refusal.message().contains("between 1678 and 2262"),
        "the refusal must say what to supply instead, said: {}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn a_record_updated_before_its_own_vintage_is_refused_rather_than_quietly_repaired() -> Result<()> {
    // `private_holdings_of` passed `object.updated_at.max(origin)` as the
    // commitment's knowability stamp, which disarmed
    // `Commitment::unscheduled`'s refusal of `known_at < origin` — "a
    // commitment cannot be known before it was made" — for exactly the record
    // that check exists to catch. A vintage of 2024 with an administrator
    // update stamped in 2023 is a plausible typing error, and the repair
    // booked it as a real obligation dated from a year nobody entered.
    let sound = details(
        dec!("400000"),
        dec!("150000"),
        Decimal::ZERO,
        dec!("160000"),
    );
    // Premise: the same record updated after its vintage assembles, so the
    // refusal below is the ordering of the two stamps.
    let mut ordered = Universe::new();
    ordered.insert(private_object_at("FUND", start(), sound.clone(), 2024)?)?;
    assert!(platform_over(ordered, dec!("1000000")).is_ok());

    let before_vintage = Timestamp::from_civil(2023, 11, 15);
    let mut inverted = Universe::new();
    inverted.insert(private_object_at("FUND", before_vintage, sound, 2024)?)?;
    let refusal = platform_over(inverted, dec!("1000000"))
        .expect_err("a commitment known before it was made must be refused, not repaired");
    assert!(
        refusal
            .message()
            .contains("a commitment cannot be known before it was made"),
        "the engine's own refusal must reach the operator, said: {}",
        refusal.message()
    );
    Ok(())
}
