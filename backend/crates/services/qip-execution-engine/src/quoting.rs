//! Blueprint §29.1, the quote loop, as arithmetic that produces a price pair
//! and nothing else.
//!
//! ```text
//! fair value   = reference mid adjusted by microstructure signal and belief
//! half spread  = base + volatility term + adverse selection term
//! skew         = f(inventory vs target)      shifts both quotes
//! size         = f(budget, volatility, queue position value, confidence)
//! bid = fair - half_spread - skew      ask = fair + half_spread - skew
//! reprice only if the change exceeds the requote threshold
//! ```
//!
//! # The one thing to read before changing anything here
//!
//! **This platform is paper trading only and never submits a live order**, and
//! a quote loop is the most dangerous thing in this workspace to get wrong,
//! because in every real venue *quoting is order submission*. So this module
//! is built to be structurally incapable of it:
//!
//! * It produces a [`QuotePair`] — two [`Decimal`] prices and a size. There is
//!   no function anywhere in this module that turns one into an
//!   [`crate::order::Order`], and [`QuotePair`] carries no venue, no client
//!   id, no time in force and no side — nothing a venue adapter could act on.
//! * Nothing here names a venue, constructs an order, or mentions a
//!   [`crate::broker::Broker`]. `qip-acceptance`'s `quote_loop` suite asserts
//!   that over this file's own source text, so a later lane that adds the
//!   missing half fails a test rather than shipping quietly.
//! * The only consumer built on it, `qip_kernel::quote_loop`, evaluates the
//!   pair against the platform's own book and reports it. That is a quote as
//!   an *intent* — priced, recorded, and never sent.
//!
//! If a future task appears to need a path from here to a venue, that is the
//! request `.claude/rules/01-security-and-safety.md` says has never yet been
//! legitimate. Stop and ask.
//!
//! # Every component of §29.1, and what makes each one fire
//!
//! The blueprint's table names seven components and the failure each prevents.
//! A component that cannot bind is worse than an absent one — it reads as a
//! control and is not — so each is listed here with the input that makes it
//! act, and each of those inputs is supplied by a test in this file.
//!
//! | Component | What makes it act |
//! |---|---|
//! | Inventory skew | `inventory` away from `inventory_target`; at the limit, quoting halts |
//! | Adverse-selection term | `adverse_selection_bps` above zero widens the half spread |
//! | Volatility term | `volatility_bps` above zero widens it and shrinks the size |
//! | Requote threshold | [`QuotePair::supersedes`] is false for a move inside it |
//! | Queue position value | [`QueuePosition::Measured`] with size ahead shrinks the size |
//! | Toxic flow detection | one-sided `directional_persistence` *and* high `adverse_selection_bps` withholds |
//! | Belief weighting | `belief_confidence` scales the signal and the size, and below the bar withholds |
//!
//! # Refusal, withholding, and the difference
//!
//! A malformed input is an [`Error`]: a caller that hands in an infinite
//! volatility or a negative inventory limit has a bug, and clamping it would
//! let that bug survive into a price. A market the loop declines to quote into
//! is a [`Withheld`] — a modelled outcome with a reason, because "we did not
//! quote AAA this pass" is a fact a report must carry, not an error.
//!
//! # Money and statistics — where they cross
//!
//! Prices, sizes, budgets and inventories are money or quantities of an
//! instrument and are [`Decimal`] throughout. Spreads, skews, volatilities and
//! confidences are statistics and are `f64`. **The two cross in exactly two
//! places in this file**, both marked at the line: [`apply_bps_offset`], which
//! turns an `f64` offset in basis points into a [`Decimal`] price, and
//! [`size_from_budget`], which turns a dimensionless `f64` scaling factor into
//! a [`Decimal`] fraction of a budget. Nothing else converts, and nothing
//! converts back.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::Serialize;

use crate::origination::OriginationMandate;

/// One whole in basis points. Every price in this module is expressed as an
/// offset from a reference, applied through [`Decimal::checked_apply_bps`],
/// which multiplies by `bps / 10_000` — so `TEN_THOUSAND_BPS` is the identity
/// and an offset is added to it.
const TEN_THOUSAND_BPS: f64 = 10_000.0;

/// The share of a full size a quote is cut to when the platform cannot see
/// where its own order sits in the queue.
///
/// Half. The pessimistic arm, taken by [`QueuePosition::Unknown`], and it is
/// the arm the kernel's production caller takes for any instrument the
/// platform holds no working order in. Committing full size to a queue whose
/// depth ahead is unknown is committing to being last in it.
const QUEUE_VALUE_FLOOR: f64 = 0.5;

/// How a quote is priced and when it is withheld.
///
/// Every field is a policy number a desk sets, and
/// [`QuotePolicy::validate`] refuses an incoherent set rather than repairing
/// one. The structural check at the end of it is the reason no arm of
/// [`quote`] has to defend against a non-positive bid.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuotePolicy {
    /// The half spread charged before any term is added, in basis points.
    /// Strictly positive: a zero base is a two-sided quote at the fair value,
    /// which loses the spread to every taker and keeps none of it.
    pub base_half_spread_bps: f64,
    /// How much of the instrument's volatility is charged into the half
    /// spread. The blueprint's failure if absent: "quotes are picked off
    /// during bursts".
    pub volatility_coefficient: f64,
    /// How much of the measured adverse selection is charged into the half
    /// spread. The blueprint's failure if absent: "the book fills you exactly
    /// when it should not — the classic way market making loses".
    pub adverse_selection_coefficient: f64,
    /// How far the microstructure signal may move the fair value away from the
    /// reference mid, in basis points, at full imbalance and full belief.
    pub imbalance_coefficient_bps: f64,
    /// The skew applied when inventory has drifted the whole way to the limit,
    /// in basis points. Shifts *both* quotes, which is what makes the position
    /// mean-revert: the blueprint's failure if absent is "inventory drifts
    /// monotonically until the position limit halts quoting".
    pub skew_bps_at_limit: f64,
    /// The widest half spread the platform will show. Beyond it the quote is
    /// withheld rather than widened, because a quote nobody could hit is a
    /// message rate with no trade in it.
    pub max_half_spread_bps: f64,
    /// How far a quote must move before it is worth republishing. The
    /// blueprint's failure if absent: "message rate explodes and the venue
    /// throttles or disconnects", and beside it "constant repricing destroys
    /// queue priority, most of the edge on a lit book".
    pub requote_threshold_bps: f64,
    /// The belief confidence below which the platform does not quote at all.
    /// The blueprint's failure if absent: "quoting confidently into a state
    /// the platform does not understand".
    pub minimum_confidence: f64,
    /// How one-sided the recent direction must be before flow is treated as
    /// toxic, as a fraction in `(0, 1]`.
    pub toxic_persistence: f64,
    /// How far the reference must move against a resting quote, in basis
    /// points, before one-sided flow is treated as toxic. Both conditions are
    /// required: one-sided flow in a quiet market is ordinary, and a volatile
    /// market with balanced flow is what the volatility term is for.
    pub toxic_adverse_selection_bps: f64,
}

impl QuotePolicy {
    /// Refuse an incoherent policy, naming what to set instead.
    pub fn validate(&self) -> Result<()> {
        finite_above("base_half_spread_bps", self.base_half_spread_bps, 0.0)?;
        finite_at_least("volatility_coefficient", self.volatility_coefficient, 0.0)?;
        finite_at_least(
            "adverse_selection_coefficient",
            self.adverse_selection_coefficient,
            0.0,
        )?;
        finite_at_least(
            "imbalance_coefficient_bps",
            self.imbalance_coefficient_bps,
            0.0,
        )?;
        finite_at_least("skew_bps_at_limit", self.skew_bps_at_limit, 0.0)?;
        finite_above("requote_threshold_bps", self.requote_threshold_bps, 0.0)?;
        finite_above(
            "toxic_adverse_selection_bps",
            self.toxic_adverse_selection_bps,
            0.0,
        )?;
        fraction("minimum_confidence", self.minimum_confidence)?;
        fraction("toxic_persistence", self.toxic_persistence)?;
        if !self.max_half_spread_bps.is_finite()
            || self.max_half_spread_bps < self.base_half_spread_bps
        {
            return Err(Error::invalid(format!(
                "max_half_spread_bps is {} and base_half_spread_bps is {}; set a ceiling at least \
                 as wide as the base, because a ceiling below the base withholds every quote and \
                 reads as a loop that is running",
                self.max_half_spread_bps, self.base_half_spread_bps
            )));
        }
        // The structural check, and the reason no later arm has to defend
        // against a non-positive bid or a crossed pair. A bid is the fair
        // value scaled by `1 - (half + skew)/10_000` and a fair value is the
        // mid scaled by `1 + imbalance/10_000`, so bounding the three below
        // one whole makes both prices positive by arithmetic rather than by a
        // check somebody has to remember to keep. An unreachable defensive
        // branch would be indistinguishable from a control, which is the
        // shape this workspace already has one recorded example of.
        let worst =
            self.imbalance_coefficient_bps + self.max_half_spread_bps + self.skew_bps_at_limit;
        if worst >= TEN_THOUSAND_BPS {
            return Err(Error::invalid(format!(
                "imbalance_coefficient_bps + max_half_spread_bps + skew_bps_at_limit is {worst} \
                 basis points; set them to total less than {TEN_THOUSAND_BPS}, because at or \
                 above one whole a quote can be priced at or below zero"
            )));
        }
        Ok(())
    }
}

/// Where the fair value is anchored.
///
/// The two arms are the two halves of blueprint §29 that meet here: an
/// instrument with a continuous market has an observable mid, and one without
/// has only what §29.3's gate admitted. There is no third arm and deliberately
/// no default — an instrument nobody can price is one this loop declines to
/// quote, by having nothing to construct.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum QuoteReference {
    /// A mid observed in a continuous market.
    ObservedMid { mid: Decimal },
    /// No continuous market. The anchor is the valuation an
    /// [`OriginationMandate`] carries, and the mandate's ceiling bounds the
    /// size — so §29.3's fourth gate is not a document filed once but the
    /// bound this loop sizes against on every pass.
    Originated {
        mandate: Box<OriginationMandate>,
        /// Money. What the platform already has at risk in the originated
        /// position, against the mandate's ceiling.
        exposure: Decimal,
    },
}

impl QuoteReference {
    /// The anchor price, before any adjustment.
    fn anchor(&self) -> Decimal {
        match self {
            Self::ObservedMid { mid } => *mid,
            Self::Originated { mandate, .. } => mandate.valuation().value,
        }
    }

    /// A bounded token for a report.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ObservedMid { .. } => "observed_mid",
            Self::Originated { .. } => "originated",
        }
    }
}

/// What the platform can see about its own place in the queue.
///
/// The blueprint's fifth component, and the one where an invented number would
/// have done the most damage: queue position value is not derivable from a
/// depth snapshot alone, only from a snapshot plus a resting order of one's
/// own. So the two states are distinguished rather than averaged, and the
/// unknown state takes the pessimistic arm.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum QueuePosition {
    /// Nothing of the platform's is resting here, or the venue publishes no
    /// depth to locate it in. Sizes at [`QUEUE_VALUE_FLOOR`].
    Unknown,
    /// Size resting ahead of the platform's own at prices that trade first,
    /// and the platform's own size at that price.
    Measured { ahead: Decimal, own: Decimal },
}

impl QueuePosition {
    /// The value of the position, in `[0, 1]`: one when nothing is ahead,
    /// falling as the queue ahead grows.
    ///
    /// A ratio of two quantities of the same instrument, so the `Decimal` →
    /// `f64` crossing here produces a dimensionless statistic and never a
    /// price. [`QueuePosition::validate`] has already refused a non-positive
    /// `own`, so the denominator is positive.
    fn value(&self) -> f64 {
        match self {
            Self::Unknown => 0.0,
            Self::Measured { ahead, own } => {
                let total = *ahead + *own;
                if total.is_positive() {
                    (own.to_f64() / total.to_f64()).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            }
        }
    }

    /// The factor a size is scaled by, in `[QUEUE_VALUE_FLOOR, 1]`.
    fn size_factor(&self) -> f64 {
        QUEUE_VALUE_FLOOR + (1.0 - QUEUE_VALUE_FLOOR) * self.value()
    }

    fn validate(&self) -> Result<()> {
        match self {
            Self::Unknown => Ok(()),
            Self::Measured { ahead, own } => {
                if ahead.is_negative() {
                    return Err(Error::invalid(format!(
                        "a measured queue position reports {ahead} resting ahead; report a \
                         non-negative size, or report QueuePosition::Unknown"
                    )));
                }
                if !own.is_positive() {
                    return Err(Error::invalid(format!(
                        "a measured queue position reports {own} of the platform's own size; a \
                         position with nothing of ours in it is QueuePosition::Unknown, not a \
                         measurement of zero"
                    )));
                }
                Ok(())
            }
        }
    }
}

/// Everything one pass of the loop reads.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuoteInputs {
    pub object_id: String,
    pub reference: QuoteReference,
    /// Signed units held. Positive is long.
    pub inventory: Decimal,
    /// Signed units wanted. A market maker's target is ordinarily flat, and
    /// the field exists so that "flat" is a stated premise rather than an
    /// assumption baked into the arithmetic.
    pub inventory_target: Decimal,
    /// The deviation from target at which quoting halts. Strictly positive.
    pub inventory_limit: Decimal,
    /// Money available to show on one side.
    pub budget: Decimal,
    /// A statistic: realised volatility over the observation window, in basis
    /// points, non-negative.
    pub volatility_bps: f64,
    /// A statistic: how far the reference moves against a resting quote over
    /// the horizon it is exposed for, in basis points, non-negative.
    pub adverse_selection_bps: f64,
    /// A statistic in `[-1, 1]`: depth imbalance at the touch, positive when
    /// the bid side is heavier.
    pub signal_imbalance: f64,
    /// A statistic in `[-1, 1]`: how one-directional recent movement has been,
    /// positive for up.
    pub directional_persistence: f64,
    /// A statistic in `[0, 1]`: the platform's own confidence in its reading
    /// of this instrument.
    pub belief_confidence: f64,
    pub queue: QueuePosition,
}

impl QuoteInputs {
    /// Refuse a malformed reading, naming what to supply instead.
    ///
    /// Nothing here is clamped. A caller that computed an infinite volatility
    /// has a defect, and a volatility silently replaced by a large finite
    /// number produces a price that looks deliberate.
    pub fn validate(&self) -> Result<()> {
        if self.object_id.trim().is_empty() {
            return Err(Error::invalid(
                "a quote pass must name the object it prices; an unattributed quote is one no \
                 report can place",
            ));
        }
        let anchor = self.reference.anchor();
        if !anchor.is_positive() {
            return Err(Error::invalid(format!(
                "{}'s quote reference is {anchor}; supply a positive anchor, because a two-sided \
                 quote around a non-positive price is not a market",
                self.object_id
            )));
        }
        if !self.inventory_limit.is_positive() {
            return Err(Error::invalid(format!(
                "{}'s inventory limit is {}; supply a positive limit — a limit of zero halts \
                 quoting on every pass and reads as a loop that is running",
                self.object_id, self.inventory_limit
            )));
        }
        if self.budget.is_negative() {
            return Err(Error::invalid(format!(
                "{}'s quote budget is {}; supply a non-negative budget",
                self.object_id, self.budget
            )));
        }
        finite_at_least("volatility_bps", self.volatility_bps, 0.0)?;
        finite_at_least("adverse_selection_bps", self.adverse_selection_bps, 0.0)?;
        signed_fraction("signal_imbalance", self.signal_imbalance)?;
        signed_fraction("directional_persistence", self.directional_persistence)?;
        if !self.belief_confidence.is_finite()
            || self.belief_confidence < 0.0
            || self.belief_confidence > 1.0
        {
            return Err(Error::invalid(format!(
                "{}'s belief confidence is {}; supply a number in [0, 1]",
                self.object_id, self.belief_confidence
            )));
        }
        if let QuoteReference::Originated { exposure, .. } = &self.reference
            && exposure.is_negative()
        {
            return Err(Error::invalid(format!(
                "{}'s originated exposure is {exposure}; supply a non-negative amount — a \
                 negative exposure would widen the ceiling the mandate imposed",
                self.object_id
            )));
        }
        self.queue.validate()
    }

    /// Signed drift from target, in units. Positive when long of target.
    fn deviation(&self) -> Decimal {
        self.inventory - self.inventory_target
    }
}

/// What the half spread is made of, so a report can attribute it.
///
/// Carried rather than recomputed: a spread a reader cannot decompose is a
/// number they have to trust, and blueprint §29.1's whole argument is that
/// each term prevents a different failure.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SpreadTerms {
    pub base_bps: f64,
    pub volatility_bps: f64,
    pub adverse_selection_bps: f64,
}

impl SpreadTerms {
    pub fn total_bps(&self) -> f64 {
        self.base_bps + self.volatility_bps + self.adverse_selection_bps
    }
}

/// A two-sided quote, priced and never sent.
///
/// Deliberately carries no venue, no side, no client id and no time in force.
/// There is nothing on this type a venue adapter could act on, and that is the
/// point: see this module's header.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuotePair {
    pub object_id: String,
    /// Money. The reference, adjusted by the microstructure signal weighted by
    /// belief.
    pub fair_value: Decimal,
    pub bid: Decimal,
    pub ask: Decimal,
    /// Quantity of the instrument to show on each side.
    pub size: Decimal,
    pub terms: SpreadTerms,
    /// Positive when long of target, shifting both quotes down.
    pub skew_bps: f64,
}

impl QuotePair {
    /// Whether this pair is far enough from `previous` to be worth
    /// republishing.
    ///
    /// Blueprint §29.1's last line. The threshold is measured against the
    /// *previous* pair's own prices rather than a shared reference, because
    /// what a venue charges for is the difference between what is resting and
    /// what would replace it. Either side moving far enough is sufficient;
    /// requiring both would hold a stale bid against a moved ask.
    ///
    /// False when the previous pair prices a different object: two objects'
    /// prices are not comparable and a threshold applied across them would
    /// republish on every pass.
    pub fn supersedes(&self, previous: &Self, threshold_bps: f64) -> bool {
        if self.object_id != previous.object_id {
            return false;
        }
        move_bps(previous.bid, self.bid).is_some_and(|bps| bps > threshold_bps)
            || move_bps(previous.ask, self.ask).is_some_and(|bps| bps > threshold_bps)
    }

    /// One line for a report.
    pub fn describe(&self) -> String {
        format!(
            "{} {} / {} on {} (fair {}, half {:.1}bp = {:.1} base + {:.1} vol + {:.1} adverse, \
             skew {:.1}bp)",
            self.object_id,
            self.bid,
            self.ask,
            self.size,
            self.fair_value,
            self.terms.total_bps(),
            self.terms.base_bps,
            self.terms.volatility_bps,
            self.terms.adverse_selection_bps,
            self.skew_bps,
        )
    }
}

/// Why a pass produced no quote.
///
/// A modelled outcome rather than an error: every arm is a market state the
/// loop is supposed to meet, and a report has to be able to say which one.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Withheld {
    /// Inventory has drifted to or past the limit. The blueprint's own
    /// sentence for the failure the skew prevents ends "…until the position
    /// limit halts quoting"; this is that halt.
    InventoryAtLimit { deviation: Decimal, limit: Decimal },
    /// The platform does not understand this instrument well enough to quote
    /// into it.
    BeliefBelowBar { confidence: f64, bar: f64 },
    /// One-sided flow that also moves the reference against a resting quote.
    ToxicFlow {
        persistence: f64,
        adverse_selection_bps: f64,
    },
    /// The half spread the terms ask for is wider than the platform will show.
    SpreadBeyondCeiling {
        half_spread_bps: f64,
        ceiling_bps: f64,
    },
    /// The originated position is already at or past the mandate's ceiling.
    OriginationCeilingReached { exposure: Decimal, ceiling: Decimal },
    /// The budget, scaled by confidence, volatility and queue value, buys no
    /// whole unit of the instrument.
    NoSize { budget: Decimal },
}

impl Withheld {
    /// A bounded token, so a caller labelling a series or a report matches a
    /// delimited word rather than a substring of a sentence.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::InventoryAtLimit { .. } => "inventory_at_limit",
            Self::BeliefBelowBar { .. } => "belief_below_bar",
            Self::ToxicFlow { .. } => "toxic_flow",
            Self::SpreadBeyondCeiling { .. } => "spread_beyond_ceiling",
            Self::OriginationCeilingReached { .. } => "origination_ceiling_reached",
            Self::NoSize { .. } => "no_size",
        }
    }

    /// The sentence a report carries, naming what would have to change.
    pub fn describe(&self) -> String {
        match self {
            Self::InventoryAtLimit { deviation, limit } => format!(
                "inventory_at_limit: {deviation} away from target against a limit of {limit}; \
                 quoting halts until the position is worked down"
            ),
            Self::BeliefBelowBar { confidence, bar } => format!(
                "belief_below_bar: confidence {confidence:.3} against a bar of {bar:.3}; the \
                 platform does not understand this instrument well enough to quote into it"
            ),
            Self::ToxicFlow {
                persistence,
                adverse_selection_bps,
            } => format!(
                "toxic_flow: direction {persistence:.3} one-sided and the reference moves \
                 {adverse_selection_bps:.1}bp against a resting quote; one counterparty is taking \
                 the spread"
            ),
            Self::SpreadBeyondCeiling {
                half_spread_bps,
                ceiling_bps,
            } => format!(
                "spread_beyond_ceiling: the terms ask for {half_spread_bps:.1}bp against a \
                 ceiling of {ceiling_bps:.1}bp; a quote that wide is a message with no trade in it"
            ),
            Self::OriginationCeilingReached { exposure, ceiling } => format!(
                "origination_ceiling_reached: {exposure} at risk against a mandate ceiling of \
                 {ceiling}; an originated position may have no exit and this one is full"
            ),
            Self::NoSize { budget } => format!(
                "no_size: a budget of {budget}, after confidence, volatility and queue value, \
                 buys no whole unit"
            ),
        }
    }
}

/// What one pass of the loop decided.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteDecision {
    Quoted(Box<QuotePair>),
    Withheld(Withheld),
}

impl QuoteDecision {
    pub fn pair(&self) -> Option<&QuotePair> {
        match self {
            Self::Quoted(pair) => Some(pair),
            Self::Withheld(_) => None,
        }
    }

    pub fn withheld(&self) -> Option<&Withheld> {
        match self {
            Self::Quoted(_) => None,
            Self::Withheld(reason) => Some(reason),
        }
    }
}

/// One pass of blueprint §29.1.
///
/// `Err` for a malformed policy or reading — a caller bug, refused rather than
/// repaired. `Ok(Withheld)` for a market the loop declines to quote into.
/// `Ok(Quoted)` for a priced pair.
///
/// The gates run cheapest-and-hardest first: the position limit, which is a
/// bound the platform set for itself; then belief, which decides whether to
/// participate at all; then toxicity, which decides whether this particular
/// moment is one to participate in; then the width, which decides whether the
/// resulting quote is one anybody could hit; then the mandate's ceiling; then
/// the size. Reordering them changes which reason a report carries for the
/// same market, so the order is stated rather than left to be read off.
pub fn quote(policy: &QuotePolicy, inputs: &QuoteInputs) -> Result<QuoteDecision> {
    policy.validate()?;
    inputs.validate()?;

    // Gate one: the position limit halts quoting.
    let deviation = inputs.deviation();
    if deviation.abs() >= inputs.inventory_limit {
        return Ok(QuoteDecision::Withheld(Withheld::InventoryAtLimit {
            deviation,
            limit: inputs.inventory_limit,
        }));
    }

    // Gate two: belief.
    if inputs.belief_confidence < policy.minimum_confidence {
        return Ok(QuoteDecision::Withheld(Withheld::BeliefBelowBar {
            confidence: inputs.belief_confidence,
            bar: policy.minimum_confidence,
        }));
    }

    // Gate three: toxic flow. Both conditions, never either: one-sided flow in
    // a quiet market is ordinary trading and a market that moves with balanced
    // flow is what the volatility term is priced for. Requiring only one of
    // them would withhold on every trending day, which is a loop that looks
    // like a control and is an outage.
    if inputs.directional_persistence.abs() >= policy.toxic_persistence
        && inputs.adverse_selection_bps >= policy.toxic_adverse_selection_bps
    {
        return Ok(QuoteDecision::Withheld(Withheld::ToxicFlow {
            persistence: inputs.directional_persistence,
            adverse_selection_bps: inputs.adverse_selection_bps,
        }));
    }

    // The half spread: base + volatility term + adverse selection term.
    let terms = SpreadTerms {
        base_bps: policy.base_half_spread_bps,
        volatility_bps: policy.volatility_coefficient * inputs.volatility_bps,
        adverse_selection_bps: policy.adverse_selection_coefficient * inputs.adverse_selection_bps,
    };
    let half_spread_bps = terms.total_bps();

    // Gate four: width. Withheld, never narrowed to the ceiling — a quote
    // shown at the ceiling when the terms asked for twice it is a quote priced
    // for a market the platform does not believe it is in.
    if half_spread_bps > policy.max_half_spread_bps {
        return Ok(QuoteDecision::Withheld(Withheld::SpreadBeyondCeiling {
            half_spread_bps,
            ceiling_bps: policy.max_half_spread_bps,
        }));
    }

    // Gate five: the mandate's ceiling, for an originated market only. The
    // headroom it returns bounds the budget below, so §29.3's fourth gate is
    // load-bearing on every pass rather than checked once at admission.
    let headroom = match &inputs.reference {
        QuoteReference::ObservedMid { .. } => None,
        QuoteReference::Originated { mandate, exposure } => {
            let ceiling = mandate.exposure_ceiling();
            if *exposure >= ceiling {
                return Ok(QuoteDecision::Withheld(
                    Withheld::OriginationCeilingReached {
                        exposure: *exposure,
                        ceiling,
                    },
                ));
            }
            Some(ceiling - *exposure)
        }
    };

    // Fair value: the reference moved by the microstructure signal, weighted
    // by belief. At zero confidence the signal contributes nothing and the
    // fair value is the reference exactly — which is the blueprint's "belief
    // weighting" acting on the price rather than only on the size.
    let signal_bps =
        policy.imbalance_coefficient_bps * inputs.signal_imbalance * inputs.belief_confidence;
    let anchor = inputs.reference.anchor();
    let fair_value = apply_bps_offset(anchor, signal_bps).ok_or_else(|| {
        Error::numeric(format!(
            "{}'s fair value could not be computed from a reference of {anchor} and a signal of \
             {signal_bps} basis points; the product is not representable",
            inputs.object_id
        ))
    })?;

    // Skew: proportional to the drift from target, reaching `skew_bps_at_limit`
    // at the limit. `deviation.abs() < inventory_limit` is established above,
    // so the ratio is inside (-1, 1) and the skew inside the bound the policy
    // validated against — which is what keeps both prices positive.
    //
    // `Decimal` → `f64` is not crossed here: `deviation_fraction` divides two
    // quantities of the same instrument and returns a dimensionless statistic.
    let skew_bps = policy.skew_bps_at_limit * deviation_fraction(deviation, inputs.inventory_limit);

    let bid = apply_bps_offset(fair_value, -(half_spread_bps + skew_bps));
    let ask = apply_bps_offset(fair_value, half_spread_bps - skew_bps);
    let (Some(bid), Some(ask)) = (bid, ask) else {
        return Err(Error::numeric(format!(
            "{}'s quote pair could not be priced from a fair value of {fair_value}; the product \
             is not representable",
            inputs.object_id
        )));
    };

    // Size: budget, scaled by confidence and by the volatility and queue
    // factors, bounded by the mandate's headroom where there is one, converted
    // to units at the fair value.
    let volatility_factor = 1.0 / (1.0 + inputs.volatility_bps / policy.base_half_spread_bps);
    let factor = inputs.belief_confidence * volatility_factor * inputs.queue.size_factor();
    let mut notional = size_from_budget(inputs.budget, factor).ok_or_else(|| {
        Error::numeric(format!(
            "{}'s quote notional could not be computed from a budget of {} and a factor of \
             {factor}",
            inputs.object_id, inputs.budget
        ))
    })?;
    if let Some(headroom) = headroom {
        notional = notional.min(headroom);
    }
    let size = notional.checked_div(fair_value).unwrap_or(Decimal::ZERO);
    if !size.is_positive() {
        return Ok(QuoteDecision::Withheld(Withheld::NoSize {
            budget: inputs.budget,
        }));
    }

    Ok(QuoteDecision::Quoted(Box::new(QuotePair {
        object_id: inputs.object_id.clone(),
        fair_value,
        bid,
        ask,
        size,
        terms,
        skew_bps,
    })))
}

/// Apply an offset in basis points to a price.
///
/// **The first of this file's two `f64` → `Decimal` crossings.** `offset_bps`
/// is a statistic; the result is money. The addition to
/// [`TEN_THOUSAND_BPS`] happens in `f64` and the multiplication in `Decimal`,
/// which is what [`Decimal::checked_apply_bps`] does — so the rounding is the
/// same rounding every other basis-point charge in this workspace takes.
fn apply_bps_offset(price: Decimal, offset_bps: f64) -> Option<Decimal> {
    price.checked_apply_bps(TEN_THOUSAND_BPS + offset_bps)
}

/// Scale a budget by a dimensionless factor.
///
/// **The second of this file's two `f64` → `Decimal` crossings.** `factor` is
/// a product of three statistics in `[0, 1]`; the result is money. `None`
/// rather than a zero for a factor that will not convert, because a zero here
/// is a size that reads as a deliberate decision not to quote.
fn size_from_budget(budget: Decimal, factor: f64) -> Option<Decimal> {
    budget.checked_mul(Decimal::from_f64(factor)?)
}

/// The signed drift from target as a fraction of the limit, in `(-1, 1)`.
///
/// Both arguments are quantities of the same instrument, so the quotient is a
/// statistic and not money — this is a ratio crossing into the `f64` lane, not
/// a price leaving the `Decimal` one. The caller has established that the
/// limit is positive and that the deviation is strictly inside it.
fn deviation_fraction(deviation: Decimal, limit: Decimal) -> f64 {
    let limit = limit.to_f64();
    if limit > 0.0 {
        deviation.to_f64() / limit
    } else {
        0.0
    }
}

/// The size of a price move, in basis points of the earlier price.
///
/// `None` for a non-positive reference, which would make the ratio meaningless
/// — and `None` reads as "not superseded" at the one call site, which is the
/// conservative direction: a pair the platform cannot compare to the resting
/// one is not a reason to send a message.
fn move_bps(from: Decimal, to: Decimal) -> Option<f64> {
    if !from.is_positive() {
        return None;
    }
    Some((to - from).abs().to_f64() / from.to_f64() * TEN_THOUSAND_BPS)
}

fn finite_above(name: &str, value: f64, bound: f64) -> Result<()> {
    if !value.is_finite() || value <= bound {
        return Err(Error::invalid(format!(
            "{name} is {value}; supply a finite number strictly above {bound}"
        )));
    }
    Ok(())
}

fn finite_at_least(name: &str, value: f64, bound: f64) -> Result<()> {
    if !value.is_finite() || value < bound {
        return Err(Error::invalid(format!(
            "{name} is {value}; supply a finite number of at least {bound}"
        )));
    }
    Ok(())
}

fn fraction(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() || value <= 0.0 || value > 1.0 {
        return Err(Error::invalid(format!(
            "{name} is {value}; supply a number in (0, 1]"
        )));
    }
    Ok(())
}

fn signed_fraction(name: &str, value: f64) -> Result<()> {
    if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
        return Err(Error::invalid(format!(
            "{name} is {value}; supply a number in [-1, 1]"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // Every `f64` compared exactly below is a product of exactly-representable
    // fixtures — 0.5 × 40.0, 1.0 × 15.0, a sum of tenths that are powers of
    // two apart — chosen so the assertion is on the arithmetic the module
    // performs and not on a tolerance the test author picked. A tolerance
    // here would let a term be dropped and replaced by something close.
    #![allow(clippy::float_cmp)]

    use super::*;
    use crate::origination::{
        AbsenceCause, AbsenceExplanation, AdverseSelectionModel, ClassApproval, OriginationRequest,
        Valuation,
    };
    use qip_core::dec;
    use qip_core::time::Timestamp;

    /// A policy wide enough that no gate fires on the clean inputs below, so
    /// every test in this file spoils exactly one thing and the refusal is
    /// attributable to it.
    fn policy() -> QuotePolicy {
        QuotePolicy {
            base_half_spread_bps: 10.0,
            volatility_coefficient: 0.5,
            adverse_selection_coefficient: 1.0,
            imbalance_coefficient_bps: 5.0,
            skew_bps_at_limit: 20.0,
            max_half_spread_bps: 100.0,
            requote_threshold_bps: 5.0,
            minimum_confidence: 0.30,
            toxic_persistence: 0.80,
            toxic_adverse_selection_bps: 25.0,
        }
    }

    fn inputs() -> QuoteInputs {
        QuoteInputs {
            object_id: "obj-AAA".to_string(),
            reference: QuoteReference::ObservedMid { mid: dec!("100") },
            inventory: Decimal::ZERO,
            inventory_target: Decimal::ZERO,
            inventory_limit: dec!("1000"),
            budget: dec!("10000"),
            volatility_bps: 0.0,
            adverse_selection_bps: 0.0,
            signal_imbalance: 0.0,
            directional_persistence: 0.0,
            belief_confidence: 1.0,
            queue: QueuePosition::Unknown,
        }
    }

    fn priced(inputs: &QuoteInputs) -> QuotePair {
        match quote(&policy(), inputs).expect("well-formed inputs are priced") {
            QuoteDecision::Quoted(pair) => *pair,
            QuoteDecision::Withheld(reason) => {
                panic!("expected a quote, got {}", reason.describe())
            }
        }
    }

    fn withheld(inputs: &QuoteInputs) -> Withheld {
        match quote(&policy(), inputs).expect("well-formed inputs are decided") {
            QuoteDecision::Quoted(pair) => {
                panic!("expected a withholding, got {}", pair.describe())
            }
            QuoteDecision::Withheld(reason) => reason,
        }
    }

    fn mandate(ceiling: &str) -> OriginationMandate {
        OriginationMandate::admit(OriginationRequest {
            object_id: "obj-ORIG".to_string(),
            instrument_class: "structured-payoff".to_string(),
            valuation: Valuation {
                method: "component-replication".to_string(),
                value: dec!("100"),
                confidence: 0.90,
            },
            absence: AbsenceExplanation {
                cause: AbsenceCause::NotYetCovered,
                evidence: "no dealer publishes a two-sided price".to_string(),
            },
            adverse_selection: AdverseSelectionModel {
                instrument_class: "structured-payoff".to_string(),
                price_impact_bps: 12.0,
                sample: 40,
            },
            exposure_ceiling: Decimal::parse(ceiling).expect("a ceiling"),
            approval: ClassApproval {
                instrument_class: "structured-payoff".to_string(),
                operator: "risk-desk-operator".to_string(),
                approved_at: Timestamp::from_secs(1_760_000_000),
                digest: "sha256:0123456789abcdef".to_string(),
            },
        })
        .expect("a complete origination request")
    }

    #[test]
    fn a_flat_book_in_a_quiet_market_is_quoted_symmetrically_around_the_reference() {
        // The admitting case, and the premise for every test below it: a loop
        // that withheld on every input would satisfy each withholding test in
        // this file and quote nothing, ever. Flat inventory, no signal, no
        // volatility — so the fair value is the mid and the pair is the base
        // half spread either side of it.
        let pair = priced(&inputs());
        assert_eq!(pair.fair_value, dec!("100"));
        assert_eq!(pair.skew_bps, 0.0, "a flat book was skewed");
        assert_eq!(pair.terms.base_bps, 10.0);
        assert_eq!(pair.terms.volatility_bps, 0.0);
        assert_eq!(pair.terms.adverse_selection_bps, 0.0);
        assert_eq!(pair.bid, dec!("99.9"));
        assert_eq!(pair.ask, dec!("100.1"));
        assert!(
            pair.bid < pair.fair_value && pair.fair_value < pair.ask,
            "the pair does not straddle the fair value: {}",
            pair.describe()
        );
        // Half the budget, because the queue position is unknown and that is
        // the pessimistic arm, at a fair value of 100: 50 units.
        assert_eq!(pair.size, dec!("50"));
    }

    #[test]
    fn a_long_book_is_quoted_lower_on_both_sides_so_the_position_mean_reverts() {
        // Blueprint §29.1's skew, and the failure it prevents: "inventory
        // drifts monotonically until the position limit halts quoting". The
        // premise is the flat pair above — asserted here rather than assumed,
        // because a skew that did nothing would leave both pairs identical and
        // a test comparing a skewed pair only to itself would pass.
        let flat = priced(&inputs());

        let mut long = inputs();
        long.inventory = dec!("500");
        let skewed = priced(&long);
        assert!(
            skewed.skew_bps > 0.0,
            "a book half way to the limit carried no skew"
        );
        assert!(
            skewed.bid < flat.bid && skewed.ask < flat.ask,
            "a long book did not move both quotes down: {} against {}",
            skewed.describe(),
            flat.describe()
        );

        // And the mirror, so the skew is a function of the signed deviation
        // rather than of its magnitude — a skew that widened on both sides
        // would read the same in the test above and never revert a short.
        let mut short = inputs();
        short.inventory = dec!("-500");
        let other = priced(&short);
        assert!(
            other.bid > flat.bid && other.ask > flat.ask,
            "a short book did not move both quotes up: {}",
            other.describe()
        );

        // The target is a premise too: the same absolute inventory against a
        // target that wants it produces no skew at all.
        let mut on_target = inputs();
        on_target.inventory = dec!("500");
        on_target.inventory_target = dec!("500");
        assert_eq!(
            priced(&on_target).skew_bps,
            0.0,
            "inventory that matched its target was still skewed against"
        );
    }

    #[test]
    fn inventory_at_the_limit_halts_quoting_rather_than_skewing_harder() {
        // The halt the blueprint's sentence ends on. Both sides of the bar are
        // asserted: one unit inside the limit is quoted, the limit itself is
        // not, so this is a boundary and not a gate that refuses everything.
        let mut inside = inputs();
        inside.inventory = inside.inventory_limit - Decimal::from_raw(1);
        assert!(
            priced(&inside).skew_bps > 0.0,
            "the premise: one unit inside the limit is still quoted, and skewed"
        );

        let mut at = inputs();
        at.inventory = at.inventory_limit;
        let reason = withheld(&at);
        assert_eq!(reason.as_str(), "inventory_at_limit");

        // And a short at the limit, because an `abs()` dropped from the
        // comparison would leave the long side passing and the short side
        // quoting into an unbounded position.
        let mut short = inputs();
        short.inventory = -short.inventory_limit;
        assert_eq!(withheld(&short).as_str(), "inventory_at_limit");
    }

    #[test]
    fn volatility_and_adverse_selection_each_widen_the_half_spread_on_their_own() {
        // Two of the blueprint's components, each isolated: "quotes are picked
        // off during bursts" and "the book fills you exactly when it should
        // not". Isolated because a single test raising both at once would pass
        // with either coefficient deleted.
        let quiet = priced(&inputs());

        let mut volatile = inputs();
        volatile.volatility_bps = 40.0;
        let widened = priced(&volatile);
        assert_eq!(
            widened.terms.volatility_bps, 20.0,
            "the volatility term is not the coefficient times the reading"
        );
        assert_eq!(
            widened.terms.adverse_selection_bps, 0.0,
            "volatility leaked into the adverse-selection term"
        );
        assert!(widened.terms.total_bps() > quiet.terms.total_bps());
        assert!(widened.bid < quiet.bid && widened.ask > quiet.ask);
        // And it shrinks the size, which is the other half of what volatility
        // is supposed to do: 40bp against a 10bp base is a factor of 1/5.
        assert_eq!(widened.size, dec!("10"));

        let mut toxic = inputs();
        toxic.adverse_selection_bps = 15.0;
        let charged = priced(&toxic);
        assert_eq!(charged.terms.adverse_selection_bps, 15.0);
        assert_eq!(
            charged.terms.volatility_bps, 0.0,
            "adverse selection leaked into the volatility term"
        );
        assert!(charged.bid < quiet.bid && charged.ask > quiet.ask);
    }

    #[test]
    fn one_sided_flow_that_also_moves_the_reference_withholds_the_quote() {
        // Blueprint §29.1's "a single informed counterparty extracts the day's
        // spread capture". Both halves of the condition are asserted alone
        // first: either on its own must still quote, or the detector is a
        // trading halt on every trending day.
        let mut one_sided = inputs();
        one_sided.directional_persistence = 0.95;
        assert!(
            quote(&policy(), &one_sided)
                .expect("decided")
                .pair()
                .is_some(),
            "one-sided flow in a quiet market withheld a quote"
        );

        let mut moving = inputs();
        moving.adverse_selection_bps = 30.0;
        assert!(
            quote(&policy(), &moving).expect("decided").pair().is_some(),
            "a moving market with balanced flow withheld a quote"
        );

        let mut both = inputs();
        both.directional_persistence = 0.95;
        both.adverse_selection_bps = 30.0;
        assert_eq!(withheld(&both).as_str(), "toxic_flow");

        // And the sign does not matter: flow that is one-sided downward is as
        // informed as flow that is one-sided upward.
        let mut down = both.clone();
        down.directional_persistence = -0.95;
        assert_eq!(withheld(&down).as_str(), "toxic_flow");
    }

    #[test]
    fn belief_weights_the_signal_scales_the_size_and_below_the_bar_withholds_entirely() {
        // Blueprint §29.1's "quoting confidently into a state the platform does
        // not understand". Three distinct effects, each asserted separately,
        // because a belief term wired into only one of them would pass a test
        // that checked any other.
        let mut confident = inputs();
        confident.signal_imbalance = 1.0;
        confident.belief_confidence = 1.0;
        let believed = priced(&confident);
        assert!(
            believed.fair_value > dec!("100"),
            "a full bid-side imbalance did not move the fair value up"
        );

        // The same signal, half believed, moves the fair value half as far.
        let mut halved = confident.clone();
        halved.belief_confidence = 0.5;
        let tempered = priced(&halved);
        assert!(
            tempered.fair_value > dec!("100") && tempered.fair_value < believed.fair_value,
            "belief did not temper the signal: {} against {}",
            tempered.describe(),
            believed.describe()
        );
        assert!(
            tempered.size < believed.size,
            "belief did not scale the size"
        );

        // And no belief at all leaves the fair value at the reference: a
        // signal the platform does not believe moves no price.
        let mut unbelieved = confident.clone();
        unbelieved.belief_confidence = policy().minimum_confidence;
        assert!(
            priced(&unbelieved).fair_value < tempered.fair_value,
            "less belief did not mean less signal"
        );

        // Below the bar, nothing is quoted. The bar itself is quoted, so this
        // is a boundary rather than a gate that refuses everything.
        let mut below = inputs();
        below.belief_confidence = policy().minimum_confidence - 0.001;
        assert_eq!(withheld(&below).as_str(), "belief_below_bar");
    }

    #[test]
    fn a_quote_is_republished_only_when_it_moves_further_than_the_requote_threshold() {
        // Blueprint §29.1's last line, and two failures at once: "message rate
        // explodes and the venue throttles or disconnects", and "constant
        // repricing destroys queue priority, most of the edge on a lit book".
        let resting = priced(&inputs());
        let threshold = policy().requote_threshold_bps;

        // A one-basis-point drift in the reference moves each side by about a
        // basis point, well inside a five-basis-point threshold.
        let mut nudged = inputs();
        nudged.reference = QuoteReference::ObservedMid {
            mid: dec!("100.01"),
        };
        let next = priced(&nudged);
        assert_ne!(
            next.bid, resting.bid,
            "the premise: the nudged pair really is a different pair"
        );
        assert!(
            !next.supersedes(&resting, threshold),
            "a one-basis-point move republished the quote"
        );

        // A one-percent move is two hundred times the threshold.
        let mut moved = inputs();
        moved.reference = QuoteReference::ObservedMid { mid: dec!("101") };
        assert!(
            priced(&moved).supersedes(&resting, threshold),
            "a one per cent move did not republish the quote"
        );

        // And a pair for another object never supersedes this one, whatever
        // the prices say: two objects' prices are not comparable, and a
        // threshold applied across them republishes on every pass.
        let mut other = inputs();
        other.object_id = "obj-BBB".to_string();
        other.reference = QuoteReference::ObservedMid { mid: dec!("500") };
        assert!(!priced(&other).supersedes(&resting, threshold));
    }

    #[test]
    fn a_known_queue_position_sizes_larger_than_an_unknown_one_and_a_long_queue_ahead_sizes_least()
    {
        // Blueprint §29.1's queue position value. The unknown arm is the one
        // the kernel's production caller takes today, and it is the
        // pessimistic one — asserted here rather than described, because a
        // fail-closed default that is not actually the smaller number is a
        // comment rather than a control.
        let unknown = priced(&inputs());

        let mut front = inputs();
        front.queue = QueuePosition::Measured {
            ahead: Decimal::ZERO,
            own: dec!("10"),
        };
        let at_front = priced(&front);
        assert!(
            at_front.size > unknown.size,
            "nothing ahead in the queue sized no larger than an unknown position: {} against {}",
            at_front.size,
            unknown.size
        );

        let mut back = inputs();
        back.queue = QueuePosition::Measured {
            ahead: dec!("990"),
            own: dec!("10"),
        };
        let at_back = priced(&back);
        assert!(
            at_back.size < at_front.size,
            "a long queue ahead sized no smaller than the front of the queue"
        );
        assert!(
            at_back.size >= unknown.size,
            "a measured position sized below the unknown floor, which is supposed to be the worst \
             case"
        );
    }

    #[test]
    fn a_half_spread_beyond_the_ceiling_withholds_the_quote_instead_of_narrowing_to_it() {
        // The `MaxExpectedShortfall` shape avoided in the other direction: a
        // quote narrowed to the ceiling would be shown at a price the terms
        // said was wrong, and would fill. The premise — the same inputs one
        // step inside the ceiling are quoted — is asserted first.
        let ceiling = policy().max_half_spread_bps;
        let mut inside = inputs();
        inside.adverse_selection_bps = ceiling - policy().base_half_spread_bps;
        let pair = priced(&inside);
        assert_eq!(
            pair.terms.total_bps(),
            ceiling,
            "the premise is the ceiling"
        );

        let mut beyond = inputs();
        beyond.adverse_selection_bps = ceiling;
        let reason = withheld(&beyond);
        assert_eq!(reason.as_str(), "spread_beyond_ceiling");
        assert!(
            reason.describe().contains("message with no trade in it"),
            "the refusal does not say what is wrong: {}",
            reason.describe()
        );
    }

    #[test]
    fn an_originated_market_is_quoted_off_its_mandate_and_stops_at_the_mandates_ceiling() {
        // §29.3 made load-bearing on §29.1: the mandate is not a document
        // filed once, it is the anchor the quote is priced off and the bound
        // the size is taken from. Three states, because a ceiling that only
        // ever refused, or only ever admitted, would read the same in a test
        // of one of them.
        let mut fresh = inputs();
        fresh.object_id = "obj-ORIG".to_string();
        fresh.reference = QuoteReference::Originated {
            mandate: Box::new(mandate("50000")),
            exposure: Decimal::ZERO,
        };
        let pair = priced(&fresh);
        assert_eq!(
            pair.fair_value,
            dec!("100"),
            "the originated quote was not anchored on the mandate's valuation"
        );
        assert_eq!(pair.size, dec!("50"), "headroom well above the budget");

        // Headroom below the budget bounds the size: 100 of headroom at a fair
        // value of 100 is one unit, against the 50 the budget alone would buy.
        let mut nearly_full = fresh.clone();
        nearly_full.reference = QuoteReference::Originated {
            mandate: Box::new(mandate("50000")),
            exposure: dec!("49900"),
        };
        assert_eq!(
            priced(&nearly_full).size,
            dec!("1"),
            "the mandate's remaining headroom did not bound the size"
        );

        // And at the ceiling there is no quote at all.
        let mut full = fresh.clone();
        full.reference = QuoteReference::Originated {
            mandate: Box::new(mandate("50000")),
            exposure: dec!("50000"),
        };
        assert_eq!(withheld(&full).as_str(), "origination_ceiling_reached");
    }

    #[test]
    fn a_budget_that_buys_nothing_withholds_rather_than_quoting_a_size_of_zero() {
        // A zero-size quote is a quote: it occupies the book, it is a message,
        // and nothing can fill it. Saying so is the honest outcome.
        let mut broke = inputs();
        broke.budget = Decimal::ZERO;
        let reason = withheld(&broke);
        assert_eq!(reason.as_str(), "no_size");
        // The premise: the smallest budget that buys a unit is still quoted,
        // so this is not a gate that refuses every budget.
        let mut thin = inputs();
        thin.budget = dec!("200");
        assert_eq!(priced(&thin).size, dec!("1"));
    }

    #[test]
    fn a_malformed_reading_is_refused_and_never_repaired() {
        // Every one of these is a caller defect. Clamping an infinite
        // volatility to something large produces a price that looks
        // deliberate, which is the failure the house rule "refuse rather than
        // guess" exists for. Each spoils exactly one field of an otherwise
        // clean reading, so a blanket refusal would not explain them.
        assert!(quote(&policy(), &inputs()).is_ok(), "the premise");

        let mut infinite = inputs();
        infinite.volatility_bps = f64::INFINITY;
        assert_eq!(
            quote(&policy(), &infinite)
                .expect_err("an infinite volatility is refused")
                .code(),
            "invalid"
        );

        let mut unbounded = inputs();
        unbounded.inventory_limit = Decimal::ZERO;
        assert!(
            quote(&policy(), &unbounded).is_err(),
            "a zero inventory limit was admitted, which halts quoting on every pass"
        );

        let mut impossible = inputs();
        impossible.signal_imbalance = 1.5;
        assert!(
            quote(&policy(), &impossible).is_err(),
            "an imbalance outside [-1, 1] was admitted"
        );

        let mut nameless = inputs();
        nameless.object_id = String::new();
        assert!(quote(&policy(), &nameless).is_err());

        let mut phantom = inputs();
        phantom.queue = QueuePosition::Measured {
            ahead: dec!("10"),
            own: Decimal::ZERO,
        };
        assert!(
            quote(&policy(), &phantom).is_err(),
            "a measured queue position with nothing of ours in it was admitted"
        );
    }

    #[test]
    fn a_policy_whose_terms_could_price_a_quote_at_or_below_zero_is_refused_outright() {
        // The structural check, and the reason no arm of `quote` defends
        // against a non-positive bid. A defensive branch nothing can reach
        // reads as a control and is not one; refusing the policy is what makes
        // the branch unnecessary rather than merely absent.
        assert!(policy().validate().is_ok(), "the premise");

        let mut wide = policy();
        wide.max_half_spread_bps = 9_000.0;
        wide.skew_bps_at_limit = 1_000.0;
        let refusal = wide
            .validate()
            .expect_err("terms totalling one whole must be refused");
        assert_eq!(refusal.code(), "invalid");

        // One basis point under the whole is admitted, so the bound is a
        // boundary and not a refusal of every wide policy.
        let mut just_inside = policy();
        just_inside.imbalance_coefficient_bps = 0.0;
        just_inside.max_half_spread_bps = 8_999.0;
        just_inside.skew_bps_at_limit = 1_000.0;
        assert!(just_inside.validate().is_ok());

        // And a ceiling below the base, which would withhold every quote.
        let mut inverted = policy();
        inverted.max_half_spread_bps = inverted.base_half_spread_bps - 1.0;
        assert!(inverted.validate().is_err());
    }
}
