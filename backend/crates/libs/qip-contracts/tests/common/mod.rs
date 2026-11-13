//! Shared checks for the `Validated` contracts: round trip, missing field,
//! unknown field, foreign version, and "refused on construction *and* on
//! decode" for every broken invariant.
//!
//! The decode half is the point. A contract whose constructor refuses and
//! whose `Deserialize` does not passes every constructor test ever written
//! and admits the same bad value from a stream; `refused` asserts both.

#![allow(dead_code, clippy::panic, clippy::expect_used)]

use qip_contracts::validated::{Contract, Validated};
use qip_core::error::Result;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

pub(crate) const DIGEST_A: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(crate) const DIGEST_B: &str =
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// Admits `fields`, encodes, decodes, and asserts the value and the bytes
/// both survive — a round trip that changed a byte would make a replayed
/// record hash differently from the original.
pub(crate) fn round_trips<T>(fields: T) -> Result<()>
where
    T: Contract + Serialize + DeserializeOwned + Clone + PartialEq + Debug,
{
    let admitted = Validated::new(fields.clone())?;
    let bytes = admitted.encode()?;
    let back = Validated::<T>::decode(&bytes)?;
    assert_eq!(back, admitted, "{} changed in a round trip", T::NAME);
    assert_eq!(
        back.encode()?,
        bytes,
        "{} re-encoded to different bytes",
        T::NAME
    );
    assert_eq!(back.into_fields(), fields);
    Ok(())
}

/// Every top-level field removed in turn is refused on decode, an unknown
/// field is refused, and a foreign `schema_version` is refused — each
/// against a premise that the intact record decodes.
pub(crate) fn refuses_missing_unknown_and_foreign_version<T>(fields: T)
where
    T: Contract + Serialize + DeserializeOwned + Clone + PartialEq + Debug,
{
    let value = serde_json::to_value(&fields).expect("fixture serialises");
    let object = value
        .as_object()
        .expect("a contract is a JSON object")
        .clone();
    assert!(
        serde_json::from_value::<Validated<T>>(value.clone()).is_ok(),
        "premise: the intact {} decodes",
        T::NAME
    );
    assert!(
        object.len() > 1,
        "premise: {} has fields to remove",
        T::NAME
    );
    for key in object.keys() {
        let mut missing = object.clone();
        missing.remove(key);
        let decoded = Validated::<T>::decode(
            &serde_json::to_vec(&serde_json::Value::Object(missing)).expect("encodes"),
        );
        let refusal = decoded.expect_err(&format!("{} without `{key}` was admitted", T::NAME));
        assert_eq!(refusal.code(), "schema", "{} without `{key}`", T::NAME);
        // The code alone cannot tell a missing field from an invariant: a
        // field given `#[serde(default)]` decodes to its default, and when
        // that default breaks an invariant `decode` still answers `schema`.
        // So a defaulted field passed this check as long as its default was
        // invalid, and the property named here is that the producer must
        // state the field, not that its default happens to be refused.
        assert!(
            refusal
                .to_string()
                .contains(&format!("missing field `{key}`")),
            "{} without `{key}` was refused for another reason, so the field is defaulted: {refusal}",
            T::NAME
        );
    }
    let mut extra = object.clone();
    extra.insert("smuggled".to_string(), serde_json::Value::Bool(true));
    assert!(
        serde_json::from_value::<Validated<T>>(serde_json::Value::Object(extra)).is_err(),
        "{} admitted an unknown field",
        T::NAME
    );
    let mut foreign = object;
    foreign.insert(
        "schema_version".to_string(),
        serde_json::Value::from(T::SCHEMA_VERSION + 1),
    );
    assert!(
        serde_json::from_value::<Validated<T>>(serde_json::Value::Object(foreign)).is_err(),
        "{} admitted a foreign schema version",
        T::NAME
    );
}

/// Every key of the nested object at `pointer` (a JSON pointer such as
/// `/trace/0`), removed in turn, is refused on decode with serde's own
/// `missing field` refusal, and an unknown key there is refused too.
///
/// The top-level check above cannot see inside a list or a sub-record: a
/// trace step or an envelope that gained `#[serde(default)]`, or lost
/// `deny_unknown_fields`, decoded as before and passed it. Each nested
/// record the contract carries is a record a producer must state in full.
pub(crate) fn refuses_missing_and_unknown_nested<T>(fields: T, pointer: &str)
where
    T: Contract + Serialize + DeserializeOwned + Clone + PartialEq + Debug,
{
    let value = serde_json::to_value(&fields).expect("fixture serialises");
    let nested = value
        .pointer(pointer)
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("premise: {} has an object at {pointer}", T::NAME))
        .clone();
    assert!(
        serde_json::from_value::<Validated<T>>(value.clone()).is_ok(),
        "premise: the intact {} decodes",
        T::NAME
    );
    assert!(
        !nested.is_empty(),
        "premise: {} has keys at {pointer} to remove",
        T::NAME
    );
    let decode = |object: serde_json::Map<String, serde_json::Value>| {
        let mut whole = value.clone();
        *whole.pointer_mut(pointer).expect("the pointer resolves") =
            serde_json::Value::Object(object);
        Validated::<T>::decode(&serde_json::to_vec(&whole).expect("encodes"))
    };
    for key in nested.keys() {
        let mut missing = nested.clone();
        missing.remove(key);
        let refusal = decode(missing).expect_err(&format!(
            "{} without `{pointer}/{key}` was admitted",
            T::NAME
        ));
        assert!(
            refusal
                .to_string()
                .contains(&format!("missing field `{key}`")),
            "{} without `{pointer}/{key}` was refused for another reason, so the field is \
             defaulted: {refusal}",
            T::NAME
        );
    }
    let mut extra = nested;
    extra.insert("smuggled".to_string(), serde_json::Value::Bool(true));
    assert!(
        decode(extra).is_err(),
        "{} admitted an unknown field at {pointer}",
        T::NAME
    );
}

/// `fields` is refused by `Validated::new` *and* by decode of its JSON.
pub(crate) fn refused<T>(label: &str, fields: T)
where
    T: Contract + Serialize + DeserializeOwned + Clone + Debug,
{
    let bytes = serde_json::to_vec(&fields).expect("a broken fixture still serialises");
    assert!(
        Validated::new(fields).is_err(),
        "{}: `{label}` was admitted by new",
        T::NAME
    );
    assert!(
        Validated::<T>::decode(&bytes).is_err(),
        "{}: `{label}` was admitted by decode",
        T::NAME
    );
}

/// One labelled way to break a valid record.
pub(crate) type Breakage<'a, T> = (&'a str, Box<dyn Fn(&mut T)>);

/// Applies each mutation to a fresh copy of `valid` and asserts each is
/// refused, after asserting `valid` itself is admitted.
pub(crate) fn each_refused<T>(valid: &T, cases: Vec<Breakage<'_, T>>)
where
    T: Contract + Serialize + DeserializeOwned + Clone + Debug,
{
    assert!(
        Validated::new(valid.clone()).is_ok(),
        "premise: the valid {} is admitted",
        T::NAME
    );
    assert!(!cases.is_empty(), "premise: there are invariants to break");
    for (label, mutate) in cases {
        let mut broken = valid.clone();
        mutate(&mut broken);
        refused(label, broken);
    }
}
