//! Schema-version retirement (CICD-072).
//!
//! A version is retired only when an inventory proves nobody still reads it.
//! "Nobody told us they do" is not proof, so a version that was never
//! surveyed is refused as firmly as one with a listed consumer: retiring on
//! silence is how a consumer that was never asked finds its decoder gone.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};

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
