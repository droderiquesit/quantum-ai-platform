//! Signals.

use qip_core::{ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use qip_numerics::stats;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The horizon a signal speaks to.
///
/// Carried explicitly because a signal is only meaningful over the horizon it
/// was measured for. Mixing a twelve-month momentum score with an intraday
/// order-flow reading, unlabelled, produces a portfolio sized for neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Horizon {
    /// Seconds to minutes. Fast Brain territory.
    Intraday,
    /// Days.
    ShortTerm,
    /// Weeks to a few months.
    MediumTerm,
    /// Six months and beyond.
    LongTerm,
}

impl Horizon {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Intraday => "intraday",
            Self::ShortTerm => "short_term",
            Self::MediumTerm => "medium_term",
            Self::LongTerm => "long_term",
        }
    }

    /// Typical holding period in trading days, used to scale expected cost
    /// against expected return.
    pub fn typical_holding_days(&self) -> f64 {
        match self {
            Self::Intraday => 0.5,
            Self::ShortTerm => 5.0,
            Self::MediumTerm => 40.0,
            Self::LongTerm => 180.0,
        }
    }

    /// Whether the signal belongs on the latency-critical path.
    pub fn is_fast(&self) -> bool {
        matches!(self, Self::Intraday)
    }
}

/// What a signal measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Momentum,
    MeanReversion,
    Value,
    Quality,
    Growth,
    Carry,
    Volatility,
    Liquidity,
    Sentiment,
    OrderFlow,
    EarningsRevision,
    CreditQuality,
    MacroSurprise,
    AlternativeData,
}

impl SignalKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Momentum => "momentum",
            Self::MeanReversion => "mean_reversion",
            Self::Value => "value",
            Self::Quality => "quality",
            Self::Growth => "growth",
            Self::Carry => "carry",
            Self::Volatility => "volatility",
            Self::Liquidity => "liquidity",
            Self::Sentiment => "sentiment",
            Self::OrderFlow => "order_flow",
            Self::EarningsRevision => "earnings_revision",
            Self::CreditQuality => "credit_quality",
            Self::MacroSurprise => "macro_surprise",
            Self::AlternativeData => "alternative_data",
        }
    }

    /// The horizon the signal is normally measured over.
    pub fn natural_horizon(&self) -> Horizon {
        match self {
            Self::OrderFlow => Horizon::Intraday,
            Self::MeanReversion | Self::Liquidity | Self::Sentiment => Horizon::ShortTerm,
            Self::Momentum
            | Self::Volatility
            | Self::EarningsRevision
            | Self::MacroSurprise
            | Self::AlternativeData => Horizon::MediumTerm,
            Self::Value | Self::Quality | Self::Growth | Self::Carry | Self::CreditQuality => {
                Horizon::LongTerm
            }
        }
    }
}

/// One scalar view on one instrument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub object_id: ObjectId,
    pub kind: SignalKind,
    /// Standardised score, typically in `[-3, 3]`. Positive is bullish.
    pub score: f64,
    pub horizon: Horizon,
    /// Confidence in the score, in `[0, 1]`.
    pub confidence: f64,
    /// When the signal was computed.
    pub at: Timestamp,
    /// Number of observations behind the score. A signal from ten points is
    /// not the same as one from a thousand, and the difference must survive.
    pub sample_size: usize,
    /// Feature names the signal was computed from, for lineage.
    pub inputs: Vec<String>,
}

impl Signal {
    pub fn new(
        object_id: ObjectId,
        kind: SignalKind,
        score: f64,
        at: Timestamp,
        sample_size: usize,
    ) -> Self {
        Self {
            object_id,
            horizon: kind.natural_horizon(),
            kind,
            score,
            // Confidence grows with sample size and saturates; a signal from a
            // handful of observations is barely a signal.
            confidence: (sample_size as f64 / (sample_size as f64 + 40.0)).clamp(0.0, 0.95),
            at,
            sample_size,
            inputs: Vec::new(),
        }
    }

    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    pub fn with_inputs(mut self, inputs: Vec<String>) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn with_horizon(mut self, horizon: Horizon) -> Self {
        self.horizon = horizon;
        self
    }

    /// Score weighted by confidence — what a portfolio should actually act on.
    pub fn effective_score(&self) -> f64 {
        self.score * self.confidence
    }

    /// Whether the signal is strong enough to be worth trading.
    pub fn is_actionable(&self, threshold: f64) -> bool {
        self.effective_score().abs() >= threshold && self.sample_size >= 20
    }

    /// Whether the expected move covers the cost of trading it.
    ///
    /// The check that stops a real but tiny edge from being traded away in
    /// spread. `expected_move_bps` is what the score implies; `cost_bps` is the
    /// round-trip cost.
    pub fn survives_costs(&self, expected_move_bps: f64, cost_bps: f64) -> bool {
        expected_move_bps.abs() * self.confidence > cost_bps
    }
}

impl EventBody for Signal {
    const TOPIC: Topic = Topic::SignalGenerated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}",
            self.object_id,
            self.kind.as_str(),
            self.at.as_nanos()
        ))
    }
}

/// A collection of signals across instruments and kinds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SignalSet {
    signals: Vec<Signal>,
}

impl SignalSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, signal: Signal) {
        self.signals.push(signal);
    }

    pub fn len(&self) -> usize {
        self.signals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.signals.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Signal> {
        self.signals.iter()
    }

    pub fn of_kind(&self, kind: SignalKind) -> Vec<&Signal> {
        self.signals.iter().filter(|s| s.kind == kind).collect()
    }

    pub fn for_object(&self, object_id: &ObjectId) -> Vec<&Signal> {
        self.signals
            .iter()
            .filter(|s| &s.object_id == object_id)
            .collect()
    }

    /// Combine every signal for each instrument into one score.
    ///
    /// Weighted by confidence, and only over signals sharing a horizon —
    /// blending a long-term value score with an intraday flow reading produces
    /// a number that means nothing.
    pub fn composite(&self, horizon: Horizon) -> BTreeMap<String, f64> {
        let mut totals: BTreeMap<String, (f64, f64)> = BTreeMap::new();
        for signal in self.signals.iter().filter(|s| s.horizon == horizon) {
            let entry = totals
                .entry(signal.object_id.as_str().to_string())
                .or_insert((0.0, 0.0));
            entry.0 += signal.score * signal.confidence;
            entry.1 += signal.confidence;
        }
        totals
            .into_iter()
            .map(|(object, (weighted, weight))| {
                (
                    object,
                    if weight > 1e-12 {
                        weighted / weight
                    } else {
                        0.0
                    },
                )
            })
            .collect()
    }

    /// Cross-sectionally standardise scores within each kind.
    ///
    /// Raw signal levels are not comparable across kinds; ranks within a kind
    /// are. This is what makes a composite meaningful.
    pub fn standardised(&self) -> SignalSet {
        let mut out = Vec::with_capacity(self.signals.len());
        let mut kinds: Vec<SignalKind> = self.signals.iter().map(|s| s.kind).collect();
        kinds.sort();
        kinds.dedup();

        for kind in kinds {
            let group: Vec<&Signal> = self.signals.iter().filter(|s| s.kind == kind).collect();
            let scores: Vec<f64> = group.iter().map(|s| s.score).collect();
            // Robust standardisation: a single outlier should not compress
            // every other score toward zero.
            let z = stats::robust_z_scores(&scores);
            for (signal, score) in group.into_iter().zip(z) {
                out.push(Signal {
                    score: score.clamp(-3.0, 3.0),
                    ..signal.clone()
                });
            }
        }
        SignalSet { signals: out }
    }

    /// The strongest signals by effective score.
    pub fn strongest(&self, limit: usize) -> Vec<&Signal> {
        let mut ranked: Vec<&Signal> = self.signals.iter().collect();
        ranked.sort_by(|a, b| {
            b.effective_score()
                .abs()
                .partial_cmp(&a.effective_score().abs())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.object_id.cmp(&b.object_id))
        });
        ranked.truncate(limit);
        ranked
    }
}

/// Momentum: trailing return over `window`, standardised by its volatility.
///
/// Volatility-scaling is what makes momentum comparable across instruments; a
/// 10% move in a bond and in a small-cap are not the same signal.
pub fn momentum(closes: &[f64], window: usize) -> Option<f64> {
    if closes.len() < window + 1 || window == 0 {
        return None;
    }
    let recent = &closes[closes.len() - window - 1..];
    let total = recent[recent.len() - 1] / recent[0] - 1.0;
    let volatility = stats::stddev(&stats::log_returns(recent)) * (window as f64).sqrt();
    if volatility <= 1e-9 {
        return None;
    }
    Some(total / volatility)
}

/// Mean reversion: the negated z-score of the latest price against its window.
pub fn mean_reversion(closes: &[f64], window: usize) -> Option<f64> {
    if closes.len() < window || window < 3 {
        return None;
    }
    let recent = &closes[closes.len() - window..];
    let mean = stats::mean(recent);
    let sd = stats::stddev(recent);
    if sd <= 1e-12 {
        return None;
    }
    Some(-(recent[recent.len() - 1] - mean) / sd)
}

/// Realised volatility over a window, annualised.
pub fn realised_volatility(closes: &[f64], window: usize, periods_per_year: f64) -> Option<f64> {
    if closes.len() < window + 1 {
        return None;
    }
    let recent = &closes[closes.len() - window - 1..];
    Some(stats::stddev(&stats::log_returns(recent)) * periods_per_year.sqrt())
}

/// Volatility-adjusted carry: yield pickup per unit of risk.
pub fn carry(yield_rate: f64, funding_rate: f64, volatility: f64) -> Option<f64> {
    if volatility <= 1e-9 {
        return None;
    }
    Some((yield_rate - funding_rate) / volatility)
}
