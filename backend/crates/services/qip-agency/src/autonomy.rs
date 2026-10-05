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
use std::collections::BTreeSet;

/// The authority an approver must independently hold to widen a policy.
pub const WIDEN_AUTHORITY: &str = "agency:widen-autonomy";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Authority {
    /// Plans, simulates, gates and reports. No adapter is called.
    #[default]
    Shadow,
    /// May call an adapter for a step of its own class whose tool is
    /// declared reversible, and for nothing else.
    NarrowReversible,
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

    /// AGENCY-054: hold the policy to what it predicted. An effect outside
    /// the predicted interval, on either side, or a side effect it did not
    /// predict, returns it to shadow. Returns the authority it now holds.
    pub fn observe(&mut self, effect: Decimal, side_effects: &[String]) -> Authority {
        let (low, high) = self.predicted_uplift;
        let drifted = effect < low
            || effect > high
            || side_effects
                .iter()
                .any(|s| !self.predicted_side_effects.contains(s));
        if drifted {
            self.authority = Authority::Shadow;
        }
        self.authority
    }
}
