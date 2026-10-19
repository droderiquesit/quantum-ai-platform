//! Schema-version retirement (CICD-072).
//!
//! A version is retired only when an inventory proves nobody still reads it.
//! "Nobody told us they do" is not proof, so a version that was never
//! surveyed is refused as firmly as one with a listed consumer: retiring on
//! silence is how a consumer that was never asked finds its decoder gone.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};
use serde::Deserialize;

use crate::topic::Topic;

/// Who consumes which `(topic, version)`.
#[derive(Clone, Debug, Default)]
pub struct ConsumerInventory {
    surveyed: BTreeMap<(Topic, u32), BTreeSet<String>>,
    retired: BTreeSet<(Topic, u32)>,
}

impl ConsumerInventory {
    /// Record that the inventory of `(topic, version)` has been taken. With no
    /// consumer added afterwards it is the proof of an empty set.
    pub fn survey(&mut self, topic: Topic, version: u32) {
        self.surveyed.entry((topic, version)).or_default();
    }

    /// A consumer depends on this version. Also counts as a survey.
    pub fn depend(&mut self, topic: Topic, version: u32, consumer: impl Into<String>) {
        self.surveyed
            .entry((topic, version))
            .or_default()
            .insert(consumer.into());
    }

    /// A consumer has migrated off this version.
    pub fn migrate_off(&mut self, topic: Topic, version: u32, consumer: &str) {
        if let Some(set) = self.surveyed.get_mut(&(topic, version)) {
            set.remove(consumer);
        }
    }

    /// Retire the version, or refuse naming the consumers that still read it.
    pub fn retire(&mut self, topic: Topic, version: u32) -> Result<()> {
        let key = (topic, version);
        let Some(consumers) = self.surveyed.get(&key) else {
            return Err(Error::denied(format!(
                "{topic} v{version} has no consumer inventory; survey it before retiring, \
                 because no record is not the same as no consumer"
            )));
        };
        if !consumers.is_empty() {
            let names: Vec<&str> = consumers.iter().map(String::as_str).collect();
            return Err(Error::denied(format!(
                "{topic} v{version} is still consumed by [{}]; migrate them off first",
                names.join(", ")
            )));
        }
        self.retired.insert(key);
        Ok(())
    }

    pub fn is_retired(&self, topic: Topic, version: u32) -> bool {
        self.retired.contains(&(topic, version))
    }
}

/// The committed consumer inventory, embedded at build time like the schema
/// lock it is read beside (ADR 0156 §1.6): the binary that judges a change
/// holds the head's inventory without being pointed at a file.
pub const EMBEDDED_CONSUMERS: &str = include_str!("../consumers.lock.json");

/// How a crate meets a body. Only [`ConsumerRole::Decodes`] holds a version
/// in place: a crate that builds the body or names the type without reading
/// one off the fabric has nothing to migrate when the version moves.
///
/// The role is a reviewed claim, not a computation (ADR 0156 §1.6): which
/// crates decode a type is not recorded in Cargo metadata, and a heuristic
/// that guessed wrong in the quiet direction would retire a version someone
/// still reads. The acceptance check proves a lower bound on who is listed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerRole {
    #[default]
    Decodes,
    Produces,
    Names,
}

/// Whether a consumer is a library crate or a deployed binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerKind {
    Crate,
    Binary,
}

/// One consumer row of `consumers.lock.json`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumerRow {
    /// The crate's directory name under `backend/crates/*/`.
    pub name: String,
    pub kind: ConsumerKind,
    /// The Rust type the crate's source uses for this body; the acceptance
    /// check finds it by name in the crate's source.
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default)]
    pub role: ConsumerRole,
    /// The crate no longer reads this version. A missing field is `false`:
    /// an unmarked consumer is a consumer.
    #[serde(default)]
    pub migrated: bool,
}

/// The consumers of one `(topic, version)`. An empty list is a survey that
/// found none, which is the proof [`ConsumerInventory::retire`] needs.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRow {
    pub topic: String,
    pub version: u32,
    pub consumers: Vec<ConsumerRow>,
}

/// Parse an inventory file's text. Pure: the caller supplies the text.
pub fn parse_inventory(text: &str) -> Result<Vec<InventoryRow>> {
    let rows: Vec<InventoryRow> = serde_json::from_str(text)
        .map_err(|e| Error::invalid(format!("the consumer inventory does not parse: {e}")))?;
    let mut seen = BTreeSet::new();
    for row in &rows {
        if Topic::from_name(&row.topic).is_none() {
            return Err(Error::invalid(format!(
                "the consumer inventory names topic '{}', which this build does not define",
                row.topic
            )));
        }
        if !seen.insert((row.topic.clone(), row.version)) {
            return Err(Error::invalid(format!(
                "the consumer inventory lists topic {} version {} twice",
                row.topic, row.version
            )));
        }
    }
    Ok(rows)
}

impl ConsumerInventory {
    /// The inventory `rows` describe: every row is a survey, and each
    /// consumer that decodes the version and has not migrated is a
    /// dependency on it.
    pub fn from_rows(rows: &[InventoryRow]) -> Result<Self> {
        let mut inventory = Self::default();
        for row in rows {
            let topic = Topic::from_name(&row.topic).ok_or_else(|| {
                Error::invalid(format!("unknown topic '{}' in the inventory", row.topic))
            })?;
            inventory.survey(topic, row.version);
            for consumer in &row.consumers {
                if consumer.role == ConsumerRole::Decodes && !consumer.migrated {
                    inventory.depend(topic, row.version, consumer.name.clone());
                }
            }
        }
        Ok(inventory)
    }
}
