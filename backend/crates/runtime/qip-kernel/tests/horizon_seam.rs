//! The §23.4 capital-pool arithmetic **at the seam the lifecycle veto crosses**,
//! not at the accessor beside it.
//!
//! `qip-optimization-engine`'s own suite proves `reconcile` and
//! `HorizonRegister`; `qip-lifecycle`'s proves what `HorizonAssurance` refuses
//! given figures. Neither can prove the thing that actually decides a
//! promotion, because it is exactly the join between them — and the join is in
//! this crate on purpose.
//!
//! # Why the join is here rather than an edge between the two
//!
//! `qip-lifecycle` vetoes promotions to rungs that hold capital.
//! `qip-optimization-engine` transitively reaches `qip-quantum`. A veto crate
//! that can reach a solver is a veto whose input is an optimiser's answer, and
//! `qip-acceptance`'s
//! `nothing_that_vetoes_executes_or_moves_money_can_reach_a_quantum_solver`
//! refuses it transitively. That is not hypothetical: `qip-lifecycle` declared
//! `qip-optimization-engine` as a dependency to reach this arithmetic, the
//! acceptance suite failed, and the test's own comment had predicted the route
//! — "not a direct edge to `qip-quantum`, which a reviewer would question, but
//! an edge to `qip-optimization-engine`, which looks entirely reasonable on a
//! risk crate until you notice what it drags behind it."
//!
//! So the arithmetic still exists exactly once, and `qip-lifecycle` reaches it
//! through `HorizonReconciler`, whose adapter is
//! [`qip_kernel::central::horizon::PoolReconciler`]. What is tested here is the
//! adapter *and the gate together*: real pools, real reconciliation, real
//! refusal.
//!
//! # What is deliberately not claimed
//!
//! No composition root constructs a `PoolReconciler` yet, so in a deployment
//! this seam is wired and unfed. These tests prove the seam is correct, not
//! that anything runs it.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::signal::StrategyId;
use qip_core::error::Result;
use qip_core::{Decimal, dec};
use qip_kernel::central::horizon::PoolReconciler;
use qip_lifecycle::horizon::{HorizonAssurance, HorizonBucket, HorizonReconciler};
use qip_lifecycle::ledger::LifecycleLedger;
use qip_optimization_engine::horizons::{CapitalPools, Horizon};
use std::collections::BTreeSet;
use std::sync::Arc;

fn candidate() -> StrategyId {
    StrategyId::new("momentum-v3")
}

fn incumbent() -> StrategyId {
    StrategyId::new("carry-v1")
}

/// Four pools summing exactly to the total. The deployable pool is deliberately
/// the tight one, so a breach can be arranged there without disturbing the sum.
fn pools(deployable: Decimal, remainder_to_reserved: Decimal) -> Result<CapitalPools> {
    CapitalPools::new(
        dec!("4000000"),
        dec!("1000000"),
        deployable,
        dec!("1000000"),
        remainder_to_reserved,
        dec!("0"),
    )
}

/// Pools with room everywhere: one million in each of the four.
fn roomy() -> Result<CapitalPools> {
    pools(dec!("1000000"), dec!("1000000"))
}

fn assurance(reconciler: PoolReconciler) -> HorizonAssurance {
    HorizonAssurance::new(Arc::new(reconciler))
}

#[test]
fn the_seam_reports_every_bucket_with_the_pool_and_the_commitment_the_reconciliation_computed()
-> Result<()> {
    // The gate reads `committed > pool` itself, so the seam has to hand it
    // every bucket rather than only the candidate's and only the breached
    // ones. A seam reporting the candidate's bucket alone would leave the
    // gate structurally unable to see a breach anywhere else on the book.
    let mut reconciler = PoolReconciler::new(roomy()?);
    reconciler.claim(&candidate(), "research-enrolment", Horizon::HoursToDays)?;
    reconciler.budget(&candidate(), dec!("250000"))?;

    let funding = reconciler.fund(&candidate(), &BTreeSet::new())?;

    // Premise first: the settlement really did name a bucket, so the standings
    // below are a reconciliation and not an empty report.
    assert_eq!(
        funding.bucket,
        Some(HorizonBucket::new("hours_to_days")?),
        "the settled bucket must be the one the only source claimed"
    );
    assert!(funding.disagreements.is_empty(), "nothing was disputed");

    let names: Vec<&str> = funding
        .standings
        .iter()
        .map(|standing| standing.bucket.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "microseconds_to_minutes",
            "hours_to_days",
            "weeks_to_months",
            "years"
        ],
        "every §23.4 bucket is reported, most liquid first, whether or not \
         anything is budgeted at it"
    );

    let deployable = funding
        .standings
        .iter()
        .find(|standing| standing.bucket.as_str() == "hours_to_days")
        .expect("the candidate's own bucket is reported");
    assert_eq!(deployable.pool, dec!("1000000"));
    assert_eq!(deployable.committed, dec!("250000"));
    assert_eq!(deployable.headroom()?, dec!("750000"));
    assert!(
        deployable.treatment.contains("deployable capital"),
        "the treatment must name the pool the blueprint means, so a refusal can \
         say which one was short: {}",
        deployable.treatment
    );
    Ok(())
}

#[test]
fn a_budget_that_fits_alone_and_not_beside_the_book_is_refused_once_the_gate_measures_both()
-> Result<()> {
    // The `MaxExpectedShortfall` shape, avoided. A pool is only ever breached
    // by the sum, so a gate that reconciled the candidate on its own would
    // admit every promotion individually and overdraw the book collectively —
    // a limit that reads as protection and cannot fire.
    let mut reconciler = PoolReconciler::new(pools(dec!("500000"), dec!("1500000"))?);
    reconciler.claim(&candidate(), "research-enrolment", Horizon::HoursToDays)?;
    reconciler.budget(&candidate(), dec!("300000"))?;
    reconciler.claim(&incumbent(), "research-enrolment", Horizon::HoursToDays)?;
    reconciler.budget(&incumbent(), dec!("300000"))?;

    // The premise, and the half that makes the refusal below meaningful: on its
    // own the candidate fits inside the deployable pool.
    let alone = reconciler.fund(&candidate(), &BTreeSet::new())?;
    assert!(
        !alone
            .standings
            .iter()
            .any(qip_lifecycle::horizon::HorizonStanding::is_breached),
        "300000 against a pool of 500000 must fit on its own, or this test is \
         measuring the wrong thing"
    );

    let book: BTreeSet<StrategyId> = [incumbent()].into_iter().collect();
    let together = reconciler.fund(&candidate(), &book)?;
    let deployable = together
        .standings
        .iter()
        .find(|standing| standing.bucket.as_str() == "hours_to_days")
        .expect("the contested bucket is reported");
    assert_eq!(
        deployable.committed,
        dec!("600000"),
        "both budgets are charged to the pool, not just the candidate's"
    );
    assert!(deployable.is_breached());
    Ok(())
}

#[test]
fn the_gate_over_the_real_pools_refuses_an_over_committed_promotion_rather_than_trimming_it()
-> Result<()> {
    // Trimming would be the platform quietly choosing which strategy goes
    // unfunded at the moment the desk most needs to make that choice. Driven
    // through `HorizonAssurance::admit` — the real gate over the real
    // arithmetic — rather than through either half alone, because the seam is
    // what neither crate's own suite can see.
    let mut reconciler = PoolReconciler::new(pools(dec!("100000"), dec!("1900000"))?);
    reconciler.claim(&candidate(), "research-enrolment", Horizon::HoursToDays)?;
    reconciler.budget(&candidate(), dec!("250000"))?;

    let ledger = LifecycleLedger::new();
    let error = assurance(reconciler)
        .admit(&ledger, &candidate())
        .expect_err("an over-committed pool must refuse");

    assert_eq!(error.code(), "denied", "{error:?}");
    let message = error.to_string();
    assert!(
        message.contains("momentum-v3"),
        "the refusal must name the candidate, so it reads as a decision about \
         this promotion rather than a standing complaint: {message}"
    );
    assert!(
        message.contains("hours_to_days is over its pool of 100000 by 150000"),
        "the refusal must name the bucket, its pool and the overdraft: {message}"
    );
    assert!(
        message.contains("will not trim them for you"),
        "the refusal must say it declined to trim: {message}"
    );
    Ok(())
}

#[test]
fn the_gate_over_the_real_pools_admits_a_promotion_that_fits() -> Result<()> {
    // The other half, without which the test above proves only that something
    // refuses. A gate that refused every promotion would satisfy every
    // assertion in this file except this one.
    let mut reconciler = PoolReconciler::new(roomy()?);
    reconciler.claim(&candidate(), "research-enrolment", Horizon::HoursToDays)?;
    reconciler.budget(&candidate(), dec!("250000"))?;

    let ledger = LifecycleLedger::new();
    let verdict = assurance(reconciler).admit(&ledger, &candidate())?;

    assert_eq!(verdict.horizon, HorizonBucket::new("hours_to_days")?);
    assert_eq!(verdict.headroom, dec!("750000"));
    assert!(
        !verdict.decided_over_disagreement(),
        "nothing was disputed, so the record must not claim a controversy"
    );
    assert_eq!(verdict.despite, None);
    Ok(())
}

#[test]
fn a_disputed_register_crosses_the_seam_as_the_argument_it_is_and_the_gate_refuses_it() -> Result<()>
{
    // The refusal is taken by the gate, on figures, not relayed from the
    // adapter. So the seam reports the disagreement rather than raising it:
    // both sides travel, and the crate that vetoes decides.
    let mut reconciler = PoolReconciler::new(roomy()?);
    reconciler.claim(&candidate(), "research-enrolment", Horizon::HoursToDays)?;
    reconciler.claim(&candidate(), "liquidity-model", Horizon::Years)?;
    reconciler.budget(&candidate(), dec!("250000"))?;

    // Premise: the adapter really did hand the argument over as data rather
    // than as an error. Were this an `Err`, the gate below would be refusing
    // something it never saw.
    let funding = reconciler.fund(&candidate(), &BTreeSet::new())?;
    assert_eq!(funding.disagreements.len(), 1);
    assert_eq!(
        funding.bucket, None,
        "an unsettled register names no bucket; an unknown bucket and a bucket \
         nobody budgeted at are not the same thing"
    );
    let claimed: Vec<&str> = funding.disagreements[0]
        .claims
        .keys()
        .map(HorizonBucket::as_str)
        .collect();
    assert_eq!(
        claimed,
        vec!["hours_to_days", "years"],
        "both sides are kept, not only the one that would win"
    );

    let ledger = LifecycleLedger::new();
    let error = assurance(reconciler)
        .admit(&ledger, &candidate())
        .expect_err("a disputed horizon must refuse");
    assert_eq!(error.code(), "denied", "{error:?}");
    let message = error.to_string();
    assert!(
        message.contains("horizon is disputed"),
        "the refusal must name the disagreement rather than some other gate: {message}"
    );
    assert!(
        message.contains("research-enrolment") && message.contains("liquidity-model"),
        "the refusal must name who disagreed, or nobody can go and repair one: {message}"
    );
    Ok(())
}

#[test]
fn a_desk_deciding_over_a_disagreement_funds_the_least_liquid_claim_and_the_record_says_so()
-> Result<()> {
    // Funding a position against a *more* liquid pool than it deserves is the
    // failure §23.4 exists to prevent, so the claim that wins is the least
    // liquid one. And the overruled claim travels onto the record: a promotion
    // taken over an unresolved argument is otherwise indistinguishable
    // afterwards from one taken on agreement.
    let reason = "funded from reserved capital until the liquidity model is repaired";
    let mut reconciler = PoolReconciler::new(roomy()?);
    reconciler.claim(&candidate(), "research-enrolment", Horizon::HoursToDays)?;
    reconciler.claim(&candidate(), "liquidity-model", Horizon::Years)?;
    reconciler.budget(&candidate(), dec!("250000"))?;
    let reconciler = reconciler.deciding_despite(reason);

    let ledger = LifecycleLedger::new();
    let verdict = assurance(reconciler).admit(&ledger, &candidate())?;

    assert_eq!(
        verdict.horizon,
        HorizonBucket::new("years")?,
        "the least liquid claim wins where a desk decides anyway"
    );
    assert_eq!(verdict.despite.as_deref(), Some(reason));
    assert!(verdict.decided_over_disagreement());
    assert_eq!(verdict.disputes.len(), 1);
    Ok(())
}

#[test]
fn the_unfunded_commitment_liability_reaches_the_gate_even_though_no_strategy_budgeted_for_it()
-> Result<()> {
    // A commitment nobody allocated against is exactly the one that surprises
    // a desk when it is called. The reserved pool must meet it *as well as* the
    // positions booked at the years horizon, so it has to cross the seam inside
    // `committed` — otherwise the gate reads a pool with room that has none.
    let pools = CapitalPools::new(
        dec!("4000000"),
        dec!("1000000"),
        dec!("1000000"),
        dec!("1000000"),
        dec!("1000000"),
        dec!("900000"),
    )?;
    let mut reconciler = PoolReconciler::new(pools);
    reconciler.claim(&candidate(), "liquidity-model", Horizon::Years)?;
    reconciler.budget(&candidate(), dec!("250000"))?;

    let funding = reconciler.fund(&candidate(), &BTreeSet::new())?;
    let years = funding
        .standings
        .iter()
        .find(|standing| standing.bucket.as_str() == "years")
        .expect("the years bucket is reported");
    // Premise: the budget alone fits the pool. Only the liability breaks it.
    assert!(
        dec!("250000") < years.pool,
        "the budget alone must fit, or this test proves nothing about the liability"
    );
    assert_eq!(
        years.committed,
        dec!("1150000"),
        "the liability is charged to the reserved pool alongside the position"
    );

    let ledger = LifecycleLedger::new();
    let error = assurance(reconciler)
        .admit(&ledger, &candidate())
        .expect_err("the liability pushes the reserved pool over");
    assert!(
        error
            .to_string()
            .contains("years is over its pool of 1000000 by 150000"),
        "{error:?}"
    );
    Ok(())
}

#[test]
fn a_strategy_drawing_on_a_pool_with_no_stated_budget_is_refused_rather_than_charged_nothing()
-> Result<()> {
    // A claim of zero and an unknown claim are not the same thing. Treating a
    // missing budget as zero would let a strategy hold capital that no pool was
    // ever charged for, and the reconciliation would report balanced.
    let mut reconciler = PoolReconciler::new(roomy()?);
    reconciler.claim(&candidate(), "research-enrolment", Horizon::HoursToDays)?;

    let error = reconciler
        .fund(&candidate(), &BTreeSet::new())
        .expect_err("a subject with no budget is refused");
    assert_eq!(error.code(), "invalid", "{error:?}");
    assert!(
        error.message().contains("no stated budget"),
        "the refusal must name what is missing: {error:?}"
    );

    // And the same reconciler admits once the budget is stated, so the refusal
    // above is about the missing figure and not about the fixture.
    reconciler.budget(&candidate(), dec!("250000"))?;
    reconciler.fund(&candidate(), &BTreeSet::new())?;
    Ok(())
}

#[test]
fn a_negative_budget_is_refused_rather_than_netted_against_another_strategys_claim() -> Result<()> {
    // A negative budget at the bucket another strategy is over-committed at
    // nets a real breach away and the reconciliation then reports balanced. A
    // short position is sized as an exposure, not as a claim on a pool.
    let mut reconciler = PoolReconciler::new(roomy()?);
    let error = reconciler
        .budget(&candidate(), dec!("-1"))
        .expect_err("a negative capital claim is refused");
    assert_eq!(error.code(), "invalid", "{error:?}");
    assert!(
        error.message().contains("short position"),
        "the refusal must say what a negative claim would really be: {error:?}"
    );
    // Zero is admitted: a strategy asking for nothing is a stated claim, and
    // refusing it would be clamping rather than validating.
    reconciler.budget(&candidate(), dec!("0"))?;
    Ok(())
}
