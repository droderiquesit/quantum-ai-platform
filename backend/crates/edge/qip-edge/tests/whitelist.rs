//! The structured cycle whitelist against the signature it must not break.
//!
//! `CycleWhitelist` gained `conversions` and `start_sizes` so a desk could be
//! built from it. The payload is signed over each slot's serialised bytes and
//! refuses unknown fields, so an additive field is safe in exactly one
//! shape: skipped when empty. This holds that shape from the verifier's side
//! — an old-era body verifies, an empty structured whitelist never reaches
//! the wire, and an unknown key inside the slot is still refused.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::policy::{CycleWhitelist, PolicyPayload, Slot};
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_edge::policy::VerifiedPolicy;
use std::collections::BTreeMap;

const KEY: &[u8] = b"a-cell-policy-key-for-tests";
const CELL: &str = "london-1";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

#[test]
fn a_payload_signed_before_the_structured_whitelist_existed_still_verifies() -> Result<()> {
    // The slot's digest is over its serialised bytes, and the new fields
    // are skipped when empty. So a payload whose whitelist is the string
    // map alone serialises without them — byte for byte the shape that was
    // signed before they existed — and a JSON body from that era, which has
    // no such keys, deserialises and verifies against the same signature.
    let whitelist = CycleWhitelist {
        cycles: BTreeMap::from([("eth-triangle".to_string(), "1".to_string())]),
        conversions: Vec::new(),
        start_sizes: BTreeMap::new(),
    };
    let mut payload = PolicyPayload::unproduced(7, CELL, t(10));
    payload.cycle_whitelist = Slot::produced(whitelist, t(10));
    let signed = payload.signed(KEY)?;
    let json = serde_json::to_string(&signed)?;
    assert!(
        !json.contains("conversions") && !json.contains("start_sizes"),
        "an empty structured whitelist reached the wire, so the digest of an old payload \
         would change: {json}"
    );
    // Premise: the whitelist itself did reach the wire, so the absence
    // above is the fields being skipped and not the slot being empty.
    assert!(json.contains("eth-triangle"), "the premise failed: {json}");

    let old_era: PolicyPayload = serde_json::from_str(&json)?;
    assert_eq!(old_era, signed, "the round trip changed the payload");
    assert!(
        VerifiedPolicy::verify(old_era, KEY, CELL, t(10)).is_ok(),
        "a payload without the structured whitelist no longer verifies"
    );

    // And a body carrying a key this crate does not know is still refused,
    // which is what keeps the additive field from being an open door.
    let widened = json.replace(r#""cycles""#, r#""ceiling":"live","cycles""#);
    assert!(
        widened != json,
        "the premise failed: the body was not widened"
    );
    assert!(
        serde_json::from_str::<PolicyPayload>(&widened).is_err(),
        "an unknown key inside the whitelist was accepted"
    );
    Ok(())
}
