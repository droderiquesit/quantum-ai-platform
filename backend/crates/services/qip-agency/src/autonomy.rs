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
//! Above narrow-reversible the levels are declared and not granted (ADR
//! 0141): [`SCHEDULE`] states, per level, the evidence that admits it, the
//! rollback it must carry and how many distinct approvers sign, and
//! [`ActionPolicy::widen_to`] reads that table rather than a hard-coded
//! branch. The evidence is checked before the grant, so a level nobody has
//! granted still refuses the lesser evidence by name -- the table is
//! enforced, not decorative -- and then refuses the grant. External action
//! and capital are not ungranted but prohibited (ADR 0114 D8; ADR 0003, 0021,
//! 0108), and refuse naming those records before anything else is read.

use crate::affordance::Method;
use crate::attribution::EffectAttribution;
use crate::plan::ActingIdentity;
use qip_core::{Decimal, Error};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Days a policy must hold its current level without drift before the
/// compensable level's evidence is met (ADR 0141 decision 2: chosen, not
/// measured).
pub const CLEAN_DAYS_BEFORE_COMPENSABLE: u32 = 30;

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
    /// Own class, an internal tool whose effect a separate compensating step
    /// undoes. Declared by ADR 0141; granted by no record.
    NarrowCompensable,
    /// Several classes, internal tools. Declared by ADR 0141; granted by no
    /// record.
    BroadInternal,
    /// Any outward surface: posting, outreach, pricing, operations. Refused
    /// by ADR 0114 D8.
    External,
    /// Any capital, hedging or liquidity surface. Refused by ADR 0003, 0021
    /// and 0108.
    Capital,
}

/// Whether any record grants a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// The starting level; nothing grants it because nothing needs to.
    Default,
    Granted,
    /// Declared, with the evidence it would need, and granted by nothing.
    Ungranted {
        declared_by: &'static str,
    },
    /// Prohibited outright; no evidence is read.
    Refused {
        by: &'static str,
    },
}

/// What the evidence for a level must attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionNeed {
    None,
    /// An identified attribution for the policy's own class.
    OwnClass,
    /// An identified attribution for every class in the widened scope.
    EveryClass,
}

/// One row of ADR 0141's schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelRequirement {
    pub level: Authority,
    pub scope: &'static str,
    /// The level a policy must already hold to be widened to this one.
    pub from: Option<Authority>,
    pub attribution: AttributionNeed,
    /// Days at `from` without drift.
    pub clean_days: u32,
    /// A compensating step per class, tested in a drill and named.
    pub compensation: bool,
    /// A kill path that returns every class to shadow at once.
    pub kill_path: bool,
    pub rollback: &'static str,
    /// Distinct approvers, each holding [`WIDEN_AUTHORITY`].
    pub approvers: usize,
    pub grant: Grant,
}

/// ADR 0141's table, in authority order. `widen_to` reads nothing else.
pub const SCHEDULE: [LevelRequirement; 6] = [
    LevelRequirement {
        level: Authority::Shadow,
        scope: "none",
        from: None,
        attribution: AttributionNeed::None,
        clean_days: 0,
        compensation: false,
        kill_path: false,
        rollback: "none",
        approvers: 0,
        grant: Grant::Default,
    },
    LevelRequirement {
        level: Authority::NarrowReversible,
        scope: "own class, reversible internal tool",
        from: Some(Authority::Shadow),
        attribution: AttributionNeed::OwnClass,
        clean_days: 0,
        compensation: false,
        kill_path: false,
        rollback: "the tool's declared reverse",
        approvers: 1,
        grant: Grant::Granted,
    },
    LevelRequirement {
        level: Authority::NarrowCompensable,
        scope: "own class, internal tool undone by a separate compensating step",
        from: Some(Authority::NarrowReversible),
        attribution: AttributionNeed::OwnClass,
        clean_days: CLEAN_DAYS_BEFORE_COMPENSABLE,
        compensation: true,
        kill_path: false,
        rollback: "a compensating step, tested in a drill and named on the policy",
        approvers: 2,
        grant: Grant::Ungranted {
            declared_by: "ADR 0141",
        },
    },
    LevelRequirement {
        level: Authority::BroadInternal,
        scope: "several classes, internal tools",
        from: Some(Authority::NarrowCompensable),
        attribution: AttributionNeed::EveryClass,
        clean_days: CLEAN_DAYS_BEFORE_COMPENSABLE,
        compensation: true,
        kill_path: true,
        rollback: "compensation per class, and a kill path returning every class to shadow",
        approvers: 2,
        grant: Grant::Ungranted {
            declared_by: "ADR 0141",
        },
    },
    LevelRequirement {
        level: Authority::External,
        scope: "any outward surface",
        from: None,
        attribution: AttributionNeed::None,
        clean_days: 0,
        compensation: false,
        kill_path: false,
        rollback: "none",
        approvers: 0,
        grant: Grant::Refused { by: "ADR 0114 D8" },
    },
    LevelRequirement {
        level: Authority::Capital,
        scope: "any capital, hedging or liquidity surface",
        from: None,
        attribution: AttributionNeed::None,
        clean_days: 0,
        compensation: false,
        kill_path: false,
        rollback: "none",
        approvers: 0,
        grant: Grant::Refused {
            by: "ADR 0003, 0021 and 0108",
        },
    },
];

impl Authority {
    /// This level's row of [`SCHEDULE`].
    pub fn requirement(self) -> &'static LevelRequirement {
        SCHEDULE
            .iter()
            .find(|row| row.level == self)
            .unwrap_or(&SCHEDULE[0])
    }
}

/// What a widening request offers against a level's row.
#[derive(Debug, Clone, Default)]
pub struct WideningEvidence<'a> {
    /// Identified attributions, one per class the widened scope covers.
    pub attributions: Vec<&'a EffectAttribution>,
    /// The classes the widened scope covers. Empty means the policy's own.
    pub classes: BTreeSet<Method>,
    /// Days held at the current level without drift.
    pub clean_days: u32,
    /// Per class, the named compensating step and the drill that tested it.
    pub compensations: Vec<(Method, String)>,
    /// The kill path returning every class to shadow at once.
    pub kill_path: Option<String>,
}

impl<'a> WideningEvidence<'a> {
    /// One attribution and nothing else: the first step's evidence.
    pub fn attribution(evidence: &'a EffectAttribution) -> Self {
        Self {
            attributions: vec![evidence],
            ..Self::default()
        }
    }
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
    /// operator authority together. The first row of [`SCHEDULE`] past
    /// shadow, through [`Self::widen_to`].
    pub fn widen(
        &mut self,
        evidence: &EffectAttribution,
        approver: &ActingIdentity,
    ) -> Result<(), Error> {
        self.widen_to(
            Authority::NarrowReversible,
            &WideningEvidence::attribution(evidence),
            &[approver],
        )
    }

    /// Widen to `target` on the evidence and approvers its [`SCHEDULE`] row
    /// demands (R12-EXPAND-6071, ADR 0141). In order: a prohibited level is
    /// refused naming the record that prohibits it; the evidence must meet
    /// the row, item by item; enough distinct approvers must hold
    /// [`WIDEN_AUTHORITY`]; the policy must hold the row's `from` level; and
    /// only then is an ungranted level refused for want of a grant. Evidence
    /// is read before the level so that every row's demands are enforced --
    /// and refused by name when short -- even for a level no policy can hold
    /// yet. Evidence never stands in for an approver, nor an approver for
    /// evidence.
    pub fn widen_to(
        &mut self,
        target: Authority,
        evidence: &WideningEvidence<'_>,
        approvers: &[&ActingIdentity],
    ) -> Result<(), Error> {
        let row = target.requirement();
        if let Grant::Refused { by } = row.grant {
            return Err(Error::denied(format!(
                "{target:?} authority is refused by {by}; no evidence or approver grants it"
            )));
        }
        self.check_attributions(row, evidence)?;
        if evidence.clean_days < row.clean_days {
            return Err(Error::denied(format!(
                "{target:?} needs {} day(s) at {:?} without drift; the evidence states {}",
                row.clean_days, self.authority, evidence.clean_days
            )));
        }
        if row.compensation {
            for class in self.scope_of(evidence) {
                let named = evidence
                    .compensations
                    .iter()
                    .any(|(c, step)| *c == class && !step.trim().is_empty());
                if !named {
                    return Err(Error::denied(format!(
                        "{target:?} needs a compensating step for {class:?}, tested in a drill \
                         and named; none is"
                    )));
                }
            }
        }
        if row.kill_path
            && evidence
                .kill_path
                .as_deref()
                .is_none_or(|k| k.trim().is_empty())
        {
            return Err(Error::denied(format!(
                "{target:?} needs a kill path that returns every class to shadow at once"
            )));
        }
        let signed: BTreeSet<&str> = approvers
            .iter()
            .filter(|a| a.authorities.contains(WIDEN_AUTHORITY))
            .map(|a| a.name.as_str())
            .collect();
        if signed.len() < row.approvers {
            let unsigned: Vec<&str> = approvers
                .iter()
                .filter(|a| !a.authorities.contains(WIDEN_AUTHORITY))
                .map(|a| a.name.as_str())
                .collect();
            return Err(Error::denied(format!(
                "{target:?} needs {} distinct approver(s) holding `{WIDEN_AUTHORITY}`; {} sign \
                 and {unsigned:?} do not hold it; evidence is a precondition, not a grant",
                row.approvers,
                signed.len()
            )));
        }
        if row.from != Some(self.authority) {
            return Err(Error::guard(format!(
                "the policy holds {:?}; {target:?} is reached only from {:?}, one level at a \
                 time, and every level above narrow-reversible needs an ADR to grant (ADR 0141)",
                self.authority,
                row.from.unwrap_or(Authority::Shadow)
            )));
        }
        if let Grant::Ungranted { declared_by } = row.grant {
            return Err(Error::denied(format!(
                "{target:?} is declared by {declared_by} and granted by no record; the evidence \
                 meets its row, and a further ADR must grant the level before a policy holds it"
            )));
        }
        self.authority = target;
        Ok(())
    }

    /// The classes a widening covers: the evidence's, or the policy's own.
    fn scope_of(&self, evidence: &WideningEvidence<'_>) -> BTreeSet<Method> {
        if evidence.classes.is_empty() {
            BTreeSet::from([self.class])
        } else {
            evidence.classes.clone()
        }
    }

    fn check_attributions(
        &self,
        row: &LevelRequirement,
        evidence: &WideningEvidence<'_>,
    ) -> Result<(), Error> {
        let needed: BTreeSet<Method> = match row.attribution {
            AttributionNeed::None => return Ok(()),
            AttributionNeed::OwnClass => BTreeSet::from([self.class]),
            AttributionNeed::EveryClass => {
                let scope = self.scope_of(evidence);
                if scope.len() < 2 || !scope.contains(&self.class) {
                    return Err(Error::denied(format!(
                        "{:?} covers several classes including the policy's own; the evidence \
                         names {scope:?}",
                        row.level
                    )));
                }
                scope
            }
        };
        for attribution in &evidence.attributions {
            // The attribution is a contracts record with public fields, so it
            // can exist unvalidated. Widening on one whose chain is empty, or
            // whose correlation has been written up as identified, would be
            // widening on a claim nobody checked.
            attribution.validate()?;
            if !needed.contains(&attribution.action_class) {
                return Err(Error::denied(format!(
                    "the attribution measures {:?}, not this policy's {:?}; evidence for one \
                     class widens no other",
                    attribution.action_class, self.class
                )));
            }
            if !attribution.identification.is_identified() {
                return Err(Error::denied(
                    "the attribution is not identified; correlation or predictive accuracy \
                     widens nothing, run an experiment or name the identifying strategy",
                ));
            }
        }
        for class in &needed {
            if !evidence
                .attributions
                .iter()
                .any(|a| a.action_class == *class)
            {
                return Err(Error::denied(format!(
                    "{:?} needs an identified attribution for {class:?}; none is offered",
                    row.level
                )));
            }
        }
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
