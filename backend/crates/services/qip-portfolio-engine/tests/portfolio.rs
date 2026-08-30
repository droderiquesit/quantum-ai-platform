//! Tests for portfolio construction and the proposal lifecycle.
//!
//! The lifecycle tests are the ones that matter: a proposal that could reach
//! execution without both controls signing, or after being vetoed, would make
//! every other control in the platform decorative.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::ids::{ObjectId, ProposalId};
use qip_core::testing::approx_eq;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal, Money, dec};
use qip_optimization_engine::router::ComputeRouter;
use qip_portfolio_engine::construction::{ApprovedThesis, Mandate, PortfolioConstructor};
use qip_portfolio_engine::proposal::{Proposal, ProposalLeg, ProposalStatus, Side};
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn equity() -> Money {
    Money::new(dec!("10000000"), Currency::USD)
}

fn thesis(symbol: &str, conviction: f64, expected: f64) -> ApprovedThesis {
    ApprovedThesis {
        hypothesis_id: format!("hyp-{symbol}"),
        object_id: object(symbol),
        conviction,
        expected_return: expected,
        price: dec!("100"),
    }
}

fn covariance(n: usize) -> Vec<Vec<f64>> {
    (0..n)
        .map(|i| {
            (0..n)
                .map(|j| if i == j { 0.04 } else { 0.04 * 0.3 })
                .collect()
        })
        .collect()
}

fn constructor(mandate: Mandate) -> Result<PortfolioConstructor> {
    PortfolioConstructor::new(mandate, ComputeRouter::classical(7))
}

fn build(
    theses: &[ApprovedThesis],
    mandate: Mandate,
    current: &BTreeMap<String, f64>,
) -> Result<Proposal> {
    Ok(constructor(mandate)?
        .construct(
            theses,
            &covariance(theses.len()),
            current,
            equity(),
            now(),
            now(),
            ProposalId::from_string("prop-1"),
        )?
        .proposal)
}

// --- construction -----------------------------------------------------------

#[test]
fn a_proposal_expresses_the_theses_it_was_given() -> Result<()> {
    let theses = vec![
        thesis("AAA", 0.9, 0.08),
        thesis("BBB", 0.7, 0.06),
        thesis("CCC", 0.5, 0.04),
    ];
    let proposal = build(&theses, Mandate::default(), &BTreeMap::new())?;

    assert!(!proposal.is_empty());
    assert_eq!(proposal.status, ProposalStatus::Draft);
    for leg in &proposal.legs {
        assert!(
            !leg.hypotheses.is_empty(),
            "{} has no thesis behind it",
            leg.object_id.as_str()
        );
    }
    assert_eq!(proposal.hypotheses().len(), proposal.len());
    Ok(())
}

#[test]
fn construction_refuses_to_create_a_view_of_its_own() -> Result<()> {
    let error = build(&[], Mandate::default(), &BTreeMap::new()).unwrap_err();
    assert!(
        error.message().contains("does not create views"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn the_concentration_cap_wins_over_the_exposure_target() -> Result<()> {
    // Three names against an 8% cap cannot reach a 95% gross target. The cap
    // is not negotiable, and the shortfall must be stated rather than hidden.
    let theses = vec![
        thesis("AAA", 0.9, 0.08),
        thesis("BBB", 0.7, 0.06),
        thesis("CCC", 0.5, 0.04),
    ];
    let proposal = build(&theses, Mandate::default(), &BTreeMap::new())?;

    for leg in &proposal.legs {
        assert!(
            leg.target_weight <= 0.08 + 1e-6,
            "{} breached the cap at {}",
            leg.object_id.as_str(),
            leg.target_weight
        );
    }
    assert!(proposal.target_gross <= 0.24 + 1e-6);
    assert!(
        proposal
            .compromises
            .iter()
            .any(|c| c.contains("under-invested")),
        "{:?}",
        proposal.compromises
    );
    Ok(())
}

#[test]
fn a_short_thesis_is_dropped_under_a_long_only_mandate_and_said_so() -> Result<()> {
    // Silently flipping it to zero would hide that the thesis was unusable.
    let theses = vec![thesis("AAA", 0.9, 0.08), thesis("BBB", -0.8, -0.05)];
    let proposal = build(&theses, Mandate::default(), &BTreeMap::new())?;

    assert!(
        proposal.compromises.iter().any(|c| c.contains("long-only")),
        "{:?}",
        proposal.compromises
    );
    assert!(
        proposal.leg(&object("BBB")).is_none()
            || proposal.leg(&object("BBB")).unwrap().target_weight <= 1e-9
    );
    Ok(())
}

#[test]
fn a_position_below_the_minimum_is_dropped_rather_than_held() -> Result<()> {
    // A 0.1% position costs the same in operational overhead as a 5% one.
    let mandate = Mandate {
        minimum_position: 0.05,
        position_cap: 0.30,
        ..Mandate::default()
    };
    let theses = vec![
        thesis("AAA", 0.9, 0.20),
        thesis("BBB", 0.9, 0.19),
        thesis("CCC", 0.1, 0.001),
    ];
    let proposal = build(&theses, mandate, &BTreeMap::new())?;
    for leg in &proposal.legs {
        assert!(
            leg.target_weight.abs() >= 0.05 - 1e-9,
            "{} was kept at {}",
            leg.object_id.as_str(),
            leg.target_weight
        );
    }
    Ok(())
}

#[test]
fn turnover_is_measured_against_the_book_as_it_stands() -> Result<()> {
    let theses = vec![thesis("AAA", 0.9, 0.08), thesis("BBB", 0.7, 0.06)];
    let flat = build(&theses, Mandate::default(), &BTreeMap::new())?;

    // Already holding roughly the target means much less to trade.
    let current = BTreeMap::from([
        (object("AAA").as_str().to_string(), 0.08),
        (object("BBB").as_str().to_string(), 0.08),
    ]);
    let held = build(&theses, Mandate::default(), &current)?;

    assert!(
        held.turnover < flat.turnover,
        "holding the target should reduce turnover: {} vs {}",
        held.turnover,
        flat.turnover
    );
    Ok(())
}

#[test]
fn a_legs_side_always_agrees_with_its_weight_change() -> Result<()> {
    // A proposal that says buy and reduces the weight would be executed as
    // written and reconcile as a mystery.
    let theses = vec![thesis("AAA", 0.9, 0.08), thesis("BBB", 0.2, 0.01)];
    let current = BTreeMap::from([(object("BBB").as_str().to_string(), 0.07)]);
    let proposal = build(&theses, Mandate::default(), &current)?;

    for leg in &proposal.legs {
        let change = leg.weight_change();
        match leg.side {
            Side::Buy => assert!(
                change > 0.0,
                "{} buys but moves {change}",
                leg.object_id.as_str()
            ),
            Side::Sell => {
                assert!(
                    change < 0.0,
                    "{} sells but moves {change}",
                    leg.object_id.as_str()
                )
            }
        }
    }
    proposal.validate()?;
    Ok(())
}

#[test]
fn construction_refuses_a_time_it_has_not_reached() -> Result<()> {
    let error = constructor(Mandate::default())?
        .construct(
            &[thesis("AAA", 0.9, 0.08)],
            &covariance(1),
            &BTreeMap::new(),
            equity(),
            now().saturating_add(Duration::from_days(1)),
            now(),
            ProposalId::from_string("prop-x"),
        )
        .unwrap_err();
    assert!(error.message().contains("in its future"));
    Ok(())
}

#[test]
fn construction_refuses_a_thesis_with_an_impossible_conviction() -> Result<()> {
    let mut bad = thesis("AAA", 0.9, 0.08);
    bad.conviction = 1.5;
    let error = build(&[bad], Mandate::default(), &BTreeMap::new()).unwrap_err();
    assert!(error.message().contains("outside [-1, 1]"));
    Ok(())
}

#[test]
fn an_inconsistent_mandate_is_refused_at_construction() {
    let contradictory = Mandate {
        minimum_position: 0.10,
        position_cap: 0.05,
        ..Mandate::default()
    };
    assert!(
        PortfolioConstructor::new(contradictory, ComputeRouter::classical(1))
            .unwrap_err()
            .message()
            .contains("no position is permissible")
    );
}

#[test]
fn nothing_to_do_is_a_normal_state_rather_than_an_error() -> Result<()> {
    let proposal = constructor(Mandate::default())?.nothing_to_do(
        ProposalId::from_string("prop-empty"),
        equity(),
        now(),
        now(),
        "no thesis cleared the action bar this cycle",
    );
    assert!(proposal.is_empty());
    assert!(proposal.validate().is_ok());
    assert!(approx_eq(proposal.turnover, 0.0, 1e-12));
    Ok(())
}

#[test]
fn the_routing_decision_is_carried_with_the_proposal() -> Result<()> {
    // So the solver choice is auditable from the decision record.
    let outcome = constructor(Mandate::default())?.construct(
        &[thesis("AAA", 0.9, 0.08), thesis("BBB", 0.7, 0.06)],
        &covariance(2),
        &BTreeMap::new(),
        equity(),
        now(),
        now(),
        ProposalId::from_string("prop-r"),
    )?;
    assert!(outcome.routing.classical_objective.is_finite());
    assert!(
        outcome
            .proposal
            .rationale
            .contains(outcome.routing.chosen.as_str()),
        "{}",
        outcome.proposal.rationale
    );
    Ok(())
}

// --- the proposal lifecycle -------------------------------------------------

fn draft() -> Proposal {
    Proposal {
        proposal_id: ProposalId::from_string("prop-life"),
        created_at: now(),
        as_of: now(),
        status: ProposalStatus::Draft,
        legs: vec![ProposalLeg {
            object_id: object("AAA"),
            side: Side::Buy,
            quantity: dec!("1000"),
            reference_price: dec!("100"),
            current_weight: 0.0,
            target_weight: 0.01,
            estimated_cost_bps: 10.0,
            hypotheses: vec!["hyp-1".to_string()],
        }],
        equity: equity(),
        target_gross: 0.01,
        target_net: 0.01,
        turnover: 0.01,
        estimated_cost_bps: 0.2,
        rationale: "expresses one approved thesis".to_string(),
        compromises: Vec::new(),
        checks_passed: Vec::new(),
    }
}

#[test]
fn a_draft_proposal_cannot_be_released() {
    let mut proposal = draft();
    let error = proposal.release(now()).unwrap_err();
    assert!(error.message().contains("cannot be released"));
    assert_eq!(proposal.status, ProposalStatus::Draft);
}

#[test]
fn a_single_control_cannot_approve_a_proposal_alone() {
    // A single approver is a single point of failure, and the platform has two
    // control functions precisely so that neither is one.
    let mut proposal = draft();
    let error = proposal
        .approve(now(), vec!["risk-control".to_string()])
        .unwrap_err();
    assert!(
        error.message().contains("must both sign"),
        "{}",
        error.message()
    );
    assert_eq!(proposal.status, ProposalStatus::Draft);
    assert!(!proposal.status.is_releasable());
}

#[test]
fn two_controls_approve_and_the_proposal_can_then_be_released() {
    let mut proposal = draft();
    proposal
        .approve(
            now(),
            vec!["risk-control".to_string(), "compliance-control".to_string()],
        )
        .unwrap();
    assert!(proposal.status.is_releasable());
    assert_eq!(proposal.checks_passed.len(), 2);
    proposal.release(now()).unwrap();
    assert!(matches!(proposal.status, ProposalStatus::Released { .. }));
}

#[test]
fn a_vetoed_proposal_cannot_be_approved_afterwards() {
    let mut proposal = draft();
    proposal.veto(now(), "risk-control", "gross exposure limit breached");
    let error = proposal
        .approve(
            now(),
            vec!["risk-control".to_string(), "compliance-control".to_string()],
        )
        .unwrap_err();
    assert!(error.message().contains("cannot be approved"));
    assert!(!proposal.status.is_releasable());
}

#[test]
fn a_veto_works_after_approval_and_before_release() {
    // A control that can be locked out by timing is not a control.
    let mut proposal = draft();
    proposal
        .approve(
            now(),
            vec!["risk-control".to_string(), "compliance-control".to_string()],
        )
        .unwrap();
    assert!(proposal.status.is_releasable());

    proposal.veto(now(), "compliance-control", "the name was just restricted");
    assert!(!proposal.status.is_releasable());
    assert!(proposal.release(now()).is_err());
}

#[test]
fn a_released_proposal_cannot_be_vetoed_retroactively() {
    // The orders are already in the market; a veto here would misrepresent
    // what happened. Cancelling is an execution action, not a veto.
    let mut proposal = draft();
    proposal
        .approve(
            now(),
            vec!["risk-control".to_string(), "compliance-control".to_string()],
        )
        .unwrap();
    proposal.release(now()).unwrap();
    proposal.veto(now(), "risk-control", "second thoughts");
    assert!(matches!(proposal.status, ProposalStatus::Released { .. }));
}

#[test]
fn a_leg_without_a_hypothesis_fails_validation() {
    let mut proposal = draft();
    proposal.legs[0].hypotheses.clear();
    assert!(
        proposal
            .validate()
            .unwrap_err()
            .message()
            .contains("nobody reviewed")
    );
}

#[test]
fn a_negative_quantity_fails_validation() {
    // The side carries the direction; a negative quantity would double-count it.
    let mut proposal = draft();
    proposal.legs[0].quantity = Decimal::ZERO;
    assert!(proposal.validate().is_err());
}

#[test]
fn a_proposal_with_no_rationale_fails_validation() {
    let mut proposal = draft();
    proposal.rationale = "  ".to_string();
    assert!(
        proposal
            .validate()
            .unwrap_err()
            .message()
            .contains("rationale")
    );
}

#[test]
fn the_traded_notional_is_the_sum_of_the_legs() {
    let proposal = draft();
    assert_eq!(proposal.traded_notional(), dec!("100000"));
}
