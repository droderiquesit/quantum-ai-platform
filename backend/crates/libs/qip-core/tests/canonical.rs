//! The bytes `canonical_json` produces are an identity: every content hash in
//! the platform is taken over them, so a change here silently re-keys every
//! stored digest. These tests pin the bytes rather than properties of them.
//!
//! The first version of this file asserted that keys come out sorted, and
//! passed with the sort deleted — in this build `serde_json::Map` already
//! iterates in key order, so no input could tell the difference. A test that
//! survives the deletion of the thing it names guards nothing. What follows
//! pins what *can* change, and a sentinel for the day the sort starts to
//! matter.

use qip_core::canonical::canonical_json;
use serde_json::json;

#[test]
fn a_key_that_needs_escaping_is_written_as_a_json_string() {
    // A key formatted without escaping would emit `{"a"b":1}` — invalid JSON,
    // and a digest over bytes no reader can parse back to the value hashed.
    let value = json!({"a\"b": 1, "line\nbreak": 2});
    let canonical = canonical_json(&value);

    assert_eq!(canonical, r#"{"a\"b":1,"line\nbreak":2}"#);
    let reparsed: serde_json::Value = serde_json::from_str(&canonical).unwrap();
    assert_eq!(reparsed, value);
}

#[test]
fn array_order_is_kept_and_every_nested_value_is_written_compactly() {
    // Arrays are sequences, not sets: reordering one changes the value, so
    // canonicalising must not sort them. Separators are part of the identity
    // too; one extra space re-keys every digest.
    let value = json!({"b": {"c": "d"}, "a": [3, 1, {"y": [true, null], "x": 1.5}]});

    assert_eq!(
        canonical_json(&value),
        r#"{"a":[3,1,{"x":1.5,"y":[true,null]}],"b":{"c":"d"}}"#
    );
}

#[test]
fn serde_json_map_does_not_preserve_insertion_order_in_this_build() {
    // Sentinel, not a test of canonical_json. While this holds, the explicit
    // key sort in canonical_json changes no output and no test can kill its
    // deletion. If this fails, serde_json's `preserve_order` feature has been
    // switched on somewhere in the build graph (cargo unifies features): the
    // sort is now load-bearing. Add a test that builds an object in reverse
    // key order and pins sorted bytes, prove it fails with the sort deleted,
    // and only then adjust this sentinel.
    let mut map = serde_json::Map::new();
    map.insert("z".to_owned(), json!(1));
    map.insert("a".to_owned(), json!(2));

    let keys: Vec<&str> = map.keys().map(String::as_str).collect();
    assert_eq!(keys, ["a", "z"]);
}
