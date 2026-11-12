//! The centre's producer of hedge narrowings (ADR 0407, R12-CAPITAL-10098).
//!
//! Until this producer existed a hedge plan could travel to a cell as a
//! signed narrowing and nothing at the centre ever made one. These tests
//! hold the rule that turns a plan's trades into per-cell targets: shared pro
//! rata across the cells holding the opposite side, never past zero, only at
//! a venue the cell is granted, and never a target that would open exposure.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::HedgePlan;
use qip_contracts::control::{
    Exposure, HedgeInstrument, HedgeLiquidity, HedgePlanFields, HedgeRollback, HedgeSide,
};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp, dec};
use qip_kernel::central::hedge_narrowing::{CellHoldings, derive};
use std::collections::BTreeMap;

const VENUE: &str = "SIM-CME";
const OTHER_VENUE: &str = "SIM-ICE";
const INSTRUMENT: &str = "ES-DEC";

fn at() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

fn plan(legs: Vec<(HedgeSide, Decimal)>) -> Result<HedgePlan> {
    HedgePlan::new(HedgePlanFields {
        schema_version: 1,
        plan_id: "hedge-plan-7".to_string(),
        exposures: vec![Exposure {
            risk_factor: "equity-beta".to_string(),
            amount: dec!("1200000"),
        }],
        scenarios_protected: vec!["credit-crisis-2008".to_string()],
        instruments: legs
            .into_iter()
            .map(|(side, quantity)| HedgeInstrument {
                instrument: INSTRUMENT.to_string(),
                venue: VenueId::new(VENUE),
                side,
                quantity,
            })
            .collect(),
        cost: dec!("950"),
        liquidity: HedgeLiquidity {
            max_adv_share_bp: 5,
            time_to_exit: Duration::from_mins(30),
        },
        rollback: HedgeRollback {
            trigger: "equity beta below 0.2".to_string(),
            steps: vec!["buy back ES-DEC".to_string()],
        },
    })
}

fn holding(held: Decimal, granted: &str) -> CellHoldings {
    let mut cell = CellHoldings::default();
    cell.held
        .insert((VENUE.to_string(), INSTRUMENT.to_string()), held);
    cell.granted_venues.insert(granted.to_string());
    cell
}

fn targets(
    distribution: &qip_kernel::central::hedge_narrowing::HedgeDistribution,
) -> BTreeMap<String, Decimal> {
    distribution
        .narrowings
        .iter()
        .flat_map(|narrowing| {
            narrowing
                .targets
                .iter()
                .map(|target| (narrowing.cell.clone(), target.target_net))
        })
        .collect()
}

/// The rule's arithmetic, and the case that would hurt if it were wrong: a
/// plan selling more than the cells hold must bind every holder to flat and
/// no further, because a target past zero is a cell asked to open the
/// opposite position. Two sell legs on one instrument are one trade, so a
/// cell receives one target for it and not two the cell would refuse.
#[test]
fn a_plans_sell_is_shared_pro_rata_across_the_cells_long_it_and_no_target_passes_zero() -> Result<()>
{
    let cells: BTreeMap<String, CellHoldings> = [
        ("london-1".to_string(), holding(dec!("60"), VENUE)),
        ("newyork-1".to_string(), holding(dec!("40"), VENUE)),
    ]
    .into_iter()
    .collect();

    // 50 of the 100 held, in two legs: each cell keeps half its holding.
    let half = derive(
        &plan(vec![
            (HedgeSide::Sell, dec!("20")),
            (HedgeSide::Sell, dec!("30")),
        ])?,
        at(),
        &cells,
        at(),
    );
    assert_eq!(half.narrowings.len(), 2, "{:?}", half.lines);
    for narrowing in &half.narrowings {
        assert_eq!(narrowing.targets.len(), 1, "one target per instrument");
        narrowing.check()?;
    }
    assert_eq!(
        targets(&half),
        [
            ("london-1".to_string(), dec!("30")),
            ("newyork-1".to_string(), dec!("20"))
        ]
        .into_iter()
        .collect()
    );

    // 150 against 100 held: both to flat, and the excess named, not sent.
    let over = derive(
        &plan(vec![(HedgeSide::Sell, dec!("150"))])?,
        at(),
        &cells,
        at(),
    );
    assert!(
        targets(&over).values().all(|target| target.is_zero()),
        "{:?}",
        targets(&over)
    );
    assert_eq!(targets(&over).len(), 2);
    assert!(
        over.lines
            .iter()
            .any(|line| line.contains("exceeds the 100")),
        "{:?}",
        over.lines
    );
    Ok(())
}

/// The plan's buy reduces only shorts. A cell long the instrument, or
/// flat, would have to buy — to open or widen a position — to meet a buy,
/// and ADR 0126 says a narrowing cannot express that: it needs an envelope.
/// So nothing is sent to it, and the reason is a line rather than a silence.
#[test]
fn a_plan_trade_that_would_open_exposure_at_every_cell_is_not_sent_and_says_why() -> Result<()> {
    let cells: BTreeMap<String, CellHoldings> = [
        ("london-1".to_string(), holding(dec!("60"), VENUE)),
        ("newyork-1".to_string(), holding(dec!("-40"), VENUE)),
    ]
    .into_iter()
    .collect();
    let premise = derive(
        &plan(vec![(HedgeSide::Buy, dec!("10"))])?,
        at(),
        &cells,
        at(),
    );
    assert_eq!(
        targets(&premise),
        [("newyork-1".to_string(), dec!("-30"))]
            .into_iter()
            .collect(),
        "premise: a buy narrows the short holder toward zero"
    );

    let long_only: BTreeMap<String, CellHoldings> =
        [("london-1".to_string(), holding(dec!("60"), VENUE))]
            .into_iter()
            .collect();
    let refused = derive(
        &plan(vec![(HedgeSide::Buy, dec!("10"))])?,
        at(),
        &long_only,
        at(),
    );
    assert!(refused.narrowings.is_empty(), "{:?}", refused.narrowings);
    assert!(
        refused
            .lines
            .iter()
            .any(|line| line.contains("would open exposure")),
        "{:?}",
        refused.lines
    );
    Ok(())
}

/// A cell refuses a whole frame naming a venue no live envelope there
/// grants, so one stray target would cost it every other. The producer
/// therefore leaves a cell out of a venue it is not granted, and the share
/// is computed over the cells that are.
#[test]
fn a_target_is_stated_only_at_a_venue_the_cells_live_envelope_grants() -> Result<()> {
    let cells: BTreeMap<String, CellHoldings> = [
        ("london-1".to_string(), holding(dec!("60"), VENUE)),
        ("newyork-1".to_string(), holding(dec!("40"), OTHER_VENUE)),
    ]
    .into_iter()
    .collect();
    let distribution = derive(
        &plan(vec![(HedgeSide::Sell, dec!("30"))])?,
        at(),
        &cells,
        at(),
    );
    assert_eq!(
        targets(&distribution),
        [("london-1".to_string(), dec!("30"))].into_iter().collect()
    );
    Ok(())
}

/// The narrowing states the plan's own instant, so a cell holding it
/// refuses a re-derivation of the same plan as at-or-behind rather than
/// compounding it, and the plan's own time to exit as its deadline. At that
/// deadline nothing is derived: a narrowing past it binds nothing.
#[test]
fn a_narrowing_carries_the_plans_instant_and_time_to_exit_and_none_is_derived_past_it() -> Result<()>
{
    let cells: BTreeMap<String, CellHoldings> =
        [("london-1".to_string(), holding(dec!("60"), VENUE))]
            .into_iter()
            .collect();
    let plan = plan(vec![(HedgeSide::Sell, dec!("30"))])?;
    let later = at().saturating_add(Duration::from_mins(10));
    let live = derive(&plan, at(), &cells, later);
    let narrowing = live
        .narrowings
        .first()
        .ok_or_else(|| qip_core::error::Error::not_found("a narrowing"))?;
    assert_eq!(narrowing.issued_at, at());
    assert_eq!(
        narrowing.deadline,
        at().saturating_add(Duration::from_mins(30))
    );
    assert_eq!(narrowing.plan_id, "hedge-plan-7");

    let past = derive(
        &plan,
        at(),
        &cells,
        at().saturating_add(Duration::from_mins(30)),
    );
    assert!(past.narrowings.is_empty());
    assert!(
        past.lines.iter().any(|line| line.contains("binds nothing")),
        "{:?}",
        past.lines
    );
    Ok(())
}
