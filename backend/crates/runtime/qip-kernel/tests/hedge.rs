//! Blueprint §31.4's hedge engine, driven through the platform that now calls
//! it.
//!
//! `qip-risk`'s hedge engine was built, tested and reachable from nothing: its
//! own module header said so, and a `grep` for it across `runtime/`, `apps/`
//! and `services/` returned nothing. Arithmetic no deployed process runs is
//! not a control, however good its unit tests are — the same finding that
//! `MaxExpectedShortfall` earned when it shipped in every default limit set
//! over a figure nothing filled. These tests hold the seam: a policy a
//! deployment declares is surveyed in DECIDE against the book the platform
//! actually holds, its outcome reaches the hash-chained log, and — the part
//! that matters most in this domain — a proposal stays a proposal.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_events::Topic;
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::risk_profile::{FactorExposures, RiskCharacteristics};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::hedge_review::{HedgePolicyDeclaration, HedgeSurveyed};
use qip_kernel::platform::Platform;
use qip_market::quote::{Trade, TradeCondition};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_risk::aggregate::AggregateFigures;
use qip_risk::hedge::{HedgeAxis, HedgeOutcome, HedgeRefusal, HedgeSide};
use qip_risk::limits::LimitSet;
use qip_streaming::envelope::StreamEnvelope;

/// The instrument the book is long, and the sector the policy defends.
const EXPOSED: &str = "obj-AAA";
/// The instrument the policy hedges with. Never held: the whole point of the
/// last assertion in the first test is that it stays that way.
const HEDGE: &str = "obj-HDG";
const SECTOR: &str = "information_technology";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// A liquid listed name, stated rather than inherited.
///
/// `LiquidityProfile` has no `Default` on purpose — the one it had asserted a
/// tight quote and a one-session exit for any instrument at all, and the
/// liquidity limits read exactly those two figures.
fn fixture_liquidity() -> LiquidityProfile {
    LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn listed(id: &str, symbol: &str, sector: Sector) -> FinancialObject {
    FinancialObject::builder(
        ObjectId::from_string(id),
        symbol,
        InstrumentType::CommonStock,
        fixture_liquidity(),
    )
    .venue("XNYS")
    .sector(sector)
    .price(dec!("100"))
    .provenance(Provenance::synthetic("test", start()))
    .build(start())
    .expect("a listed equity record")
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(listed(EXPOSED, "AAA", Sector::InformationTechnology))
        .expect("insertable");
    // Deliberately outside the hedged sector: an instrument that added to the
    // exposure it hedges would make the sizing test pass for the wrong reason.
    universe
        .insert(listed(HEDGE, "HDG", Sector::Financials))
        .expect("insertable");
    universe
}

/// One full-hedge policy on the sector the book is long.
///
/// Beta of one so the arithmetic in the assertions is legible; the engine's
/// own tests pin ratios under which multiply and divide disagree.
fn sector_policy() -> HedgePolicyDeclaration {
    HedgePolicyDeclaration::new(
        "technology-concentration",
        HedgeAxis::Sector,
        SECTOR,
        ObjectId::from_string(HEDGE),
        dec!("1"),
    )
    .with_rationale("the book's technology concentration is hedged with the index proxy")
}

fn platform_with(policies: Vec<HedgePolicyDeclaration>) -> Result<Platform> {
    platform_over(universe(), policies)
}

/// The same platform over a stated catalogue.
///
/// Split out because the exposure-read refusals below differ from every other
/// test here in exactly one input — the catalogue — and a fixture that could
/// not vary it would have left the whole of `exposures_of`'s refusal arm
/// undrivable from the seam that actually runs it.
fn platform_over(universe: Universe, policies: Vec<HedgePolicyDeclaration>) -> Result<Platform> {
    let config = PlatformConfig::default()
        .with_initial_equity(Decimal::from_int(10_000_000))
        .with_hedge_policies(policies);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
}

/// Put the book long the hedged sector, through the full control path.
fn go_long(platform: &mut Platform, shares: Decimal) -> Result<()> {
    let order = platform.order_from(
        ObjectId::from_string(EXPOSED),
        Side::Buy,
        shares,
        dec!("100"),
        "prop-hedge",
        vec!["hyp-hedge".to_string()],
        start(),
    );
    platform.submit_order(order, start())
}

/// A printed trade in the hedge instrument, which is the only thing that gives
/// the survey a price to size against.
fn print_hedge_price(platform: &mut Platform, price: Decimal) {
    let absorbed = platform.observe(vec![SensedRecord::Trade(Trade {
        object_id: ObjectId::from_string(HEDGE),
        venue: "XNYS".into(),
        at: start(),
        price,
        size: Decimal::from_int(100),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: None,
        quality: Default::default(),
    })]);
    assert_eq!(absorbed, 1, "the print must reach the snapshot");
}

/// The hedge survey the log holds for a cycle, decoded.
fn logged_survey(platform: &Platform) -> Result<HedgeSurveyed> {
    let records = platform.event_log().by_topic(Topic::RiskEvaluated);
    let mut surveys = Vec::new();
    for record in records {
        if let Ok(envelope) = StreamEnvelope::from_frame(record)
            && let Ok(decoded) = envelope.decode::<HedgeSurveyed>()
        {
            surveys.push(decoded.body);
        }
    }
    assert_eq!(
        surveys.len(),
        1,
        "exactly one hedge survey was journalled for one cycle"
    );
    Ok(surveys.remove(0))
}

#[test]
fn a_declared_hedge_policy_is_surveyed_against_the_book_in_decide_and_reaches_the_log() -> Result<()>
{
    let mut platform = platform_with(vec![sector_policy()])?;
    go_long(&mut platform, dec!("900"))?;
    print_hedge_price(&mut platform, dec!("100"));

    // Premise 1: the book really is long the hedged sector. Without this the
    // survey would report no action and the test would pass on an empty book,
    // which is the shape of failure this repository has already shipped once.
    let exposed = platform
        .risk_figures()
        .position_notionals()
        .get(EXPOSED)
        .copied()
        .unwrap_or(Decimal::ZERO);
    assert!(
        exposed.is_positive(),
        "the fixture order did not leave the book long {EXPOSED}, so there is nothing to hedge"
    );
    // Premise 2: nothing is held in the hedge instrument yet, so the last
    // assertion below is about this cycle and not about an empty map.
    assert!(
        !platform
            .risk_figures()
            .position_notionals()
            .contains_key(HEDGE),
        "the book already held the hedge instrument before the survey ran"
    );

    let report = platform.run_cycle(start());
    let decide = report
        .stage(Stage::Decide)
        .expect("the decide stage ran in the cycle");
    assert!(
        decide
            .detail
            .contains("1 hedge policy(ies) surveyed: 1 proposed"),
        "the decide stage did not report the survey: {}",
        decide.detail
    );

    // The log is the record. A proposal a person cannot re-derive from it is
    // not attributable, which is the whole argument for §31.4 existing.
    let survey = logged_survey(&platform)?;
    assert_eq!(survey.declared, 1);
    assert_eq!(survey.proposed, 1);
    assert_eq!(survey.refused, 0);
    let HedgeOutcome::Proposed(proposal) = &survey.outcomes[0] else {
        panic!("the survey's one outcome is not a proposal: {survey:?}");
    };
    // A long excess is sold off. A proposal on the same side as the exposure
    // would double the position while being called a hedge.
    assert_eq!(proposal.side, HedgeSide::Sell);
    assert_eq!(proposal.observed_net, exposed);
    // Beta one, so the hedge notional would be the whole excess if the lot
    // allowed it. It does not: the fixture's fill slipped, the exposure is not
    // a round number of hundred-dollar units, and the engine rounds **down**
    // to the lot. So the property asserted is the invariant rather than an
    // arithmetic identity — the hedge never exceeds the excess, and one more
    // lot would. The second half is what makes this more than "no hedge at
    // all": a sizing that always returned zero would satisfy the first.
    assert_eq!(proposal.price, dec!("100"));
    assert!(
        proposal.quantity.is_positive(),
        "a hedge of nothing is not a hedge"
    );
    assert_eq!(
        proposal.quantity,
        proposal
            .quantity
            .floor_to_step(proposal.instrument.lot_size),
        "the hedge quantity is not a whole number of lots"
    );
    assert!(
        proposal.hedge_notional <= exposed,
        "the hedge of {} exceeds the exposure of {exposed} it was sized to reduce",
        proposal.hedge_notional
    );
    let one_lot_more = (proposal.quantity + proposal.instrument.lot_size)
        * proposal.price
        * proposal.instrument.contract_multiplier;
    assert!(
        one_lot_more > exposed,
        "one more lot would still fit inside the exposure, so the hedge is under-sized by more \
         than the rounding: {one_lot_more} against {exposed}"
    );
    // The under-hedge invariant, end to end through the composition: the
    // residual never crosses zero, because past the target a hedge is a new
    // naked position the other way.
    assert!(
        !proposal.expected_residual.is_negative(),
        "the proposal over-hedged a long book: residual {}",
        proposal.expected_residual
    );
    // The mechanics came from the catalogue, not from the declaration — which
    // carries neither field, and could not have supplied them.
    assert_eq!(proposal.instrument.contract_multiplier, Decimal::ONE);
    assert_eq!(proposal.instrument.symbol, "HDG");

    // **The proposal did not become a position.** There is no path from the
    // survey to a broker, and this is the assertion that says so in behaviour
    // rather than in prose: a cycle that proposed a hedge of nine hundred
    // units left the book holding none of them.
    assert!(
        !platform
            .risk_figures()
            .position_notionals()
            .contains_key(HEDGE),
        "a hedge proposal became a position; the approval path is the only road to a market"
    );
    assert!(platform.event_log().verify_chain().is_ok());
    Ok(())
}

#[test]
fn a_hedge_whose_instrument_has_no_price_is_refused_and_the_stage_names_the_exposure() -> Result<()>
{
    // No print in the hedge instrument, so the platform has seen neither a
    // trade nor a quote in it. Sizing against the catalogue's stored price
    // would be the guessed price the engine refuses — and a guessed hedge on a
    // real exposure is a real position.
    let mut platform = platform_with(vec![sector_policy()])?;
    go_long(&mut platform, dec!("900"))?;

    // Premise: the exposure exists, so the refusal below is about the missing
    // price and not about a book with nothing to hedge.
    assert!(
        platform
            .risk_figures()
            .position_notionals()
            .get(EXPOSED)
            .copied()
            .unwrap_or(Decimal::ZERO)
            .is_positive(),
        "the fixture left the book flat, so no hedge would have been sized anyway"
    );

    let report = platform.run_cycle(start());
    let decide = report
        .stage(Stage::Decide)
        .expect("the decide stage ran in the cycle");
    assert!(
        decide
            .detail
            .contains("1 hedge policy(ies) surveyed: 0 proposed, 1 refused"),
        "the decide stage did not report the refusal: {}",
        decide.detail
    );
    // Named on the stage, not folded into a count: an operator can only act on
    // the exposure that is still naked if the report says which one it is.
    assert!(
        decide
            .problems
            .iter()
            .any(|problem| problem.starts_with("a hedge was refused:") && problem.contains("HDG")),
        "the refusal did not name the instrument: {:?}",
        decide.problems
    );

    let survey = logged_survey(&platform)?;
    assert_eq!(survey.refused, 1);
    let HedgeOutcome::Refused(refusal) = &survey.outcomes[0] else {
        panic!("the survey's one outcome is not a refusal: {survey:?}");
    };
    assert!(
        matches!(refusal, HedgeRefusal::UnusablePrice { .. }),
        "an unpriced hedge instrument was refused for the wrong reason: {refusal:?}"
    );
    Ok(())
}

#[test]
fn a_platform_declaring_no_hedge_policy_says_so_on_the_stage_rather_than_falling_silent()
-> Result<()> {
    let mut platform = platform_with(Vec::new())?;
    go_long(&mut platform, dec!("900"))?;

    // Premise: the book carries an exposure somebody might have expected to be
    // hedged. A silent stage over an unhedged book reads exactly like a silent
    // stage over a hedged one with nothing to do, and the second is the state
    // an operator would assume.
    assert!(
        platform
            .risk_figures()
            .position_notionals()
            .get(EXPOSED)
            .copied()
            .unwrap_or(Decimal::ZERO)
            .is_positive(),
        "the fixture left the book flat, so there was nothing anyone would expect hedged"
    );

    let report = platform.run_cycle(start());
    let decide = report
        .stage(Stage::Decide)
        .expect("the decide stage ran in the cycle");
    assert!(
        decide
            .detail
            .contains("no hedge policy is declared, so no exposure is hedged"),
        "an unhedged book said nothing about being unhedged: {}",
        decide.detail
    );
    // And nothing is journalled: a record that says "nothing" every cycle is a
    // record nobody reads, and the line above already said it.
    let surveys = platform
        .event_log()
        .by_topic(Topic::RiskEvaluated)
        .into_iter()
        .filter(|record| {
            StreamEnvelope::from_frame(record)
                .is_ok_and(|envelope: StreamEnvelope| envelope.decode::<HedgeSurveyed>().is_ok())
        })
        .count();
    assert_eq!(surveys, 0);
    Ok(())
}

/// The factor the second policy of the pair below names.
const FACTOR: &str = "momentum";

/// The exposed name's record, with one stated factor loading.
///
/// `RiskCharacteristics::is_coherent` — the gate both `ObjectBuilder::build`
/// and `Universe::insert` run — reads the volatility, the beta and the three
/// fractions and never looks at `factor_exposures`. A loading of `NaN` is
/// therefore registrable, which is why `exposures_of` refuses it downstream
/// and why that refusal is reachable from a real catalogue rather than only
/// from a hand-built state.
fn listed_with_loading(loading: f64) -> FinancialObject {
    FinancialObject::builder(
        ObjectId::from_string(EXPOSED),
        "AAA",
        InstrumentType::CommonStock,
        fixture_liquidity(),
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .risk(RiskCharacteristics {
        factor_exposures: FactorExposures::new().with(FACTOR, loading),
        ..RiskCharacteristics::default()
    })
    .provenance(Provenance::synthetic("test", start()))
    .build(start())
    .expect("a listed equity record; the coherence gate does not read loadings")
}

/// The fixture catalogue with the held name's loading stated.
fn universe_loading(loading: f64) -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(listed_with_loading(loading))
        .expect("a record with any loading is insertable");
    universe
        .insert(listed(HEDGE, "HDG", Sector::Financials))
        .expect("insertable");
    universe
}

/// One policy on the factor axis the record poisons, one on the sector axis it
/// never touches. The second is what makes "every policy" an assertion rather
/// than a restatement of the first.
fn factor_and_sector_policies() -> Vec<HedgePolicyDeclaration> {
    vec![
        HedgePolicyDeclaration::new(
            "factor-concentration",
            HedgeAxis::Factor,
            FACTOR,
            ObjectId::from_string(HEDGE),
            dec!("1"),
        ),
        sector_policy(),
    ]
}

#[test]
fn a_well_formed_catalogue_over_this_book_proposes_on_both_axes_and_the_proposal_series_moves()
-> Result<()> {
    // The admit half, and the premise for the refusal test below. A survey
    // that refused every catalogue would satisfy every assertion in that test
    // and would be a gate nobody could pass — the thing the infrastructure
    // rule names as the difference between a working gate and one that refuses
    // everything.
    let mut platform = platform_over(universe_loading(0.5), factor_and_sector_policies())?;
    go_long(&mut platform, dec!("900"))?;
    print_hedge_price(&mut platform, dec!("100"));
    assert!(
        platform
            .risk_figures()
            .position_notionals()
            .get(EXPOSED)
            .copied()
            .unwrap_or(Decimal::ZERO)
            .is_positive(),
        "the fixture left the book flat, so nothing would have been proposed anyway"
    );

    let report = platform.run_cycle(start());
    let decide = report
        .stage(Stage::Decide)
        .expect("the decide stage ran in the cycle");
    assert!(
        decide
            .detail
            .contains("2 hedge policy(ies) surveyed: 2 proposed, 0 refused"),
        "a well-formed catalogue did not produce a proposal on each axis: {}",
        decide.detail
    );

    // The series the survey had none of until this lane. The DECIDE line and
    // the journal both answer for one cycle; a desk watching charts could not
    // see that a book had gone unhedged for a week.
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::HEDGE_PROPOSALS, &labels([])),
        2,
        "the proposal series did not move with the two proposals the stage reported"
    );
    assert_eq!(
        snapshot.counter_total(names::HEDGE_REFUSALS),
        0,
        "a survey that refused nothing counted a refusal"
    );
    Ok(())
}

#[test]
fn a_stated_loading_that_is_not_a_number_leaves_every_declared_exposure_unhedged_and_charts_it()
-> Result<()> {
    // Blueprint §31.4's fail-closed arm, driven end to end for the first time.
    // It was reachable — a catalogue record states the loading and the
    // coherence gate does not read it — and no test anywhere drove it, so
    // nothing proved it fired, that it refused *every* policy rather than the
    // failing axis, or that a person would be told. A refusal nothing drives
    // reads as protection and is not; this repository has already shipped that
    // defect once, in a limit over a figure nothing filled.
    let mut platform = platform_over(universe_loading(f64::NAN), factor_and_sector_policies())?;
    go_long(&mut platform, dec!("900"))?;
    print_hedge_price(&mut platform, dec!("100"));

    // Premise: the book is long the sector the second policy defends and the
    // hedge instrument has a price, so over a well-formed catalogue this is
    // exactly the platform that proposed twice in the test above. Nothing
    // below is true of an empty book.
    assert!(
        platform
            .risk_figures()
            .position_notionals()
            .get(EXPOSED)
            .copied()
            .unwrap_or(Decimal::ZERO)
            .is_positive(),
        "the fixture left the book flat, so there was nothing to leave unhedged"
    );

    let report = platform.run_cycle(start());
    let decide = report
        .stage(Stage::Decide)
        .expect("the decide stage ran in the cycle");
    // "none surveyed" and not "0 proposed": the second phrasing is the one a
    // book inside its thresholds produces, and an operator who could not tell
    // the two apart would read a survey that never ran as a survey with
    // nothing to do.
    assert!(
        decide
            .detail
            .contains("2 hedge policy(ies) declared and none surveyed"),
        "the stage did not say that no policy was surveyed at all: {}",
        decide.detail
    );
    assert!(
        decide.problems.iter().any(|problem| {
            problem.starts_with(
                "no hedge policy could be surveyed, so every declared exposure \
                                 is unhedged:",
            ) && problem.contains(FACTOR)
        }),
        "the stage raised no problem naming the factor a person must correct: {:?}",
        decide.problems
    );

    // The log carries the refusal, and carries no proposal: the sector policy
    // proposed a moment ago over the same book in the test above, and this is
    // where a survey that failed open would show it.
    let survey = logged_survey(&platform)?;
    assert_eq!(
        survey.declared, 2,
        "the premise: two policies were declared"
    );
    assert_eq!(survey.proposed, 0);
    assert_eq!(survey.refused, 0);
    assert!(
        survey.outcomes.is_empty(),
        "an outcome was produced over a book the platform could not describe: {:?}",
        survey.outcomes
    );
    assert!(
        survey.exposures_refused.as_deref().is_some_and(
            |refusal| refusal.contains("not a number a position notional can be multiplied by")
        ),
        "the journalled survey does not carry the reason: {:?}",
        survey.exposures_refused
    );
    assert!(platform.event_log().verify_chain().is_ok());

    // Charted, and under the survey-wide reason rather than a per-policy one:
    // this is one book nobody could describe, not one policy refusing.
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(
            names::HEDGE_REFUSALS,
            &labels([("reason", "exposures_unreadable")])
        ),
        1,
        "the survey-wide refusal is not on the series"
    );
    assert_eq!(
        snapshot.counter_total(names::HEDGE_REFUSALS),
        1,
        "the one unreadable book was counted more than once, or under a second reason as well"
    );
    assert_eq!(
        snapshot.counter(names::HEDGE_PROPOSALS, &labels([])),
        0,
        "a survey that produced no proposal counted one"
    );
    Ok(())
}

#[test]
fn a_refusal_the_engine_produced_is_charted_under_its_own_reason_and_not_the_survey_wide_one()
-> Result<()> {
    // Two refusals that need opposite corrections must not fold into one
    // number: "the platform has seen no price in your hedge instrument" and
    // "this book cannot be described at all" are different jobs for different
    // people. The label is what carries that, and an exhaustive match over
    // `HedgeRefusal` is what bounds it.
    let mut platform = platform_with(vec![sector_policy()])?;
    go_long(&mut platform, dec!("900"))?;
    // Deliberately no print in the hedge instrument.

    assert!(
        platform
            .risk_figures()
            .position_notionals()
            .get(EXPOSED)
            .copied()
            .unwrap_or(Decimal::ZERO)
            .is_positive(),
        "the fixture left the book flat, so no policy would have reached the price check"
    );

    let report = platform.run_cycle(start());
    let decide = report
        .stage(Stage::Decide)
        .expect("the decide stage ran in the cycle");
    assert!(
        decide
            .detail
            .contains("1 hedge policy(ies) surveyed: 0 proposed, 1 refused"),
        "the premise: the survey ran and refused one policy: {}",
        decide.detail
    );

    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(
            names::HEDGE_REFUSALS,
            &labels([("reason", "unusable_price")])
        ),
        1,
        "the engine's own refusal reason is not on the series"
    );
    assert_eq!(
        snapshot.counter(
            names::HEDGE_REFUSALS,
            &labels([("reason", "exposures_unreadable")])
        ),
        0,
        "one policy's refusal was charted as a book nobody could describe"
    );
    assert_eq!(snapshot.counter_total(names::HEDGE_REFUSALS), 1);
    Ok(())
}
