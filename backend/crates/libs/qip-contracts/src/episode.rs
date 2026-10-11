//! `Episode` (blueprint v12.0 §27, R12-CONTRACT-7070): one decision episode
//! as the learning loop reads it — the world state it started from, the
//! beliefs held, what was done and what was declined, the outcome at each
//! horizon, how surprising it was, and what the alternatives would have
//! returned.
//!
//! Episodes exist today as `qip-agents` memory entries and as the edge's
//! `RegionalEpisode`, and neither carries all six. The one a learner cannot
//! do without is the counterfactual: an episode that records only what was
//! done teaches the system to repeat what it did, so an episode with no
//! counterfactual is refused here, as is one with a decline nobody gave a
//! reason for.

use crate::validated::{
    Contract, Validated, after, basis_points, digest, listed, named, names, positive_duration,
    refuse,
};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Acted,
    Declined,
}

/// One action taken or declined in the episode, with the reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeDecision {
    pub id: String,
    pub kind: DecisionKind,
    pub reason: String,
}

/// The realised outcome at one horizon, in the account currency.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HorizonOutcome {
    pub horizon: Duration,
    pub realised: Decimal,
}

/// What an alternative would have returned, by the scorer that estimated it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counterfactual {
    pub alternative: String,
    pub estimated_outcome: Decimal,
    pub method: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeFields {
    pub schema_version: u32,
    pub episode_id: String,
    /// SHA-256 of the world-state snapshot the episode started from.
    pub world_state_digest: String,
    /// Belief ids held at decision time; at least one.
    pub beliefs: Vec<String>,
    /// At least one.
    pub decisions: Vec<EpisodeDecision>,
    /// Strictly increasing horizons; at least one.
    pub outcomes: Vec<HorizonOutcome>,
    /// How far the outcome departed from expectation, in basis points.
    pub surprise_bp: u32,
    /// At least one.
    pub counterfactuals: Vec<Counterfactual>,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
}

/// The §27 episode. Built only through [`Validated::new`].
pub type Episode = Validated<EpisodeFields>;

impl Contract for EpisodeFields {
    const NAME: &'static str = "Episode";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank id, a malformed world-state digest, no belief, no
    /// decision or one listed twice, a decision without a reason, no outcome,
    /// a non-positive or non-increasing horizon, a surprise above one, no
    /// counterfactual or one without its alternative or method, and an end
    /// not after the start.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "episode_id", &self.episode_id)?;
        digest(c, "world_state_digest", &self.world_state_digest)?;
        names(c, "beliefs", &self.beliefs)?;
        listed(c, "decisions", &self.decisions)?;
        let mut ids = BTreeSet::new();
        for d in &self.decisions {
            named(c, "decisions.id", &d.id)?;
            named(c, "decisions.reason", &d.reason)?;
            if !ids.insert(d.id.as_str()) {
                return refuse(c, format!("lists decision `{}` twice", d.id));
            }
        }
        listed(c, "outcomes", &self.outcomes)?;
        for o in &self.outcomes {
            positive_duration(c, "outcomes.horizon", o.horizon)?;
        }
        if self
            .outcomes
            .windows(2)
            .any(|w| w[1].horizon <= w[0].horizon)
        {
            return refuse(
                c,
                "states outcome horizons that are not strictly increasing",
            );
        }
        basis_points(c, "surprise_bp", self.surprise_bp)?;
        listed(c, "counterfactuals", &self.counterfactuals)?;
        for k in &self.counterfactuals {
            named(c, "counterfactuals.alternative", &k.alternative)?;
            named(c, "counterfactuals.method", &k.method)?;
        }
        after(c, "ended_at", self.ended_at, "started_at", self.started_at)
    }
}
