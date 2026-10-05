//! AGENCY-003 / AGENCY-041: a desired outcome is a typed `GoalSpec`, never a
//! free-form instruction.
//!
//! A free-form goal ("grow the book") carries no budget, no envelope and no
//! identity, so a planner reading it has nothing to stay inside. `GoalSpec`
//! is only constructible through [`GoalSpecDraft::build`] and its serde form
//! routes through the same constructor, so a JSON document missing a field
//! is refused with that field's name rather than defaulted.

use crate::{non_empty, required, text};
use qip_core::{Decimal, Error};
use serde::{Deserialize, Serialize};

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
/// not a deserialisation error naming nothing useful.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoalSpecDraft {
    pub class: Option<GoalClass>,
    pub target_state: Option<String>,
    pub entities_affected: Option<Vec<String>>,
    pub time_horizon_secs: Option<u64>,
    pub success_metric: Option<String>,
    /// Largest acceptable uncertainty on the outcome, in `(0, 1]`.
    pub acceptable_uncertainty: Option<Decimal>,
    pub budget: Option<Decimal>,
    /// Largest summed exposure a plan may carry.
    pub risk_envelope: Option<Decimal>,
    pub jurisdictions: Option<Vec<String>>,
    pub acting_identity: Option<String>,
    /// May be declared empty ("none"), but must be declared.
    pub prohibited_side_effects: Option<Vec<String>>,
    pub prohibited_methods: Option<Vec<String>>,
    pub stop_conditions: Option<Vec<String>>,
}

/// A validated goal. `non_exhaustive`: outside this crate it can only come
/// from [`GoalSpecDraft::build`] or deserialisation, both validated.
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
}

impl GoalSpecDraft {
    /// Refuse, naming the first missing or malformed field.
    pub fn build(self) -> Result<GoalSpec, Error> {
        let class = required("class", self.class)?;
        let target_state = text("target_state", self.target_state)?;
        let entities_affected = non_empty("entities_affected", self.entities_affected)?;
        let horizon = required("time_horizon_secs", self.time_horizon_secs)?;
        if horizon == 0 {
            return Err(Error::invalid(
                "`time_horizon_secs` is zero; a goal with no horizon cannot be planned",
            ));
        }
        let success_metric = text("success_metric", self.success_metric)?;
        let uncertainty = required("acceptable_uncertainty", self.acceptable_uncertainty)?;
        if uncertainty <= Decimal::ZERO || uncertainty > Decimal::ONE {
            return Err(Error::invalid(
                "`acceptable_uncertainty` must lie in (0, 1]; it is refused, not clamped",
            ));
        }
        let budget = required("budget", self.budget)?;
        let risk_envelope = required("risk_envelope", self.risk_envelope)?;
        if budget.is_negative() || risk_envelope.is_negative() {
            return Err(Error::invalid(
                "`budget` and `risk_envelope` must not be negative",
            ));
        }
        let jurisdictions = non_empty("jurisdictions", self.jurisdictions)?;
        let acting_identity = text("acting_identity", self.acting_identity)?;
        let prohibited_side_effects =
            required("prohibited_side_effects", self.prohibited_side_effects)?;
        let prohibited_methods = required("prohibited_methods", self.prohibited_methods)?;
        let stop_conditions = non_empty("stop_conditions", self.stop_conditions)?;
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
        })
    }
}

impl GoalSpec {
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
    pub fn decompose(&self, parts: Vec<GoalSpecDraft>) -> Result<Vec<GoalSpec>, Error> {
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
    fn try_from(draft: GoalSpecDraft) -> Result<Self, Error> {
        draft.build()
    }
}
