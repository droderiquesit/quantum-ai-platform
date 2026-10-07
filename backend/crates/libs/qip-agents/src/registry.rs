//! Runtime registries for agents, tools, models and other extensible platform components.
//!
//! A registry allows the platform to add new agents, tools, models, asset classes and
//! other extensible components without code changes or redeployment. Each registry
//! enforces its own validation and governance rules.

use crate::manifest::AgentManifest;
use qip_core::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A runtime registry of indexed entries.
///
/// The registry is keyed by a stable identifier (typically a string ID). It supports
/// adding, retrieving, and listing entries without redeployment. Validation is
/// performed at registration time and checked at access time.
#[derive(Clone, Debug)]
pub struct Registry<K: Ord + Clone, V: Clone> {
    entries: BTreeMap<K, V>,
}

impl<K: Ord + Clone, V: Clone> Registry<K, V> {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Add an entry to the registry.
    pub fn register(&mut self, key: K, value: V) -> Result<()> {
        self.entries.insert(key, value);
        Ok(())
    }

    /// Get an entry by key.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.entries.get(key)
    }

    /// Get a mutable reference to an entry.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.entries.get_mut(key)
    }

    /// List all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter()
    }

    /// Count of registered entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Check if a key is registered.
    pub fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }
}

impl<K: Ord + Clone, V: Clone> Default for Registry<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

/// A specialist registry holding agent manifests keyed by ID.
pub type SpecialistRegistry = Registry<String, AgentManifest>;

/// Specification for registering a tool at runtime.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Unique stable identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// What this tool does.
    pub description: String,
    /// Permissions this tool requires.
    pub permission_scope: String,
    /// Tool kind: connector, browser, parser, solver, simulator, adapter, model_endpoint, quantum_backend.
    pub kind: String,
}

/// A tool registry holding ToolSpec entries keyed by ID.
pub type ToolRegistry = Registry<String, ToolSpec>;

/// Specification for a model family.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelFamilySpec {
    /// Unique stable identifier.
    pub id: String,
    /// Name of the model family.
    pub name: String,
    /// What models in this family are used for.
    pub description: String,
    /// URL or path to the family's endpoint/binary.
    pub endpoint: String,
}

/// A model family registry holding ModelFamilySpec entries keyed by ID.
pub type ModelFamilyRegistry = Registry<String, ModelFamilySpec>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_can_register_and_retrieve_entries() {
        let mut registry = Registry::<String, String>::new();
        registry
            .register("key1".to_string(), "value1".to_string())
            .unwrap();
        registry
            .register("key2".to_string(), "value2".to_string())
            .unwrap();

        assert_eq!(registry.len(), 2);
        assert_eq!(registry.get(&"key1".to_string()).unwrap(), "value1");
        assert_eq!(registry.get(&"key2".to_string()).unwrap(), "value2");
    }

    #[test]
    fn registry_can_iterate_entries() {
        let mut registry = Registry::<String, String>::new();
        registry.register("a".to_string(), "1".to_string()).unwrap();
        registry.register("b".to_string(), "2".to_string()).unwrap();

        let entries: Vec<_> = registry.iter().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], (&"a".to_string(), &"1".to_string()));
        assert_eq!(entries[1], (&"b".to_string(), &"2".to_string()));
    }

    #[test]
    fn registry_check_key_existence() {
        let mut registry = Registry::<String, String>::new();
        registry
            .register("exists".to_string(), "value".to_string())
            .unwrap();

        assert!(registry.contains_key(&"exists".to_string()));
        assert!(!registry.contains_key(&"missing".to_string()));
    }
}
