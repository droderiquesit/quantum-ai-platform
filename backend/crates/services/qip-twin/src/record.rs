//! The compact record of one decision, kept so the platform can learn from it.
//!
//! The target for this crate says *store intelligence, not noise*, and the
//! discipline that follows from it is subtractive: a record holds the state a
//! future model would condition on and the outcome it would be scored against,
//! and nothing else. Whole order books, full agent transcripts and raw feature
//! vectors are deliberately absent — they are recoverable from the event log by
//! trace id, and a learning corpus that carries them is a copy of the event log
//! with worse indexing.
//!
//! One trap is worth naming because a previous crate found it the hard way.
//! [`qip_core::Timestamp`] serializes as RFC 3339 with **millisecond**
//! precision, so a nanosecond that is not a whole millisecond does not survive
//! a round trip, and [`Timestamp::MAX`] survives least of all.
//! [`LearningRecord::validate`] refuses such a record on the way out rather
//! than letting a corpus accumulate timestamps that quietly change when read
//! back. Use [`LearningRecord::to_json`], which validates first.

use crate::value::Simulated;
use qip_contracts::message::BookSide;
use qip_contracts::signal::{Conviction, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::{DecisionId, ObjectId};
use qip_core::lineage::TraceId;
use qip_core::time::{Duration, NANOS_PER_MILLI, Timestamp};
use qip_learning_engine::attribution::PositionAttribution;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The tape, compressed to what a model would condition on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketState {
    /// The regime label in force, e.g. `risk_off`, `trending`.
    pub regime: String,
    /// Trailing volatility. A statistic, so `f64`.
    pub volatility_f64: f64,
    /// Quoted spread at the decision.
    pub spread_bps_f64: f64,
    /// The price the decision was taken against. Exact.
    pub reference_price: Decimal,
    /// Whether the venue was open, halted, in auction.
    pub venue_status: String,
}

/// Everything outside the tape that the decision rested on.
///
/// A map rather than a struct because the world model's vocabulary changes
/// faster than this record should, and a record that has to be migrated every
/// time a new factor is added stops being written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldState {
    /// Named scalar readings: policy rate expectations, positioning, flows.
    pub factors: BTreeMap<String, f64>,
    /// One sentence on what the world was doing.
    pub narrative: String,
}

/// What one agent said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentOutput {
    pub agent: String,
    /// The position taken, e.g. `for`, `against`, `abstain`.
    pub stance: String,
    pub confidence: Conviction,
}

/// The fill, if there was one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillSummary {
    pub quantity: Decimal,
    pub price: Decimal,
    pub venue: VenueId,
    pub at: Timestamp,
}

/// What the market then did, independent of what the platform earned.
///
/// Kept separately from the P&L because a right call executed badly and a wrong
/// call executed well produce the same P&L sign and should not train the same
/// lesson.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketOutcome {
    pub horizon: Duration,
    /// The instrument's return over the horizon. A statistic.
    pub return_f64: f64,
    pub exit_price: Decimal,
}

/// The risk the decision consumed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskSummary {
    /// Value at risk attributed to the position. A statistic.
    pub var_f64: f64,
    /// Notional exposure taken on. Exact.
    pub exposure: Decimal,
    /// Fraction of the applicable limit used.
    pub limit_used_f64: f64,
}

/// One alternative, compressed to the two things worth keeping.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CounterfactualSummary {
    /// The alternative's kind, e.g. `do_not_trade`.
    pub alternative: String,
    /// Counterfactual less actual. Simulated, and serialized with its taint.
    pub difference: Simulated<Decimal>,
    /// What became of it: `filled`, `not_traded`, `unfillable`, `unfilled`.
    pub fill: String,
}

/// Everything worth remembering about one decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LearningRecord {
    pub decision_id: DecisionId,
    /// The chain this record belongs to. Always present, so the full event log
    /// is one query away and does not have to be copied in here.
    pub trace: TraceId,
    pub at: Timestamp,
    pub object_id: ObjectId,
    pub market: MarketState,
    pub world: WorldState,
    /// Feature keys with the revision each was read at, not the values. The
    /// values are reproducible from the revision; carrying them would multiply
    /// the corpus by the width of the feature space.
    pub features: Vec<(String, u64)>,
    /// The data sources the decision read.
    pub sources: Vec<String>,
    /// Model name to version, for every model that contributed.
    pub model_versions: Vec<(String, String)>,
    pub agents: Vec<AgentOutput>,
    pub strategy: StrategyId,
    /// What was decided, in one label, e.g. `filled`, `missed_opportunity`.
    pub decision: String,
    pub side: BookSide,
    /// The size that was actually put on. Exact.
    pub position_size: Decimal,
    /// How it was worked, e.g. `twap`, `participation`.
    pub execution_method: String,
    /// What execution cost. Exact.
    pub cost: Decimal,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fill: Option<FillSummary>,
    pub market_outcome: MarketOutcome,
    /// Realised P&L. Exact, and holding nothing simulated.
    pub realised_pnl: Decimal,
    /// The same P&L decomposed, keyed by
    /// [`qip_learning_engine::attribution::Source::as_str`].
    ///
    /// Taken from the attribution the LEARN stage already produces rather than
    /// recomputed here. Two decompositions of the same P&L in one platform is
    /// how a report and a corpus come to disagree about what a trade earned,
    /// and the one that gets believed is whichever was read last. Empty when
    /// the decomposition is not available; when present it must add up, which
    /// [`LearningRecord::validate`] checks.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pnl_by_source: BTreeMap<String, Decimal>,
    pub counterfactuals: Vec<CounterfactualSummary>,
    pub risk: RiskSummary,
    /// Time from the source event to the outcome.
    pub latency: Duration,
    /// What the platform believed, with the evidence behind it.
    pub confidence: Conviction,
}

impl LearningRecord {
    /// Take the P&L and its decomposition from an attribution already computed.
    ///
    /// The composition point with the LEARN stage. `qip-learning-engine`
    /// decomposes a period into where the money came from and asserts the
    /// decomposition closes exactly; this crate's job is to remember that
    /// answer next to the state it was earned in, not to compute a second one.
    pub fn with_attribution(mut self, position: &PositionAttribution) -> Self {
        self.realised_pnl = position.total;
        self.pnl_by_source = position.components.clone();
        self
    }

    /// Every timestamp the record carries, for the representability check.
    fn timestamps(&self) -> Vec<(&'static str, Timestamp)> {
        let mut stamps = vec![("at", self.at)];
        if let Some(fill) = &self.fill {
            stamps.push(("fill.at", fill.at));
        }
        stamps
    }

    /// Refuse a record that would not survive being written and read back.
    ///
    /// The check is not paranoia about a hypothetical. `Timestamp` renders as
    /// RFC 3339 with millisecond precision, so a sub-millisecond instant loses
    /// its tail and [`Timestamp::MAX`] — the point-in-time sentinel meaning "no
    /// upper bound" — comes back as a different instant entirely. A corpus that
    /// silently shifts its own timestamps trains models on a history that never
    /// happened.
    pub fn validate(&self) -> Result<()> {
        if self.trace.as_str().trim().is_empty() {
            return Err(Error::invalid(
                "a learning record needs a trace id; a lesson nobody can trace back is not evidence",
            ));
        }
        for (field, stamp) in self.timestamps() {
            if stamp == Timestamp::MAX {
                return Err(Error::schema(format!(
                    "{field} is Timestamp::MAX, the point-in-time sentinel; it does not survive millisecond serialization and is not an instant anything happened at"
                )));
            }
            if stamp.as_nanos() % NANOS_PER_MILLI != 0 {
                return Err(Error::schema(format!(
                    "{field} is {stamp} with sub-millisecond precision, which a round trip would truncate; round it before recording it"
                )));
            }
        }
        if !self.pnl_by_source.is_empty() {
            let attributed = self
                .pnl_by_source
                .values()
                .fold(Decimal::ZERO, |a, b| a + *b);
            // The same exactness the attribution itself insists on. A
            // decomposition that nearly adds up hides exactly the component
            // nobody understood, and a corpus is the worst place for it to hide.
            if (attributed - self.realised_pnl).abs() > Decimal::from_raw(1) {
                return Err(Error::invalid(format!(
                    "the P&L decomposition sums to {attributed} against a realised {}; an attribution that nearly adds up hides the part nobody understood",
                    self.realised_pnl
                )));
            }
        }
        Ok(())
    }

    /// Serialize, refusing anything that would not come back unchanged.
    pub fn to_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string(self).map_err(|error| {
            Error::schema(format!("a learning record would not serialize: {error}"))
        })
    }

    /// Read one back.
    pub fn from_json(json: &str) -> Result<Self> {
        let record: Self = serde_json::from_str(json).map_err(|error| {
            Error::schema(format!("a learning record would not parse: {error}"))
        })?;
        record.validate()?;
        Ok(record)
    }
}
