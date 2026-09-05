//! The episode record and its fixed feature encoding.
//!
//! An [`Episode`] is what the platform keeps of one reasoned situation once
//! the raw material — bars, findings, the hypothesis text — has been folded
//! into the world model and discarded (blueprint §10.1, §32). It is compressed
//! meaning, not retained observation: a few dozen numbers and short labels.
//!
//! The encoding into a fixed-length vector is stated in full at
//! [`EPISODE_DIMENSIONS`] so that a neighbour returned by the index can be
//! explained in terms of the fields that made it near, rather than by
//! pointing at a model nobody can inspect.

use crate::embedding::Embedding;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256;
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// Length of the episode feature vector.
///
/// The layout, by index. Every block is a pure function of the named field
/// with no learned weight, and an absent or unrecognised label encodes as
/// zeros in its block rather than as a guess:
///
/// | Index | Field | Encoding |
/// |---|---|---|
/// | 0–7 | instrument | eight signs from `sha256(instrument)` bytes 0–7 (high bit set → `+1/√8`, else `−1/√8`), so distinct instruments are near-orthogonal and the same instrument is identical |
/// | 8–12 | market regime | one-hot over `trending, mean_reverting, crisis, illiquid, quiet` |
/// | 13–16 | volatility regime | one-hot over `low, normal, high, extreme` |
/// | 17–24 | claim | one-hot over `overvalued, undervalued, volatility_underpriced, volatility_overpriced, spread_widens, spread_narrows, regime_shift, event_occurs` |
/// | 25 | claim direction | `+1`, `−1` or `0` |
/// | 26 | claim confidence | as stated, in `[0, 1]` |
/// | 27 | findings coverage | runs that produced a finding over runs asked, in `[0, 1]` |
/// | 28 | mean analyst conviction | over the stances, in `[0, 1]`; `0` with no stances |
/// | 29 | positive stance share | stances positive over all stances |
/// | 30 | negative stance share | stances negative over all stances |
/// | 31 | horizon | `ln(1 + days) / ln(1 + 365)`, so a year encodes as `1` and longer saturates |
///
/// Similarity is cosine over this vector, so the instrument block (norm 1)
/// and the categorical blocks (norm 1 each when set) carry equal weight and
/// the scalar block is a tie-breaker among episodes that share them. That is
/// the intended ranking: same name in the same regime with the same claim
/// first, then by how the panel and the platform actually leaned.
pub const EPISODE_DIMENSIONS: usize = 32;

/// The model name stamped on every episode embedding.
///
/// [`Embedding::cosine_similarity`] returns zero across models, so an index
/// built under one encoding version cannot silently rank vectors from
/// another. Bump this when the layout above changes.
pub const EPISODE_ENCODING: &str = "episode-fixed-v1";

const MARKET_REGIMES: [&str; 5] = ["trending", "mean_reverting", "crisis", "illiquid", "quiet"];
const VOLATILITY_REGIMES: [&str; 4] = ["low", "normal", "high", "extreme"];
const CLAIMS: [&str; 8] = [
    "overvalued",
    "undervalued",
    "volatility_underpriced",
    "volatility_overpriced",
    "spread_widens",
    "spread_narrows",
    "regime_shift",
    "event_occurs",
];

const INSTRUMENT_AT: usize = 0;
const MARKET_AT: usize = 8;
const VOLATILITY_AT: usize = 13;
const CLAIM_AT: usize = 17;
const DIRECTION_AT: usize = 25;
const CONFIDENCE_AT: usize = 26;
const COVERAGE_AT: usize = 27;
const CONVICTION_AT: usize = 28;
const POSITIVE_AT: usize = 29;
const NEGATIVE_AT: usize = 30;
const HORIZON_AT: usize = 31;

/// The regime in force when the episode was formed, as the labels the cost
/// router's closed enums print.
///
/// Strings rather than the enums themselves because this crate is a library
/// below the services and may not depend on `qip-cost-router`; the one-hot
/// tables above are the closed sets, and a label outside them encodes as
/// zeros rather than being refused, so a new regime added upstream degrades
/// retrieval instead of stopping the cycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegimeLabel {
    pub market: String,
    pub volatility: String,
}

/// What the detectors and the panel produced, in aggregate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FindingsSummary {
    /// Agent runs asked.
    pub runs: usize,
    /// Findings that came back.
    pub findings: usize,
    /// Runs that produced a finding over runs asked, in `[0, 1]`.
    pub coverage: f64,
    /// Whether the panel disagreed on direction.
    pub contested: bool,
}

/// Which way an analyst leaned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StanceDirection {
    Positive,
    Negative,
    /// A view that the move is two-sided.
    Ambiguous,
    /// No view.
    Neutral,
}

impl StanceDirection {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
            Self::Ambiguous => "ambiguous",
            Self::Neutral => "neutral",
        }
    }
}

/// One analyst's position on the question, kept by name so a precedent can
/// say who was right last time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalystStance {
    pub agent_id: String,
    pub direction: StanceDirection,
    /// In `[0, 1]`.
    pub conviction: f64,
}

/// What the hypothesis claimed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaimRecord {
    /// The hypothesis class, e.g. the anomaly kind that raised it.
    pub class: String,
    /// The claim label, one of the eight the reasoning engine names.
    pub claim: String,
    /// `+1`, `−1`, or `0` where the claim has no inherent direction.
    pub direction: f64,
    /// The effective confidence after review, in `[0, 1]`.
    pub confidence: f64,
}

/// What the platform did with the hypothesis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionTaken {
    /// Approved on review and handed to construction as a thesis.
    Approved,
    /// The red team rejected it.
    RejectedOnReview,
    /// Approved, but no thesis could be sized from it.
    NotSizeable,
}

impl DecisionTaken {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::RejectedOnReview => "rejected_on_review",
            Self::NotSizeable => "not_sizeable",
        }
    }
}

/// What followed, once the claim resolved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeOutcome {
    pub resolved_at: Timestamp,
    /// The move the platform's own series recorded over the horizon, in
    /// basis points of the reference the claim was made against.
    pub realised_move_bps: f64,
    /// Realised P&L attributed to the hypothesis, as a statistic.
    pub realised_pnl: f64,
}

impl EpisodeOutcome {
    /// Whether the realised move went the way `direction` claimed.
    ///
    /// `None` where either side has no sign: a directionless claim cannot be
    /// agreed with, and a move of exactly zero agrees with nothing.
    pub fn agrees_with(&self, direction: f64) -> Option<bool> {
        if direction == 0.0 || self.realised_move_bps == 0.0 {
            return None;
        }
        Some(direction.is_sign_positive() == self.realised_move_bps.is_sign_positive())
    }
}

/// One reasoned situation and what came of it.
///
/// `at` is when the situation was true; `known_at` is when the record became
/// knowable, which for a resolved episode is the resolution instant. An
/// episode is retrievable only after `known_at`, which is what keeps a
/// backtest from recalling an outcome the platform had not yet seen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub episode_id: String,
    pub instrument: String,
    pub regime: RegimeLabel,
    pub findings: FindingsSummary,
    /// In agent-id order, so two episodes from the same panel encode and
    /// serialise identically.
    pub stances: Vec<AnalystStance>,
    pub claim: ClaimRecord,
    pub horizon: Duration,
    pub decision: DecisionTaken,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub outcome: Option<EpisodeOutcome>,
    pub at: Timestamp,
    pub known_at: Timestamp,
}

impl Episode {
    /// Refuse an episode the index could not honestly hold.
    ///
    /// A `known_at` before `at` is a record knowable before it was true —
    /// the exact leakage the bitemporal stamp exists to prevent — and a
    /// confidence outside `[0, 1]` would put the vector off the scale every
    /// other episode was encoded on.
    pub fn validate(&self) -> Result<()> {
        if self.episode_id.is_empty() {
            return Err(Error::invalid("an episode needs an id"));
        }
        if self.instrument.is_empty() {
            return Err(Error::invalid(format!(
                "episode {} names no instrument",
                self.episode_id
            )));
        }
        if self.known_at < self.at {
            return Err(Error::invalid(format!(
                "episode {} is known at {} but true at {}; a record cannot be knowable before \
                 it was true",
                self.episode_id,
                self.known_at.to_rfc3339(),
                self.at.to_rfc3339()
            )));
        }
        if !(0.0..=1.0).contains(&self.claim.confidence) {
            return Err(Error::invalid(format!(
                "episode {} has confidence {}, outside [0, 1]",
                self.episode_id, self.claim.confidence
            )));
        }
        if !(0.0..=1.0).contains(&self.findings.coverage) {
            return Err(Error::invalid(format!(
                "episode {} has coverage {}, outside [0, 1]",
                self.episode_id, self.findings.coverage
            )));
        }
        if let Some(stance) = self
            .stances
            .iter()
            .find(|stance| !(0.0..=1.0).contains(&stance.conviction))
        {
            return Err(Error::invalid(format!(
                "episode {} records {} at conviction {}, outside [0, 1]",
                self.episode_id, stance.agent_id, stance.conviction
            )));
        }
        if self.horizon.as_nanos() < 0 {
            return Err(Error::invalid(format!(
                "episode {} has a negative horizon",
                self.episode_id
            )));
        }
        Ok(())
    }

    /// The fixed encoding, per [`EPISODE_DIMENSIONS`].
    pub fn embedding(&self) -> Embedding {
        encode(
            &self.instrument,
            &self.regime,
            Some(&self.claim),
            Some(&self.findings),
            &self.stances,
            self.horizon,
        )
    }

    /// The query this episode would have been, before its outcome was known.
    pub fn as_query(&self) -> EpisodeQuery {
        EpisodeQuery {
            instrument: self.instrument.clone(),
            regime: self.regime.clone(),
            claim: Some(self.claim.clone()),
            findings: Some(self.findings.clone()),
            stances: self.stances.clone(),
            horizon: self.horizon,
        }
    }
}

/// A situation to find precedents for.
///
/// The same fields as an [`Episode`] minus the things that are not yet known
/// when the question is asked: the claim and findings are optional because
/// the REASON stage may recall before the panel has reported, and absent
/// blocks encode as zeros.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeQuery {
    pub instrument: String,
    pub regime: RegimeLabel,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub claim: Option<ClaimRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub findings: Option<FindingsSummary>,
    pub stances: Vec<AnalystStance>,
    pub horizon: Duration,
}

impl EpisodeQuery {
    /// The fixed encoding, per [`EPISODE_DIMENSIONS`].
    pub fn embedding(&self) -> Embedding {
        encode(
            &self.instrument,
            &self.regime,
            self.claim.as_ref(),
            self.findings.as_ref(),
            &self.stances,
            self.horizon,
        )
    }
}

fn one_hot(values: &mut [f32], at: usize, table: &[&str], label: &str) {
    if let Some(index) = table.iter().position(|entry| *entry == label) {
        values[at + index] = 1.0;
    }
}

fn encode(
    instrument: &str,
    regime: &RegimeLabel,
    claim: Option<&ClaimRecord>,
    findings: Option<&FindingsSummary>,
    stances: &[AnalystStance],
    horizon: Duration,
) -> Embedding {
    let mut values = vec![0.0_f32; EPISODE_DIMENSIONS];

    // Instrument identity: eight signs from the digest, unit norm as a block.
    let digest = sha256(instrument.as_bytes());
    let scale = 1.0 / 8.0_f32.sqrt();
    for (offset, byte) in digest.iter().take(8).enumerate() {
        values[INSTRUMENT_AT + offset] = if *byte >= 128 { scale } else { -scale };
    }

    one_hot(&mut values, MARKET_AT, &MARKET_REGIMES, &regime.market);
    one_hot(
        &mut values,
        VOLATILITY_AT,
        &VOLATILITY_REGIMES,
        &regime.volatility,
    );

    if let Some(claim) = claim {
        one_hot(&mut values, CLAIM_AT, &CLAIMS, &claim.claim);
        // Not `signum`, which calls zero positive.
        values[DIRECTION_AT] = if claim.direction > 0.0 {
            1.0
        } else if claim.direction < 0.0 {
            -1.0
        } else {
            0.0
        };
        values[CONFIDENCE_AT] = claim.confidence as f32;
    }
    if let Some(findings) = findings {
        values[COVERAGE_AT] = findings.coverage as f32;
    }
    if !stances.is_empty() {
        let count = stances.len() as f32;
        let conviction: f32 = stances.iter().map(|s| s.conviction as f32).sum::<f32>() / count;
        let positive = stances
            .iter()
            .filter(|s| s.direction == StanceDirection::Positive)
            .count() as f32
            / count;
        let negative = stances
            .iter()
            .filter(|s| s.direction == StanceDirection::Negative)
            .count() as f32
            / count;
        values[CONVICTION_AT] = conviction;
        values[POSITIVE_AT] = positive;
        values[NEGATIVE_AT] = negative;
    }
    // Statistic to feature: the horizon is a duration and becomes a float
    // here, on a log scale so a day and a week are far apart and a year and
    // two years are not.
    let days = horizon.as_days_f64().max(0.0);
    values[HORIZON_AT] = ((1.0 + days).ln() / (1.0 + 365.0_f64).ln()).min(1.0) as f32;

    Embedding::new(values, EPISODE_ENCODING)
}
