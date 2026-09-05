//! ADR 0039: the centre partitions a region's grant into disjoint per-cell
//! shares, and refuses a plan it cannot partition.
//!
//! These tests build the allocation plan by hand — every field of
//! `AllocationPlan` is public and audited — so what is proved is the
//! partitioner over a plan, not the allocator's behaviour, which has its own
//! suite. The envelope-backed half (that a cell's manifest names only grants
//! whose gross fits its share) lives in `central.rs`, beside the fixtures
//! that can issue one.

#![allow(clippy::panic_in_result_fn)]

use qip_capital::allocation::{Allocation, AllocationPlan};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Timestamp, dec};
use qip_kernel::central::{CentralConfig, CentralPlane, RegionMembership};
use std::collections::BTreeMap;

const LONDON: &str = "europe-west2";
const NEW_YORK: &str = "us-east4";

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn plane() -> Result<CentralPlane> {
    CentralPlane::new(&[7u8; 32], CentralConfig::default())
}

fn amount(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn allocation(strategy: &str, cell: &str, notional: &str) -> Allocation {
    Allocation {
        strategy: StrategyId::new(strategy),
        cell: cell.to_string(),
        venue: VenueId::new("XLON"),
        notional: amount(notional),
        indicated: amount(notional),
        risk_adjusted_edge: 0.01,
        binding_constraints: Vec::new(),
    }
}

fn plan(allocations: Vec<Allocation>) -> AllocationPlan {
    let budget = allocations
        .iter()
        .map(|allocation| allocation.notional)
        .fold(Decimal::ZERO, |left, right| left + right);
    AllocationPlan {
        at: now(),
        total_budget: budget,
        drawdown: 0.0,
        drawdown_multiplier: Decimal::ONE,
        budget,
        allocations,
        refusals: Vec::new(),
    }
}

/// Three cells in two regions: two in London, one in New York.
fn membership(london: &str, new_york: &str) -> Result<RegionMembership> {
    RegionMembership::new(
        BTreeMap::from([
            (LONDON.to_string(), amount(london)),
            (NEW_YORK.to_string(), amount(new_york)),
        ]),
        BTreeMap::from([
            ("lon-1".to_string(), LONDON.to_string()),
            ("lon-2".to_string(), LONDON.to_string()),
            ("nyc-1".to_string(), NEW_YORK.to_string()),
        ]),
    )
}

#[test]
fn a_regions_shares_are_disjoint_and_sum_to_at_most_its_grant() -> Result<()> {
    let plan = plan(vec![
        allocation("alpha", "lon-1", "300"),
        allocation("beta", "lon-1", "100"),
        allocation("gamma", "lon-2", "400"),
        allocation("delta", "nyc-1", "500"),
    ]);
    assert_eq!(
        plan.allocated(),
        dec!("1300"),
        "the premise: the plan allocates across three cells"
    );
    let membership = membership("1000", "500")?;

    let shares = plane()?.region_shares(&plan, &membership, now())?;
    assert!(
        shares.withheld().is_empty(),
        "a cell was withheld a share for no reason: {:?}",
        shares.withheld()
    );
    // Each cell's share is its own gross in the plan, and nothing else's.
    for (cell, expected) in [("lon-1", "400"), ("lon-2", "400"), ("nyc-1", "500")] {
        let share = shares
            .for_cell(cell)
            .unwrap_or_else(|| panic!("{cell} received no share"));
        assert_eq!(
            share.amount(),
            amount(expected),
            "{cell}'s share is not its gross"
        );
        assert_eq!(share.amount(), plan.for_cell(cell));
    }
    // Disjoint: the shares sum to exactly what the plan allocated, so no
    // unit of the plan is in two shares.
    let all: Decimal = shares
        .shares()
        .values()
        .map(|share| share.amount())
        .fold(Decimal::ZERO, |left, right| left + right);
    assert_eq!(all, plan.allocated(), "the shares double-count the plan");
    // And per region, at most the grant — with London's remainder left
    // unallocated rather than rounded onto anyone.
    assert_eq!(shares.region_total(LONDON)?, dec!("800"));
    assert!(shares.region_total(LONDON)? <= dec!("1000"));
    assert_eq!(shares.region_total(NEW_YORK)?, dec!("500"));
    assert!(shares.region_total(NEW_YORK)? <= dec!("500"));
    Ok(())
}

#[test]
fn a_plan_whose_cells_exceed_a_regions_grant_is_refused_not_scaled() -> Result<()> {
    let plan = plan(vec![
        allocation("alpha", "lon-1", "300"),
        allocation("gamma", "lon-2", "400"),
        allocation("delta", "nyc-1", "500"),
    ]);
    // The premise: the same plan partitions under a grant that covers it.
    assert!(
        plane()?
            .region_shares(&plan, &membership("700", "500")?, now())
            .is_ok(),
        "the plan does not partition even under a covering grant"
    );

    let refused = plane()?.region_shares(&plan, &membership("600", "500")?, now());
    let error = match refused {
        Ok(shares) => panic!(
            "a plan allocating 700 to a region granted 600 was partitioned: {:?}",
            shares.shares()
        ),
        Err(error) => error,
    };
    let message = error.message();
    assert!(
        message.contains(LONDON) && message.contains("700") && message.contains("600"),
        "the refusal did not name the region, the sum and the grant: {message}"
    );
    assert!(
        message.contains("refused rather than scaled"),
        "the refusal did not say what it is not doing: {message}"
    );
    Ok(())
}

#[test]
fn a_cell_in_no_region_receives_no_share() -> Result<()> {
    let plan = plan(vec![
        allocation("alpha", "lon-1", "300"),
        allocation("omega", "sin-1", "200"),
    ]);
    assert_eq!(
        plan.for_cell("sin-1"),
        dec!("200"),
        "the premise: the plan allocates to the cell in no region"
    );
    let shares = plane()?.region_shares(&plan, &membership("1000", "500")?, now())?;
    assert!(
        shares.for_cell("sin-1").is_none(),
        "a cell in no region received a share: {:?}",
        shares.for_cell("sin-1")
    );
    let reason = shares
        .withheld()
        .get("sin-1")
        .unwrap_or_else(|| panic!("the cell was neither shared nor withheld with a reason"));
    assert!(
        reason.contains("in no region"),
        "the reason did not say why: {reason}"
    );
    // The cells in a region still get theirs; the stray does not poison the
    // plan.
    assert_eq!(
        shares.for_cell("lon-1").map(|share| share.amount()),
        Some(dec!("300"))
    );
    // A cell in a region but absent from the plan gets a share of nothing,
    // stated, which its table reads as "nothing".
    assert_eq!(
        shares.for_cell("nyc-1").map(|share| share.amount()),
        Some(Decimal::ZERO)
    );
    assert!(
        shares
            .for_cell("nyc-1")
            .is_some_and(|share| share.live_grants().is_empty())
    );
    Ok(())
}

#[test]
fn a_membership_that_files_a_cell_under_an_ungranted_region_is_refused() {
    let ungranted = RegionMembership::new(
        BTreeMap::from([(LONDON.to_string(), dec!("1000"))]),
        BTreeMap::from([("sin-1".to_string(), "asia-southeast1".to_string())]),
    );
    assert!(
        ungranted.is_err(),
        "a cell was filed under a region with no grant, which would share it a silent nothing"
    );
    let unfunded = RegionMembership::new(
        BTreeMap::from([(LONDON.to_string(), Decimal::ZERO)]),
        BTreeMap::new(),
    );
    assert!(unfunded.is_err(), "a region granted nothing was admitted");
    assert!(
        membership("1000", "500").is_ok(),
        "the premise: a valid membership is admitted"
    );
}
