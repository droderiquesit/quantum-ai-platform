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
