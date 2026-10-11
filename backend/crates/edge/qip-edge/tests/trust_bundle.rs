//! A cell holding a trust bundle (ADR 0110) accepts the centre's artefacts
//! only under the ECDSA key published for their class, and refuses an HMAC,
//! a key outside the bundle, and a signature moved between classes.
//!
//! Driven through the downlinks' real verify path over a scripted frame
//! source — the same path the mesh and the fabric deliver through — so the
//! property is what a cell applies, not what a helper returns.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{HaltCommand, PolicyPayload};
use qip_contracts::signal::StrategyId;
use qip_contracts::trust::{ArtefactClass, ArtefactSigner, TrustBundle, TrustedKey, sign_artefact};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{CorrelationId, Id, Lineage, Timestamp, dec};
use qip_edge::TrustRoot;
use qip_edge::envelope::sign_payload;
use qip_edge::mesh::{
    CapitalDownlink, CapitalGrantTopic, DEFAULT_GRANT_MEMORY, HaltTopic, PolicyDownlink,
    PolicyPayloadTopic, ScriptedFrames,
};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_transport::trust::{BundleVerifier, LocalSigner};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const CELL: &str = "london-1";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct GrantBody(CapitalEnvelope);
impl EventBody for GrantBody {
    const TOPIC: Topic = CapitalGrantTopic::TOPIC;
    const SCHEMA_VERSION: u32 = CapitalGrantTopic::SCHEMA_VERSION;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct PolicyBody(PolicyPayload);
impl EventBody for PolicyBody {
    const TOPIC: Topic = PolicyPayloadTopic::TOPIC;
    const SCHEMA_VERSION: u32 = PolicyPayloadTopic::SCHEMA_VERSION;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct HaltBody(HaltCommand);
impl EventBody for HaltBody {
    const TOPIC: Topic = HaltTopic::TOPIC;
    const SCHEMA_VERSION: u32 = HaltTopic::SCHEMA_VERSION;
}

fn frame<B: EventBody>(body: B, event_id: &str) -> Result<AnyEvent> {
    Envelope::new(
        Id::from_string(event_id.to_string()),
        t(0),
        t(0),
        Lineage::root(
            CorrelationId::from_string(format!("COR{event_id}")),
            "qip-edge-trust-tests",
        ),
        body,
    )
    .erase()
}

/// The centre's three class keys and the bundle that publishes them.
struct Keys {
    policy: LocalSigner,
    halt: LocalSigner,
    envelope: LocalSigner,
}

fn keys() -> Result<Keys> {
    Ok(Keys {
        policy: LocalSigner::generate(ArtefactClass::Policy, "test/policy/1")?,
        halt: LocalSigner::generate(ArtefactClass::Halt, "test/halt/1")?,
        envelope: LocalSigner::generate(ArtefactClass::Envelope, "test/envelope/1")?,
    })
}

fn root(keys: &Keys) -> Result<TrustRoot> {
    let entries = [&keys.policy, &keys.halt, &keys.envelope]
        .into_iter()
        .map(|signer| TrustedKey {
            class: signer.class(),
            key_version: signer.key_version().to_string(),
            public_key: signer.public_key_hex(),
            not_after: Timestamp::from_secs(4_000_000_000),
        })
        .collect();
    Ok(TrustRoot::bundle(BundleVerifier::new(TrustBundle::new(
        entries,
        BTreeSet::new(),
    )?)))
}

fn grant(signature: impl Fn(&CapitalEnvelope) -> Result<String>) -> Result<CapitalEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("mean-reversion-1"),
            CELL,
            dec!("10000"),
            dec!("400"),
            dec!("2000"),
            vec![VenueId::new("XLON")],
            t(0),
            t(7_200),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    build(&signature(&unsigned)?)
}

/// Mutation: in `TrustRoot::check`, route the bundle arm through the shared
/// key's MAC comparison (or accept any signature that parses) and the HMAC
/// and stranger grants are applied; this fails.
#[test]
fn a_bundle_cell_applies_a_grant_only_under_the_envelope_key() -> Result<()> {
    let keys = keys()?;
    let stranger = LocalSigner::generate(ArtefactClass::Envelope, "test/envelope/1")?;
    let genuine = grant(|unsigned| {
        sign_artefact(
            &keys.envelope,
            ArtefactClass::Envelope,
            unsigned.signing_payload().as_bytes(),
        )
    })?;
    let mac = grant(|unsigned| {
        Ok(sign_payload(
            b"a-shared-key-that-once-verified-everything",
            &unsigned.signing_payload(),
        ))
    })?;
    let forged = grant(|unsigned| {
        sign_artefact(
            &stranger,
            ArtefactClass::Envelope,
            unsigned.signing_payload().as_bytes(),
        )
    })?;
    // Signed under the policy key: a real signature from a trusted key, of
    // the wrong class.
    let crossed = grant(|unsigned| {
        sign_artefact(
            &keys.policy,
            ArtefactClass::Policy,
            unsigned.signing_payload().as_bytes(),
        )
    })?;

    let mut source = ScriptedFrames::new(8)?;
    for (body, id) in [
        (genuine, "EVT-GENUINE"),
        (mac, "EVT-HMAC"),
        (forged, "EVT-STRANGER"),
        (crossed, "EVT-CROSSED"),
    ] {
        source.push(frame(GrantBody(body), id)?)?;
    }
    let mut downlink = CapitalDownlink::from_source_trusted(
        CELL,
        root(&keys)?,
        DEFAULT_GRANT_MEMORY,
        Box::new(source),
    )?;
    let batch = downlink.poll(t(1))?;
    assert_eq!(batch.verified.len(), 1, "{batch:?}");
    let refused: Vec<&str> = batch
        .refused
        .iter()
        .map(|refusal| refusal.event_id.as_str())
        .collect();
    assert_eq!(refused, vec!["EVT-HMAC", "EVT-STRANGER", "EVT-CROSSED"]);
    Ok(())
}

/// Mutation: verify halts under the policy key (pass `ArtefactClass::Policy`
/// in `VerifiedHalt::verify_with`) and the halt signed by the halt key is
/// refused while the one signed by the policy key is applied; this fails.
#[test]
fn a_bundle_cell_applies_payloads_and_halts_each_under_its_own_class_key() -> Result<()> {
    let keys = keys()?;
    let payload = PolicyPayload::unproduced(7, CELL, t(0)).signed_by(&keys.policy)?;
    let hmac_payload = PolicyPayload::unproduced(8, CELL, t(0))
        .signed(b"a-shared-key-that-once-verified-everything")?;
    let halt = HaltCommand::new(CELL, t(0), "operator stop").signed_by(&keys.halt)?;
    // A halt carrying a genuine policy-key signature over its own bytes.
    let mut policy_signed_halt = HaltCommand::new(CELL, t(0), "operator stop");
    policy_signed_halt.signature = sign_artefact(
        &keys.policy,
        ArtefactClass::Policy,
        policy_signed_halt.signing_payload().as_bytes(),
    )?;

    let mut source = ScriptedFrames::new(8)?;
    source.push(frame(PolicyBody(payload), "EVT-POLICY")?)?;
    source.push(frame(PolicyBody(hmac_payload), "EVT-POLICY-HMAC")?)?;
    source.push(frame(HaltBody(halt), "EVT-HALT")?)?;
    source.push(frame(HaltBody(policy_signed_halt), "EVT-HALT-WRONG-CLASS")?)?;
    let mut downlink = PolicyDownlink::from_source_trusted(CELL, root(&keys)?, Box::new(source));
    let batch = downlink.poll(t(1))?;
    assert_eq!(batch.verified.len(), 1, "{batch:?}");
    assert_eq!(batch.verified[0].sequence(), 7);
    assert_eq!(batch.halts.len(), 1, "{batch:?}");
    let refused: Vec<&str> = batch
        .refused
        .iter()
        .map(|refusal| refusal.event_id.as_str())
        .collect();
    assert_eq!(refused, vec!["EVT-POLICY-HMAC", "EVT-HALT-WRONG-CLASS"]);
    Ok(())
}

/// The other direction: a cell still on the shared key refuses an
/// asymmetric signature by name rather than trying it as a MAC.
/// Mutation: drop the `ArtefactSignature::parse` arm in the shared-key
/// branch and the refusal reason changes; this fails.
#[test]
fn a_shared_key_cell_refuses_an_asymmetric_payload_naming_the_bundle() -> Result<()> {
    let keys = keys()?;
    let payload = PolicyPayload::unproduced(9, CELL, t(0)).signed_by(&keys.policy)?;
    let mut source = ScriptedFrames::new(4)?;
    source.push(frame(PolicyBody(payload), "EVT-ASYMMETRIC")?)?;
    let mut downlink = PolicyDownlink::from_source_trusted(
        CELL,
        TrustRoot::shared_key(b"a-shared-key-that-once-verified-everything")?,
        Box::new(source),
    );
    let batch = downlink.poll(t(1))?;
    assert!(batch.verified.is_empty());
    assert_eq!(batch.refused.len(), 1);
    assert!(
        batch.refused[0].reason.contains("trust bundle"),
        "{}",
        batch.refused[0].reason
    );
    Ok(())
}
