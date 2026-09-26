//! The v2 reflex digest and the posting fields on `Filled`, from outside the
//! contract crate.
//!
//! Two properties a reader of a mirrored journal relies on without being able
//! to see the writer: that a v2 digest names a decision's content and not the
//! order some serialiser wrote it in (red-team M13), and that adding the
//! posting fields to `Filled` (red-team B2) left every fill a cell sealed
//! before them byte-identical, so its v1 digest still verifies.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::BookSide;
use qip_contracts::reflex::{Decision, canonical_decision, chain_digest_v1, chain_digest_v2};
use qip_core::Timestamp;
use qip_core::error::Result;

/// The canonical body of the posted fill below, written out by hand in
/// sorted-key order, so the pinned digest does not come from the code under
/// test.
const CANONICAL_POSTED_FILL: &str = "{\"Filled\":{\"fee\":\"0.25\",\"object\":\"obj-1\",\
     \"order_id\":\"ord-1\",\"price\":\"101.5\",\"quantity\":\"10\",\"quote_unit\":\"USD\",\
     \"shares\":[[\"alpha\",\"10\"]],\"side\":\"Ask\",\"simulated\":true,\"venue\":\"SIM\"}}";

#[test]
fn a_v2_digest_is_the_same_whatever_order_the_decisions_fields_are_serialised_in() -> Result<()> {
    // The same fill, arriving as text in two field orders: declaration order,
    // and reversed. A reader that re-serialised either and hashed what came
    // out would get whatever its serialiser's order was, and two honest
    // parties would disagree about one entry.
    let declared = "{\"Filled\":{\"order_id\":\"ord-1\",\"venue\":\"SIM\",\"object\":\"obj-1\",\
        \"quantity\":\"10\",\"price\":\"101.5\",\"simulated\":true,\
        \"shares\":[[\"alpha\",\"10\"]],\"side\":\"Ask\",\"quote_unit\":\"USD\",\"fee\":\"0.25\"}}";
    let reversed = "{\"Filled\":{\"fee\":\"0.25\",\"quote_unit\":\"USD\",\"side\":\"Ask\",\
        \"shares\":[[\"alpha\",\"10\"]],\"simulated\":true,\"price\":\"101.5\",\
        \"quantity\":\"10\",\"object\":\"obj-1\",\"venue\":\"SIM\",\"order_id\":\"ord-1\"}}";
    let first: Decision = serde_json::from_str(declared).expect("the declared order reads");
    let second: Decision = serde_json::from_str(reversed).expect("the reversed order reads");
    assert_eq!(first, second, "premise: both texts name one decision");
    assert!(
        matches!(
            &first,
            Decision::Filled { side: Some(BookSide::Ask), fee: Some(fee), .. } if fee == "0.25"
        ),
        "premise: the posting fields were read"
    );

    // Premise: declaration-order JSON is not the canonical form, so a digest
    // over the one is distinguishable from a digest over the other.
    let declaration_order = serde_json::to_string(&first).expect("serialises");
    assert_ne!(declaration_order, CANONICAL_POSTED_FILL);

    let at = Timestamp::from_nanos(1_700_000_000_123_456_789);
    assert_eq!(canonical_decision(&first)?, CANONICAL_POSTED_FILL);
    let expected = qip_core::sha256_hex(
        format!("v2|genesis|7|1700000000123456789|{CANONICAL_POSTED_FILL}").as_bytes(),
    );
    assert_eq!(chain_digest_v2("genesis", 7, at, &first)?, expected);
    assert_eq!(chain_digest_v2("genesis", 7, at, &second)?, expected);
    Ok(())
}

#[test]
fn a_filled_entry_without_side_quote_unit_or_fee_serialises_exactly_as_before() {
    // Every fill a cell sealed before the posting fields existed was hashed
    // under v1 through this exact text. If an absent field were written back
    // as `null` the text would change and every one of those digests would
    // stop verifying.
    let unposted = Decision::Filled {
        order_id: "ord-1".to_string(),
        venue: "SIM".to_string(),
        object: "obj-1".to_string(),
        quantity: "10".to_string(),
        price: "101.5".to_string(),
        simulated: true,
        shares: vec![("alpha".to_string(), "10".to_string())],
        side: None,
        quote_unit: None,
        fee: None,
    };
    let posted = Decision::Filled {
        order_id: "ord-1".to_string(),
        venue: "SIM".to_string(),
        object: "obj-1".to_string(),
        quantity: "10".to_string(),
        price: "101.5".to_string(),
        simulated: true,
        shares: vec![("alpha".to_string(), "10".to_string())],
        side: None,
        quote_unit: None,
        fee: Some("0".to_string()),
    };
    // Premise: a reported fee — even a zero one, which is a venue fact and
    // not an absence — is written, so the absence below is the attribute's
    // doing and not a field the serialiser never sees.
    assert!(
        serde_json::to_string(&posted)
            .expect("serialises")
            .ends_with(",\"fee\":\"0\"}}"),
        "a reported fee was not written"
    );

    assert_eq!(
        serde_json::to_string(&unposted).expect("serialises"),
        "{\"Filled\":{\"order_id\":\"ord-1\",\"venue\":\"SIM\",\"object\":\"obj-1\",\
         \"quantity\":\"10\",\"price\":\"101.5\",\"simulated\":true,\
         \"shares\":[[\"alpha\",\"10\"]]}}"
    );
    // The digest SLICE-07 pinned from a reproduction of the pre-move code.
    assert_eq!(
        chain_digest_v1("genesis", 0, Timestamp::from_secs(1_700_000_000), &unposted),
        "9f27d4aac9d56ec26c8844c7757e32252e1980a8a5d37d6378f4e6ae93706f85"
    );
}
