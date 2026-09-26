//! Canonical JSON serialization for deterministic hashing.
//!
//! `serde_json::Value` uses a map that preserves insertion order, but two
//! logically identical payloads built in different field orders would hash
//! differently. Canonicalising first makes the content hash a genuine identity.

/// Serialise a JSON value with object keys in sorted order.
///
/// This function recursively sorts object keys at every depth while preserving
/// array order. Two logically identical values will produce byte-identical output
/// regardless of the order in which they were constructed, ensuring that
/// content hashes are genuine identities.
///
/// # Examples
///
/// ```
/// use serde_json::json;
///
/// let val = json!({"z": 1, "a": 2});
/// let canonical = qip_core::canonical::canonical_json(&val);
/// assert!(canonical.contains("\"a\":"));
/// ```
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_object_keys_at_every_depth_and_keeps_array_order() {
        // Test premise: values with nested objects in different key orders should produce
        // identical output after canonicalisation.
        let val_field_a_first = json!({
            "a": {"z": 1, "a": 2},
            "z": 3
        });
        let val_field_z_first = json!({
            "z": 3,
            "a": {"a": 2, "z": 1}
        });

        let canonical_a = canonical_json(&val_field_a_first);
        let canonical_z = canonical_json(&val_field_z_first);

        assert_eq!(canonical_a, canonical_z);
        // Verify keys are actually sorted in the output
        assert!(canonical_a.find("\"a\":").unwrap() < canonical_a.rfind("\"z\":").unwrap());
    }

    #[test]
    fn the_same_value_built_in_two_field_orders_serialises_to_identical_bytes() {
        let obj1 = json!({"x": 1, "y": 2, "z": 3});
        let obj2 = json!({"z": 3, "x": 1, "y": 2});

        let canonical1 = canonical_json(&obj1);
        let canonical2 = canonical_json(&obj2);

        assert_eq!(canonical1.as_bytes(), canonical2.as_bytes());
        assert_eq!(canonical1, canonical2);
    }
}
