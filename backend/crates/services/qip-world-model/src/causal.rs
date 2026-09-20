//! The causal layer.
//!
//! Separate from the relationship graph on purpose. That Kestrel supplies
//! Northwind is a fact. That a disruption at Kestrel moves Northwind's price by
//! roughly a certain amount, after roughly a certain delay, through a named
//! mechanism, is a *claim* — and it needs a mechanism, a lag, a strength and
//! evidence before anything should act on it.
//!
//! Propagation is bounded and attenuating: each hop multiplies the shock by the
//! edge's strength, and effects below a floor are dropped. Without that, a
//! shock reaches everything and the "third-order effect" the charter asks for
//! becomes a list of every instrument in the universe.
//!
//! # Retirement — ADR 0087
//!
//! Blueprint §9.4's second handling ends "an edge that fails its conditions is
//! retired, not patched", and until ADR 0087 nothing here retired anything:
//! [`CausalGraph::record_condition_failure`] wrote `KnownToFail` and every
//! reader went on propagating along the edge. An edge whose own test has
//! refused it [`RETIREMENT_CONSECUTIVE_FAILURES`] passes running in one regime
//! is now retired *from inference*: it leaves [`CausalGraph::outgoing`] and
//! [`CausalGraph::incoming`], and so [`CausalGraph::propagate`] and
//! [`CausalGraph::explanations`], as of the instant it retired; it is skipped
//! by [`CausalGraph::reestimate`] and never re-estimated back; and it stays in
//! [`CausalGraph::edges`] under [`EdgeStanding::Retired`] with the regime, the
//! instant and the run that retired it, because a record deleted is a record
//! nobody can audit. A re-established link is a new edge with new evidence.
//! What retirement deliberately does *not* do is release anything a
//! whole-graph reader constrains — see the ADR.

use qip_core::{Duration, Error, Result, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// How many consecutive condition failures in one regime retire an edge —
/// ADR 0087's N.
///
/// Three, and the argument is a debounce, not a significance level. The
/// platform's precedence pass re-runs a pair's test every cycle over a
/// window that slides by one bar, so consecutive results share almost all of
/// their data and cannot be counted as independent refutations; the number
/// that would make them independent is the window length, and a run that
/// long would outlive most regimes. What three buys is narrower and honest:
/// one failure is a sighting, and it already writes
/// [`ConditionStanding::KnownToFail`]; the second is the same window one bar
/// on, which can still be a single gap or corporate action in the tape; the
/// third is a run — the test has refused the claim on every pass since the
/// run began, and a hold in between would have reset it
/// ([`CausalGraph::add`]). The reversal condition is in the ADR: when the
/// pass records the window it tested over, this count should become
/// failures over non-overlapping windows.
pub const RETIREMENT_CONSECUTIVE_FAILURES: usize = 3;

/// A link, as re-estimation keys it: cause, effect and the mechanism claimed
/// between them.
///
/// The mechanism is part of the key because evidence about a cost pass-through
/// is not evidence about a sentiment spillover, even between the same pair.
/// Ordered, and carried in a [`BTreeMap`], because a re-estimation report
/// reaches an operator and a replay that reorders is not a replay.
type EdgeKey = (String, String, Mechanism);

/// How one thing affects another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mechanism {
    /// Input cost or availability passes through to the buyer.
    SupplyChain,
    /// Lost sales at one firm are gained by a rival.
    CompetitiveSubstitution,
    /// Demand for a product moves demand for its inputs.
    DemandLinkage,
    /// A change in policy rates repricing discount rates.
    DiscountRate,
    /// A currency move altering translated revenue.
    CurrencyTranslation,
    /// A commodity price entering a cost base.
    InputCost,
    /// A commodity price entering revenue.
    OutputPrice,
    /// Credit conditions altering funding cost or availability.
    CreditConditions,
    /// Sentiment or positioning spilling across similar names.
    Sentiment,
    /// Index membership forcing mechanical flows.
    IndexFlow,
    /// Shared ownership forcing correlated liquidation.
    CommonOwnership,
    /// Regulatory action applying across an industry.
    Regulatory,
    /// Established only by lagged statistical precedence (blueprint §9.2's
    /// Granger-style method, via [`crate::granger::establish_temporal_precedence`]),
    /// same direction. No economic channel is proposed — the effect is that
    /// the cause's past co-moves with the effect's future — which is why
    /// this and [`Self::InverseTemporalPrecedence`] are the two mechanisms
    /// [`CausalEdge::confidence`] is capped well below the others' default
    /// for: precedence is not a mechanism, and §9.4's unaddressed-confounders
    /// limit applies to every edge either variant produces.
    TemporalPrecedence,
    /// The same establishment method as [`Self::TemporalPrecedence`], where
    /// the cause's past instead co-moves with the *opposite* of the effect's
    /// future.
    InverseTemporalPrecedence,
}

impl Mechanism {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SupplyChain => "supply_chain",
            Self::CompetitiveSubstitution => "competitive_substitution",
            Self::DemandLinkage => "demand_linkage",
            Self::DiscountRate => "discount_rate",
            Self::CurrencyTranslation => "currency_translation",
            Self::InputCost => "input_cost",
            Self::OutputPrice => "output_price",
            Self::CreditConditions => "credit_conditions",
            Self::Sentiment => "sentiment",
            Self::IndexFlow => "index_flow",
            Self::CommonOwnership => "common_ownership",
            Self::Regulatory => "regulatory",
            Self::TemporalPrecedence => "temporal_precedence",
            Self::InverseTemporalPrecedence => "inverse_temporal_precedence",
        }
    }

    /// A readable description of the transmission, used in causal chains.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::SupplyChain => "input availability or cost passes through to the buyer",
            Self::CompetitiveSubstitution => "demand lost by one firm is captured by a rival",
            Self::DemandLinkage => "demand for the product moves demand for its inputs",
            Self::DiscountRate => "a change in rates reprices future cash flows",
            Self::CurrencyTranslation => "a currency move alters translated revenue and costs",
            Self::InputCost => "a commodity price moves the cost base",
            Self::OutputPrice => "a commodity price moves realised revenue",
            Self::CreditConditions => "funding cost or availability changes",
            Self::Sentiment => "positioning and sentiment spill across similar names",
            Self::IndexFlow => "index membership forces mechanical buying or selling",
            Self::CommonOwnership => "shared holders liquidate correlated positions",
            Self::Regulatory => "a regulatory action applies across the industry",
            Self::TemporalPrecedence => {
                "the cause's past co-moves with the effect's future, established only by a \
                 lagged statistical test — no mechanism is proposed"
            }
            Self::InverseTemporalPrecedence => {
                "the cause's past co-moves with the opposite of the effect's future, established \
                 only by a lagged statistical test — no mechanism is proposed"
            }
        }
    }

    /// Whether the effect moves in the same direction as its cause.
    ///
    /// Competitive substitution and [`Self::InverseTemporalPrecedence`] are
    /// the inversions: a rival's loss is a gain, and a statistically negative
    /// lag coefficient names an opposite move rather than a shared one.
    /// Treating either as same-signed would produce a thesis pointed exactly
    /// backwards.
    pub fn preserves_sign(&self) -> bool {
        !matches!(
            self,
            Self::CompetitiveSubstitution | Self::InverseTemporalPrecedence
        )
    }
}

/// A claimed causal link.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CausalEdge {
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// Fraction of the cause's move transmitted, in `[0, 1]`.
    pub strength: f64,
    /// Typical delay before the effect is observable.
    pub lag: Duration,
    /// Confidence in the claim itself, in `[0, 1]`.
    pub confidence: f64,
    /// Ids of the evidence supporting the claim.
    pub evidence: Vec<String>,
    /// When the platform recorded the claim.
    pub recorded_at: Timestamp,
    /// When a re-estimation last found no supporting claim inside its horizon,
    /// if one ever has.
    ///
    /// A mark, not an attenuation. [`CausalGraph::reestimate`] reports every
    /// decayed edge to its caller and leaves [`Self::transmission`] alone on
    /// purpose: silently shrinking a stale claim would move every propagation
    /// result in the platform with nothing in the record naming the number
    /// that changed, which is the shape of failure this crate exists to
    /// refuse. Cleared the moment a claim inside the horizon supports the edge
    /// again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decayed_at: Option<Timestamp>,
    /// Ids of the common causes that were genuinely put into both sides of
    /// the comparison this edge was established by — blueprint §9.1's
    /// confounders layer, "explicit, and adjusted for".
    ///
    /// Empty on every edge established by a method that adjusts for nothing,
    /// which includes every hand-asserted mechanism claim: a person claiming
    /// a supply-chain link is not running a regression, and recording an
    /// empty set there is honest rather than a gap.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub adjusted_for: BTreeSet<String>,
    /// Ids of common causes that are plausible and that the platform holds
    /// no series for — §9.4's "confounders are often unobserved".
    ///
    /// Recording one is an admission and not a remedy. Its entire effect is
    /// [`Self::standing`]: a non-empty set makes the edge
    /// [`EdgeStanding::Suggestive`] whatever its p-value, which is §9.4's
    /// instruction in the one place a reader of the edge will see it.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub suspected_confounders: BTreeSet<String>,
    /// Regime keys under which this edge's own test has cleared its bar —
    /// blueprint §9.1's conditions layer, "the regime under which an edge
    /// holds", segmented from history rather than asserted.
    ///
    /// Empty on every edge written by a method that does not know what
    /// regime it was in, which includes every hand-asserted mechanism claim.
    /// Empty is "nobody asked", never "holds nowhere" — see
    /// [`Self::in_regime`], which answers [`ConditionStanding::Untested`] for
    /// it rather than inventing a negative from an unasked question.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub holds_in: BTreeSet<String>,
    /// Regime keys under which the same test has been re-run and did **not**
    /// clear that bar — §9.1's "the conditions under which it is known to
    /// fail".
    ///
    /// Recorded only where a test genuinely ran: a pair with too little
    /// history has not failed in a regime, it has not been asked about one,
    /// and conflating the two would build exactly the control that reads as
    /// protection and cannot fire.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub fails_in: BTreeSet<String>,
    /// The run of consecutive condition failures this edge is currently on,
    /// if any — the counter ADR 0087's retirement fires on.
    ///
    /// One run, in one regime: a failure recorded under a different regime
    /// starts a fresh run at one, so nothing carries across a regime
    /// boundary and a regime the edge has never been observed in starts from
    /// zero. A recorded hold for the same pair in the run's regime clears it
    /// ([`CausalGraph::add`]). `None` on every edge that has never failed,
    /// and on a retired one — the run that retired it is inside
    /// [`Self::retired`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_run: Option<FailureRun>,
    /// Why and when this edge was retired, if it has been — ADR 0087.
    ///
    /// A mark that readers honour, not a deletion: the edge stays in
    /// [`CausalGraph::edges`] so the record of what was claimed, what refuted
    /// it and when survives. Point in time is the load-bearing detail: a
    /// reader asking about an instant *before* [`Retirement::at`] still sees
    /// the edge, because at that instant it was not yet retired, and a
    /// backtest that saw the future retirement would be reasoning from a
    /// graph it did not have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired: Option<Retirement>,
}

/// The consecutive condition failures an edge is currently accumulating in
/// one regime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureRun {
    /// The regime every failure in the run was recorded under.
    pub regime: String,
    /// How many, including the first sighting.
    pub failures: usize,
    /// When the first failure of the run became knowable.
    pub began: Timestamp,
}

/// Why and when an edge was retired — the record §9.4's "retired, not
/// patched" leaves behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retirement {
    /// The regime the retiring run was recorded under.
    pub regime: String,
    /// When the retiring failure became knowable — the instant from which
    /// readers stop seeing the edge.
    pub at: Timestamp,
    /// How many consecutive failures the run reached.
    pub consecutive_failures: usize,
    /// When the run began.
    pub run_began: Timestamp,
}

/// How an edge stands in one named regime — blueprint §9.1's conditions
/// layer as a reader sees it.
///
/// A mark, never an attenuation, for the same reason [`EdgeStanding`] is one:
/// silently shrinking an edge's transmission because its own test failed
/// under today's regime would move every propagation in the platform with
/// nothing in the record naming the number that changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionStanding {
    /// The edge's test has cleared its bar in this regime and has never
    /// failed in it.
    Holds,
    /// The edge's test has never been run in this regime. An unasked question
    /// is not a negative answer.
    Untested,
    /// The edge's test has been run in this regime and did not clear its bar.
    KnownToFail,
}

impl ConditionStanding {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Holds => "holds",
            Self::Untested => "untested",
            Self::KnownToFail => "known_to_fail",
        }
    }
}

/// How far an edge may be relied on — blueprint §9.4's own two words.
///
/// A mark, never an attenuation, for exactly the reason
/// [`CausalEdge::decayed_at`] is one: silently shrinking an edge's
/// transmission because a confounder was admitted would move every
/// propagation result in the platform with nothing in the record naming the
/// number that changed. The establishment method lowers its own confidence
/// *ceiling* at the moment of creation instead, where the choice is visible
/// in the edge it wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeStanding {
    /// No plausible unobserved confounder is recorded against it.
    Established,
    /// At least one is. §9.4: "the edge is treated as suggestive rather than
    /// established".
    Suggestive,
    /// Its own test refused it [`RETIREMENT_CONSECUTIVE_FAILURES`] passes
    /// running in one regime — ADR 0087. Relied on for nothing; kept for the
    /// record. Outranks the other two, because a retired edge with no
    /// confounder is not "established", it is retired.
    Retired,
}

impl EdgeStanding {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Established => "established",
            Self::Suggestive => "suggestive",
            Self::Retired => "retired",
        }
    }
}

impl CausalEdge {
    /// The confidence an edge carries when its writer states none.
    ///
    /// Named rather than written as a literal because
    /// [`crate::granger::TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING`] is argued
    /// against it in prose — a temporal-precedence edge may never reach the
    /// default a mechanism-backed claim starts at — and two numbers compared
    /// in a doc comment and nowhere in the code drift apart in silence.
    pub const DEFAULT_CONFIDENCE: f64 = 0.7;

    /// Claim an edge, refusing a strength that is not a real fraction.
    ///
    /// Refused rather than clamped, and the distinction is not academic here.
    /// This read `strength: strength.clamp(0.0, 1.0)` until 2026-09-15, and it
    /// failed in both directions at once. A caller passing `1.8` — a
    /// percentage where a fraction was wanted — had its bug silently rewritten
    /// into a plausible number the platform then sized against, which is
    /// exactly the caller bug that survives. And `f64::NAN.clamp(0.0, 1.0)` is
    /// `NAN`, so the one value the clamp most needed to stop was the one value
    /// it passed through untouched: a NaN strength reached
    /// [`Self::transmission`], and from there every propagated magnitude and
    /// every [`CausalGraph::explanations`] ordering — `partial_cmp` answers
    /// `None` on a NaN and both sorts here fall back to `Equal`, so the
    /// ranking silently stops ranking.
    ///
    /// That path is reachable with no attacker and no hand-written claim:
    /// [`crate::granger::establish_temporal_precedence_controlling_for`] takes
    /// its strength from a regression's `partial_r_squared`, and the guard
    /// above it (`partial_r_squared < MIN_EFFECT`) is `false` for a NaN, so a
    /// degenerate regression walks straight into this constructor.
    pub fn new(
        cause: impl Into<String>,
        effect: impl Into<String>,
        mechanism: Mechanism,
        strength: f64,
        lag: Duration,
        recorded_at: Timestamp,
    ) -> Result<Self> {
        let cause = cause.into();
        let effect = effect.into();
        Self::check_fraction("strength", strength, &cause, &effect, mechanism)?;
        Ok(Self {
            cause,
            effect,
            mechanism,
            strength,
            lag,
            confidence: Self::DEFAULT_CONFIDENCE,
            evidence: Vec::new(),
            recorded_at,
            decayed_at: None,
            adjusted_for: BTreeSet::new(),
            suspected_confounders: BTreeSet::new(),
            holds_in: BTreeSet::new(),
            fails_in: BTreeSet::new(),
            failure_run: None,
            retired: None,
        })
    }

    /// Refuse a fraction that is not a real number in `[0, 1]`.
    ///
    /// One function for both fields, called from [`Self::new`],
    /// [`Self::with_confidence`] and [`Self::validate`], because two
    /// statements of one rule disagree eventually and the one that disagreed
    /// would be the one guarding the edge nobody built through a constructor.
    ///
    /// The `is_finite` test is first and is not redundant with the range test:
    /// `(0.0..=1.0).contains(&f64::NAN)` is already `false`, so a NaN would be
    /// refused either way — but it would be refused by a message saying the
    /// value lies outside `[0, 1]`, which sends the reader hunting for a
    /// number that is too large. A NaN is not a number that is too large; it
    /// is an arithmetic result nobody computed, and the message has to say so
    /// or the operator repairs the wrong end.
    fn check_fraction(
        field: &str,
        value: f64,
        cause: &str,
        effect: &str,
        mechanism: Mechanism,
    ) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::invalid(format!(
                "the {field} claimed for {cause:?} -> {effect:?} via {} is {value}, which is not a \
                 real number; it is an arithmetic result nobody computed — a division by a zero \
                 variance, an overflow — so repair the estimator that produced it rather than \
                 admitting it, because a non-finite {field} propagates as a magnitude no reader \
                 can interpret and silently disorders every ranking it reaches",
                mechanism.as_str()
            )));
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(Error::invalid(format!(
                "the {field} claimed for {cause:?} -> {effect:?} via {} is {value}, and a {field} \
                 is a fraction in [0, 1]; state it as a fraction at the source that produced it — \
                 a value corrected into range here would be that source's bug surviving into a \
                 number the platform sizes against",
                mechanism.as_str()
            )));
        }
        Ok(())
    }

    /// Refuse an edge that names no cause or no effect, or whose strength or
    /// confidence is not a real fraction.
    ///
    /// Called by [`crate::WorldModel::claim_causal`], which is the only
    /// production path by which an edge reaches a graph — `CausalGraph::add`
    /// is reached from there and from tests, and `CausalGraph::reestimate`
    /// never creates an edge, only re-scores one the graph already holds.
    ///
    /// The failure this closes is three stages from where it would have been
    /// read. `qip_risk::SharedCauseExposure::attribute` refuses a blank driver
    /// — correctly, since a bucket under an empty name is a counter nobody
    /// chose — and `qip_kernel::shared_cause`'s producer answers any such
    /// refusal by refusing **every** shared-cause level, which
    /// `PreTradeChecker::check` turns into a rejection of every order the
    /// platform sends. So one empty symbol reaching `price_history`, one
    /// `discover_temporal_precedence` pass, and one edge claimed with no
    /// cause was a whole-platform stop with no rate limit, clearable only by
    /// repairing the world model, and with a symptom that named none of that.
    /// Fail-closed was the right direction and the wrong distance: the edge is
    /// refused where it is written instead, which is the one place the caller
    /// still knows what it was reading.
    ///
    /// Refused rather than dropped: a claim silently discarded leaves the
    /// caller believing the graph holds a link it does not.
    ///
    /// # Why the two fractions are re-checked here and not only in the
    /// constructors
    ///
    /// Because this type's fields are `pub` and it derives `Deserialize`, so
    /// [`Self::new`] and [`Self::with_confidence`] guard a door with no wall
    /// beside it: a struct literal, a decoded record, or a later assignment to
    /// `edge.strength` reaches the graph having faced neither. A refusal only
    /// a constructor makes is a refusal of that constructor, not of the type.
    ///
    /// The sibling remedy — `qip_capital_fabric::tolerance::ToleranceBasis`,
    /// which shut its decode path with a private `RawBasis` and
    /// `#[serde(into/try_from)]` — was considered and deliberately not copied.
    /// It works there because that type's fields are private, so once the
    /// decode path goes through the constructor there is no other way in at
    /// all. Here the fields are read in dozens of places and
    /// [`CausalGraph::reestimate`] writes `edge.strength` in place by design,
    /// so a serde shim would close one of two doors while making the type look
    /// as though it had closed both — and it would also duplicate ten field
    /// declarations, including their `serde` attributes, as a second statement
    /// of the wire form that can drift from the first. Nothing in the tree
    /// decodes a `CausalEdge` today; everything that admits one to a graph
    /// goes through [`crate::WorldModel::claim_causal`], which calls this. So
    /// the check is put where every path meets rather than on the one path
    /// that is currently hypothetical.
    pub fn validate(&self) -> Result<()> {
        if self.cause.trim().is_empty() || self.effect.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a causal edge must name both the cause it runs from and the effect it runs to, \
                 and this one names {:?} -> {:?} via {}; an unnamed end is a subject nobody can \
                 look up and a bucket nobody can read, so fix the identifier at the source that \
                 produced it rather than claiming the edge",
                self.cause,
                self.effect,
                self.mechanism.as_str()
            )));
        }
        Self::check_fraction(
            "strength",
            self.strength,
            &self.cause,
            &self.effect,
            self.mechanism,
        )?;
        Self::check_fraction(
            "confidence",
            self.confidence,
            &self.cause,
            &self.effect,
            self.mechanism,
        )?;
        // Checked here as well as in `with_conditions`, for the reason the
        // fraction checks are: this is where every path admitting an edge to
        // a graph meets, and an edge built by deserialising a record rather
        // than through the builder would otherwise carry a blank condition
        // into the graph.
        for regime in self.holds_in.iter().chain(self.fails_in.iter()) {
            Self::check_regime(regime, &self.cause, &self.effect, self.mechanism)?;
        }
        // ADR 0087: a re-established link is a new edge with new evidence.
        // An edge arriving already retired would be admitted as a record of
        // a retirement nobody here observed, and one arriving mid-run would
        // retire on a failure that was not the third — a run smuggled in
        // rather than accumulated.
        if let Some(retirement) = &self.retired {
            return Err(Error::invalid(format!(
                "the edge {:?} -> {:?} via {} is being claimed carrying a retirement recorded at \
                 {} under regime {:?}; a retired edge is never re-admitted, so claim a new edge \
                 with the evidence that re-establishes the link instead",
                self.cause,
                self.effect,
                self.mechanism.as_str(),
                retirement.at.to_rfc3339(),
                retirement.regime
            )));
        }
        if let Some(run) = &self.failure_run {
            return Err(Error::invalid(format!(
                "the edge {:?} -> {:?} via {} is being claimed already {} failure(s) into a run \
                 under regime {:?}; a claimed edge starts its own run from nothing, so drop the \
                 run rather than claiming an edge the graph would retire early",
                self.cause,
                self.effect,
                self.mechanism.as_str(),
                run.failures,
                run.regime
            )));
        }
        Ok(())
    }

    /// State how far the claim itself is believed, refusing anything that is
    /// not a real fraction.
    ///
    /// Fallible for the same reason [`Self::new`] is, and for one more: this
    /// clamped too, so a confidence of `NAN` — which
    /// [`crate::granger::establish_temporal_precedence_controlling_for`] can
    /// compute from a NaN p-value — survived it unchanged and multiplied into
    /// [`Self::transmission`], where a single NaN edge makes every chain
    /// through it unorderable rather than merely wrong.
    pub fn with_confidence(mut self, confidence: f64) -> Result<Self> {
        Self::check_fraction(
            "confidence",
            confidence,
            &self.cause,
            &self.effect,
            self.mechanism,
        )?;
        self.confidence = confidence;
        Ok(self)
    }

    /// Record what this edge was and was not adjusted for.
    ///
    /// Both sets at once, deliberately. Two builders would let a caller set
    /// `adjusted_for` and forget `suspected_confounders`, and an edge that
    /// names controls while staying silent about what it could not control
    /// for reads *more* trustworthy than one that names neither — which is
    /// the wrong way round and is exactly the impression §9.4 exists to
    /// forbid.
    pub fn with_confounders(
        mut self,
        adjusted_for: BTreeSet<String>,
        suspected: BTreeSet<String>,
    ) -> Self {
        self.adjusted_for = adjusted_for;
        self.suspected_confounders = suspected;
        self
    }

    /// Record the regimes this edge's own test has cleared its bar in, and
    /// the regimes it has been re-run in and failed — blueprint §9.1's
    /// conditions layer.
    ///
    /// Both sets at once, for the reason [`Self::with_confounders`] takes
    /// both: an edge naming the regimes it holds in while staying silent
    /// about the ones it is known to fail in reads *better* supported than
    /// one naming neither, which is the wrong way round.
    ///
    /// Fallible, and refusing rather than dropping. A blank regime key is a
    /// label nobody can look the edge up under and a bucket nobody can read,
    /// and a set quietly filtered here would leave the caller believing a
    /// condition was recorded when none was — the caller bug that survives.
    pub fn with_conditions(
        mut self,
        holds_in: BTreeSet<String>,
        fails_in: BTreeSet<String>,
    ) -> Result<Self> {
        for regime in holds_in.iter().chain(fails_in.iter()) {
            Self::check_regime(regime, &self.cause, &self.effect, self.mechanism)?;
        }
        self.holds_in = holds_in;
        self.fails_in = fails_in;
        Ok(self)
    }

    /// Refuse a regime key that names nothing.
    fn check_regime(regime: &str, cause: &str, effect: &str, mechanism: Mechanism) -> Result<()> {
        if regime.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a condition recorded against {cause:?} -> {effect:?} via {} names the regime                  {regime:?}, and a regime a reader cannot name is a condition nobody can check;                  label it at the segmenter that produced it rather than recording an anonymous                  one, because an edge carrying blank conditions reads as conditioned and is not",
                mechanism.as_str()
            )));
        }
        Ok(())
    }

    /// How this edge stands under `regime` — §9.1's conditions layer read
    /// back.
    ///
    /// **A recorded failure wins over a recorded hold, and the order is the
    /// safety property.** An edge that cleared its bar in a regime last
    /// quarter and failed in the same regime last week is an edge whose test
    /// has been refuted under those conditions; answering `Holds` because a
    /// stale success is still on the record would be §9.4's "patched" in
    /// place of its "retired". The failure record is historical and does not
    /// expire on its own, so this is the fail-closed direction.
    ///
    /// A regime in neither set answers [`ConditionStanding::Untested`]. An
    /// unasked question is not a negative answer — the same convention
    /// [`Self::is_decayed`] and [`Self::standing`] already use — and a reader
    /// that wants to know whether anything was asked at all must check, which
    /// is why this returns three answers and not a `bool`.
    pub fn in_regime(&self, regime: &str) -> ConditionStanding {
        if self.fails_in.contains(regime) {
            ConditionStanding::KnownToFail
        } else if self.holds_in.contains(regime) {
            ConditionStanding::Holds
        } else {
            ConditionStanding::Untested
        }
    }

    /// Whether a plausible unobserved confounder stands against this edge.
    ///
    /// An edge nobody asked the question of answers
    /// [`EdgeStanding::Established`], which is the same honest convention
    /// [`Self::is_decayed`] uses: an unasked question is not a negative
    /// answer, and the remedy for a method that never considers confounders
    /// is to make it consider them, not to have this function guess on its
    /// behalf.
    pub fn standing(&self) -> EdgeStanding {
        if self.retired.is_some() {
            EdgeStanding::Retired
        } else if self.suspected_confounders.is_empty() {
            EdgeStanding::Established
        } else {
            EdgeStanding::Suggestive
        }
    }

    /// Whether this edge has been retired at all — ADR 0087.
    ///
    /// Not point-in-time; most readers want [`Self::retired_by`]. This one is
    /// for the record: "was this edge ever retired", which a report over the
    /// whole graph asks and a propagation must not.
    pub fn is_retired(&self) -> bool {
        self.retired.is_some()
    }

    /// Whether this edge was retired at or before `known_at`.
    ///
    /// The point-in-time reading, and the one every inference reader uses.
    /// An edge retired *after* the instant asked about was, at that instant,
    /// a live edge; answering "retired" for it would let a backtest reason
    /// from a refutation it had not yet obtained.
    pub fn retired_by(&self, known_at: Timestamp) -> bool {
        self.retired
            .as_ref()
            .is_some_and(|retirement| retirement.at <= known_at)
    }

    pub fn with_evidence(mut self, evidence: Vec<String>) -> Self {
        self.evidence = evidence;
        self
    }

    /// Effective transmission: strength discounted by confidence in the claim.
    ///
    /// A strong mechanism nobody is sure of should not move a portfolio as much
    /// as a weaker one that is well established.
    pub fn transmission(&self) -> f64 {
        self.strength * self.confidence
    }

    /// Whether the claim rests on anything.
    pub fn is_evidenced(&self) -> bool {
        !self.evidence.is_empty()
    }

    /// Whether the last re-estimation found no claim inside its horizon for
    /// this link.
    ///
    /// A graph nobody has re-estimated answers `false` for every edge, which
    /// is honest: an unasked question is not a negative answer.
    pub fn is_decayed(&self) -> bool {
        self.decayed_at.is_some()
    }

    /// This edge's key, as [`CausalGraph::reestimate`] matches claims to it.
    fn key(&self) -> EdgeKey {
        (self.cause.clone(), self.effect.clone(), self.mechanism)
    }
}

/// Evidence bearing on a link the causal graph already holds.
///
/// Deliberately not a [`CausalEdge`]. An edge is the claim a shock is
/// propagated along; a supporting claim is one observation of what that
/// transmission measured, on a day, from named evidence. Absorbing support as
/// a second edge would leave two edges for one link, and
/// [`CausalGraph::propagate`] keeps the *strongest* path to a target rather
/// than the newest — so a link re-measured downwards would go on propagating
/// its old, larger number for ever. Re-estimation exists so that the newest
/// evidence inside the horizon changes the edge instead of accumulating
/// beside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SupportingClaim {
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// The transmission this evidence measured, in `[0, 1]`.
    ///
    /// Not clamped: a reading outside the range is a producer's bug, and
    /// [`CausalGraph::reestimate`] refuses it naming the link. A value
    /// silently corrected is a caller bug that survives into a strength the
    /// platform sizes against. This sentence read "unlike [`CausalEdge::new`]"
    /// until 2026-09-15, when the edge constructor stopped clamping and became
    /// fallible for the same reason — the two now agree, and the asymmetry
    /// that had one path refuse what its neighbour quietly rewrote is gone.
    pub strength: f64,
    /// When the platform recorded the evidence — the instant it became
    /// knowable here, not the instant the world produced it.
    pub recorded_at: Timestamp,
    /// Ids of the evidence, carried onto the edge when the claim is used, so
    /// that a strength which moved names what moved it.
    ///
    /// Empty is legitimate and stays empty. Manufacturing an id would silence
    /// [`CausalGraph::unevidenced`], which exists to surface a claim resting
    /// on nothing.
    pub evidence: Vec<String>,
}

impl SupportingClaim {
    pub fn new(
        cause: impl Into<String>,
        effect: impl Into<String>,
        mechanism: Mechanism,
        strength: f64,
        recorded_at: Timestamp,
    ) -> Self {
        Self {
            cause: cause.into(),
            effect: effect.into(),
            mechanism,
            strength,
            recorded_at,
            evidence: Vec::new(),
        }
    }

    pub fn with_evidence(mut self, evidence: Vec<String>) -> Self {
        self.evidence = evidence;
        self
    }

    fn key(&self) -> EdgeKey {
        (self.cause.clone(), self.effect.clone(), self.mechanism)
    }
}

/// One edge whose strength a re-estimation moved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrengthUpdate {
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// The strength the graph propagated along before.
    pub previous: f64,
    /// The mean of the in-horizon claims that supported the link.
    pub updated: f64,
    /// How many claims that mean is over. A caller reading a large move off
    /// one observation needs to see the one.
    pub claims: usize,
    /// The newest claim used — the instant the updated strength became
    /// knowable, and the edge's `recorded_at` from here on unless the edge was
    /// already recorded later than that.
    pub newest_claim: Timestamp,
}

/// One edge no claim inside the horizon supported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecayedEdge {
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// When the edge itself was recorded — how old the newest evidence for
    /// this link is.
    pub recorded_at: Timestamp,
    /// The edge's effective transmission, so a caller can tell a decayed link
    /// that matters from one that never moved anything.
    pub transmission: f64,
    /// When an earlier re-estimation already marked it, if one did. `None` is
    /// the transition into decay — the one a caller records, so that repeating
    /// a re-estimation does not repeat the entry.
    pub previously_marked: Option<Timestamp>,
}

/// What one re-estimation did, in full.
///
/// Returned rather than logged: the decay of a link is a fact about the
/// platform's evidence, and a caller that must decide what to do about it
/// cannot read a log line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reestimation {
    /// The instant re-estimated at, and — when [`Self::refreshed`] — the
    /// graph's new `last_updated`.
    pub at: Timestamp,
    pub horizon: Duration,
    /// Every claim offered, in horizon or not.
    pub claims_considered: usize,
    /// Claims inside the horizon that matched an edge and moved a strength.
    /// The count [`Self::refreshed`] is decided on.
    pub claims_used: usize,
    /// Edges re-estimated, in key order.
    pub updated: Vec<StrengthUpdate>,
    /// Edges with no claim inside the horizon, in key order. Marked and
    /// reported; never dropped from the graph.
    pub decayed: Vec<DecayedEdge>,
    /// In-horizon claims naming a link the graph does not hold, in key order.
    /// Reported rather than turned into an edge: inventing a link from
    /// evidence about one is how a correlation becomes a thesis.
    pub unmatched: Vec<SupportingClaim>,
    /// Whether the graph's `last_updated` moved to [`Self::at`].
    pub refreshed: bool,
}

/// One edge a condition failure retired — ADR 0087.
///
/// The key and the transmission are carried so the caller can journal what
/// was retired without reading the graph back, and so the entry names the
/// number the platform stops propagating along.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetiredEdge {
    pub cause: String,
    pub effect: String,
    pub mechanism: Mechanism,
    /// The edge's effective transmission at retirement.
    pub transmission: f64,
    pub retirement: Retirement,
}

/// What one recorded condition failure did.
///
/// Returned rather than reduced to a count, because retirement is a fact a
/// caller must journal and a count of marked edges cannot carry it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConditionFailures {
    /// Live edges of the pair, knowable by then, that took the failure.
    pub marked: usize,
    /// The subset this failure retired, in the graph's own order.
    pub retired: Vec<RetiredEdge>,
}

/// One node in a propagated shock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub target: String,
    /// Hops from the origin: 1 is a direct effect, 2 second-order, and so on.
    pub order: usize,
    /// Signed magnitude relative to the original shock.
    pub magnitude: f64,
    /// When the effect should be observable.
    pub expected_at: Timestamp,
    /// The chain of mechanisms that produced it, in order.
    pub chain: Vec<Mechanism>,
    /// Nodes traversed, starting at the origin.
    pub path: Vec<String>,
    /// Product of the confidences along the chain.
    pub confidence: f64,
}

impl Effect {
    /// A sentence describing the transmission, for a thesis.
    pub fn explain(&self) -> String {
        if self.chain.is_empty() {
            return format!("{} is the origin of the shock", self.target);
        }
        let steps: Vec<String> = self
            .chain
            .iter()
            .zip(self.path.windows(2))
            .map(|(mechanism, pair)| {
                format!("{} to {} ({})", pair[0], pair[1], mechanism.describe())
            })
            .collect();
        format!(
            "order {} effect on {} at {:+.1}% of the original move, via {}",
            self.order,
            self.target,
            self.magnitude * 100.0,
            steps.join("; then ")
        )
    }
}

/// The result of propagating a shock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropagationResult {
    pub origin: String,
    pub initial_shock: f64,
    pub effects: Vec<Effect>,
    /// Effects dropped for falling below the magnitude floor.
    pub truncated: usize,
}

impl PropagationResult {
    /// Effects at exactly one order.
    pub fn at_order(&self, order: usize) -> Vec<&Effect> {
        self.effects.iter().filter(|e| e.order == order).collect()
    }

    /// The largest effects, by absolute magnitude.
    pub fn strongest(&self, limit: usize) -> Vec<&Effect> {
        let mut ranked: Vec<&Effect> = self.effects.iter().collect();
        ranked.sort_by(|a, b| {
            b.magnitude
                .abs()
                .partial_cmp(&a.magnitude.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.target.cmp(&b.target))
        });
        ranked.truncate(limit);
        ranked
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

/// A directed graph of causal claims.
#[derive(Debug, Default)]
pub struct CausalGraph {
    edges: Vec<CausalEdge>,
    /// Cause to the indices of its outgoing edges.
    by_cause: BTreeMap<String, Vec<usize>>,
    by_effect: BTreeMap<String, Vec<usize>>,
    /// The newest instant at which the graph absorbed evidence. `None` until
    /// the first claim.
    ///
    /// This is the fact §6.2 row 2 is judged on. It is recorded at the seams
    /// rather than derived from the edges on demand so that the answer the
    /// degradation table reads is the one the graph wrote when the evidence
    /// landed, not a scan somebody could later change the rule of.
    ///
    /// Exactly two writers, and no others: [`Self::add`], from the absorbed
    /// edge's own `recorded_at`, and [`Self::reestimate`], from the instant it
    /// re-estimated at — and that one only when at least one claim inside the
    /// horizon was actually used. A re-estimation that used nothing must leave
    /// the fact alone, or every stale graph would read fresh the moment
    /// somebody asked it a question.
    last_updated: Option<Timestamp>,
}

impl CausalGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Record a claim, and the instant the platform recorded it as the
    /// graph's newest update.
    ///
    /// The instant is the edge's own `recorded_at` — the one fact the edge
    /// carries about when the platform absorbed it — and it only ever moves
    /// forward: a claim backfilled with an older `recorded_at` is still a
    /// claim the graph absorbed, but it does not make the graph *less*
    /// current than the newest thing it holds, and letting it rewind would
    /// turn a replay of history into a stale reading.
    pub fn add(&mut self, edge: CausalEdge) {
        // ADR 0087: a hold recorded for the pair breaks a run of failures.
        // The platform's precedence pass writes a pass as a *new* edge
        // carrying `holds_in = {regime}` rather than marking the edge it
        // already holds, so without this the older edge's run would count
        // only the failures and never the pass between them — "consecutive"
        // would mean "cumulative", and a link that clears its bar every
        // other cycle would retire on the third miss.
        if !edge.holds_in.is_empty()
            && let Some(indices) = self.by_cause.get(&edge.cause)
        {
            let siblings: Vec<usize> = indices.clone();
            for index in siblings {
                let Some(held) = self.edges.get_mut(index) else {
                    continue;
                };
                if held.effect != edge.effect || held.retired.is_some() {
                    continue;
                }
                if held
                    .failure_run
                    .as_ref()
                    .is_some_and(|run| edge.holds_in.contains(&run.regime))
                {
                    held.failure_run = None;
                }
            }
        }
        let index = self.edges.len();
        self.by_cause
            .entry(edge.cause.clone())
            .or_default()
            .push(index);
        self.by_effect
            .entry(edge.effect.clone())
            .or_default()
            .push(index);
        self.last_updated = Some(match self.last_updated {
            Some(held) if held >= edge.recorded_at => held,
            _ => edge.recorded_at,
        });
        self.edges.push(edge);
    }

    /// The newest instant at which a claim was absorbed or used, or `None` if
    /// none ever was.
    ///
    /// What `qip_contracts::degradation::CausalGraphFreshness::assess` reads.
    /// Queries — propagation, explanation, the point-in-time views — never
    /// move it: reading the graph is not evidence about the world. Neither
    /// does a re-estimation that found nothing inside its horizon, for the
    /// same reason.
    pub fn last_updated(&self) -> Option<Timestamp> {
        self.last_updated
    }

    /// Re-estimate every edge from the supporting claims absorbed inside
    /// `horizon`, and report what that did.
    ///
    /// The failure this closes: a graph carried whichever strength arrived
    /// first, for ever. Nothing re-estimated it from what the world model kept
    /// absorbing, so §6.2 row 2 read stale on the demo seed — correctly, and
    /// with no path to reading anything else.
    ///
    /// What it does, and what it deliberately does not:
    ///
    /// * An edge with at least one claim inside the horizon takes the **mean**
    ///   of those claims as its strength, and the evidence ids that produced
    ///   it are merged into its own. The mean rather than a blend with the
    ///   prior: a blend weight would be a policy number nobody here measured,
    ///   and the horizon already decides which evidence counts.
    /// * That edge's `recorded_at` moves forward to the newest claim used.
    ///   Without this the re-estimated strength would be readable by a
    ///   point-in-time query at an instant before the evidence for it existed
    ///   — leakage a backtest cannot see, because the record itself would
    ///   claim to have been available. It moves forward only; a claim older
    ///   than the edge does not make the edge knowable earlier than it was.
    /// * An edge with no claim inside the horizon is **marked** decayed and
    ///   reported, never dropped and never silently attenuated. Dropping it
    ///   would delete a relationship nobody disproved; attenuating it would
    ///   change every propagation with nothing naming the number that moved.
    /// * A claim naming a link the graph does not hold is reported unmatched.
    ///   Adding it would invent a causal edge from evidence about one, which
    ///   is what [`CausalGraph::add`] is for and what a mechanism, a lag and a
    ///   confidence exist to make deliberate.
    /// * `last_updated` moves to `now` only if at least one in-horizon claim
    ///   was used. A re-estimation over nothing is not evidence, and a graph
    ///   that refreshed itself by being asked would report a freshness it had
    ///   not earned.
    ///
    /// Deterministic: claims are grouped and reported through [`BTreeMap`]s
    /// keyed on the link, so the report and the resulting edges depend on the
    /// input and not on its order or on any hash seed.
    ///
    /// Refuses, before touching a single edge — so a refused re-estimation
    /// leaves the graph exactly as it was: a non-positive horizon, a strength
    /// outside `[0, 1]` or not finite, a claim recorded after `now`, and a
    /// `now` before the instant the graph has already absorbed. The last two
    /// are clock bugs, and reading either as new evidence would size against
    /// one.
    pub fn reestimate<I>(
        &mut self,
        claims: I,
        horizon: Duration,
        now: Timestamp,
    ) -> Result<Reestimation>
    where
        I: IntoIterator<Item = SupportingClaim>,
    {
        if horizon <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "a causal re-estimation horizon of {horizon:?} admits no claim at all; pass a \
                 positive horizon — the centre's is qip_contracts::degradation::\
                 CAUSAL_GRAPH_HORIZON"
            )));
        }
        if let Some(held) = self.last_updated
            && held > now
        {
            return Err(Error::invalid(format!(
                "the causal graph last absorbed evidence at {}, after the {} it is being \
                 re-estimated at; a re-estimation cannot rewind the freshness fact — fix the \
                 clock rather than the reading",
                held.to_rfc3339(),
                now.to_rfc3339()
            )));
        }

        let mut claims_considered = 0usize;
        let mut in_horizon: BTreeMap<EdgeKey, Vec<SupportingClaim>> = BTreeMap::new();
        for claim in claims {
            claims_considered += 1;
            if !claim.strength.is_finite() || !(0.0..=1.0).contains(&claim.strength) {
                return Err(Error::invalid(format!(
                    "a claim supporting {} -> {} measured a transmission of {}; a transmission is \
                     a fraction in [0, 1] — fix the reading at its source rather than clamping it \
                     into the graph",
                    claim.cause, claim.effect, claim.strength
                )));
            }
            if claim.recorded_at > now {
                return Err(Error::invalid(format!(
                    "a claim supporting {} -> {} is recorded at {}, after the {} it is being \
                     re-estimated at; a claim from the future is a clock bug, not new evidence",
                    claim.cause,
                    claim.effect,
                    claim.recorded_at.to_rfc3339(),
                    now.to_rfc3339()
                )));
            }
            // Counted and then deliberately unused: an old claim is what the
            // horizon exists to exclude, and its absence is what marks decay.
            if now.since(claim.recorded_at) > horizon {
                continue;
            }
            in_horizon.entry(claim.key()).or_default().push(claim);
        }

        // Keyed by link *and* edge index: two edges may claim the same link,
        // and both are re-estimated, so the key alone would collapse them.
        let mut updated: BTreeMap<(EdgeKey, usize), StrengthUpdate> = BTreeMap::new();
        let mut decayed: BTreeMap<(EdgeKey, usize), DecayedEdge> = BTreeMap::new();
        let mut matched: BTreeSet<EdgeKey> = BTreeSet::new();
        for (index, edge) in self.edges.iter_mut().enumerate() {
            // ADR 0087: never re-estimated back. A retired edge takes no
            // strength from new claims and is not reported decayed either —
            // it is neither live nor stale, it is retired — so the claims
            // that would have matched it fall through to `unmatched` below
            // unless a live edge holds the same link, which is the new edge
            // a re-established link is.
            if edge.retired.is_some() {
                continue;
            }
            let key = edge.key();
            let Some(support) = in_horizon.get(&key) else {
                let previously_marked = edge.decayed_at;
                let entry = DecayedEdge {
                    cause: edge.cause.clone(),
                    effect: edge.effect.clone(),
                    mechanism: edge.mechanism,
                    recorded_at: edge.recorded_at,
                    transmission: edge.transmission(),
                    previously_marked,
                };
                edge.decayed_at = Some(now);
                decayed.insert((key, index), entry);
                continue;
            };
            // At least one claim, because a group exists only where one was
            // pushed: the mean below cannot divide by zero.
            let mut total = 0.0f64;
            let mut newest: Option<Timestamp> = None;
            let mut fresh_evidence: BTreeSet<String> = BTreeSet::new();
            for claim in support {
                total += claim.strength;
                newest = Some(match newest {
                    Some(held) if held >= claim.recorded_at => held,
                    _ => claim.recorded_at,
                });
                for id in &claim.evidence {
                    if !edge.evidence.contains(id) {
                        fresh_evidence.insert(id.clone());
                    }
                }
            }
            // The group is never empty; the fallback is the edge's own instant
            // so that an empty one could only ever leave knowability where it
            // already was.
            let newest_claim = newest.unwrap_or(edge.recorded_at);
            let previous = edge.strength;
            edge.strength = total / support.len() as f64;
            if newest_claim > edge.recorded_at {
                edge.recorded_at = newest_claim;
            }
            edge.decayed_at = None;
            edge.evidence.extend(fresh_evidence);
            updated.insert(
                (key.clone(), index),
                StrengthUpdate {
                    cause: edge.cause.clone(),
                    effect: edge.effect.clone(),
                    mechanism: edge.mechanism,
                    previous,
                    updated: edge.strength,
                    claims: support.len(),
                    newest_claim,
                },
            );
            matched.insert(key);
        }

        let mut claims_used = 0usize;
        let mut unmatched: Vec<SupportingClaim> = Vec::new();
        for (key, support) in in_horizon {
            if matched.contains(&key) {
                claims_used += support.len();
            } else {
                unmatched.extend(support);
            }
        }
        let refreshed = claims_used > 0;
        if refreshed {
            self.last_updated = Some(now);
        }
        Ok(Reestimation {
            at: now,
            horizon,
            claims_considered,
            claims_used,
            updated: updated.into_values().collect(),
            decayed: decayed.into_values().collect(),
            unmatched,
            refreshed,
        })
    }

    /// Record that the test behind `cause -> effect` was re-run under
    /// `regime` and did not clear its bar — blueprint §9.1's conditions
    /// layer, written.
    ///
    /// Reports how many edges were marked and which, if any, this failure
    /// retired. **Zero marked is a real and ordinary answer**: a link nobody
    /// ever claimed has no edge to condition, and a caller that read a zero
    /// as "marked" would be reporting a segmentation that never happened.
    ///
    /// # Retirement — ADR 0087
    ///
    /// Each live edge of the pair keeps one [`FailureRun`]. A failure under
    /// the run's regime extends it; a failure under any other regime starts
    /// a fresh run at one, so a regime boundary resets the count and a regime
    /// the edge has never been observed in cannot retire it on its first
    /// sighting — the same first-sighting rule the kernel's regime-transition
    /// marker uses. When a run reaches [`RETIREMENT_CONSECUTIVE_FAILURES`]
    /// the edge is retired as of `known_at`: [`CausalEdge::retired`] records
    /// the regime, the instant and the run, the run itself is cleared, and
    /// from that instant [`Self::outgoing`] and [`Self::incoming`] — and so
    /// [`Self::propagate`] and [`Self::explanations`] — no longer return it.
    /// An edge already retired is left alone and not counted as marked: it
    /// takes no further failures, and it takes no further holds either.
    ///
    /// # What this deliberately does not do
    ///
    /// It does not drop the edge, attenuate its strength, or move
    /// [`Self::last_updated`].
    ///
    /// Not dropping, even on retirement, for [`CausalEdge::decayed_at`]'s
    /// reason: a retired edge is the record of a claim and of what refuted
    /// it, and deleting it would destroy the evidence a later reviewer would
    /// judge the re-established link against.
    ///
    /// Not `last_updated`, and that one is the load-bearing refusal.
    /// `qip_contracts::degradation::CausalGraphFreshness::assess` reads that
    /// instant and narrows the platform's sizing when the graph goes stale. A
    /// pass whose only news is that the graph's own edges are failing their
    /// conditions must not be the thing that makes the graph read *fresh* —
    /// that would be a degradation control switched off by the very evidence
    /// it exists to react to.
    ///
    /// # Point in time
    ///
    /// Only edges recorded at or before `known_at` are marked. An edge that
    /// was not yet knowable cannot have been tested, and marking it would
    /// write a condition into the past.
    pub fn record_condition_failure(
        &mut self,
        cause: &str,
        effect: &str,
        regime: &str,
        known_at: Timestamp,
    ) -> Result<ConditionFailures> {
        if regime.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a condition failure recorded against {cause:?} -> {effect:?} names the regime                  {regime:?}; label the regime at the segmenter that produced it, because a                  failure filed under no condition is a failure no reader can ever match to one"
            )));
        }
        let mut report = ConditionFailures::default();
        let Some(indices) = self.by_cause.get(cause) else {
            return Ok(report);
        };
        // Collected first so the immutable borrow of `by_cause` ends before
        // the edges are touched.
        let targets: Vec<usize> = indices.clone();
        for index in targets {
            let Some(edge) = self.edges.get_mut(index) else {
                continue;
            };
            if edge.effect != effect || edge.recorded_at > known_at || edge.retired.is_some() {
                continue;
            }
            edge.fails_in.insert(regime.to_string());
            report.marked += 1;
            let run = match edge.failure_run.take() {
                Some(mut run) if run.regime == regime => {
                    run.failures += 1;
                    run
                }
                // A different regime, or no run at all: the first sighting
                // in this regime, counted as one and never as a retirement.
                _ => FailureRun {
                    regime: regime.to_string(),
                    failures: 1,
                    began: known_at,
                },
            };
            if run.failures >= RETIREMENT_CONSECUTIVE_FAILURES {
                let retirement = Retirement {
                    regime: run.regime,
                    at: known_at,
                    consecutive_failures: run.failures,
                    run_began: run.began,
                };
                report.retired.push(RetiredEdge {
                    cause: edge.cause.clone(),
                    effect: edge.effect.clone(),
                    mechanism: edge.mechanism,
                    transmission: edge.transmission(),
                    retirement: retirement.clone(),
                });
                edge.retired = Some(retirement);
            } else {
                edge.failure_run = Some(run);
            }
        }
        Ok(report)
    }

    /// Edges retired at or before `known_at` — ADR 0087's record, read back.
    ///
    /// Point-in-time like every other reader, so a report as of an instant
    /// before a retirement does not list it.
    pub fn retired(&self, known_at: Timestamp) -> Vec<&CausalEdge> {
        self.edges
            .iter()
            .filter(|edge| edge.retired_by(known_at))
            .collect()
    }

    /// Edges known by `known_at` whose own test has failed under the regime
    /// `regime_of` names for that edge's effect.
    ///
    /// The regime is asked for per effect rather than passed as one string:
    /// the platform labels a regime per instrument, and one label applied to
    /// a whole graph would match edges against conditions measured on
    /// somebody else's tape.
    pub fn failing_their_regime<F>(&self, known_at: Timestamp, regime_of: F) -> Vec<&CausalEdge>
    where
        F: Fn(&str) -> String,
    {
        self.edges
            .iter()
            .filter(|edge| edge.recorded_at <= known_at)
            .filter(|edge| {
                edge.in_regime(&regime_of(&edge.effect)) == ConditionStanding::KnownToFail
            })
            .collect()
    }

    pub fn edges(&self) -> &[CausalEdge] {
        &self.edges
    }

    /// Edges leaving `cause`, known by `known_at` and not retired by then.
    ///
    /// The two filters are the two halves of point in time: an edge recorded
    /// after the instant was not yet knowable, and an edge retired at or
    /// before it was no longer relied on (ADR 0087). Both are asked of the
    /// same `known_at`, so a reader cannot see a future claim or a past
    /// refutation it did not have.
    pub fn outgoing(&self, cause: &str, known_at: Timestamp) -> Vec<&CausalEdge> {
        self.by_cause
            .get(cause)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|i| self.edges.get(*i))
                    .filter(|e| e.recorded_at <= known_at && !e.retired_by(known_at))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Edges arriving at `effect` — what could explain a move — known by
    /// `known_at` and not retired by then, on [`Self::outgoing`]'s terms.
    pub fn incoming(&self, effect: &str, known_at: Timestamp) -> Vec<&CausalEdge> {
        self.by_effect
            .get(effect)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|i| self.edges.get(*i))
                    .filter(|e| e.recorded_at <= known_at && !e.retired_by(known_at))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Propagate a shock from `origin`, breadth-first with attenuation.
    ///
    /// Each hop multiplies by the edge's transmission and may flip the sign.
    /// Anything falling below `magnitude_floor` is dropped and counted, so the
    /// caller knows the chain was cut rather than exhausted.
    pub fn propagate(
        &self,
        origin: &str,
        initial_shock: f64,
        max_order: usize,
        magnitude_floor: f64,
        at: Timestamp,
        known_at: Timestamp,
    ) -> PropagationResult {
        let mut effects: Vec<Effect> = Vec::new();
        let mut truncated = 0usize;
        // The strongest path to each target wins; a weaker route to somewhere
        // already reached adds nothing but noise.
        let mut best: BTreeMap<String, f64> = BTreeMap::new();
        let mut visited: BTreeSet<String> = BTreeSet::new();
        visited.insert(origin.to_string());

        let mut queue: VecDeque<Effect> = VecDeque::new();
        queue.push_back(Effect {
            target: origin.to_string(),
            order: 0,
            magnitude: initial_shock,
            expected_at: at,
            chain: Vec::new(),
            path: vec![origin.to_string()],
            confidence: 1.0,
        });

        while let Some(current) = queue.pop_front() {
            if current.order >= max_order {
                continue;
            }
            for edge in self.outgoing(&current.target, known_at) {
                if current.path.contains(&edge.effect) {
                    continue; // a cycle would amplify without limit
                }
                let sign = if edge.mechanism.preserves_sign() {
                    1.0
                } else {
                    -1.0
                };
                let magnitude = current.magnitude * edge.transmission() * sign;
                if magnitude.abs() < magnitude_floor {
                    truncated += 1;
                    continue;
                }

                let mut chain = current.chain.clone();
                chain.push(edge.mechanism);
                let mut path = current.path.clone();
                path.push(edge.effect.clone());

                let effect = Effect {
                    target: edge.effect.clone(),
                    order: current.order + 1,
                    magnitude,
                    expected_at: current.expected_at.saturating_add(edge.lag),
                    chain,
                    path,
                    confidence: current.confidence * edge.confidence,
                };

                let previous = best.get(&edge.effect).copied().unwrap_or(0.0);
                if magnitude.abs() > previous.abs() {
                    best.insert(edge.effect.clone(), magnitude);
                    effects.retain(|e| e.target != edge.effect);
                    effects.push(effect.clone());
                }
                if visited.insert(edge.effect.clone()) || magnitude.abs() > previous.abs() {
                    queue.push_back(effect);
                }
            }
        }

        effects.sort_by(|a, b| {
            a.order
                .cmp(&b.order)
                .then_with(|| {
                    b.magnitude
                        .abs()
                        .partial_cmp(&a.magnitude.abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.target.cmp(&b.target))
        });

        PropagationResult {
            origin: origin.to_string(),
            initial_shock,
            effects,
            truncated,
        }
    }

    /// Plausible causes of a move at `target`, strongest first.
    ///
    /// The inverse question to propagation, and the one the reasoning engine
    /// asks when something moved and nobody knows why.
    pub fn explanations(&self, target: &str, known_at: Timestamp) -> Vec<&CausalEdge> {
        let mut candidates = self.incoming(target, known_at);
        candidates.sort_by(|a, b| {
            b.transmission()
                .partial_cmp(&a.transmission())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cause.cmp(&b.cause))
        });
        candidates
    }

    /// Claims with no supporting evidence.
    ///
    /// Surfaced so an unevidenced claim can be challenged rather than quietly
    /// accumulating influence over decisions.
    pub fn unevidenced(&self) -> Vec<&CausalEdge> {
        self.edges.iter().filter(|e| !e.is_evidenced()).collect()
    }
}
