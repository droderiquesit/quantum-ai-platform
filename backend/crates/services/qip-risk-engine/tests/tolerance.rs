//! The pre-trade check under a risk tolerance (ADR 0405): the limit in force
//! is the operator's ceiling narrowed by the desk mandate's tolerance, and an
//! order is checked against that.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{Decimal, dec};
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};
use qip_risk_engine::pretrade::{PreTradeChecker, PreTradeDecision, ProposedOrder};
use std::collections::BTreeMap;

fn now() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

/// The operator's ceiling: three maxima and one minimum.
fn ceiling() -> LimitSet {
    LimitSet::new("desk")
        .with(Limit::new(
            "max-position-weight",
            LimitKind::MaxPositionWeight { limit: 0.10 },
        ))
        .with(Limit::new(
            "max-leverage",
            LimitKind::MaxLeverage { limit: 1.5 },
        ))
        .with(Limit::new(
            "max-order-notional",
            LimitKind::MaxOrderNotional {
                limit: dec!("2000000"),
            },
        ))
        .with(Limit::new(
            "cash-buffer",
            LimitKind::MinCashBuffer { limit: 0.02 },
        ))
}

/// Ten million of equity, all of it cash, nothing held.
fn flat() -> RiskState {
    RiskState {
        equity: dec!("10000000"),
        cash: dec!("10000000"),
        ..RiskState::default()
    }
}

fn order(quantity: &str) -> ProposedOrder {
    ProposedOrder {
        object_id: ObjectId::from_string("obj-AAA".to_string()),
        quantity: Decimal::parse(quantity).unwrap(),
        reference_price: dec!("100"),
        axes: BTreeMap::new(),
        scope: "momentum".to_string(),
    }
}

// The test decision 30 makes the bar for the whole change: the narrowing can
// fire. A tolerance that reached the limit set and refused nothing the
// ceiling admits would be `MaxExpectedShortfall` again, a control that reads
// as protection and cannot trigger.
#[test]
fn a_narrowed_limit_refuses_an_order_that_the_ceiling_alone_admits() -> Result<()> {
    // 7,000 at 100 is 700,000: seven per cent of equity, inside the
    // operator's ten per cent and outside half of it.
    let seven_per_cent = order("7000");

    let at_the_ceiling = PreTradeChecker::new(ceiling()).check(&seven_per_cent, &flat(), now())?;
    assert!(
        at_the_ceiling.is_approved(),
        "the premise: the ceiling alone admits it — {}",
        at_the_ceiling.decision.describe()
    );

    let narrowed = PreTradeChecker::under_tolerance(&ceiling(), dec!("0.5"))?;
    let result = narrowed.check(&seven_per_cent, &flat(), now())?;
    let PreTradeDecision::Rejected { reasons } = &result.decision else {
        panic!(
            "half the tolerance must refuse it: {}",
            result.decision.describe()
        );
    };
    // Refused by the position weight and by nothing else: leverage in force
    // is 0.75 and the order cap in force is 1,000,000.
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    let reason = &reasons[0];
    assert!(reason.starts_with("max-position-weight: "), "{reason}");
    // A person can say why: the three figures are in the refusal itself.
    for needle in [
        "the limit in force, 0.05,",
        "ceiling of 0.1 ",
        "risk tolerance of 0.5 ",
    ] {
        assert!(
            reason.contains(needle),
            "{needle:?} missing from {reason:?}"
        );
    }
    Ok(())
}

// "Do not change what any existing limit set allows when the tolerance is 1."
// The whole result is compared, not only whether it was approved: the checks
// run, the breaches, their text and their severities.
#[test]
fn a_tolerance_of_one_reproduces_the_ceilings_verdict_on_every_order() -> Result<()> {
    let at_the_ceiling = PreTradeChecker::new(ceiling());
    let at_one = PreTradeChecker::under_tolerance(&ceiling(), Decimal::ONE)?;
    assert_eq!(at_one.limits(), at_the_ceiling.limits());

    let mut approved = 0;
    let mut refused = 0;
    // From a small order, through the warning band (nine per cent), to past
    // the position weight, the order cap and leverage.
    for quantity in ["100", "7000", "9000", "15000", "25000", "200000", "-7000"] {
        let expected = at_the_ceiling.check(&order(quantity), &flat(), now())?;
        let actual = at_one.check(&order(quantity), &flat(), now())?;
        assert_eq!(actual, expected, "quantity {quantity}");
        if expected.is_approved() {
            approved += 1;
        } else {
            refused += 1;
        }
    }
    assert!(
        approved > 0 && refused > 0,
        "the comparison must cover both verdicts: {approved} approved, {refused} refused"
    );
    Ok(())
}

// ADR 0405 D6. A desk that tolerates no risk is sent no order, by the same
// arithmetic and with no separate halt to keep in step with it.
#[test]
fn a_tolerance_of_zero_refuses_every_order() -> Result<()> {
    let at_the_ceiling = PreTradeChecker::new(ceiling());
    let at_zero = PreTradeChecker::under_tolerance(&ceiling(), Decimal::ZERO)?;
    // The smallest order the type can carry, an ordinary one, and a sale.
    for quantity in ["0.000001", "1", "7000", "-1"] {
        assert!(
            at_the_ceiling
                .check(&order(quantity), &flat(), now())?
                .is_approved(),
            "the premise: the ceiling admits {quantity}"
        );
        let result = at_zero.check(&order(quantity), &flat(), now())?;
        assert!(
            matches!(result.decision, PreTradeDecision::Rejected { .. }),
            "{quantity}: {}",
            result.decision.describe()
        );
        assert!(
            result.decision.describe().contains("max-order-notional"),
            "{quantity}: {}",
            result.decision.describe()
        );
    }
    Ok(())
}

// Principle 2 and 3. A tolerance nobody can read must stop the checker being
// built; falling back to the ceiling would be the loosest answer available.
#[test]
fn a_tolerance_outside_zero_to_one_builds_no_checker() {
    for tolerance in [dec!("1.5"), dec!("-0.2")] {
        let error = PreTradeChecker::under_tolerance(&ceiling(), tolerance)
            .expect_err("no checker is built on an invalid tolerance");
        assert!(
            error.message().contains("must be a fraction in [0, 1]"),
            "{tolerance}: {}",
            error.message()
        );
    }
}

// A floor multiplied by a fraction is a looser floor. The cash buffer in
// force under any tolerance is the operator's two per cent.
#[test]
fn a_narrowed_checker_still_refuses_at_the_operators_cash_floor() -> Result<()> {
    // 9,850,000 spent of 10,000,000 leaves 1.5 per cent in cash.
    let nearly_all_cash = order("98500");
    let floor_only = LimitSet::new("floor").with(Limit::new(
        "cash-buffer",
        LimitKind::MinCashBuffer { limit: 0.02 },
    ));
    let at_the_ceiling = PreTradeChecker::new(floor_only.clone());
    let narrowed = PreTradeChecker::under_tolerance(&floor_only, dec!("0.5"))?;
    assert_eq!(narrowed.limits(), &floor_only);
    let expected = at_the_ceiling.check(&nearly_all_cash, &flat(), now())?;
    assert!(
        !expected.is_approved(),
        "the premise: the operator's floor refuses it — {}",
        expected.decision.describe()
    );
    assert_eq!(narrowed.check(&nearly_all_cash, &flat(), now())?, expected);
    Ok(())
}
