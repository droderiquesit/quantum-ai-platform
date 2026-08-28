//! Schema validation and the version check.
//!
//! A source changes its payload without telling anybody. The two ways that
//! happens have different answers:
//!
//! * It **adds** a field. Not a fault; the feed must not stop. Ignored by
//!   default ([`super::manifest::UnknownFieldPolicy::Ignore`]).
//! * It **removes or retypes** a field this connector reads. The decoder would
//!   either fail or, worse, succeed against the wrong field. Refused here, at
//!   the boundary, and the payload goes to quarantine with the path named.
//!
//! # Why a type check and not just a presence check
//!
//! A price that arrives as `101.75` instead of `"101.75"` is present and has
//! already lost precision, because JSON's number is a double. Checking only
//! that `price` exists would admit it. [`super::manifest::FieldKind`] tells
//! the two apart, which is the point of having the kind in the manifest.
//!
//! # Why the version check is separate
//!
//! A payload can satisfy every field rule and still be a major version this
//! connector was not written for — the fields kept their names and changed
//! their meaning. Only the source can say that, so the check is on the
//! declared version and it is a refusal, not a warning.

use super::manifest::{FieldKind, FieldSpec, SchemaContract, SchemaVersion, UnknownFieldPolicy};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One way a payload failed its contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaViolation {
    /// The dotted path, so an operator can find it in the payload rather than
    /// re-deriving which field "a missing field" meant.
    pub path: String,
    pub detail: String,
}

impl SchemaViolation {
    pub fn new(path: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            detail: detail.into(),
        }
    }
}

/// Whether a payload may be decoded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaOutcome {
    Conforms,
    Violates(Vec<SchemaViolation>),
}

impl SchemaOutcome {
    pub const fn conforms(&self) -> bool {
        matches!(self, Self::Conforms)
    }

    pub fn violations(&self) -> &[SchemaViolation] {
        match self {
            Self::Conforms => &[],
            Self::Violates(violations) => violations,
        }
    }

    /// The violations as one line, for a quarantine record.
    pub fn describe(&self) -> String {
        self.violations()
            .iter()
            .map(|violation| format!("{}: {}", violation.path, violation.detail))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// A contract, ready to check payloads against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaGuard {
    contract: SchemaContract,
    source_id: String,
}

impl SchemaGuard {
    pub fn new(source_id: impl Into<String>, contract: SchemaContract) -> Self {
        Self {
            contract,
            source_id: source_id.into(),
        }
    }

    pub const fn contract(&self) -> &SchemaContract {
        &self.contract
    }

    pub const fn version(&self) -> SchemaVersion {
        self.contract.version
    }

    /// Whether a payload declaring `observed` may be decoded.
    ///
    /// A source that declares nothing is admitted: most public endpoints
    /// version their URL rather than their body, and refusing every payload
    /// without a version field would refuse every source that has one URL per
    /// version. What is refused is a *stated* incompatible version, which is
    /// the source telling us plainly.
    pub fn admit_version(&self, observed: Option<SchemaVersion>) -> Result<()> {
        let Some(observed) = observed else {
            return Ok(());
        };
        if self.contract.version.admits(observed) {
            return Ok(());
        }
        Err(Error::schema(format!(
            "`{}` is serving schema {observed} and this connector reads {}. A major version \
             change keeps the field names and changes what they mean, so the payload is refused \
             rather than decoded into records that would look ordinary and be wrong",
            self.source_id, self.contract.version
        )))
    }

    /// Check one payload against every required field.
    pub fn check(&self, payload: &Value) -> SchemaOutcome {
        let mut violations = Vec::new();
        for field in &self.contract.required_fields {
            match resolve(payload, &field.path) {
                None => violations.push(SchemaViolation::new(
                    &field.path,
                    format!(
                        "required and absent; this connector decodes it, so a payload without it \
                         cannot become a record ({} expected)",
                        field.kind.as_str()
                    ),
                )),
                Some(value) => {
                    if let Some(detail) = mismatch(field, value) {
                        violations.push(SchemaViolation::new(&field.path, detail));
                    }
                }
            }
        }
        if matches!(self.contract.unknown_fields, UnknownFieldPolicy::Quarantine) {
            violations.extend(self.unknown_top_level(payload));
        }
        if violations.is_empty() {
            SchemaOutcome::Conforms
        } else {
            SchemaOutcome::Violates(violations)
        }
    }

    /// Top-level keys the contract does not name, for the sources whose
    /// additions have historically been breaking.
    fn unknown_top_level(&self, payload: &Value) -> Vec<SchemaViolation> {
        let Some(object) = payload.as_object() else {
            return Vec::new();
        };
        let named: Vec<&str> = self
            .contract
            .required_fields
            .iter()
            .map(|field| field.path.split('.').next().unwrap_or(field.path.as_str()))
            .collect();
        object
            .keys()
            .filter(|key| !named.contains(&key.as_str()))
            .map(|key| {
                SchemaViolation::new(
                    key,
                    "not named by the contract, and this source's contract quarantines additions",
                )
            })
            .collect()
    }
}

/// Follow a dotted path, where an integer segment indexes an array.
///
/// Returns `None` for a missing path *and* for an explicit JSON `null`: a
/// field present and null is a field this connector cannot decode, and telling
/// the two apart would only let a null through a presence check.
fn resolve<'a>(payload: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = payload;
    for segment in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    if current.is_null() {
        None
    } else {
        Some(current)
    }
}

/// How `value` fails to be `field.kind`, or `None` when it does not.
fn mismatch(field: &FieldSpec, value: &Value) -> Option<String> {
    let actual = describe(value);
    match field.kind {
        FieldKind::String => value.is_string().then_some(()).map_or(
            Some(format!("expected a string and found {actual}")),
            |()| None,
        ),
        FieldKind::Number => value.is_number().then_some(()).map_or(
            Some(format!("expected a number and found {actual}")),
            |()| None,
        ),
        FieldKind::DecimalString => match value.as_str() {
            None => Some(format!(
                "expected an exact number written as a string and found {actual}. A price sent \
                 as a JSON number has already lost precision by the time this code sees it"
            )),
            Some(text) if Decimal::parse(text).is_none() => Some(format!(
                "the string {text:?} is not an exact decimal this platform can hold"
            )),
            Some(_) => None,
        },
        FieldKind::Timestamp => match value.as_str() {
            None => Some(format!("expected an RFC 3339 instant and found {actual}")),
            Some(text) if Timestamp::parse_rfc3339(text).is_none() => {
                Some(format!("the string {text:?} is not an RFC 3339 instant"))
            }
            Some(_) => None,
        },
        FieldKind::Bool => value
            .is_boolean()
            .then_some(())
            .map_or(Some(format!("expected a bool and found {actual}")), |()| {
                None
            }),
        FieldKind::Object => value.is_object().then_some(()).map_or(
            Some(format!("expected an object and found {actual}")),
            |()| None,
        ),
        FieldKind::Array => value.is_array().then_some(()).map_or(
            Some(format!("expected an array and found {actual}")),
            |()| None,
        ),
    }
}

fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a bool",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}
