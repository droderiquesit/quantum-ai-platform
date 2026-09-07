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
use qip_capital_fabric::custody::{
    Attestation, ClassConstraints, CorridorKind, CustodyClass, CustodyPolicy, EnforcementPoint,
    EnforcementPoints, Identity, PolicyRule, RefusalReason, TransferAuthority,
};
use qip_capital_fabric::destination::{
    ACTIVATION_DELAY, Approver, Asset, DestinationKey, DestinationRegistry, DestinationStatus,
    SignatureRecord,
};
use qip_capital_fabric::gate::{
    AnomalyFlag, CarriedTransfer, CorridorFunding, FundingStanding, GateCheck, KillSwitchState,
    SourceBalances, StatedPurpose, TransferGate, TransferHistory, TransferIntent, VelocityBreaker,
    VelocityState, Vetoed,
};
use qip_capital_fabric::location::{CapitalLocation, Region};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Currency, Decimal, Duration, Timestamp, dec};
use std::collections::BTreeMap;

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

/// The identity that trades. §37.4 forbids it appearing among the attestors.
const TRADING: &str = "trading-svc";

/// The service identity each enforcement point attests under in the satisfied
/// fixture. Three distinct names, none of them [`TRADING`].
fn attesting_identity(point: EnforcementPoint) -> &'static str {
    match point {
        EnforcementPoint::TransferGate => "gate-svc",
        EnforcementPoint::CustodyPolicy => "custody-policy-svc",
        EnforcementPoint::VenueAllowlist => "venue-ops-oob",
    }
}

fn attestation(point: EnforcementPoint, identity: &str) -> Result<Attestation> {
    Attestation::new(
        point,
        Identity::new(identity)?,
        format!("{}-record-1", point.as_str()),
        signed_at(),
    )
}

/// §37.4's closing rule satisfied: all three points attested, under three
/// distinct identities, none of which is the one that trades.
fn authority() -> Result<TransferAuthority> {
    authority_with(|point| Some(attesting_identity(point)))
}

/// A [`TransferAuthority`] whose attestations are whatever `identity` says:
/// `None` leaves the point silent, and a repeated name collapses two points
/// onto one identity.
fn authority_with(
    identity: impl Fn(EnforcementPoint) -> Option<&'static str>,
) -> Result<TransferAuthority> {
    let mut points = EnforcementPoints::new();
    for point in EnforcementPoint::ALL {
        if let Some(name) = identity(point) {
            points.attest(attestation(point, name)?)?;
        }
    }
    Ok(TransferAuthority::new(points, Identity::new(TRADING)?))
}

/// The Intelligence layer's ruling as the satisfied fixture carries it: every
/// strategy the corridor funds has reached the rung that holds capital at full
/// size, so the corridor may carry up to a ceiling above every amount any test
/// here proposes. Deliberately not the binding constraint in the fixture — a
/// ruling that refused first would make every other check's test pass for the
/// wrong reason. A test about the ruling overrides this one field.
fn funding() -> Result<CorridorFunding> {
    CorridorFunding::new(
        FundingStanding::Permitted,
        dec!("50000"),
        "every strategy this corridor funds has reached scaled; the weakest, alpha-1, is at scaled",
    )
}

/// Run the gate with every input satisfied except whatever the caller
/// overrode.
struct Inputs {
    intent: TransferIntent,
    corridor: Corridor,
    registry: DestinationRegistry,
    custody: CustodyPolicy,
    authority: TransferAuthority,
    funding: CorridorFunding,
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
            funding: funding()?,
            authority: authority()?,
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
            &self.authority,
            &self.funding,
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

// --- §37.4's closing rule, through the gate ---------------------------------
//
// `EnforcementPoints::all_agree` and `Agreement::disjoint_from_trading_authority`
// existed with no caller outside these tests: they computed §37.4's closing
// rule and nothing consulted the answer. That is the defect
// `risk-and-execution.md` names — `MaxExpectedShortfall` shipped in every
// default limit set and could never fire — so the tests below drive the gate,
// not the predicate. A test that called `all_agree` directly would have
// passed before the gate was wired to it and after it was unwired again.

#[test]
fn an_intent_whose_enforcement_points_have_not_all_attested_vetoes_on_corridor_authority()
-> Result<()> {
    // The failure prevented: two of three enforcement points agreeing, read as
    // agreement. §37.4 requires all three, and the point that stayed silent is
    // the one an operator needs named — a refusal saying only "the authority
    // is incomplete" sends them to read three service logs.
    //
    // Premise: the fixture with all three attesting is admitted, so what each
    // iteration below refuses is the missing attestation and not the fixture.
    let satisfied = Inputs::satisfied()?;
    assert!(
        satisfied.assess().is_ok(),
        "premise: the fixture with three distinct attestations is admitted"
    );

    for missing in EnforcementPoint::ALL {
        let mut inputs = Inputs::satisfied()?;
        inputs.authority =
            authority_with(|point| (point != missing).then(|| attesting_identity(point)))?;
        // Premise for this iteration: exactly the other two attested.
        assert!(inputs.authority.points().attestation(missing).is_none());
        assert_eq!(
            EnforcementPoint::ALL
                .iter()
                .filter(|point| inputs.authority.points().attestation(**point).is_some())
                .count(),
            2
        );

        let veto = inputs.veto();
        assert_eq!(veto.check, GateCheck::CorridorAuthority);
        assert!(veto.alert, "a corridor-authority failure alerts, per §37.3");
        // Parenthesised so the token is delimited: `enforcement_point_missing`
        // is otherwise a substring of nothing here today and of whatever a
        // later `enforcement_point_missing_at_venue` would be called.
        assert!(
            veto.reason.contains(&format!(
                "({})",
                RefusalReason::EnforcementPointMissing { point: missing }.as_str()
            )),
            "{}",
            veto.reason
        );
        assert!(
            veto.reason.contains(&format!("{missing} has not attested")),
            "{}",
            veto.reason
        );
    }
    Ok(())
}

#[test]
fn an_intent_whose_enforcement_points_share_an_identity_vetoes_on_corridor_authority() -> Result<()>
{
    // The failure §37.4 names outright: the gate and the custody policy
    // deployed under one service identity count as two approvals while being
    // one decision made twice. Every pair is tried, so a check that compared
    // only adjacent points would be caught.
    //
    // Premise: three distinct identities are admitted.
    assert!(
        Inputs::satisfied()?.assess().is_ok(),
        "premise: three distinct attesting identities are admitted"
    );

    let pairs = [
        (
            EnforcementPoint::TransferGate,
            EnforcementPoint::CustodyPolicy,
        ),
        (
            EnforcementPoint::TransferGate,
            EnforcementPoint::VenueAllowlist,
        ),
        (
            EnforcementPoint::CustodyPolicy,
            EnforcementPoint::VenueAllowlist,
        ),
    ];
    for (first, second) in pairs {
        let mut inputs = Inputs::satisfied()?;
        inputs.authority = authority_with(|point| {
            Some(if point == first || point == second {
                "shared-svc"
            } else {
                attesting_identity(point)
            })
        })?;
        // Premise: all three attested, so this is a refusal of the collapse
        // and not of a silent point.
        assert!(
            EnforcementPoint::ALL.iter().all(|point| inputs
                .authority
                .points()
                .attestation(*point)
                .is_some())
        );

        let veto = inputs.veto();
        assert_eq!(veto.check, GateCheck::CorridorAuthority);
        assert!(veto.alert);
        assert!(
            veto.reason.contains(&format!(
                "({})",
                RefusalReason::SharedIdentity { first, second }.as_str()
            )),
            "{}",
            veto.reason
        );
        assert!(
            veto.reason
                .contains(&format!("{first} and {second} both attested as shared-svc")),
            "{}",
            veto.reason
        );
    }
    Ok(())
}

#[test]
fn an_intent_attested_by_the_trading_identity_vetoes_on_corridor_authority() -> Result<()> {
    // "Trading authority and transfer authority never share an identity."
    // The failure prevented is the one the blueprint states: a compromised or
    // runaway trading process that can also attest to its own capital
    // movement needs no second credential to move money out.
    //
    // Checked separately from the pairwise distinctness because three
    // identities differing from each other says nothing about whether one of
    // them trades — each point is put in the trading identity's place in turn,
    // and each must be refused.
    assert!(
        Inputs::satisfied()?.assess().is_ok(),
        "premise: no attestor is the trading identity in the satisfied fixture"
    );

    for attestor in EnforcementPoint::ALL {
        let mut inputs = Inputs::satisfied()?;
        inputs.authority = authority_with(|point| {
            Some(if point == attestor {
                TRADING
            } else {
                attesting_identity(point)
            })
        })?;
        // Premise: the three identities are still pairwise distinct, so the
        // pairwise check cannot be what fires and the refusal below is about
        // trading authority specifically.
        assert!(
            inputs.authority.points().all_agree().is_ok(),
            "premise: the three attesting identities are pairwise distinct"
        );

        let veto = inputs.veto();
        assert_eq!(veto.check, GateCheck::CorridorAuthority);
        assert!(veto.alert);
        assert!(
            veto.reason.contains(&format!(
                "({})",
                RefusalReason::TradingIdentityHoldsTransferAuthority { point: attestor }.as_str()
            )),
            "{}",
            veto.reason
        );
        assert!(
            veto.reason
                .contains(&format!("{attestor} attested as {TRADING}")),
            "{}",
            veto.reason
        );
    }
    Ok(())
}

#[test]
fn an_admitted_assessment_records_the_three_identities_it_was_admitted_on() -> Result<()> {
    // The agreement the gate computes is kept on the `Approved` rather than
    // discarded. A gate that checked the rule and recorded only that it held
    // would leave "who authorised this movement" answerable from nothing but
    // the gate's own word, which is the second source of truth the boundaries
    // rule refuses.
    let inputs = Inputs::satisfied()?;
    let approved = match inputs.assess() {
        Ok(approved) => approved,
        Err(veto) => panic!("the satisfied fixture was vetoed: {veto}"),
    };
    let attested: Vec<(EnforcementPoint, &str)> = approved
        .authority()
        .attestations()
        .iter()
        .map(|attestation| (attestation.point, attestation.identity.as_str()))
        .collect();
    assert_eq!(
        attested,
        EnforcementPoint::ALL
            .iter()
            .map(|point| (*point, attesting_identity(*point)))
            .collect::<Vec<_>>()
    );
    // And none of them is the identity that trades — asserted on the record
    // rather than on the fixture, because it is the record an operator reads.
    assert!(
        attested.iter().all(|(_, identity)| *identity != TRADING),
        "the approval names the trading identity among its attestors: {attested:?}"
    );
    Ok(())
}

#[test]
fn a_corridor_the_intelligence_layer_suspended_is_vetoed_on_check_one_with_every_other_input_satisfied()
-> Result<()> {
    // The failure prevented, and it is the one this input exists for: the
    // fabric held corridors as records — signed, allowlisted, capped,
    // attested — that nothing measured against a policy. A corridor whose
    // strategies have all been retired satisfies every one of those records
    // exactly as well as one funding a scaled book, so before the ruling
    // reached the gate a retired book's corridor was admitted at full size.
    let mut inputs = Inputs::satisfied()?;
    inputs.funding = CorridorFunding::new(
        FundingStanding::Suspended,
        Decimal::ZERO,
        "alpha-1 is at retired and holds no capital, so this corridor has nothing to fund",
    )?;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::CorridorAuthority);
    assert!(
        veto.alert,
        "§37.3 pairs a check 1 veto with an alert, and a corridor that should never have \
         generated an intent is exactly that case"
    );
    // The reason must carry the deriving layer's own words, not a summary:
    // an operator reading "suspended" alone would go looking for a corridor
    // fault, which is the wrong problem. The rung is what they need.
    assert!(
        veto.reason.contains("alpha-1 is at retired"),
        "the veto does not name the rung that decided it: {}",
        veto.reason
    );
    Ok(())
}

#[test]
fn an_amount_above_the_narrowed_ceiling_is_vetoed_on_caps_though_every_signed_cap_admits_it()
-> Result<()> {
    // The failure prevented: a pilot-rung strategy is "live with capital,
    // deliberately limited", and a corridor that keeps its full signed
    // ceiling while the strategy behind it is limited has undone the limit.
    // The signed caps cannot express this — they were signed before the rung
    // moved and are wider on purpose.
    let mut inputs = Inputs::satisfied()?;
    inputs.funding = CorridorFunding::new(
        FundingStanding::Narrowed,
        dec!("100"),
        "alpha-1 is at pilot, which is live with capital and deliberately limited",
    )?;
    // Premise: the amount is inside every cap the desk signed, so the veto
    // below can only be the derived ceiling. Without this the test would pass
    // on a fixture the per-transfer cap already refused.
    assert!(inputs.intent.amount() < caps()?.max_per_transfer());
    assert!(inputs.intent.amount() < caps()?.max_per_hour());
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::Caps);
    assert!(
        veto.reason.contains("narrowed ceiling of 100"),
        "the veto does not name the ceiling that refused it: {}",
        veto.reason
    );
    assert!(
        veto.reason.contains("alpha-1 is at pilot"),
        "the veto does not name the rung that set the ceiling: {}",
        veto.reason
    );
    Ok(())
}

#[test]
fn a_ruling_that_suspends_a_corridor_and_gives_it_a_ceiling_is_refused_by_the_gate_as_well_as_by_its_constructor()
-> Result<()> {
    // The failure prevented is the one `CustodyPolicy::conforms` and
    // `TransferAuthority::agreement` already document: a `CorridorFunding`
    // travels inside a `GateCommand` and arrives deserialised off the event
    // log, where the constructor never runs. A ruling that says "suspended"
    // and carries a ceiling of 400 is two claims about the same fact, and
    // whichever check was asked first would decide — check 1 would refuse it
    // and check 2 would admit 400 through a corridor carrying nothing.
    let refused = CorridorFunding::new(
        FundingStanding::Suspended,
        dec!("400"),
        "alpha-1 is at retired and holds no capital",
    );
    assert!(
        refused.is_err(),
        "the constructor admitted a suspended corridor with a ceiling"
    );
    // Off the log, past the constructor, exactly as a replay would build it.
    let malformed: CorridorFunding = serde_json::from_str(
        r#"{"standing":"suspended","permitted":"400","reason":"alpha-1 is at retired and holds no capital"}"#,
    )
    .map_err(|err| qip_core::error::Error::invalid(err.to_string()))?;
    assert_eq!(
        malformed.permitted(),
        dec!("400"),
        "premise: serde built the contradiction the constructor refuses"
    );
    let mut inputs = Inputs::satisfied()?;
    inputs.funding = malformed;
    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::CorridorAuthority);
    assert!(
        veto.reason.contains("contradicts itself"),
        "the veto is not the well-formedness refusal: {}",
        veto.reason
    );
    Ok(())
}

#[test]
fn an_admitted_assessment_records_the_ruling_it_was_admitted_under() -> Result<()> {
    // The same reason the agreement is kept: an approval read six months
    // later must say which standing the corridor was on at the time, not
    // which one it is on now. A rung moves; the record does not.
    let inputs = Inputs::satisfied()?;
    let approved = match inputs.assess() {
        Ok(approved) => approved,
        Err(veto) => panic!("the satisfied fixture was vetoed: {veto}"),
    };
    assert_eq!(approved.funding(), &funding()?);
    assert_eq!(approved.funding().standing(), FundingStanding::Permitted);
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

// --- inputs that arrive deserialised, past their own constructors -----------
//
// Every argument the gate takes is supplied by the caller, and on a replay the
// caller is `crate::replay` handing back a `GateCommand` decoded from the
// event log. `serde` builds each of those types from its fields and calls no
// constructor, so a rule held only by `CustodyPolicy::from_constraints`,
// `TransferHistory::new`, `SourceBalances::new` or `TransferIntent::new` is a
// rule the replay never re-derives — and the replay is the thing that has to
// catch a record written by something other than the control. Worse than
// merely uncaught: the replay *re-executes* the command and compares
// outcomes, so a gate that did not re-ask would confirm the forged admission
// and the chain would verify.
//
// Each test below therefore builds the tampered value the only way it can be
// built — through serde — asserts as its premise that the old checks admit
// it, and asserts the gate now vetoes on the named check.

/// The blueprint table as rows, for a test that wants to break one.
fn blueprint_rows() -> BTreeMap<CustodyClass, ClassConstraints> {
    let blueprint = CustodyPolicy::blueprint();
    CustodyClass::ALL
        .into_iter()
        .filter_map(|class| blueprint.constraints(class).map(|row| (class, row.clone())))
        .collect()
}

/// Round-trip `value` through JSON into `T`, which is the path every gate
/// input takes on a replay: serialised into the log, deserialised out of it,
/// no constructor in between.
fn off_the_log<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T> {
    Ok(serde_json::from_value(value)?)
}

/// A custody table as a JSON object, so a test can hand the gate one that
/// `CustodyPolicy::from_constraints` refuses to build.
fn policy_json(rows: &BTreeMap<CustodyClass, ClassConstraints>) -> Result<serde_json::Value> {
    let mut object = serde_json::Map::new();
    object.insert("classes".to_string(), serde_json::to_value(rows)?);
    Ok(serde_json::Value::Object(object))
}

#[test]
fn a_custody_table_off_the_log_that_makes_collateral_transferable_is_vetoed_rather_than_believed()
-> Result<()> {
    // The sharpest instance, because here the pre-existing check answers
    // *yes*: `permits` reads the row it is given, and the row says collateral
    // may leave through an institution approval flow. §37.4 says collateral is
    // inventory and never a transfer source at all, `from_constraints` refuses
    // to record a table saying otherwise, and before `conforms` was re-run by
    // the gate that refusal lived only in a constructor no replayed record
    // calls. A forged gate record carrying this table would have been admitted
    // by the control and then confirmed by the replay.
    let mut rows = blueprint_rows();
    let collateral = rows
        .get_mut(&CustodyClass::CollateralAndMargin)
        .expect("the blueprint table has a collateral row");
    collateral.may_be_transfer_source = true;
    collateral
        .permitted_corridors
        .insert(CorridorKind::InstitutionApprovalFlow);
    // Premise: no constructor in this crate will build this table, so serde is
    // the only way it can reach the gate — which is exactly the replay path.
    assert!(
        CustodyPolicy::from_constraints(rows.clone()).is_err(),
        "premise: from_constraints refuses a transferable collateral row"
    );
    let tampered: CustodyPolicy = off_the_log(policy_json(&rows)?)?;
    // Premise: the per-question check admits it. Without this the test could
    // pass on `permits` refusing, and would then prove nothing about
    // `conforms`.
    assert!(
        tampered
            .permits(
                CustodyClass::CollateralAndMargin,
                CorridorKind::InstitutionApprovalFlow
            )
            .is_ok(),
        "premise: permits answers yes on the tampered table"
    );

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
    assert_eq!(corridor.stage(), CorridorStage::Active);
    inputs.corridor = corridor;
    inputs.custody = tampered;

    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::CorridorAuthority);
    assert!(veto.alert);
    // The delimited token, and the rule by name. `contains` on a bare word
    // would match the neighbouring reasons; the parenthesised form cannot.
    assert!(
        veto.reason.contains("(policy_contradicts_blueprint)"),
        "{}",
        veto.reason
    );
    assert!(
        veto.reason
            .contains("collateral and margin are inventory and never a transfer source"),
        "{}",
        veto.reason
    );
    // And it is the table that was refused, not the question: `class_never_transfers`
    // is what the *untampered* table would have said, and it cannot fire here.
    assert!(
        !veto.reason.contains("(class_never_transfers)"),
        "the veto must come from the table's own rules, not from the row it no longer has: {}",
        veto.reason
    );
    assert_eq!(
        CustodyPolicy::blueprint()
            .conforms()
            .map_err(|refusal| refusal.reason),
        Ok(()),
        "premise: the blueprint table conforms, so conforms() is not refusing everything"
    );
    Ok(())
}

#[test]
fn a_custody_table_off_the_log_marking_self_custody_single_party_vetoes_even_an_unrelated_fiat_transfer()
-> Result<()> {
    // §37.4's self-custody rule is "no single component can sign". A table
    // that denies it is not a table with one bad row to be routed around: it
    // is a table this platform will not answer questions from, including
    // questions about fiat whose own row is untouched. The failure prevented
    // is a policy edited to single-party in one row and relied on in another,
    // which reads as a custody boundary and is a record of one.
    let mut inputs = Inputs::satisfied()?;
    // Premise: with the blueprint table these exact inputs are admitted, so
    // the veto below is caused by the table and by nothing else.
    assert!(
        inputs.assess().is_ok(),
        "premise: the satisfied fixture is admitted before the table is broken"
    );

    let mut rows = blueprint_rows();
    rows.get_mut(&CustodyClass::CryptoSelfCustody)
        .expect("the blueprint table has a self-custody row")
        .requires_multi_party_release = false;
    let tampered: CustodyPolicy = off_the_log(policy_json(&rows)?)?;
    // Premise: the fiat row still answers yes, so nothing about this
    // corridor's own question has changed.
    assert!(
        tampered
            .permits(
                CustodyClass::FiatAtInstitutionOfRecord,
                CorridorKind::InstitutionApprovalFlow
            )
            .is_ok(),
        "premise: the fiat row is untouched and still permits the corridor"
    );
    inputs.custody = tampered;

    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::CorridorAuthority);
    assert!(veto.alert);
    assert!(
        veto.reason.contains("no single component can sign"),
        "{}",
        veto.reason
    );
    // The rule is named as a token an operator can grep and a metric can
    // label, not only as prose.
    assert_eq!(
        PolicyRule::SelfCustodyIsMultiParty.as_str(),
        "self_custody_is_multi_party"
    );
    match CustodyPolicy::from_constraints(rows) {
        Err(_) => {}
        Ok(_) => panic!("from_constraints must refuse the same table the gate refuses"),
    }
    Ok(())
}

#[test]
fn a_carried_history_off_the_log_in_the_wrong_order_vetoes_instead_of_clearing_the_interval()
-> Result<()> {
    // `TransferHistory::new` sorts, so oldest-first is an invariant nothing
    // states once the value is deserialised — and `last_carried_at` takes the
    // last element. Reversed, the history names a transfer two hours old as
    // the latest one, and the fifteen-minute minimum interval is cleared by a
    // corridor that moved capital ten minutes ago.
    let recent = CarriedTransfer {
        at: now().saturating_sub(Duration::from_mins(10)),
        amount: dec!("900"),
    };
    let older = CarriedTransfer {
        at: now().saturating_sub(Duration::from_hours(2)),
        amount: dec!("900"),
    };
    let mut inputs = Inputs::satisfied()?;
    inputs.history = TransferHistory::new(vec![recent, older])?;
    // Premise: in the right order this history vetoes on check 3, so the
    // interval check is live and the tampering below has something to defeat.
    let ordered_veto = inputs.veto();
    assert_eq!(ordered_veto.check, GateCheck::MinimumInterval);

    let mut json = serde_json::to_value(&inputs.history)?;
    let carried = json
        .get_mut("carried")
        .and_then(serde_json::Value::as_array_mut)
        .expect("a history serialises with a carried array");
    assert_eq!(carried.len(), 2, "premise: both transfers were serialised");
    carried.reverse();
    let tampered: TransferHistory = off_the_log(json)?;
    // Premise: the tampering does exactly one thing — it moves the latest
    // transfer out of last place, which is what the interval check reads.
    assert_eq!(
        tampered.last_carried_at(),
        Some(older.at),
        "premise: the reversed history names the older transfer as the last one"
    );
    assert_eq!(tampered.carried_total(), dec!("1800"));
    inputs.history = tampered;

    let veto = inputs.veto();
    assert_eq!(
        veto.check,
        GateCheck::Caps,
        "a history that is not oldest-first is refused before the caps it would distort \
         are measured, not silently re-sorted"
    );
    assert!(
        veto.reason.contains("a history is oldest first"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn a_carried_history_off_the_log_recording_a_refund_vetoes_instead_of_freeing_the_cumulative_cap()
-> Result<()> {
    // A negative carried amount subtracts from `carried_total`, and
    // `carried_total` is what the cumulative cap is measured against. An
    // exhausted corridor would come back to life.
    let mut inputs = Inputs::satisfied()?;
    // Three days back, so the rolling hour and the rolling day are empty and
    // the cumulative cap is the only one this history can reach.
    let spent = CarriedTransfer {
        at: now().saturating_sub(Duration::from_days(3)),
        amount: dec!("49800"),
    };
    inputs.history = TransferHistory::new(vec![spent])?;
    // Premise: the corridor is exhausted — 49,800 carried against a 50,000
    // cumulative cap leaves no room for the 500 the intent asks for.
    let exhausted = inputs.veto();
    assert_eq!(exhausted.check, GateCheck::Caps);
    assert!(
        exhausted.reason.contains("cumulative cap"),
        "{}",
        exhausted.reason
    );

    let mut json = serde_json::to_value(&inputs.history)?;
    let entry = json
        .get_mut("carried")
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|carried| carried.first_mut())
        .expect("a history serialises with a carried array");
    entry
        .as_object_mut()
        .expect("a carried transfer serialises as an object")
        .insert("amount".to_string(), serde_json::to_value(dec!("-49800"))?);
    let tampered: TransferHistory = off_the_log(json)?;
    // Premise: flipped, the cumulative cap has room again, so nothing but the
    // well-formedness rule can be what refuses.
    assert_eq!(tampered.carried_total(), dec!("-49800"));
    assert!(
        tampered.carried_total() + dec!("500") < dec!("50000"),
        "premise: the tampered history leaves the cumulative cap unreached"
    );
    inputs.history = tampered;

    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::Caps);
    assert!(
        veto.reason
            .contains("history records what left, and nothing else"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn source_balances_off_the_log_with_a_negative_claim_veto_instead_of_funding_the_transfer()
-> Result<()> {
    // A claim on a balance is subtracted by `free`, so a negative one is
    // added. `SourceBalances::new` refuses it and a replayed record never
    // calls `new`: the sufficiency check — the one thing standing between an
    // intent and a source that cannot fund it — would admit a transfer of
    // money that is not there.
    let mut inputs = Inputs::satisfied()?;
    inputs.balances = SourceBalances::new(dec!("100"), dec!("0"), dec!("0"), dec!("0"))?;
    // Premise: with an honest balance of 100 the 500 the intent asks for is
    // refused by check 5.
    let poor = inputs.veto();
    assert_eq!(poor.check, GateCheck::SourceBalance);

    let mut json = serde_json::to_value(inputs.balances)?;
    json.as_object_mut()
        .expect("balances serialise as an object")
        .insert(
            "reserved".to_string(),
            serde_json::to_value(dec!("-10000"))?,
        );
    let tampered: SourceBalances = off_the_log(json)?;
    // Premise: the negative claim has made the source look funded, so an
    // admission here would be the gate believing arithmetic it was handed.
    assert!(
        tampered.free() > inputs.intent.amount(),
        "premise: the tampered balances free {} against an intent of {}",
        tampered.free(),
        inputs.intent.amount()
    );
    inputs.balances = tampered;

    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::SourceBalance);
    assert!(
        veto.reason
            .contains("a claim on a balance cannot be negative"),
        "{}",
        veto.reason
    );
    assert!(
        veto.reason.contains("reserved is -10000"),
        "{}",
        veto.reason
    );
    Ok(())
}

#[test]
fn an_intent_off_the_log_asking_for_nothing_vetoes_instead_of_passing_every_cap_vacuously()
-> Result<()> {
    // Every cap is an upper bound, so an amount of zero is under all four of
    // them and inside any balance. `TransferIntent::new` refuses a
    // non-positive amount; a record decoded off the log does not go through
    // it, and the assessment would be admitted — an approval on the record
    // for a transfer nobody asked for, against a corridor's signature.
    let mut inputs = Inputs::satisfied()?;
    let mut json = serde_json::to_value(&inputs.intent)?;
    json.as_object_mut()
        .expect("an intent serialises as an object")
        .insert("amount".to_string(), serde_json::to_value(dec!("0"))?);
    let tampered: TransferIntent = off_the_log(json)?;
    // Premise: the amount really is zero and really is under the per-transfer
    // cap, so no other check can be what refuses.
    assert_eq!(tampered.amount(), dec!("0"));
    assert!(tampered.amount() < caps()?.max_per_transfer());
    assert!(tampered.amount() < inputs.balances.free());
    inputs.intent = tampered;

    let veto = inputs.veto();
    assert_eq!(veto.check, GateCheck::Caps);
    assert!(
        veto.reason
            .contains("a transfer of nothing is not a transfer"),
        "{}",
        veto.reason
    );
    Ok(())
}
