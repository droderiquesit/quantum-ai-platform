//! A compute-routing choice, scored against the answer it bought (ADR 0391,
//! R12-MODEL-10123).
//!
//! REASON's routing is the one routed decision here that can be wrong: the
//! cost router places "whether to act on this opportunity" on a rung and the
//! platform convenes a reasoner or declines. What that buys is a thesis, and
//! LEARN grades theses. Until this module the grade never reached the
//! routing: `qip_cost_router::ReputationBook` had no caller outside its own
//! crate's tests, so "the router keeps a reputation" described a type.
//!
//! The record here is what LEARN journals when it resolves a routing's
//! judgement. It is a score and nothing reads it back into a routing, a
//! budget or a size; letting it narrow one is a later decision.

use qip_core::time::Timestamp;
use qip_cost_router::Judgement;
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

/// The producer every [`RoutingScored`] record is journaled under.
pub const ROUTING_SCORE_ORIGIN: &str = "kernel/routing-score";

/// The build a rung's record is kept under: a new build's reasoner does not
/// inherit the record of the one it replaced.
pub fn rung_build() -> String {
    format!("qip-kernel-{}", env!("CARGO_PKG_VERSION"))
}

/// One routing choice resolved against the thesis it bought.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingScored {
    /// The cycle whose LEARN stage resolved it.
    pub cycle: u64,
    /// The hypothesis the convened rung produced, whose thesis was graded.
    pub hypothesis_id: String,
    /// The judgement minted at the routing decision: the rung that ran and
    /// the conditions it was chosen under.
    pub judgement: Judgement,
    /// The thesis's verdict, as `Verdict::as_str`.
    pub verdict: String,
    /// Whether the verdict counts as correct.
    pub correct: bool,
    pub at: Timestamp,
}

impl EventBody for RoutingScored {
    const TOPIC: Topic = Topic::ModelEvaluated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("routing-scored:{}", self.hypothesis_id))
    }
}
