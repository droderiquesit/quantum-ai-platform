//! Tests for the destination registry, the corridor lifecycle and the
//! transfer gate — the deterministic half of blueprint §37 and §38.4 that
//! ADR 0021 permits.
//!
//! Every test here is about a refusal. The gate can only veto, the registry
//! can only say "not yet" or "not ever", and the lifecycle table can only
//! refuse an edge. The failure each prevents is the same one in a different
//! coat: a control that reads as protection and is not, because the check
//! that should have fired was satisfied by something other than the fact it
//! was written to check. So the fixture satisfies every check, each test
//! breaks exactly one, and the assertion is that the *named* check fired —
//! never merely that something did.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital_fabric::corridor::{
    Corridor, CorridorCaps, CorridorId, CorridorStage, PermittedHours,
};
use qip_capital_fabric::custody::{CorridorKind, CustodyClass, CustodyPolicy};
use qip_capital_fabric::destination::{
    ACTIVATION_DELAY, Approver, Asset, DestinationKey, DestinationRegistry, DestinationStatus,
    SignatureRecord,
};
use qip_capital_fabric::gate::{
    AnomalyFlag, CarriedTransfer, GateCheck, KillSwitchState, SourceBalances, StatedPurpose,
    TransferGate, TransferHistory, TransferIntent, VelocityBreaker, VelocityState, Vetoed,
};
use qip_capital_fabric::location::{CapitalLocation, Region};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Currency, Decimal, Duration, Timestamp, dec};

// --- fixtures ---------------------------------------------------------------

/// Thursday 7 March 2024, 09:00 UTC: when everything was proposed.
fn proposed_at() -> Timestamp {
    Timestamp::from_civil(2024, 3, 7).saturating_add(Duration::from_hours(9))
}

/// When the signature was recorded: two hours after proposal.
fn signed_at() -> Timestamp {
    proposed_at().saturating_add(Duration::from_hours(2))
}

/// When the gate is asked: the delay has elapsed with an hour to spare.
fn now() -> Timestamp {
    signed_at()
        .saturating_add(ACTIVATION_DELAY)
        .saturating_add(Duration::from_hours(1))
}

fn alice() -> Result<Approver> {
    Approver::new("alice")
}

fn bob() -> Result<Approver> {
    Approver::new("bob")
}

fn carol() -> Result<Approver> {
    Approver::new("carol")
}

fn treasury() -> CapitalLocation {
    CapitalLocation::new(Region::new("namr"), Currency::USD, VenueId::new("TREASURY"))
}

fn destination() -> Result<DestinationKey> {
    DestinationKey::new(Asset::new("USD")?, "BANK-XYZ-ACCT-1")
}

fn signature(at: Timestamp, reference: &str) -> Result<SignatureRecord> {
    SignatureRecord::new(carol()?, at, reference)
}

fn caps() -> Result<CorridorCaps> {
    CorridorCaps::new(
        dec!("1000"),
        dec!("3000"),
        dec!("10000"),
        dec!("50000"),
        Duration::from_mins(15),
        PermittedHours::ALL_DAY,
    )
}

/// A destination proposed, verified and signed, so it is usable at [`now`].
fn usable_registry() -> Result<DestinationRegistry> {
    let mut registry = DestinationRegistry::new();
    let key = destination()?;
    registry.propose(key.clone(), alice()?, proposed_at())?;
    registry.verify(
        &key,
        bob()?,
        proposed_at().saturating_add(Duration::from_hours(1)),
    )?;
    registry.record_signature(&key, signature(signed_at(), "vault/dest/1")?)?;
    Ok(registry)
}

/// A corridor walked through every stage to active, on the same clock.
fn active_corridor() -> Result<Corridor> {
    let mut corridor = Corridor::propose(
        CorridorId::new("treasury-to-xyz")?,
        treasury(),
        CustodyClass::FiatAtInstitutionOfRecord,
        CorridorKind::InstitutionApprovalFlow,
        destination()?,
        caps()?,
        "fund the XYZ margin account ahead of forecast demand",
        alice()?,
        proposed_at(),
    )?;
    corridor.review(
        bob()?,
        proposed_at().saturating_add(Duration::from_hours(1)),
    )?;
    corridor.record_signature(signature(signed_at(), "vault/corridor/1")?)?;
    corridor.begin_delay(signed_at())?;
    corridor.activate(signed_at().saturating_add(ACTIVATION_DELAY))?;
    Ok(corridor)
}

fn intent() -> Result<TransferIntent> {
    TransferIntent::new(
        treasury(),
        destination()?,
        dec!("500"),
        StatedPurpose::new(dec!("1000"), dec!("500"))?,
    )
}

fn balances() -> Result<SourceBalances> {
    SourceBalances::new(dec!("10000"), dec!("1000"), dec!("1000"), dec!("1000"))
}

/// Run the gate with every input satisfied except whatever the caller
/// overrode.
struct Inputs {
    intent: TransferIntent,
    corridor: Corridor,
    registry: DestinationRegistry,
    custody: CustodyPolicy,
    history: TransferHistory,
    balances: SourceBalances,
    velocity: VelocityState,
    kill_switch: KillSwitchState,
    now: Timestamp,
}

impl Inputs {
    fn satisfied() -> Result<Self> {
        Ok(Self {
            intent: intent()?,
            corridor: active_corridor()?,
            registry: usable_registry()?,
            custody: CustodyPolicy::blueprint(),
            history: TransferHistory::empty(),
            balances: balances()?,
            velocity: VelocityState::CLEAR,
            kill_switch: KillSwitchState::Armed,
            now: now(),
        })
    }

    fn assess(&self) -> std::result::Result<qip_capital_fabric::gate::Approved, Vetoed> {
        TransferGate::assess(
            &self.intent,
            &self.corridor,
            &self.registry,
            &self.custody,
            &self.history,
            &self.balances,
            self.velocity,
            self.kill_switch,
            self.now,
        )
    }

    /// The veto, asserting the premise that the untouched inputs approve.
    fn veto(&self) -> Vetoed {
        match self.assess() {
            Err(veto) => veto,
            Ok(approved) => panic!("expected a veto, got approval {approved:?}"),
        }
    }
}

fn history(entries: &[(Timestamp, Decimal)]) -> Result<TransferHistory> {
    TransferHistory::new(
        entries
            .iter()
            .map(|&(at, amount)| CarriedTransfer { at, amount })
            .collect(),
    )
}

// --- the gate ---------------------------------------------------------------

#[test]
fn an_intent_that_satisfies_every_check_is_approved_naming_all_seven_in_order() -> Result<()> {
    // Premise for every veto test below: the untouched fixture is approved.
    // Without this, a veto test could pass because the fixture was broken in
    // two places and the check under test never ran.
    let inputs = Inputs::satisfied()?;
    let approved = match inputs.assess() {
        Ok(approved) => approved,
        Err(veto) => panic!("the satisfied fixture was vetoed: {veto}"),
    };
    assert_eq!(approved.checks_passed(), &GateCheck::ALL);
    assert_eq!(approved.checks_passed().len(), 7);
    assert_eq!(approved.corridor().as_str(), "treasury-to-xyz");
    assert_eq!(approved.signature_reference(), "vault/corridor/1");
    assert_eq!(approved.assessed_at(), now());
    assert_eq!(approved.intent(), &intent()?);
    Ok(())
}

#[test]
fn a_suspended_corridor_vetoes_on_corridor_authority_with_an_alert() -> Result<()> {
    // The failure prevented: a transfer approved along a corridor a human or
    // an anomaly detector halted an hour ago, because the gate checked a
    // cached "active" rather than the corridor's own stage.
    let mut inputs = Inputs::satisfied()?;
    inputs
        .corridor
        .suspend(None, "reconciliation break", now())?;
    assert_eq!(inputs.corridor.stage(), CorridorStage::Suspended);
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::CorridorAuthority);
    assert!(veto.alert, "a corridor failure alerts, per §37.3");
    assert!(veto.reason.contains("suspended"), "{}", veto.reason);
    assert!(
        veto.reason.contains("reactivation needs approval"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn an_intent_naming_a_different_destination_than_its_corridor_vetoes_on_corridor_authority()
-> Result<()> {
    // The failure prevented: a corridor signed for one address used to
    // authorise a transfer to another, because the gate trusted the caller's
    // pairing of intent and corridor. The allowlist bounds the blast radius
    // only if the corridor's destination is the one the money goes to.
    let mut inputs = Inputs::satisfied()?;
    let other = DestinationKey::new(Asset::new("USD")?, "BANK-XYZ-ACCT-2")?;
    inputs.intent = TransferIntent::new(treasury(), other, dec!("500"), intent()?.purpose())?;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::CorridorAuthority);
    assert!(veto.reason.contains("BANK-XYZ-ACCT-2"), "{}", veto.reason);
    assert!(veto.reason.contains("treasury-to-xyz"), "{}", veto.reason);
    Ok(())
}

#[test]
fn a_destination_still_inside_its_delay_vetoes_on_corridor_authority_naming_when_it_becomes_usable()
-> Result<()> {
    // The failure prevented: an active corridor used against a destination
    // whose own twenty-four hours have not run, because the corridor's delay
    // and the destination's were assumed to be the same clock. §38.4 makes
    // the registry an independent check and this proves the gate consults it.
    let mut inputs = Inputs::satisfied()?;
    let key = destination()?;
    let late_signing = now().saturating_sub(Duration::from_hours(1));
    let mut registry = DestinationRegistry::new();
    registry.propose(key.clone(), alice()?, proposed_at())?;
    registry.verify(&key, bob()?, proposed_at())?;
    registry.record_signature(&key, signature(late_signing, "vault/dest/late")?)?;
    // Premise: the registry itself says not yet, and names the instant.
    let usable_from = late_signing.saturating_add(ACTIVATION_DELAY);
    let err = registry
        .usable(&key, now())
        .err()
        .map(|e| e.message().to_string());
    assert!(
        err.is_some(),
        "the registry admitted a destination signed an hour ago"
    );
    assert!(
        err.as_deref()
            .is_some_and(|m| m.contains(&usable_from.to_string())),
        "{err:?}"
    );
    inputs.registry = registry;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::CorridorAuthority);
    assert!(veto.alert);
    assert!(
        veto.reason.contains("delay has not elapsed"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn a_corridor_whose_source_class_the_custody_policy_says_never_transfers_vetoes_on_corridor_authority()
-> Result<()> {
    // The failure prevented: a corridor a human reviewed, signed and waited a
    // day for, carrying collateral out of a venue. §37.4 says collateral and
    // margin is inventory and never a transfer source, and the custody
    // policy is the enforcement point that says so; the signature proves a
    // person approved the corridor, not that the class may leave at all.
    let mut inputs = Inputs::satisfied()?;
    let mut corridor = Corridor::propose(
        CorridorId::new("treasury-to-xyz")?,
        treasury(),
        CustodyClass::CollateralAndMargin,
        CorridorKind::InstitutionApprovalFlow,
        destination()?,
        caps()?,
        "release posted collateral to the bank",
        alice()?,
        proposed_at(),
    )?;
    corridor.review(
        bob()?,
        proposed_at().saturating_add(Duration::from_hours(1)),
    )?;
    corridor.record_signature(signature(signed_at(), "vault/corridor/collateral")?)?;
    corridor.begin_delay(signed_at())?;
    corridor.activate(signed_at().saturating_add(ACTIVATION_DELAY))?;
    // Premise: every other part of check one is satisfied — the corridor is
    // active and signed, and the destination is usable — so only the policy
    // can be what refuses it.
    assert_eq!(corridor.stage(), CorridorStage::Active);
    assert!(corridor.signed().is_some());
    assert!(inputs.registry.usable(&destination()?, now()).is_ok());
    inputs.corridor = corridor;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::CorridorAuthority);
    assert!(veto.alert);
    assert!(
        veto.reason.contains(
            "collateral_and_margin is inventory at its custodian and never a transfer source"
        ),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn an_amount_over_the_per_transfer_cap_vetoes_on_caps_naming_the_cap() -> Result<()> {
    // The failure prevented: the per-transfer cap being the one limit that
    // never fires because the hourly cap was checked first and was wider.
    let mut inputs = Inputs::satisfied()?;
    inputs.intent = TransferIntent::new(
        treasury(),
        destination()?,
        dec!("1500"),
        intent()?.purpose(),
    )?;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::Caps);
    assert!(
        !veto.alert,
        "a cap veto is a veto without an alert, per §37.3"
    );
    assert!(
        veto.reason.contains("per-transfer cap of 1000"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn the_hourly_cap_is_rolling_so_a_transfer_sixty_one_minutes_ago_counts_against_the_day_but_not_the_hour()
-> Result<()> {
    // The failure prevented: an hourly cap measured on wall-clock hours, which
    // permits a full hour's cap at 09:59 and another at 10:00. Also the
    // inverse: a rolling window that never forgets, which would make every
    // hourly cap a cumulative one.
    let inside = now().saturating_sub(Duration::from_mins(30));
    let outside = now().saturating_sub(Duration::from_mins(61));

    // 2600 inside the hour plus this 500 breaches 3000.
    let mut inputs = Inputs::satisfied()?;
    inputs.history = history(&[(inside, dec!("2600"))])?;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::Caps);
    assert!(
        veto.reason.contains("hourly cap of 3000"),
        "{}",
        veto.reason
    );

    // The same 2600 sixty-one minutes ago no longer counts against the hour,
    // and 2600 + 500 is inside the day, so the gate approves.
    let mut inputs = Inputs::satisfied()?;
    inputs.history = history(&[(outside, dec!("2600"))])?;
    assert!(inputs.assess().is_ok(), "a rolling hour must roll");

    // But 9600 sixty-one minutes ago still counts against the day.
    let mut inputs = Inputs::satisfied()?;
    inputs.history = history(&[(outside, dec!("9600"))])?;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::Caps);
    assert!(
        veto.reason.contains("daily cap of 10000"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn a_transfer_that_would_exhaust_the_cumulative_cap_vetoes_on_caps() -> Result<()> {
    // The failure prevented: a corridor that carries its cumulative cap once
    // a day for as long as nobody revokes it. Spread the history over days so
    // neither the hourly nor the daily cap can be the one that fires.
    let mut inputs = Inputs::satisfied()?;
    let entries: Vec<(Timestamp, Decimal)> = (1..=5)
        .map(|day| {
            (
                now().saturating_sub(Duration::from_days(day + 1)),
                dec!("9950"),
            )
        })
        .collect();
    inputs.history = history(&entries)?;
    assert_eq!(inputs.history.carried_total(), dec!("49750"));
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::Caps);
    assert!(
        veto.reason.contains("cumulative cap of 50000"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn an_assessment_outside_permitted_hours_vetoes_on_caps() -> Result<()> {
    // The failure prevented: a corridor signed for business hours carrying a
    // transfer at 03:00, when nobody who could suspend it is watching.
    let mut inputs = Inputs::satisfied()?;
    let business_hours = CorridorCaps::new(
        dec!("1000"),
        dec!("3000"),
        dec!("10000"),
        dec!("50000"),
        Duration::from_mins(15),
        PermittedHours::new(8, 18)?,
    )?;
    inputs
        .corridor
        .tighten_caps(business_hours, bob()?, now())?;
    // Three in the morning the following day: after the delay and after the
    // last stage change, so nothing but the hours can refuse it.
    let three_am = now()
        .saturating_add(Duration::from_days(1))
        .start_of_day()
        .saturating_add(Duration::from_hours(3));
    assert!(three_am > now());
    assert_eq!(three_am.civil_time().0, 3);
    inputs.now = three_am;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::Caps);
    assert!(
        veto.reason.contains("permitted hours of 08:00-18:00 UTC"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn a_transfer_inside_the_minimum_interval_vetoes_on_minimum_interval_naming_the_next_permitted_instant()
-> Result<()> {
    // The failure prevented: a burst of small transfers each inside every
    // amount cap, which is how a compromised engine drains a corridor without
    // ever tripping a cap. The amount is small enough that no cap fires.
    let mut inputs = Inputs::satisfied()?;
    let five_minutes_ago = now().saturating_sub(Duration::from_mins(5));
    inputs.history = history(&[(five_minutes_ago, dec!("100"))])?;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::MinimumInterval);
    let next = five_minutes_ago.saturating_add(Duration::from_mins(15));
    assert!(veto.reason.contains(&next.to_string()), "{}", veto.reason);
    Ok(())
}

#[test]
fn a_transfer_that_does_not_reduce_deviation_vetoes_with_no_transfer_without_a_stated_purpose()
-> Result<()> {
    // The failure prevented: capital moved because a corridor permitted it
    // rather than because the book needed it. §37.3's words are the veto.
    let mut inputs = Inputs::satisfied()?;
    inputs.intent = TransferIntent::new(
        treasury(),
        destination()?,
        dec!("500"),
        StatedPurpose::new(dec!("500"), dec!("500"))?,
    )?;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::StatedPurpose);
    assert!(!veto.alert);
    assert!(
        veto.reason.contains("no transfer without a stated purpose"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn a_source_short_after_reservations_in_flight_and_commitments_vetoes_on_source_balance()
-> Result<()> {
    // The failure prevented: a balance read as sufficient because the ledger
    // figure ignored the three claims already on it. The bare balance here is
    // ample; only the net is short.
    let mut inputs = Inputs::satisfied()?;
    inputs.balances = SourceBalances::new(dec!("10000"), dec!("4000"), dec!("3000"), dec!("2600"))?;
    assert!(inputs.balances.balance > inputs.intent.amount());
    assert_eq!(inputs.balances.free(), dec!("400"));
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::SourceBalance);
    assert!(veto.reason.contains("400 free"), "{}", veto.reason);
    Ok(())
}

#[test]
fn a_tripped_velocity_breaker_or_a_raised_anomaly_flag_vetoes_all_with_an_alert() -> Result<()> {
    // The failure prevented: a breaker that trips and is then consulted only
    // by the code path that tripped it. Both halves of check six are tested
    // separately so neither can be satisfied by the other.
    let mut inputs = Inputs::satisfied()?;
    inputs.velocity = VelocityState {
        breaker: VelocityBreaker::Tripped,
        anomaly: AnomalyFlag::Clear,
    };
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::VelocityAndAnomaly);
    assert!(veto.alert);
    assert!(veto.reason.contains("velocity breaker"), "{}", veto.reason);

    let mut inputs = Inputs::satisfied()?;
    inputs.velocity = VelocityState {
        breaker: VelocityBreaker::Armed,
        anomaly: AnomalyFlag::Raised,
    };
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::VelocityAndAnomaly);
    assert!(veto.alert);
    assert!(veto.reason.contains("anomaly detector"), "{}", veto.reason);
    Ok(())
}

#[test]
fn a_tripped_kill_switch_vetoes_everything() -> Result<()> {
    // The failure prevented: a kill switch that stops orders but not
    // transfers, because the transfer path was written later by someone who
    // did not know there was one.
    let mut inputs = Inputs::satisfied()?;
    inputs.kill_switch = KillSwitchState::Tripped;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::KillSwitch);
    assert!(
        veto.reason.contains("kill switch is tripped"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn the_checks_run_in_the_order_of_the_table_and_the_first_veto_is_the_one_reported() -> Result<()> {
    // The failure prevented: an operator paged about the kill switch when the
    // finding was that a corridor with no purpose reached the gate at all.
    let mut inputs = Inputs::satisfied()?;
    inputs.intent = TransferIntent::new(
        treasury(),
        destination()?,
        dec!("500"),
        StatedPurpose::new(dec!("500"), dec!("500"))?,
    )?;
    inputs.kill_switch = KillSwitchState::Tripped;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::StatedPurpose);
    assert!(GateCheck::StatedPurpose < GateCheck::KillSwitch);
    Ok(())
}

// --- the lifecycle ----------------------------------------------------------

/// Every edge named in `CorridorStage::transition`, and nothing else.
const LEGAL_EDGES: &[(CorridorStage, CorridorStage)] = &[
    (CorridorStage::Proposed, CorridorStage::Reviewed),
    (CorridorStage::Proposed, CorridorStage::Revoked),
    (CorridorStage::Reviewed, CorridorStage::Signed),
    (CorridorStage::Reviewed, CorridorStage::Revoked),
    (CorridorStage::Signed, CorridorStage::TimeDelayed),
    (CorridorStage::Signed, CorridorStage::Revoked),
    (CorridorStage::TimeDelayed, CorridorStage::Active),
    (CorridorStage::TimeDelayed, CorridorStage::Suspended),
    (CorridorStage::TimeDelayed, CorridorStage::Revoked),
    (CorridorStage::Active, CorridorStage::TimeDelayed),
    (CorridorStage::Active, CorridorStage::Suspended),
    (CorridorStage::Active, CorridorStage::Revoked),
    (CorridorStage::Suspended, CorridorStage::Active),
    (CorridorStage::Suspended, CorridorStage::Revoked),
];

const ALL_STAGES: &[CorridorStage] = &[
    CorridorStage::Proposed,
    CorridorStage::Reviewed,
    CorridorStage::Signed,
    CorridorStage::TimeDelayed,
    CorridorStage::Active,
    CorridorStage::Suspended,
    CorridorStage::Revoked,
];

#[test]
fn every_legal_corridor_edge_transitions_and_nothing_else_does() {
    // Premise: the table is neither empty nor the full cross product, so this
    // can tell a table that grew or shrank from one that stayed the same.
    assert!(!LEGAL_EDGES.is_empty());
    assert!(LEGAL_EDGES.len() < ALL_STAGES.len() * ALL_STAGES.len());
    // The failure prevented: a revoked corridor walked back to active by a
    // late event, or a proposed one jumping straight to active because a
    // "fast path" was added for tests.
    for &from in ALL_STAGES {
        for &to in ALL_STAGES {
            let expect_legal = LEGAL_EDGES.contains(&(from, to));
            let outcome = from.transition(to);
            assert_eq!(
                outcome.is_ok(),
                expect_legal,
                "{} -> {} was {:?}, expected legal = {expect_legal}",
                from.as_str(),
                to.as_str(),
                outcome
            );
        }
    }
}

#[test]
fn revoked_is_terminal_and_nothing_leaves_it() {
    // Premise: revoked is reachable from every other stage.
    for &from in ALL_STAGES {
        if from == CorridorStage::Revoked {
            continue;
        }
        assert!(
            from.transition(CorridorStage::Revoked).is_ok(),
            "{}",
            from.as_str()
        );
    }
    for &to in ALL_STAGES {
        assert!(
            CorridorStage::Revoked.transition(to).is_err(),
            "revoked moved to {}",
            to.as_str()
        );
    }
}

#[test]
fn a_corridor_cannot_activate_before_its_delay_has_elapsed_and_the_refusal_names_the_instant()
-> Result<()> {
    // The failure prevented: a corridor that activated because the clock
    // handed in was generous. The refusal names the instant so a replay can
    // show exactly when the platform would have said yes.
    let mut corridor = Corridor::propose(
        CorridorId::new("c")?,
        treasury(),
        CustodyClass::FiatAtInstitutionOfRecord,
        CorridorKind::InstitutionApprovalFlow,
        destination()?,
        caps()?,
        "purpose",
        alice()?,
        proposed_at(),
    )?;
    corridor.review(bob()?, proposed_at())?;
    corridor.record_signature(signature(signed_at(), "vault/c")?)?;
    let activation_at = corridor.begin_delay(signed_at())?;
    assert_eq!(activation_at, signed_at().saturating_add(ACTIVATION_DELAY));
    let too_early = activation_at.saturating_sub(Duration::from_secs(1));
    let err = corridor
        .activate(too_early)
        .err()
        .map(|e| e.message().to_string());
    assert!(err.is_some(), "activated a second early");
    assert!(
        err.as_deref()
            .is_some_and(|m| m.contains(&activation_at.to_string())),
        "{err:?}"
    );
    assert_eq!(corridor.stage(), CorridorStage::TimeDelayed);
    corridor.activate(activation_at)?;
    assert_eq!(corridor.stage(), CorridorStage::Active);
    Ok(())
}

#[test]
fn the_proposer_cannot_review_their_own_corridor() -> Result<()> {
    // The failure prevented: one credential proposing and reviewing, which
    // makes the review a second click rather than a second person.
    let mut corridor = Corridor::propose(
        CorridorId::new("c")?,
        treasury(),
        CustodyClass::FiatAtInstitutionOfRecord,
        CorridorKind::InstitutionApprovalFlow,
        destination()?,
        caps()?,
        "purpose",
        alice()?,
        proposed_at(),
    )?;
    let err = corridor
        .review(alice()?, proposed_at())
        .err()
        .map(|e| e.message().to_string());
    assert!(
        err.as_deref()
            .is_some_and(|m| m.contains("cannot also review")),
        "{err:?}"
    );
    assert_eq!(corridor.stage(), CorridorStage::Proposed);
    corridor.review(bob()?, proposed_at())?;
    assert_eq!(corridor.stage(), CorridorStage::Reviewed);
    Ok(())
}

// --- caps: tighten is immediate, loosen re-enters the delay -----------------

#[test]
fn tightening_a_cap_on_an_active_corridor_is_immediate_and_the_gate_enforces_it_at_once()
-> Result<()> {
    // The failure prevented: a human lowering a cap during an incident and the
    // lower cap taking effect tomorrow. §37.2 removes the delay from every
    // change that cannot widen where money goes.
    let mut inputs = Inputs::satisfied()?;
    let tighter = CorridorCaps::new(
        dec!("400"),
        dec!("3000"),
        dec!("10000"),
        dec!("50000"),
        Duration::from_mins(15),
        PermittedHours::ALL_DAY,
    )?;
    inputs.corridor.tighten_caps(tighter, bob()?, now())?;
    assert_eq!(inputs.corridor.stage(), CorridorStage::Active);
    // The signed definition is unchanged; the current caps sit inside it.
    let signed = inputs.corridor.signed().map(|s| s.caps.max_per_transfer());
    assert_eq!(signed, Some(dec!("1000")));
    assert_eq!(inputs.corridor.caps().max_per_transfer(), dec!("400"));
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::Caps);
    assert!(
        veto.reason.contains("per-transfer cap of 400"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn loosening_a_cap_re_enters_the_delay_and_the_gate_refuses_until_it_has_elapsed() -> Result<()> {
    // The failure prevented: a raised cap in force the moment it was entered,
    // which is the change an attacker with one approval credential would
    // make. The loosening needs a fresh signature record and a day.
    let mut inputs = Inputs::satisfied()?;
    let looser = CorridorCaps::new(
        dec!("2000"),
        dec!("3000"),
        dec!("10000"),
        dec!("50000"),
        Duration::from_mins(15),
        PermittedHours::ALL_DAY,
    )?;
    let activation_at =
        inputs
            .corridor
            .loosen_caps(looser, signature(now(), "vault/corridor/2")?, now())?;
    assert_eq!(activation_at, now().saturating_add(ACTIVATION_DELAY));
    assert_eq!(inputs.corridor.stage(), CorridorStage::TimeDelayed);
    assert_eq!(
        inputs
            .corridor
            .signed()
            .map(|s| s.signature.reference.as_str()),
        Some("vault/corridor/2")
    );

    // Inside the delay: even a small transfer is refused on authority.
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::CorridorAuthority);
    assert!(veto.reason.contains("time_delayed"), "{}", veto.reason);

    // A second early: still refused, by the corridor itself.
    assert!(
        inputs
            .corridor
            .activate(activation_at.saturating_sub(Duration::from_secs(1)))
            .is_err()
    );

    // On the instant: active, and the wider cap is in force.
    inputs.corridor.activate(activation_at)?;
    inputs.now = activation_at;
    inputs.intent = TransferIntent::new(
        treasury(),
        destination()?,
        dec!("1500"),
        intent()?.purpose(),
    )?;
    let approved = inputs.assess();
    assert!(approved.is_ok(), "{approved:?}");
    Ok(())
}

#[test]
fn tighten_caps_refuses_a_set_that_loosens_any_dimension_and_loosen_caps_refuses_one_that_loosens_none()
-> Result<()> {
    // The failure prevented: a "tightening" that raised one cap and lowered
    // five slipping through the delay-free path. Any dimension looser is a
    // loosening. And the converse, so a caller confused about which change it
    // made is told rather than accommodated.
    let mut corridor = active_corridor()?;
    let mixed = CorridorCaps::new(
        dec!("100"),
        dec!("300"),
        dec!("1000"),
        dec!("5000"),
        Duration::from_mins(10), // shorter interval: looser
        PermittedHours::ALL_DAY,
    )?;
    assert!(mixed.is_looser_than(corridor.caps()));
    let err = corridor
        .tighten_caps(mixed.clone(), bob()?, now())
        .err()
        .map(|e| e.message().to_string());
    assert!(
        err.as_deref().is_some_and(|m| m.contains("loosen_caps")),
        "{err:?}"
    );
    assert_eq!(corridor.caps(), &caps()?);
    assert_eq!(corridor.stage(), CorridorStage::Active);

    let strictly_tighter = CorridorCaps::new(
        dec!("100"),
        dec!("300"),
        dec!("1000"),
        dec!("5000"),
        Duration::from_mins(30),
        PermittedHours::new(9, 17)?,
    )?;
    assert!(!strictly_tighter.is_looser_than(corridor.caps()));
    let err = corridor
        .loosen_caps(strictly_tighter, signature(now(), "vault/x")?, now())
        .err()
        .map(|e| e.message().to_string());
    assert!(
        err.as_deref().is_some_and(|m| m.contains("tighten_caps")),
        "{err:?}"
    );
    assert_eq!(corridor.stage(), CorridorStage::Active);
    Ok(())
}

// --- the destination registry ----------------------------------------------

#[test]
fn a_destination_is_unusable_until_twenty_four_hours_after_its_signature_and_usable_on_the_instant()
-> Result<()> {
    // The failure prevented: a destination usable the moment it was signed,
    // which removes the day a human has to notice the wrong address. Checked
    // on either side of the boundary, on the platform clock.
    let registry = usable_registry()?;
    let key = destination()?;
    let usable_from = signed_at().saturating_add(ACTIVATION_DELAY);
    assert_eq!(
        registry.get(&key).map(|r| match &r.status {
            DestinationStatus::Signed { usable_from, .. } => Some(*usable_from),
            _ => None,
        }),
        Some(Some(usable_from))
    );
    let before = usable_from.saturating_sub(Duration::from_secs(1));
    let err = registry
        .usable(&key, before)
        .err()
        .map(|e| e.message().to_string());
    assert!(
        err.as_deref()
            .is_some_and(|m| m.contains("delay has not elapsed")),
        "{err:?}"
    );
    assert!(registry.usable(&key, usable_from).is_ok());
    Ok(())
}

#[test]
fn a_proposed_or_merely_verified_destination_is_unusable_and_the_refusal_names_the_next_step()
-> Result<()> {
    // The failure prevented: "on the allowlist" meaning "somebody typed it
    // in". Each earlier stage refuses and says what would move it on.
    let mut registry = DestinationRegistry::new();
    let key = destination()?;
    let far_future = now().saturating_add(Duration::from_days(30));
    assert!(
        registry.usable(&key, far_future).is_err(),
        "an unknown key was usable"
    );

    registry.propose(key.clone(), alice()?, proposed_at())?;
    let err = registry
        .usable(&key, far_future)
        .err()
        .map(|e| e.message().to_string());
    assert!(
        err.as_deref().is_some_and(|m| m.contains("unverified")),
        "{err:?}"
    );

    registry.verify(&key, bob()?, proposed_at())?;
    let err = registry
        .usable(&key, far_future)
        .err()
        .map(|e| e.message().to_string());
    assert!(
        err.as_deref().is_some_and(|m| m.contains("unsigned")),
        "{err:?}"
    );

    // And it cannot skip a stage: signing a proposed one, verifying twice.
    let mut fresh = DestinationRegistry::new();
    fresh.propose(key.clone(), alice()?, proposed_at())?;
    assert!(
        fresh
            .record_signature(&key, signature(signed_at(), "x")?)
            .is_err()
    );
    assert!(registry.verify(&key, bob()?, proposed_at()).is_err());
    Ok(())
}

#[test]
fn a_revoked_destination_is_unusable_forever_and_cannot_be_re_proposed() -> Result<()> {
    // The failure prevented: an attacker with a proposal credential removing
    // and re-adding a destination to restart it clean.
    let mut registry = usable_registry()?;
    let key = destination()?;
    assert!(registry.usable(&key, now()).is_ok());
    registry.revoke(&key, bob()?, now())?;
    let err = registry
        .usable(&key, now())
        .err()
        .map(|e| e.message().to_string());
    assert!(
        err.as_deref()
            .is_some_and(|m| m.contains("revocation is permanent")),
        "{err:?}"
    );
    let err = registry
        .propose(key.clone(), alice()?, now())
        .err()
        .map(|e| e.message().to_string());
    assert!(
        err.as_deref()
            .is_some_and(|m| m.contains("never re-proposed")),
        "{err:?}"
    );
    assert_eq!(registry.len(), 1);
    Ok(())
}

// --- cap validation ---------------------------------------------------------

#[test]
fn corridor_caps_refuse_a_non_positive_or_out_of_order_value_by_name() -> Result<()> {
    // Premise: the ordered, positive set is accepted.
    assert!(caps().is_ok());
    // The failure prevented: a cap set in which one limit can never bind,
    // reading as a control and being a spare part.
    let attempt = |t: &str, h: &str, d: &str, c: &str| {
        CorridorCaps::new(
            Decimal::parse(t).unwrap_or(Decimal::ZERO),
            Decimal::parse(h).unwrap_or(Decimal::ZERO),
            Decimal::parse(d).unwrap_or(Decimal::ZERO),
            Decimal::parse(c).unwrap_or(Decimal::ZERO),
            Duration::from_mins(15),
            PermittedHours::ALL_DAY,
        )
        .err()
        .map(|e| e.message().to_string())
    };
    let zero = attempt("0", "3000", "10000", "50000");
    assert!(
        zero.as_deref()
            .is_some_and(|m| m.starts_with("max_per_transfer is 0")),
        "{zero:?}"
    );
    let negative = attempt("1000", "3000", "-1", "50000");
    assert!(
        negative
            .as_deref()
            .is_some_and(|m| m.starts_with("max_per_day is -1")),
        "{negative:?}"
    );
    let transfer_over_hour = attempt("4000", "3000", "10000", "50000");
    assert!(
        transfer_over_hour
            .as_deref()
            .is_some_and(|m| m.contains("max_per_transfer (4000) exceeds max_per_hour (3000)")),
        "{transfer_over_hour:?}"
    );
    let hour_over_day = attempt("1000", "20000", "10000", "50000");
    assert!(
        hour_over_day
            .as_deref()
            .is_some_and(|m| m.contains("max_per_hour (20000) exceeds max_per_day (10000)")),
        "{hour_over_day:?}"
    );
    let day_over_cumulative = attempt("1000", "3000", "60000", "50000");
    assert!(
        day_over_cumulative
            .as_deref()
            .is_some_and(|m| m.contains("max_per_day (60000) exceeds max_cumulative (50000)")),
        "{day_over_cumulative:?}"
    );
    // Equal neighbours are in order, not out of it.
    assert!(attempt("1000", "1000", "1000", "1000").is_none());
    Ok(())
}

#[test]
fn permitted_hours_refuse_an_empty_inverted_or_overlong_window() {
    assert!(PermittedHours::new(0, 24).is_ok());
    assert!(
        PermittedHours::new(9, 9).is_err(),
        "an empty window can never fire"
    );
    assert!(PermittedHours::new(17, 9).is_err(), "an inverted window");
    assert!(PermittedHours::new(0, 25).is_err(), "a 25th hour");
}
