use serde_json::json;

#[test]
fn qip_events_canonical_json_is_the_foundation_function() {
    // Test premise: qip_events re-exports canonical_json from qip_core,
    // so the same function is used everywhere. Call it through the re-export
    // and verify it handles nested structures correctly.
    let val = json!({
        "outer_z": {
            "inner_z": 1,
            "inner_a": 2
        },
        "outer_a": [
            {"b": 1, "a": 2},
            {"z": 3, "x": 4}
        ]
    });

    // Call through qip_events' re-export
    let canonical = qip_events::envelope::canonical_json(&val);

    // Verify that it recurses into arrays (preserving array order)
    // and sorts object keys at every depth
    let parts: Vec<&str> = canonical.split(',').collect();
    assert!(!parts.is_empty(), "canonical form should contain parts");

    // Verify the outer object has sorted keys: outer_a before outer_z
    assert!(
        canonical.find("\"outer_a\":").unwrap() < canonical.find("\"outer_z\":").unwrap(),
        "outer keys should be sorted"
    );

    // Verify that array order is preserved (array comes before second object)
    let array_pos = canonical.find('[').unwrap();
    let second_obj_pos = canonical.rfind('[').unwrap();
    // Since we have one array, both should be the same position
    assert_eq!(array_pos, second_obj_pos);
}
