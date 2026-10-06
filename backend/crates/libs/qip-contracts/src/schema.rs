//! Schema registry contract (SLICE-05): immutable schema IDs and versioning.
//!
//! Schemas must be versioned and immutable: once registered under an ID,
//! that ID always denotes the same bytes.

use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Backward/forward compatibility policy for schema evolution
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityPolicy {
    /// New schema must be compatible with the previous version
    Backward,
    /// New schema must accept records written by previous version
    Forward,
    /// New schema must be compatible in both directions
    Full,
    /// No compatibility check; the old and new are independent
    None,
}

impl CompatibilityPolicy {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Backward => "backward",
            Self::Forward => "forward",
            Self::Full => "full",
            Self::None => "none",
        }
    }
}

/// A registered schema in the registry, immutable after registration
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaDefinition {
    pub schema_id: u32,
    pub version: u32,
    pub content_hash: String,
    pub schema_bytes: String,
    pub registered_at_unix_secs: u64,
    pub registered_by: String,
    pub compatibility_policy: CompatibilityPolicy,
    pub previous_schema_id: Option<u32>,
    pub documentation: String,
}

impl SchemaDefinition {
    /// Register a new schema, computing its content hash
    pub fn register(
        schema_id: u32,
        version: u32,
        schema_bytes: impl Into<String>,
        registered_by: impl Into<String>,
        compatibility_policy: CompatibilityPolicy,
    ) -> Result<Self> {
        let schema_bytes = schema_bytes.into();
        if schema_bytes.is_empty() {
            return Err(Error::invalid("schema bytes cannot be empty"));
        }

        let content_hash = sha256_hex(schema_bytes.as_bytes());

        Ok(Self {
            schema_id,
            version,
            content_hash,
            schema_bytes,
            registered_at_unix_secs: 0,
            registered_by: registered_by.into(),
            compatibility_policy,
            previous_schema_id: None,
            documentation: String::new(),
        })
    }

    pub fn with_documentation(mut self, doc: impl Into<String>) -> Self {
        self.documentation = doc.into();
        self
    }

    pub fn with_previous_schema(mut self, previous_id: u32) -> Self {
        self.previous_schema_id = Some(previous_id);
        self
    }

    /// Verify that the stored content_hash matches the schema_bytes
    pub fn is_consistent(&self) -> bool {
        sha256_hex(self.schema_bytes.as_bytes()) == self.content_hash
    }
}

/// A schema registry: holds all registered schemas immutably
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaRegistry {
    pub schemas: BTreeMap<u32, SchemaDefinition>,
    pub stream_schemas: BTreeMap<String, BTreeMap<String, u32>>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a schema in the registry.
    /// If the schema ID already exists with identical bytes, return success.
    /// If the schema ID exists with different bytes, return error (immutable).
    pub fn register(&mut self, definition: SchemaDefinition) -> Result<u32> {
        if definition.schema_id == 0 {
            return Err(Error::invalid("schema ID 0 is reserved and cannot be used"));
        }

        match self.schemas.get(&definition.schema_id) {
            Some(existing) => {
                // ID exists: bytes must be identical
                if existing.schema_bytes != definition.schema_bytes {
                    return Err(Error::invalid(format!(
                        "schema ID {} already holds different content; registered schema IDs are immutable",
                        definition.schema_id
                    )));
                }
                // Same content: idempotent success
                Ok(definition.schema_id)
            }
            None => {
                // New ID: register it
                let id = definition.schema_id;
                self.schemas.insert(id, definition);
                Ok(id)
            }
        }
    }

    /// Register a schema for a stream (subject tracking)
    pub fn register_for_stream(
        &mut self,
        stream_name: impl Into<String>,
        subject: impl Into<String>,
        schema_id: u32,
    ) -> Result<()> {
        // First ensure the schema exists
        if !self.schemas.contains_key(&schema_id) {
            return Err(Error::invalid(format!(
                "cannot assign schema ID {} to a stream until it is registered",
                schema_id
            )));
        }

        let stream = stream_name.into();
        let subj = subject.into();
        self.stream_schemas
            .entry(stream)
            .or_default()
            .insert(subj, schema_id);
        Ok(())
    }

    /// Get a schema by ID
    pub fn get(&self, schema_id: u32) -> Option<&SchemaDefinition> {
        self.schemas.get(&schema_id)
    }

    /// Get the latest schema ID for a stream subject
    pub fn get_latest_for_stream(&self, stream_name: &str, subject: &str) -> Option<u32> {
        self.stream_schemas
            .get(stream_name)
            .and_then(|subjects| subjects.get(subject).copied())
    }

    /// Check compatibility between two schemas under a policy
    pub fn check_compatibility(
        &self,
        old_schema_id: u32,
        new_schema_id: u32,
        policy: CompatibilityPolicy,
    ) -> Result<bool> {
        let old = self
            .get(old_schema_id)
            .ok_or_else(|| Error::invalid(format!("old schema ID {} not found", old_schema_id)))?;

        let new = self
            .get(new_schema_id)
            .ok_or_else(|| Error::invalid(format!("new schema ID {} not found", new_schema_id)))?;

        match policy {
            CompatibilityPolicy::None => Ok(true),
            CompatibilityPolicy::Backward => {
                Ok(old.schema_bytes != new.schema_bytes || old.version < new.version)
            }
            CompatibilityPolicy::Forward => {
                Ok(old.schema_bytes != new.schema_bytes || old.version < new.version)
            }
            CompatibilityPolicy::Full => {
                Ok(old.schema_bytes != new.schema_bytes || old.version < new.version)
            }
        }
    }

    /// Validate the registry's consistency
    pub fn validate(&self) -> Result<()> {
        for definition in self.schemas.values() {
            if !definition.is_consistent() {
                return Err(Error::invalid(format!(
                    "schema ID {} has inconsistent content hash",
                    definition.schema_id
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_registration_is_immutable() {
        let mut registry = SchemaRegistry::new();

        let schema1 = SchemaDefinition::register(1, 1, "bytes1", "test", CompatibilityPolicy::Full)
            .expect("failed to create schema");
        assert!(registry.register(schema1).is_ok());

        // Registering the same ID with the same bytes is idempotent
        let schema1_again =
            SchemaDefinition::register(1, 1, "bytes1", "test", CompatibilityPolicy::Full)
                .expect("failed to create schema");
        assert!(registry.register(schema1_again).is_ok());

        // Registering the same ID with different bytes is refused
        let schema1_different =
            SchemaDefinition::register(1, 2, "different_bytes", "test", CompatibilityPolicy::Full)
                .expect("failed to create schema");
        let result = registry.register(schema1_different);
        assert!(result.is_err());
    }

    #[test]
    fn schema_content_hash_is_verified() {
        let schema =
            SchemaDefinition::register(1, 1, "test_content", "user", CompatibilityPolicy::Full)
                .expect("failed to create schema");
        assert!(schema.is_consistent());

        // Manually corrupt the hash
        let mut corrupted = schema;
        corrupted.content_hash = "wrong_hash".to_string();
        assert!(!corrupted.is_consistent());
    }

    #[test]
    fn stream_schema_registration_requires_existing_schema() {
        let mut registry = SchemaRegistry::new();

        // Cannot register a stream for a non-existent schema
        let result = registry.register_for_stream("my_stream", "value", 999);
        assert!(result.is_err());

        // First register the schema
        let schema = SchemaDefinition::register(1, 1, "content", "user", CompatibilityPolicy::Full)
            .expect("failed to create schema");
        let _ = registry.register(schema);

        // Now registering for a stream succeeds
        assert!(
            registry
                .register_for_stream("my_stream", "value", 1)
                .is_ok()
        );
    }

    #[test]
    fn schema_registry_validates_consistency() {
        let mut registry = SchemaRegistry::new();

        let mut schema =
            SchemaDefinition::register(1, 1, "bytes", "test", CompatibilityPolicy::Full)
                .expect("failed to create schema");
        let _ = registry.register(schema.clone());

        // Corrupt a schema in the registry
        schema.content_hash = "invalid".to_string();
        registry.schemas.insert(1, schema);

        assert!(registry.validate().is_err());
    }
}
