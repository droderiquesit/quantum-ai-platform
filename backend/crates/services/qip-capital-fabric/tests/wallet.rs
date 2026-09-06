//! Tests for the wallet read model and the custody policy (blueprint §37.4,
//! §38.1–38.3, ADR 0021).
//!
//! The properties here are the ones whose failure is silent: an expected
//! balance with a sign error in the reservation, a halt that stops the wrong
//! venue-asset, a stale balance reconciled as though it were current, a
//! wallet that grows a way to fix the ledger, a custody table that can be
//! configured to let collateral leave, and three approvals that are one
//! identity three times. Each test states its premise before its claim, so a
//! test that passed on an empty list would have failed on the premise first.

// The workspace denies `panic_in_result_fn` for production code. In a test the
// assertion is the deliverable, and `?` keeps the fixtures readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital_fabric::custody::{
    Agreement, Attestation, ClassConstraints, CorridorKind, Custodian, CustodyClass, CustodyPolicy,
    EnforcementPoint, EnforcementPoints, Identity, RefusalReason,
};
use qip_capital_fabric::wallet::{
    Asset, BreakCause, HoldingObservation, LedgerView, Provenance, ReconciliationOutcome,
    TolerancePolicy, VenueAsset, Wallet,
};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp, dec};
use std::collections::{BTreeMap, BTreeSet};

// --- fixtures ---------------------------------------------------------------

/// Thursday 7 March 2024, 09:00 UTC.
fn now() -> Timestamp {
    Timestamp::from_civil(2024, 3, 7).saturating_add(Duration::from_hours(9))
}

fn freshness() -> Duration {
    Duration::from_mins(5)
}

fn venue(name: &str) -> VenueId {
    VenueId::new(name)
}

fn asset(name: &str) -> Result<Asset> {
    Asset::new(name)
}

fn observed(venue_name: &str, asset_name: &str, balance: Decimal) -> Result<HoldingObservation> {
    Ok(HoldingObservation::new(
        venue(venue_name),
        asset(asset_name)?,
        balance,
        now().saturating_sub(Duration::from_secs(30)),
        Provenance::ReadOnlyApiKey,
    ))
}

fn booked(
    venue_name: &str,
    asset_name: &str,
    ledger_balance: Decimal,
    reserved: Decimal,
    in_flight: Decimal,
) -> Result<LedgerView> {
    LedgerView::new(
        venue(venue_name),
        asset(asset_name)?,
        ledger_balance,
        reserved,
        in_flight,
    )
}

fn key(venue_name: &str, asset_name: &str) -> Result<VenueAsset> {
    Ok(VenueAsset {
        venue: venue(venue_name),
        asset: asset(asset_name)?,
    })
}

fn attestation(point: EnforcementPoint, identity: &str) -> Result<Attestation> {
    Attestation::new(
        point,
        Identity::new(identity)?,
        format!("{}-record-1", point.as_str()),
        now(),
    )
}

fn three_distinct() -> Result<EnforcementPoints> {
    let mut points = EnforcementPoints::new();
    points.attest(attestation(EnforcementPoint::TransferGate, "gate-svc")?)?;
    points.attest(attestation(
        EnforcementPoint::CustodyPolicy,
        "custody-policy-svc",
    )?)?;
    points.attest(attestation(
        EnforcementPoint::VenueAllowlist,
        "venue-ops-oob",
    )?)?;
    Ok(points)
}

// --- §38.3 arithmetic --------------------------------------------------------

#[test]
fn the_expected_balance_is_ledger_minus_reserved_plus_in_flight() -> Result<()> {
    // The failure this prevents: a sign slip on either term. Adding the
    // reservation instead of subtracting it hides a shortfall of exactly the
    // reserved size; subtracting in-flight hides an arrival. The three figures
    // are distinct and non-zero so that either slip changes the answer.
    let view = booked("XCBT", "USD", dec!("1000"), dec!("150"), dec!("75"))?;
    assert!(!view.reserved.is_zero() && !view.in_flight.is_zero());
    assert_ne!(view.reserved, view.in_flight);

    assert_eq!(view.expected()?, dec!("925"));
    Ok(())
}

#[test]
fn a_negative_reservation_or_in_flight_is_refused_at_the_ledger_view() -> Result<()> {
    // A negative reservation would *raise* the expected balance and let the
    // venue's shortfall of that size reconcile as clean.
    let negative_reserved = booked("XCBT", "USD", dec!("1000"), dec!("-1"), dec!("0"));
    assert!(matches!(negative_reserved, Err(Error::Invalid(_))));
    let negative_in_flight = booked("XCBT", "USD", dec!("1000"), dec!("0"), dec!("-1"));
    assert!(matches!(negative_in_flight, Err(Error::Invalid(_))));
    // And the gate admits the good one.
    assert!(booked("XCBT", "USD", dec!("1000"), dec!("0"), dec!("0")).is_ok());
    Ok(())
}

// --- halting ---------------------------------------------------------------

#[test]
fn a_delta_at_tolerance_halts_exactly_that_venue_asset_and_no_other() -> Result<()> {
    // Three venue-assets, two of them clean and one whose delta sits exactly
    // on the tolerance. §38.3 halts at `|delta| >= tolerance`, so the boundary
    // halts; and it halts only its own venue-asset. The failure this prevents
    // is a halt that stops the venue, or the wallet, rather than the one
    // balance that broke — or a `>` that lets a delta exactly at tolerance
    // through.
    let tolerances = TolerancePolicy::new()
        .with_tolerance(asset("USD")?, dec!("10"))?
        .with_tolerance(asset("BTC")?, dec!("0.0001"))?;
    let wallet = Wallet::assemble(
        vec![
            observed("XCBT", "USD", dec!("930"))?, // expected 925, delta +5: clean
            observed("XNAS", "USD", dec!("490"))?, // expected 500, delta -10: at tolerance
            observed("COIN", "BTC", dec!("2.5"))?, // expected 2.5, delta 0: clean
        ],
        vec![
            booked("XCBT", "USD", dec!("1000"), dec!("150"), dec!("75"))?,
            booked("XNAS", "USD", dec!("500"), dec!("0"), dec!("0"))?,
            booked("COIN", "BTC", dec!("3"), dec!("0.5"), dec!("0"))?,
        ],
        freshness(),
        now(),
    )?;

    let outcomes = wallet.reconcile(&tolerances)?;
    // Premise: every venue-asset produced an outcome.
    assert_eq!(outcomes.len(), 3);

    let halts: Vec<&ReconciliationOutcome> = outcomes.iter().filter(|o| o.is_halt()).collect();
    assert_eq!(
        halts.len(),
        1,
        "exactly one venue-asset halts: {outcomes:?}"
    );
    match halts[0] {
        ReconciliationOutcome::Halt {
            venue,
            asset,
            delta,
            alert,
        } => {
            assert_eq!(*venue, VenueId::new("XNAS"));
            assert_eq!(asset.as_str(), "USD");
            assert_eq!(*delta, dec!("-10"));
            assert_eq!(alert.cause, BreakCause::DeltaBeyondTolerance);
            assert_eq!(alert.expected, dec!("500"));
            assert_eq!(alert.observed, dec!("490"));
            assert_eq!(alert.tolerance, dec!("10"));
            assert!(alert.message.contains("writes no correction"));
        }
        other => panic!("expected a halt, got {other:?}"),
    }

    // The other two reconciled, and kept their deltas for the persistent-delta
    // ticket §38.3 asks for.
    let clean: BTreeMap<VenueAsset, Decimal> = outcomes
        .iter()
        .filter_map(|o| match o {
            ReconciliationOutcome::Reconciled { delta, .. } => Some((o.venue_asset(), *delta)),
            ReconciliationOutcome::Halt { .. } => None,
        })
        .collect();
    assert_eq!(clean.get(&key("XCBT", "USD")?), Some(&dec!("5")));
    assert_eq!(clean.get(&key("COIN", "BTC")?), Some(&Decimal::ZERO));
    Ok(())
}

#[test]
fn a_surplus_beyond_tolerance_halts_as_a_shortfall_does() -> Result<()> {
    // §38.3: an external balance exceeding expectation is treated with the
    // same severity. The failure this prevents is a `delta >= tolerance`
    // written without the absolute value, which halts on missing money and
    // waves through money the ledger cannot explain.
    let tolerances = TolerancePolicy::new().with_tolerance(asset("USD")?, dec!("10"))?;
    let wallet = Wallet::assemble(
        vec![observed("XCBT", "USD", dec!("1025"))?],
        vec![booked("XCBT", "USD", dec!("1000"), dec!("0"), dec!("0"))?],
        freshness(),
        now(),
    )?;
    let outcomes = wallet.reconcile(&tolerances)?;
    assert_eq!(outcomes.len(), 1);
    assert!(
        outcomes[0].is_halt(),
        "a surplus of 25 against tolerance 10 must halt"
    );
    Ok(())
}

#[test]
fn a_holding_the_ledger_never_booked_halts_as_unrecorded() -> Result<()> {
    // A venue reporting a balance the ledger has no row for is a break, not a
    // discovery. The failure this prevents is the wallet silently adopting the
    // venue's figure as the ledger's — which is a correction by another name.
    let tolerances = TolerancePolicy::new().with_tolerance(asset("ETH")?, dec!("0.01"))?;
    let wallet = Wallet::assemble(
        vec![observed("COIN", "ETH", dec!("4"))?],
        vec![],
        freshness(),
        now(),
    )?;
    let outcomes = wallet.reconcile(&tolerances)?;
    assert_eq!(outcomes.len(), 1);
    match &outcomes[0] {
        ReconciliationOutcome::Halt { delta, alert, .. } => {
            assert_eq!(alert.cause, BreakCause::UnrecordedByLedger);
            assert_eq!(alert.expected, Decimal::ZERO);
            assert_eq!(*delta, dec!("4"));
        }
        other => panic!("expected a halt, got {other:?}"),
    }
    Ok(())
}

// --- evidence --------------------------------------------------------------

#[test]
fn a_stale_observation_is_refused_as_evidence_rather_than_reconciled() -> Result<()> {
    // The failure this prevents: yesterday's balance reconciled against
    // today's ledger, producing a break the wallet manufactured — or, worse,
    // masking a real one. A stale observation is refused at assembly, so no
    // outcome is ever computed from it. The observation exactly at the bound
    // is admitted, which is what distinguishes a working gate from one that
    // refuses everything.
    let bound = now().saturating_sub(freshness());
    let one_second_past = HoldingObservation::new(
        venue("XCBT"),
        asset("USD")?,
        dec!("1000"),
        bound.saturating_sub(Duration::from_secs(1)),
        Provenance::Statement,
    );
    let ledger = vec![booked("XCBT", "USD", dec!("1000"), dec!("0"), dec!("0"))?];

    let refused = Wallet::assemble(vec![one_second_past], ledger.clone(), freshness(), now());
    match refused {
        Err(Error::Invalid(message)) => {
            assert!(
                message.contains("old"),
                "the refusal must name staleness: {message}"
            );
            assert!(
                message.contains("XCBT/USD"),
                "and the venue-asset: {message}"
            );
        }
        other => panic!("a stale observation must be refused, got {other:?}"),
    }

    let at_bound = HoldingObservation::new(
        venue("XCBT"),
        asset("USD")?,
        dec!("1000"),
        bound,
        Provenance::Statement,
    );
    assert!(Wallet::assemble(vec![at_bound], ledger, freshness(), now()).is_ok());
    Ok(())
}

#[test]
fn an_observation_dated_in_the_future_is_refused() -> Result<()> {
    // A balance from after `now` is a clock fault somewhere, and reconciling
    // it would age it negatively past every freshness check.
    let future = HoldingObservation::new(
        venue("XCBT"),
        asset("USD")?,
        dec!("1000"),
        now().saturating_add(Duration::from_secs(1)),
        Provenance::ReadOnlyApiKey,
    );
    let result = Wallet::assemble(vec![future], vec![], freshness(), now());
    assert!(matches!(result, Err(Error::Invalid(ref m)) if m.contains("future")));
    Ok(())
}

#[test]
fn two_observations_of_one_venue_asset_are_refused_rather_than_chosen_between() -> Result<()> {
    // Two claims about one fact. The failure this prevents is the last one
    // written winning silently, which makes the outcome depend on input order.
    let result = Wallet::assemble(
        vec![
            observed("XCBT", "USD", dec!("1000"))?,
            observed("XCBT", "USD", dec!("999"))?,
        ],
        vec![],
        freshness(),
        now(),
    );
    assert!(matches!(result, Err(Error::Invalid(ref m)) if m.contains("two observations")));
    Ok(())
}

#[test]
fn a_ledger_view_with_no_observation_is_refused_as_unreconcilable() -> Result<()> {
    // The ledger's belief is not evidence. A venue-asset nobody read must not
    // appear in the outcomes as anything.
    let result = Wallet::assemble(
        vec![],
        vec![booked("XCBT", "USD", dec!("1000"), dec!("0"), dec!("0"))?],
        freshness(),
        now(),
    );
    assert!(matches!(result, Err(Error::Invalid(ref m)) if m.contains("no observation")));
    Ok(())
}

#[test]
fn a_tolerance_that_is_not_strictly_positive_is_refused() -> Result<()> {
    // Zero halts on every reconciled balance; negative halts on none. Both
    // read as a configured control and are neither.
    assert!(matches!(
        TolerancePolicy::new().with_tolerance(asset("USD")?, Decimal::ZERO),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        TolerancePolicy::new().with_tolerance(asset("USD")?, dec!("-1")),
        Err(Error::Invalid(_))
    ));
    assert!(
        TolerancePolicy::new()
            .with_tolerance(asset("USD")?, dec!("0.01"))
            .is_ok()
    );
    Ok(())
}

#[test]
fn an_asset_without_a_tolerance_is_refused_rather_than_guessed() -> Result<()> {
    // The failure this prevents: a default tolerance wide enough that a
    // forgotten asset always reconciles.
    let wallet = Wallet::assemble(
        vec![observed("COIN", "BTC", dec!("1"))?],
        vec![booked("COIN", "BTC", dec!("1"), dec!("0"), dec!("0"))?],
        freshness(),
        now(),
    )?;
    let only_usd = TolerancePolicy::new().with_tolerance(asset("USD")?, dec!("10"))?;
    assert!(matches!(
        wallet.reconcile(&only_usd),
        Err(Error::Invalid(ref m)) if m.contains("BTC")
    ));
    Ok(())
}

// --- no write path ---------------------------------------------------------

/// The wallet module's own source, read at compile time. The assertions over
/// it are about the API's shape, which no runtime call can observe.
const WALLET_SOURCE: &str = include_str!("../src/wallet.rs");

#[test]
fn the_wallet_exposes_no_mutation_of_ledger_state() -> Result<()> {
    // §38.3: "The Wallet never writes a correction to the ledger." Here that
    // is a property of the API rather than of restraint, and this test pins
    // it three ways. The failure this prevents is the predictable one: a
    // `Wallet::correct(&mut self, ...)` added "for the reconciliation
    // runbook", after which the read path is the write path.

    // Premise: the source is the real module, and reconciliation is a `&self`
    // method on it.
    assert!(WALLET_SOURCE.contains("pub struct Wallet"));
    assert!(WALLET_SOURCE.contains("pub fn reconcile(&self"));

    // 1. Nothing in the module takes `&mut self`, so no method on any type in
    //    it — wallet, view, observation, alert or outcome — can change state
    //    after construction.
    let mutating: Vec<&str> = WALLET_SOURCE
        .lines()
        .filter(|line| line.contains("&mut self"))
        .collect();
    assert!(
        mutating.is_empty(),
        "the wallet module has grown a mutating method: {mutating:?}"
    );

    // 2. An outcome is a value with no borrow into the ledger. If it held a
    //    reference, this bound would not hold and the test would not compile.
    fn owns_everything<T: 'static>() {}
    owns_everything::<ReconciliationOutcome>();

    // 3. Through the API: reconciling a wallet with a break leaves the
    //    ledger's view byte-identical.
    let wallet = Wallet::assemble(
        vec![observed("XCBT", "USD", dec!("900"))?],
        vec![booked("XCBT", "USD", dec!("1000"), dec!("0"), dec!("0"))?],
        freshness(),
        now(),
    )?;
    let before = wallet.ledger_view(&key("XCBT", "USD")?).cloned();
    let tolerances = TolerancePolicy::new().with_tolerance(asset("USD")?, dec!("10"))?;
    let outcomes = wallet.reconcile(&tolerances)?;
    assert!(outcomes[0].is_halt(), "premise: this reconciliation breaks");
    let after = wallet.ledger_view(&key("XCBT", "USD")?).cloned();
    assert_eq!(before, after);
    assert_eq!(after.map(|v| v.ledger_balance), Some(dec!("1000")));
    Ok(())
}

#[test]
fn no_wallet_field_has_a_type_that_could_carry_a_credential() {
    // §38.1: the read path holds no key material. The blueprint enforces it by
    // dependency audit; here it is structural, and this test is the audit. It
    // walks every struct field declared in the module and checks its type
    // against the closed set of value types the read model is built from.
    // The failure this prevents is a `source_key: String` or an
    // `address: String` added to an observation "for traceability", which is
    // the first field of a credential store.
    let permitted_types: BTreeSet<&str> = [
        "VenueId",
        "Asset",
        "Decimal",
        "Timestamp",
        "Provenance",
        "VenueAsset",
        "BreakCause",
        "BTreeMap<Asset, Decimal>",
        "BTreeMap<VenueAsset, HoldingObservation>",
        "BTreeMap<VenueAsset, LedgerView>",
        "ReconciliationAlert",
        // The one String in the module: the alert's sentence for an operator.
        "String",
    ]
    .into_iter()
    .collect();
    let mut fields = 0usize;
    let mut string_fields = Vec::new();
    let mut declarations = 0usize;
    // Only the bodies of `pub struct` and `pub enum` declarations are walked,
    // tracked by brace depth, so a struct-literal line inside a method body
    // (`venue: key.venue.clone(),`) is not mistaken for a field.
    let mut depth = 0usize;
    for line in WALLET_SOURCE.lines() {
        let trimmed = line.trim();
        if depth == 0 {
            if (trimmed.starts_with("pub struct ") || trimmed.starts_with("pub enum "))
                && trimmed.ends_with('{')
            {
                declarations += 1;
                depth = 1;
            }
            continue;
        }
        let opens = trimmed.matches('{').count();
        let closes = trimmed.matches('}').count();
        let inside_before = depth;
        depth = (depth + opens).saturating_sub(closes);
        if inside_before == 0 || trimmed.starts_with("//") {
            continue;
        }
        // A field: `name: Type,` with optional `pub`.
        let Some((name, ty)) = trimmed
            .strip_prefix("pub ")
            .unwrap_or(trimmed)
            .split_once(": ")
        else {
            continue;
        };
        if !trimmed.ends_with(',') || name.contains(' ') || name.contains('(') {
            continue;
        }
        let ty = ty.trim_end_matches(',');
        fields += 1;
        assert!(
            permitted_types.contains(ty),
            "field `{name}: {ty}` in wallet.rs is not one of the read model's value types; \
             if it is a new balance figure add it to the set, if it is anything that could \
             identify or unlock a channel it does not belong here"
        );
        if ty == "String" {
            string_fields.push(name);
        }
    }
    // Premise: the walk found the declarations and fields it was written to
    // check. Nine braced types live in the module; a walk finding fewer is not
    // reading it.
    assert!(
        declarations >= 9,
        "only {declarations} struct/enum declarations were found; the walk is not reading \
         the module"
    );
    assert!(
        fields >= 20,
        "only {fields} fields were found; the walk is not reading the structs"
    );
    assert_eq!(
        string_fields,
        vec!["message"],
        "the only free-text field in the wallet is the alert's sentence"
    );
    // And the newtype's inner string is a name, declared as a tuple field
    // rather than a named one, so it is asserted separately.
    assert!(WALLET_SOURCE.contains("pub struct Asset(String);"));
}

// --- §37.4 custody policy as data -------------------------------------------

#[test]
fn the_blueprint_policy_permits_each_class_exactly_its_own_corridors() {
    // The §37.4 table, row by row, against every corridor kind. The failure
    // this prevents is a lookup that answers on the class alone, or on the
    // corridor alone — either of which lets fiat leave through a venue
    // withdrawal or self-custody through a bank's approval flow.
    let policy = CustodyPolicy::blueprint();
    let every_kind = [
        CorridorKind::InstitutionApprovalFlow,
        CorridorKind::InternalAtSameInstitution,
        CorridorKind::VenueAllowlistedWithdrawal,
        CorridorKind::OnChainAfterGateApproval,
        CorridorKind::CapitalCallFromReserve,
    ];
    let expected: BTreeMap<CustodyClass, BTreeSet<CorridorKind>> = [
        (
            CustodyClass::FiatAtInstitutionOfRecord,
            BTreeSet::from([
                CorridorKind::InstitutionApprovalFlow,
                CorridorKind::InternalAtSameInstitution,
            ]),
        ),
        (
            CustodyClass::CryptoInVenueCustody,
            BTreeSet::from([CorridorKind::VenueAllowlistedWithdrawal]),
        ),
        (
            CustodyClass::CryptoSelfCustody,
            BTreeSet::from([CorridorKind::OnChainAfterGateApproval]),
        ),
        (CustodyClass::CollateralAndMargin, BTreeSet::new()),
        (
            CustodyClass::PrivateCommitment,
            BTreeSet::from([CorridorKind::CapitalCallFromReserve]),
        ),
    ]
    .into_iter()
    .collect();

    let mut permitted = 0usize;
    let mut refused = 0usize;
    for class in CustodyClass::ALL {
        for kind in every_kind {
            let verdict = policy.permits(class, kind);
            if expected[&class].contains(&kind) {
                assert!(verdict.is_ok(), "{class} through {kind} must be permitted");
                permitted += 1;
            } else {
                let refusal = verdict.expect_err("must be refused");
                assert_eq!(refusal.class, Some(class));
                assert_eq!(refusal.corridor, Some(kind));
                refused += 1;
            }
        }
    }
    // Premise: both branches were exercised, many times.
    assert_eq!(permitted, 5);
    assert_eq!(refused, 20);
}

#[test]
fn collateral_and_margin_is_never_a_transfer_source() {
    // §37.4: "Managed as inventory, not transfers. Never leaves." Every
    // corridor kind, and the refusal names the reason rather than merely
    // "not in the list", because the list being empty is the rule and not an
    // omission.
    let policy = CustodyPolicy::blueprint();
    let row = policy
        .constraints(CustodyClass::CollateralAndMargin)
        .expect("the blueprint policy has a collateral row");
    assert!(!row.may_be_transfer_source);
    assert!(row.permitted_corridors.is_empty());
    assert_eq!(row.custodian, Custodian::Venue);
    for kind in [
        CorridorKind::InstitutionApprovalFlow,
        CorridorKind::InternalAtSameInstitution,
        CorridorKind::VenueAllowlistedWithdrawal,
        CorridorKind::OnChainAfterGateApproval,
        CorridorKind::CapitalCallFromReserve,
    ] {
        let refusal = policy
            .permits(CustodyClass::CollateralAndMargin, kind)
            .expect_err("collateral never leaves");
        assert_eq!(refusal.reason, RefusalReason::ClassNeverTransfers);
    }
}

#[test]
fn self_custody_always_requires_multi_party_release_and_a_policy_saying_otherwise_is_refused()
-> Result<()> {
    // §37.4's rule for self-custody is "no single component can sign". It is
    // recorded as a policy fact and the policy refuses to be built without it.
    // The failure this prevents is a configured table that marks self-custody
    // single-party and reaches `permits`, which would then answer yes to a
    // movement the blueprint forbids unconditionally.
    let blueprint = CustodyPolicy::blueprint();
    let row = blueprint
        .constraints(CustodyClass::CryptoSelfCustody)
        .expect("the blueprint policy has a self-custody row");
    assert!(row.requires_multi_party_release);
    assert_eq!(row.custodian, Custodian::SelfCustody);

    let mut rows: BTreeMap<CustodyClass, ClassConstraints> = CustodyClass::ALL
        .into_iter()
        .filter_map(|class| blueprint.constraints(class).map(|row| (class, row.clone())))
        .collect();
    // The blueprint's own rows rebuild: the gate admits the good table.
    assert_eq!(rows.len(), 5, "premise: every row was copied");
    assert!(CustodyPolicy::from_constraints(rows.clone()).is_ok());

    let single_party = rows
        .get_mut(&CustodyClass::CryptoSelfCustody)
        .expect("row present");
    single_party.requires_multi_party_release = false;
    match CustodyPolicy::from_constraints(rows) {
        Err(Error::Denied(message)) => {
            assert!(
                message.contains("no single component can sign"),
                "{message}"
            );
        }
        other => panic!("a single-party self-custody policy must be refused, got {other:?}"),
    }
    Ok(())
}

#[test]
fn a_policy_giving_collateral_a_corridor_is_refused() {
    // The other unconditional rule of §37.4. A table that lists a corridor for
    // collateral while also marking it as never a source is contradictory, and
    // one that marks it a source is wrong; both are refused, neither is fixed.
    let blueprint = CustodyPolicy::blueprint();
    let mut rows: BTreeMap<CustodyClass, ClassConstraints> = CustodyClass::ALL
        .into_iter()
        .filter_map(|class| blueprint.constraints(class).map(|row| (class, row.clone())))
        .collect();
    let collateral = rows
        .get_mut(&CustodyClass::CollateralAndMargin)
        .expect("row present");
    collateral
        .permitted_corridors
        .insert(CorridorKind::InternalAtSameInstitution);
    assert!(matches!(
        CustodyPolicy::from_constraints(rows.clone()),
        Err(Error::Denied(_))
    ));

    let collateral = rows
        .get_mut(&CustodyClass::CollateralAndMargin)
        .expect("row present");
    collateral.permitted_corridors.clear();
    collateral.may_be_transfer_source = true;
    assert!(matches!(
        CustodyPolicy::from_constraints(rows),
        Err(Error::Denied(_))
    ));
}

// --- §37.4 closing rule: three points, three identities ----------------------

#[test]
fn three_distinct_identities_across_all_three_points_agree() -> Result<()> {
    // The affirmative case, so the refusals below are known to be refusals of
    // something and not of everything.
    let agreement: Agreement = three_distinct()?
        .all_agree()
        .expect("three distinct attestations agree");
    assert_eq!(agreement.attestations().len(), 3);
    let points: Vec<EnforcementPoint> = agreement.attestations().iter().map(|a| a.point).collect();
    assert_eq!(points, EnforcementPoint::ALL.to_vec());
    Ok(())
}

#[test]
fn enforcement_points_refuse_when_any_two_share_an_identity() -> Result<()> {
    // The failure this prevents is the one §37.4 names: the gate and the
    // custody policy deployed under one service identity, counted as two
    // approvals while being one decision made twice. Each pair is tried, so a
    // check that only compared neighbours would be caught.
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
        let mut points = EnforcementPoints::new();
        for point in EnforcementPoint::ALL {
            let identity = if point == first || point == second {
                "shared-svc"
            } else {
                "other-svc"
            };
            points.attest(attestation(point, identity)?)?;
        }
        // Premise: all three attested.
        assert!(
            EnforcementPoint::ALL
                .iter()
                .all(|p| points.attestation(*p).is_some())
        );

        let refusal = points
            .all_agree()
            .expect_err("two points under one identity must be refused");
        assert_eq!(
            refusal.reason,
            RefusalReason::SharedIdentity { first, second }
        );
        assert!(refusal.detail.contains("shared-svc"));
    }
    Ok(())
}

#[test]
fn enforcement_points_refuse_two_of_three() -> Result<()> {
    // Two of three agreeing is not agreement. Each point is left out in turn,
    // and the refusal names the one that is missing rather than the two that
    // are present.
    for missing in EnforcementPoint::ALL {
        let mut points = EnforcementPoints::new();
        for point in EnforcementPoint::ALL.into_iter().filter(|p| *p != missing) {
            points.attest(attestation(point, &format!("{}-svc", point.as_str()))?)?;
        }
        // Premise: exactly two attested.
        assert_eq!(
            EnforcementPoint::ALL
                .iter()
                .filter(|p| points.attestation(**p).is_some())
                .count(),
            2
        );
        let refusal = points
            .all_agree()
            .expect_err("two of three must be refused");
        assert_eq!(
            refusal.reason,
            RefusalReason::EnforcementPointMissing { point: missing }
        );
    }
    Ok(())
}

#[test]
fn a_point_cannot_attest_twice() -> Result<()> {
    // A second attestation from the same point is refused rather than
    // replacing the first; otherwise a point could change its answer after
    // the others had given theirs.
    let mut points = EnforcementPoints::new();
    points.attest(attestation(EnforcementPoint::TransferGate, "gate-svc")?)?;
    let again = points.attest(attestation(EnforcementPoint::TransferGate, "gate-svc-2")?);
    assert!(matches!(again, Err(Error::Denied(_))));
    assert_eq!(
        points
            .attestation(EnforcementPoint::TransferGate)
            .map(|a| a.identity.as_str()),
        Some("gate-svc"),
        "the first attestation stands"
    );
    Ok(())
}

#[test]
fn an_agreement_refuses_the_trading_identity_among_its_attestors() -> Result<()> {
    // "Trading authority and transfer authority never share an identity."
    // Three distinct transfer identities pass the pairwise check, and the
    // check against the trading identity is separate so that it cannot be
    // satisfied by the three merely differing from each other.
    let agreement = three_distinct()?
        .all_agree()
        .expect("premise: three distinct identities agree");
    assert!(
        agreement
            .disjoint_from_trading_authority(&Identity::new("trading-svc")?)
            .is_ok()
    );
    let refusal = agreement
        .disjoint_from_trading_authority(&Identity::new("custody-policy-svc")?)
        .expect_err("an attestor that also trades must be refused");
    assert_eq!(
        refusal.reason,
        RefusalReason::TradingIdentityHoldsTransferAuthority {
            point: EnforcementPoint::CustodyPolicy
        }
    );
    Ok(())
}

#[test]
fn an_empty_identity_or_reference_is_refused_at_construction() -> Result<()> {
    // Two empty identities compare equal, so an anonymous pair would be
    // refused for the wrong reason — or, in a check that trimmed before
    // comparing, not at all. An attestation with no reference says nothing
    // about what it agreed to.
    assert!(matches!(Identity::new("  "), Err(Error::Invalid(_))));
    assert!(matches!(
        Attestation::new(
            EnforcementPoint::TransferGate,
            Identity::new("gate-svc")?,
            "",
            now()
        ),
        Err(Error::Invalid(_))
    ));
    Ok(())
}
