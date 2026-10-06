//! Knowledge inputs to the agency engine from arbitration and specialists.
//!
//! The Causal Agency engine consumes knowledge from the Meta/Arbitration Brain
//! and the Specialist Brain Society, recording which inputs informed each plan.
//! A KnowledgeInput represents either an arbitration output (world-model belief)
//! or a specialist finding (expert opinion), identified by source and kind.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Source of a knowledge input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum KnowledgeSource {
    /// World-model arbitration output from the Meta/Arbitration Brain.
    Arbitration,
    /// Finding from a specialist agent in the Specialist Brain Society.
    Specialist,
}

/// A typed knowledge message for the agency engine.
/// Represents either an arbitrated world-model belief or a specialist finding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct KnowledgeInput {
    /// Source: arbitration or specialist.
    pub source: KnowledgeSource,
    /// Identifier for the input (e.g., "specialist:chief" or "arbitration:covariance").
    pub id: String,
    /// Brief description of the knowledge.
    pub content: String,
}

impl KnowledgeInput {
    /// Create a knowledge input from arbitration.
    pub fn from_arbitration(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            source: KnowledgeSource::Arbitration,
            id: id.into(),
            content: content.into(),
        }
    }

    /// Create a knowledge input from a specialist.
    pub fn from_specialist(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            source: KnowledgeSource::Specialist,
            id: id.into(),
            content: content.into(),
        }
    }
}

/// A log of which knowledge inputs informed a plan.
/// Tracks which arbitration outputs and specialist findings were used
/// to create an InterventionPlan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeLog {
    /// Set of knowledge input IDs used.
    pub used_inputs: BTreeSet<String>,
}

impl KnowledgeLog {
    /// Create an empty knowledge log.
    pub fn new() -> Self {
        Self {
            used_inputs: BTreeSet::new(),
        }
    }

    /// Record that a knowledge input was used.
    pub fn record(&mut self, input: &KnowledgeInput) {
        self.used_inputs.insert(input.id.clone());
    }

    /// Record multiple knowledge inputs.
    pub fn record_batch(&mut self, inputs: &[KnowledgeInput]) {
        for input in inputs {
            self.record(input);
        }
    }

    /// Check if any knowledge was recorded.
    pub fn has_inputs(&self) -> bool {
        !self.used_inputs.is_empty()
    }

    /// Get the recorded input IDs.
    pub fn input_ids(&self) -> &BTreeSet<String> {
        &self.used_inputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knowledge_input_from_arbitration() {
        let input = KnowledgeInput::from_arbitration("arb:covariance", "A and B covariate");
        assert_eq!(input.source, KnowledgeSource::Arbitration);
        assert_eq!(input.id, "arb:covariance");
        assert_eq!(input.content, "A and B covariate");
    }

    #[test]
    fn knowledge_input_from_specialist() {
        let input = KnowledgeInput::from_specialist("specialist:chief", "Inflation risk rising");
        assert_eq!(input.source, KnowledgeSource::Specialist);
        assert_eq!(input.id, "specialist:chief");
    }

    #[test]
    fn knowledge_log_records_inputs() {
        let mut log = KnowledgeLog::new();
        assert!(!log.has_inputs());

        let input1 = KnowledgeInput::from_arbitration("arb:1", "content1");
        let input2 = KnowledgeInput::from_specialist("spec:1", "content2");

        log.record(&input1);
        assert!(log.has_inputs());
        assert!(log.input_ids().contains("arb:1"));

        log.record(&input2);
        assert_eq!(log.input_ids().len(), 2);
    }

    #[test]
    fn knowledge_log_deduplicates() {
        let mut log = KnowledgeLog::new();
        let input = KnowledgeInput::from_arbitration("arb:1", "content");

        log.record(&input);
        log.record(&input);
        log.record(&input);

        assert_eq!(log.input_ids().len(), 1);
    }

    #[test]
    fn knowledge_log_batch_record() {
        let mut log = KnowledgeLog::new();
        let inputs = vec![
            KnowledgeInput::from_arbitration("arb:1", "content1"),
            KnowledgeInput::from_specialist("spec:1", "content2"),
            KnowledgeInput::from_arbitration("arb:2", "content3"),
        ];

        log.record_batch(&inputs);
        assert_eq!(log.input_ids().len(), 3);
    }
}
