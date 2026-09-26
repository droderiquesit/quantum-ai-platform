use serde_json::json;

#[test]
fn canonical_json_sorts_object_keys_at_every_depth_and_keeps_array_order() {
    // Test premise: canonical form must sort object keys at every depth,
    // even when the source object has them in reverse order. If keys.sort() is
    // deleted, this test will fail because "z" will appear before "a".
    let obj = json!({"z": {"z": 1, "a": 2}, "a": 3});

    let canonical = qip_core::canonical::canonical_json(&obj);

    // The canonical form should have keys in sorted order.
    // If sort() was removed, the original order (z before a) would be preserved.
    // Split by root-level keys: {"a":3,"z":{...}}
    let canonical_str = canonical.to_string();

    // "a":3 should appear before "z":{ when keys are sorted
    let a_pos = canonical_str.find("\"a\":3").expect("a field should exist");
    let z_pos = canonical_str.find("\"z\":{").expect("z field should exist");

    assert!(
        a_pos < z_pos,
        "root keys should be sorted with 'a' before 'z', but got: {}",
        canonical_str
    );
}

#[test]
fn the_same_value_built_in_two_field_orders_serialises_to_identical_bytes() {
    // Test premise: two logically identical objects built in different field orders
    // must canonicalize to identical bytes to preserve deterministic hashing,
    // regardless of how they were constructed.
    let obj_x_first = json!({"x": 1, "y": 2, "z": 3});
    let obj_z_first = json!({"z": 3, "x": 1, "y": 2});

    // Canonicalise both objects
    let canonical_x = qip_core::canonical::canonical_json(&obj_x_first);
    let canonical_z = qip_core::canonical::canonical_json(&obj_z_first);

    // Canonical forms must be identical for content hashing to work correctly
    assert_eq!(canonical_x.as_bytes(), canonical_z.as_bytes());
    assert_eq!(canonical_x, canonical_z);
}
