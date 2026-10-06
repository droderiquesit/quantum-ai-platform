//! Opportunity definitions that are evaluated by the strategy runtime against
//! live local state.
//!
//! An opportunity definition is a condition that, when true, raises an
//! opportunity. Definitions are shipped in the policy payload and evaluated
//! on every pass by the strategy runtime.

use qip_contracts::{FeatureKey, OpportunityId};
use std::collections::BTreeMap;

/// An opportunity definition that can be evaluated against a feature vector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpportunityDefinition {
    /// The unique ID for this opportunity.
    id: OpportunityId,
    /// The features this definition requires to be present in the vector.
    required_features: Vec<FeatureKey>,
}

impl OpportunityDefinition {
    /// Create a new opportunity definition.
    pub fn new(id: OpportunityId, required_features: Vec<FeatureKey>) -> Self {
        Self {
            id,
            required_features,
        }
    }

    /// The opportunity ID.
    pub const fn id(&self) -> &OpportunityId {
        &self.id
    }

    /// The features required by this definition.
    pub fn required_features(&self) -> &[FeatureKey] {
        &self.required_features
    }

    /// Check if all required features are present in a feature vector.
    pub fn is_satisfiable(&self, vector: &qip_contracts::FeatureVector) -> bool {
        self.required_features
            .iter()
            .all(|key| vector.get(key).is_some())
    }
}

/// A set of opportunity definitions keyed by ID.
#[derive(Clone, Debug, Default)]
pub struct OpportunityCatalogue {
    definitions: BTreeMap<OpportunityId, OpportunityDefinition>,
}

impl OpportunityCatalogue {
    /// Create a new opportunity catalogue.
    pub fn new() -> Self {
        Self {
            definitions: BTreeMap::new(),
        }
    }

    /// Add an opportunity definition to the catalogue.
    pub fn add(&mut self, definition: OpportunityDefinition) {
        self.definitions.insert(definition.id().clone(), definition);
    }

    /// Get an opportunity definition by ID.
    pub fn get(&self, id: &OpportunityId) -> Option<&OpportunityDefinition> {
        self.definitions.get(id)
    }

    /// Iterate over all definitions.
    pub fn iter(&self) -> impl Iterator<Item = &OpportunityDefinition> {
        self.definitions.values()
    }

    /// The number of definitions in the catalogue.
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Whether the catalogue is empty.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::FeatureVector;
    use qip_core::ObjectId;

    #[test]
    fn an_opportunity_definition_can_be_created_and_retrieved() {
        let subject = ObjectId::from_string("OBJ00000000000000000000AAA");
        let key = FeatureKey::new("test_feature", subject.clone());
        let opp_id = OpportunityId::new("test_opportunity");

        let def = OpportunityDefinition::new(opp_id.clone(), vec![key.clone()]);
        assert_eq!(def.id(), &opp_id);
        assert_eq!(def.required_features(), &[key]);
    }

    #[test]
    fn an_opportunity_catalogue_can_store_and_retrieve_definitions() {
        let subject = ObjectId::from_string("OBJ00000000000000000000BBB");
        let key = FeatureKey::new("feature_a", subject.clone());
        let opp_id = OpportunityId::new("opportunity_a");

        let def = OpportunityDefinition::new(opp_id.clone(), vec![key]);
        let mut catalogue = OpportunityCatalogue::new();

        catalogue.add(def);
        assert_eq!(catalogue.len(), 1);
        assert!(catalogue.get(&opp_id).is_some());
        assert_eq!(catalogue.get(&opp_id).unwrap().id(), &opp_id);
    }

    #[test]
    fn an_opportunity_is_satisfiable_when_all_required_features_are_present() {
        let subject = ObjectId::from_string("OBJ00000000000000000000CCC");
        let key = FeatureKey::new("feature_b", subject.clone());
        let opp_id = OpportunityId::new("opportunity_b");

        let def = OpportunityDefinition::new(opp_id, vec![key.clone()]);

        let mut vector = FeatureVector::new(Default::default());
        vector.insert(
            key.clone(),
            qip_contracts::FeatureValue::Flag(true),
            Default::default(),
        );

        assert!(def.is_satisfiable(&vector));
    }

    #[test]
    fn an_opportunity_is_not_satisfiable_when_a_required_feature_is_missing() {
        let subject = ObjectId::from_string("OBJ00000000000000000000DDD");
        let key = FeatureKey::new("feature_c", subject.clone());
        let opp_id = OpportunityId::new("opportunity_c");

        let def = OpportunityDefinition::new(opp_id, vec![key]);
        let vector = FeatureVector::new(Default::default());

        assert!(!def.is_satisfiable(&vector));
    }

    #[test]
    fn a_catalogue_can_store_and_retrieve_multiple_definitions() {
        let subject = ObjectId::from_string("OBJ00000000000000000000EEE");
        let key = FeatureKey::new("feature_d", subject.clone());
        let opp_id1 = OpportunityId::new("opportunity_d1");
        let opp_id2 = OpportunityId::new("opportunity_d2");

        let def1 = OpportunityDefinition::new(opp_id1.clone(), vec![key.clone()]);
        let def2 = OpportunityDefinition::new(opp_id2.clone(), vec![key]);

        let mut catalogue = OpportunityCatalogue::new();
        catalogue.add(def1);
        catalogue.add(def2);

        assert_eq!(catalogue.len(), 2);
        assert!(catalogue.get(&opp_id1).is_some());
        assert!(catalogue.get(&opp_id2).is_some());
    }

    #[test]
    fn an_empty_catalogue_is_empty() {
        let catalogue = OpportunityCatalogue::new();
        assert_eq!(catalogue.len(), 0);
        assert!(catalogue.is_empty());
    }
}
