//! The shape of a payload, fingerprinted, and what changed between two
//! observations.
//!
//! Drift detection names fields rather than reporting a boolean because the
//! three kinds of change need three different responses. A field that vanished
//! breaks a parser loudly. A field that appeared is usually harmless. A field
//! that changed *type* is the dangerous one: it parses, it produces numbers,
//! and the numbers mean something else. `volume` arriving as a string of
//! thousands separators instead of an integer does not fail — it becomes a
//! smaller number, quietly, in every model downstream.
//!
//! That is why a retype quarantines a source rather than degrading it.

use qip_core::error::{Error, Result};
use qip_core::sha256_hex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The type of one leaf field.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldType {
    Null,
    Boolean,
    Integer,
    Number,
    Text,
    Array {
        element: Box<FieldType>,
    },
    /// A nested record reached inside an array, carrying how many fields it
    /// had. Objects at the top level are flattened into dotted paths instead;
    /// only the ones inside arrays land here, and the field count is what
    /// makes a record losing a field visible at all.
    Object {
        fields: usize,
    },
    /// An array whose elements disagree, or a field seen with two types in
    /// one payload. Recorded rather than collapsed, because a field that is
    /// sometimes a string is a field a parser will fail on eventually.
    Mixed,
    /// An empty array: the element type is unobservable from this payload.
    Unknown,
}

impl FieldType {
    pub fn name(&self) -> String {
        match self {
            Self::Null => "null".to_string(),
            Self::Boolean => "boolean".to_string(),
            Self::Integer => "integer".to_string(),
            Self::Number => "number".to_string(),
            Self::Text => "text".to_string(),
            Self::Array { element } => format!("array<{}>", element.name()),
            Self::Object { fields } => format!("object<{fields}>"),
            Self::Mixed => "mixed".to_string(),
            Self::Unknown => "unknown".to_string(),
        }
    }

    fn of(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(_) => Self::Boolean,
            Value::Number(number) => {
                if number.is_i64() || number.is_u64() {
                    Self::Integer
                } else {
                    Self::Number
                }
            }
            Value::String(_) => Self::Text,
            Value::Array(items) => {
                let mut element: Option<Self> = None;
                for item in items {
                    let observed = Self::of(item);
                    element = Some(match element {
                        None => observed,
                        Some(current) if current == observed => current,
                        Some(_) => Self::Mixed,
                    });
                }
                Self::Array {
                    element: Box::new(element.unwrap_or(Self::Unknown)),
                }
            }
            Value::Object(fields) => Self::Object {
                fields: fields.len(),
            },
        }
    }
}

/// The observed shape of a source's payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSchema {
    fields: BTreeMap<String, FieldType>,
    fingerprint: String,
}

impl SourceSchema {
    /// Derive a schema from a JSON payload.
    ///
    /// Nested objects are flattened to dotted paths so that a change three
    /// levels down is named at the field that changed rather than reported as
    /// "the root object differs". A payload that is an array of records takes
    /// the shape of its records, because that is what an adapter parses; an
    /// empty array yields an empty schema, which is itself a finding worth
    /// seeing rather than an error.
    pub fn from_json(payload: &Value) -> Self {
        let mut fields = BTreeMap::new();
        match payload {
            Value::Array(items) => {
                for item in items {
                    flatten("", item, &mut fields);
                }
            }
            other => flatten("", other, &mut fields),
        }
        let fingerprint = fingerprint_of(&fields);
        Self {
            fields,
            fingerprint,
        }
    }

    /// Derive a schema from a payload body.
    pub fn parse(body: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(body).map_err(|error| {
            Error::schema(format!(
                "the sampled payload is not JSON and cannot be fingerprinted: {error}"
            ))
        })?;
        Ok(Self::from_json(&value))
    }

    /// Build a schema directly, for a source whose shape is declared rather
    /// than sampled.
    pub fn from_fields(fields: impl IntoIterator<Item = (String, FieldType)>) -> Self {
        let fields: BTreeMap<String, FieldType> = fields.into_iter().collect();
        let fingerprint = fingerprint_of(&fields);
        Self {
            fields,
            fingerprint,
        }
    }

    pub fn fields(&self) -> &BTreeMap<String, FieldType> {
        &self.fields
    }

    /// A digest over the field names and types, stable across payloads with
    /// the same shape and different values.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// What changed between this schema and a later observation.
    pub fn drift_to(&self, observed: &Self) -> SchemaDrift {
        let appeared: Vec<String> = observed
            .fields
            .keys()
            .filter(|field| !self.fields.contains_key(*field))
            .cloned()
            .collect();
        let vanished: Vec<String> = self
            .fields
            .keys()
            .filter(|field| !observed.fields.contains_key(*field))
            .cloned()
            .collect();
        let retyped: Vec<FieldRetype> = self
            .fields
            .iter()
            .filter_map(|(field, was)| {
                observed.fields.get(field).and_then(|now| {
                    (was != now).then(|| FieldRetype {
                        field: field.clone(),
                        was: was.clone(),
                        now: now.clone(),
                    })
                })
            })
            .collect();
        SchemaDrift {
            appeared,
            vanished,
            retyped,
            was_fingerprint: self.fingerprint.clone(),
            now_fingerprint: observed.fingerprint.clone(),
        }
    }
}

/// One field whose type changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldRetype {
    pub field: String,
    pub was: FieldType,
    pub now: FieldType,
}

impl FieldRetype {
    pub fn describe(&self) -> String {
        format!(
            "`{}` changed from {} to {}",
            self.field,
            self.was.name(),
            self.now.name()
        )
    }
}

/// How severely a source's shape moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftSeverity {
    /// Nothing changed.
    Stable,
    /// Only new fields. Nothing downstream reads them yet.
    Additive,
    /// Fields vanished. Parsers fail, loudly, at the boundary.
    Breaking,
    /// A field changed type. Parsers succeed and mean something else.
    Silent,
}

impl DriftSeverity {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Additive => "additive",
            Self::Breaking => "breaking",
            Self::Silent => "silent",
        }
    }

    /// Whether a source in this state must stop being consumed.
    ///
    /// A vanished field and a retyped field both quarantine. The severity
    /// ordering is about which is worse to *discover late*, not about which
    /// is safe to ignore.
    pub const fn requires_quarantine(&self) -> bool {
        matches!(self, Self::Breaking | Self::Silent)
    }
}

/// Exactly what moved between two observations of a source's shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaDrift {
    appeared: Vec<String>,
    vanished: Vec<String>,
    retyped: Vec<FieldRetype>,
    was_fingerprint: String,
    now_fingerprint: String,
}

impl SchemaDrift {
    /// Fields present now and not before.
    pub fn appeared(&self) -> &[String] {
        &self.appeared
    }

    /// Fields present before and not now.
    pub fn vanished(&self) -> &[String] {
        &self.vanished
    }

    /// Fields present in both, with a different type.
    pub fn retyped(&self) -> &[FieldRetype] {
        &self.retyped
    }

    pub fn was_fingerprint(&self) -> &str {
        &self.was_fingerprint
    }

    pub fn now_fingerprint(&self) -> &str {
        &self.now_fingerprint
    }

    pub fn is_stable(&self) -> bool {
        self.appeared.is_empty() && self.vanished.is_empty() && self.retyped.is_empty()
    }

    pub fn severity(&self) -> DriftSeverity {
        if !self.retyped.is_empty() {
            DriftSeverity::Silent
        } else if !self.vanished.is_empty() {
            DriftSeverity::Breaking
        } else if !self.appeared.is_empty() {
            DriftSeverity::Additive
        } else {
            DriftSeverity::Stable
        }
    }

    /// The change in words, naming every field.
    pub fn describe(&self) -> String {
        if self.is_stable() {
            return "the schema is unchanged".to_string();
        }
        let mut parts = Vec::new();
        if !self.retyped.is_empty() {
            parts.push(format!(
                "retyped: {}",
                self.retyped
                    .iter()
                    .map(FieldRetype::describe)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.vanished.is_empty() {
            parts.push(format!("vanished: {}", self.vanished.join(", ")));
        }
        if !self.appeared.is_empty() {
            parts.push(format!("appeared: {}", self.appeared.join(", ")));
        }
        parts.join("; ")
    }
}

/// Flatten a JSON value into dotted leaf paths.
///
/// A field observed twice with different types within one payload — common in
/// an array of heterogeneous records — becomes [`FieldType::Mixed`] rather
/// than whichever record was parsed last.
fn flatten(prefix: &str, value: &Value, into: &mut BTreeMap<String, FieldType>) {
    match value {
        Value::Object(fields) if !fields.is_empty() => {
            for (key, child) in fields {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&path, child, into);
            }
        }
        other => {
            if prefix.is_empty() {
                // A scalar payload has no field name; record it under a
                // reserved path so the fingerprint still moves when the
                // scalar's type does.
                record(into, "$".to_string(), FieldType::of(other));
                return;
            }
            record(into, prefix.to_string(), FieldType::of(other));
        }
    }
}

fn record(into: &mut BTreeMap<String, FieldType>, path: String, observed: FieldType) {
    match into.get(&path) {
        Some(existing) if *existing == observed => {}
        Some(_) => {
            into.insert(path, FieldType::Mixed);
        }
        None => {
            into.insert(path, observed);
        }
    }
}

fn fingerprint_of(fields: &BTreeMap<String, FieldType>) -> String {
    let mut rendered = String::new();
    for (field, kind) in fields {
        rendered.push_str(field);
        rendered.push(':');
        rendered.push_str(&kind.name());
        rendered.push('\n');
    }
    sha256_hex(rendered.as_bytes())
}
