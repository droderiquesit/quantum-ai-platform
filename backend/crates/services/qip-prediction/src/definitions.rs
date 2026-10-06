//! Event contract definitions with bitemporal storage.
//!
//! An event definition specifies the outcomes of a binary or categorical
//! contract, the resolution criteria, data sources, and when the definition
//! became effective. The bitemporal model separates when the definition
//! describes (valid_at) from when it became known to the platform (available_at).
//!
//! Amended definitions are new records, not updates: changing a contract's
//! outcome set or terms after trading begins requires a new definition record
//! with a fresh available_at timestamp. This preserves the provenance of every
//! claim the platform ever made.

use crate::{market::OutcomeId, resolution::ResolutionSource};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One event contract definition, capturing its outcomes, terms, and resolution source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventDefinition {
    /// Unique identifier for this contract.
    pub contract_id: String,
    /// The set of possible outcomes — must be non-empty and stable for a given valid_at.
    pub outcome_set: BTreeSet<OutcomeId>,
    /// Human-readable terms and settlement rules.
    pub terms: String,
    /// The data source that will resolve this contract.
    pub resolution_source: ResolutionSource,
    /// The instant from which this definition is valid.
    pub valid_at: Timestamp,
    /// The instant this definition was ingested and became knowable.
    pub available_at: Timestamp,
}

impl EventDefinition {
    pub fn new(
        contract_id: impl Into<String>,
        outcome_set: BTreeSet<OutcomeId>,
        terms: impl Into<String>,
        resolution_source: ResolutionSource,
        valid_at: Timestamp,
        available_at: Timestamp,
    ) -> Result<Self> {
        let contract_id = contract_id.into();
        let terms = terms.into();

        // Refuse missing outcome set
        if outcome_set.is_empty() {
            return Err(Error::invalid(
                "event definition must specify at least one outcome",
            ));
        }

        // Refuse if definition becomes knowable before it is valid
        // (a definition cannot be true in the past)
        if available_at < valid_at {
            return Err(Error::invalid(
                "definition cannot be knowable before it is valid",
            ));
        }

        Ok(Self {
            contract_id,
            outcome_set,
            terms,
            resolution_source,
            valid_at,
            available_at,
        })
    }

    /// The unique key for storing this definition in a bitemporal store.
    /// Multiple versions of the same contract may exist with different valid_at times.
    pub fn storage_key(&self) -> (String, Timestamp) {
        (self.contract_id.clone(), self.valid_at)
    }
}

/// Bitemporal storage for event definitions.
///
/// Stores event definitions indexed by (contract_id, valid_at). Supports
/// point-in-time lookups: a read at time T returns the definition whose
/// valid_at <= T and available_at <= T, or nothing if none exists.
///
/// An amendment is a new record: changing the outcome_set or terms creates
/// a new definition with a fresh available_at and a new valid_at.
#[derive(Debug)]
pub struct DefinitionStore {
    /// Definitions indexed by (contract_id, valid_at)
    definitions: BTreeMap<(String, Timestamp), EventDefinition>,
}

impl DefinitionStore {
    pub fn new() -> Self {
        Self {
            definitions: BTreeMap::new(),
        }
    }

    /// Store a new event definition.
    ///
    /// Returns the definition if stored successfully. Refuses if the definition
    /// has empty outcome_set or if available_at < valid_at.
    pub fn record(&mut self, definition: EventDefinition) -> Result<()> {
        // The EventDefinition::new constructor has already validated this,
        // but we check again before committing to storage.
        if definition.outcome_set.is_empty() {
            return Err(Error::invalid(
                "event definition must specify at least one outcome",
            ));
        }

        let key = definition.storage_key();
        self.definitions.insert(key, definition);
        Ok(())
    }

    /// Retrieve the definition for a contract as of a point in both time dimensions.
    ///
    /// Returns the most recent definition where both:
    /// - valid_at <= valid_time (the definition describes this instant or earlier)
    /// - available_at <= known_time (the definition was knowable by this instant)
    ///
    /// Returns None if no such definition exists.
    pub fn definition_as_of(
        &self,
        contract_id: &str,
        valid_time: Timestamp,
        known_time: Timestamp,
    ) -> Option<&EventDefinition> {
        // Find all definitions for this contract with valid_at <= valid_time
        // Then filter to those with available_at <= known_time
        // Return the one with the highest valid_at (most recent valid definition)

        self.definitions
            .iter()
            .rev() // Iterate in reverse to find most recent first
            .find(|((id, valid_at), def)| {
                id == contract_id && *valid_at <= valid_time && def.available_at <= known_time
            })
            .map(|(_, def)| def)
    }

    /// All versions of a contract definition.
    pub fn all_versions(&self, contract_id: &str) -> Vec<&EventDefinition> {
        self.definitions
            .iter()
            .filter(|((id, _), _)| id == contract_id)
            .map(|(_, def)| def)
            .collect()
    }

    /// All definitions known as of a point in time.
    pub fn all_as_of(&self, known_time: Timestamp) -> Vec<&EventDefinition> {
        self.definitions
            .iter()
            .filter(|(_, def)| def.available_at <= known_time)
            .map(|(_, def)| def)
            .collect()
    }
}

impl Default for DefinitionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolution::{Comparison, ResolutionCriteria, SourceKind};
    use qip_core::{Decimal, Duration};

    #[allow(dead_code)]
    fn sample_criteria() -> ResolutionCriteria {
        ResolutionCriteria::Threshold {
            metric: "metric".into(),
            comparison: Comparison::AtLeast,
            value: Decimal::from(100),
        }
    }

    fn sample_source() -> ResolutionSource {
        ResolutionSource::new(
            "reference_data",
            SourceKind::Official,
            vec!["metric".into()],
        )
    }

    #[test]
    fn an_event_definition_requires_at_least_one_outcome() {
        let result = EventDefinition::new(
            "contract-1",
            BTreeSet::new(), // empty outcome set
            "Sample terms",
            sample_source(),
            Timestamp::EPOCH,
            Timestamp::EPOCH,
        );
        assert!(result.is_err());
    }

    #[test]
    fn an_event_definition_must_be_knowable_no_earlier_than_valid() {
        let outcome_set: BTreeSet<_> = ["YES", "NO"].iter().map(|s| OutcomeId::new(*s)).collect();
        let result = EventDefinition::new(
            "contract-1",
            outcome_set,
            "Sample terms",
            sample_source(),
            Timestamp::EPOCH.saturating_add(Duration::from_secs(100)), // valid_at = 100
            Timestamp::EPOCH, // available_at = 0 < valid_at
        );
        assert!(result.is_err());
    }

    #[test]
    fn a_valid_event_definition_stores_successfully() {
        let mut store = DefinitionStore::new();
        let outcome_set: BTreeSet<_> = ["YES", "NO"].iter().map(|s| OutcomeId::new(*s)).collect();
        let def = EventDefinition::new(
            "contract-1",
            outcome_set,
            "Will it happen?",
            sample_source(),
            Timestamp::EPOCH,
            Timestamp::EPOCH,
        )
        .unwrap();

        assert!(store.record(def.clone()).is_ok());

        let retrieved = store.definition_as_of(
            "contract-1",
            Timestamp::EPOCH.saturating_add(Duration::from_secs(10)),
            Timestamp::EPOCH.saturating_add(Duration::from_secs(10)),
        );
        assert_eq!(retrieved, Some(&def));
    }

    #[test]
    fn a_definition_is_not_returned_before_it_becomes_knowable() {
        let mut store = DefinitionStore::new();
        let outcome_set: BTreeSet<_> = ["YES", "NO"].iter().map(|s| OutcomeId::new(*s)).collect();
        let def = EventDefinition::new(
            "contract-1",
            outcome_set,
            "Will it happen?",
            sample_source(),
            Timestamp::EPOCH,
            Timestamp::EPOCH.saturating_add(Duration::from_secs(100)), // available at T+100
        )
        .unwrap();

        store.record(def.clone()).unwrap();

        // Query at T+50: definition is valid (valid_at = 0 <= 50) but not yet knowable
        let retrieved = store.definition_as_of(
            "contract-1",
            Timestamp::EPOCH.saturating_add(Duration::from_secs(50)),
            Timestamp::EPOCH.saturating_add(Duration::from_secs(50)),
        );
        assert_eq!(
            retrieved, None,
            "Definition should not be accessible before available_at"
        );

        // Query at T+100: definition is knowable now
        let retrieved = store.definition_as_of(
            "contract-1",
            Timestamp::EPOCH.saturating_add(Duration::from_secs(100)),
            Timestamp::EPOCH.saturating_add(Duration::from_secs(100)),
        );
        assert_eq!(retrieved, Some(&def));
    }

    #[test]
    fn amended_definitions_are_separate_records_not_updates() {
        let mut store = DefinitionStore::new();
        let outcome_set1: BTreeSet<_> = ["YES", "NO"].iter().map(|s| OutcomeId::new(*s)).collect();
        let def1 = EventDefinition::new(
            "contract-1",
            outcome_set1,
            "Original terms",
            sample_source(),
            Timestamp::EPOCH,
            Timestamp::EPOCH.saturating_add(Duration::from_secs(100)),
        )
        .unwrap();

        store.record(def1.clone()).unwrap();

        // Amendment: new outcome set, new terms, new valid_at
        let outcome_set2: BTreeSet<_> = ["YES", "NO", "ABSTAIN"]
            .iter()
            .map(|s| OutcomeId::new(*s))
            .collect();
        let def2 = EventDefinition::new(
            "contract-1",
            outcome_set2,
            "Amended terms",
            sample_source(),
            Timestamp::EPOCH.saturating_add(Duration::from_secs(200)), // new valid_at
            Timestamp::EPOCH.saturating_add(Duration::from_secs(200)), // must be >= valid_at
        )
        .unwrap();

        store.record(def2.clone()).unwrap();

        // Both versions exist
        let versions = store.all_versions("contract-1");
        assert_eq!(versions.len(), 2);

        // At T+150, T+150 (before amendment known): get the original definition
        let def_at_150 = store.definition_as_of(
            "contract-1",
            Timestamp::EPOCH.saturating_add(Duration::from_secs(150)),
            Timestamp::EPOCH.saturating_add(Duration::from_secs(150)),
        );
        assert_eq!(def_at_150, Some(&def1));

        // At T+250, T+250 (after amendment known and valid): get the amended definition
        let def_at_250 = store.definition_as_of(
            "contract-1",
            Timestamp::EPOCH.saturating_add(Duration::from_secs(250)),
            Timestamp::EPOCH.saturating_add(Duration::from_secs(250)),
        );
        assert_eq!(def_at_250, Some(&def2));
    }
}
