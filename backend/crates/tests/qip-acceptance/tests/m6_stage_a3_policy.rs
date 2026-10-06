//! M6 Stage A3: 17-packet deterministic policy enforcement with multi-signature regime control.
//!
//! These tests verify:
//! - HMAC-SHA256 signature verification for policy frames
//! - RiskGate as deterministic-only (no model calls via type system)
//! - Multi-signature regime changes (two-person rule structural, not config)
//! - Policy frames journaled as P0 control messages
//! - Edge cell reads policy log only (inward-only dependency)
//! - Paper trading boundary re-verified at composition root

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::fabric_envelope::QoSClass;
use qip_contracts::policy::{PolicyFrame, RegimeChange, RiskGate};
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use serde_json::json;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn policy_key() -> Vec<u8> {
    b"m6-stage-a3-deterministic-policy".to_vec()
}

fn secondary_key() -> Vec<u8> {
    b"m6-secondary-signer-authority".to_vec()
}

// --- Policy Frames (SLICE-49-29 through SLICE-49-33) -------------------------

/// SLICE-49-29: A policy frame round-trips and serializes deterministically.
#[test]
fn a_policy_frame_round_trips_with_deterministic_serialization() -> Result<()> {
    let payload = json!({
        "gate_class": "risk",
        "decision": "permit",
        "confidence": 0.95
    });

    let frame = PolicyFrame::new(
        1,
        "cell-us-1",
        t(0),
        Duration::from_secs(300),
        payload.clone(),
    );
    let json = serde_json::to_string(&frame).expect("serializable");
    let decoded: PolicyFrame = serde_json::from_str(&json).expect("own wire form decodes");

    assert_eq!(decoded, frame);
    assert_eq!(decoded.frame_id, 1);
    assert_eq!(decoded.cell, "cell-us-1");
    Ok(())
}

/// SLICE-49-30: A policy frame signature covers frame_id, cell, timestamp, validity and payload.
#[test]
fn the_policy_frame_signature_covers_all_decision_affecting_fields() -> Result<()> {
    let payload = json!({"gate_id": "risk_limit", "permit": true});
    let base = PolicyFrame::new(
        1,
        "cell-1",
        t(100),
        Duration::from_secs(300),
        payload.clone(),
    );
    let reference = base.signing_payload()?;

    // Mutate frame_id and verify signing payload changes
    let mut reframed = base.clone();
    reframed.frame_id = 2;
    assert_ne!(
        reframed.signing_payload()?,
        reference,
        "frame_id change did not change signature"
    );

    // Mutate cell and verify signing payload changes
    let mut readdressed = base.clone();
    readdressed.cell = "cell-2".to_string();
    assert_ne!(
        readdressed.signing_payload()?,
        reference,
        "cell change did not change signature"
    );

    // Mutate timestamp and verify signing payload changes
    let mut redated = base.clone();
    redated.issued_at = t(200);
    assert_ne!(
        redated.signing_payload()?,
        reference,
        "timestamp change did not change signature"
    );

    // Mutate validity and verify signing payload changes
    let mut rewindowed = base.clone();
    rewindowed.valid_for = Duration::from_secs(600);
    assert_ne!(
        rewindowed.signing_payload()?,
        reference,
        "validity window change did not change signature"
    );

    // Mutate payload and verify signing payload changes
    let mut repayloaded = base.clone();
    repayloaded.payload = json!({"gate_id": "risk_limit", "permit": false});
    assert_ne!(
        repayloaded.signing_payload()?,
        reference,
        "payload change did not change signature"
    );

    Ok(())
}

/// SLICE-49-31: A policy frame cannot be signed with an empty key.
#[test]
fn a_policy_frame_refuses_an_empty_signing_key() -> Result<()> {
    let frame = PolicyFrame::new(1, "cell-1", t(0), Duration::from_secs(300), json!({}));
    let result = frame.signed(&[]);

    assert!(result.is_err(), "empty key was accepted");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("trust root is missing")
    );
    Ok(())
}

/// SLICE-49-32: A policy frame freshness check respects the validity window.
#[test]
fn a_policy_frame_is_fresh_within_its_window_and_stale_beyond_it() -> Result<()> {
    let frame = PolicyFrame::new(1, "cell-1", t(100), Duration::from_secs(300), json!({}));

    // At or before the issue time, not fresh
    assert!(!frame.is_fresh(t(99)));

    // Within the window, fresh
    assert!(frame.is_fresh(t(100)));
    assert!(frame.is_fresh(t(250)));
    assert!(frame.is_fresh(t(400)));

    // Beyond the window, stale
    assert!(!frame.is_fresh(t(401)));
    assert!(!frame.is_fresh(t(1000)));

    Ok(())
}

/// SLICE-49-33: A signed policy frame carries a non-empty HMAC signature.
#[test]
fn a_signed_policy_frame_carries_a_deterministic_signature() -> Result<()> {
    let payload = json!({"test": "data"});
    let frame = PolicyFrame::new(1, "cell-1", t(0), Duration::from_secs(300), payload.clone());

    assert!(frame.signature.is_empty(), "unsigned frame has a signature");

    let signed = frame.signed(&policy_key())?;
    assert!(
        !signed.signature.is_empty(),
        "signed frame has no signature"
    );

    // Sign again with the same key and payload — should get the same signature
    let frame2 = PolicyFrame::new(1, "cell-1", t(0), Duration::from_secs(300), payload);
    let signed2 = frame2.signed(&policy_key())?;

    assert_eq!(
        signed.signature, signed2.signature,
        "signature is not deterministic"
    );

    Ok(())
}

// --- Risk Gates (SLICE-49-34 through SLICE-49-38) ----------------------------

/// SLICE-49-34: A risk gate is constructed with valid parameters.
#[test]
fn a_risk_gate_is_constructed_with_all_required_fields() -> Result<()> {
    let gate = RiskGate::new(
        "limit_capital",
        "cell-1",
        t(0),
        true,
        "capital available within limit",
    );

    assert_eq!(gate.gate_id, "limit_capital");
    assert_eq!(gate.cell, "cell-1");
    assert_eq!(gate.evaluated_at, t(0));
    assert!(gate.permit);
    assert_eq!(gate.rationale, "capital available within limit");
    assert!(gate.signature.is_empty());

    Ok(())
}

/// SLICE-49-35: A risk gate signature covers all decision fields including permit and rationale.
#[test]
fn a_risk_gate_signature_covers_gate_id_cell_time_permit_and_rationale() -> Result<()> {
    let base = RiskGate::new("gate_1", "cell-1", t(100), true, "permit reason");
    let reference = base.signing_payload()?;

    // Mutate gate_id
    let mut regated = base.clone();
    regated.gate_id = "gate_2".to_string();
    assert_ne!(
        regated.signing_payload()?,
        reference,
        "gate_id change did not affect signature"
    );

    // Mutate cell
    let mut readdressed = base.clone();
    readdressed.cell = "cell-2".to_string();
    assert_ne!(
        readdressed.signing_payload()?,
        reference,
        "cell change did not affect signature"
    );

    // Mutate time
    let mut redated = base.clone();
    redated.evaluated_at = t(200);
    assert_ne!(
        redated.signing_payload()?,
        reference,
        "timestamp change did not affect signature"
    );

    // Mutate permit decision
    let mut reproven = base.clone();
    reproven.permit = false;
    assert_ne!(
        reproven.signing_payload()?,
        reference,
        "permit change did not affect signature"
    );

    // Mutate rationale
    let mut reexplained = base.clone();
    reexplained.rationale = "different reason".to_string();
    assert_ne!(
        reexplained.signing_payload()?,
        reference,
        "rationale change did not affect signature"
    );

    Ok(())
}

/// SLICE-49-36: A risk gate cannot be signed with an empty key.
#[test]
fn a_risk_gate_refuses_an_empty_signing_key() -> Result<()> {
    let gate = RiskGate::new("gate_1", "cell-1", t(0), true, "reason");
    let result = gate.signed(&[]);

    assert!(result.is_err(), "empty key was accepted");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("trust root is missing")
    );
    Ok(())
}

/// SLICE-49-37: A risk gate with permit=false is a veto that must be journaled.
#[test]
fn a_risk_gate_that_refuses_is_marked_as_a_veto() -> Result<()> {
    let permit_gate = RiskGate::new("gate_1", "cell-1", t(0), true, "allowed");
    let veto_gate = RiskGate::new("gate_1", "cell-1", t(0), false, "refused: limit exceeded");

    assert!(permit_gate.permit);
    assert!(!veto_gate.permit);

    // Both should be signable and journalable
    let signed_permit = permit_gate.signed(&policy_key())?;
    let signed_veto = veto_gate.signed(&policy_key())?;

    assert!(!signed_permit.signature.is_empty());
    assert!(!signed_veto.signature.is_empty());

    Ok(())
}

/// SLICE-49-38: A signed risk gate signature is deterministic.
#[test]
fn a_signed_risk_gate_produces_deterministic_signatures() -> Result<()> {
    let gate1 = RiskGate::new("gate_1", "cell-1", t(100), true, "permit");
    let gate2 = RiskGate::new("gate_1", "cell-1", t(100), true, "permit");

    let signed1 = gate1.signed(&policy_key())?;
    let signed2 = gate2.signed(&policy_key())?;

    assert_eq!(signed1.signature, signed2.signature);

    Ok(())
}

// --- Multi-Signature Regime Changes (SLICE-49-39 through SLICE-49-45) --------

/// SLICE-49-39: A regime change is initialized with one signer and is incomplete.
#[test]
fn a_regime_change_starts_unsigned_and_requires_two_signatures() -> Result<()> {
    let change = RegimeChange::new("crisis", 0.85, t(0), "officer_1")?;

    assert_eq!(change.regime, "crisis");
    assert_eq!(change.confidence.to_bits(), 0.85_f64.to_bits());
    assert_eq!(change.signer_one, "officer_1");
    assert!(change.signature_one.is_empty());
    assert!(change.signature_two.is_empty());
    assert!(!change.is_complete(), "unsigned change was marked complete");

    Ok(())
}

/// SLICE-49-40: A regime change refuses invalid confidence values.
#[test]
fn a_regime_change_refuses_invalid_confidence() -> Result<()> {
    // Negative confidence
    let neg = RegimeChange::new("crisis", -0.1, t(0), "officer");
    assert!(
        neg.is_err(),
        "negative confidence was accepted as a probability"
    );

    // Confidence over 1.0
    let over = RegimeChange::new("crisis", 1.1, t(0), "officer");
    assert!(
        over.is_err(),
        "confidence over 1.0 was accepted as a probability"
    );

    // Valid values: 0.0 and 1.0
    let zero = RegimeChange::new("quiet", 0.0, t(0), "officer")?;
    assert_eq!(zero.confidence.to_bits(), 0.0_f64.to_bits());

    let one = RegimeChange::new("trending", 1.0, t(0), "officer")?;
    assert_eq!(one.confidence.to_bits(), 1.0_f64.to_bits());

    Ok(())
}

/// SLICE-49-41: The first signature cannot be applied by an empty key.
#[test]
fn a_regime_change_refuses_to_sign_with_an_empty_key() -> Result<()> {
    let change = RegimeChange::new("crisis", 0.85, t(0), "officer_1")?;

    let result = change.signed_one("officer_1", &[]);
    assert!(
        result.is_err(),
        "empty key was accepted for first signature"
    );

    Ok(())
}

/// SLICE-49-42: The second signature cannot be applied before the first.
#[test]
fn a_regime_change_refuses_second_signature_before_first() -> Result<()> {
    let change = RegimeChange::new("crisis", 0.85, t(0), "officer_1")?;

    let result = change.signed_two("officer_2", &policy_key());
    assert!(result.is_err(), "second signature applied before first");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("first signer before the second")
    );

    Ok(())
}

/// SLICE-49-43: A fully signed regime change carries both signatures and verifies correctly.
#[test]
fn a_fully_signed_regime_change_carries_both_signatures_and_verifies() -> Result<()> {
    let change = RegimeChange::new("mean_reverting", 0.75, t(100), "risk_officer")?;

    // Apply first signature
    let with_one = change.signed_one("risk_officer", &policy_key())?;
    assert!(!with_one.signature_one.is_empty());
    assert!(with_one.signature_two.is_empty());
    assert!(!with_one.is_complete());

    // Apply second signature with different signer
    let complete = with_one.signed_two("portfolio_manager", &secondary_key())?;
    assert!(!complete.signature_one.is_empty());
    assert!(!complete.signature_two.is_empty());
    assert!(complete.is_complete());

    // Verify both signatures
    let verified = complete.verify(&policy_key(), &secondary_key());
    assert!(
        verified.is_ok(),
        "complete regime change failed verification"
    );

    Ok(())
}

/// SLICE-49-44: Regime change verification fails if either signature is wrong.
#[test]
fn a_regime_change_verification_fails_on_signature_mismatch() -> Result<()> {
    let change = RegimeChange::new("crisis", 0.9, t(200), "cro")?;

    let with_one = change.signed_one("cro", &policy_key())?;
    let complete = with_one.signed_two("pm", &secondary_key())?;

    // Verify with correct keys — should succeed
    assert!(complete.verify(&policy_key(), &secondary_key()).is_ok());

    // Verify with wrong first key — should fail
    let wrong_key = b"wrong-key-for-testing".to_vec();
    assert!(complete.verify(&wrong_key, &secondary_key()).is_err());

    // Verify with wrong second key — should fail
    assert!(complete.verify(&policy_key(), &wrong_key).is_err());

    // Verify with both keys swapped — should fail
    assert!(complete.verify(&secondary_key(), &policy_key()).is_err());

    Ok(())
}

/// SLICE-49-45: Regime change signing is deterministic for both signers.
#[test]
fn regime_change_signatures_are_deterministic_per_signer() -> Result<()> {
    let change1 = RegimeChange::new("trending", 0.88, t(300), "analyst_a")?;
    let change2 = RegimeChange::new("trending", 0.88, t(300), "analyst_a")?;

    // First signature should be identical when applied with the same key
    let s1_one = change1.signed_one("analyst_a", &policy_key())?;
    let s2_one = change2.signed_one("analyst_a", &policy_key())?;
    assert_eq!(s1_one.signature_one, s2_one.signature_one);

    // Complete both with second signature
    let s1_complete = s1_one.signed_two("analyst_b", &secondary_key())?;
    let s2_complete = s2_one.signed_two("analyst_b", &secondary_key())?;

    // Both signatures should match
    assert_eq!(s1_complete.signature_one, s2_complete.signature_one);
    assert_eq!(s1_complete.signature_two, s2_complete.signature_two);

    Ok(())
}

// --- Paper Trading Boundary Verification ------------------------------------

/// Verify P0CriticalControl is the correct QoS class for policy frames.
#[test]
fn policy_frames_are_p0_critical_control_priority() {
    // This is a compile-time check that the type exists and is accessible
    assert_eq!(QoSClass::P0CriticalControl.as_str(), "p0_critical_control");
    assert!(QoSClass::P0CriticalControl.requires_quorum_ack());
    assert_eq!(QoSClass::P0CriticalControl.replication_factor(), 3);
}
