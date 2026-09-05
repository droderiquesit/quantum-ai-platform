//! The credit engine: default probability, recovery, spread decomposition and
//! covenant state.
//!
//! Before this suite the platform's credit capability was two `f64` fields on
//! a risk-profile struct. Every test here asserts a property the engine has to
//! hold rather than a call it has to make.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::testing::approx_eq;
use qip_core::{Decimal, dec};
use qip_financial::credit::{
    Covenant, CovenantKind, CovenantState, CreditProfile, DefaultPrior, indicative_recovery_rate,
};
use qip_financial::extensions::{CreditRating, Seniority};

fn senior(default_probability: f64, recovery: f64) -> Result<CreditProfile> {
    CreditProfile::new(
        "Obligor SA",
        Seniority::SeniorUnsecured,
        default_probability,
        recovery,
    )
}

#[test]
fn a_default_probability_outside_zero_to_one_is_refused_rather_than_clamped() {
    // The unit error this catches is a percentage fed in as a fraction: 4.0
    // meaning four per cent. Clamped to 1.0 it becomes a certainty of default
    // that reads as a modelled conclusion, and the upstream bug survives.
    // The premise: the same obligor with a fraction is accepted.
    assert!(
        senior(0.04, 0.4).is_ok(),
        "the premise fails: a well-formed profile is already refused"
    );

    let refusal = senior(4.0, 0.4).expect_err("a default probability of 4.0 was accepted");
    let message = refusal.to_string();
    assert!(
        message.contains("default probability of Obligor SA is 4"),
        "the refusal does not name the value or the obligor: {message}"
    );
    assert!(
        message.contains("as a fraction rather than a percentage"),
        "the refusal does not say what to do instead: {message}"
    );
    assert!(
        senior(-0.01, 0.4).is_err(),
        "a negative default probability was accepted"
    );
    assert!(
        senior(f64::NAN, 0.4).is_err(),
        "a non-finite default probability was accepted"
    );
}

#[test]
fn a_recovery_rate_above_one_is_refused_naming_the_obligor() {
    // A recovery above one claims a defaulted claim pays more than it owed,
    // and it makes `loss_given_default` negative — an expected loss that
    // *adds* value, which every aggregate downstream would take at face.
    assert!(
        senior(0.04, 1.0).is_ok(),
        "the premise fails: a full recovery is already refused"
    );
    let refusal = senior(0.04, 1.2).expect_err("a recovery rate of 1.2 was accepted");
    let message = refusal.to_string();
    assert!(
        message.contains("recovery rate of Obligor SA is 1.2"),
        "the refusal does not name the value or the obligor: {message}"
    );
}

#[test]
fn an_obligor_with_no_name_is_refused_because_a_credit_fact_needs_one() {
    assert!(
        CreditProfile::new("   ", Seniority::SeniorUnsecured, 0.04, 0.4).is_err(),
        "a profile attributable to nobody was accepted"
    );
}

#[test]
fn survival_falls_with_the_horizon_and_matches_the_stated_one_year_probability()
-> Result<Result<()>> {
    // The property the whole engine rests on: the hazard rate is calibrated so
    // that the one-year survival is exactly `1 - p₁`. An implementation that
    // used `p₁` as the hazard directly would be close at small `p` and wrong
    // by a visible margin at large `p`, which is where credit decisions are
    // actually made.
    let profile = senior(0.20, 0.4)?;
    let one_year = profile.survival_probability(1.0)?;
    assert!(
        approx_eq(one_year, 0.80, 1e-12),
        "one-year survival is {one_year}, not the stated 1 - 0.20"
    );

    let five_year = profile.survival_probability(5.0)?;
    assert!(
        five_year < one_year,
        "survival did not fall over a longer horizon: {five_year} against {one_year}"
    );
    assert!(
        approx_eq(profile.survival_probability(0.0)?, 1.0, 1e-12),
        "an obligor defaulted before any time had passed"
    );
    assert!(
        approx_eq(profile.cumulative_default_probability(1.0)?, 0.20, 1e-12),
        "the cumulative default probability disagrees with the survival curve"
    );
    Ok(Ok(()))
}

#[test]
fn a_horizon_before_the_present_is_refused_rather_than_projected() -> Result<()> {
    let profile = senior(0.04, 0.4)?;
    // The premise: a forward horizon is projected.
    assert!(profile.survival_probability(3.0).is_ok());
    let refusal = profile
        .survival_probability(-1.0)
        .expect_err("survival was projected backwards");
    assert!(
        refusal.to_string().contains("already matured is"),
        "the refusal does not say what to do instead: {refusal}"
    );
    assert!(profile.survival_probability(f64::INFINITY).is_err());
    Ok(())
}

#[test]
fn an_obligor_already_certain_to_default_has_no_hazard_rate() -> Result<()> {
    // `-ln(0)` is infinite. Every survival probability, spread and expected
    // loss computed from an infinite hazard is a number nobody can act on, and
    // an expected loss of `inf` propagates through an aggregate as silently as
    // a zero would.
    let alive = senior(0.99, 0.4)?;
    assert!(
        alive.hazard_rate().is_ok(),
        "the premise fails: an obligor short of certain default has no hazard rate either"
    );

    let defaulted = senior(1.0, 0.4)?;
    let refusal = defaulted
        .hazard_rate()
        .expect_err("a certain default produced a hazard rate");
    assert!(
        refusal
            .to_string()
            .contains("book the recovery on the claim"),
        "the refusal does not say what to do instead: {refusal}"
    );
    assert!(
        defaulted.survival_probability(1.0).is_err(),
        "a survival curve was projected for a defaulted obligor"
    );
    Ok(())
}

#[test]
fn the_spread_decomposition_multiplies_out_to_the_spread_in_exact_arithmetic() -> Result<()> {
    // The identity the decomposition exists to make checkable by a person:
    // spread = cumulative default probability x loss given default. Asserted
    // in `Decimal` on purpose — the same identity in `f64` holds only to
    // within a rounding, and a report a person reconciles by hand needs it to
    // hold exactly.
    let profile = senior(0.10, 0.40)?;
    let decomposition = profile.spread_decomposition(3.0)?;
    assert!(
        decomposition.default_probability > Decimal::ZERO,
        "the premise fails: three years carries no default probability"
    );
    assert_eq!(
        decomposition.loss_given_default,
        dec!("0.6"),
        "loss given default is not 1 - recovery"
    );
    assert_eq!(
        decomposition.default_probability * decomposition.loss_given_default,
        decomposition.spread,
        "the components do not multiply out to the spread"
    );
    Ok(())
}

#[test]
fn an_expected_loss_is_money_and_scales_with_both_the_exposure_and_the_horizon() -> Result<()> {
    // The money crossing. The property: doubling the exposure doubles the
    // loss exactly, which `f64` money would only do to within a rounding, and
    // a longer horizon loses more.
    let profile = senior(0.10, 0.40)?;
    let one = profile.expected_loss(dec!("1000000"), 3.0)?;
    let two = profile.expected_loss(dec!("2000000"), 3.0)?;
    assert!(
        one > Decimal::ZERO,
        "the premise fails: a defaultable claim carries no expected loss"
    );
    assert_eq!(one * dec!("2"), two, "the loss is not linear in exposure");
    assert!(
        profile.expected_loss(dec!("1000000"), 5.0)? > one,
        "a longer horizon did not carry more expected loss"
    );
    assert!(
        profile.expected_loss(dec!("-1000000"), 3.0).is_err(),
        "a negative exposure was taken as a claim"
    );
    Ok(())
}

#[test]
fn a_riskless_obligor_carries_no_expected_loss_and_a_worthless_one_loses_it_all() -> Result<()> {
    // Both ends, because a mistaken sign or an inverted recovery reads as
    // plausible in the middle of the range and is obvious only at the edges.
    let riskless = senior(0.0, 0.4)?;
    assert_eq!(riskless.expected_loss(dec!("1000000"), 5.0)?, Decimal::ZERO);

    let unsecured = CreditProfile::new("Zero Recovery SA", Seniority::Subordinated, 0.5, 0.0)?;
    let loss = unsecured.expected_loss(dec!("1000000"), 1.0)?;
    assert_eq!(
        loss,
        dec!("500000"),
        "a one-year 50% default with no recovery did not lose half the claim"
    );

    // And the recovery must actually be applied. A mutation multiplying the
    // exposure by the default probability alone survived both cases above,
    // because a zero recovery and a zero default probability each make loss
    // given default invisible — the exact class of hole mutation testing
    // exists to find. At a 40% recovery the two differ by the recovery.
    let partial = CreditProfile::new("Partial Recovery SA", Seniority::SeniorUnsecured, 0.5, 0.4)?;
    assert_eq!(
        partial.expected_loss(dec!("1000000"), 1.0)?,
        dec!("300000"),
        "a 50% default with a 40% recovery did not lose 50% x 60% of the claim"
    );
    Ok(())
}

#[test]
fn a_covenant_is_tested_in_the_direction_its_kind_states() -> Result<()> {
    // The single worst failure this module can have is testing a covenant the
    // wrong way round, which reports a breached borrower as compliant. A
    // ceiling and a floor at the same threshold and the same observation must
    // disagree.
    let ceiling = Covenant::new("net_debt_to_ebitda", CovenantKind::Ceiling, 6.0, 7.5)?;
    let floor = Covenant::new("interest_coverage", CovenantKind::Floor, 6.0, 7.5)?;
    assert_eq!(
        ceiling.state(),
        CovenantState::Breached,
        "leverage above its ceiling was not a breach"
    );
    assert_eq!(
        floor.state(),
        CovenantState::Compliant,
        "coverage above its floor was called a breach"
    );

    // Met, but inside the watch band: a state with only two arms would report
    // this identically to a borrower at half the threshold.
    let tight = Covenant::new("net_debt_to_ebitda", CovenantKind::Ceiling, 6.0, 5.8)?;
    assert_eq!(tight.state(), CovenantState::Watch);
    let comfortable = Covenant::new("net_debt_to_ebitda", CovenantKind::Ceiling, 6.0, 2.0)?;
    assert_eq!(comfortable.state(), CovenantState::Compliant);
    Ok(())
}

#[test]
fn a_covenant_with_a_non_finite_observation_is_refused_rather_than_tested() {
    // `NaN <= threshold` is false and `NaN >= threshold` is false, so the same
    // missing number reads as a breach in both directions — an operator paged
    // on arithmetic rather than on a borrower.
    assert!(
        Covenant::new("leverage", CovenantKind::Ceiling, 6.0, 5.0).is_ok(),
        "the premise fails: a well-formed covenant is already refused"
    );
    let refusal = Covenant::new("leverage", CovenantKind::Ceiling, 6.0, f64::NAN)
        .expect_err("a covenant with no observation was tested");
    assert!(
        refusal
            .to_string()
            .contains("omit the covenant until the borrower reports"),
        "the refusal does not say what to do instead: {refusal}"
    );
    assert!(Covenant::new("leverage", CovenantKind::Ceiling, f64::NAN, 5.0).is_err());
    assert!(Covenant::new("  ", CovenantKind::Ceiling, 6.0, 5.0).is_err());
}

#[test]
fn an_obligor_with_no_covenant_reports_no_state_rather_than_compliance() -> Result<()> {
    // The `MaxExpectedShortfall` shape, refused: "compliant" asserts that
    // tests were run and passed. An obligor nobody wrote a covenant for has
    // had nothing tested, and reporting the two identically is a control that
    // reads as protection and is not.
    let untested = senior(0.04, 0.4)?;
    assert_eq!(
        untested.covenant_state(),
        None,
        "an untested obligor reported a covenant state"
    );

    let tested = senior(0.04, 0.4)?.with_covenant(Covenant::new(
        "leverage",
        CovenantKind::Ceiling,
        6.0,
        2.0,
    )?)?;
    assert_eq!(tested.covenant_state(), Some(CovenantState::Compliant));
    Ok(())
}

#[test]
fn the_worst_covenant_is_what_the_profile_reports() -> Result<()> {
    // One breach among compliant tests must not be averaged away. The premise
    // asserts the register holds more than the breached test, so a profile
    // that dropped everything but the worst would not pass this either.
    let profile = senior(0.04, 0.4)?
        .with_covenant(Covenant::new("coverage", CovenantKind::Floor, 2.0, 4.0)?)?
        .with_covenant(Covenant::new("leverage", CovenantKind::Ceiling, 6.0, 9.0)?)?;
    assert_eq!(
        profile.covenants().count(),
        2,
        "the premise fails: the profile did not keep both covenants"
    );
    assert_eq!(profile.covenant_state(), Some(CovenantState::Breached));
    let breached = profile.breached_covenants();
    assert_eq!(
        breached.len(),
        1,
        "the compliant covenant was called a breach"
    );
    assert_eq!(breached[0].name, "leverage");
    assert!(
        breached[0].describe().contains("breached"),
        "the description does not name the state: {}",
        breached[0].describe()
    );
    Ok(())
}

#[test]
fn a_covenant_registered_twice_under_one_name_is_refused_naming_it() -> Result<()> {
    // Overwriting would let a stale test silently replace a live one, and the
    // register would then report a state nobody could reproduce from the
    // credit agreement.
    let once = senior(0.04, 0.4)?.with_covenant(Covenant::new(
        "leverage",
        CovenantKind::Ceiling,
        6.0,
        2.0,
    )?)?;
    let refusal = once
        .with_covenant(Covenant::new("leverage", CovenantKind::Ceiling, 4.0, 3.0)?)
        .expect_err("a covenant name was registered twice");
    assert!(
        refusal
            .to_string()
            .contains("covenant leverage is already registered"),
        "the refusal does not name the covenant: {refusal}"
    );
    Ok(())
}

#[test]
fn a_rated_profile_carries_the_ratings_own_default_rate_and_says_where_it_came_from() -> Result<()>
{
    // A rating-implied through-the-cycle prior and an issuer-specific estimate
    // are not the same claim. A register reporting only the number would let
    // the weaker be read as the stronger.
    let rated =
        CreditProfile::from_rating("Issuer AG", Seniority::SecuredFirstLien, CreditRating::B)?;
    assert_eq!(rated.prior(), DefaultPrior::Rated);
    assert_eq!(rated.rating(), Some(CreditRating::B));
    assert!(
        approx_eq(
            rated.one_year_default_probability(),
            CreditRating::B.indicative_default_probability(),
            1e-12
        ),
        "the profile does not carry the rating's own default rate"
    );
    // Recovery follows the capital structure, and a first-lien claim must
    // recover more than a subordinated one — the ordering is the property,
    // not the individual levels.
    assert!(
        indicative_recovery_rate(Seniority::SecuredFirstLien)
            > indicative_recovery_rate(Seniority::SeniorUnsecured),
        "a first-lien claim does not recover more than an unsecured one"
    );
    assert!(
        indicative_recovery_rate(Seniority::SeniorUnsecured)
            > indicative_recovery_rate(Seniority::Subordinated),
        "an unsecured claim does not recover more than a subordinated one"
    );

    let stated = senior(0.04, 0.4)?;
    assert_eq!(stated.prior(), DefaultPrior::Stated);
    assert_eq!(stated.rating(), None);

    // A worse rating must default more often, or the scale carries no
    // information at all.
    let strong =
        CreditProfile::from_rating("Issuer AG", Seniority::SeniorUnsecured, CreditRating::Aa)?;
    assert!(
        strong.one_year_default_probability() < rated.one_year_default_probability(),
        "a Aa obligor is not safer than a B one"
    );
    Ok(())
}
