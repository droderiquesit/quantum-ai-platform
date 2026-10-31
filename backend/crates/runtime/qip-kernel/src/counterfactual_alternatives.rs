//! The committed counterfactual alternatives (R12-CAPITAL-4091, decision 17
//! of 2026-10-31): the different-hedge and different-venue arms every scored
//! decision is compared against, declared once in a file compiled into the
//! kernel and loaded by every central composition root.
//!
//! **Counterfactual only.** Nothing here reaches `PlatformConfig::hedge_policies`,
//! so the hedge review journals no hedge proposal from it, and nothing here
//! sizes or routes an order: an arm only scores an alternative the twin
//! prices against history.
//!
//! **No figure is invented.** An arm is either declared, with the figures a
//! person or the tree already states, or absent with its reason. The
//! committed file declares neither: the universe holds no hedge instrument,
//! and no alternate venue with its cost is on record. A declaration that
//! states both an arm and a reason, or neither, is refused.
//!
//! **Fails closed to "no arm".** [`CounterfactualAlternatives::resolve`]
//! drops an arm whose instrument the universe does not hold, naming why,
//! and `Platform::evaluate_alternatives` drops a hedge arm whose instrument
//! has no held bars, so a missing input costs that one comparison and never
//! the whole set.
//!
//! Embedded under `backend/` because the image is built from `backend/`
//! alone (`infrastructure/docker/Dockerfile`), so a file under `data/`
//! would not reach the build.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_financial::universe::Universe;
use serde::{Deserialize, Serialize};

/// The committed declaration, as compiled in.
pub const COMMITTED: &str = include_str!("counterfactual_alternatives.json");

/// A declared different-hedge arm: the instrument and the units of it that
/// offset one unit of the traded exposure. Declared by a person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HedgeArm {
    pub instrument: ObjectId,
    pub ratio: Decimal,
}

/// A declared different-venue arm: a venue other than the desk's own and
/// its proportional taker cost, as a fraction of notional, as the tree
/// states it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueArm {
    pub name: String,
    pub taker_cost: Decimal,
}

/// Each arm declared or absent with its reason, never both and never
/// neither.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CounterfactualAlternatives {
    pub hedge: Option<HedgeArm>,
    pub hedge_absent_reason: Option<String>,
    pub venue: Option<VenueArm>,
    pub venue_absent_reason: Option<String>,
}

/// What assembly took from a declaration: the arms the universe can hold,
/// and why each missing one is missing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedAlternatives {
    pub hedge: Option<HedgeArm>,
    pub venue: Option<VenueArm>,
    pub absent: Vec<String>,
}

impl CounterfactualAlternatives {
    /// Parse and validate a declaration.
    pub fn from_json(text: &str) -> Result<Self> {
        let declared: Self = serde_json::from_str(text).map_err(|error| {
            Error::invalid(format!(
                "configuration: the counterfactual alternatives do not parse: {error}"
            ))
        })?;
        declared.validate()?;
        Ok(declared)
    }

    /// The declaration compiled into this kernel. A root that cannot load it
    /// stops rather than scoring decisions against arms nobody declared.
    pub fn committed() -> Result<Self> {
        Self::from_json(COMMITTED)
    }

    fn validate(&self) -> Result<()> {
        let stated = |reason: &Option<String>| {
            reason
                .as_deref()
                .is_some_and(|reason| !reason.trim().is_empty())
        };
        match (&self.hedge, stated(&self.hedge_absent_reason)) {
            (Some(_), true) | (None, false) => {
                return Err(Error::invalid(
                    "configuration: the counterfactual alternatives must either declare a hedge \
                     arm or state why there is none, and not both",
                ));
            }
            (Some(arm), false) if !arm.ratio.is_positive() => {
                return Err(Error::invalid(format!(
                    "configuration: the declared hedge ratio {} is not positive",
                    arm.ratio
                )));
            }
            _ => {}
        }
        match (&self.venue, stated(&self.venue_absent_reason)) {
            (Some(_), true) | (None, false) => Err(Error::invalid(
                "configuration: the counterfactual alternatives must either declare a venue arm \
                 or state why there is none, and not both",
            )),
            (Some(arm), false)
                if arm.name.trim().is_empty()
                    || arm.taker_cost.is_negative()
                    || arm.taker_cost >= Decimal::ONE =>
            {
                Err(Error::invalid(format!(
                    "configuration: the declared venue arm {:?} needs a name and a taker cost in \
                     [0, 1)",
                    arm.name
                )))
            }
            _ => Ok(()),
        }
    }

    /// The arms the platform can use, against the universe it was assembled
    /// over. A hedge instrument the universe does not hold is dropped with
    /// its reason rather than carried into a menu the twin would refuse. The
    /// venue arm is carried as declared; [`ResolvedAlternatives::venue_other_than`]
    /// drops it where it names the desk's own venue.
    pub fn resolve(&self, universe: &Universe) -> ResolvedAlternatives {
        let mut resolved = ResolvedAlternatives::default();
        match &self.hedge {
            Some(arm) if universe.get(&arm.instrument).is_some() => {
                resolved.hedge = Some(arm.clone());
            }
            Some(arm) => resolved.absent.push(format!(
                "hedge: {} is not in the universe this platform was assembled over",
                arm.instrument
            )),
            None => resolved.absent.push(format!(
                "hedge: {}",
                self.hedge_absent_reason.clone().unwrap_or_default()
            )),
        }
        match &self.venue {
            Some(arm) => resolved.venue = Some(arm.clone()),
            None => resolved.absent.push(format!(
                "venue: {}",
                self.venue_absent_reason.clone().unwrap_or_default()
            )),
        }
        resolved
    }
}

impl ResolvedAlternatives {
    /// The venue arm, unless it names the desk's own venue: an alternative
    /// to trading where the desk trades is no alternative at all.
    pub fn venue_other_than(&self, desk_venue: &str) -> Option<&VenueArm> {
        self.venue.as_ref().filter(|arm| arm.name != desk_venue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed file parses, validates and declares no arm, each with
    /// its reason: decision 17 forbids inventing either figure.
    #[test]
    fn the_committed_declaration_loads_and_invents_no_arm() {
        let committed = CounterfactualAlternatives::committed();
        assert!(
            committed.is_ok(),
            "the committed declaration: {committed:?}"
        );
        let Ok(committed) = committed else { return };
        assert!(committed.hedge.is_none() && committed.venue.is_none());
        let resolved = committed.resolve(&Universe::new());
        assert_eq!(resolved.absent.len(), 2, "{resolved:?}");
    }

    /// A declaration that states an arm and a reason, or neither, is refused.
    #[test]
    fn a_declaration_must_state_each_arm_or_its_absence_and_not_both() {
        let neither = r#"{"hedge":null,"hedge_absent_reason":null,"venue":null,"venue_absent_reason":"none"}"#;
        assert!(CounterfactualAlternatives::from_json(neither).is_err());
        let both = r#"{"hedge":{"instrument":"OBJ1","ratio":"1"},"hedge_absent_reason":"x","venue":null,"venue_absent_reason":"none"}"#;
        assert!(CounterfactualAlternatives::from_json(both).is_err());
    }
}
