//! Canonical JSON: the one serialisation every content hash in the platform
//! is taken over.
//!
//! It lives in the foundation crate because `qip-contracts` must hash
//! canonical JSON (the reflex chain digest) and may depend on nothing but
//! `qip-core`; a second copy there would be two definitions of one identity,
//! free to drift apart one edit at a time.
//!
//! **Why sort explicitly when the map already sorts.** In this build
//! `serde_json::Map` is a `BTreeMap`, so its keys already iterate in order and
//! the sort below changes no output. That is a property of the build graph,
//! not of this crate: `serde_json`'s `preserve_order` feature swaps the map
//! for an insertion-ordered one, and cargo unifies features, so any crate
//! anywhere in the graph could turn it on. Every stored digest would then
//! depend on the order a caller happened to build its fields in. The explicit
//! sort keeps the identity independent of that switch;
//! `qip-core/tests/canonical.rs` carries a sentinel that fails the moment the
//! switch flips, because that is when the sort starts carrying weight and
//! needs a test that can kill it.

/// Serialise a JSON value with object keys sorted at every depth and array
/// order kept, so two logically identical values give identical bytes.
pub fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::Value::String((*k).clone()),
                        canonical_json(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}
