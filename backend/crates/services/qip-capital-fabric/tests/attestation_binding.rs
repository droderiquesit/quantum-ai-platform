//! §37.4's other two references: the half of the custody boundary that proved
//! three points *spoke* and read what only one of them said.
//!
//! `EnforcementPoints::all_agree` proves three enforcement points attested
//! under three identities. What each agreed *to* is its
//! `Attestation::reference`, and until ADR 0051 that field was validated only
//! by `Attestation::new`, for being non-empty. The venue's allowlist was bound
//! to the destination (`custody_mirror.rs`); the transfer gate's and the
//! custody policy's were bound to nothing at all. So an approval could name
//! three identities as having agreed to a movement while two of them had
//! agreed to nothing in particular — the same defect the venue mirror closed,
//! two rows along, and this time in the two points that are the platform's
//! own.
//!
//! Every test here asserts its premise first, and the premise is always the
//! same shape and is the half that matters: the *correct* reference is
//! admitted. A check that refused every reference would pass a test asserting
//! only that a wrong one is refused, and would be a control that has stopped
//! the platform rather than one that protects it.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital_fabric::assessment::AssessmentId;
use qip_capital_fabric::corridor::{Corridor, CorridorCaps, CorridorId, PermittedHours};
use qip_capital_fabric::custody::{
    Attestation, ClassConstraints, CorridorKind, Custodian, CustodyClass, CustodyPolicy,
    EnforcementPoint, EnforcementPoints, Identity, RefusalReason, TransferAuthority,
};
use qip_capital_fabric::destination::{
    ACTIVATION_DELAY, Approver, Asset, DestinationKey, DestinationRegistry, SignatureRecord,
};
use qip_capital_fabric::gate::{
    Approved, CorridorFunding, FundingStanding, GateCheck, KillSwitchState, SourceBalances,
    StatedPurpose, TransferGate, TransferHistory, TransferIntent, VelocityState, Vetoed,
};
use qip_capital_fabric::location::{CapitalLocation, Region};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Duration, Timestamp, dec};
use std::collections::{BTreeMap, BTreeSet};

// --- fixtures ---------------------------------------------------------------

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

fn treasury() -> CapitalLocation {
    CapitalLocation::new(Region::new("namr"), Currency::USD, VenueId::new("TREASURY"))
}

fn destination() -> Result<DestinationKey> {
    DestinationKey::new(Asset::new("USD")?, "BANK-XYZ-ACCT-1")
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

fn usable_registry() -> Result<DestinationRegistry> {
    let mut registry = DestinationRegistry::new();
    let key = destination()?;
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
    Ok(registry)
}

fn active_corridor() -> Result<Corridor> {
    let mut corridor = Corridor::propose(
        CorridorId::new("treasury-to-xyz")?,
        treasury(),
        CustodyClass::FiatAtInstitutionOfRecord,
        CorridorKind::InstitutionApprovalFlow,
        destination()?,
        caps()?,
        "fund the XYZ margin account ahead of forecast demand",
        Approver::new("alice")?,
        proposed_at(),
    )?;
    corridor.review(
        Approver::new("bob")?,
        proposed_at().saturating_add(Duration::from_hours(1)),
    )?;
    corridor.record_signature(SignatureRecord::new(
        Approver::new("carol")?,
        signed_at(),
        "vault/corridor/1",
    )?)?;
    corridor.begin_delay(signed_at())?;
    corridor.activate(signed_at().saturating_add(ACTIVATION_DELAY))?;
    Ok(corridor)
}

fn intent_of(amount: Decimal) -> Result<TransferIntent> {
    TransferIntent::new(
        treasury(),
        destination()?,
        amount,
        StatedPurpose::new(dec!("1000"), dec!("500"))?,
    )
}

fn funding() -> Result<CorridorFunding> {
    CorridorFunding::new(
        FundingStanding::Permitted,
        dec!("900"),
        "every strategy this corridor funds has reached scaled; the weakest, alpha-1, is at scaled",
    )
}

/// The three attestations, under three distinct identities none of which
/// trades, referencing exactly what the caller says.
fn authority(gate_reference: &str, custody_reference: &str) -> Result<TransferAuthority> {
    let mut points = EnforcementPoints::new();
    for (point, identity, reference) in [
        (EnforcementPoint::TransferGate, "gate-svc", gate_reference),
        (
            EnforcementPoint::CustodyPolicy,
            "custody-policy-svc",
            custody_reference,
        ),
        (
            EnforcementPoint::VenueAllowlist,
            "venue-ops-oob",
            "venue_allowlist-record-1",
        ),
    ] {
        points.attest(Attestation::new(
            point,
            Identity::new(identity)?,
            reference,
            signed_at(),
        )?)?;
    }
    Ok(TransferAuthority::new(
        points,
        Identity::new("trading-svc")?,
    ))
}

/// Assess `amount` at `at` under `custody`, with the two bound references the
/// caller supplies and every other input satisfied.
fn assess(
    amount: Decimal,
    at: Timestamp,
    custody: &CustodyPolicy,
    gate_reference: &str,
    custody_reference: &str,
) -> Result<std::result::Result<Approved, Vetoed>> {
    Ok(TransferGate::assess(
        &intent_of(amount)?,
        &active_corridor()?,
        &usable_registry()?,
        custody,
        &authority(gate_reference, custody_reference)?,
        &funding()?,
        &TransferHistory::empty(),
        &SourceBalances::new(dec!("10000"), dec!("1000"), dec!("1000"), dec!("1000"))?,
        VelocityState::CLEAR,
        KillSwitchState::Armed,
        at,
    ))
}

/// The identity of the assessment of `amount` at `at` on the fixture corridor.
fn identity_of(amount: Decimal, at: Timestamp) -> Result<AssessmentId> {
    Ok(AssessmentId::of(
        active_corridor()?.id(),
        &treasury(),
        &destination()?,
        amount,
        at,
    ))
}

// --- the transfer gate's reference ------------------------------------------

/// The failure this prevents: an attestation made about one movement satisfies
/// the agreement for a different movement, so the record names an identity as
/// having agreed to a transfer it was never shown.
///
/// The wrong reference here is not junk — it is a *real* attestation, filed by
/// the same identity, for a real assessment of the same amount on the same
/// corridor to the same destination, differing only in the instant it was
/// made about. That is the case a check on shape rather than on value would
/// pass, and it is the one an attestor could produce by replaying yesterday's
/// paperwork.
#[test]
fn an_attestation_naming_another_assessments_identity_is_vetoed_on_corridor_authority() -> Result<()>
{
    let policy = CustodyPolicy::blueprint();
    let fingerprint = policy.fingerprint().to_string();
    let later = now().saturating_add(Duration::from_hours(1));

    // Premise, and it is the whole half that distinguishes a working binding
    // from one that refuses everything: each assessment, attested with its
    // own identity, is admitted.
    let mine = identity_of(dec!("500"), now())?;
    let theirs = identity_of(dec!("500"), later)?;
    assert_ne!(
        mine, theirs,
        "premise: two assessments an hour apart must have different identities, or the \
         substitution below is not a substitution"
    );
    assert!(
        assess(dec!("500"), now(), &policy, mine.as_str(), &fingerprint)?.is_ok(),
        "premise: the assessment attested with its own identity is admitted"
    );
    assert!(
        assess(dec!("500"), later, &policy, theirs.as_str(), &fingerprint)?.is_ok(),
        "premise: the other assessment is a real one and is admitted on its own attestation"
    );

    // Now file the second assessment's attestation against the first.
    let vetoed = assess(dec!("500"), now(), &policy, theirs.as_str(), &fingerprint)?
        .expect_err("an attestation about another assessment must be vetoed");
    assert_eq!(
        vetoed.check,
        GateCheck::CorridorAuthority,
        "the binding is part of check 1's question of whether this corridor may carry this at \
         all; got: {vetoed}"
    );
    assert!(
        vetoed.alert,
        "§37.3 pairs a corridor-authority veto with an alert: something reached the gate that \
         should not have been generated"
    );
    // Parenthesised so the token is delimited. A bare `contains` on the reason
    // token is the trap `01-testing-strategy.md` records: one reason's name is
    // a substring of the next one somebody adds.
    assert!(
        vetoed.reason.contains(&format!(
            "({})",
            RefusalReason::GateAttestationNamesAnotherAssessment.as_str()
        )),
        "the veto must name the reason as a token an operator can grep: {}",
        vetoed.reason
    );
    assert!(
        vetoed.reason.contains(theirs.as_str()) && vetoed.reason.contains(mine.as_str()),
        "the veto must name both the identity attested and the identity expected, or an \
         operator cannot tell which of the two is wrong: {}",
        vetoed.reason
    );
    Ok(())
}

/// Every field the identity is derived from changes it, and nothing else does.
///
/// The failure this prevents is a digest that ignores one of its inputs — a
/// binding that looked bound and let an attestation about a $1 transfer
/// satisfy a $1,000,000 one because only the corridor and the clock reached
/// the hash. Each of the five is varied on its own, so a dropped field is
/// named rather than merely counted.
#[test]
fn the_assessment_identity_moves_with_every_field_the_movement_is_made_of() -> Result<()> {
    let corridor = active_corridor()?;
    let base = AssessmentId::of(
        corridor.id(),
        &treasury(),
        &destination()?,
        dec!("500"),
        now(),
    );

    let elsewhere = CapitalLocation::new(Region::new("emea"), Currency::USD, VenueId::new("OTHER"));
    let other_destination = DestinationKey::new(Asset::new("USD")?, "BANK-XYZ-ACCT-2")?;
    let variants: Vec<(&str, AssessmentId)> = vec![
        (
            "corridor",
            AssessmentId::of(
                &CorridorId::new("treasury-to-abc")?,
                &treasury(),
                &destination()?,
                dec!("500"),
                now(),
            ),
        ),
        (
            "source",
            AssessmentId::of(
                corridor.id(),
                &elsewhere,
                &destination()?,
                dec!("500"),
                now(),
            ),
        ),
        (
            "destination",
            AssessmentId::of(
                corridor.id(),
                &treasury(),
                &other_destination,
                dec!("500"),
                now(),
            ),
        ),
        (
            "amount",
            AssessmentId::of(
                corridor.id(),
                &treasury(),
                &destination()?,
                dec!("500.000000001"),
                now(),
            ),
        ),
        (
            "instant",
            AssessmentId::of(
                corridor.id(),
                &treasury(),
                &destination()?,
                dec!("500"),
                now().saturating_add(Duration::from_nanos(1)),
            ),
        ),
    ];

    for (field, variant) in &variants {
        assert_ne!(
            *variant, base,
            "an assessment differing only in its {field} shares the base identity, so an \
             attestation about one binds to the other"
        );
    }
    // Pairwise too, so a digest that mapped two different fields onto one
    // value is caught rather than only a digest that ignored one.
    let distinct: BTreeSet<&AssessmentId> = variants
        .iter()
        .map(|(_, id)| id)
        .chain(std::iter::once(&base))
        .collect();
    assert_eq!(
        distinct.len(),
        variants.len() + 1,
        "two of the six assessments share one identity: {variants:?}"
    );

    // And it is stable: derived twice from the same movement, it is the same
    // value. A digest that moved between two derivations would refuse the
    // attestation filed against it moments earlier.
    assert_eq!(
        base,
        AssessmentId::of(
            corridor.id(),
            &treasury(),
            &destination()?,
            dec!("500"),
            now()
        ),
        "the identity is not stable across two derivations from the same movement"
    );
    Ok(())
}

/// The admitted record carries the identity the attestation was held to.
///
/// Without this the value the control compared against exists only inside the
/// check, and an operator reconciling an approval against the attestation
/// beside it has nothing but the gate's own word that the two matched — which
/// is the second source of truth `00-boundaries.md` refuses, inverted.
#[test]
fn an_admitted_assessment_records_the_identity_its_attestation_was_bound_to() -> Result<()> {
    let policy = CustodyPolicy::blueprint();
    let expected = identity_of(dec!("500"), now())?;
    let approved = assess(
        dec!("500"),
        now(),
        &policy,
        expected.as_str(),
        &policy.fingerprint().to_string(),
    )?
    .map_err(|vetoed| Error::invalid(vetoed.to_string()))?;
    assert_eq!(
        approved.assessment(),
        &expected,
        "the approval must record the assessment identity the gate bound the attestation to"
    );
    // And it is the identity the attestation actually named, read off the
    // record rather than off the fixture.
    let attested = approved
        .authority()
        .attestation(EnforcementPoint::TransferGate)
        .ok_or_else(|| Error::invalid("an admitted assessment carries the gate's attestation"))?;
    assert_eq!(attested.reference, approved.assessment().as_str());
    Ok(())
}

// --- the custody policy's reference -----------------------------------------

/// A table that conforms to §37.4 and permits this corridor, and is still not
/// the table the custody point attested to.
///
/// Built from the blueprint with one corridor removed from a row the fixture
/// does not use, so that `conforms` passes, `permits` passes, and the *only*
/// thing that differs is which table it is.
fn a_different_conforming_table() -> Result<CustodyPolicy> {
    let blueprint = CustodyPolicy::blueprint();
    let mut rows: BTreeMap<CustodyClass, ClassConstraints> = CustodyClass::ALL
        .into_iter()
        .filter_map(|class| blueprint.constraints(class).map(|row| (class, row.clone())))
        .collect();
    let fiat = rows
        .get_mut(&CustodyClass::FiatAtInstitutionOfRecord)
        .ok_or_else(|| Error::invalid("the blueprint has a fiat row"))?;
    let removed = fiat
        .permitted_corridors
        .remove(&CorridorKind::InternalAtSameInstitution);
    if !removed {
        return Err(Error::invalid(
            "the fiat row was expected to offer an internal transfer for this table to differ by",
        ));
    }
    CustodyPolicy::from_constraints(rows)
}

/// The failure this prevents: the custody point's attestation says a policy
/// agreed, and a record replayed later proves only that *some* policy did.
///
/// The wrong reference here is the fingerprint of a table that is perfectly
/// valid, conforms to §37.4 and permits this exact corridor through this exact
/// corridor kind. Nothing about it is malformed. What is wrong is that it is
/// not the table this assessment is being made under, and that is precisely
/// the claim an attestation is for — a check that only rejected an invalid
/// table would pass a test written against junk and fire on nothing real.
#[test]
fn an_attestation_made_against_a_different_custody_table_is_vetoed_on_corridor_authority()
-> Result<()> {
    let blueprint = CustodyPolicy::blueprint();
    let other = a_different_conforming_table()?;
    assert_ne!(
        blueprint, other,
        "premise: the two tables must actually differ"
    );
    assert_ne!(
        blueprint.fingerprint(),
        other.fingerprint(),
        "premise: and their fingerprints must differ, or the substitution below is not one"
    );
    // Premise: the other table is not a broken one. It conforms and it permits
    // this corridor, so nothing but its identity can be what refuses.
    assert!(other.conforms().is_ok());
    assert!(
        other
            .permits(
                CustodyClass::FiatAtInstitutionOfRecord,
                CorridorKind::InstitutionApprovalFlow
            )
            .is_ok()
    );

    let identity = identity_of(dec!("500"), now())?;
    // Premise: each table, attested against its own fingerprint, is admitted.
    for (name, table) in [("blueprint", &blueprint), ("other", &other)] {
        assert!(
            assess(
                dec!("500"),
                now(),
                table,
                identity.as_str(),
                &table.fingerprint().to_string()
            )?
            .is_ok(),
            "premise: the {name} table attested against its own fingerprint is admitted"
        );
    }

    // Now assess under the blueprint with the other table's fingerprint
    // attested — the custody point agreed to a policy that is not in force.
    let vetoed = assess(
        dec!("500"),
        now(),
        &blueprint,
        identity.as_str(),
        &other.fingerprint().to_string(),
    )?
    .expect_err("an attestation made against another table must be vetoed");
    assert_eq!(vetoed.check, GateCheck::CorridorAuthority, "{vetoed}");
    assert!(vetoed.alert);
    assert!(
        vetoed.reason.contains(&format!(
            "({})",
            RefusalReason::CustodyAttestationNamesAnotherTable.as_str()
        )),
        "the veto must name the reason as a delimited token: {}",
        vetoed.reason
    );
    assert!(
        vetoed
            .reason
            .contains(&other.fingerprint().as_str().to_string())
            && vetoed
                .reason
                .contains(&blueprint.fingerprint().as_str().to_string()),
        "the veto must name the fingerprint attested and the one in force: {}",
        vetoed.reason
    );
    Ok(())
}

/// Every field of every row changes the fingerprint, and the same table always
/// produces the same one.
///
/// The failure this prevents is a fingerprint that is not one: a digest
/// skipping a field certifies a table that differs in it, so an attestation
/// made against a policy whose self-custody row required multi-party release
/// would satisfy an assessment made under one that did not. Each field is
/// varied on its own, so a skipped field is named.
#[test]
fn the_policy_fingerprint_moves_with_every_field_of_the_table_and_is_stable() -> Result<()> {
    let blueprint = CustodyPolicy::blueprint();
    let base = blueprint.fingerprint();

    // Stability first, and across a re-derivation from rows inserted in the
    // reverse of the table's order: iteration is over a `BTreeMap`, so the
    // material must not depend on how the rows arrived. A fingerprint that did
    // would refuse the very table it was made against.
    let mut reversed: BTreeMap<CustodyClass, ClassConstraints> = BTreeMap::new();
    for class in CustodyClass::ALL.into_iter().rev() {
        if let Some(row) = blueprint.constraints(class) {
            reversed.insert(class, row.clone());
        }
    }
    assert_eq!(reversed.len(), 5, "premise: every row was copied");
    assert_eq!(
        CustodyPolicy::from_constraints(reversed)?.fingerprint(),
        base,
        "the fingerprint depends on the order rows were inserted in, so it is not a \
         fingerprint of the table"
    );

    // Then one variant per field of one row, plus a dropped row.
    let row_of = |class: CustodyClass| -> Result<ClassConstraints> {
        blueprint
            .constraints(class)
            .cloned()
            .ok_or_else(|| Error::invalid("the blueprint has this row"))
    };
    let rows = || -> BTreeMap<CustodyClass, ClassConstraints> {
        CustodyClass::ALL
            .into_iter()
            .filter_map(|class| blueprint.constraints(class).map(|row| (class, row.clone())))
            .collect()
    };

    let mut variants: Vec<(&str, CustodyPolicy)> = Vec::new();

    let mut custodian = rows();
    let mut row = row_of(CustodyClass::PrivateCommitment)?;
    row.custodian = Custodian::InstitutionOfRecord;
    custodian.insert(CustodyClass::PrivateCommitment, row);
    variants.push(("custodian", CustodyPolicy::from_constraints(custodian)?));

    let mut corridors = rows();
    let mut row = row_of(CustodyClass::PrivateCommitment)?;
    row.permitted_corridors = BTreeSet::from([CorridorKind::InstitutionApprovalFlow]);
    corridors.insert(CustodyClass::PrivateCommitment, row);
    variants.push((
        "permitted_corridors",
        CustodyPolicy::from_constraints(corridors)?,
    ));

    let mut mirrored = rows();
    let mut row = row_of(CustodyClass::PrivateCommitment)?;
    row.venue_allowlist_mirrored = true;
    mirrored.insert(CustodyClass::PrivateCommitment, row);
    variants.push((
        "venue_allowlist_mirrored",
        CustodyPolicy::from_constraints(mirrored)?,
    ));

    let mut multi_party = rows();
    let mut row = row_of(CustodyClass::PrivateCommitment)?;
    row.requires_multi_party_release = true;
    multi_party.insert(CustodyClass::PrivateCommitment, row);
    variants.push((
        "requires_multi_party_release",
        CustodyPolicy::from_constraints(multi_party)?,
    ));

    // `may_be_transfer_source` is varied on a row with no corridors, because
    // `conforms` refuses a row that lists corridors and denies being a source.
    let mut source = rows();
    let mut row = row_of(CustodyClass::CollateralAndMargin)?;
    row.may_be_transfer_source = true;
    source.insert(CustodyClass::CollateralAndMargin, row);
    // Built past `from_constraints` deliberately: §37.4 refuses this table, and
    // the fingerprint is a digest of whatever table is in hand rather than a
    // second conformance check. `conforms` is the control that refuses it, and
    // the gate runs that first.
    let source: CustodyPolicy = serde_json::from_value(
        serde_json::to_value(&source).map(|classes| serde_json::json!({ "classes": classes }))?,
    )?;
    variants.push(("may_be_transfer_source", source));

    let mut dropped = rows();
    dropped.remove(&CustodyClass::PrivateCommitment);
    assert_eq!(dropped.len(), 4, "premise: exactly one row was dropped");
    variants.push(("a dropped row", CustodyPolicy::from_constraints(dropped)?));

    for (field, variant) in &variants {
        assert_ne!(
            variant.fingerprint(),
            base,
            "a table differing in {field} shares the blueprint's fingerprint, so an \
             attestation made against one certifies the other"
        );
    }
    let distinct: BTreeSet<String> = variants
        .iter()
        .map(|(_, policy)| policy.fingerprint().as_str().to_string())
        .chain(std::iter::once(base.as_str().to_string()))
        .collect();
    assert_eq!(
        distinct.len(),
        variants.len() + 1,
        "two of the tables share one fingerprint"
    );
    Ok(())
}

/// A gate attestation cannot be satisfied by a note that merely mentions the
/// identity, and a custody one cannot be satisfied by a note mentioning the
/// fingerprint.
///
/// The failure this prevents is the one `01-testing-strategy.md` records as
/// having already survived a mutation in this repository: a containment check
/// that reads as an equality check. A reference of "approved, see
/// <identity>, filed by ops" is a note about an assessment, not an agreement
/// to it, and an attestor can write one without meaning to attest.
#[test]
fn a_reference_that_merely_contains_the_expected_value_is_not_an_attestation_to_it() -> Result<()> {
    let policy = CustodyPolicy::blueprint();
    let identity = identity_of(dec!("500"), now())?;
    let fingerprint = policy.fingerprint().to_string();
    // Premise: the exact values are admitted.
    assert!(assess(dec!("500"), now(), &policy, identity.as_str(), &fingerprint)?.is_ok());

    for (name, gate_reference, custody_reference, expected) in [
        (
            "the gate's",
            format!("approved, see {identity}, filed by ops"),
            fingerprint.clone(),
            RefusalReason::GateAttestationNamesAnotherAssessment,
        ),
        (
            "the custody policy's",
            identity.as_str().to_string(),
            format!("policy {fingerprint} as reviewed on Tuesday"),
            RefusalReason::CustodyAttestationNamesAnotherTable,
        ),
    ] {
        // Premise: the note really does contain the expected value, so a
        // containment check would admit it and only an equality check refuses.
        assert!(
            gate_reference.contains(identity.as_str())
                && custody_reference.contains(fingerprint.as_str())
        );
        let vetoed = assess(
            dec!("500"),
            now(),
            &policy,
            &gate_reference,
            &custody_reference,
        )?
        .expect_err("a note wrapping the expected value is not an attestation to it");
        assert_eq!(
            vetoed.check,
            GateCheck::CorridorAuthority,
            "a note wrapping {name} expected value was admitted or refused elsewhere: {vetoed}"
        );
        assert!(
            vetoed.reason.contains(&format!("({})", expected.as_str())),
            "a note wrapping {name} expected value must be refused by that point's own reason: \
             {}",
            vetoed.reason
        );
    }
    Ok(())
}
