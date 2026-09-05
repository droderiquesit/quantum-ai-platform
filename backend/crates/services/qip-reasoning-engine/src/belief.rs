//! The belief state's own record of when it last absorbed evidence.
//!
//! The reasoning engine's beliefs are the hypotheses it forms: each one is a
//! confidence computed from evidence by [`crate::bayes::update`], and that
//! confidence is what the centre sizes on. Blueprint §6.2 row 4 asks how old
//! the newest of those beliefs is, and the answer has to be a fact the engine
//! wrote at the moment evidence became a belief — not a scan of hypotheses
//! somebody else holds, and not a guess made at sizing time.
//!
//! [`BeliefState`] is that fact and nothing more. It deliberately does not
//! hold the hypotheses themselves; they travel on the
//! [`crate::engine::ReasoningOutcome`] to the kernel, which records them in
//! the event log. Holding a second copy here would be a second source of
//! truth for what the log already has.

use qip_core::Timestamp;
use serde::{Deserialize, Serialize};

/// When the engine last turned evidence into a belief, and how many times it
/// has.
///
/// `last_updated` is `None` until the first hypothesis is formed, and the
/// degradation table reads that as unavailable rather than fresh: an engine
/// that has never reasoned has no confidence to size on, and treating its
/// silence as a current belief is the failure the row exists to prevent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeliefState {
    last_updated: Option<Timestamp>,
    hypotheses_formed: u64,
}

impl BeliefState {
    /// No belief formed yet.
    pub const fn new() -> Self {
        Self {
            last_updated: None,
            hypotheses_formed: 0,
        }
    }

    /// The instant the newest belief was formed, or `None` if none ever was.
    ///
    /// What `qip_contracts::degradation::BeliefFreshness::assess` reads.
    pub const fn last_updated(&self) -> Option<Timestamp> {
        self.last_updated
    }

    /// How many hypotheses have been formed, so a test can assert that a
    /// moved instant came with a belief and not from somewhere else.
    pub const fn hypotheses_formed(&self) -> u64 {
        self.hypotheses_formed
    }

    /// Record that evidence became a belief at `at`.
    ///
    /// Crate-private on purpose: the only seam at which evidence becomes a
    /// belief is `ReasoningEngine::reason`, and a caller outside the crate
    /// that could move this instant could call the belief state current
    /// without having formed anything. The instant only ever moves forward,
    /// for the same reason the causal graph's does — a replay reasoning as of
    /// an earlier clock has still formed a belief, and it must not make the
    /// engine read less current than the newest belief it has formed.
    pub(crate) fn absorbed(&mut self, at: Timestamp) {
        self.hypotheses_formed = self.hypotheses_formed.saturating_add(1);
        self.last_updated = Some(match self.last_updated {
            Some(held) if held >= at => held,
            _ => at,
        });
    }
}
