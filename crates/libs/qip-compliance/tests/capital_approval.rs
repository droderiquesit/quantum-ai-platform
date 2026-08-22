//! Control 4 — human capital approvals.
//!
//! Every test tries to obtain capital without a human genuinely deciding to
//! grant it: by approving one's own request, by being both approvers, by
//! reusing a morning's login, by skipping the second signature on a large
//! grant, or by building an envelope by hand and presenting it downstream.

#![allow(clippy::panic_in_result_fn)]

use qip_compliance::approval::{ApprovalChain, CapitalRequest, OperatorCredential};
use qip_compliance::signing::SigningKey;
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::governance::Approval;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp, dec};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn key() -> Result<SigningKey> {
    SigningKey::from_secret("capital-key-2026-01", &[7u8; 32])
}

/// Two approvers needed above one million.
fn chain() -> Result<ApprovalChain> {
    ApprovalChain::new(dec!("1000000"), key()?)
}

fn request(gross: Decimal) -> CapitalRequest {
    CapitalRequest {
        strategy: StrategyId::new("stat-arb-eu"),
        cell: "frankfurt-1".to_string(),
        gross_limit: gross,
        order_limit: dec!("50000"),
        loss_limit: dec!("25000"),
        venues: vec![VenueId::new("XETR")],
        expires_at: now().saturating_add(Duration::from_days(1)),
        requested_by: "r.mensah".to_string(),
    }
}

fn credential(subject: &str, age: Duration) -> Result<OperatorCredential> {
    OperatorCredential::verified(subject, "hardware-token", now().saturating_sub(age))
}

fn approval(gross: Decimal, approver: &str) -> Result<Approval> {
    Approval::new(
        request(gross).subject(),
        approver,
        now(),
        "reviewed the backtest, the shadow run and the venue limits",
    )
}

#[test]
fn capital_is_granted_only_through_the_approval_chain() -> Result<()> {
    let mut chain = chain()?;
    let approved = chain.grant(
        &request(dec!("500000")),
        &approval(dec!("500000"), "k.almeida")?,
        &[credential("k.almeida", Duration::from_mins(2))?],
        now(),
    )?;

    assert_eq!(approved.envelope().gross_limit(), dec!("500000"));
    assert_eq!(approved.approvers(), vec!["k.almeida"]);
    assert_eq!(approved.key_id(), "capital-key-2026-01");
    assert_eq!(chain.grants().len(), 1);
    // The grant is signed by the chain, so a cell can check it after a restart.
    assert!(chain.verifies(approved.envelope()));
    Ok(())
}

#[test]
fn a_requester_cannot_approve_their_own_request() -> Result<()> {
    // One person wearing two hats is the failure this control exists for.
    let mut chain = chain()?;
    let error = chain
        .grant(
            &request(dec!("500000")),
            &approval(dec!("500000"), "r.mensah")?,
            &[credential("r.mensah", Duration::from_mins(1))?],
            now(),
        )
        .expect_err("self-approval must be impossible");

    assert!(error.message().contains("r.mensah"));
    assert!(error.message().contains("own capital request"));
    assert!(chain.grants().is_empty());
    assert_eq!(chain.refusals().len(), 1);
    Ok(())
}

#[test]
fn the_same_person_cannot_be_both_approvers() -> Result<()> {
    // `Approval::countersigned_by` refuses this at the source; the chain
    // re-checks because `Approval` deserialises and its fields are public, so
    // a value can reach the chain without ever passing that constructor.
    let refused = approval(dec!("5000000"), "k.almeida")?.countersigned_by("k.almeida");
    assert!(refused.is_err());

    let mut forged = approval(dec!("5000000"), "k.almeida")?;
    forged.second_approver = Some("k.almeida".to_string());

    let mut chain = chain()?;
    let error = chain
        .grant(
            &request(dec!("5000000")),
            &forged,
            &[credential("k.almeida", Duration::from_mins(1))?],
            now(),
        )
        .expect_err("one person cannot be two approvers");
    assert!(error.message().contains("is not a second approver"));
    Ok(())
}

#[test]
fn a_grant_above_the_threshold_needs_two_different_people() -> Result<()> {
    let mut chain = chain()?;
    assert!(chain.requires_dual_approval(dec!("5000000")));
    assert!(!chain.requires_dual_approval(dec!("500000")));

    // One approver is not enough above the threshold.
    let error = chain
        .grant(
            &request(dec!("5000000")),
            &approval(dec!("5000000"), "k.almeida")?,
            &[credential("k.almeida", Duration::from_mins(1))?],
            now(),
        )
        .expect_err("a large grant needs a second approver");
    assert!(error.message().contains("dual-approval threshold"));

    // Two named people, both authenticated, and it goes through.
    let approved = chain.grant(
        &request(dec!("5000000")),
        &approval(dec!("5000000"), "k.almeida")?.countersigned_by("s.iyer")?,
        &[
            credential("k.almeida", Duration::from_mins(1))?,
            credential("s.iyer", Duration::from_mins(3))?,
        ],
        now(),
    )?;
    assert_eq!(approved.approvers(), vec!["k.almeida", "s.iyer"]);
    Ok(())
}

#[test]
fn a_second_approver_who_requested_the_capital_does_not_count() -> Result<()> {
    let mut chain = chain()?;
    let error = chain
        .grant(
            &request(dec!("5000000")),
            &approval(dec!("5000000"), "k.almeida")?.countersigned_by("r.mensah")?,
            &[
                credential("k.almeida", Duration::from_mins(1))?,
                credential("r.mensah", Duration::from_mins(1))?,
            ],
            now(),
        )
        .expect_err("the requester cannot be the second approver either");
    assert!(error.message().contains("r.mensah"));
    Ok(())
}

#[test]
fn a_stale_credential_is_refused() -> Result<()> {
    // Sixteen minutes, one minute past the window the risk engine uses for an
    // autonomy change. A session token from this morning is not evidence that
    // anybody is at the keyboard now.
    let mut chain = chain()?;
    let error = chain
        .grant(
            &request(dec!("500000")),
            &approval(dec!("500000"), "k.almeida")?,
            &[credential("k.almeida", Duration::from_mins(16))?],
            now(),
        )
        .expect_err("a stale credential must not authorise capital");

    assert!(error.message().contains("k.almeida"));
    assert!(error.message().contains("stale"));
    assert_eq!(chain.maximum_credential_age(), Duration::from_mins(15));
    Ok(())
}

#[test]
fn a_named_approver_with_no_credential_at_all_is_refused() -> Result<()> {
    // A name in a record is not evidence that the person was there.
    let mut chain = chain()?;
    let error = chain
        .grant(
            &request(dec!("500000")),
            &approval(dec!("500000"), "k.almeida")?,
            &[],
            now(),
        )
        .expect_err("an approver who never authenticated is a name in a file");
    assert!(error.message().contains("k.almeida"));
    assert!(error.message().contains("no authenticated credential"));
    Ok(())
}

#[test]
fn an_approval_for_a_different_strategy_cannot_be_replayed() -> Result<()> {
    // Otherwise a pilot's approval is replayed against a scaled request.
    let mut chain = chain()?;
    let elsewhere = Approval::new(
        "capital:momentum-us@newyork-2",
        "k.almeida",
        now(),
        "approved the other book's pilot allocation this morning",
    )?;

    let error = chain
        .grant(
            &request(dec!("500000")),
            &elsewhere,
            &[credential("k.almeida", Duration::from_mins(1))?],
            now(),
        )
        .expect_err("an approval names what it approves");
    assert!(error.message().contains("momentum-us"));
    assert!(error.message().contains("stat-arb-eu"));
    Ok(())
}

#[test]
fn an_envelope_built_by_hand_cannot_become_approved_capital() -> Result<()> {
    // `CapitalEnvelope::new` is public in qip-contracts and always will be, so
    // this is the honest boundary: anyone can build an envelope, nobody can
    // turn an unsigned one into `ApprovedCapital`. Components take
    // `ApprovedCapital`, which is what makes that sufficient.
    let chain = chain()?;
    let hand_rolled = CapitalEnvelope::new(
        StrategyId::new("stat-arb-eu"),
        "frankfurt-1",
        dec!("99000000"),
        dec!("50000"),
        dec!("25000"),
        vec![VenueId::new("XETR")],
        now(),
        now().saturating_add(Duration::from_days(1)),
        "k.almeida",
        "not-a-real-signature",
    )?;

    assert!(!chain.verifies(&hand_rolled));
    let error = chain
        .admit(
            hand_rolled,
            approval(dec!("99000000"), "k.almeida")?,
            now(),
        )
        .expect_err("an unsigned envelope is not approved capital");
    assert!(error.message().contains("does not verify"));
    Ok(())
}

#[test]
fn editing_a_signed_envelopes_limits_invalidates_its_signature() -> Result<()> {
    // The signature covers every field that bounds what the cell may do, so a
    // widened limit is a different payload and no longer verifies.
    let mut chain = chain()?;
    let approved = chain.grant(
        &request(dec!("500000")),
        &approval(dec!("500000"), "k.almeida")?,
        &[credential("k.almeida", Duration::from_mins(2))?],
        now(),
    )?;

    let widened = CapitalEnvelope::new(
        StrategyId::new("stat-arb-eu"),
        "frankfurt-1",
        dec!("50000000"),
        dec!("50000"),
        dec!("25000"),
        vec![VenueId::new("XETR")],
        now(),
        now().saturating_add(Duration::from_days(1)),
        "k.almeida",
        approved.envelope().signature(),
    )?;

    assert!(chain.verifies(approved.envelope()));
    assert!(!chain.verifies(&widened));
    Ok(())
}

#[test]
fn a_signed_envelope_can_be_readmitted_after_a_restart() -> Result<()> {
    // The legitimate case the verification path exists for: a cell restarts,
    // reads its envelope back out of storage, and has to re-establish that it
    // is approved capital rather than bytes.
    let mut chain = chain()?;
    let approved = chain.grant(
        &request(dec!("500000")),
        &approval(dec!("500000"), "k.almeida")?,
        &[credential("k.almeida", Duration::from_mins(2))?],
        now(),
    )?;

    let readmitted = chain.admit(
        approved.envelope().clone(),
        approved.approval().clone(),
        approved.granted_at(),
    )?;
    assert_eq!(readmitted.envelope(), approved.envelope());
    Ok(())
}

#[test]
fn a_short_signing_secret_is_refused() {
    // A key short enough to search offline makes every signature decoration.
    assert!(SigningKey::from_secret("weak", &[1u8; 16]).is_err());
    assert!(SigningKey::from_secret("", &[1u8; 32]).is_err());
}
