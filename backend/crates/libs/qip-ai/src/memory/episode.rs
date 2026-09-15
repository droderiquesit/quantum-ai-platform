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
/// | 32 | book drawdown | as stated, in `[0, 1]` |
/// | 33 | volatility ratio | `r / (1 + r)`, so "as usual" is `0.5` and an unmeasured state is `0` |
/// | 34 | spread ratio | `r / (1 + r)`, likewise |
/// | 35 | recent return direction | `+1`, `−1` or `0` |
/// | 36 | recent return magnitude | `m / (1 + m)` on `m = |bps| / 10_000`, the move as a fraction of its own reference |
/// | 37 | causal in-edges | `k / (1 + k)` on the number of edges recorded as active |
/// | 38 | mean transmission | over those edges, in `[0, 1]`; `0` with none |
/// | 39 | established share | edges with no suspected confounder over all of them |
///
/// Similarity is cosine over this vector, so the instrument block (norm 1)
/// and the categorical blocks (norm 1 each when set) carry equal weight and
/// the scalar block ranks among episodes that share them. That is the
/// intended ranking: same name in the same regime with the same claim first,
/// then by the state the platform was in and how the panel leaned.
///
/// **The bounded maps in indices 33, 34, 36 and 37 are `x / (1 + x)` and not
/// a squash against a chosen scale**, because a chosen scale is a number
/// nobody measured sitting inside the thing retrieval ranks on. `x / (1 + x)`
/// is monotone, fixes `0` at `0`, saturates at `1`, and introduces no
/// constant at all; the one constant that does appear — the `10_000` in
/// index 36 — is the definition of a basis point rather than a threshold.
///
/// **This is not the blueprint's "few hundred numbers" and deliberately is
/// not.** §10.1 gives that figure while sizing years of episodes at
/// single-digit gigabytes; it is an illustration of storage, not a length
/// this encoding must reach. Padding to it would mean dimensions computed
/// from nothing, and a retrieval ranked partly on dimensions nobody measured
/// is worse than a short vector, because it reads as a richer state.
pub const EPISODE_DIMENSIONS: usize = 40;

/// How much of the vector the index buckets on: the leading
/// identity-and-reasoning block, indices `0..32`.
///
/// **The index gathers candidates on what question was asked about what name;
/// it does not gather on the state the market was in.** Ranking still uses
/// the whole vector — the store re-ranks its candidates by exact cosine over
/// all [`EPISODE_DIMENSIONS`] — so the state and the causal context do decide
/// which neighbour comes back first. They just do not decide which episodes
/// are looked at.
///
/// The distinction is not fastidiousness, and it was found by a test rather
/// than reasoned out in advance. The state block is continuous and moves with
/// every bar; the hash bucket is the sign of a random projection. Bucketing
/// on the state put the *same* claim about the *same* instrument in a
/// different bucket once the tape had swung, the one-bit probe missed it, and
/// the memory answered "no precedent" — precisely in the situation precedent
/// is wanted for, because a market that has moved is the reason anyone asks
/// whether this has happened before. The kernel's
/// `the_kernel_records_precedents_on_a_hypothesis_once_prior_episodes_resolved_and_leaves_the_confidence_alone`
/// is where that was caught, on the state block as it then stood.
///
/// **The guard is `tests/episodic.rs`'s
/// `the_state_a_market_was_in_ranks_the_neighbours_and_never_moves_the_bucket_they_are_found_in`,
/// and naming the kernel test instead would have been a lie by the time this
/// line was written.** Raising this constant to [`EPISODE_DIMENSIONS`] makes
/// the guard fail and leaves the whole kernel suite green — because whether
/// a probe misses depends on how far *that fixture's* two states happen to
/// sit apart, and the fixture's state block was rewritten hours later to read
/// the closes rather than the surprise series. A test that catches a fault
/// only when the fixture is unlucky is not the guard; the one that asserts
/// the bucket directly is.
pub const EPISODE_INDEX_DIMENSIONS: usize = 32;

const _: () = assert!(
    EPISODE_INDEX_DIMENSIONS <= EPISODE_DIMENSIONS,
    "the index cannot hash more of the vector than the vector has"
);

/// The model name stamped on every episode embedding.
///
/// [`Embedding::cosine_similarity`] returns zero across models, so an index
/// built under one encoding version cannot silently rank vectors from
/// another. Bump this when the layout above changes.
pub const EPISODE_ENCODING: &str = "episode-fixed-v2";

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
const DRAWDOWN_AT: usize = 32;
const VOLATILITY_RATIO_AT: usize = 33;
const SPREAD_RATIO_AT: usize = 34;
const RETURN_SIGN_AT: usize = 35;
const RETURN_MAGNITUDE_AT: usize = 36;
const EDGE_COUNT_AT: usize = 37;
const TRANSMISSION_AT: usize = 38;
const ESTABLISHED_AT: usize = 39;

/// A quantity in `[0, ∞)` mapped into `[0, 1)`, monotone, with `0` at `0`.
///
/// The map every ratio in the encoding goes through. See
/// [`EPISODE_DIMENSIONS`] for why it is this and not a squash against a
/// scale somebody picked.
fn saturating(x: f64) -> f32 {
    if !x.is_finite() || x <= 0.0 {
        return 0.0;
    }
    (x / (1.0 + x)) as f32
}

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

/// The compressed market and world state at the instant the episode was
/// formed — blueprint §10.1's `state_vector`, as the fields it is computed
/// from rather than as an opaque array.
///
/// Kept as named quantities and encoded by [`EPISODE_DIMENSIONS`] rather than
/// stored pre-encoded, for the reason the module doc gives for the whole
/// encoding: a neighbour has to be explainable by the facts that made it
/// near. An array of forty floats on the record would also be a second
/// statement of a state the named fields already hold, and the two would
/// disagree the first time the layout changed with no way to tell which was
/// the episode.
///
/// Every field is a ratio or a fraction the platform already measures for its
/// own regime decisions. None is a threshold and none is fitted.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketState {
    /// The book's drawdown at the instant, as a fraction in `[0, 1]`. The
    /// "world" half of the state: the same tape met from a full book and from
    /// a drawn-down one is not the same situation.
    pub drawdown: f64,
    /// The subject's recent realised deviation over its own long-run
    /// deviation. `1.0` is "as usual"; being a ratio it carries no unit and
    /// no asset-class bias, which is why a fixed basis-point threshold is not
    /// used: one would call every digital asset extreme and every government
    /// bond calm, and say something about the asset class rather than the
    /// day.
    ///
    /// `None` where there was not enough history to form the quotient, and an
    /// option rather than a neutral `1.0` because "as usual" is a claim about
    /// the market and "not measured" is a claim about the platform. A reader
    /// who cannot tell those apart will read a cold start as a calm day.
    pub volatility_ratio: Option<f64>,
    /// The latest quoted spread over the subject's own median spread, on the
    /// same argument, and optional for the same reason.
    pub spread_ratio: Option<f64>,
    /// The subject's return over the recent window, in basis points, signed;
    /// `None` where the series could not be differenced.
    pub recent_return_bps: Option<f64>,
    /// How many observations of the subject the three figures above rest on.
    ///
    /// Recorded and deliberately **not** encoded. An episode is retrieved by
    /// the state the platform was in, not by how much tape produced that
    /// state; but a reader comparing two episodes needs to be able to see
    /// when one of them rests on four bars.
    pub observations: usize,
}

impl MarketState {
    /// Refuse a state the encoder could not honestly place on the scale every
    /// other episode sits on.
    ///
    /// Refused rather than corrected, and the distinction bites here. A ratio
    /// of `NaN` — which a degenerate series produces from a zero denominator
    /// — maps through [`saturating`] to `0.0`, which is exactly the value an
    /// *unmeasured* state encodes as. A broken measurement would therefore
    /// arrive at the index wearing the label "nothing was measured", and no
    /// reader of the vector or of the record could tell the two apart.
    fn validate(&self, episode_id: &str) -> Result<()> {
        if !(0.0..=1.0).contains(&self.drawdown) {
            return Err(Error::invalid(format!(
                "episode {episode_id} records a drawdown of {}, outside [0, 1]; a drawdown is a \
                 fraction of the high-water mark, so fix the capital reading rather than the \
                 episode",
                self.drawdown
            )));
        }
        for (name, value) in [
            ("volatility_ratio", self.volatility_ratio),
            ("spread_ratio", self.spread_ratio),
        ] {
            let Some(value) = value else { continue };
            if !value.is_finite() || value < 0.0 {
                return Err(Error::invalid(format!(
                    "episode {episode_id} records {name} as {value}; a ratio of one measured \
                     quantity to another is finite and not negative, so record no ratio where the \
                     denominator was zero rather than recording the quotient"
                )));
            }
        }
        if self.recent_return_bps.is_some_and(|bps| !bps.is_finite()) {
            return Err(Error::invalid(format!(
                "episode {episode_id} records a recent return of {:?} basis points, which is not \
                 a number; record none where the series could not be differenced",
                self.recent_return_bps
            )));
        }
        Ok(())
    }
}

/// One causal edge that was active for this episode's instrument when it was
/// formed — blueprint §10.1's `causal_context`, "which edges were active and
/// their strength".
///
/// Strings and a float rather than the world model's own `CausalEdge`,
/// because this crate is a library below the services and may not depend on
/// `qip-world-model`; the same reason [`RegimeLabel`] carries strings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CausalContextEdge {
    /// The cause end of the edge. The effect end is the episode's own
    /// instrument, so it is not repeated here.
    pub cause: String,
    /// The mechanism claimed, as the world model's enum prints it.
    pub mechanism: String,
    /// Strength discounted by confidence in the claim — the graph's own
    /// effective transmission, in `[0, 1]`, which is the number a propagation
    /// actually moves on. The two factors are not stored separately because a
    /// record carrying both invites a reader to multiply them a second time.
    pub transmission: f64,
    /// Whether no plausible unobserved confounder stands against the edge.
    /// §9.4's distinction, kept because an episode recalled on a context of
    /// suggestive edges is weaker evidence than one recalled on established
    /// ones, and a mean transmission alone cannot say which it was.
    pub established: bool,
}

impl CausalContextEdge {
    /// Refuse an edge whose transmission is not a real fraction.
    ///
    /// The same refusal `CausalEdge::new` makes upstream, restated here
    /// because this type is also reachable by decoding a record, and a
    /// transmission of `NaN` reaching the mean at index 38 makes every
    /// episode in the store unorderable against every other — a `partial_cmp`
    /// answering `None` is the failure the world model already paid for once.
    fn validate(&self, episode_id: &str) -> Result<()> {
        if self.cause.trim().is_empty() {
            return Err(Error::invalid(format!(
                "episode {episode_id} records a causal edge from an unnamed cause; name the cause \
                 at the source that claimed the edge"
            )));
        }
        if !(0.0..=1.0).contains(&self.transmission) {
            return Err(Error::invalid(format!(
                "episode {episode_id} records the edge from {} at transmission {}, outside \
                 [0, 1]; transmission is strength discounted by confidence and both are \
                 fractions",
                self.cause, self.transmission
            )));
        }
        Ok(())
    }
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
    /// What the claim said would happen over this horizon, in basis points of
    /// the **same** reference [`Self::realised_move_bps`] is measured
    /// against, signed the way the claim pointed.
    ///
    /// `None` where the claim was never written down in a gradeable form —
    /// a `RegimeShift` names no direction and a series standing at zero has
    /// no magnitude to be a fraction of — which is an absence rather than an
    /// expectation of no move, and is why this is an option and not a zero.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub expected_move_bps: Option<f64>,
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

    /// Blueprint §10.1's `surprise`: how far the outcome was from what was
    /// expected, in basis points, signed, `None` where nothing was expected.
    ///
    /// **Derived rather than stored, and that is the point.** Surprise is the
    /// difference of two numbers this record already holds. Writing it down
    /// beside them would be a third claim about one fact, and the three would
    /// disagree the first time a writer set two of them and forgot the
    /// third — the failure the platform's own principle about two independent
    /// claims names, and the one a reader of an episode could never detect,
    /// because a stored surprise looks exactly as authoritative as a computed
    /// one.
    ///
    /// Signed, not absolute: the sign says whether the move overshot the
    /// claim or fell short of it, and those two are different lessons. A
    /// caller wanting magnitude takes `.abs()` and can still see which it
    /// was.
    pub fn surprise_bps(&self) -> Option<f64> {
        self.expected_move_bps
            .map(|expected| self.realised_move_bps - expected)
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
    /// The compressed market and world state at `at`, or `None` where the
    /// platform had measured none of it — a name it had seen no tape in.
    /// Absent rather than a neutral reading, because a neutral reading is a
    /// claim about the market and an absence is a claim about the platform.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<MarketState>,
    /// The causal edges into this instrument that the graph held at `at`,
    /// strongest first and bounded by whoever wrote the episode. Empty where
    /// the graph knew of none, which is also how an episode formed before a
    /// graph existed reads.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub causal_context: Vec<CausalContextEdge>,
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
        if let Some(state) = &self.state {
            state.validate(&self.episode_id)?;
        }
        for edge in &self.causal_context {
            edge.validate(&self.episode_id)?;
        }
        Ok(())
    }

    /// The fixed encoding, per [`EPISODE_DIMENSIONS`].
    pub fn embedding(&self) -> Embedding {
        encode(
            &self.instrument,
            &self.regime,
            self.state.as_ref(),
            &self.causal_context,
            Some(&self.claim),
            Some(&self.findings),
            &self.stances,
            self.horizon,
        )
    }

    /// Blueprint §10.1's `surprise`, or `None` while the claim is unresolved
    /// or was never gradeable. See [`EpisodeOutcome::surprise_bps`] for why
    /// it is computed here rather than held on the record.
    pub fn surprise_bps(&self) -> Option<f64> {
        self.outcome.as_ref().and_then(EpisodeOutcome::surprise_bps)
    }

    /// The query this episode would have been, before its outcome was known.
    pub fn as_query(&self) -> EpisodeQuery {
        EpisodeQuery {
            instrument: self.instrument.clone(),
            regime: self.regime.clone(),
            state: self.state,
            causal_context: self.causal_context.clone(),
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
    /// The state the platform is in as the question is asked. Present on the
    /// query as well as on the record, because a state block that only ever
    /// appeared on stored episodes would be dimensions no query could match
    /// on: cosine would count every one of them against every candidate and
    /// the richest state would rank worst.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<MarketState>,
    /// The causal edges into the instrument that the graph holds now.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub causal_context: Vec<CausalContextEdge>,
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
            self.state.as_ref(),
            &self.causal_context,
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

#[allow(clippy::too_many_arguments)]
fn encode(
    instrument: &str,
    regime: &RegimeLabel,
    state: Option<&MarketState>,
    causal_context: &[CausalContextEdge],
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

    if let Some(state) = state {
        values[DRAWDOWN_AT] = state.drawdown as f32;
        values[VOLATILITY_RATIO_AT] = state.volatility_ratio.map_or(0.0, saturating);
        values[SPREAD_RATIO_AT] = state.spread_ratio.map_or(0.0, saturating);
        if let Some(bps) = state.recent_return_bps {
            // Not `signum`, which calls zero positive — the same reason the
            // claim direction above spells the three cases out.
            values[RETURN_SIGN_AT] = if bps > 0.0 {
                1.0
            } else if bps < 0.0 {
                -1.0
            } else {
                0.0
            };
            // Basis points to a fraction of the reference, then through the
            // same bounded map the ratios use.
            values[RETURN_MAGNITUDE_AT] = saturating(bps.abs() / 10_000.0);
        }
    }

    if !causal_context.is_empty() {
        let count = causal_context.len() as f64;
        values[EDGE_COUNT_AT] = saturating(count);
        let transmission: f64 = causal_context.iter().map(|edge| edge.transmission).sum();
        values[TRANSMISSION_AT] = (transmission / count) as f32;
        let established = causal_context
            .iter()
            .filter(|edge| edge.established)
            .count() as f64;
        values[ESTABLISHED_AT] = (established / count) as f32;
    }

    Embedding::new(values, EPISODE_ENCODING)
}
