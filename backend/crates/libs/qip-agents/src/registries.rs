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
use std::collections::BTreeMap;

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

    /// Register a type. Refuses a spec that fails its own validation (a
    /// version of zero would be a stored type carrying no version, which is
    /// the ambiguity the registry exists to remove), refuses the exact same
    /// (family, name, version) twice, and refuses a version not above the
    /// latest already held for that name. The last refusal matters: without
    /// it, registering v1 after v2 silently moved `get_latest` back to v1, so
    /// a late-arriving old definition replaced the current one.
    pub fn register(&mut self, spec: OntologyTypeSpec) -> Result<()> {
        spec.validate()?;
        let key = (spec.family, spec.name.clone(), spec.version);

        if self.types.contains_key(&key) {
            return Err(Error::invalid(format!(
                "OntologyTypeSpec {:?}.{} version {} already registered; register a higher version",
                spec.family, spec.name, spec.version
            )));
        }

        let name_key = (spec.family, spec.name.clone());
        if let Some(&latest) = self.latest_version_per_name.get(&name_key)
            && spec.version <= latest
        {
            return Err(Error::invalid(format!(
                "OntologyTypeSpec {:?}.{} version {} is not above the latest registered version {latest}; register version {} or higher",
                spec.family,
                spec.name,
                spec.version,
                latest.saturating_add(1)
            )));
        }
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

    /// Add or update a capability and bump the version. Refuses an entry that
    /// fails its own validation — an empty implementation list or a
    /// confidence outside [0, 1] would otherwise be recorded as a capability
    /// the platform has.
    pub fn add_capability(&mut self, entry: CapabilityEntry) -> Result<()> {
        entry.validate()?;
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
    use std::collections::BTreeSet;

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
        // Reading at the old version must return the old definition, not
        // merely something: a registry that overwrote v1 with v2 under both
        // keys would pass an `is_some` check.
        let v1 = reg.get_type(OntologyTypeFamily::Relationship, "Owns", 1);
        assert_eq!(v1.map(|s| s.schema), Some(r#"{"v": 1}"#.to_string()));
        let v2 = reg.get_type(OntologyTypeFamily::Relationship, "Owns", 2);
        assert_eq!(v2.map(|s| s.schema), Some(r#"{"v": 2}"#.to_string()));
    }

    #[test]
    fn ontology_refuses_a_type_with_version_zero() {
        let mut reg = OntologyRegistry::new();
        let spec = OntologyTypeSpec {
            family: OntologyTypeFamily::Asset,
            name: "Bond".to_string(),
            version: 0,
            schema: "{}".to_string(),
            description: "unversioned".to_string(),
        };
        let err = reg.register(spec);
        assert!(
            matches!(&err, Err(e) if e.to_string().contains("version must be at least 1")),
            "a stored type must carry a version: {err:?}"
        );
        assert_eq!(reg.type_count(), 0);
    }

    #[test]
    fn ontology_refuses_an_older_version_after_a_newer_one_and_latest_stays_put() {
        let mut reg = OntologyRegistry::new();
        let make = |version: u32| OntologyTypeSpec {
            family: OntologyTypeFamily::Product,
            name: "Swap".to_string(),
            version,
            schema: format!("{{\"v\": {version}}}"),
            description: "swap".to_string(),
        };
        reg.register(make(2)).unwrap();
        let err = reg.register(make(1));
        assert!(
            matches!(&err, Err(e) if e.to_string().contains("not above the latest registered version 2")),
            "a late older version must be refused: {err:?}"
        );
        let latest = reg.get_latest(OntologyTypeFamily::Product, "Swap");
        assert_eq!(latest.map(|s| s.version), Some(2));
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
        let families = [
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
    fn capability_refuses_an_entry_with_no_implementations_and_keeps_its_version() {
        let mut reg = CapabilityRegistry::new();
        let cap = CapabilityEntry {
            verb: CapabilityVerb::Transfer,
            implementations: vec![],
            confidence_level: 0.5,
            eligibility: "test".to_string(),
        };
        let err = reg.add_capability(cap);
        assert!(
            matches!(&err, Err(e) if e.to_string().contains("implementations")),
            "a capability with no implementation is not a capability: {err:?}"
        );
        assert_eq!(reg.current_version(), 1);
        assert_eq!(reg.get_capability(CapabilityVerb::Transfer), None);
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
        let verbs = [
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
