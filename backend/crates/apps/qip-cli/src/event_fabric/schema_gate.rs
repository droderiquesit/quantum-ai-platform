//! `qip event-fabric schema-gate`: the CI compatibility gate (CICD-070).
//!
//! `schemas.lock.json` proves a bound body still has the shape its row
//! records. It cannot prove the commit that *changed* a row was allowed to:
//! the lock suite compares the tree to itself, so a row regenerated in place
//! after a field was removed passes it, at the same version every consumer
//! already trusts. This command compares two locks instead — the base
//! branch's and the head's — and judges each topic with the one rule the
//! broker and the registry already use,
//! [`qip_events::event_fabric::schema_id::check_compatible`]: a field added
//! at the same version passes, a field removed, renamed or retyped at the
//! same version does not, and a version bump is the producer's deliberate
//! break (ADR 0100 §5).
//!
//! It reads two files and decides. It fetches nothing and runs no `git`:
//! which commit is the base is the pipeline's fact, and `ci.yml` hands the
//! base lock in as a file.
//!
//! Exit codes follow `qip replay`: [`COMPATIBLE`] is 0, [`INCOMPATIBLE`] is
//! 3, and a lock that cannot be read or parsed is the family's refusal, 1 —
//! "I could not look" must not read as either verdict.

use qip_core::error::{Error, Result};
use qip_events::event_fabric::schema_id::{SchemaId, Shape, check_compatible};

use super::{Environment, Outcome};

/// The subcommand's name under `qip event-fabric`.
pub const SUBCOMMAND: &str = "schema-gate";

/// Every base topic is still readable by a consumer built against the base.
pub const COMPATIBLE: u8 = 0;

/// At least one base topic is not. Three, as `qip replay`'s DIFFERS is.
pub const INCOMPATIBLE: u8 = 3;

/// One row of `schemas.lock.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockRow {
    pub topic: String,
    pub version: u32,
    pub type_name: String,
    pub schema_id: String,
    /// The recursive shape `schema_id` hashes. `None` only for a row written
    /// before the lock carried shapes; such a row can be compared by id and
    /// by nothing else.
    pub shape: Option<Shape>,
}

/// Parse a lock file's text. `origin` names the file in a refusal.
pub fn parse(origin: &str, text: &str) -> Result<Vec<LockRow>> {
    let refuse = |what: String| {
        Error::invalid(format!(
            "{origin} is not a schema lock: {what}. Regenerate it from the \
             event_fabric_schema_lock suite's output; do not hand-edit it"
        ))
    };
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| refuse(e.to_string()))?;
    let rows = value
        .as_array()
        .ok_or_else(|| refuse("the top level is not an array".to_string()))?;

    let mut parsed = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let text_field = |name: &str| {
            row.get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| refuse(format!("row {index} has no string '{name}'")))
        };
        let version = row
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| refuse(format!("row {index} has no 32-bit 'version'")))?;
        let shape = match row.get("shape") {
            None => None,
            Some(shape) => Some(
                serde_json::from_value::<Shape>(shape.clone())
                    .map_err(|e| refuse(format!("row {index}'s shape does not parse: {e}")))?,
            ),
        };
        let row = LockRow {
            topic: text_field("topic")?,
            version,
            type_name: text_field("type_name")?,
            schema_id: text_field("schema_id")?,
            shape,
        };
        // A shape that does not hash to its row's id is a row somebody
        // edited by hand, and the comparison below would be judging a shape
        // nothing ever published under that id.
        if let Some(shape) = &row.shape {
            let hashed = SchemaId::new(&row.topic, row.version, shape);
            if hashed.as_str() != row.schema_id {
                return Err(refuse(format!(
                    "row {index} ({} version {}) carries a shape that does not hash to its \
                     schema_id",
                    row.topic, row.version
                )));
            }
        }
        parsed.push(row);
    }
    Ok(parsed)
}

/// What the gate found for the whole lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// One line per base topic that is no longer readable. Empty is the pass.
    pub findings: Vec<String>,
    /// One line per base topic that changed and is still readable.
    pub admitted: Vec<String>,
    /// How many base topics were compared — reported so a pass over an empty
    /// base lock does not read like a pass over nine.
    pub compared: usize,
}

/// Judge `head` against `base`, topic by topic.
///
/// Only base topics are judged: a topic the head adds has no consumer at the
/// base to break.
pub fn judge(base: &[LockRow], head: &[LockRow]) -> Verdict {
    let mut verdict = Verdict {
        findings: Vec::new(),
        admitted: Vec::new(),
        compared: 0,
    };
    for old in base {
        verdict.compared += 1;
        let topic = &old.topic;
        let Some(new) = head
            .iter()
            .filter(|row| &row.topic == topic)
            .max_by_key(|row| row.version)
        else {
            verdict.findings.push(format!(
                "{topic}: locked at version {} on the base and absent from the head. Keep its \
                 row: a version is retired with a consumer inventory \
                 (qip_events::retirement), never by deleting what consumers are checked against",
                old.version
            ));
            continue;
        };
        if new.version < old.version {
            verdict.findings.push(format!(
                "{topic}: the head rolls version {} back to {}. Publish the change under a \
                 version above {}",
                old.version, new.version, old.version
            ));
        } else if new.version > old.version {
            verdict.admitted.push(format!(
                "{topic}: version {} -> {}, the producer's deliberate break (ADR 0100 §5)",
                old.version, new.version
            ));
        } else if new.schema_id != old.schema_id {
            match (&old.shape, &new.shape) {
                (Some(before), Some(after)) => {
                    match check_compatible(old.version, before, new.version, after) {
                        Ok(()) => verdict.admitted.push(format!(
                            "{topic}: version {} gained fields only; an old reader still reads it",
                            old.version
                        )),
                        Err(error) => verdict
                            .findings
                            .push(format!("{topic} ({}): {error}", new.type_name)),
                    }
                }
                // Refused, not waved through: with one shape unknown there is
                // no telling an added field from a removed one, and guessing
                // "added" is how a removal merges.
                _ => verdict.findings.push(format!(
                    "{topic}: its schema id changed at version {} and one of the two locks \
                     carries no shape to compare. Bump the schema version",
                    old.version
                )),
            }
        }
    }
    verdict
}

/// `qip event-fabric schema-gate --base <lock> --head <lock>`.
pub fn run(arguments: &[String], _environment: &Environment) -> Result<Outcome> {
    let (base_path, head_path) = parse_arguments(arguments)?;
    let read = |path: &str| {
        std::fs::read_to_string(path).map_err(|e| {
            Error::invalid(format!(
                "cannot read the schema lock at {path}: {e}. Pass the path of a \
                 schemas.lock.json"
            ))
        })
    };
    let base = parse(&base_path, &read(&base_path)?)?;
    let head = parse(&head_path, &read(&head_path)?)?;
    let verdict = judge(&base, &head);

    let mut lines = verdict.admitted.clone();
    lines.extend(verdict.findings.iter().cloned());
    let code = if verdict.findings.is_empty() {
        lines.push(format!(
            "schema gate: COMPATIBLE — {} base topic(s) compared, {} changed and still readable",
            verdict.compared,
            verdict.admitted.len()
        ));
        COMPATIBLE
    } else {
        lines.push(format!(
            "schema gate: INCOMPATIBLE — {} of {} base topic(s) can no longer be read by a \
             consumer built against the base",
            verdict.findings.len(),
            verdict.compared
        ));
        INCOMPATIBLE
    };
    Ok(Outcome { lines, code })
}

fn parse_arguments(arguments: &[String]) -> Result<(String, String)> {
    let usage = "usage: qip event-fabric schema-gate --base <schemas.lock.json> --head \
                 <schemas.lock.json>";
    let mut base = None;
    let mut head = None;
    let mut rest = arguments.iter();
    while let Some(flag) = rest.next() {
        let slot = match flag.as_str() {
            "--base" => &mut base,
            "--head" => &mut head,
            other => {
                return Err(Error::invalid(format!(
                    "unknown argument {other:?}; {usage}"
                )));
            }
        };
        let value = rest
            .next()
            .ok_or_else(|| Error::invalid(format!("{flag} needs a path; {usage}")))?;
        if slot.replace(value.clone()).is_some() {
            return Err(Error::invalid(format!("{flag} was given twice; {usage}")));
        }
    }
    match (base, head) {
        (Some(base), Some(head)) => Ok((base, head)),
        _ => Err(Error::invalid(format!("both locks are required; {usage}"))),
    }
}
