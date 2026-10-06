//! Section 27 goal-to-effect contracts: `GoalSpec`, `InterventionPlan`,
//! `EffectAttribution` (CONTRACT-027/028/030) and the self-evaluation pair
//! `GapSignal`, `CurriculumItem` (CONTRACT-031/032).
//!
//! These are shadow-only records. Nothing here executes an action; an action
//! a plan names goes through the intent path where the paper-trading boundary
//! binds, and `AuthorityClass` has no live arm so a goal cannot even ask.
//!
//! Every list a record must carry is refused when empty. A blank list reads as
//! "considered and found nothing" when it usually means "not considered", so a
//! writer who means the former must say so in words.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

fn text(record: &str, field: &str, v: &str) -> Result<()> {
    if v.trim().is_empty() {
        return Err(Error::invalid(format!(
            "{record}.{field} is blank; state it rather than leave it out"
        )));
    }
    Ok(())
}

fn list(record: &str, field: &str, v: &[String]) -> Result<()> {
    if v.is_empty() || v.iter().any(|s| s.trim().is_empty()) {
        return Err(Error::invalid(format!(
            "{record}.{field} is empty or holds a blank entry; name at least one, or say none apply in words"
        )));
    }
    Ok(())
}

fn positive(record: &str, field: &str, v: Decimal) -> Result<()> {
    if !v.is_positive() {
        return Err(Error::invalid(format!(
            "{record}.{field} must be greater than zero"
        )));
    }
    Ok(())
}

fn finite(record: &str, field: &str, v: f64, lo: f64, hi: f64) -> Result<()> {
    if !v.is_finite() || v < lo || v > hi {
        return Err(Error::invalid(format!(
            "{record}.{field} must be finite and within [{lo}, {hi}]"
        )));
    }
    Ok(())
}

/// A distribution summary. Statistics, so `f64` (core-rust rule).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effect {
    pub mean: f64,
    pub std_dev: f64,
}

impl Effect {
    fn validate(&self, record: &str, field: &str) -> Result<()> {
        finite(record, field, self.mean, f64::MIN, f64::MAX)?;
        finite(record, field, self.std_dev, 0.0, f64::MAX)
    }
}

/// What a goal may drive. No live arm exists: ADR 0003.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityClass {
    Observe,
    Shadow,
    PaperTrading,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalSpec {
    pub desired_state: String,
    pub target_entities: Vec<String>,
    pub horizon_ms: u64,
    pub success_metric: String,
    pub budget: Decimal,
    pub risk_constraints: Vec<String>,
    pub identity: String,
    pub jurisdiction: String,
    pub authority_class: AuthorityClass,
    pub prohibited_methods: Vec<String>,
    pub stop_rollback_conditions: Vec<String>,
}

impl GoalSpec {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "GoalSpec";
        text(R, "desired_state", &self.desired_state)?;
        list(R, "target_entities", &self.target_entities)?;
        if self.horizon_ms == 0 {
            return Err(Error::invalid("GoalSpec.horizon_ms must be above zero"));
        }
        text(R, "success_metric", &self.success_metric)?;
        positive(R, "budget", self.budget)?;
        list(R, "risk_constraints", &self.risk_constraints)?;
        text(R, "identity", &self.identity)?;
        text(R, "jurisdiction", &self.jurisdiction)?;
        list(R, "prohibited_methods", &self.prohibited_methods)?;
        list(
            R,
            "stop_rollback_conditions",
            &self.stop_rollback_conditions,
        )
    }

    /// Strict decode: unknown or missing fields, then `validate`.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let v: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::schema(format!("not a GoalSpec: {e}; supply every field")))?;
        v.validate()?;
        Ok(v)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    Reversible,
    PartlyReversible,
    Irreversible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterventionPlan {
    pub causal_hypothesis: String,
    pub levers: Vec<String>,
    /// Ordered; a conditional action carries its condition in its text.
    pub candidate_actions: Vec<String>,
    pub tool_ids: Vec<String>,
    pub expected_effect: Effect,
    pub cost: Decimal,
    pub timing_ms: u64,
    pub reversibility: Reversibility,
    pub simulations_run: Vec<String>,
    pub no_action_baseline: String,
    pub required_approvals: Vec<String>,
    pub abort_rules: Vec<String>,
}

impl InterventionPlan {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "InterventionPlan";
        text(R, "causal_hypothesis", &self.causal_hypothesis)?;
        list(R, "levers", &self.levers)?;
        list(R, "candidate_actions", &self.candidate_actions)?;
        list(R, "tool_ids", &self.tool_ids)?;
        self.expected_effect.validate(R, "expected_effect")?;
        if self.cost.is_negative() {
            return Err(Error::invalid("InterventionPlan.cost must not be negative"));
        }
        if self.timing_ms == 0 {
            return Err(Error::invalid(
                "InterventionPlan.timing_ms must be above zero",
            ));
        }
        list(R, "simulations_run", &self.simulations_run)?;
        text(R, "no_action_baseline", &self.no_action_baseline)?;
        list(R, "required_approvals", &self.required_approvals)?;
        list(R, "abort_rules", &self.abort_rules)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let v: Self = serde_json::from_slice(bytes).map_err(|e| {
            Error::schema(format!("not an InterventionPlan: {e}; supply every field"))
        })?;
        v.validate()?;
        Ok(v)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HorizonObservation {
    pub horizon_ms: u64,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectAttribution {
    pub goal_id: String,
    pub intervention_id: String,
    pub action_id: String,
    pub observations: Vec<HorizonObservation>,
    pub counterfactual_method: String,
    pub effect: Effect,
    pub confounders: Vec<String>,
    pub side_effects: Vec<String>,
    /// Identifiability in `[0, 1]`.
    pub identifiability: f64,
    pub resulting_updates: Vec<String>,
}

impl EffectAttribution {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "EffectAttribution";
        text(R, "goal_id", &self.goal_id)?;
        text(R, "intervention_id", &self.intervention_id)?;
        text(R, "action_id", &self.action_id)?;
        if self.observations.is_empty() {
            return Err(Error::invalid(
                "EffectAttribution.observations is empty; an effect needs at least one horizon observed",
            ));
        }
        for o in &self.observations {
            if o.horizon_ms == 0 || !o.value.is_finite() {
                return Err(Error::invalid(
                    "EffectAttribution.observations holds a zero horizon or non-finite value",
                ));
            }
        }
        text(R, "counterfactual_method", &self.counterfactual_method)?;
        self.effect.validate(R, "effect")?;
        list(R, "confounders", &self.confounders)?;
        list(R, "side_effects", &self.side_effects)?;
        finite(R, "identifiability", self.identifiability, 0.0, 1.0)?;
        list(R, "resulting_updates", &self.resulting_updates)
    }

    /// Refuses an attribution naming a goal, intervention or action that does
    /// not exist, so an effect is never credited to something never planned.
    pub fn validate_references(
        &self,
        goals: &BTreeSet<String>,
        interventions: &BTreeSet<String>,
        actions: &BTreeSet<String>,
    ) -> Result<()> {
        self.validate()?;
        for (kind, id, known) in [
            ("goal", &self.goal_id, goals),
            ("intervention", &self.intervention_id, interventions),
            ("action", &self.action_id, actions),
        ] {
            if !known.contains(id) {
                return Err(Error::invalid(format!(
                    "EffectAttribution names {kind} {id:?}, which does not exist; record it first"
                )));
            }
        }
        Ok(())
    }
}

/// The section 23.1 gap classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapClass {
    Data,
    Ontology,
    Causal,
    Memory,
    Model,
    Tool,
    Specialist,
    Execution,
    ComputeQuantumResearch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GapSignal {
    pub observation: String,
    pub affected_domains: Vec<String>,
    pub evidence: Vec<String>,
    /// Severity in `[0, 1]`.
    pub severity: f64,
    pub economic_value: Decimal,
    pub gap_class: GapClass,
}

impl GapSignal {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "GapSignal";
        text(R, "observation", &self.observation)?;
        list(R, "affected_domains", &self.affected_domains)?;
        list(R, "evidence", &self.evidence)?;
        finite(R, "severity", self.severity, 0.0, 1.0)?;
        if self.economic_value.is_negative() {
            return Err(Error::invalid(
                "GapSignal.economic_value must not be negative",
            ));
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let v: Self = serde_json::from_slice(bytes).map_err(|e| {
            Error::schema(format!(
                "not a GapSignal: {e}; supply every field and one of the section 23.1 classes"
            ))
        })?;
        v.validate()?;
        Ok(v)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurriculumItem {
    pub research_question: String,
    pub expected_information_value: f64,
    pub expected_economic_value: Decimal,
    pub tools_and_data: Vec<String>,
    pub budget: Decimal,
    pub owner: String,
    pub evaluation_suite: String,
    pub stop_conditions: Vec<String>,
}

impl CurriculumItem {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "CurriculumItem";
        text(R, "research_question", &self.research_question)?;
        finite(
            R,
            "expected_information_value",
            self.expected_information_value,
            0.0,
            f64::MAX,
        )?;
        if self.expected_economic_value.is_negative() {
            return Err(Error::invalid(
                "CurriculumItem.expected_economic_value must not be negative",
            ));
        }
        list(R, "tools_and_data", &self.tools_and_data)?;
        positive(R, "budget", self.budget)?;
        text(R, "owner", &self.owner)?;
        text(R, "evaluation_suite", &self.evaluation_suite)?;
        list(R, "stop_conditions", &self.stop_conditions)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let v: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::schema(format!("not a CurriculumItem: {e}; supply every field")))?;
        v.validate()?;
        Ok(v)
    }
}

/// The eight type families in an Ontology Registry (EXPAND-034).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OntologyTypeFamily {
    Entity,
    Event,
    Relationship,
    Market,
    Asset,
    Product,
    CausalDriver,
    Lifecycle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OntologyTypeSpec {
    pub family: OntologyTypeFamily,
    pub name: String,
    pub version: u32,
    pub schema: String,
    pub description: String,
}

impl OntologyTypeSpec {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "OntologyTypeSpec";
        text(R, "name", &self.name)?;
        text(R, "schema", &self.schema)?;
        text(R, "description", &self.description)?;
        if self.version == 0 {
            return Err(Error::invalid(
                "OntologyTypeSpec.version must be at least 1",
            ));
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let v: Self = serde_json::from_slice(bytes).map_err(|e| {
            Error::schema(format!("not an OntologyTypeSpec: {e}; supply every field"))
        })?;
        v.validate()?;
        Ok(v)
    }
}

/// The ten capability verbs in a Capability Registry (EXPAND-040).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityVerb {
    Sense,
    Reason,
    Simulate,
    Trade,
    Settle,
    Hedge,
    Transfer,
    Purchase,
    Create,
    Operate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityEntry {
    pub verb: CapabilityVerb,
    pub implementations: Vec<String>,
    pub confidence_level: f64,
    pub eligibility: String,
}

impl CapabilityEntry {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "CapabilityEntry";
        list(R, "implementations", &self.implementations)?;
        finite(R, "confidence_level", self.confidence_level, 0.0, 1.0)?;
        text(R, "eligibility", &self.eligibility)?;
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let v: Self = serde_json::from_slice(bytes).map_err(|e| {
            Error::schema(format!("not a CapabilityEntry: {e}; supply every field"))
        })?;
        v.validate()?;
        Ok(v)
    }
}
