//! EXPAND-034/040: Ontology and Capability registries.
//!
//! The Ontology Registry holds versioned entity, event, relationship, market,
//! asset, product, causal-driver, and lifecycle type definitions. Each type
//! family's definitions are stored by name, versioned independently.
//!
//! The Capability Registry records which platform capabilities (the ten verbs:
//! sense, reason, simulate, trade, settle, hedge, transfer, purchase, create,
//! operate) exist and through which implementations, versioned as a whole.
//! Capabilities are added or removed only by creating a new registry version.

use qip_contracts::expansion::{
    CapabilityEntry, CapabilityVerb, OntologyTypeFamily, OntologyTypeSpec,
};
use qip_core::error::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

/// Ontology Registry: versioned definitions of entity, event, relationship,
/// market, asset, product, causal-driver, and lifecycle types.
#[derive(Debug, Clone)]
pub struct OntologyRegistry {
    types: BTreeMap<(OntologyTypeFamily, String, u32), OntologyTypeSpec>,
    latest_version_per_name: BTreeMap<(OntologyTypeFamily, String), u32>,
}

impl OntologyRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self {
            types: BTreeMap::new(),
            latest_version_per_name: BTreeMap::new(),
        }
    }

    /// Register a type. Rejects if the exact same (family, name, version)
    /// already exists. Accepts new versions of existing names.
    pub fn register(&mut self, spec: OntologyTypeSpec) -> Result<()> {
        let key = (spec.family, spec.name.clone(), spec.version);

        if self.types.contains_key(&key) {
            return Err(Error::invalid(format!(
                "OntologyTypeSpec {}.{} version {} already registered",
                format!("{:?}", spec.family),
                spec.name,
                spec.version
            )));
        }

        let name_key = (spec.family, spec.name.clone());
        self.latest_version_per_name.insert(name_key, spec.version);
        self.types.insert(key, spec);
        Ok(())
    }

    /// Retrieve a type by family, name, and version. Returns None if not found.
    pub fn get_type(
        &self,
        family: OntologyTypeFamily,
        name: &str,
        version: u32,
    ) -> Option<OntologyTypeSpec> {
        self.types
            .get(&(family, name.to_string(), version))
            .cloned()
    }

    /// Retrieve the latest version of a type by family and name.
    pub fn get_latest(&self, family: OntologyTypeFamily, name: &str) -> Option<OntologyTypeSpec> {
        let name_key = (family, name.to_string());
        self.latest_version_per_name
            .get(&name_key)
            .and_then(|&version| self.get_type(family, name, version))
    }

    /// List all types in a family at their latest versions.
    pub fn types_by_family(&self, family: OntologyTypeFamily) -> Vec<OntologyTypeSpec> {
        self.latest_version_per_name
            .iter()
            .filter(|((f, _), _)| *f == family)
            .filter_map(|((_, name), &version)| self.get_type(family, name, version))
            .collect()
    }

    /// Count the total number of registered type definitions.
    pub fn type_count(&self) -> usize {
        self.types.len()
    }
}

impl Default for OntologyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Capability Registry: records which platform capabilities exist, versioned as
/// a whole. Each capability is identified by its verb and a set of
/// implementations and confidence level.
#[derive(Debug, Clone)]
pub struct CapabilityRegistry {
    version: u32,
    capabilities: BTreeMap<CapabilityVerb, CapabilityEntry>,
    version_history: BTreeMap<u32, BTreeMap<CapabilityVerb, CapabilityEntry>>,
}

impl CapabilityRegistry {
    /// Construct a new, empty registry at version 1.
    pub fn new() -> Self {
        Self {
            version: 1,
            capabilities: BTreeMap::new(),
            version_history: BTreeMap::new(),
        }
    }

    /// Get the current version.
    pub fn current_version(&self) -> u32 {
        self.version
    }

    /// Get the capabilities at a specific version. Returns None if the version
    /// does not exist.
    pub fn capabilities_at_version(&self, version: u32) -> Option<Vec<CapabilityEntry>> {
        if version == self.version {
            Some(self.capabilities.values().cloned().collect())
        } else {
            self.version_history
                .get(&version)
                .map(|caps| caps.values().cloned().collect())
        }
    }

    /// Get a specific capability entry by verb.
    pub fn get_capability(&self, verb: CapabilityVerb) -> Option<CapabilityEntry> {
        self.capabilities.get(&verb).cloned()
    }

    /// Add or update a capability and bump the version.
    pub fn add_capability(&mut self, entry: CapabilityEntry) -> Result<()> {
        self.version_history
            .insert(self.version, self.capabilities.clone());
        self.capabilities.insert(entry.verb, entry);
        self.version += 1;

        Ok(())
    }

    /// Remove a capability and bump the version. Returns an error if the
    /// capability does not exist.
    pub fn remove_capability(&mut self, verb: CapabilityVerb) -> Result<()> {
        if !self.capabilities.contains_key(&verb) {
            return Err(Error::invalid(format!(
                "CapabilityVerb::{:?} not found in registry",
                verb
            )));
        }

        self.version_history
            .insert(self.version, self.capabilities.clone());
        self.capabilities.remove(&verb);
        self.version += 1;

        Ok(())
    }

    /// List all verbs that have a capability registered.
    pub fn all_verbs(&self) -> Vec<CapabilityVerb> {
        self.capabilities.keys().cloned().collect()
    }

    /// Count the number of capabilities at the current version.
    pub fn capability_count(&self) -> usize {
        self.capabilities.len()
    }
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ontology_type_can_be_registered_and_retrieved() {
        let mut reg = OntologyRegistry::new();
        let spec = OntologyTypeSpec {
            family: OntologyTypeFamily::Entity,
            name: "Customer".to_string(),
            version: 1,
            schema: r#"{"type": "object"}"#.to_string(),
            description: "A customer entity".to_string(),
        };

        reg.register(spec.clone()).unwrap();
        let retrieved = reg.get_type(OntologyTypeFamily::Entity, "Customer", 1);
        assert_eq!(retrieved, Some(spec));
    }

    #[test]
    fn ontology_rejects_duplicate_family_name_version() {
        let mut reg = OntologyRegistry::new();
        let spec = OntologyTypeSpec {
            family: OntologyTypeFamily::Event,
            name: "Trade".to_string(),
            version: 1,
            schema: r#"{"type": "object"}"#.to_string(),
            description: "A trade event".to_string(),
        };

        reg.register(spec.clone()).unwrap();
        let err = reg.register(spec);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("already registered"));
    }

    #[test]
    fn ontology_permits_new_versions_of_existing_name() {
        let mut reg = OntologyRegistry::new();
        let spec_v1 = OntologyTypeSpec {
            family: OntologyTypeFamily::Relationship,
            name: "Owns".to_string(),
            version: 1,
            schema: r#"{"v": 1}"#.to_string(),
            description: "v1".to_string(),
        };
        let spec_v2 = OntologyTypeSpec {
            family: OntologyTypeFamily::Relationship,
            name: "Owns".to_string(),
            version: 2,
            schema: r#"{"v": 2}"#.to_string(),
            description: "v2".to_string(),
        };

        reg.register(spec_v1).unwrap();
        reg.register(spec_v2).unwrap();

        assert_eq!(reg.type_count(), 2);
        assert!(
            reg.get_type(OntologyTypeFamily::Relationship, "Owns", 1)
                .is_some()
        );
        assert!(
            reg.get_type(OntologyTypeFamily::Relationship, "Owns", 2)
                .is_some()
        );
    }

    #[test]
    fn ontology_get_latest_returns_highest_version() {
        let mut reg = OntologyRegistry::new();
        let spec_v1 = OntologyTypeSpec {
            family: OntologyTypeFamily::Market,
            name: "Equity".to_string(),
            version: 1,
            schema: r#"{"v": 1}"#.to_string(),
            description: "v1".to_string(),
        };
        let spec_v2 = OntologyTypeSpec {
            family: OntologyTypeFamily::Market,
            name: "Equity".to_string(),
            version: 2,
            schema: r#"{"v": 2}"#.to_string(),
            description: "v2".to_string(),
        };

        reg.register(spec_v1).unwrap();
        reg.register(spec_v2).unwrap();

        let latest = reg
            .get_latest(OntologyTypeFamily::Market, "Equity")
            .unwrap();
        assert_eq!(latest.version, 2);
    }

    #[test]
    fn ontology_types_by_family_lists_latest_of_each_type() {
        let mut reg = OntologyRegistry::new();
        let entity1 = OntologyTypeSpec {
            family: OntologyTypeFamily::Entity,
            name: "Account".to_string(),
            version: 1,
            schema: r#"{"v": 1}"#.to_string(),
            description: "v1".to_string(),
        };
        let entity1_v2 = OntologyTypeSpec {
            family: OntologyTypeFamily::Entity,
            name: "Account".to_string(),
            version: 2,
            schema: r#"{"v": 2}"#.to_string(),
            description: "v2".to_string(),
        };
        let entity2 = OntologyTypeSpec {
            family: OntologyTypeFamily::Entity,
            name: "Person".to_string(),
            version: 1,
            schema: r#"{}"#.to_string(),
            description: "Person".to_string(),
        };

        reg.register(entity1).unwrap();
        reg.register(entity1_v2).unwrap();
        reg.register(entity2).unwrap();

        let entities = reg.types_by_family(OntologyTypeFamily::Entity);
        assert_eq!(entities.len(), 2);
        let names: BTreeSet<String> = entities.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains("Account"));
        assert!(names.contains("Person"));
        let account = entities.iter().find(|e| e.name == "Account").unwrap();
        assert_eq!(account.version, 2);
    }

    #[test]
    fn ontology_all_eight_families_can_be_registered() {
        let mut reg = OntologyRegistry::new();
        let families = vec![
            OntologyTypeFamily::Entity,
            OntologyTypeFamily::Event,
            OntologyTypeFamily::Relationship,
            OntologyTypeFamily::Market,
            OntologyTypeFamily::Asset,
            OntologyTypeFamily::Product,
            OntologyTypeFamily::CausalDriver,
            OntologyTypeFamily::Lifecycle,
        ];

        for (i, family) in families.iter().enumerate() {
            let spec = OntologyTypeSpec {
                family: *family,
                name: format!("Type{}", i),
                version: 1,
                schema: "{}".to_string(),
                description: "test".to_string(),
            };
            reg.register(spec).unwrap();
        }

        assert_eq!(reg.type_count(), 8);
    }

    #[test]
    fn a_capability_can_be_registered_and_retrieved() {
        let mut reg = CapabilityRegistry::new();
        let cap = CapabilityEntry {
            verb: CapabilityVerb::Sense,
            implementations: vec!["market_feed".to_string()],
            confidence_level: 0.95,
            eligibility: "any_agent".to_string(),
        };

        reg.add_capability(cap.clone()).unwrap();
        let retrieved = reg.get_capability(CapabilityVerb::Sense);
        assert_eq!(retrieved, Some(cap));
    }

    #[test]
    fn capability_add_increments_version() {
        let mut reg = CapabilityRegistry::new();
        assert_eq!(reg.current_version(), 1);

        let cap = CapabilityEntry {
            verb: CapabilityVerb::Reason,
            implementations: vec!["model_v1".to_string()],
            confidence_level: 0.85,
            eligibility: "approved".to_string(),
        };
        reg.add_capability(cap).unwrap();
        assert_eq!(reg.current_version(), 2);
    }

    #[test]
    fn capability_remove_increments_version_and_removes_verb() {
        let mut reg = CapabilityRegistry::new();
        let cap = CapabilityEntry {
            verb: CapabilityVerb::Simulate,
            implementations: vec!["sim_engine".to_string()],
            confidence_level: 0.90,
            eligibility: "test".to_string(),
        };
        reg.add_capability(cap).unwrap();
        let version_before_remove = reg.current_version();

        reg.remove_capability(CapabilityVerb::Simulate).unwrap();
        assert_eq!(reg.current_version(), version_before_remove + 1);
        assert!(!reg.all_verbs().contains(&CapabilityVerb::Simulate));
    }

    #[test]
    fn capability_remove_nonexistent_returns_error() {
        let mut reg = CapabilityRegistry::new();
        let err = reg.remove_capability(CapabilityVerb::Trade);
        assert!(err.is_err());
    }

    #[test]
    fn capability_registry_tracks_history() {
        let mut reg = CapabilityRegistry::new();
        let cap1 = CapabilityEntry {
            verb: CapabilityVerb::Settle,
            implementations: vec!["broker_1".to_string()],
            confidence_level: 0.88,
            eligibility: "live".to_string(),
        };
        let cap2 = CapabilityEntry {
            verb: CapabilityVerb::Hedge,
            implementations: vec!["hedge_algo".to_string()],
            confidence_level: 0.92,
            eligibility: "approved".to_string(),
        };

        reg.add_capability(cap1).unwrap();
        let v1 = reg.current_version();
        reg.add_capability(cap2).unwrap();
        let v2 = reg.current_version();

        let caps_at_v1 = reg.capabilities_at_version(v1).unwrap();
        assert_eq!(caps_at_v1.len(), 1);
        assert_eq!(caps_at_v1[0].verb, CapabilityVerb::Settle);

        let caps_at_v2 = reg.capabilities_at_version(v2).unwrap();
        assert_eq!(caps_at_v2.len(), 2);
    }

    #[test]
    fn capability_all_ten_verbs_can_be_registered() {
        let mut reg = CapabilityRegistry::new();
        let verbs = vec![
            CapabilityVerb::Sense,
            CapabilityVerb::Reason,
            CapabilityVerb::Simulate,
            CapabilityVerb::Trade,
            CapabilityVerb::Settle,
            CapabilityVerb::Hedge,
            CapabilityVerb::Transfer,
            CapabilityVerb::Purchase,
            CapabilityVerb::Create,
            CapabilityVerb::Operate,
        ];

        for (i, verb) in verbs.iter().enumerate() {
            let cap = CapabilityEntry {
                verb: *verb,
                implementations: vec![format!("impl_{}", i)],
                confidence_level: 0.8 + (i as f64 * 0.01),
                eligibility: "test".to_string(),
            };
            reg.add_capability(cap).unwrap();
        }

        assert_eq!(reg.capability_count(), 10);
        assert_eq!(reg.all_verbs().len(), 10);
    }
}
