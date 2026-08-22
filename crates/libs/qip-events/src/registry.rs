//! Schema registry.
//!
//! Records the topic, current schema version and field shape of every event
//! body registered at start-up. Two things depend on it: the contract test that
//! fails when a payload changes without a version bump, and the documentation
//! test that fails when `docs/architecture/events.md` drifts from the code.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::envelope::EventBody;
use crate::topic::Topic;

/// The registered shape of one event body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchemaDescriptor {
    pub topic: Topic,
    pub version: u32,
    /// Rust type name, for diagnostics.
    pub type_name: String,
    /// Top-level field names, sorted. Enough to detect a shape change without
    /// carrying a full JSON Schema implementation.
    pub fields: Vec<String>,
    /// Hash over topic, version and fields — the contract fingerprint.
    pub fingerprint: String,
}

/// All registered event schemas.
#[derive(Debug, Default, Clone)]
pub struct SchemaRegistry {
    descriptors: BTreeMap<Topic, SchemaDescriptor>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a body type by serialising a sample instance to learn its shape.
    ///
    /// A sample is required because the fields are read off the serialised
    /// form; there is no reflection to interrogate instead.
    pub fn register<T: EventBody>(&mut self, sample: &T) -> Result<()> {
        let value = serde_json::to_value(sample)?;
        let fields = match &value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<String> = map.keys().cloned().collect();
                keys.sort();
                keys
            }
            _ => Vec::new(),
        };
        let type_name = std::any::type_name::<T>().to_string();
        let material = format!(
            "{}|{}|{}",
            T::TOPIC.name(),
            T::SCHEMA_VERSION,
            fields.join(",")
        );
        let descriptor = SchemaDescriptor {
            topic: T::TOPIC,
            version: T::SCHEMA_VERSION,
            type_name,
            fields,
            fingerprint: qip_core::hash::sha256_hex(material.as_bytes()),
        };

        if let Some(existing) = self.descriptors.get(&T::TOPIC)
            && existing.type_name != descriptor.type_name
        {
            return Err(Error::schema(format!(
                "topic {} is already claimed by {}",
                T::TOPIC,
                existing.type_name
            )));
        }
        self.descriptors.insert(T::TOPIC, descriptor);
        Ok(())
    }

    pub fn get(&self, topic: Topic) -> Option<&SchemaDescriptor> {
        self.descriptors.get(&topic)
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SchemaDescriptor> {
        self.descriptors.values()
    }

    /// Topics with no registered body type.
    pub fn unregistered_topics(&self) -> Vec<Topic> {
        Topic::ALL
            .iter()
            .copied()
            .filter(|t| !self.descriptors.contains_key(t))
            .collect()
    }

    /// A stable fingerprint over the whole contract surface. A change here
    /// without a corresponding version bump is what the contract test catches.
    pub fn fingerprint(&self) -> String {
        let material: Vec<String> = self
            .descriptors
            .values()
            .map(|d| format!("{}={}", d.topic.name(), d.fingerprint))
            .collect();
        qip_core::hash::sha256_hex(material.join(";").as_bytes())
    }
}
