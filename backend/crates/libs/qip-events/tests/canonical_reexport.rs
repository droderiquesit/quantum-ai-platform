//! `qip_events::envelope::canonical_json` must be the foundation function,
//! not a copy of it. Two copies are two definitions of one identity: a fix to
//! one would re-key half the platform's digests and leave the other half
//! alone, and every event hashed through this crate would stop matching the
//! reflex chain hashed through `qip-contracts`.

use serde_json::json;

#[test]
fn qip_events_canonical_json_is_the_foundation_function() {
    // A key that needs escaping is where a hand-copied serialiser most often
    // diverges, so it is the input that tells a copy from the original.
    let value = json!({"outer": [{"q\"k": 1, "a": 2}], "b": null});
    let through_events = qip_events::envelope::canonical_json(&value);

    assert_eq!(through_events, qip_core::canonical::canonical_json(&value));
    assert_eq!(through_events, r#"{"b":null,"outer":[{"a":2,"q\"k":1}]}"#);
}
