//! Section 27 goal-to-effect contracts: `GoalSpec`, `InterventionPlan`,
//! `EffectAttribution` (CONTRACT-027/028/030) and the self-evaluation pair
//! `GapSignal`, `CurriculumItem` (CONTRACT-031/032).
//!
//! These are shadow-only records. Nothing here executes an action; an action
//! a plan names goes through the intent path where the paper-trading boundary
//! binds. A goal carries no authority at all, so it cannot even ask for a
//! live one: authority is the action policy's, and that starts at shadow.
//!
//! Every list a record must carry is refused when empty. A blank list reads as
//! "considered and found nothing" when it usually means "not considered", so a
//! writer who means the former must say so in words. The one exception is a
//! goal's two prohibition lists, which may be declared empty but never
//! omitted: the shipped `qip agency shadow` request document declares them
//! empty, and "nothing beyond the platform's own prohibitions" is a real
//! declaration there.

use crate::action::Method;
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

/// A required text declaration: present and not blank, else a refusal naming
/// it. The wording matches what `qip-agency` returned before the goal moved
/// here, because operators and tests read these messages.
fn declared_text(field: &str, value: Option<String>) -> Result<String> {
    match value {
        Some(v) if !v.trim().is_empty() => Ok(v),
        Some(_) => Err(Error::invalid(format!(
            "`{field}` is blank; state it explicitly"
        ))),
        None => Err(Error::invalid(format!(
            "`{field}` is not declared; it is required"
        ))),
    }
}

fn declared<T>(field: &str, value: Option<T>) -> Result<T> {
    value.ok_or_else(|| Error::invalid(format!("`{field}` is not declared; it is required")))
}

fn declared_list(field: &str, value: Option<Vec<String>>) -> Result<Vec<String>> {
    let list = declared(field, value)?;
    if list.is_empty() || list.iter().any(|s| s.trim().is_empty()) {
        return Err(Error::invalid(format!(
            "`{field}` must list at least one non-blank entry"
        )));
    }
    Ok(list)
}

/// The eight classes of objective a goal may express (AGENCY-041).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalClass {
    Financial,
    Operational,
    Research,
    Product,
    Liquidity,
    RiskReduction,
    InformationAcquisition,
    Communications,
}

/// Every declaration optional, so absence is a refusal naming the field and
/// not a deserialisation error naming nothing useful. Unknown fields are
/// refused: a goal document carrying a field nothing reads (an
/// `authority_class: "live"`, say) would otherwise be silently accepted, and
/// its author would believe the platform had read it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalSpecDraft {
    pub class: Option<GoalClass>,
    pub target_state: Option<String>,
    pub entities_affected: Option<Vec<String>>,
    pub time_horizon_secs: Option<u64>,
    pub success_metric: Option<String>,
    /// Largest acceptable uncertainty on the outcome, in `(0, 1]`.
    pub acceptable_uncertainty: Option<Decimal>,
    /// Strictly positive.
    pub budget: Option<Decimal>,
    /// Largest summed exposure a plan may carry.
    pub risk_envelope: Option<Decimal>,
    pub jurisdictions: Option<Vec<String>>,
    pub acting_identity: Option<String>,
    /// May be declared empty ("none"), but must be declared.
    pub prohibited_side_effects: Option<Vec<String>>,
    /// May be declared empty ("none"), but must be declared.
    pub prohibited_methods: Option<Vec<String>>,
    pub stop_conditions: Option<Vec<String>>,
    /// What undoes the goal's effects if a stop condition fires.
    pub rollback_conditions: Option<Vec<String>>,
}

/// CONTRACT-027 / AGENCY-003 / R12-AGENCY-6079: the one `GoalSpec`.
///
/// There were two: this crate's, with an `authority_class` and a single
/// jurisdiction but no acceptable uncertainty, risk envelope or prohibited
/// side effects, and `qip-agency`'s, which the engine actually planned
/// against. A goal validated by one was not a goal the other had checked.
/// This is `qip-agency`'s shape, moved here so both crates name one type, and
/// `qip_agency::goal` re-exports it.
///
/// `authority_class` is gone rather than merged. What a goal may drive is not
/// the goal's to declare: authority lives on `qip-agency`'s `ActionPolicy`,
/// which starts at shadow and has no live rung, and a document that still
/// carries the field is refused as unknown rather than read as a request.
///
/// `non_exhaustive`: outside this crate it comes only from
/// [`GoalSpecDraft::build`] or deserialisation, both validated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "GoalSpecDraft")]
#[non_exhaustive]
pub struct GoalSpec {
    pub class: GoalClass,
    pub target_state: String,
    pub entities_affected: Vec<String>,
    pub time_horizon_secs: u64,
    pub success_metric: String,
    pub acceptable_uncertainty: Decimal,
    pub budget: Decimal,
    pub risk_envelope: Decimal,
    pub jurisdictions: Vec<String>,
    pub acting_identity: String,
    pub prohibited_side_effects: Vec<String>,
    pub prohibited_methods: Vec<String>,
    pub stop_conditions: Vec<String>,
    /// What is undone, and how, once a stop condition fires: each entry names
    /// a condition and the unwinding it requires. Required beside
    /// `stop_conditions` because a stop says when to halt and not what to
    /// leave behind. A goal that halts mid-intervention with nothing declared
    /// leaves its half-applied lever in place, and nobody wrote down that it
    /// should not be there.
    pub rollback_conditions: Vec<String>,
}

impl GoalSpecDraft {
    /// Refuse, naming the first missing or malformed field.
    pub fn build(self) -> Result<GoalSpec> {
        let class = declared("class", self.class)?;
        let target_state = declared_text("target_state", self.target_state)?;
        let entities_affected = declared_list("entities_affected", self.entities_affected)?;
        let horizon = declared("time_horizon_secs", self.time_horizon_secs)?;
        if horizon == 0 {
            return Err(Error::invalid(
                "`time_horizon_secs` is zero; a goal with no horizon cannot be planned",
            ));
        }
        let success_metric = declared_text("success_metric", self.success_metric)?;
        let uncertainty = declared("acceptable_uncertainty", self.acceptable_uncertainty)?;
        if uncertainty <= Decimal::ZERO || uncertainty > Decimal::ONE {
            return Err(Error::invalid(
                "`acceptable_uncertainty` must lie in (0, 1]; it is refused, not clamped",
            ));
        }
        let budget = declared("budget", self.budget)?;
        // Positive, as this crate's goal always required: a zero budget is a
        // goal that may spend nothing, which is a misdeclaration rather than
        // a constraint, and the agency shape had let it through.
        if !budget.is_positive() {
            return Err(Error::invalid(
                "`budget` must be greater than zero; a goal that may spend nothing plans nothing",
            ));
        }
        let risk_envelope = declared("risk_envelope", self.risk_envelope)?;
        if risk_envelope.is_negative() {
            return Err(Error::invalid("`risk_envelope` must not be negative"));
        }
        let jurisdictions = declared_list("jurisdictions", self.jurisdictions)?;
        let acting_identity = declared_text("acting_identity", self.acting_identity)?;
        let prohibited_side_effects =
            declared("prohibited_side_effects", self.prohibited_side_effects)?;
        let prohibited_methods = declared("prohibited_methods", self.prohibited_methods)?;
        if let Some(blank) = prohibited_side_effects
            .iter()
            .chain(prohibited_methods.iter())
            .find(|s| s.trim().is_empty())
        {
            return Err(Error::invalid(format!(
                "a prohibition is blank ({blank:?}); name what is prohibited"
            )));
        }
        let stop_conditions = declared_list("stop_conditions", self.stop_conditions)?;
        let rollback_conditions = declared_list("rollback_conditions", self.rollback_conditions)?;
        Ok(GoalSpec {
            class,
            target_state,
            entities_affected,
            time_horizon_secs: horizon,
            success_metric,
            acceptable_uncertainty: uncertainty,
            budget,
            risk_envelope,
            jurisdictions,
            acting_identity,
            prohibited_side_effects,
            prohibited_methods,
            stop_conditions,
            rollback_conditions,
        })
    }
}

impl GoalSpec {
    /// Strict decode: unknown or missing fields are refused, then every
    /// declaration is validated by [`GoalSpecDraft::build`].
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let draft: GoalSpecDraft = serde_json::from_slice(bytes)
            .map_err(|e| Error::schema(format!("not a GoalSpec: {e}; supply every field")))?;
        draft.build()
    }

    /// AGENCY-035: split this goal into sub-goals, each a `GoalSpec` that
    /// sits inside it.
    ///
    /// The failure prevented is a decomposition that launders authority: two
    /// children each "within budget" that together spend it twice, or a child
    /// acting as somebody the parent never named. A child inherits the
    /// parent's acting identity when it declares none and the parent's
    /// prohibitions always (it may add to them, never drop one); its budget
    /// and envelope are its own declarations, and the sums over the children
    /// must fit the parent's. Refused, never scaled down to fit.
    pub fn decompose(&self, parts: Vec<GoalSpecDraft>) -> Result<Vec<GoalSpec>> {
        if parts.len() < 2 {
            return Err(Error::invalid(
                "a decomposition needs at least two sub-goals; one is the goal itself",
            ));
        }
        let mut budget = Decimal::ZERO;
        let mut envelope = Decimal::ZERO;
        let mut children = Vec::with_capacity(parts.len());
        for mut part in parts {
            if part.acting_identity.is_none() {
                part.acting_identity = Some(self.acting_identity.clone());
            }
            let mut child = part.build()?;
            if child.acting_identity != self.acting_identity {
                return Err(Error::denied(format!(
                    "sub-goal acts as `{}`, outside its parent's identity `{}`",
                    child.acting_identity, self.acting_identity
                )));
            }
            if child.time_horizon_secs > self.time_horizon_secs {
                return Err(Error::guard(
                    "sub-goal horizon runs past its parent's; shorten it",
                ));
            }
            if let Some(outside) = child
                .jurisdictions
                .iter()
                .find(|j| !self.jurisdictions.contains(j))
            {
                return Err(Error::denied(format!(
                    "sub-goal names jurisdiction `{outside}`, which its parent does not"
                )));
            }
            for inherited in &self.prohibited_methods {
                if !child.prohibited_methods.contains(inherited) {
                    child.prohibited_methods.push(inherited.clone());
                }
            }
            for inherited in &self.prohibited_side_effects {
                if !child.prohibited_side_effects.contains(inherited) {
                    child.prohibited_side_effects.push(inherited.clone());
                }
            }
            budget = budget
                .checked_add(child.budget)
                .ok_or_else(|| Error::numeric("sub-goal budgets overflowed"))?;
            envelope = envelope
                .checked_add(child.risk_envelope)
                .ok_or_else(|| Error::numeric("sub-goal risk envelopes overflowed"))?;
            children.push(child);
        }
        if budget > self.budget {
            return Err(Error::guard(format!(
                "sub-goal budgets sum to {budget}, outside the parent's {}; lower one",
                self.budget
            )));
        }
        if envelope > self.risk_envelope {
            return Err(Error::guard(format!(
                "sub-goal risk envelopes sum to {envelope}, outside the parent's {}; lower one",
                self.risk_envelope
            )));
        }
        Ok(children)
    }
}

impl TryFrom<GoalSpecDraft> for GoalSpec {
    type Error = Error;
    fn try_from(draft: GoalSpecDraft) -> Result<Self> {
        draft.build()
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

/// How an effect was told apart from what would have happened anyway.
///
/// AGENCY-056 reads this: predictive accuracy and correlation are both
/// `NotIdentified`, however good the fit, and nothing widens on them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Identification {
    /// A randomised or staged experiment assigned the treatment.
    Experiment,
    /// An observational estimate with a named identifying strategy.
    Observational { strategy: String },
    /// A correlation, a forecast that came true, or anything else with no
    /// identifying strategy.
    NotIdentified,
}

impl Identification {
    pub fn is_identified(&self) -> bool {
        !matches!(self, Self::NotIdentified)
    }
}

/// CONTRACT-030 / AGENCY-048 / R12-AGENCY-6104: the one effect attribution.
///
/// There were two, each holding half of what an auditable attribution needs.
/// This crate's had per-horizon observations, a counterfactual method and
/// identifiability, but no causal chain and no rival explanation, so "the KPI
/// moved at three horizons" passed with no mechanism named. `qip-agency`'s had
/// the chain, confounders, rivals and an identification strategy, but one
/// number and no horizon, so an effect that appeared at a minute and reversed
/// by a day was one figure. Both halves are required here, and
/// `qip_agency::attribution` re-exports this type.
///
/// Every list is refused empty: an attribution that considered no rival says
/// so in words ("none considered"), which a reviewer can challenge, rather
/// than by omission, which a reviewer cannot see.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectAttribution {
    pub goal_id: String,
    pub intervention_id: String,
    pub action_id: String,
    /// The class of action whose effect this is; evidence for one class
    /// widens no other.
    pub action_class: Method,
    pub identification: Identification,
    /// Distinct horizons, each observed once.
    pub observations: Vec<HorizonObservation>,
    pub counterfactual_method: String,
    pub effect: Effect,
    /// The mechanism, cause first.
    pub causal_chain: Vec<String>,
    pub confounders: Vec<String>,
    pub competing_explanations: Vec<String>,
    pub side_effects: Vec<String>,
    /// Identifiability in `[0, 1]`: 1 an effect established by experiment, 0
    /// a bare correlation.
    pub identifiability: f64,
    pub resulting_updates: Vec<String>,
}

impl EffectAttribution {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "EffectAttribution";
        text(R, "goal_id", &self.goal_id)?;
        text(R, "intervention_id", &self.intervention_id)?;
        text(R, "action_id", &self.action_id)?;
        if let Identification::Observational { strategy } = &self.identification {
            text(R, "identification strategy", strategy)?;
        }
        if self.observations.is_empty() {
            return Err(Error::invalid(
                "EffectAttribution.observations is empty; an effect needs at least one horizon observed",
            ));
        }
        let mut horizons = BTreeSet::new();
        for o in &self.observations {
            if o.horizon_ms == 0 || !o.value.is_finite() {
                return Err(Error::invalid(
                    "EffectAttribution.observations holds a zero horizon or non-finite value",
                ));
            }
            if !horizons.insert(o.horizon_ms) {
                return Err(Error::invalid(format!(
                    "EffectAttribution.observations names horizon {}ms twice; one value per horizon",
                    o.horizon_ms
                )));
            }
        }
        text(R, "counterfactual_method", &self.counterfactual_method)?;
        self.effect.validate(R, "effect")?;
        list(R, "causal_chain", &self.causal_chain)?;
        list(R, "confounders", &self.confounders)?;
        list(R, "competing_explanations", &self.competing_explanations)?;
        list(R, "side_effects", &self.side_effects)?;
        finite(R, "identifiability", self.identifiability, 0.0, 1.0)?;
        // R12-AGENCY-6103: a correlation is not upgraded to a causal claim by
        // writing a high number beside it. With no identifying strategy the
        // only honest identifiability is zero; anything above it is a causal
        // certainty the record has no evidence for.
        if !self.identification.is_identified() && self.identifiability > 0.0 {
            return Err(Error::invalid(format!(
                "EffectAttribution.identifiability is {} on an effect that is not identified; \
                 a correlation is recorded at 0, name the experiment or strategy to claim more",
                self.identifiability
            )));
        }
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
    /// What closing the gap is worth: the return a better model, source or
    /// tool would earn.
    pub economic_value: Decimal,
    /// What leaving the gap open exposes the book to: the loss the platform
    /// carries while the thing it cannot explain keeps moving. Distinct from
    /// [`Self::economic_value`] because a gap can be worth little to close
    /// and dangerous to leave — a liquidity hole the desk never trades
    /// through for profit is still one it can be trapped in — and a single
    /// figure ranks that gap as harmless.
    pub risk_value: Decimal,
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
        if self.risk_value.is_negative() {
            return Err(Error::invalid(
                "GapSignal.risk_value must not be negative; state zero when the gap exposes nothing",
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
/// The artifact world-coverage expansion produces (section 23, R12-EXPAND-6026):
/// one source the scouts found, with where it came from, what it may be used
/// for, what it costs and how fresh it says it is. A record of a *decision
/// already made* by the data finder - it grants nothing, and a spec with no
/// lineage is refused, because a source that appeared from nowhere cannot be
/// re-derived when someone asks why it is read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    pub source_id: String,
    /// The discovery method: where the scout found it.
    pub discovered_from: String,
    pub discovered_at: qip_core::Timestamp,
    /// The source this one was found to replace, if any.
    pub replaces: Option<String>,
    /// The licence identifier the registration was decided on, or the word
    /// the licensing posture is in when no licence was read.
    pub licence: String,
    /// What the licence permits, as the usage names a gate checks.
    pub permitted_usages: Vec<String>,
    pub monthly_fee: Decimal,
    pub currency: String,
    /// How often the source says it updates.
    pub freshness: String,
}

impl SourceSpec {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "SourceSpec";
        text(R, "source_id", &self.source_id)?;
        text(R, "discovered_from", &self.discovered_from)?;
        text(R, "licence", &self.licence)?;
        text(R, "currency", &self.currency)?;
        text(R, "freshness", &self.freshness)?;
        if self.monthly_fee.is_negative() {
            return Err(Error::invalid(
                "SourceSpec.monthly_fee must not be negative; a source that pays the platform is a rebate no vendor offers",
            ));
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let v: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::schema(format!("not a SourceSpec: {e}; supply every field")))?;
        v.validate()?;
        Ok(v)
    }
}

/// Brain Registry entry: records a specialist agent or model with all six attributes.
/// Every brain has competency domains, tools, evaluations, calibration, authority and version lineage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrainSpec {
    pub id: String,
    pub name: String,
    pub competency_domains: Vec<String>,
    pub tools: Vec<String>,
    pub source_permissions: Vec<String>,
    pub memory_scope: String,
    pub model_stack: Vec<String>,
    pub evaluations: Vec<String>,
    pub abstention_rules: Vec<String>,
    pub authority_envelope: String,
    pub calibration: f64,
    pub version: u32,
    pub parent_version_id: Option<String>,
}

impl BrainSpec {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "BrainSpec";
        text(R, "id", &self.id)?;
        text(R, "name", &self.name)?;
        list(R, "competency_domains", &self.competency_domains)?;
        list(R, "tools", &self.tools)?;
        list(R, "source_permissions", &self.source_permissions)?;
        text(R, "memory_scope", &self.memory_scope)?;
        list(R, "model_stack", &self.model_stack)?;
        list(R, "evaluations", &self.evaluations)?;
        list(R, "abstention_rules", &self.abstention_rules)?;
        text(R, "authority_envelope", &self.authority_envelope)?;
        finite(R, "calibration", self.calibration, 0.0, 1.0)?;
        if self.version == 0 {
            return Err(Error::invalid(
                "BrainSpec.version must be greater than zero",
            ));
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let v: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::schema(format!("not a BrainSpec: {e}; supply every field")))?;
        v.validate()?;
        Ok(v)
    }
}

/// Memory kinds in the registry: episodes, semantic knowledge, procedures, failures, simulations, and compressed abstractions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Episodic,
    Semantic,
    Procedural,
    Failure,
    Simulation,
    CompressedAbstraction,
}

/// Retention policy: how long a memory entry is kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionPolicy {
    pub max_age_ms: u64,
    pub description: String,
}

impl RetentionPolicy {
    pub fn validate(&self, record: &str) -> Result<()> {
        if self.max_age_ms == 0 {
            return Err(Error::invalid(format!(
                "{record}.max_age_ms must be above zero"
            )));
        }
        text(record, "description", &self.description)?;
        Ok(())
    }
}

/// Value policy: how much an entry is worth keeping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValuePolicy {
    /// Importance in [0, 1]; entries below this threshold can be evicted.
    pub importance_threshold: f64,
    pub rationale: String,
}

impl ValuePolicy {
    pub fn validate(&self, record: &str) -> Result<()> {
        finite(
            record,
            "importance_threshold",
            self.importance_threshold,
            0.0,
            1.0,
        )?;
        text(record, "rationale", &self.rationale)?;
        Ok(())
    }
}

/// Memory Registry entry: holds semantic knowledge, episodes, procedures, failures, simulations or compressed abstractions,
/// governed by both retention and value policies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEntry {
    pub id: String,
    pub kind: MemoryKind,
    pub content: String,
    pub retention_policy: RetentionPolicy,
    pub value_policy: ValuePolicy,
    pub created_at_ms: u64,
    pub importance: f64,
    pub tags: Vec<String>,
}

impl MemoryEntry {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "MemoryEntry";
        text(R, "id", &self.id)?;
        text(R, "content", &self.content)?;
        self.retention_policy.validate(R)?;
        self.value_policy.validate(R)?;
        if self.created_at_ms == 0 {
            return Err(Error::invalid(
                "MemoryEntry.created_at_ms must be above zero",
            ));
        }
        finite(R, "importance", self.importance, 0.0, 1.0)?;
        list(R, "tags", &self.tags)?;
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let v: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::schema(format!("not a MemoryEntry: {e}; supply every field")))?;
        v.validate()?;
        Ok(v)
    }
}

// --- OntologyPatch (R12-EXPAND-6029) ------------------------------------------

/// The artifact ontology expansion produces: a proposed type the current
/// schema cannot explain an observation without, as a candidate version of a
/// named type in one family, citing the observations that wanted it. A patch
/// proposes; `qip_agents::registries::OntologyRegistry::register` is what
/// admits a candidate, and a patch citing nothing is refused because a type
/// nobody observed a need for is a schema change with no reason on record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OntologyPatch {
    pub family: OntologyTypeFamily,
    pub name: String,
    /// The version this patch proposes; at least one.
    pub version: u32,
    pub schema: String,
    /// The observation or signal ids that wanted the type; at least one.
    pub cited_observations: Vec<String>,
    pub rationale: String,
}

impl OntologyPatch {
    pub fn validate(&self) -> Result<()> {
        const R: &str = "OntologyPatch";
        text(R, "name", &self.name)?;
        if self.version == 0 {
            return Err(Error::invalid(format!(
                "{R}.version must be at least one; version zero names no type"
            )));
        }
        text(R, "schema", &self.schema)?;
        list(R, "cited_observations", &self.cited_observations)?;
        text(R, "rationale", &self.rationale)?;
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let v: Self = serde_json::from_slice(bytes)
            .map_err(|e| Error::schema(format!("not an OntologyPatch: {e}; supply every field")))?;
        v.validate()?;
        Ok(v)
    }

    /// The type spec this patch proposes, for the registry to admit.
    pub fn as_type_spec(&self) -> OntologyTypeSpec {
        OntologyTypeSpec {
            family: self.family,
            name: self.name.clone(),
            version: self.version,
            schema: self.schema.clone(),
            description: format!(
                "{} [cited: {}]",
                self.rationale,
                self.cited_observations.join(", ")
            ),
        }
    }
}

// --- SyntheticEpisode (ADR 0149 decision 18) --------------------------------

/// How a synthetic episode was produced, which decides whether it has a seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntheticGenerator {
    /// A deterministic function of an ambient signal and the recorded prices
    /// of the subjects it names. No stream is drawn.
    Conditional,
    /// A named scenario from the library. No stream is drawn.
    Library,
    /// A Monte Carlo path, drawn from a seeded stream.
    MonteCarlo,
}

impl SyntheticGenerator {
    /// Whether the generator draws random numbers, and so owes a seed.
    pub const fn is_stochastic(self) -> bool {
        matches!(self, Self::MonteCarlo)
    }
}

/// The wire form of [`SyntheticEpisode`], checked by `TryFrom`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntheticEpisodeFields {
    pub scenario: String,
    pub generator: SyntheticGenerator,
    pub input_digest: String,
    pub seed: Option<u64>,
}

/// A record that a scenario was generated rather than observed: what it was
/// called, how it was made, the digest of the inputs it was made from, and the
/// seed only where something random was drawn.
///
/// A seed on a deterministic generator would be a number nobody used, and a
/// stochastic generator without one cannot be replayed; both are refused at
/// construction and on decode. Written where a scenario is generated or run,
/// never by a backtester, which replays recorded bars and would label a
/// historical replay synthetic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SyntheticEpisodeFields")]
pub struct SyntheticEpisode {
    scenario: String,
    generator: SyntheticGenerator,
    input_digest: String,
    seed: Option<u64>,
}

impl SyntheticEpisode {
    pub fn new(
        scenario: impl Into<String>,
        generator: SyntheticGenerator,
        input_digest: impl Into<String>,
        seed: Option<u64>,
    ) -> Result<Self> {
        Self::try_from(SyntheticEpisodeFields {
            scenario: scenario.into(),
            generator,
            input_digest: input_digest.into(),
            seed,
        })
    }

    pub fn scenario(&self) -> &str {
        &self.scenario
    }

    pub const fn generator(&self) -> SyntheticGenerator {
        self.generator
    }

    /// The SHA-256 of the canonical inputs, lower-case hex.
    pub fn input_digest(&self) -> &str {
        &self.input_digest
    }

    pub const fn seed(&self) -> Option<u64> {
        self.seed
    }
}

impl TryFrom<SyntheticEpisodeFields> for SyntheticEpisode {
    type Error = Error;

    fn try_from(fields: SyntheticEpisodeFields) -> Result<Self> {
        text("SyntheticEpisode", "scenario", &fields.scenario)?;
        let digest = &fields.input_digest;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::invalid(
                "SyntheticEpisode.input_digest must be a 64-character lower-case SHA-256 hex digest",
            ));
        }
        match (fields.generator.is_stochastic(), fields.seed) {
            (true, None) => {
                return Err(Error::invalid(format!(
                    "SyntheticEpisode {} names a stochastic generator and no seed; without one \
                     it cannot be replayed, and a default would be a seed nobody drew",
                    fields.scenario
                )));
            }
            (false, Some(_)) => {
                return Err(Error::invalid(format!(
                    "SyntheticEpisode {} names a deterministic generator and a seed; nothing \
                     was drawn, so the seed would be a number nobody used",
                    fields.scenario
                )));
            }
            _ => {}
        }
        Ok(Self {
            scenario: fields.scenario,
            generator: fields.generator,
            input_digest: fields.input_digest,
            seed: fields.seed,
        })
    }
}
