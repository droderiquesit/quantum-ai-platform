//! §37.4's venue-allowlist mirror: the flag that read as a control and was not.
//!
//! `ClassConstraints::venue_allowlist_mirrored` is documented as a
//! precondition — the venue's own allowlist, configured out of band, must be
//! mirrored before a corridor of the class is permissible — and until the
//! check these tests cover, no code read it. It was set by
//! `CustodyPolicy::blueprint`, written to the hash-chained event log inside
//! every `journal::GateCommand` (whose `custody` field is a whole
//! `CustodyPolicy`), replayed out of it, and consulted by nothing on either
//! pass. That is the shape `.claude/rules/domains/
//! risk-and-execution.md` names by its other instance: `MaxExpectedShortfall`
//! shipped in every default limit set against a figure that was always empty,
//! so the limit could never fire and read as protection regardless.
//!
//! Two halves are asserted here, because either alone is a control that can be
//! turned off:
//!
//! 1. The gate refuses a venue-custody corridor whose venue-allowlist
//!    attestation names an address other than the corridor's destination.
//!    `EnforcementPoints::all_agree` proves the point *spoke*; it never reads
//!    what it spoke about.
//! 2. `CustodyPolicy::conforms` refuses a table that offers a venue-side
//!    withdrawal with the mirror flag clear — otherwise a policy deserialised
//!    off the event log disables check 1 by flipping a boolean, and the replay
//!    re-runs the control and confirms the record.

// The workspace denies `panic_in_result_fn` for production code. In a test the
// assertion is the deliverable and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital_fabric::assessment::AssessmentId;
use qip_capital_fabric::corridor::{Corridor, CorridorCaps, CorridorId, PermittedHours};
use qip_capital_fabric::custody::{
    Attestation, ClassConstraints, CorridorKind, CustodyClass, CustodyPolicy, EnforcementPoint,
    EnforcementPoints, Identity, PolicyRule, RefusalReason, TransferAuthority,
};
use qip_capital_fabric::destination::{
    ACTIVATION_DELAY, Approver, Asset, DestinationKey, DestinationRegistry, SignatureRecord,
};
use qip_capital_fabric::gate::{
    CorridorFunding, FundingStanding, GateCheck, KillSwitchState, SourceBalances, StatedPurpose,
    TransferGate, TransferHistory, TransferIntent, VelocityState, Vetoed,
};
use qip_capital_fabric::location::{CapitalLocation, Region};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Duration, Timestamp, dec};
use std::collections::BTreeMap;

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

/// The venue-custody wallet the corridor withdraws from.
fn venue_wallet() -> CapitalLocation {
    CapitalLocation::new(Region::new("emea"), Currency::USD, VenueId::new("VENUE-A"))
}

/// The address the venue allowlisted out of band.
fn allowlisted() -> Result<DestinationKey> {
    DestinationKey::new(Asset::new("USDC")?, "addr-1")
}

/// A different address. Deliberately one whose rendering has the allowlisted
/// one as a *prefix*: `USDC@addr-1` is a prefix of `USDC@addr-10`, so a
/// containment check would admit this and an equality check refuses it. That
/// substring is the class of defect `01-testing-strategy.md` records as having
/// already survived a mutation once in this repository.
fn other_address() -> Result<DestinationKey> {
    DestinationKey::new(Asset::new("USDC")?, "addr-10")
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

/// A registry in which the corridor's destination is proposed, verified,
/// signed and past its delay.
fn usable_registry(key: &DestinationKey) -> Result<DestinationRegistry> {
    let mut registry = DestinationRegistry::new();
    registry.propose(key.clone(), Approver::new("alice")?, proposed_at())?;
    registry.verify(
        key,
        Approver::new("bob")?,
        proposed_at().saturating_add(Duration::from_hours(1)),
    )?;
    registry.record_signature(
        key,
        SignatureRecord::new(Approver::new("carol")?, signed_at(), "vault/dest/crypto-1")?,
    )?;
    Ok(registry)
}

/// An active corridor carrying crypto in venue custody out through the venue's
/// allowlisted-withdrawal route — the one class whose §37.4 row demands the
/// mirror.
fn active_venue_corridor() -> Result<Corridor> {
    let mut corridor = Corridor::propose(
        CorridorId::new("venue-a-to-addr-1")?,
        venue_wallet(),
        CustodyClass::CryptoInVenueCustody,
        CorridorKind::VenueAllowlistedWithdrawal,
        allowlisted()?,
        caps()?,
        "sweep venue balance to the allowlisted treasury address",
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
        "vault/corridor/crypto-1",
    )?)?;
    corridor.begin_delay(signed_at())?;
    corridor.activate(signed_at().saturating_add(ACTIVATION_DELAY))?;
    Ok(corridor)
}

/// The three enforcement points, under three distinct identities, none of them
/// the one that trades — with the venue-allowlist point attesting against
/// `venue_reference`.
///
/// The other two references are the bound ones ADR 0051 added: the transfer
/// gate's is the `AssessmentId` of the movement `assess_with` proposes, and
/// the custody policy's is the fingerprint of the blueprint table. They are
/// derived here so that the only reference a test in this file varies is the
/// venue's — otherwise a mirror test would be refused on a different point's
/// binding and would assert the wrong refusal.
fn authority_referencing(venue_reference: &str) -> Result<TransferAuthority> {
    let mut points = EnforcementPoints::new();
    for (point, identity, reference) in [
        (
            EnforcementPoint::TransferGate,
            "gate-svc",
            AssessmentId::of(
                active_venue_corridor()?.id(),
                &venue_wallet(),
                &allowlisted()?,
                dec!("500"),
                now(),
            )
            .to_string(),
        ),
        (
            EnforcementPoint::CustodyPolicy,
            "custody-policy-svc",
            CustodyPolicy::blueprint().fingerprint().to_string(),
        ),
        (
            EnforcementPoint::VenueAllowlist,
            "venue-ops-oob",
            venue_reference.to_string(),
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

/// Every gate input satisfied except the venue-allowlist reference, which the
/// caller chooses.
fn assess_with(venue_reference: &str) -> Result<std::result::Result<(), Vetoed>> {
    let corridor = active_venue_corridor()?;
    let intent = TransferIntent::new(
        venue_wallet(),
        allowlisted()?,
        dec!("500"),
        StatedPurpose::new(dec!("1000"), dec!("500"))?,
    )?;
    Ok(TransferGate::assess(
        &intent,
        &corridor,
        &usable_registry(&allowlisted()?)?,
        &CustodyPolicy::blueprint(),
        &authority_referencing(venue_reference)?,
        &CorridorFunding::new(
            FundingStanding::Permitted,
            dec!("1000"),
            "every strategy behind this corridor is at full size",
        )?,
        &TransferHistory::empty(),
        &SourceBalances::new(dec!("10000"), dec!("1000"), dec!("1000"), dec!("1000"))?,
        VelocityState::CLEAR,
        KillSwitchState::Armed,
        now(),
    )
    .map(|_| ()))
}

// --- the gate half ----------------------------------------------------------

/// The failure this prevents: the venue's allowlist counts as one of §37.4's
/// three enforcement points while enforcing nothing about *where the money
/// goes*.
///
/// `all_agree` checks that three points spoke under three identities and never
/// reads an attestation's `reference`, which is validated only for being
/// non-empty. So an attestation filed against one allowlist entry admitted a
/// corridor running to a different address, and the approval record then named
/// three identities as having agreed to a movement one of them had not seen.
///
/// The premise is asserted first, and it is the half that distinguishes a
/// working gate from one that refuses everything: the same corridor with the
/// attestation naming the corridor's own destination is *admitted*.
#[test]
fn a_venue_custody_corridor_whose_allowlist_attestation_names_another_address_is_vetoed()
-> Result<()> {
    let matching = allowlisted()?.to_string();
    assert!(
        assess_with(&matching)?.is_ok(),
        "premise: an attestation naming the corridor's own destination must be admitted, \
         or the refusal below would prove only that the gate refuses everything"
    );

    let elsewhere = other_address()?.to_string();
    assert_ne!(
        elsewhere, matching,
        "premise: the two references must actually differ"
    );
    assert!(
        elsewhere.starts_with(&matching),
        "premise: the wrong address must have the right one as a prefix, so this test \
         would catch a containment check as well as a missing one"
    );

    let vetoed =
        assess_with(&elsewhere)?.expect_err("an attestation naming another address must be vetoed");
    assert_eq!(
        vetoed.check,
        GateCheck::CorridorAuthority,
        "the mirror is part of check 1's question of whether this corridor may carry this \
         at all; got: {vetoed}"
    );
    assert!(
        vetoed.alert,
        "§37.3 pairs a corridor-authority veto with an alert: something reached the gate \
         that should not have been generated"
    );
    assert!(
        vetoed
            .reason
            .contains("venue's own allowlist to be mirrored"),
        "the veto must say which rule fired; got: {}",
        vetoed.reason
    );
    assert!(
        vetoed.reason.contains(&elsewhere) && vetoed.reason.contains(&matching),
        "the veto must name both the reference attested and the destination expected, or an \
         operator cannot tell which of the two is wrong; got: {}",
        vetoed.reason
    );
    Ok(())
}

/// A class whose row does not demand the mirror is not held to it.
///
/// Fiat at an institution of record has no venue allowlist to mirror, and a
/// check that fired for every class would be a check nobody could satisfy for
/// four of the five rows — which is how a control gets removed rather than
/// fixed. The flag decides, and this asserts the flag is what decides.
#[test]
fn a_class_whose_row_does_not_demand_the_mirror_is_not_asked_for_one() -> Result<()> {
    let policy = CustodyPolicy::blueprint();
    let fiat = policy
        .constraints(CustodyClass::FiatAtInstitutionOfRecord)
        .ok_or_else(|| Error::invalid("the blueprint policy has a fiat row"))?;
    // Premise: this row really does have the flag clear, and the venue-custody
    // row really does have it set, so the two cases below differ by the flag.
    assert!(!fiat.venue_allowlist_mirrored);
    let crypto = policy
        .constraints(CustodyClass::CryptoInVenueCustody)
        .ok_or_else(|| Error::invalid("the blueprint policy has a venue-custody row"))?;
    assert!(crypto.venue_allowlist_mirrored);

    let agreement = authority_referencing("some-unrelated-allowlist-note")?
        .agreement()
        .map_err(|refusal| Error::invalid(refusal.to_string()))?;
    assert!(
        policy
            .mirrors_the_venue_allowlist(
                CustodyClass::FiatAtInstitutionOfRecord,
                &allowlisted()?,
                &agreement,
            )
            .is_ok(),
        "fiat at an institution of record has no venue allowlist, so an unrelated reference \
         is not a mirror failure"
    );
    let refusal = policy
        .mirrors_the_venue_allowlist(
            CustodyClass::CryptoInVenueCustody,
            &allowlisted()?,
            &agreement,
        )
        .expect_err("venue custody demands the mirror and this reference is not one");
    assert_eq!(
        refusal.reason,
        RefusalReason::VenueAllowlistNotMirrored {
            class: CustodyClass::CryptoInVenueCustody
        }
    );
    Ok(())
}

/// The failure this prevents: the mirror compares `destination.to_string()`
/// against the attestation's reference, and that rendering names two different
/// destinations.
///
/// `DestinationKey` renders `asset@address`. Nothing forbade an `@` inside
/// either half, so asset `USDC` with address `a@b` and asset `USDC@a` with
/// address `b` both rendered `USDC@a@b`: one venue-allowlist attestation
/// mirrored two destinations, and a corridor running to the second was
/// satisfied by an entry the venue allowlisted for the first. Whole-value
/// equality does not catch it — the strings really are equal — so this is not
/// the prefix trap the test above covers and would have survived that fix.
///
/// Two things close it and both are asserted here, because either alone leaves
/// a path open. `Asset::new` refuses `@`, so the pair cannot be *constructed*;
/// and the mirror compares a parsed `DestinationKey`, so the pair cannot be
/// *deserialised* into a match either — which matters because a
/// `DestinationKey` reaches this control off the hash-chained event log inside
/// a `GateCommand`, where serde builds an `Asset` from its field and calls no
/// constructor.
#[test]
fn an_attestation_is_mirrored_against_the_parsed_key_so_one_rendering_cannot_name_two_destinations()
-> Result<()> {
    // Premise 1: the constructor closes the constructible half.
    let refused =
        Asset::new("USDC@a").expect_err("an asset name holding the key separator must be refused");
    assert!(
        refused.message().contains("may not contain '@'"),
        "the refusal must name the character and why; got: {}",
        refused.message()
    );

    // Premise 2: serde builds the pair the constructor refuses, exactly as the
    // event log would hand it to the gate.
    let ambiguous: DestinationKey = serde_json::from_str(r#"{"asset":"USDC@a","address":"b"}"#)?;
    let legitimate = DestinationKey::new(Asset::new("USDC")?, "a@b")?;
    assert_ne!(
        ambiguous, legitimate,
        "premise: these are two different destinations — different asset, different address"
    );
    assert_eq!(
        ambiguous.to_string(),
        legitimate.to_string(),
        "premise: and they share one rendering, which is the whole defect; if this ever fails \
         the fixture no longer reproduces it and the assertions below prove nothing"
    );

    // The attestation is filed against the destination the venue actually
    // allowlisted, written the only way a key is written.
    let agreement = authority_referencing(&legitimate.to_string())?
        .agreement()
        .map_err(|refusal| Error::invalid(refusal.to_string()))?;
    let policy = CustodyPolicy::blueprint();
    assert!(
        policy
            .mirrors_the_venue_allowlist(
                CustodyClass::CryptoInVenueCustody,
                &legitimate,
                &agreement,
            )
            .is_ok(),
        "premise: the destination the attestation names must mirror, or the refusal below \
         would prove only that the check refuses everything"
    );

    let refusal = policy
        .mirrors_the_venue_allowlist(CustodyClass::CryptoInVenueCustody, &ambiguous, &agreement)
        .expect_err(
            "an attestation filed against USDC/a@b must not mirror a corridor running to \
             USDC@a/b, however the two render",
        );
    assert_eq!(
        refusal.reason,
        RefusalReason::VenueAllowlistNotMirrored {
            class: CustodyClass::CryptoInVenueCustody
        }
    );
    Ok(())
}

// --- the table half ---------------------------------------------------------

/// The flag cannot be cleared to switch the mirror off.
///
/// A `CustodyPolicy` travels inside a `GateCommand` on the hash-chained event
/// log and reaches the gate deserialised, where no constructor runs. Without
/// this rule in `conforms` — which the gate re-runs on every assessment, live
/// and replayed alike — a table with `venue_allowlist_mirrored: false` on a
/// row that still lists a venue-side withdrawal would make the mirror check
/// return `Ok` for every destination, and the replay would re-execute the
/// control and *confirm* the record. A hash chain proves a record has not
/// changed; only the control re-asking the question proves it was true.
#[test]
fn a_table_offering_a_venue_withdrawal_without_the_mirror_is_refused_by_name() -> Result<()> {
    let blueprint = CustodyPolicy::blueprint();
    let mut rows: BTreeMap<CustodyClass, ClassConstraints> = CustodyClass::ALL
        .into_iter()
        .filter_map(|class| blueprint.constraints(class).map(|row| (class, row.clone())))
        .collect();
    // Premise: the blueprint's own rows rebuild, so the refusal below is
    // caused by the one field this test changes and not by the copy.
    assert_eq!(rows.len(), 5, "every row was copied");
    assert!(CustodyPolicy::from_constraints(rows.clone()).is_ok());

    let venue_row = rows
        .get_mut(&CustodyClass::CryptoInVenueCustody)
        .ok_or_else(|| Error::invalid("the venue-custody row was copied"))?;
    assert!(
        venue_row
            .permitted_corridors
            .contains(&CorridorKind::VenueAllowlistedWithdrawal),
        "premise: the row still offers the corridor whose mirror is being waived"
    );
    venue_row.venue_allowlist_mirrored = false;

    match CustodyPolicy::from_constraints(rows) {
        Err(Error::Denied(message)) => {
            assert!(
                message.contains("removes a point by flipping a flag"),
                "the refusal must name what the flag would remove; got: {message}"
            );
        }
        other => panic!("a table waiving the venue-allowlist mirror must be denied, got {other:?}"),
    }
    Ok(())
}

/// The same rule stated through `conforms`, which is what the gate calls, and
/// with the rule token an operator would see in a log.
///
/// Asserted separately from `from_constraints` because they are two different
/// paths to the same rule and only one of them is on the replay: a rule that
/// held in the constructor and not in `conforms` would pass the test above and
/// still admit the record the event log carries.
#[test]
fn conforms_names_the_venue_withdrawal_mirror_rule_and_the_blueprint_table_satisfies_it()
-> Result<()> {
    assert!(
        CustodyPolicy::blueprint().conforms().is_ok(),
        "premise: the §37.4 table as written must conform, or this rule refuses the platform's \
         own policy"
    );

    // Written as JSON rather than built through `from_constraints`, because
    // that is exactly the shape a `GateCommand` carries on the event log: the
    // constructor never runs on this path, and the point of the test is that
    // the rule holds anyway.
    let policy: CustodyPolicy = serde_json::from_str(
        r#"{
            "classes": {
                "crypto_in_venue_custody": {
                    "custodian": "venue",
                    "permitted_corridors": ["venue_allowlisted_withdrawal"],
                    "may_be_transfer_source": true,
                    "venue_allowlist_mirrored": false,
                    "requires_multi_party_release": false
                }
            }
        }"#,
    )?;
    // Premise: the table really did arrive through serde with the waiver on
    // it, so the refusal below is `conforms` doing the work.
    let row = policy
        .constraints(CustodyClass::CryptoInVenueCustody)
        .ok_or_else(|| Error::invalid("the deserialised policy carries the row"))?;
    assert!(!row.venue_allowlist_mirrored);
    assert!(
        row.permitted_corridors
            .contains(&CorridorKind::VenueAllowlistedWithdrawal)
    );

    let refusal = policy
        .conforms()
        .expect_err("a deserialised table waiving the mirror must still be refused");
    assert_eq!(
        refusal.reason,
        RefusalReason::PolicyContradictsBlueprint {
            class: CustodyClass::CryptoInVenueCustody,
            rule: PolicyRule::VenueWithdrawalMirrorsTheAllowlist,
        },
        "the refusal must name the rule as a token, so a log line and a metric can say the \
         same thing the refusal does"
    );
    assert_eq!(
        PolicyRule::VenueWithdrawalMirrorsTheAllowlist.as_str(),
        "venue_withdrawal_mirrors_the_allowlist"
    );
    Ok(())
}
