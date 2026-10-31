//! The policy payload's decision origin (ADR 0129 decision 5, R12-OBS-7018,
//! R12-OBS-9154): signed when present, and absent from both the wire and the
//! signing string when not.
//!
//! The failure the golden vector prevents is a centre and a fleet of cells
//! upgraded on different days. A field that leaked into the signing string
//! while empty would move the MAC of every payload a not-yet-upgraded centre
//! signs, and every upgraded cell would refuse all of them: the fleet narrows
//! to its conservative floor on a deploy, with nothing wrong but a `|` too
//! many. The vector below was captured at commit `20abd11d`, before the field
//! existed, so it is the old code's bytes and not this code's opinion of them.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::policy::{DecisionOrigin, GrantManifest, PolicyPayload, Slot};
use qip_core::error::Result;
use qip_core::{CorrelationId, Timestamp};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

const KEY: &[u8] = b"policy-golden-vector-key";

/// The fixture the golden vector was taken over: one produced slot, so the
/// vector also pins that a produced slot's digest still lands where it did.
fn payload() -> PolicyPayload {
    let mut payload = PolicyPayload::unproduced(42, "london-1", t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec!["grant-a".to_string()],
        },
        t(5),
    );
    payload
}

const UNPRODUCED: &str = "9513d0f9cc1216b965af87ad066c47b39bad1b7c365d2c9050846b4bc0685260";

/// The signing string `20abd11d` produced for [`payload`].
fn golden_signing() -> String {
    let slot = |name: &str| format!("{name}={UNPRODUCED}");
    [
        "42".to_string(),
        "8:london-1".to_string(),
        "1760000005".to_string(),
        "300000000000".to_string(),
        "false".to_string(),
        slot("trained_models"),
        slot("compiled_plan"),
        slot("belief_priors"),
        slot("episodic_digest"),
        slot("causal_digest"),
        slot("regime_state"),
        "capital_grants=7ea68a80bff5fdfa4a8d30ff0c026beffd0fd22708b147708f96f5d401d8cd48"
            .to_string(),
        slot("cycle_whitelist"),
        slot("risk_envelope"),
        slot("inventory_targets"),
        slot("feasibility_constraints"),
        slot("adversary_profiles"),
    ]
    .join("|")
}

/// The MAC `20abd11d` produced for [`payload`] under [`KEY`].
const GOLDEN_SIGNATURE: &str = "fbb5eb791218bce69b3a732e9109eb93332b371de9aa0e8a481b9dadb887e20d";

/// The wire form `20abd11d` produced for the signed [`payload`].
const GOLDEN_WIRE: &str = concat!(
    r#"{"sequence":42,"cell":"london-1","issued_at":"2025-10-09T08:53:25.000Z","#,
    r#""valid_for":300000000000,"halted":false,"#,
    r#""trained_models":{"value":null,"produced_at":null},"#,
    r#""compiled_plan":{"value":null,"produced_at":null},"#,
    r#""belief_priors":{"value":null,"produced_at":null},"#,
    r#""episodic_digest":{"value":null,"produced_at":null},"#,
    r#""causal_digest":{"value":null,"produced_at":null},"#,
    r#""regime_state":{"value":null,"produced_at":null},"#,
    r#""capital_grants":{"value":{"live_grants":["grant-a"]},"produced_at":"2025-10-09T08:53:25.000Z"},"#,
    r#""cycle_whitelist":{"value":null,"produced_at":null},"#,
    r#""risk_envelope":{"value":null,"produced_at":null},"#,
    r#""inventory_targets":{"value":null,"produced_at":null},"#,
    r#""feasibility_constraints":{"value":null,"produced_at":null},"#,
    r#""adversary_profiles":{"value":null,"produced_at":null},"#,
    r#""signature":"fbb5eb791218bce69b3a732e9109eb93332b371de9aa0e8a481b9dadb887e20d"}"#
);

#[test]
fn a_payload_with_no_decision_origin_signs_and_serialises_byte_for_byte_as_before_the_field()
-> Result<()> {
    let unsigned = payload();
    assert!(unsigned.decision.is_none(), "premise: no origin stated");
    assert_eq!(
        unsigned.signing_payload()?,
        golden_signing(),
        "an absent decision origin moved the signing string, so every payload a centre built \
         before the field now fails to verify at an upgraded cell"
    );
    let signed = unsigned.signed(KEY)?;
    assert_eq!(signed.signature, GOLDEN_SIGNATURE);
    let wire = serde_json::to_string(&signed).expect("serialisable");
    assert_eq!(
        wire, GOLDEN_WIRE,
        "an absent decision origin is on the wire, so an old cell refuses the payload under \
         deny_unknown_fields"
    );
    // A payload a pre-field centre sent decodes to the same value, with no
    // origin, and re-signs to the signature it arrived with.
    let decoded: PolicyPayload = serde_json::from_str(GOLDEN_WIRE).expect("a pre-field payload");
    assert!(decoded.decision.is_none());
    assert_eq!(decoded, signed);
    assert_eq!(decoded.signed(KEY)?.signature, GOLDEN_SIGNATURE);
    Ok(())
}

#[test]
fn a_decision_origin_is_signed_so_a_payload_cannot_be_reattributed_to_another_cycle() -> Result<()>
{
    let origin = DecisionOrigin::new(3, CorrelationId::from_string("corr-cycle-3"))?;
    let mut stamped = payload();
    stamped.decision = Some(origin.clone());

    let signing = stamped.signing_payload()?;
    assert_eq!(
        signing,
        format!("{}|decision=3:12:corr-cycle-3", golden_signing()),
        "a stated origin is not in the signing string exactly once, after every slot"
    );
    let signed = stamped.clone().signed(KEY)?;
    assert_ne!(
        signed.signature, GOLDEN_SIGNATURE,
        "a stated origin left the MAC unchanged, so a relay could graft one on"
    );

    // Re-attributing a signed payload — another cycle, another correlation,
    // or the origin stripped — no longer matches the MAC it carries.
    let resign =
        |payload: &PolicyPayload| -> Result<String> { Ok(payload.clone().signed(KEY)?.signature) };
    let mut other_cycle = signed.clone();
    other_cycle.decision = Some(DecisionOrigin::new(4, origin.correlation.clone())?);
    let mut other_correlation = signed.clone();
    other_correlation.decision = Some(DecisionOrigin::new(
        3,
        CorrelationId::from_string("corr-cycle-4"),
    )?);
    let mut stripped = signed.clone();
    stripped.decision = None;
    for (label, forged) in [
        ("cycle", &other_cycle),
        ("correlation", &other_correlation),
        ("stripped", &stripped),
    ] {
        assert_ne!(
            resign(forged)?,
            signed.signature,
            "a payload whose origin was {label}-edited still verifies"
        );
    }

    // It rides the wire and comes back whole.
    let wire = serde_json::to_string(&signed).expect("serialisable");
    assert!(
        wire.contains(r#""decision":{"cycle":3,"correlation":"corr-cycle-3"}"#),
        "{wire}"
    );
    let decoded: PolicyPayload = serde_json::from_str(&wire).expect("decodes");
    assert_eq!(decoded.decision, Some(origin));
    assert_eq!(resign(&decoded)?, signed.signature);
    Ok(())
}

#[test]
fn a_decision_origin_naming_no_cycle_is_refused() {
    let zero = DecisionOrigin::new(0, CorrelationId::from_string("corr-1")).unwrap_err();
    assert!(zero.message().contains("cycle zero"), "{}", zero.message());
    let empty = DecisionOrigin::new(1, CorrelationId::from_string("")).unwrap_err();
    assert!(
        empty.message().contains("empty correlation"),
        "{}",
        empty.message()
    );
}
