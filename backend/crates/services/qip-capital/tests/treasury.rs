//! Tests for the Capital Brain's own books: floor, partition, leverage,
//! internal funding and financing permissions.

#![allow(clippy::panic_in_result_fn)]

use qip_capital::treasury::{
    CapitalBook, FinancingFunction, FinancingPermissions, InternalFunding, LeverageBook, Use,
};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Timestamp, dec};

const USES: [Use; 4] = [Use::Grant, Use::Placement, Use::Collateral, Use::Yield];

#[test]
fn across_random_commitments_the_floor_holds_every_step_and_the_breaching_one_is_refused_naming_it()
-> Result<()> {
    let floor = dec!("1000");
    let mut book = CapitalBook::new(dec!("10000"), floor)?;
    let mut rng = Xoshiro256::seeded(0xF100_4);
    let (mut accepted, mut refused) = (0, 0);
    for step in 0..400u64 {
        let id = format!("c{step}");
        let amount = Decimal::from_int(1 + rng.below(1500) as i64);
        let before = book.committed();
        match book.commit(&id, USES[rng.below(4) as usize], amount) {
            Ok(()) => accepted += 1,
            Err(e) => {
                refused += 1;
                // The refusal names the floor and changed nothing.
                assert!(e.to_string().contains("floor of 1000"), "{e}");
                assert_eq!(book.committed(), before);
            }
        }
        assert!(book.committed() + floor <= book.total());
        // Free capital now and then so the sweep keeps crossing the floor both ways.
        if step % 7 == 0 {
            let _ = book.release(&format!("c{}", step.saturating_sub(3)));
        }
    }
    // Premise: the sweep exercised both outcomes.
    assert!(accepted > 20 && refused > 20, "{accepted} {refused}");
    Ok(())
}

#[test]
fn a_commitment_is_refused_whole_rather_than_reduced_to_fit_the_floor() -> Result<()> {
    let mut book = CapitalBook::new(dec!("100"), dec!("40"))?;
    // 61 does not fit (39 would remain); the book must not take 60 instead.
    assert!(book.commit("g", Use::Grant, dec!("61")).is_err());
    assert_eq!(book.committed(), Decimal::ZERO);
    book.commit("g", Use::Grant, dec!("60"))?;
    assert_eq!(book.uncommitted(), dec!("40"));
    Ok(())
}

#[test]
fn deployable_capital_divides_exactly_into_the_four_uses_at_every_step() -> Result<()> {
    let mut book = CapitalBook::new(dec!("5000.25"), dec!("0"))?;
    let mut rng = Xoshiro256::seeded(0x21);
    let mut released = 0;
    for step in 0..300u64 {
        let id = format!("c{}", rng.below(40));
        if rng.below(10) < 4 {
            if book.release(&id).is_ok() {
                released += 1;
            }
        } else {
            let _ = book.commit(
                &id,
                USES[rng.below(4) as usize],
                Decimal::from_int(1 + rng.below(300) as i64),
            );
        }
        let by_use = USES
            .iter()
            .fold(Decimal::ZERO, |a, u| a + book.assigned_to(*u));
        assert_eq!(by_use + book.uncommitted(), book.total(), "step {step}");
        assert!(!book.uncommitted().is_negative());
    }
    assert!(released > 10, "the sweep never released anything");
    Ok(())
}

#[test]
fn the_same_commitment_id_cannot_be_assigned_to_two_uses() -> Result<()> {
    let mut book = CapitalBook::new(dec!("100"), dec!("0"))?;
    book.commit("x", Use::Grant, dec!("10"))?;
    assert!(book.commit("x", Use::Yield, dec!("10")).is_err());
    assert_eq!(book.committed(), dec!("10"));
    Ok(())
}

#[test]
fn an_allocation_whose_implied_leverage_exceeds_the_recorded_decision_is_refused() -> Result<()> {
    let mut book = LeverageBook::new();
    // Premise: with no decision, nothing is allowed.
    assert!(
        book.check("mm", "eu", dec!("1"), dec!("1000"))
            .unwrap_err()
            .to_string()
            .contains("no leverage decision")
    );
    book.decide("mm", "eu", dec!("2"), Timestamp::from_secs(10))?;
    book.check("mm", "eu", dec!("2000"), dec!("1000"))?;
    let refusal = book.check("mm", "eu", dec!("2000.01"), dec!("1000"));
    assert!(refusal.is_err());
    // Per strategy and region: another region has no decision of its own.
    assert!(book.check("mm", "us", dec!("1"), dec!("1000")).is_err());
    Ok(())
}

#[test]
fn every_leverage_decision_is_journaled_and_the_latest_is_the_one_in_force() -> Result<()> {
    let mut book = LeverageBook::new();
    book.decide("mm", "eu", dec!("3"), Timestamp::from_secs(1))?;
    book.decide("mm", "eu", dec!("1.5"), Timestamp::from_secs(2))?;
    assert_eq!(book.journal().len(), 2);
    assert_eq!(book.journal()[0].max_leverage, dec!("3"));
    assert!(book.check("mm", "eu", dec!("2000"), dec!("1000")).is_err());
    assert!(
        book.decide("mm", "eu", Decimal::ZERO, Timestamp::from_secs(3))
            .is_err()
    );
    assert_eq!(book.journal().len(), 2);
    Ok(())
}

#[test]
fn every_internal_funding_is_balanced_and_only_a_repayment_extinguishes_it() -> Result<()> {
    let mut f = InternalFunding::new();
    f.fund("f1", "treasury", "alpha", dec!("1000"))?;
    f.fund("f2", "treasury", "beta", dec!("250.5"))?;
    // Premise: obligations exist and are visible on both sides.
    assert_eq!(f.receivable("treasury"), dec!("1250.5"));
    assert_eq!(f.payable("alpha"), dec!("1000"));
    assert!(f.is_balanced());
    // Refusals leave the obligation untouched.
    assert!(f.fund("f1", "treasury", "alpha", dec!("1")).is_err());
    assert!(f.fund("f3", "alpha", "alpha", dec!("1")).is_err());
    assert!(f.repay("f1", dec!("1000.01")).is_err());
    assert!(!f.is_extinguished("f1"));
    assert_eq!(f.repay("f1", dec!("400"))?, dec!("600"));
    assert!(!f.is_extinguished("f1"));
    assert!(f.is_balanced());
    assert_eq!(f.repay("f1", dec!("600"))?, Decimal::ZERO);
    assert!(f.is_extinguished("f1"));
    assert_eq!(f.receivable("treasury"), dec!("250.5"));
    assert!(f.is_balanced());
    Ok(())
}

#[test]
fn with_an_empty_registry_every_financing_function_refuses_and_only_the_recorded_combination_passes()
{
    let mut reg = FinancingPermissions::new();
    for f in FinancingFunction::ALL {
        assert!(reg.require(f, "entity-a", "cp-1", "GB").is_err());
    }
    reg.permit(FinancingFunction::Repo, "entity-a", "cp-1", "GB");
    let mut passed = 0;
    for f in FinancingFunction::ALL {
        for entity in ["entity-a", "entity-b"] {
            for cp in ["cp-1", "cp-2"] {
                for j in ["GB", "US"] {
                    if reg.require(f, entity, cp, j).is_ok() {
                        passed += 1;
                        assert_eq!(
                            (f, entity, cp, j),
                            (FinancingFunction::Repo, "entity-a", "cp-1", "GB")
                        );
                    }
                }
            }
        }
    }
    assert_eq!(passed, 1);
}
