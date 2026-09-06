//! The valuation plane's term structure and credit engine, as a running
//! platform actually composes them.
//!
//! Both engines were previously unreachable from any deployed process:
//! `qip_market::curve::TermStructure` was built and called by nothing, and
//! credit was two `f64` fields on a risk-profile struct. These tests assert
//! the seam that closed that — the register the kernel derives at assembly and
//! the UNDERSTAND stage that reports it — rather than re-asserting the engines'
//! own arithmetic, which their crates' own suites cover.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ObjectId, dec};
use qip_financial::asset_class::InstrumentType;
use qip_financial::credit::CovenantState;
use qip_financial::extensions::{
    BondDetails, CouponFrequency, CreditRating, DayCount, Extension, LoanDetails, Seniority,
};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{LicensingClass, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn licensed() -> Provenance {
    Provenance::new("vendor", start(), start()).with_licensing(LicensingClass::Licensed)
}

fn limits() -> LimitSet {
    LimitSet::new("valuation-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

/// A sovereign issue at `tenor_years`, quoting `yield_to_maturity`. These are
/// what the register's discount curve is built from.
fn govvie(symbol: &str, tenor_years: i64, yield_to_maturity: f64) -> Result<FinancialObject> {
    let maturity = start().saturating_add(Duration::from_days(tenor_years * 365 + tenor_years / 4));
    FinancialObject::builder(
        ObjectId::from_string(format!("obj-{symbol}")),
        symbol,
        InstrumentType::GovernmentBond,
        fixture_liquidity(),
    )
    .venue("OTC")
    .price(dec!("100"))
    .provenance(licensed())
    .extension(Extension::Bond(BondDetails {
        issuer: "Sovereign".into(),
        coupon_rate: yield_to_maturity,
        coupon_frequency: CouponFrequency::SemiAnnual,
        maturity,
        issue_date: start(),
        face_value: dec!("100"),
        day_count: DayCount::ActualActual,
        seniority: Seniority::SeniorUnsecured,
        credit_rating: Some(CreditRating::Aaa),
        yield_to_maturity,
        modified_duration: tenor_years as f64,
        convexity: 0.0,
        option_adjusted_spread_bps: 0.0,
        callable: false,
        puttable: false,
        inflation_index: None,
    }))
    .build(start())
}

/// A leveraged loan at 9.4 turns, with or without the ceiling its credit
/// agreement sets.
///
/// The parameter is the whole point of the pair of tests below. With
/// `Some(6.0)` the borrower has broken a term somebody agreed to; with `None`
/// nobody captured the agreement and the platform is testing against an
/// assumption of its own. The two must not report identically, and until this
/// argument existed they did.
fn leveraged_loan(leverage_covenant: Option<f64>) -> Result<FinancialObject> {
    let mut object = FinancialObject::builder(
        ObjectId::from_string("obj-LOAN"),
        "LOAN",
        InstrumentType::Loan,
        fixture_liquidity(),
    )
    .venue("OTC")
    .price(dec!("98"))
    .provenance(licensed())
    .extension(Extension::Loan(LoanDetails {
        borrower: "Overlevered Ltd".into(),
        maturity: start().saturating_add(Duration::from_days(1826)),
        commitment: dec!("50000000"),
        drawn: dec!("50000000"),
        spread_bps: 525.0,
        benchmark: "SOFR".into(),
        seniority: Seniority::SecuredFirstLien,
        modified_duration: 0.25,
        covenant_lite: false,
        leverage_covenant,
        net_debt_to_ebitda: 9.4,
        is_amortising: false,
    }))
    .build(start())?;
    // The loan's own default estimate. Without one the engine refuses to
    // profile the borrower at all, which the second test relies on.
    object.risk.default_probability = 0.08;
    object.risk.recovery_rate = 0.6;
    Ok(object)
}

/// The loan as its credit agreement states it: a six-turn ceiling, breached at
/// 9.4.
fn breached_loan() -> Result<FinancialObject> {
    leveraged_loan(Some(6.0))
}

fn platform_over(universe: Universe) -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits())
}

#[test]
fn a_universe_of_credit_claims_is_valued_against_a_curve_built_from_its_own_sovereigns()
-> Result<()> {
    // The seam this proves: the kernel constructs a `TermStructure` from the
    // universe's sovereign issues and a `CreditProfile` from every credit
    // claim in it, and discounts one with the other. Before this existed no
    // deployed process constructed either type — the curve had monotone-cubic
    // interpolation, forward rates and shock scenarios, and nothing anywhere
    // called them.
    let mut universe = Universe::new();
    universe.insert(govvie("UST2", 2, 0.0450)?)?;
    universe.insert(govvie("UST10", 10, 0.0435)?)?;
    universe.insert(breached_loan()?)?;
    // Premise: the fixture holds sovereigns at two distinct tenors and one
    // non-sovereign credit claim. A universe of only govvies would make the
    // register below trivially true.
    assert_eq!(universe.len(), 3);

    let platform = platform_over(universe)?;
    let register = platform.credit_register();

    let curve = register
        .curve(qip_core::Currency::USD)
        .expect("the sovereign issues did not produce a USD curve");
    assert_eq!(
        curve.points().len(),
        2,
        "the curve was not built from both sovereign issues"
    );
    // 2s10s inverted, exactly as the fixture quotes it. This is the curve
    // reading the macro path has always wanted and never had a producer for.
    assert!(
        curve.is_inverted(),
        "the curve does not read the fixture's inversion"
    );

    // The composition: the credit engine produces the loss in money, the term
    // structure discounts it. Both are exact decimal arithmetic.
    let years = 5.0;
    let discounted = register.discounted_expected_loss(
        "obj-LOAN",
        qip_core::Currency::USD,
        dec!("50000000"),
        years,
    )?;
    assert!(
        discounted.is_positive(),
        "a defaultable loan carries no discounted expected loss"
    );
    // Undiscounted, from the profile alone, must be strictly larger — if the
    // curve were not applied the two would be equal, which is precisely the
    // "computed and ignored" failure this wiring exists to avoid.
    let (_, profile) = register
        .profiles()
        .find(|(id, _)| id.as_str() == "obj-LOAN")
        .expect("the loan is not in the register");
    let undiscounted = profile.expected_loss(dec!("50000000"), years)?;
    assert!(
        undiscounted > discounted,
        "the curve was not applied: {undiscounted} against {discounted}"
    );

    // A currency with no sovereign issue is refused rather than discounted at
    // an invented flat rate.
    assert!(
        register
            .discounted_expected_loss("obj-LOAN", qip_core::Currency::JPY, dec!("1"), 1.0)
            .is_err(),
        "an expected loss was discounted on a curve that does not exist"
    );
    Ok(())
}

#[test]
fn a_breached_covenant_is_raised_as_a_problem_by_the_stage_that_reports_the_register() -> Result<()>
{
    // A register that computed a covenant state and told nobody would be the
    // control-that-reads-as-protection shape this platform already records one
    // example of (`MaxExpectedShortfall`, which shipped in every default limit
    // set and could never fire). The property: a borrower through its leverage
    // ceiling reaches the cycle report.
    //
    // The fixture now states the ceiling its credit agreement sets. It did
    // not, and this test passed anyway, because every non-covenant-lite loan
    // was given a six-turn ceiling manufactured for it — so what the test
    // proved was that a heuristic could raise a breach, which is the defect
    // rather than the property. Its companion below covers the loan whose
    // agreement nobody captured.
    let mut universe = Universe::new();
    universe.insert(govvie("UST5", 5, 0.0420)?)?;
    universe.insert(breached_loan()?)?;

    let mut platform = platform_over(universe)?;
    let register = platform.credit_register();
    // Premise, asserted before the cycle: the engine reads the fixture as
    // breached. A test that only checked the report would pass on a register
    // that never looked at a covenant, because the report would be empty
    // either way.
    let (_, profile) = register
        .profiles()
        .find(|(id, _)| id.as_str() == "obj-LOAN")
        .expect("the loan is not in the register");
    assert_eq!(
        profile.covenant_state(),
        Some(CovenantState::Breached),
        "the fixture's leverage is not read as a breach"
    );
    assert_eq!(register.breaches().len(), 1);

    let report = platform.run_cycle(start());
    let understand = report
        .stages
        .iter()
        .find(|stage| stage.stage == Stage::Understand)
        .expect("the cycle ran no UNDERSTAND stage");
    assert!(understand.ran, "the UNDERSTAND stage did not run");
    assert!(
        understand.problems.iter().any(|problem| problem
            .starts_with("covenant breached: obj-LOAN (Overlevered Ltd):")
            && problem.contains("(agreement ceiling 6)")),
        "the breach did not reach the cycle report as a breach of an agreed term: {:?}",
        understand.problems
    );
    // And the detail says the register exists at all, so an operator reading
    // one line knows the plane ran.
    assert!(
        understand.detail.contains("credit register holds"),
        "the UNDERSTAND detail does not report the register: {}",
        understand.detail
    );
    Ok(())
}

#[test]
fn a_borrower_past_a_level_this_platform_assumed_is_not_reported_as_a_covenant_breach() -> Result<()>
{
    // The failure this prevents, and it was live: the same loan, with nobody
    // having captured its credit agreement, reached the operator as
    // "covenant breached: obj-LOAN (Overlevered Ltd): net_debt_to_ebitda
    // (ceiling 6) observed at 9.4: breached" — a sentence that cannot be told
    // apart from a breach of a term the agreement actually contains. The
    // ceiling was manufactured by `LOAN_LEVERAGE_COVENANT` for every
    // non-covenant-lite loan in the universe.
    let mut universe = Universe::new();
    universe.insert(govvie("UST5", 5, 0.0420)?)?;
    universe.insert(leveraged_loan(None)?)?;

    let mut platform = platform_over(universe)?;
    let register = platform.credit_register();

    // Premise: the register profiled the borrower and did read its leverage,
    // so an assertion about the sentence is about the sentence and not about a
    // claim that went missing.
    let (_, profile) = register
        .profiles()
        .find(|(id, _)| id.as_str() == "obj-LOAN")
        .expect("the loan is not in the register");
    assert_eq!(
        profile.covenants().count(),
        1,
        "the borrower's leverage stopped being tested at all, which trades a \
         mislabelled control for a missing one"
    );
    assert_eq!(
        profile.covenant_state(),
        None,
        "an obligor whose only test is this platform's own assumption reported a \
         covenant state, which is the reading `covenant_state` returns an Option to prevent"
    );
    assert!(
        register.breaches().is_empty(),
        "a level nobody agreed to was reported as a breach: {:?}",
        register.breaches()
    );
    assert_eq!(
        register.leverage_above_assumed_levels().len(),
        1,
        "the borrower's nine turns of leverage were dropped rather than reported"
    );

    let report = platform.run_cycle(start());
    let understand = report
        .stages
        .iter()
        .find(|stage| stage.stage == Stage::Understand)
        .expect("the cycle ran no UNDERSTAND stage");
    assert!(understand.ran, "the UNDERSTAND stage did not run");

    // The operator-visible distinction, on the rendered sentence. Matched with
    // `starts_with` on the delimited leading clause rather than on a substring:
    // "breach" appears in both sentences, and the assumed one says so on
    // purpose.
    assert!(
        !understand
            .problems
            .iter()
            .any(|problem| problem.starts_with("covenant breached:")),
        "an assumed level was escalated as a covenant breach: {:?}",
        understand.problems
    );
    let finding = understand
        .problems
        .iter()
        .find(|problem| problem.starts_with("leverage above an assumed level:"))
        .unwrap_or_else(|| panic!("the leverage finding is missing: {:?}", understand.problems));
    assert!(
        finding.contains("no agreement level supplied"),
        "the finding does not say the agreement was never captured: {finding}"
    );
    assert!(
        finding.contains("not a covenant breach"),
        "the finding does not say what it is not: {finding}"
    );

    // And the same loan with its agreement's own ceiling reports the other
    // way, so this is a distinction the register draws rather than a channel
    // it always uses. Without this half the test would pass on a platform that
    // had simply stopped reporting breaches.
    let mut agreed = Universe::new();
    agreed.insert(govvie("UST5", 5, 0.0420)?)?;
    agreed.insert(breached_loan()?)?;
    let agreed = platform_over(agreed)?;
    assert_eq!(
        agreed.credit_register().breaches().len(),
        1,
        "an agreed ceiling stopped being reported as a breach"
    );
    assert!(
        agreed
            .credit_register()
            .leverage_above_assumed_levels()
            .is_empty(),
        "a contractual breach was filed as an assumption being exceeded"
    );
    Ok(())
}

#[test]
fn a_sovereign_yield_quoted_in_percent_is_refused_rather_than_valuing_every_claim_at_zero()
-> Result<()> {
    // The failure this prevents, and it was live: a vendor quoting
    // `yield_to_maturity` in percent — 4.35 rather than 0.0435 — built a
    // perfectly valid curve, and `exp(-4.35 * 10)` rounded to `Decimal::ZERO`
    // at the nine decimal places money is held at. Every
    // `discounted_expected_loss` returned exactly 0, and the UNDERSTAND stage
    // printed "worst claim obj-UST2 at 0.000001489 of discounted expected loss
    // per unit" — a universe rendered as carrying almost no credit risk, with
    // the ranking decided by whose arithmetic collapsed last rather than by
    // whose credit is worst.
    let mut percent = Universe::new();
    percent.insert(govvie("UST2", 2, 4.50)?)?;
    percent.insert(govvie("UST10", 10, 4.35)?)?;
    percent.insert(breached_loan()?)?;
    let mut platform = platform_over(percent)?;
    let register = platform.credit_register();

    // Premise: the claims are in the register, so what follows is about the
    // curve and not about a universe the platform never read.
    assert_eq!(
        register.profiles().count(),
        3,
        "the fixture's credit claims are not in the register"
    );
    assert!(
        register.curve(qip_core::Currency::USD).is_none(),
        "a curve was built through a yield quoted in percent"
    );
    assert!(
        register.worst_claim().is_none(),
        "a worst claim was reported off a curve nothing could discount on: {:?}",
        register.worst_claim()
    );
    assert!(
        !register.summary().contains("worst claim"),
        "the summary still names a worst claim: {}",
        register.summary()
    );

    // Both benchmarks are named as excluded, and each says what to do. A point
    // that silently vanished would take the curve's shape with it while the
    // remaining points still fit a curve that answers every query.
    let excluded: Vec<_> = register.excluded_curve_points().collect();
    assert_eq!(excluded.len(), 2, "the excluded benchmarks were not named");
    assert!(
        excluded
            .iter()
            .all(|(_, reason)| reason.contains("as a fraction rather than a percentage")),
        "the refusal does not say what to do instead: {excluded:?}"
    );

    let report = platform.run_cycle(start());
    let understand = report
        .stages
        .iter()
        .find(|stage| stage.stage == Stage::Understand)
        .expect("the cycle ran no UNDERSTAND stage");
    assert!(
        understand
            .problems
            .iter()
            .any(|problem| problem.starts_with("sovereign issue obj-UST2 is not on")),
        "the unit error did not reach the cycle report: {:?}",
        understand.problems
    );
    assert!(
        understand
            .problems
            .iter()
            .any(|problem| problem.starts_with("credit claim obj-LOAN could not be discounted:")),
        "a claim nothing could value was silently skipped: {:?}",
        understand.problems
    );

    // The other half of a working gate: the same universe in the right unit is
    // admitted and produces a real worst claim. A guard that refused both
    // would be indistinguishable from one that refused everything.
    let mut fraction = Universe::new();
    fraction.insert(govvie("UST2", 2, 0.0450)?)?;
    fraction.insert(govvie("UST10", 10, 0.0435)?)?;
    fraction.insert(breached_loan()?)?;
    let admitted = platform_over(fraction)?;
    let admitted = admitted.credit_register();
    assert!(
        admitted.curve(qip_core::Currency::USD).is_some(),
        "a correctly quoted curve was refused too"
    );
    let (worst_id, worst_loss) = admitted
        .worst_claim()
        .expect("a universe with three credit claims has a worst one");
    assert_eq!(
        worst_id, "obj-LOAN",
        "the worst claim is not the 8%-default loan"
    );
    assert!(
        worst_loss.is_positive(),
        "the worst claim carries a loss of {worst_loss}, which is the manufactured \
         zero this guard exists to refuse"
    );
    assert!(
        admitted.excluded_curve_points().count() == 0,
        "a well-quoted benchmark was excluded"
    );
    Ok(())
}

#[test]
fn a_claim_maturing_beyond_the_curves_longest_quote_is_refused_rather_than_flat_extrapolated()
-> Result<()> {
    // The failure this prevents: `qip_numerics`'s curve flat-extrapolates by
    // design, which is right for a government curve read at 50y off a 2y-30y
    // fit and wrong here. A universe holding one 5y benchmark answered a
    // 30-year claim with the 5y rate, and the resulting present value carried
    // an observation's provenance without an observation behind it.
    let mut universe = Universe::new();
    universe.insert(govvie("UST5", 5, 0.0420)?)?;
    universe.insert(breached_loan()?)?;
    let platform = platform_over(universe)?;
    let register = platform.credit_register();

    let curve = register
        .curve(qip_core::Currency::USD)
        .expect("the sovereign issue did not produce a USD curve");
    let (shortest, longest) = curve.tenor_range();
    // Premise: the curve still answers outside its own range, so the refusal
    // below is the register's decision and not the curve running out of
    // arithmetic.
    assert!(
        curve.rate_at(longest + 25.0).is_finite(),
        "the curve stopped extrapolating, so this proves nothing about the guard"
    );
    // And the premise that the guard admits the tenor it was quoted at.
    assert!(
        register
            .discounted_expected_loss(
                "obj-LOAN",
                qip_core::Currency::USD,
                dec!("1000000"),
                longest
            )
            .is_ok(),
        "the guard refuses the curve's own longest quoted tenor"
    );

    let refusal = register
        .discounted_expected_loss(
            "obj-LOAN",
            qip_core::Currency::USD,
            dec!("1000000"),
            longest + 25.0,
        )
        .expect_err("a claim was discounted 25 years past the curve's longest quote");
    assert!(
        refusal.to_string().contains("outside the"),
        "the refusal does not name the range: {refusal}"
    );
    assert!(
        refusal
            .to_string()
            .contains("supply a sovereign issue at that tenor"),
        "the refusal does not say what to do instead: {refusal}"
    );

    // The short end refuses too, and for the same reason: a three-month claim
    // read off a 5y-only curve is the 5y rate wearing a three-month label.
    assert!(
        register
            .discounted_expected_loss(
                "obj-LOAN",
                qip_core::Currency::USD,
                dec!("1000000"),
                shortest / 2.0,
            )
            .is_err(),
        "a claim shorter than anything quoted was discounted anyway"
    );
    Ok(())
}

#[test]
fn a_credit_claim_with_no_rating_and_no_stated_default_probability_is_reported_unquantified()
-> Result<()> {
    // Refuse, do not substitute. A bond carrying neither a rating nor an
    // estimate has unquantified credit risk, and a register that filled in a
    // prior would publish an expected loss nobody computed — indistinguishable
    // downstream from a measured one.
    let mut unrated = govvie("CORP", 7, 0.0620)?;
    match &mut unrated.extension {
        Extension::Bond(details) => {
            details.credit_rating = None;
            details.issuer = "Unrated Corp".into();
        }
        other => panic!("the fixture stopped being a bond: {other:?}"),
    }

    let mut universe = Universe::new();
    universe.insert(unrated)?;
    let platform = platform_over(universe)?;
    let register = platform.credit_register();

    // Premise: the register found the claim at all rather than skipping it.
    assert_eq!(
        register.profiles().count(),
        0,
        "an unquantified claim was given a profile anyway"
    );
    let refusals: Vec<_> = register.refusals().collect();
    assert_eq!(refusals.len(), 1, "the claim was dropped rather than named");
    assert_eq!(refusals[0].0, "obj-CORP");
    assert!(
        refusals[0]
            .1
            .contains("neither a credit rating nor a stated default probability"),
        "the refusal does not say what is missing: {}",
        refusals[0].1
    );
    assert!(
        register
            .problems()
            .iter()
            .any(|problem| problem.contains("credit claim obj-CORP is unquantified")),
        "the refusal is not raised as a problem: {:?}",
        register.problems()
    );
    Ok(())
}

#[test]
fn a_universe_with_no_credit_in_it_reports_no_credit_clause_at_all() -> Result<()> {
    // The negative case, which is what stops the clause above being vacuous: a
    // summary that always said something would make the first test pass on a
    // register that never read the universe. An equity-only desk sees nothing.
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string("obj-AAA"),
            "AAA",
            InstrumentType::CommonStock,
            fixture_liquidity(),
        )
        .venue("XNYS")
        .price(dec!("100"))
        .provenance(licensed())
        .build(start())?,
    )?;

    let mut platform = platform_over(universe)?;
    assert_eq!(platform.credit_register().profiles().count(), 0);
    assert!(platform.credit_register().summary().is_empty());

    let report = platform.run_cycle(start());
    let understand = report
        .stages
        .iter()
        .find(|stage| stage.stage == Stage::Understand)
        .expect("the cycle ran no UNDERSTAND stage");
    // Premise: the stage produced a detail at all, so an empty-clause
    // assertion is about the clause and not about a missing stage.
    assert!(
        understand.detail.contains("world model holds"),
        "the UNDERSTAND stage said nothing: {}",
        understand.detail
    );
    assert!(
        !understand.detail.contains("credit register"),
        "an equity-only universe reported a credit register: {}",
        understand.detail
    );
    Ok(())
}
