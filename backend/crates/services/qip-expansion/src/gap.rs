//! Steps 1 and 2 of the expansion loop: a detection becomes exactly one
//! [`GapSignal`] naming its trigger (EXPAND-025) and carrying one of the nine
//! gap classes (EXPAND-026).
//!
//! The failure prevented is a default class. A gap filed as "data" because
//! nobody could say what was missing sends the research to the wrong desk and
//! reads afterwards as a data problem that was investigated and not found.

use qip_contracts::expansion::{GapClass, GapSignal};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// The seven things that open the loop (§23.1 step 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapTrigger {
    Surprise,
    LowConfidence,
    Contradiction,
    MissedOpportunity,
    UnexplainedPnl,
    ModelDrift,
    UnsupportedRequest,
}

impl GapTrigger {
    pub const ALL: [Self; 7] = [
        Self::Surprise,
        Self::LowConfidence,
        Self::Contradiction,
        Self::MissedOpportunity,
        Self::UnexplainedPnl,
        Self::ModelDrift,
        Self::UnsupportedRequest,
    ];
}

/// What one piece of evidence shows the platform to be missing.
///
/// `Unknown` is a legitimate thing for a detector to say: it saw the symptom
/// and cannot name the lack. It is refused by [`classify`], never mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wanting {
    /// A feed, document family or sensor is absent, stale or too thin.
    Source,
    /// No entity, event or relationship type exists for what was observed.
    Type,
    /// A causal link is missing or was contradicted.
    CausalLink,
    /// The record of past episodes is too thin to estimate from.
    Episodes,
    /// A model is absent, decayed or miscalibrated.
    Model,
    Tool,
    Specialist,
    /// Fills, routing or venue behaviour fell short of what was planned.
    Execution,
    /// The question is answerable but not within the compute available.
    Compute,
    Unknown,
}

/// The one class every piece of evidence agrees on.
///
/// Refuses evidence that names nothing, names `Unknown`, or points at two
/// different lacks: a gap that is both a data gap and a model gap is two
/// signals, and picking the first would hide the second.
pub fn classify(wanting: &[Wanting]) -> Result<GapClass> {
    let Some((first, rest)) = wanting.split_first() else {
        return Err(Error::invalid(
            "a gap with no statement of what is missing cannot be classified; say what the evidence shows is wanting",
        ));
    };
    if let Some(other) = rest.iter().find(|w| *w != first) {
        return Err(Error::invalid(format!(
            "the evidence points at two different lacks ({first:?} and {other:?}); raise one signal for each"
        )));
    }
    Ok(match first {
        Wanting::Source => GapClass::Data,
        Wanting::Type => GapClass::Ontology,
        Wanting::CausalLink => GapClass::Causal,
        Wanting::Episodes => GapClass::Memory,
        Wanting::Model => GapClass::Model,
        Wanting::Tool => GapClass::Tool,
        Wanting::Specialist => GapClass::Specialist,
        Wanting::Execution => GapClass::Execution,
        Wanting::Compute => GapClass::ComputeQuantumResearch,
        Wanting::Unknown => {
            return Err(Error::invalid(
                "the detector could not say what is missing, and no class is assumed for it; investigate the observation before raising it",
            ));
        }
    })
}

/// What a detector saw, before it is a signal.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub trigger: GapTrigger,
    /// What each piece of evidence shows to be missing.
    pub wanting: Vec<Wanting>,
    pub observation: String,
    pub affected_domains: Vec<String>,
    pub evidence: Vec<String>,
    /// Severity in `[0, 1]`, as the detector measured it.
    pub severity: f64,
    pub economic_value: Decimal,
}

/// A classified signal and the trigger that raised it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RaisedGap {
    pub trigger: GapTrigger,
    pub signal: GapSignal,
}

/// One observation in, exactly one classified signal out, or a refusal.
pub fn raise(observation: Observation) -> Result<RaisedGap> {
    let signal = GapSignal {
        gap_class: classify(&observation.wanting)?,
        observation: observation.observation,
        affected_domains: observation.affected_domains,
        evidence: observation.evidence,
        severity: observation.severity,
        economic_value: observation.economic_value,
    };
    signal.validate()?;
    Ok(RaisedGap {
        trigger: observation.trigger,
        signal,
    })
}
