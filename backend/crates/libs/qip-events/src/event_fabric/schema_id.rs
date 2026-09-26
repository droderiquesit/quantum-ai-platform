//! Content-derived schema identifiers, and the gate that refuses a breaking
//! change to one without a version bump.
//!
//! See ADR 0100 §1 for the event fabric's architecture. `prost` is
//! BLOCKED(C2) (ADR 0099), so there is no wire-format compiler to derive an
//! identifier from a `.proto` file's message descriptor. [`Shape`] is the
//! in-tree substitute: it is read off a live Rust sample the same way
//! [`crate::registry::SchemaRegistry::register`] already reads a type's
//! top-level field names (`registry.rs:44`), but it recurses into every
//! nested value instead of stopping at the top level. That inverts
//! FABRIC-025's usual direction — normally the wire type is generated from a
//! schema, not the other way round — and this file is that inversion,
//! recorded as a deviation pending C2 rather than claimed as codegen.
//!
//! # Why the registry's own fingerprint is not enough
//!
//! `SchemaRegistry`'s existing fingerprint (`registry.rs:44`) hashes
//! `topic|version|sorted-top-level-field-names`. Two payloads whose top-level
//! field names are identical hash identically even when a field nested two
//! levels down has silently changed kind — an order reference that used to
//! carry a numeric id and now carries a string one, say. A stream admitting
//! producers by schema id on that fingerprint would accept the retyped
//! payload as if nothing had changed, and every consumer that reads the
//! nested field as a number would misparse it. [`SchemaId`] closes that by
//! hashing the full recursive [`Shape`], not just the field names one level
//! down.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The typed structure of a value, with every literal stripped out.
///
/// `Shape` answers "what kind is this, all the way down", never "what value
/// is this". Two samples of the same Rust type produce the same `Shape` even
/// when their field values differ, and that is the property [`SchemaId`]
/// relies on: an id that changed with the *data* rather than the *shape*
/// would be a new id on every publish, which is useless as a compatibility
/// key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shape {
    Null,
    Bool,
    Number,
    String,
    /// Boxed because a value can nest arbitrarily deep and `Shape` is
    /// otherwise a fixed-size enum.
    Array(Box<Shape>),
    /// `BTreeMap` rather than `HashMap` so that two structurally identical
    /// objects always canonicalise to the same string regardless of the
    /// field declaration order in the source struct — iteration order here
    /// reaches [`SchemaId::new`]'s hash input, and a hash keyed on
    /// declaration order would not be a hash of the shape.
    Object(BTreeMap<String, Shape>),
}

impl Shape {
    /// Derive a shape from a live sample by serialising it, since there is no
    /// reflection to interrogate a Rust type directly — the same constraint
    /// `SchemaRegistry::register` works under.
    pub fn of<T: Serialize>(sample: &T) -> Result<Self> {
        let value = serde_json::to_value(sample)?;
        Ok(Self::from_json(&value))
    }

    /// Derive a shape from an already-serialised value, so a caller holding
    /// one (the registry, mid-`register`) does not pay for a second
    /// serialisation of the same sample.
    pub fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Shape::Null,
            serde_json::Value::Bool(_) => Shape::Bool,
            serde_json::Value::Number(_) => Shape::Number,
            serde_json::Value::String(_) => Shape::String,
            serde_json::Value::Array(items) => {
                // An empty sample array names no element type. `Null` stands
                // in for "unknown" here rather than a fabricated guess, and a
                // non-empty sample is what a caller should register with.
                let element = items.first().map(Shape::from_json).unwrap_or(Shape::Null);
                Shape::Array(Box::new(element))
            }
            serde_json::Value::Object(map) => Shape::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), Shape::from_json(v)))
                    .collect(),
            ),
        }
    }

    /// A deterministic string over the whole shape tree, used as the
    /// [`SchemaId`] hash's material. Private because the string's format is
    /// not a contract — only that it is a pure, deterministic function of the
    /// shape.
    fn canonical(&self) -> String {
        match self {
            Shape::Null => "null".to_string(),
            Shape::Bool => "bool".to_string(),
            Shape::Number => "number".to_string(),
            Shape::String => "string".to_string(),
            Shape::Array(element) => format!("[{}]", element.canonical()),
            Shape::Object(fields) => {
                // `fields` is a `BTreeMap`, so this iterates in key order
                // regardless of the struct's declared field order.
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(name, shape)| format!("{name}:{}", shape.canonical()))
                    .collect();
                format!("{{{}}}", parts.join(","))
            }
        }
    }
}

/// A content-derived schema identifier: SHA-256 over the topic, the schema
/// version and the full recursive [`Shape`].
///
/// Identical content — same topic, same version, same shape, regardless of
/// which sample produced it — always yields the same id, and any change to
/// any of the three yields a different one. That is what lets a stream admit
/// a producer "by id": two producers presenting the same id are provably
/// interchangeable, without either having to trust the other's version claim
/// alone (a version number a publisher forgot to bump is just a string).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SchemaId(String);

impl SchemaId {
    pub fn new(topic: &str, version: u32, shape: &Shape) -> Self {
        let material = format!("{topic}|{version}|{}", shape.canonical());
        Self(qip_core::hash::sha256_hex(material.as_bytes()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SchemaId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Refuse a schema evolution that removes, renames or retypes a field
/// without a version bump; admit one that only adds a field.
///
/// A field that vanishes between `old` and `new` — whether dropped outright
/// or renamed to something else — is refused identically: a structural check
/// cannot tell a rename from a remove-and-unrelated-add, because both leave
/// the old name absent from the new shape, and a reader built for the old
/// shape fails the same way either way. A field that changes kind in place
/// is refused for the same reason a reader built for the old kind would
/// misparse the new one. A field that is only *added* is admitted without a
/// bump: an old reader that never looked for it does not fail by its
/// presence, which is the same asymmetry `qip-streaming`'s envelope schema
/// documents at `ENVELOPE_SCHEMA_VERSION` (tolerant of an unknown field,
/// strict about a known one changing shape).
///
/// A version bump (`new_version > old_version`) is the sanctioned escape
/// hatch: it is the producer saying "this is deliberately incompatible",
/// and every consumer is expected to have decided how it handles that
/// version before subscribing to it.
pub fn check_compatible(
    old_version: u32,
    old: &Shape,
    new_version: u32,
    new: &Shape,
) -> Result<()> {
    if new_version > old_version {
        return Ok(());
    }
    compatible_shape(old, new)
}

fn compatible_shape(old: &Shape, new: &Shape) -> Result<()> {
    match (old, new) {
        (Shape::Object(old_fields), Shape::Object(new_fields)) => {
            for (name, old_field) in old_fields {
                let new_field = new_fields.get(name).ok_or_else(|| {
                    Error::schema(format!(
                        "field '{name}' is absent from the new shape without a version \
                         bump; bump the schema version if it was removed or renamed"
                    ))
                })?;
                compatible_shape(old_field, new_field)?;
            }
            Ok(())
        }
        (Shape::Array(old_element), Shape::Array(new_element)) => {
            compatible_shape(old_element, new_element)
        }
        _ if old == new => Ok(()),
        _ => Err(Error::schema(
            "a field changed kind without a version bump; bump the schema \
             version if the retype was deliberate"
                .to_string(),
        )),
    }
}
