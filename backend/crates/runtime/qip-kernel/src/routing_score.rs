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

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_cost_router::{Judgement, ReputationBook};
use qip_events::log::EventLog;
use qip_events::{EventBody, Topic};
use qip_streaming::envelope::StreamEnvelope;
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

/// The book a sequence of scored routings builds, in the order given.
///
/// A routing score is booked from the record, never from a fresh reading, so
/// the book after a restart is the book the log can reproduce: the same
/// `observe` the cycle called when it journaled the record (ADR 0680,
/// R12-MODEL-10123 item 3).
pub fn book_from<'a>(scored: impl IntoIterator<Item = &'a RoutingScored>) -> ReputationBook {
    let mut book = ReputationBook::new();
    for record in scored {
        book.observe(&record.judgement, record.correct);
    }
    book
}

/// The routing reputation book rebuilt from every [`RoutingScored`] the log
/// holds, so a restarted platform continues the record rather than starting
/// the rungs again from no evidence.
///
/// Not rebuilt: a judgement still held between REASON and LEARN when the
/// process stopped. It was never scored, so the log has nothing to rebuild it
/// from; it is dropped, as `Platform::score_routings` documents.
pub fn resume_book(log: &EventLog) -> Result<ReputationBook> {
    let mut scored = Vec::new();
    for record in log.records() {
        if record.event.topic == RoutingScored::TOPIC
            && record.event.lineage.producer == ROUTING_SCORE_ORIGIN
        {
            scored.push(
                StreamEnvelope::from_frame(&record.event)?
                    .decode::<RoutingScored>()?
                    .body,
            );
        }
    }
    Ok(book_from(&scored))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_cost_router::{Conditions, Horizon, MarketRegime, Region, VolatilityRegime};
    use qip_financial::asset_class::AssetClass;

    fn scored(hypothesis: &str, correct: bool) -> RoutingScored {
        let conditions = Conditions::new(
            AssetClass::Equity,
            Region::new("us-east"),
            MarketRegime::Trending,
            VolatilityRegime::Normal,
            Horizon::Intraday,
        );
        let judgement: Judgement = serde_json::from_value(serde_json::json!({
            "model": "multi_agent_reasoning@qip-kernel-test",
            "conditions": conditions,
        }))
        .expect("a rung reference and its conditions mint a judgement");
        RoutingScored {
            cycle: 1,
            hypothesis_id: hypothesis.to_string(),
            judgement,
            verdict: "confirmed".to_string(),
            correct,
            at: Timestamp::from_secs(1_760_000_000),
        }
    }

    /// The rebuilt book holds exactly what the records say: three scored
    /// routings, two correct, in one cell. Premise first: the book built from
    /// nothing is empty.
    ///
    /// Mutation: not run (nothing was compiled, owner instruction 2026-11-15).
    #[test]
    fn a_book_rebuilt_from_scored_routings_holds_each_rungs_record() {
        assert!(
            book_from(&Vec::<RoutingScored>::new()).is_empty(),
            "premise: no records, no book"
        );
        let records = [scored("h1", true), scored("h2", true), scored("h3", false)];
        let book = book_from(&records);
        let rung = records[0].judgement.model().to_string();
        let record = book
            .reputation(&rung)
            .expect("the rung has a record")
            .record(records[0].judgement.conditions());
        assert_eq!(record.observations, 3);
        assert_eq!(record.correct, 2);
    }
}
