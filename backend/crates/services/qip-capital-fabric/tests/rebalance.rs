//! Treasury rebalancing (CAPITAL-020): after drift, the planned transfers
//! bring every holding back within tolerance of the placement, and each one
//! runs along an active custody corridor.
//!
//! "Runs along a corridor" is checked two ways, because the first alone can
//! be satisfied by a planner that merely copies a corridor's id onto an
//! intent. Each move is compared against the corridor it names, and then put
//! to the real [`TransferGate`], whose first check is that the intent and the
//! corridor agree about where the money goes.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_capital_fabric::assessment::AssessmentId;
use qip_capital_fabric::corridor::{
    Corridor, CorridorCaps, CorridorId, CorridorStage, PermittedHours,
};
use qip_capital_fabric::custody::{
    Attestation, CorridorKind, CustodyClass, CustodyPolicy, EnforcementPoint, EnforcementPoints,
    Identity, TransferAuthority,
};
use qip_capital_fabric::destination::{
    ACTIVATION_DELAY, Approver, Asset, DestinationKey, DestinationRegistry, SignatureRecord,
};
use qip_capital_fabric::gate::{
    CorridorFunding, FundingStanding, KillSwitchState, SourceBalances, TransferGate,
    TransferHistory, VelocityState,
};
use qip_capital_fabric::location::{CapitalLocation, Region};
use qip_capital_fabric::rebalance::{RebalancePlan, rebalance};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Currency, Decimal, Duration, Timestamp, dec};
use std::collections::BTreeMap;

fn proposed_at() -> Timestamp {
    Timestamp::from_civil(2024, 3, 7).saturating_add(Duration::from_hours(9))
}

fn signed_at() -> Timestamp {
    proposed_at().saturating_add(Duration::from_hours(2))
}

fn now() -> Timestamp {
    signed_at()
        .saturating_add(ACTIVATION_DELAY)
        .saturating_add(Duration::from_hours(1))
}

fn location(region: &str, venue: &str) -> CapitalLocation {
    CapitalLocation::new(Region::new(region), Currency::USD, VenueId::new(venue))
}

/// The account a location is credited through.
fn account(at: &CapitalLocation) -> Result<DestinationKey> {
    DestinationKey::new(Asset::new("USD")?, format!("ACCT-{}", at.venue))
}

fn caps(per_transfer: Decimal) -> Result<CorridorCaps> {
    CorridorCaps::new(
        per_transfer,
        dec!("100000"),
        dec!("100000"),
        dec!("1000000"),
        Duration::from_mins(15),
        PermittedHours::ALL_DAY,
    )
}

/// A corridor proposed and reviewed, and no further.
fn reviewed_corridor(
    from: &CapitalLocation,
    to: &CapitalLocation,
    per_transfer: Decimal,
) -> Result<Corridor> {
    let mut corridor = Corridor::propose(
        CorridorId::new(format!("{}-to-{}", from.venue, to.venue).to_lowercase())?,
        from.clone(),
        CustodyClass::FiatAtInstitutionOfRecord,
        CorridorKind::InstitutionApprovalFlow,
        account(to)?,
        caps(per_transfer)?,
        "restore the decided placement",
        Approver::new("alice")?,
        proposed_at(),
    )?;
    corridor.review(
        Approver::new("bob")?,
        proposed_at().saturating_add(Duration::from_hours(1)),
    )?;
    Ok(corridor)
}

/// The same corridor walked through signature and delay to active.
fn active_corridor(
    from: &CapitalLocation,
    to: &CapitalLocation,
    per_transfer: Decimal,
) -> Result<Corridor> {
    let mut corridor = reviewed_corridor(from, to, per_transfer)?;
    corridor.record_signature(SignatureRecord::new(
        Approver::new("carol")?,
        signed_at(),
        "vault/corridor/1",
    )?)?;
    corridor.begin_delay(signed_at())?;
    corridor.activate(signed_at().saturating_add(ACTIVATION_DELAY))?;
    Ok(corridor)
}

fn accounts(locations: &[CapitalLocation]) -> Result<BTreeMap<DestinationKey, CapitalLocation>> {
    locations
        .iter()
        .map(|at| Ok((account(at)?, at.clone())))
        .collect()
}

fn book(pairs: &[(&CapitalLocation, Decimal)]) -> BTreeMap<CapitalLocation, Decimal> {
    pairs.iter().map(|(at, v)| ((*at).clone(), *v)).collect()
}

/// Holdings as they would stand had every planned transfer been carried.
fn carried(
    holdings: &BTreeMap<CapitalLocation, Decimal>,
    plan: &RebalancePlan,
) -> BTreeMap<CapitalLocation, Decimal> {
    let mut after = holdings.clone();
    for planned in &plan.moves {
        *after.entry(planned.from.clone()).or_insert(Decimal::ZERO) -= planned.intent.amount();
        *after.entry(planned.to.clone()).or_insert(Decimal::ZERO) += planned.intent.amount();
    }
    after
}

fn furthest(
    placement: &BTreeMap<CapitalLocation, Decimal>,
    holdings: &BTreeMap<CapitalLocation, Decimal>,
) -> Decimal {
    placement
        .iter()
        .map(|(at, target)| (holdings.get(at).copied().unwrap_or(Decimal::ZERO) - *target).abs())
        .fold(Decimal::ZERO, Decimal::max)
}

/// Every planned move agrees with the corridor it names.
fn assert_on_corridors(
    plan: &RebalancePlan,
    corridors: &[Corridor],
    accounts: &BTreeMap<DestinationKey, CapitalLocation>,
) {
    for planned in &plan.moves {
        let corridor = corridors
            .iter()
            .find(|c| c.id() == &planned.corridor)
            .expect("a planned move names a corridor that exists");
        assert_eq!(corridor.stage(), CorridorStage::Active, "{}", corridor.id());
        assert_eq!(corridor.source(), &planned.from);
        assert_eq!(planned.intent.source(), &planned.from);
        assert_eq!(planned.intent.destination(), corridor.destination());
        assert_eq!(accounts.get(corridor.destination()), Some(&planned.to));
        assert!(planned.intent.amount() <= corridor.caps().max_per_transfer());
        assert!(planned.intent.purpose().reduces_deviation());
    }
}

/// Put one planned move to the real gate with every other input satisfied.
fn gate_admits(
    planned: &qip_capital_fabric::rebalance::RebalanceMove,
    corridors: &[Corridor],
    registry: &DestinationRegistry,
) -> Result<bool> {
    let corridor = corridors
        .iter()
        .find(|c| c.id() == &planned.corridor)
        .expect("a planned move names a corridor that exists");
    let custody = CustodyPolicy::blueprint();
    let mut points = EnforcementPoints::new();
    for (point, identity, reference) in [
        (
            EnforcementPoint::TransferGate,
            "gate-svc",
            AssessmentId::of(
                corridor.id(),
                planned.intent.source(),
                planned.intent.destination(),
                planned.intent.amount(),
                now(),
            )
            .to_string(),
        ),
        (
            EnforcementPoint::CustodyPolicy,
            "custody-policy-svc",
            custody.fingerprint().to_string(),
        ),
        (
            EnforcementPoint::VenueAllowlist,
            "venue-ops-oob",
            format!("{}-record-1", EnforcementPoint::VenueAllowlist.as_str()),
        ),
    ] {
        points.attest(Attestation::new(
            point,
            Identity::new(identity)?,
            reference,
            signed_at(),
        )?)?;
    }
    let verdict = TransferGate::assess(
        &planned.intent,
        corridor,
        registry,
        &custody,
        &TransferAuthority::new(points, Identity::new("trading-svc")?),
        &CorridorFunding::new(
            FundingStanding::Permitted,
            dec!("100000"),
            "every strategy this corridor funds has reached scaled",
        )?,
        &TransferHistory::empty(),
        &SourceBalances::new(dec!("100000"), dec!("0"), dec!("0"), dec!("0"))?,
        VelocityState::CLEAR,
        KillSwitchState::Armed,
        now(),
    );
    Ok(verdict.is_ok())
}

/// Every destination proposed, verified and signed, so it is usable at
/// [`now`].
fn registry(locations: &[CapitalLocation]) -> Result<DestinationRegistry> {
    let mut registry = DestinationRegistry::new();
    for at in locations {
        let key = account(at)?;
        registry.propose(key.clone(), Approver::new("alice")?, proposed_at())?;
        registry.verify(
            &key,
            Approver::new("bob")?,
            proposed_at().saturating_add(Duration::from_hours(1)),
        )?;
        registry.record_signature(
            &key,
            SignatureRecord::new(Approver::new("carol")?, signed_at(), "vault/dest/1")?,
        )?;
    }
    Ok(registry)
}

#[test]
fn after_injected_drift_the_planned_transfers_bring_every_holding_within_tolerance_and_each_runs_along_an_active_corridor()
-> Result<()> {
    let treasury = location("namr", "TREASURY");
    let new_york = location("namr", "XNYS");
    let london = location("emea", "XLON");
    let all = [treasury.clone(), new_york.clone(), london.clone()];
    let placement = book(&[
        (&treasury, dec!("4000")),
        (&new_york, dec!("3000")),
        (&london, dec!("3000")),
    ]);
    // Drift injected: the treasury holds what two venues are short of.
    let holdings = book(&[
        (&treasury, dec!("6550")),
        (&new_york, dec!("1000")),
        (&london, dec!("2450")),
    ]);
    let tolerance = dec!("100");
    let corridors = vec![
        active_corridor(&treasury, &new_york, dec!("5000"))?,
        active_corridor(&treasury, &london, dec!("5000"))?,
        active_corridor(&new_york, &treasury, dec!("5000"))?,
    ];
    let accounts = accounts(&all)?;
    // Premise: the drift is real. Without it "within tolerance afterwards"
    // is true of a planner that does nothing.
    assert_eq!(furthest(&placement, &holdings), dec!("2550"));

    let plan = rebalance(&placement, &holdings, tolerance, &corridors, &accounts)?;

    assert!(plan.unrouted.is_empty(), "{:?}", plan.unrouted);
    assert_eq!(plan.moves.len(), 2);
    let after = carried(&holdings, &plan);
    assert!(
        furthest(&placement, &after) <= tolerance,
        "a holding is still {} from its placement",
        furthest(&placement, &after)
    );
    assert_on_corridors(&plan, &corridors, &accounts);
    // And the control itself agrees each one is on its corridor.
    let registry = registry(&all)?;
    for planned in &plan.moves {
        assert!(
            gate_admits(planned, &corridors, &registry)?,
            "the gate vetoed {planned:?}"
        );
    }
    Ok(())
}

#[test]
fn a_drift_whose_only_corridor_is_not_active_is_reported_and_no_transfer_is_planned_for_it()
-> Result<()> {
    let treasury = location("namr", "TREASURY");
    let new_york = location("namr", "XNYS");
    let all = [treasury.clone(), new_york.clone()];
    let placement = book(&[(&treasury, dec!("4000")), (&new_york, dec!("3000"))]);
    let holdings = book(&[(&treasury, dec!("6000")), (&new_york, dec!("1000"))]);
    let accounts = accounts(&all)?;
    // Premise: with the corridor active the same drift is corrected, so the
    // refusal below is about the corridor's stage and nothing else.
    let active = vec![active_corridor(&treasury, &new_york, dec!("5000"))?];
    let corrected = rebalance(&placement, &holdings, dec!("100"), &active, &accounts)?;
    assert_eq!(corrected.moves.len(), 1);
    assert_eq!(corrected.moves[0].intent.amount(), dec!("2000"));

    // Reviewed, never signed: a route somebody drew and nobody authorised.
    let unsigned = vec![reviewed_corridor(&treasury, &new_york, dec!("5000"))?];
    assert_eq!(unsigned[0].stage(), CorridorStage::Reviewed);
    let plan = rebalance(&placement, &holdings, dec!("100"), &unsigned, &accounts)?;

    assert!(
        plan.moves.is_empty(),
        "a transfer was planned off-corridor: {:?}",
        plan.moves
    );
    let reported: Vec<&CapitalLocation> = plan.unrouted.iter().map(|u| &u.location).collect();
    assert_eq!(reported, vec![&treasury, &new_york]);
    assert!(
        plan.unrouted[0].reason.contains("no active corridor"),
        "{}",
        plan.unrouted[0].reason
    );
    Ok(())
}

#[test]
fn a_correction_larger_than_the_corridors_per_transfer_cap_is_reported_rather_than_planned_for_the_gate_to_veto()
-> Result<()> {
    let treasury = location("namr", "TREASURY");
    let new_york = location("namr", "XNYS");
    let placement = book(&[(&treasury, dec!("4000")), (&new_york, dec!("3000"))]);
    let holdings = book(&[(&treasury, dec!("6000")), (&new_york, dec!("1000"))]);
    let accounts = accounts(&[treasury.clone(), new_york.clone()])?;
    // The correction is 2000 and the corridor carries at most 1999 at a time.
    let corridors = vec![active_corridor(&treasury, &new_york, dec!("1999"))?];
    assert_eq!(corridors[0].stage(), CorridorStage::Active);

    let plan = rebalance(&placement, &holdings, dec!("100"), &corridors, &accounts)?;

    assert!(plan.moves.is_empty(), "{:?}", plan.moves);
    assert_eq!(plan.unrouted.len(), 2);
    Ok(())
}

#[test]
fn for_generated_drift_over_a_connected_set_of_corridors_every_holding_ends_within_tolerance()
-> Result<()> {
    let all = [
        location("namr", "TREASURY"),
        location("namr", "XNYS"),
        location("emea", "XLON"),
        location("apac", "XTKS"),
    ];
    let mut corridors = Vec::new();
    for from in &all {
        for to in all.iter().filter(|to| *to != from) {
            corridors.push(active_corridor(from, to, dec!("50000"))?);
        }
    }
    let accounts = accounts(&all)?;
    let tolerance = dec!("50");
    let mut rng = Xoshiro256::seeded(0x00CA_0020);
    let (mut drifted, mut moved) = (0, 0);
    for case in 0..200u64 {
        let placement: BTreeMap<CapitalLocation, Decimal> = all
            .iter()
            .map(|at| {
                (
                    at.clone(),
                    Decimal::from_int(1_000 + rng.below(9_000) as i64),
                )
            })
            .collect();
        // Drift that moves capital between locations and creates none.
        let mut holdings = placement.clone();
        for _ in 0..rng.below(6) {
            let from = &all[rng.below(4) as usize];
            let to = &all[rng.below(4) as usize];
            let shifted = Decimal::from_int(rng.below(900) as i64);
            *holdings.entry(from.clone()).or_insert(Decimal::ZERO) -= shifted;
            *holdings.entry(to.clone()).or_insert(Decimal::ZERO) += shifted;
        }
        let was_outside = furthest(&placement, &holdings) > tolerance;
        drifted += usize::from(was_outside);

        let plan = rebalance(&placement, &holdings, tolerance, &corridors, &accounts)?;

        assert!(plan.unrouted.is_empty(), "case {case}: {:?}", plan.unrouted);
        assert_eq!(was_outside, !plan.moves.is_empty(), "case {case}");
        assert!(
            furthest(&placement, &carried(&holdings, &plan)) <= tolerance,
            "case {case}: a holding is still outside tolerance"
        );
        assert_on_corridors(&plan, &corridors, &accounts);
        moved += plan.moves.len();
    }
    // Premise: the sweep really drifted, and really planned.
    assert!(drifted > 50 && moved > 50, "{drifted} {moved}");
    Ok(())
}
