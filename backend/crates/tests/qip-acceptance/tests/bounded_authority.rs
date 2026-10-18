//! Discovery, research, training, promotion, deployment and allocation happen
//! only inside bounded authority.
//!
//! R12-GOV-1017. Each activity has a bound of its own, and each bound had a
//! test of its own in its own crate. Nothing drove them side by side, so a
//! bound removed from one activity left the others green and the register
//! reading "all bounded". This suite drives the four bounds that carry the
//! claim and requires each to refuse:
//!
//! * research: the agent budget ledger refuses the call that would exceed it
//!   (`qip_agents::budget`);
//! * promotion: a strategy with no evidence is not promoted, and a
//!   capital-holding rung needs a dual approval
//!   (`qip_lifecycle::ledger::attempt_promotion`, `AuthorisedPromotion`);
//! * allocation and deployment: a verified capital envelope refuses an order
//!   that does not fit whole, an expired grant, a foreign cell and an
//!   unverified signature (`qip_edge::envelope`);
//! * the live ceiling: no configured level above paper is deployable
//!   (`qip_risk_engine::autonomy::AutonomyLevel::deployable`).

#![allow(clippy::panic_in_result_fn)]

use qip_agents::budget::{Budget, BudgetLedger};
use qip_contracts::capital::{CapitalAdmission, CapitalEnvelope, Utilisation};
use qip_contracts::gate::GateStage;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Timestamp, dec};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_lifecycle::evidence::StrategyEvidence;
use qip_lifecycle::ledger::{AuthorisedPromotion, LifecycleLedger, attempt_promotion};
use qip_risk_engine::autonomy::AutonomyLevel;

const KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const CELL: &str = "london-1";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_700_000_000 + secs)
}

fn signed_envelope(cell: &str, gross: &str, order: &str) -> Result<CapitalEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("mean-reversion-1"),
            cell,
            Decimal::parse(gross).expect("a decimal literal"),
            Decimal::parse(order).expect("a decimal literal"),
            dec!("50000"),
            vec![VenueId::new("XLON")],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    build(&sign_payload(KEY, &unsigned.signing_payload()))
}

#[test]
fn research_is_bounded_by_the_budget_ledger() {
    let budget = Budget::research_default();
    assert!(
        budget.tool_calls > 0,
        "premise: the research budget allows some calls, or the refusal proves nothing"
    );
    let mut ledger = BudgetLedger::new(budget, "macro");
    for _ in 0..budget.tool_calls {
        ledger
            .charge_tool_call()
            .expect("a call inside the budget is admitted");
    }
    assert!(
        ledger.charge_tool_call().is_err(),
        "the call past the budget was admitted"
    );
    assert!(!ledger.has_headroom());
}

#[test]
fn promotion_needs_evidence_and_a_capital_holding_rung_needs_two_approvers() -> Result<()> {
    let strategy = StrategyId::new("momentum-v3");
    let mut ledger = LifecycleLedger::new();
    let before = ledger.stage_of(&strategy);
    assert_eq!(
        before,
        GateStage::Candidate,
        "premise: a new strategy is a candidate"
    );
    let empty = StrategyEvidence::default();
    assert!(
        attempt_promotion(&mut ledger, &strategy, &empty, None, "no evidence", t(0)).is_err(),
        "a candidate with no evidence was promoted"
    );
    assert_eq!(
        ledger.stage_of(&strategy),
        before,
        "a refused promotion moved the strategy"
    );
    // The rung that holds capital cannot be reached without a dual approval.
    let mut approval_gated = 0usize;
    for stage in [GateStage::Shadow, GateStage::Pilot] {
        if stage
            .next()
            .is_some_and(|next| next.requires_human_approval())
        {
            approval_gated += 1;
            assert!(
                AuthorisedPromotion::advance(stage, None, t(0)).is_err(),
                "a capital-holding rung was reached with no approval"
            );
        }
    }
    assert!(
        approval_gated > 0,
        "no rung needed an approval, so the loop proved nothing"
    );
    Ok(())
}

#[test]
fn allocation_is_bounded_by_a_verified_capital_envelope() -> Result<()> {
    let verified =
        VerifiedEnvelope::verify(signed_envelope(CELL, "1000", "400")?, KEY, CELL, t(10))?;
    let venue = VenueId::new("XLON");
    let used = Utilisation {
        gross_committed: dec!("800"),
        realised_loss: Decimal::ZERO,
        orders_sent: 2,
    };
    assert!(
        matches!(
            verified.admit(&venue, dec!("200"), &used, t(10)),
            CapitalAdmission::Full
        ),
        "premise: an order that fits the headroom is admitted"
    );
    assert!(
        verified
            .admit(&venue, dec!("400"), &used, t(10))
            .is_refused(),
        "an order past the headroom was not refused"
    );
    assert!(
        verified
            .admit(
                &VenueId::new("XNYS"),
                dec!("10"),
                &Utilisation::default(),
                t(10)
            )
            .is_refused(),
        "an order at a venue outside the grant was not refused"
    );
    // The grant is only as good as its provenance and its window.
    assert!(
        VerifiedEnvelope::verify(signed_envelope("tokyo-1", "1000", "400")?, KEY, CELL, t(10))
            .is_err(),
        "an envelope for another cell was accepted"
    );
    assert!(
        VerifiedEnvelope::verify(signed_envelope(CELL, "1000", "400")?, b"", CELL, t(10)).is_err(),
        "an envelope verified against an empty key"
    );
    assert!(
        VerifiedEnvelope::verify(signed_envelope(CELL, "1000", "400")?, KEY, CELL, t(4000))
            .is_err(),
        "an expired envelope was accepted"
    );
    Ok(())
}

#[test]
fn deployment_is_bounded_by_the_paper_ceiling() {
    let live: Vec<AutonomyLevel> = AutonomyLevel::all()
        .into_iter()
        .filter(AutonomyLevel::is_live)
        .collect();
    assert!(!live.is_empty(), "premise: the ladder has live rungs");
    for level in live {
        assert!(
            AutonomyLevel::deployable(Some(level.as_str())).is_err(),
            "a live ceiling was deployable"
        );
    }
    assert!(
        AutonomyLevel::deployable(Some("paper_trading")).is_ok(),
        "premise: paper trading deploys, or the refusals prove nothing"
    );
}
