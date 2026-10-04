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
use crate::event_fabric::schema_id::{SchemaId, Shape};
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
    /// Content-derived id over topic, version and the full recursive shape
    /// (`event_fabric::schema_id`). Unlike `fingerprint`, this changes when a
    /// *nested* field's kind changes even though the top-level field names
    /// this descriptor also carries did not — see that module's doc comment
    /// for why the top-level fingerprint alone is not enough for a stream to
    /// admit a producer by schema id.
    pub schema_id: SchemaId,
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
        // Reuses `value`, already computed above, rather than serialising
        // `sample` a second time to learn its shape.
        let shape = Shape::from_json(&value);
        let descriptor = SchemaDescriptor {
            topic: T::TOPIC,
            version: T::SCHEMA_VERSION,
            type_name,
            fields,
            fingerprint: qip_core::hash::sha256_hex(material.as_bytes()),
            schema_id: SchemaId::new(T::TOPIC.name(), T::SCHEMA_VERSION, &shape),
        };

        self.admit(descriptor)
    }

    /// Admit a descriptor through the compatibility gate. `register` goes
    /// through here, so a descriptor built by hand (a candidate read from a
    /// CI manifest, say) meets the same rules as a registered type.
    pub fn admit(&mut self, descriptor: SchemaDescriptor) -> Result<()> {
        let topic = descriptor.topic;
        if let Some(existing) = self.descriptors.get(&topic) {
            if existing.type_name != descriptor.type_name {
                return Err(Error::schema(format!(
                    "topic {} is already claimed by {}",
                    topic, existing.type_name
                )));
            }
            // An identical re-registration is a no-op; a changed shape under
            // the same version, a rollback or a field removal is refused.
            check_compatible(existing, &descriptor)?;
            if existing == &descriptor {
                return Ok(());
            }
        }
        self.descriptors.insert(topic, descriptor);
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

/// Backward-compatibility gate between a registered schema and a candidate
/// for the same topic (CONTRACT-046, CONTRACT-047).
///
/// Prevents a payload silently losing or changing a field under a version
/// number consumers already trust. The same version must carry the same
/// `schema_id`; a lower version is a rollback; a higher version may add
/// fields but must keep every field the registered one had, because a
/// consumer built against the old shape would otherwise read a missing field.
/// Top-level names only: nested kinds are covered by the `schema_id` equality
/// at an unchanged version.
pub fn check_compatible(registered: &SchemaDescriptor, candidate: &SchemaDescriptor) -> Result<()> {
    use std::cmp::Ordering;
    match candidate.version.cmp(&registered.version) {
        Ordering::Equal if candidate.schema_id != registered.schema_id => {
            Err(Error::schema(format!(
                "topic {} version {} is already registered with different content; \
                 bump SCHEMA_VERSION to publish a changed shape",
                registered.topic, registered.version
            )))
        }
        Ordering::Equal => Ok(()),
        Ordering::Less => Err(Error::schema(format!(
            "topic {} is registered at version {}; version {} would roll it back",
            registered.topic, registered.version, candidate.version
        ))),
        Ordering::Greater => {
            let removed: Vec<&str> = registered
                .fields
                .iter()
                .filter(|f| !candidate.fields.contains(f))
                .map(String::as_str)
                .collect();
            if removed.is_empty() {
                Ok(())
            } else {
                Err(Error::schema(format!(
                    "topic {} version {} removes field(s) {} that version {} carried; \
                     keep them or publish a new topic",
                    registered.topic,
                    candidate.version,
                    removed.join(","),
                    registered.version
                )))
            }
        }
    }
}
