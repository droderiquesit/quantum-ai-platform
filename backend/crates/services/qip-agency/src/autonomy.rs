//! AGENCY-054 / 055 / 056: how much an action policy may do on its own, and
//! the only ways that changes.
//!
//! This is not the trading autonomy ladder (`qip-risk-engine`'s
//! `AutonomyController`), and nothing here can reach it. It governs what the
//! agency engine may hand to an action adapter, and it starts, and by default
//! stays, at nothing.
//!
//! Three failures are prevented. An engine that acts the day it is built:
//! every policy starts at [`Authority::Shadow`]. A policy widened because its
//! forecasts came true: [`ActionPolicy::widen`] wants an identified
//! [`EffectAttribution`] for its own action class *and* an approver holding
//! [`WIDEN_AUTHORITY`], and either alone is refused. A policy that keeps its
//! authority after the world stopped matching its predictions:
//! [`ActionPolicy::observe`] drops it back to shadow with nobody asked.
//!
//! There is deliberately no rung above narrow-reversible. §24 says broader
//! authority "comes only after that stage"; what it would be is a decision
//! for an ADR, not for a variant somebody adds here.

use crate::affordance::Method;
use crate::attribution::EffectAttribution;
use crate::plan::ActingIdentity;
use qip_core::{Decimal, Error};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The authority an approver must independently hold to widen a policy.
pub const WIDEN_AUTHORITY: &str = "agency:widen-autonomy";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    /// Plans, simulates, gates and reports. No adapter is called.
    #[default]
    Shadow,
    /// May call an adapter for a step of its own class whose tool is
    /// declared reversible, and for nothing else.
    NarrowReversible,
}

/// One way an observed effect departed from a policy's prediction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Drift {
    /// The effect fell below the predicted interval.
    BelowPredicted { effect: Decimal, low: Decimal },
    /// The effect rose above it: a prediction wrong in the pleasing
    /// direction is still a model that does not describe the world.
    AbovePredicted { effect: Decimal, high: Decimal },
    /// A side effect the policy did not predict.
    UnpredictedSideEffect(String),
}

/// One action class's policy: what it predicts, and what it may do.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionPolicy {
    class: Method,
    authority: Authority,
    predicted_uplift: (Decimal, Decimal),
    predicted_side_effects: BTreeSet<String>,
}

impl ActionPolicy {
    /// A new policy, at shadow. `low..=high` is the uplift it predicts and
    /// `predicted_side_effects` every side effect it expects; both are what
    /// [`Self::observe`] later holds it to.
    pub fn new(
        class: Method,
        low: Decimal,
        high: Decimal,
        predicted_side_effects: BTreeSet<String>,
    ) -> Result<Self, Error> {
        if class.is_deceptive() {
            return Err(Error::denied(format!(
                "{class:?} is a deceptive or manipulative method; it has no policy"
            )));
        }
        if low > high {
            return Err(Error::invalid(
                "predicted uplift interval is inverted; state low then high",
            ));
        }
        Ok(Self {
            class,
            authority: Authority::Shadow,
            predicted_uplift: (low, high),
            predicted_side_effects,
        })
    }

    pub fn class(&self) -> Method {
        self.class
    }

    pub fn authority(&self) -> Authority {
        self.authority
    }

    /// The uplift interval `low..=high` this policy is held to by
    /// [`Self::observe`]. Journaled beside every learning record so the
    /// drift verdict can be recomputed from the log alone.
    pub fn predicted_uplift(&self) -> (Decimal, Decimal) {
        self.predicted_uplift
    }

    /// Every side effect this policy predicted; one it did not returns it to
    /// shadow.
    pub fn predicted_side_effects(&self) -> &BTreeSet<String> {
        &self.predicted_side_effects
    }

    /// AGENCY-056: shadow to narrow-reversible, on measured causal effect and
    /// operator authority together.
    pub fn widen(
        &mut self,
        evidence: &EffectAttribution,
        approver: &ActingIdentity,
    ) -> Result<(), Error> {
        if self.authority != Authority::Shadow {
            return Err(Error::guard(
                "no authority above narrow-reversible exists; broader agency needs an ADR",
            ));
        }
        // The attribution is a contracts record with public fields, so it can
        // exist unvalidated. Widening on one whose chain is empty, or whose
        // correlation has been written up as identified, would be widening on
        // a claim nobody checked.
        evidence.validate()?;
        if evidence.action_class != self.class {
            return Err(Error::denied(format!(
                "the attribution measures {:?}, not this policy's {:?}; evidence for one class \
                 widens no other",
                evidence.action_class, self.class
            )));
        }
        if !evidence.identification.is_identified() {
            return Err(Error::denied(
                "the attribution is not identified; correlation or predictive accuracy widens \
                 nothing, run an experiment or name the identifying strategy",
            ));
        }
        if !approver.authorities.contains(WIDEN_AUTHORITY) {
            return Err(Error::denied(format!(
                "`{}` does not hold `{WIDEN_AUTHORITY}`; evidence is a precondition, not a grant",
                approver.name
            )));
        }
        self.authority = Authority::NarrowReversible;
        Ok(())
    }

    /// How an observed effect departs from what this policy predicted: one
    /// [`Drift`] per departure, empty when it held. [`Self::observe`] drops
    /// the policy to shadow exactly when this is non-empty, and
    /// [`crate::engine::learn`] files the same list in a
    /// [`crate::memory::FailureEpisode`] (R12-RES-7111).
    pub fn drift(&self, effect: Decimal, side_effects: &[String]) -> Vec<Drift> {
        let (low, high) = self.predicted_uplift;
        let mut drift = Vec::new();
        if effect < low {
            drift.push(Drift::BelowPredicted { effect, low });
        }
        if effect > high {
            drift.push(Drift::AbovePredicted { effect, high });
        }
        for side in side_effects {
            if !self.predicted_side_effects.contains(side) {
                drift.push(Drift::UnpredictedSideEffect(side.clone()));
            }
        }
        drift
    }

    /// AGENCY-054: hold the policy to what it predicted. An effect outside
    /// the predicted interval, on either side, or a side effect it did not
    /// predict, returns it to shadow. Returns the authority it now holds.
    pub fn observe(&mut self, effect: Decimal, side_effects: &[String]) -> Authority {
        if !self.drift(effect, side_effects).is_empty() {
            self.authority = Authority::Shadow;
        }
        self.authority
    }
}
