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
use crate::event_fabric::schema_id::{SchemaId, Shape, check_compatible};
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
    /// The recursive shape `schema_id` hashes, kept so a later registration
    /// of the same topic can be compared field by field rather than by hash.
    pub shape: Shape,
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

    /// Register multiple event body types by their samples.
    ///
    /// Convenience method for bulk registration of known types at startup.
    pub fn register_samples<T: EventBody>(
        &mut self,
        samples: impl IntoIterator<Item = T>,
    ) -> Result<()> {
        for sample in samples {
            self.register(&sample)?;
        }
        Ok(())
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
            shape,
        };

        self.admit(descriptor)
    }

    /// Admit a descriptor, holding CONTRACT-046's rule that a schema id
    /// always denotes one schema. Without it a second registration overwrote
    /// the first, so a payload could change shape under a version consumers
    /// already trusted. Identical content is a no-op; a changed shape under
    /// the same version is refused naming the field that moved
    /// (`schema_id::check_compatible`, the same gate the CI schema lock
    /// uses); a lower version is a rollback; a higher one is the producer's
    /// deliberate break (ADR 0100 §5) and replaces the descriptor.
    pub fn admit(&mut self, descriptor: SchemaDescriptor) -> Result<()> {
        let topic = descriptor.topic;
        if let Some(existing) = self.descriptors.get(&topic) {
            if existing.type_name != descriptor.type_name {
                return Err(Error::schema(format!(
                    "topic {} is already claimed by {}",
                    topic, existing.type_name
                )));
            }
            if descriptor.version < existing.version {
                return Err(Error::schema(format!(
                    "topic {} is registered at version {}; version {} would roll it back",
                    topic, existing.version, descriptor.version
                )));
            }
            if descriptor.version == existing.version {
                if descriptor.schema_id == existing.schema_id {
                    return Ok(());
                }
                // Name the field if it is a removal or retype; an additive
                // change is compatible but still changes the id, so it needs
                // a bump too.
                check_compatible(
                    existing.version,
                    &existing.shape,
                    descriptor.version,
                    &descriptor.shape,
                )?;
                return Err(Error::schema(format!(
                    "topic {} version {} is already registered with different content; \
                     bump SCHEMA_VERSION to publish a changed shape",
                    topic, existing.version
                )));
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

    /// Every descriptor of `rows`, admitted in ascending version order so a
    /// topic that carries two versions ends at its highest, as
    /// [`Self::admit`] decides.
    pub fn admit_all(&mut self, rows: impl IntoIterator<Item = SchemaDescriptor>) -> Result<()> {
        let mut rows: Vec<SchemaDescriptor> = rows.into_iter().collect();
        rows.sort_by_key(|d| (d.topic, d.version));
        for row in rows {
            self.admit(row)?;
        }
        Ok(())
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

/// The committed lock, embedded at build time (ADR 0156 §1.2). A deployed
/// broker therefore admits against the lock its image was built with and
/// cannot be pointed at another one at run time; nothing reads a file.
pub const EMBEDDED_LOCK: &str = include_str!("../schemas.lock.json");

/// One row of the lock as committed. Private: the lock's shape is this
/// module's concern and callers receive [`SchemaDescriptor`]s.
#[derive(Deserialize)]
struct LockRow {
    topic: String,
    version: u32,
    type_name: String,
    schema_id: String,
    shape: Shape,
}

/// The descriptors of the lock embedded in this build, one per row.
pub fn locked_descriptors() -> Result<Vec<SchemaDescriptor>> {
    descriptors_from_lock(EMBEDDED_LOCK)
}

/// Read descriptors from lock text.
///
/// Pure: the caller supplies the text. Refuses, naming the row, a topic this
/// build does not know, a row whose `schema_id` is not the hash of its own
/// topic, version and shape (a lock edited by hand, or a shape that moved
/// without the id), and a `(topic, version)` appearing twice, because
/// whichever row a reader met first would decide what the version meant.
pub fn descriptors_from_lock(lock: &str) -> Result<Vec<SchemaDescriptor>> {
    let rows: Vec<LockRow> = serde_json::from_str(lock)
        .map_err(|e| Error::schema(format!("the schema lock does not parse: {e}")))?;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let topic = Topic::from_name(&row.topic).ok_or_else(|| {
            Error::schema(format!(
                "the schema lock names topic '{}', which this build does not define",
                row.topic
            ))
        })?;
        if !seen.insert((topic, row.version)) {
            return Err(Error::schema(format!(
                "the schema lock holds topic {} version {} more than once",
                row.topic, row.version
            )));
        }
        let computed = SchemaId::new(&row.topic, row.version, &row.shape);
        if computed.as_str() != row.schema_id {
            return Err(Error::schema(format!(
                "the schema lock row for topic {} version {} carries schema id {} but its \
                 shape hashes to {}; regenerate the lock rather than editing it",
                row.topic,
                row.version,
                row.schema_id,
                computed.as_str()
            )));
        }
        let fields = match &row.shape {
            Shape::Object(map) => map.keys().cloned().collect(),
            _ => Vec::new(),
        };
        let material = format!("{}|{}|{}", row.topic, row.version, fields_joined(&fields));
        out.push(SchemaDescriptor {
            topic,
            version: row.version,
            type_name: row.type_name,
            fingerprint: qip_core::hash::sha256_hex(material.as_bytes()),
            fields,
            schema_id: computed,
            shape: row.shape,
        });
    }
    Ok(out)
}

fn fields_joined(fields: &[String]) -> String {
    fields.join(",")
}
