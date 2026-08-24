//! Tests for the hedge engine.
//!
//! The two that matter most are mutation-grade on the financial arithmetic.
//! The rounding test first exhibits the exact case where rounding half-up
//! would over-hedge — producing a naked position the other way — and then
//! shows the engine rounds down through it. The beta tests pin the ratio with
//! numbers under which multiply and divide give different answers, so a
//! transposed operator cannot survive the suite.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::{Decimal, ObjectId, Timestamp};
use qip_portfolio::exposure::{Exposure, ExposureBreakdown};
use qip_risk::hedge::{
    HedgeAxis, HedgeEngine, HedgeExposures, HedgeInstrument, HedgeOutcome, HedgePolicy,
    HedgeProposal, HedgeRefusal, HedgeSide, propose_hedge,
};
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use std::collections::BTreeMap;

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("test fixture decimal")
}

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn spy() -> HedgeInstrument {
    HedgeInstrument::new(ObjectId::from_string("SPY"), "SPY")
}

/// Exposures with one named sector bucket and nothing else.
fn sector_exposure(bucket: &str, net: Decimal) -> HedgeExposures {
    let mut breakdown = ExposureBreakdown::default();
    breakdown.by_sector.add(bucket, net);
    breakdown.by_asset_class.add("equity", net);
    HedgeExposures::new(breakdown)
}

fn wide_limits() -> LimitSet {
    LimitSet::new("wide")
}

fn solvent_state() -> RiskState {
    RiskState {
        equity: Decimal::from_int(10_000_000),
        cash: Decimal::from_int(10_000_000),
        ..RiskState::default()
    }
}

fn tech_policy(beta: &str) -> HedgePolicy {
    HedgePolicy::new("tech-hedge", HedgeAxis::Sector, "technology", d(beta))
        .with_instrument(spy())
        .with_rationale("technology concentration is hedged with the index")
}

fn propose(policy: &HedgePolicy, exposures: &HedgeExposures, price: Decimal) -> HedgeOutcome {
    let prices = BTreeMap::from([("SPY".to_string(), price)]);
    propose_hedge(
        policy,
        exposures,
        &prices,
        &wide_limits(),
        &solvent_state(),
        at(),
    )
}

fn expect_proposal(outcome: HedgeOutcome) -> HedgeProposal {
    match outcome {
        HedgeOutcome::Proposed(proposal) => *proposal,
        other => panic!("expected a proposal, got {other:?}"),
    }
}

// --- the under-hedge rounding invariant -------------------------------------

#[test]
fn rounding_half_up_would_over_hedge_and_the_engine_rounds_down_through_that_case() {
    // The premise, exhibited before the engine touches it: a long technology
    // exposure of 10,050 with beta 1 at a price of 100 wants exactly 100.5
    // units. Round half-up and you sell 101 units — 10,100 of notional against
    // 10,050 of exposure. The "hedge" is then 50 short of technology beta that
    // nothing on the book offsets: a new naked position the other way.
    let excess = d("10050");
    let price = d("100");
    let rounded_up_notional = d("101") * price;
    assert!(
        rounded_up_notional > excess,
        "the premise of this test is that 101 units at 100 ({rounded_up_notional}) exceeds the \
         exposure of {excess}; if this fails the fixture no longer exhibits the over-hedge case"
    );

    // The engine, given the same case, sells 100 — never 101.
    let proposal = expect_proposal(propose(
        &tech_policy("1"),
        &sector_exposure("technology", excess),
        price,
    ));
    assert_eq!(proposal.quantity, d("100"), "100.5 units must floor to 100");
    assert_eq!(proposal.hedge_notional, d("10000"));
    assert!(
        proposal.hedge_notional <= excess,
        "the hedge notional may never exceed the exposure it reduces"
    );
    // The residual is the 50 left un-hedged, on the same side as the excess.
    assert_eq!(proposal.expected_residual, d("50"));
    assert_eq!(
        proposal.expected_residual.signum(),
        proposal.excess.signum()
    );
}

#[test]
fn the_residual_never_flips_to_the_other_side_of_the_target() {
    // Across a spread of awkward sizes, the expected residual is zero or on
    // the same side as the excess. A residual on the far side *is* an
    // over-hedge, whatever the rounding path that produced it.
    for (net, beta, price) in [
        ("10050", "1", "100"),
        ("9999.999999999", "1", "3.7"),
        ("123456.78", "0.85", "417.12"),
        ("-98765.4321", "1.3", "52.5"),
        ("1000000", "1.17", "333.33"),
        ("-7", "0.5", "3"),
    ] {
        let outcome = propose(
            &tech_policy(beta),
            &sector_exposure("technology", d(net)),
            d(price),
        );
        match outcome {
            HedgeOutcome::Proposed(proposal) => {
                assert!(
                    proposal.expected_residual.is_zero()
                        || proposal.expected_residual.signum() == proposal.excess.signum(),
                    "net {net} beta {beta} price {price}: residual {} flipped past the target \
                     against an excess of {}",
                    proposal.expected_residual,
                    proposal.excess
                );
            }
            HedgeOutcome::NoAction { .. } => {}
            other => panic!("net {net}: unexpected outcome {other:?}"),
        }
    }
}

#[test]
fn a_quantity_below_one_lot_becomes_no_action_rather_than_a_rounded_up_lot() {
    // 40 of exposure at a price of 100 is 0.4 of a one-unit lot. Rounding up
    // to one lot would hedge 100 against 40 — a 60 naked short — so the
    // engine proposes nothing and says why. De-minimis is zero here, so this
    // exercises the lot rounding, not the threshold.
    let outcome = propose(
        &tech_policy("1"),
        &sector_exposure("technology", d("40")),
        d("100"),
    );
    match outcome {
        HedgeOutcome::NoAction {
            residual, detail, ..
        } => {
            assert_eq!(residual, d("40"));
            assert!(detail.contains("less than one"), "detail was: {detail}");
        }
        other => panic!("expected no action below one lot, got {other:?}"),
    }
}

// --- the declared beta ratio ------------------------------------------------

#[test]
fn the_hedge_notional_is_beta_times_the_excess_not_the_excess_divided_by_beta() {
    // With beta 1.25 on 100,000 of exposure, multiplying gives 125,000 and
    // dividing gives 80,000. The proposal must carry the former: beta is
    // declared as hedge notional per unit of exposure.
    let proposal = expect_proposal(propose(
        &tech_policy("1.25"),
        &sector_exposure("technology", d("100000")),
        d("100"),
    ));
    assert_eq!(proposal.hedge_notional, d("125000"));
    assert_eq!(proposal.quantity, d("1250"));

    // And with beta below one the hedge shrinks — 0.5 halves it. Together the
    // two betas bracket a transposed multiply/divide from both sides.
    let proposal = expect_proposal(propose(
        &tech_policy("0.5"),
        &sector_exposure("technology", d("100000")),
        d("100"),
    ));
    assert_eq!(proposal.hedge_notional, d("50000"));
    assert_eq!(proposal.quantity, d("500"));
}

#[test]
fn a_long_excess_is_sold_and_a_short_excess_is_bought_back() {
    let long = expect_proposal(propose(
        &tech_policy("1"),
        &sector_exposure("technology", d("100000")),
        d("100"),
    ));
    assert_eq!(
        long.side,
        HedgeSide::Sell,
        "a long exposure is hedged by selling"
    );

    let short = expect_proposal(propose(
        &tech_policy("1"),
        &sector_exposure("technology", d("-100000")),
        d("100"),
    ));
    assert_eq!(
        short.side,
        HedgeSide::Buy,
        "a short exposure is hedged by buying back"
    );
    assert_eq!(
        short.quantity,
        d("1000"),
        "the size is the same either side of flat"
    );
}

#[test]
fn the_hedge_reduces_toward_a_declared_target_not_necessarily_to_flat() {
    // Net 300,000 with a declared target of 100,000: only the 200,000 excess
    // is hedged. An engine that hedges the whole net would overshoot the
    // governance decision by exactly the target.
    let policy = tech_policy("1").with_target(d("100000"));
    let proposal = expect_proposal(propose(
        &policy,
        &sector_exposure("technology", d("300000")),
        d("100"),
    ));
    assert_eq!(proposal.excess, d("200000"));
    assert_eq!(proposal.hedge_notional, d("200000"));
    assert_eq!(proposal.observed_net, d("300000"));
    assert_eq!(proposal.target_net, d("100000"));
}

#[test]
fn contract_multiplier_scales_the_quantity_down_not_the_notional_up() {
    // A futures-like instrument: multiplier 50 at a price of 100 carries
    // 5,000 of notional per contract, so 100,000 of exposure takes 20
    // contracts, not 1,000.
    let instrument = spy().with_multiplier(d("50"));
    let policy = HedgePolicy::new("tech-hedge", HedgeAxis::Sector, "technology", d("1"))
        .with_instrument(instrument);
    let proposal = expect_proposal(propose(
        &policy,
        &sector_exposure("technology", d("100000")),
        d("100"),
    ));
    assert_eq!(proposal.quantity, d("20"));
    assert_eq!(proposal.hedge_notional, d("100000"));
}

#[test]
fn a_beta_that_was_never_declared_positive_is_refused_not_defaulted() {
    // A zero or negative beta cannot be "close enough to one": the engine
    // refuses rather than estimating or defaulting, because a silent estimate
    // becomes a doubled position when the correlation flips.
    for beta in ["0", "-1.2"] {
        let outcome = propose(
            &tech_policy(beta),
            &sector_exposure("technology", d("100000")),
            d("100"),
        );
        match outcome {
            HedgeOutcome::Refused(HedgeRefusal::MisdeclaredPolicy { detail, .. }) => {
                assert!(detail.contains("beta"), "detail was: {detail}");
            }
            other => panic!("beta {beta}: expected a misdeclared-policy refusal, got {other:?}"),
        }
    }
}

// --- refusals ---------------------------------------------------------------

#[test]
fn a_policy_with_no_hedge_instrument_is_refused_outright() {
    let policy = HedgePolicy::new("tech-hedge", HedgeAxis::Sector, "technology", d("1"));
    assert!(
        policy.instrument.is_none(),
        "the premise is a policy with no instrument"
    );
    let outcome = propose(
        &policy,
        &sector_exposure("technology", d("100000")),
        d("100"),
    );
    match outcome {
        HedgeOutcome::Refused(HedgeRefusal::NoInstrumentDeclared { policy, detail }) => {
            assert_eq!(policy, "tech-hedge");
            assert!(
                detail.contains("no hedge instrument"),
                "detail was: {detail}"
            );
        }
        other => panic!("expected a no-instrument refusal, got {other:?}"),
    }
}

#[test]
fn an_exposure_inside_the_de_minimis_threshold_produces_no_proposal() {
    let policy = tech_policy("1").with_de_minimis(d("50000"));
    let outcome = propose(
        &policy,
        &sector_exposure("technology", d("49999")),
        d("100"),
    );
    match outcome {
        HedgeOutcome::NoAction {
            residual, detail, ..
        } => {
            assert_eq!(residual, d("49999"));
            assert!(detail.contains("de-minimis"), "detail was: {detail}");
        }
        other => panic!("expected no action inside de-minimis, got {other:?}"),
    }

    // One unit past the threshold, the same policy proposes.
    let outcome = propose(
        &policy,
        &sector_exposure("technology", d("50100")),
        d("100"),
    );
    assert!(
        outcome.is_proposal(),
        "past the threshold the policy must act: {outcome:?}"
    );
}

#[test]
fn a_hedge_whose_own_notional_would_breach_a_limit_is_refused_with_both_numbers() {
    // A 100,000 hedge against a 50,000 order-notional limit. The refusal must
    // carry the notional that was refused and the limit it broke, with the
    // observed value and the bound, so the refusal can be re-derived.
    let limits = LimitSet::new("order-cap").with(Limit::new(
        "order-notional",
        LimitKind::MaxOrderNotional {
            limit: Decimal::from_int(50_000),
        },
    ));
    let prices = BTreeMap::from([("SPY".to_string(), d("100"))]);
    let outcome = propose_hedge(
        &tech_policy("1"),
        &sector_exposure("technology", d("100000")),
        &prices,
        &limits,
        &solvent_state(),
        at(),
    );
    match outcome {
        HedgeOutcome::Refused(HedgeRefusal::WouldBreachLimits {
            hedge_notional,
            breaches,
            detail,
            ..
        }) => {
            assert_eq!(
                hedge_notional,
                d("100000"),
                "the first number: what was proposed"
            );
            let breach = breaches
                .first()
                .expect("the breach that bound must be carried");
            assert_eq!(breach.limit_name, "order-notional");
            assert!((breach.observed - 100_000.0).abs() < 1e-6);
            assert!(
                (breach.bound - 50_000.0).abs() < 1e-6,
                "the second number: the bound"
            );
            assert!(
                detail.contains("100000") && detail.contains("50000"),
                "detail was: {detail}"
            );
        }
        other => panic!("expected a limit refusal, got {other:?}"),
    }
}

#[test]
fn the_limit_projection_counts_the_hedge_against_gross_and_position_limits() {
    // Equity 1,000,000, gross already 1,400,000, leverage cap 1.5×: the book
    // has 100,000 of gross headroom. A 200,000 hedge must be refused even
    // though it *reduces* the sector exposure, because gross is projected
    // additively — the venue nets, the limit engine must not assume so.
    let limits = LimitSet::new("leverage")
        .with(Limit::new("leverage", LimitKind::MaxLeverage { limit: 1.5 }).forcing_reduction());
    let state = RiskState {
        equity: Decimal::from_int(1_000_000),
        cash: Decimal::from_int(100_000),
        gross_exposure: Decimal::from_int(1_400_000),
        ..RiskState::default()
    };
    let prices = BTreeMap::from([("SPY".to_string(), d("100"))]);
    let outcome = propose_hedge(
        &tech_policy("1"),
        &sector_exposure("technology", d("200000")),
        &prices,
        &limits,
        &state,
        at(),
    );
    match outcome {
        HedgeOutcome::Refused(HedgeRefusal::WouldBreachLimits { breaches, .. }) => {
            assert_eq!(breaches[0].limit_name, "leverage");
            assert!(
                (breaches[0].observed - 1.6).abs() < 1e-9,
                "gross must be projected additively: (1.4M + 0.2M) / 1M"
            );
        }
        other => panic!("expected a leverage refusal, got {other:?}"),
    }
}

#[test]
fn a_missing_price_for_the_hedge_instrument_is_a_refusal_not_a_guess() {
    let empty_prices: BTreeMap<String, Decimal> = BTreeMap::new();
    let outcome = propose_hedge(
        &tech_policy("1"),
        &sector_exposure("technology", d("100000")),
        &empty_prices,
        &wide_limits(),
        &solvent_state(),
        at(),
    );
    match outcome {
        HedgeOutcome::Refused(HedgeRefusal::UnusablePrice {
            instrument, detail, ..
        }) => {
            assert_eq!(instrument, "SPY");
            assert!(detail.contains("no price"), "detail was: {detail}");
        }
        other => panic!("expected an unusable-price refusal, got {other:?}"),
    }
}

// --- proposals are proposals, and the survey names every policy -------------

#[test]
fn a_proposal_carries_its_reasoning_and_never_an_order_id() {
    let proposal = expect_proposal(propose(
        &tech_policy("1.25"),
        &sector_exposure("technology", d("100000")),
        d("100"),
    ));
    assert!(
        !proposal.reasoning.is_empty(),
        "a proposal must explain itself"
    );
    assert!(
        proposal.reasoning.iter().any(|line| line.contains("1.25")),
        "the declared beta must appear in the reasoning"
    );
    assert!(
        proposal
            .reasoning
            .iter()
            .any(|line| line.contains("proposal")),
        "the reasoning must say this is a proposal, not an order"
    );
    // Deterministic identity: same policy, same time, same id — a replay
    // proposes under the same name.
    assert_eq!(
        proposal.proposal_id,
        format!("hedge-tech-hedge-{}", at().as_nanos())
    );
}

#[test]
fn the_survey_answers_for_every_policy_including_the_refused_ones() {
    let engine = HedgeEngine::new()
        .with(tech_policy("1"))
        .with(HedgePolicy::new(
            "orphan",
            HedgeAxis::Currency,
            "EUR",
            d("1"),
        ))
        .with(tech_policy("1").with_de_minimis(d("500000")));
    let prices = BTreeMap::from([("SPY".to_string(), d("100"))]);
    let outcomes = engine.survey(
        &sector_exposure("technology", d("100000")),
        &prices,
        &wide_limits(),
        &solvent_state(),
        at(),
    );
    assert_eq!(outcomes.len(), 3, "one outcome per policy, never silence");
    assert!(outcomes[0].is_proposal());
    assert!(matches!(
        outcomes[1],
        HedgeOutcome::Refused(HedgeRefusal::NoInstrumentDeclared { .. })
    ));
    assert!(matches!(outcomes[2], HedgeOutcome::NoAction { .. }));
}

#[test]
fn exposures_are_read_along_the_declared_axis_including_per_instrument() {
    // The same book seen along two axes: the currency policy reads the
    // currency bucket, the instrument policy reads the instrument bucket, and
    // neither sees the other's number.
    let mut breakdown = ExposureBreakdown::default();
    breakdown.by_currency.add("EUR", d("-80000"));
    let mut by_instrument = Exposure::new();
    by_instrument.add("AAPL", d("60000"));
    let exposures = HedgeExposures::new(breakdown).with_instruments(by_instrument);

    let fx = HedgePolicy::new("eur", HedgeAxis::Currency, "EUR", d("1")).with_instrument(
        HedgeInstrument::new(ObjectId::from_string("EURUSD"), "EURUSD"),
    );
    let single_name =
        HedgePolicy::new("aapl", HedgeAxis::Instrument, "AAPL", d("1")).with_instrument(spy());
    let prices = BTreeMap::from([
        ("EURUSD".to_string(), d("1.10")),
        ("SPY".to_string(), d("100")),
    ]);

    let fx_outcome = propose_hedge(
        &fx,
        &exposures,
        &prices,
        &wide_limits(),
        &solvent_state(),
        at(),
    );
    let fx_proposal = expect_proposal(fx_outcome);
    assert_eq!(fx_proposal.observed_net, d("-80000"));
    assert_eq!(fx_proposal.side, HedgeSide::Buy);

    let name_outcome = propose_hedge(
        &single_name,
        &exposures,
        &prices,
        &wide_limits(),
        &solvent_state(),
        at(),
    );
    let name_proposal = expect_proposal(name_outcome);
    assert_eq!(name_proposal.observed_net, d("60000"));
    assert_eq!(name_proposal.side, HedgeSide::Sell);
}
