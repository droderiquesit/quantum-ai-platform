//! Blueprint §14.2's sources and §14.3's `Testing` gate: a falsifier evaluated
//! against held-out data.
//!
//! Two gaps close here, and they are the same gap seen from two ends.
//!
//! **§14.2.** A hypothesis had no recorded provenance. The blueprint names six
//! sources; this platform proposes from exactly one, and that one is not on
//! the list. [`SourceCensus`] makes that statable rather than inferable: every
//! source is enumerated, each is either wired — with a count of what it
//! proposed — or unwired with the reason it is unwired, and the census prints
//! a line even when nothing proposed anything. A source that produced nothing
//! and a source that does not exist are different facts, and a register that
//! answered both with silence would let the second masquerade as the first for
//! as long as nobody looked.
//!
//! **§14.3.** A falsifier was recorded at formation and never evaluated. A
//! falsifier nothing evaluates is indistinguishable from one that passed, so
//! every hypothesis this platform has ever formed sat permanently in
//! `Proposed` while reading, to anyone downstream, as though it had cleared
//! `Testing`.
//!
//! # The defect this module exists to refuse
//!
//! Evaluating a falsifier against data the hypothesis could already see proves
//! nothing, and it proves nothing *while producing a number*, which is worse
//! than producing none. [`HeldOut`] decides admissibility on the **knowable**
//! instant — [`FeatureValue::available_at`] — and never on the valid instant.
//! A bar whose bucket opened before a hypothesis was formed and closed after
//! it is held-out data, because its close was not knowable until the bucket
//! shut; a restatement valid last March but published this morning is *not*
//! held-out data for a hypothesis formed last week, because it is knowable now
//! and was not then. Only the second timestamp can tell those two apart, which
//! is why this module reads it and not the first.
//!
//! [`HeldOut::admit`] therefore refuses, rather than filters, four things:
//!
//! * a record knowable before it was true, which is impossible and means the
//!   stamps are wrong somewhere upstream;
//! * a record knowable at or before formation — the leakage case;
//! * a record not knowable by the evaluation instant — the same leakage
//!   running the other way, an evaluation reading its own future;
//! * a non-finite value, which compares false against every threshold and so
//!   would report every falsifier as survived.
//!
//! [`TrialLedger::test`] refuses the whole sample if any member is
//! inadmissible. It does not quietly drop the offender, because a sample that
//! silently shrinks is a leak that has already happened and left no trace. A
//! caller holding a mixed history calls [`HeldOut::partition`] first, which
//! separates the held-out records from the rest **and counts what it
//! separated** in a [`LeakageTally`] the caller is expected to report. Dropping
//! in-sample rows is then a deliberate, counted act rather than a side effect.
//!
//! # Trials are charged
//!
//! §14.3's `Testing` row says a falsifier evaluation "counts against the
//! family's cumulative trial budget". [`TrialLedger`] charges one trial per
//! evaluation and returns [`Verdict::BudgetExhausted`] once a family has spent
//! its budget — a refusal to keep testing, not a test that keeps passing. A
//! family whose trials were not tracked would have an unlimited budget, so the
//! ledger refuses a new family once its table is full rather than forgetting
//! an old one.
//!
//! # Money
//!
//! Nothing here is a [`qip_core::Decimal`] and nothing here is money. The
//! feature store holds `close` as an `f64` — [`crate::world::WorldModel`]'s
//! `absorb_bars` is where a bar's exact `Decimal` close becomes a statistic in
//! this store — so by the time a value reaches this module the crossing has
//! already happened upstream, and a falsifier level is a threshold on a stored
//! series in that series' own units. No arithmetic on a money quantity happens
//! in this file.

use crate::features::FeatureValue;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Where a hypothesis came from.
///
/// The first six are blueprint §14.2's table, in its order. The seventh is
/// what this platform actually does, and it is named rather than folded into
/// one of the six: a DISCOVER-stage detector firing is not a gap in a causal
/// graph, not a high-surprise episode and not a counterfactual anomaly, and
/// filing it under one of those would make the census report a source as
/// wired that has never proposed anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisSource {
    /// Two drivers that co-move with no known mechanism connecting them.
    CausalGraphGap,
    /// Situations where the outcome diverged most from expectation.
    HighSurpriseEpisode,
    /// A rule that vetoes profitable paths implies an unmodelled relationship.
    CounterfactualAnomaly,
    /// Structural exposures in the entity graph that no strategy expresses.
    WorldModelTraversal,
    /// A mechanism read out of filings, research or news.
    LanguageModelProposal,
    /// A mechanism established in one asset class, proposed in another.
    CrossAssetTransfer,
    /// A DISCOVER-stage detector firing. Not on §14.2's list; this platform's
    /// only actual source.
    DetectedAnomaly,
}

/// Every source, in blueprint order, with this platform's own last.
///
/// An array rather than a derived iterator so that adding a source is a change
/// to a list a reviewer reads, and so [`SourceCensus`] cannot enumerate a
/// subset by accident.
pub const SOURCES: [HypothesisSource; 7] = [
    HypothesisSource::CausalGraphGap,
    HypothesisSource::HighSurpriseEpisode,
    HypothesisSource::CounterfactualAnomaly,
    HypothesisSource::WorldModelTraversal,
    HypothesisSource::LanguageModelProposal,
    HypothesisSource::CrossAssetTransfer,
    HypothesisSource::DetectedAnomaly,
];

/// How many of [`SOURCES`] blueprint §14.2 itself names.
pub const BLUEPRINT_SOURCES: usize = 6;

impl HypothesisSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CausalGraphGap => "causal_graph_gap",
            Self::HighSurpriseEpisode => "high_surprise_episode",
            Self::CounterfactualAnomaly => "counterfactual_anomaly",
            Self::WorldModelTraversal => "world_model_traversal",
            Self::LanguageModelProposal => "language_model_proposal",
            Self::CrossAssetTransfer => "cross_asset_transfer",
            Self::DetectedAnomaly => "detected_anomaly",
        }
    }

    /// Whether the source is one blueprint §14.2 names.
    ///
    /// [`Self::DetectedAnomaly`] is not, and the census says so rather than
    /// letting a platform that implements none of the six report six sevenths
    /// of a table as covered.
    pub const fn is_blueprint_source(self) -> bool {
        !matches!(self, Self::DetectedAnomaly)
    }
}

impl fmt::Display for HypothesisSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a source stands this cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceStanding {
    /// A production path proposes from this source. `proposed` may be zero —
    /// a wired source with nothing to say this cycle is a different fact from
    /// a source that does not exist, and flattening the two is how an unbuilt
    /// source reads as a quiet one.
    Wired { proposed: usize },
    /// Nothing proposes from this source, and why not.
    Unwired { reason: String },
}

impl SourceStanding {
    pub const fn is_wired(&self) -> bool {
        matches!(self, Self::Wired { .. })
    }

    pub const fn proposed(&self) -> usize {
        match self {
            Self::Wired { proposed } => *proposed,
            Self::Unwired { .. } => 0,
        }
    }
}

/// What each §14.2 source produced, and what each unbuilt one is waiting on.
///
/// Constructed with every source unwired. A source becomes wired the first
/// time something declares it — [`Self::declare`] for presence with nothing to
/// say, [`Self::record`] for a proposal — so the census cannot claim a source
/// is wired on the strength of a constant somebody wrote down once.
#[derive(Clone, Debug)]
pub struct SourceCensus {
    standings: BTreeMap<HypothesisSource, SourceStanding>,
}

impl Default for SourceCensus {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceCensus {
    /// A census in which nothing is wired.
    ///
    /// The reasons are this repository's state as recorded in the §14.2 row of
    /// `docs/DELIVERY-STATUS.md`, and each is a fact about another section
    /// rather than about this module: they are stated here because an operator
    /// reading "unwired" needs to know whether that is a gap somebody forgot
    /// or a gap waiting on something specific.
    pub fn new() -> Self {
        let mut standings = BTreeMap::new();
        for source in SOURCES {
            standings.insert(
                source,
                SourceStanding::Unwired {
                    reason: unwired_reason(source).to_string(),
                },
            );
        }
        Self { standings }
    }

    /// Declare a source wired without a proposal this cycle.
    ///
    /// The silent-when-idle case, made loud. A source whose production path
    /// exists but which found nothing to propose reads as `wired, 0 proposed`
    /// and not as absence.
    pub fn declare(&mut self, source: HypothesisSource) {
        let standing = self
            .standings
            .entry(source)
            .or_insert(SourceStanding::Wired { proposed: 0 });
        if let SourceStanding::Unwired { .. } = standing {
            *standing = SourceStanding::Wired { proposed: 0 };
        }
    }

    /// Record one proposal from a source, which also declares it wired.
    pub fn record(&mut self, source: HypothesisSource) {
        self.declare(source);
        if let Some(SourceStanding::Wired { proposed }) = self.standings.get_mut(&source) {
            *proposed = proposed.saturating_add(1);
        }
    }

    pub fn standing(&self, source: HypothesisSource) -> Option<&SourceStanding> {
        self.standings.get(&source)
    }

    pub fn wired(&self) -> usize {
        self.standings
            .values()
            .filter(|standing| standing.is_wired())
            .count()
    }

    pub fn proposed(&self) -> usize {
        self.standings.values().map(SourceStanding::proposed).sum()
    }

    /// How many of blueprint §14.2's six sources are wired.
    ///
    /// Separate from [`Self::wired`] on purpose: this platform's one source is
    /// not one of the six, so a count that mixed them would report progress
    /// against the blueprint that has not happened.
    pub fn blueprint_sources_wired(&self) -> usize {
        self.standings
            .iter()
            .filter(|(source, standing)| source.is_blueprint_source() && standing.is_wired())
            .count()
    }

    /// One line, always. Ordered by the source enum, so two cycles that
    /// proposed the same things print the same string.
    pub fn describe(&self) -> String {
        let wired: Vec<String> = self
            .standings
            .iter()
            .filter_map(|(source, standing)| match standing {
                SourceStanding::Wired { proposed } => Some(format!("{source} {proposed}")),
                SourceStanding::Unwired { .. } => None,
            })
            .collect();
        let detail = if wired.is_empty() {
            "none proposing".to_string()
        } else {
            wired.join(", ")
        };
        format!(
            "sources: {}/{} wired ({detail}), {}/{BLUEPRINT_SOURCES} of the §14.2 table",
            self.wired(),
            SOURCES.len(),
            self.blueprint_sources_wired()
        )
    }
}

/// Why a §14.2 source has no production path yet.
const fn unwired_reason(source: HypothesisSource) -> &'static str {
    match source {
        HypothesisSource::CausalGraphGap => {
            "the causal graph has no co-movement walk, so there is nothing to find a gap in"
        }
        HypothesisSource::HighSurpriseEpisode => {
            "an episode records no surprise, so the most-wrong situations cannot be ranked"
        }
        HypothesisSource::CounterfactualAnomaly => {
            "counterfactual scores are read by sizing and by no proposer"
        }
        HypothesisSource::WorldModelTraversal => {
            "no traversal turns a structural exposure into a claim"
        }
        HypothesisSource::LanguageModelProposal => {
            "no analyst asks a language model to propose; deliberate, and a decision to reopen"
        }
        HypothesisSource::CrossAssetTransfer => {
            "no established mechanism is carried across asset classes"
        }
        HypothesisSource::DetectedAnomaly => "no detector has raised an anomaly",
    }
}

/// Which way an observation has to move to contradict a claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Breach {
    /// Falsified when the series reaches or exceeds the level.
    RisesTo,
    /// Falsified when the series reaches or falls below the level.
    FallsTo,
}

impl Breach {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RisesTo => "rises_to",
            Self::FallsTo => "falls_to",
        }
    }

    /// Whether `observed` contradicts a claim guarded at `level`.
    fn breached(self, observed: f64, level: f64) -> bool {
        match self {
            Self::RisesTo => observed >= level,
            Self::FallsTo => observed <= level,
        }
    }
}

/// A falsifier something can actually evaluate.
///
/// A hypothesis carries its falsifiers as prose, which is the right form for a
/// red team and no form at all for a machine. This is the same statement with
/// the three things an evaluation needs attached: which stored series to read,
/// which way a contradiction points, and at what level. The prose is kept
/// beside them so the record still says, in words, what was being tested.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Falsifier {
    statement: String,
    feature: String,
    subject: String,
    breach: Breach,
    level: f64,
    minimum_observations: usize,
}

impl Falsifier {
    /// Build a falsifier, refusing one that could never fire or never pass.
    ///
    /// `minimum_observations` is the number of admissible held-out records
    /// below which the falsifier reports [`Verdict::Undetermined`] rather than
    /// [`Verdict::Survived`]. Zero is refused: a falsifier that survives on no
    /// evidence promotes every hypothesis the instant it is formed, which is
    /// the `Testing` gate doing exactly nothing while reading as though it
    /// ran.
    pub fn new(
        statement: impl Into<String>,
        feature: impl Into<String>,
        subject: impl Into<String>,
        breach: Breach,
        level: f64,
        minimum_observations: usize,
    ) -> Result<Self> {
        let statement = statement.into();
        let feature = feature.into();
        let subject = subject.into();
        if statement.trim().is_empty() {
            return Err(Error::invalid(
                "a falsifier needs the sentence it stands for; a threshold with no statement \
                 cannot be reviewed by anyone who did not write it",
            ));
        }
        if feature.trim().is_empty() || subject.trim().is_empty() {
            return Err(Error::invalid(
                "a falsifier needs the feature and subject it reads; name the stored series, so \
                 a claim about one instrument cannot be settled by an observation of another",
            ));
        }
        if !level.is_finite() {
            return Err(Error::numeric(
                "a falsifier level must be finite; a non-finite threshold compares false against \
                 every observation and so reports the falsifier survived forever",
            ));
        }
        if minimum_observations == 0 {
            return Err(Error::invalid(
                "a falsifier needs at least one held-out observation before it may report \
                 survival; zero would clear the §14.3 Testing gate on no evidence at all",
            ));
        }
        Ok(Self {
            statement,
            feature,
            subject,
            breach,
            level,
            minimum_observations,
        })
    }

    pub fn statement(&self) -> &str {
        &self.statement
    }

    pub fn feature(&self) -> &str {
        &self.feature
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub const fn breach(&self) -> Breach {
        self.breach
    }

    pub const fn level(&self) -> f64 {
        self.level
    }

    pub const fn minimum_observations(&self) -> usize {
        self.minimum_observations
    }
}

/// Why a record is not held-out data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inadmissible {
    /// Knowable before the instant it describes. Impossible; the stamps are
    /// wrong upstream and no verdict computed from them means anything.
    KnowableBeforeTrue,
    /// Knowable at or before the hypothesis was formed, so the hypothesis
    /// could already read it. This is the leakage case.
    InSample,
    /// Not knowable by the evaluation instant — an evaluation reading its own
    /// future, which is the same leakage the other way round.
    NotYetKnowable,
    /// Not a number. Compares false against every threshold, so admitting it
    /// would report survival on evidence that says nothing.
    NotFinite,
}

impl Inadmissible {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::KnowableBeforeTrue => "knowable_before_true",
            Self::InSample => "in_sample",
            Self::NotYetKnowable => "not_yet_knowable",
            Self::NotFinite => "not_finite",
        }
    }
}

/// What a partition withheld, by reason.
///
/// Counted rather than discarded. A held-out sample assembled from a mixed
/// history has always dropped something, and the number that was dropped is
/// the difference between "the hypothesis was tested on six days of new data"
/// and "the hypothesis was tested on six days, four of which it had already
/// seen when it was formed".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeakageTally {
    pub in_sample: usize,
    pub not_yet_knowable: usize,
    pub knowable_before_true: usize,
    pub not_finite: usize,
}

impl LeakageTally {
    pub const fn total(&self) -> usize {
        self.in_sample + self.not_yet_knowable + self.knowable_before_true + self.not_finite
    }

    pub const fn is_empty(&self) -> bool {
        self.total() == 0
    }

    fn count(&mut self, reason: Inadmissible) {
        match reason {
            Inadmissible::InSample => self.in_sample += 1,
            Inadmissible::NotYetKnowable => self.not_yet_knowable += 1,
            Inadmissible::KnowableBeforeTrue => self.knowable_before_true += 1,
            Inadmissible::NotFinite => self.not_finite += 1,
        }
    }

    pub fn absorb(&mut self, other: Self) {
        self.in_sample += other.in_sample;
        self.not_yet_knowable += other.not_yet_knowable;
        self.knowable_before_true += other.knowable_before_true;
        self.not_finite += other.not_finite;
    }

    pub fn describe(&self) -> String {
        format!(
            "{} record(s) withheld ({} in-sample, {} not yet knowable, {} impossibly stamped, {} \
             non-finite)",
            self.total(),
            self.in_sample,
            self.not_yet_knowable,
            self.knowable_before_true,
            self.not_finite
        )
    }
}

/// The two instants that decide what counts as held out.
///
/// `formed_at` is when the hypothesis was written down, and everything
/// knowable by then is in-sample. `evaluated_at` is the instant the evaluation
/// stands at, and nothing knowable after it may be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldOut {
    formed_at: Timestamp,
    evaluated_at: Timestamp,
}

impl HeldOut {
    /// The window between formation and evaluation.
    ///
    /// Refuses an evaluation instant at or before formation rather than
    /// swapping the two: a window that ran backwards would admit exactly the
    /// records the hypothesis was built on, which is the leak this type exists
    /// to stop, and correcting the caller's arithmetic would keep its bug in
    /// production.
    pub fn between(formed_at: Timestamp, evaluated_at: Timestamp) -> Result<Self> {
        if evaluated_at <= formed_at {
            return Err(Error::invalid(format!(
                "a held-out window must open after the hypothesis was formed; formed at {} and \
                 evaluated at {} leaves nothing the hypothesis could not already see",
                formed_at.to_rfc3339(),
                evaluated_at.to_rfc3339()
            )));
        }
        Ok(Self {
            formed_at,
            evaluated_at,
        })
    }

    pub const fn formed_at(&self) -> Timestamp {
        self.formed_at
    }

    pub const fn evaluated_at(&self) -> Timestamp {
        self.evaluated_at
    }

    /// Whether one record is held-out data, and why not when it is not.
    ///
    /// Every branch reads [`FeatureValue::available_at`] — the knowable
    /// instant. `valid_at` appears in exactly one comparison, against
    /// `available_at`, to catch a record claiming to have been knowable before
    /// it was true. Deciding admissibility on the valid instant is the mistake
    /// this whole module is built around refusing: a bar that opened before
    /// formation and closed after it is held out, and a restatement valid a
    /// year ago and published this morning is not.
    pub fn admit<'a>(
        &self,
        value: &'a FeatureValue,
    ) -> std::result::Result<&'a FeatureValue, Inadmissible> {
        if value.available_at < value.valid_at {
            return Err(Inadmissible::KnowableBeforeTrue);
        }
        if value.available_at <= self.formed_at {
            return Err(Inadmissible::InSample);
        }
        if value.available_at > self.evaluated_at {
            return Err(Inadmissible::NotYetKnowable);
        }
        if !value.value.is_finite() {
            return Err(Inadmissible::NotFinite);
        }
        Ok(value)
    }

    /// Split a mixed history into the held-out part and a count of the rest.
    ///
    /// The only supported way to build a sample from a store that holds both.
    /// It exists so that dropping a record is a deliberate act with a number
    /// attached, rather than something [`TrialLedger::test`] does quietly —
    /// which is why `test` refuses an inadmissible member instead of filtering
    /// one.
    pub fn partition<'a>(
        &self,
        values: &[&'a FeatureValue],
    ) -> (Vec<&'a FeatureValue>, LeakageTally) {
        let mut held_out = Vec::new();
        let mut tally = LeakageTally::default();
        for value in values {
            match self.admit(value) {
                Ok(value) => held_out.push(value),
                Err(reason) => tally.count(reason),
            }
        }
        (held_out, tally)
    }
}

/// A rolling statistic over held-out records, stamped with its window's own
/// knowable instant.
///
/// **This is the subtlest form of the leak this module exists to refuse, and
/// the one a per-record filter cannot catch.** A claim about twenty-day
/// realised volatility is not settled by a close; it is settled by a statistic
/// computed from twenty-one of them. Filter the *closes* for held-out data and
/// then compute the statistic over whatever window happens to be to hand, and
/// the number that comes out is mostly made of closes the hypothesis could
/// already see. A window is held out only if every record in it is.
///
/// That is what this enforces structurally: the output is computed only from
/// the slice handed in, and its `available_at` is the **latest** availability
/// in the window, not the last record's. Order matters — a restatement
/// published late sitting in the middle of a window is what makes the whole
/// window knowable later than its last element — so the maximum is taken
/// across the slice rather than read off the end.
///
/// `values` must be in valid-time order, which is the order
/// [`crate::features::FeatureStore::history`] returns and which
/// [`HeldOut::partition`] preserves. An unordered slice is refused rather than
/// sorted: a caller whose series is out of order has a bug somewhere earlier,
/// and a rolling window over a silently reordered series is a different
/// statistic wearing the same name.
///
/// The outputs are **not** independent observations. Five consecutive
/// twenty-one-record windows share sixteen of their records, so five of them
/// is nowhere near five independent tests — which is exactly why
/// [`Verdict::Survived`] is called that here and not `Supported`.
pub fn rolling_statistic(
    values: &[&FeatureValue],
    window: usize,
    statistic: impl Fn(&[f64]) -> f64,
) -> Result<Vec<FeatureValue>> {
    if window == 0 {
        return Err(Error::invalid(
            "a rolling window must cover at least one record; zero would derive a statistic \
             from no data and stamp it as though something had been observed",
        ));
    }
    if values
        .windows(2)
        .any(|pair| pair[1].valid_at < pair[0].valid_at)
    {
        return Err(Error::invalid(
            "a rolling statistic needs its records in valid-time order; sorting them here would \
             hide whichever earlier step lost the order and silently change what the statistic \
             is a statistic of",
        ));
    }
    if values.len() < window {
        return Ok(Vec::new());
    }

    let mut derived = Vec::with_capacity(values.len() - window + 1);
    for slice in values.windows(window) {
        let numbers: Vec<f64> = slice.iter().map(|value| value.value).collect();
        let computed = statistic(&numbers);
        if !computed.is_finite() {
            return Err(Error::numeric(
                "a rolling statistic came out non-finite; a non-finite observation compares \
                 false against every falsifier threshold and would report survival on evidence \
                 that says nothing",
            ));
        }
        // The last record decides what instant the statistic describes; the
        // latest availability in the whole window decides when it could have
        // been computed. Those are different records whenever anything in the
        // window arrived late, and using the last record's availability for
        // both is the leak.
        let valid_at = slice
            .last()
            .map(|value| value.valid_at)
            .unwrap_or(Timestamp::from_nanos(0));
        let available_at = slice
            .iter()
            .map(|value| value.available_at)
            .max()
            .unwrap_or(valid_at);
        let confidence = slice
            .iter()
            .map(|value| value.confidence)
            .fold(f64::INFINITY, f64::min);
        let imputed = slice.iter().any(|value| value.imputed);
        let mut value = FeatureValue::new(computed, valid_at, available_at);
        // The weakest link in the window, because a statistic is no more
        // trustworthy than the least trustworthy number that went into it.
        value.confidence = if confidence.is_finite() {
            confidence
        } else {
            1.0
        };
        value.imputed = imputed;
        derived.push(value);
    }
    Ok(derived)
}

/// What one evaluation of one falsifier concluded.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// A held-out observation contradicted the claim. One is enough: a
    /// falsifier is a statement about what must not happen, and its having
    /// happened once is the whole finding.
    Refuted {
        observed: f64,
        valid_at: Timestamp,
        knowable_at: Timestamp,
    },
    /// Enough held-out observations, none of them contradicting. §14.3's
    /// `Supported` row, and deliberately not called that here: surviving a
    /// falsifier is this module's finding, and what a platform does with a
    /// surviving hypothesis is a decision made elsewhere.
    Survived { observations: usize },
    /// Too little held-out data to say. Reported rather than treated as
    /// survival, because a hypothesis nothing has contradicted yet and a
    /// hypothesis that withstood evidence are not the same claim.
    Undetermined {
        observations: usize,
        required: usize,
    },
    /// The family has spent its cumulative trial budget. The evaluation did
    /// not run and nothing was charged.
    BudgetExhausted { spent: usize, budget: usize },
}

impl Verdict {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Refuted { .. } => "refuted",
            Self::Survived { .. } => "survived",
            Self::Undetermined { .. } => "undetermined",
            Self::BudgetExhausted { .. } => "budget_exhausted",
        }
    }

    pub const fn is_refuted(&self) -> bool {
        matches!(self, Self::Refuted { .. })
    }
}

/// Trials one hypothesis family may spend before the ledger stops testing it.
///
/// Sixty-four is chosen from what the budget is for rather than from a table.
/// A family here is a hypothesis *class* — the detector kind behind the claim
/// — and the loop forms at most about one claim per cycle per class. At
/// sixty-four trials a class that has never survived one has been tested for
/// months of cycles and is not about to start; continuing to test it is the
/// multiple-comparisons problem being run deliberately.
pub const FAMILY_TRIAL_BUDGET: usize = 64;

/// Distinct families one ledger tracks.
///
/// Bounded like everything else that grows with what arrives. The ledger
/// refuses a new family at the bound rather than evicting an old one, because
/// evicting a family resets its spend to zero and hands it a fresh budget — a
/// cap that makes the thing it caps unlimited.
pub const FAMILY_LIMIT: usize = 256;

/// The longest a family key may be.
const FAMILY_KEY_CHARS: usize = 128;

/// Cumulative trials and refutations, per family.
///
/// §14.3's last row — "Refuted or retired: recorded permanently. A refuted
/// hypothesis is valuable — it stops the same idea being re-proposed" — is
/// [`Self::refuted_statements`] and [`Self::already_refuted`]. Permanently
/// within this ledger's bounds: the refutation is also a fact for the event
/// log, and the ledger is a working set, not the archive.
#[derive(Clone, Debug)]
pub struct TrialLedger {
    budget: usize,
    family_limit: usize,
    spent: BTreeMap<String, usize>,
    refuted: BTreeMap<String, BTreeSet<String>>,
}

impl Default for TrialLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl TrialLedger {
    pub fn new() -> Self {
        Self {
            budget: FAMILY_TRIAL_BUDGET,
            family_limit: FAMILY_LIMIT,
            spent: BTreeMap::new(),
            refuted: BTreeMap::new(),
        }
    }

    /// A ledger with a stated budget and family bound.
    ///
    /// Refuses zero for either rather than substituting one. A zero budget
    /// would refuse every evaluation the platform ever attempts while
    /// reporting a working gate, and a zero family bound would refuse every
    /// family; both are caller arithmetic that should stop where it is still
    /// legible.
    pub fn with_bounds(budget: usize, family_limit: usize) -> Result<Self> {
        if budget == 0 {
            return Err(Error::invalid(
                "a family trial budget must be at least 1; zero refuses every evaluation while \
                 still reporting a gate that ran",
            ));
        }
        if family_limit == 0 {
            return Err(Error::invalid(
                "a ledger must be able to track at least one family; zero refuses every \
                 hypothesis class there is",
            ));
        }
        Ok(Self {
            budget,
            family_limit,
            spent: BTreeMap::new(),
            refuted: BTreeMap::new(),
        })
    }

    pub const fn budget(&self) -> usize {
        self.budget
    }

    pub const fn family_limit(&self) -> usize {
        self.family_limit
    }

    /// Trials charged to a family so far.
    pub fn spent(&self, family: &str) -> usize {
        self.spent.get(family).copied().unwrap_or(0)
    }

    pub fn families(&self) -> usize {
        self.spent.len()
    }

    /// Statements refuted under a family, so the same idea proposed again is
    /// recognisable as one that has already failed.
    pub fn refuted_statements(&self, family: &str) -> Option<&BTreeSet<String>> {
        self.refuted.get(family)
    }

    pub fn refutations(&self) -> usize {
        self.refuted.values().map(BTreeSet::len).sum()
    }

    /// Whether this exact statement has already been refuted under this
    /// family. §14.3's reason for recording a refutation at all.
    pub fn already_refuted(&self, family: &str, statement: &str) -> bool {
        self.refuted
            .get(family)
            .is_some_and(|statements| statements.contains(statement))
    }

    /// Evaluate one falsifier against a held-out sample, charging a trial.
    ///
    /// Refuses, rather than filters, a sample containing anything
    /// [`HeldOut::admit`] rejects: a leaked record removed quietly is a leak
    /// that happened and left no trace. Callers assemble the sample with
    /// [`HeldOut::partition`] and report its [`LeakageTally`].
    ///
    /// A trial is charged only when an evaluation actually happens. A refused
    /// sample and an exhausted budget both spend nothing, so a family cannot
    /// be talked out of its budget by a caller passing rubbish.
    pub fn test(
        &mut self,
        family: &str,
        falsifier: &Falsifier,
        boundary: &HeldOut,
        sample: &[&FeatureValue],
    ) -> Result<Verdict> {
        let key = self.admissible_family(family)?;

        let spent = self.spent(&key);
        if spent >= self.budget {
            return Ok(Verdict::BudgetExhausted {
                spent,
                budget: self.budget,
            });
        }

        for value in sample {
            if let Err(reason) = boundary.admit(value) {
                return Err(Error::invalid(format!(
                    "a held-out sample for {} on {} carries a record that is {}; partition the \
                     history with HeldOut::partition and report what it withheld, rather than \
                     handing an evaluation data the hypothesis could already see",
                    falsifier.feature(),
                    falsifier.subject(),
                    reason.as_str()
                )));
            }
        }

        *self.spent.entry(key.clone()).or_insert(0) += 1;

        // Oldest first, so the refutation names the first observation that
        // contradicted the claim rather than whichever the caller listed
        // first. `partition` preserves the store's valid-time order.
        let breached = sample
            .iter()
            .find(|value| falsifier.breach.breached(value.value, falsifier.level));
        if let Some(value) = breached {
            self.record_refutation(&key, falsifier.statement());
            return Ok(Verdict::Refuted {
                observed: value.value,
                valid_at: value.valid_at,
                knowable_at: value.available_at,
            });
        }

        let observations = sample.len();
        if observations < falsifier.minimum_observations() {
            return Ok(Verdict::Undetermined {
                observations,
                required: falsifier.minimum_observations(),
            });
        }
        Ok(Verdict::Survived { observations })
    }

    /// The family key, or why it is not one.
    ///
    /// Fails closed at the bound: a family the ledger cannot track is a family
    /// with no budget at all, and admitting it untracked is how a class gets
    /// unlimited trials.
    fn admissible_family(&self, family: &str) -> Result<String> {
        let trimmed = family.trim();
        if trimmed.is_empty() {
            return Err(Error::invalid(
                "a trial has to be charged to a named hypothesis family; an unnamed family \
                 shares a budget with every other unnamed one",
            ));
        }
        if trimmed.chars().count() > FAMILY_KEY_CHARS {
            return Err(Error::invalid(format!(
                "a hypothesis family name may be at most {FAMILY_KEY_CHARS} characters; a longer \
                 one is data wearing a key's clothes"
            )));
        }
        if !self.spent.contains_key(trimmed) && self.spent.len() >= self.family_limit {
            return Err(Error::denied(format!(
                "this ledger already tracks {} families and will not add another; a family it \
                 cannot track has no budget, and testing it would be the budget doing nothing",
                self.family_limit
            )));
        }
        Ok(trimmed.to_string())
    }

    fn record_refutation(&mut self, family: &str, statement: &str) {
        let statements = self.refuted.entry(family.to_string()).or_default();
        // Bounded by the same argument as the family table: a refutation set
        // that grows without limit is an archive, and the event log is the
        // archive.
        if statements.len() < FAMILY_TRIAL_BUDGET {
            statements.insert(statement.to_string());
        }
    }
}

/// What one cycle's falsification pass did.
///
/// Every field is reported even when it is zero, and [`Self::describe`]
/// returns a line for a pass that tested nothing. A gate that says nothing
/// when it has no subject is indistinguishable from one that is not running,
/// and on a platform whose steady state is "no claim has reached its horizon
/// yet" that is the only state anybody would ever observe.
#[derive(Clone, Debug, Default)]
pub struct FalsificationPass {
    pub open_claims: usize,
    pub tested: usize,
    pub refuted: usize,
    pub survived: usize,
    pub undetermined: usize,
    pub budget_exhausted: usize,
    pub unevaluable: usize,
    pub leakage: LeakageTally,
}

impl FalsificationPass {
    /// Fold one verdict into the pass.
    ///
    /// [`Verdict::BudgetExhausted`] is counted outside `tested`, because the
    /// ledger refused to run an evaluation rather than running one — two
    /// numbers that both claimed the same event would make the pass's own
    /// arithmetic a second source of truth about how much testing happened.
    pub fn observe(&mut self, verdict: &Verdict) {
        match verdict {
            Verdict::Refuted { .. } => {
                self.tested += 1;
                self.refuted += 1;
            }
            Verdict::Survived { .. } => {
                self.tested += 1;
                self.survived += 1;
            }
            Verdict::Undetermined { .. } => {
                self.tested += 1;
                self.undetermined += 1;
            }
            Verdict::BudgetExhausted { .. } => self.budget_exhausted += 1,
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "falsification: {} open claim(s), {} tested against held-out data ({} refuted, {} \
             survived, {} undetermined), {} over budget, {} with no series to read; {}",
            self.open_claims,
            self.tested,
            self.refuted,
            self.survived,
            self.undetermined,
            self.budget_exhausted,
            self.unevaluable,
            self.leakage.describe()
        )
    }
}
