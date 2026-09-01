//! What the platform does when a cognitive capability is unavailable or stale.
//!
//! The rule this module states is that **losing a capability narrows the
//! platform; it never halts it**. That distinction is the whole point. A
//! system that halts on a stale input is a system that stops trading every
//! time a warm-path job is late, and the operational pressure to "just ignore
//! the staleness" then becomes irresistible — which is how a safety property
//! gets deleted by the people it protects.
//!
//! **This is a typed contract awaiting its consumer, and it enforces nothing
//! yet.** No engine calls it: `DegradationState` has no caller outside this
//! crate and its tests. Saying otherwise would be the exact overclaim this
//! module's own reasoning rejects elsewhere — the reason the self-model and
//! valuation rows are omitted below is that a control which cannot fire reads
//! as protection and is not, and that argument applies to the five rows here
//! just as well while nothing consults them.
//!
//! It is built before its consumer deliberately. The consumer is the signed
//! twelve-item payload of blueprint §41.5, whose stale-item narrowing is
//! defined in exactly these terms, and having the vocabulary settled first is
//! what stops that work from inventing a second one. Until it is wired, treat
//! this as a specification with a compiler checking it, not as a control.
//!
//! This is *capability*-level degradation and it composes with, rather than
//! replaces, the mechanism-level rules already in the tree: a stale book
//! supplying nothing (`qip-edge`'s seam) and venue health (`qip-routing`) are
//! about one book and one venue, and answer a different question from "the
//! causal graph has not been re-estimated for an hour, so how large may we
//! size?".
//!
//! Two design decisions carry the safety argument:
//!
//! * **Absence is the worst case, not the best.** [`DegradationState`] reports
//!   [`Freshness::Unavailable`] for a capability nobody has said anything
//!   about. A capability whose reporter has itself died would otherwise read
//!   as healthy, which is precisely the failure mode that makes a monitoring
//!   gap indistinguishable from good news.
//! * **Degradations compound rather than compete.** Two independent reasons to
//!   distrust a size multiply, so adding a degradation can only ever make the
//!   multiplier smaller. Taking the most conservative single rule instead
//!   would let a second, independent loss of confidence cost nothing.
//!
//! Only the capabilities this repository actually has are represented.
//! Blueprint §6.2 also names a self-model and a valuation engine; neither
//! exists here, and inventing an enum variant for a capability that can never
//! be unavailable would be a control that cannot fire — a mistake this
//! repository has already made nine times and does not need a tenth of.

use qip_core::{Decimal, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A cognitive capability whose loss changes what the platform will do.
///
/// Ordered, and iterated through [`BTreeMap`], because the narrowing a state
/// reports reaches an operator's screen and a replay that reorders is not a
/// replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Observation of the world. Blueprint §6.2 row 1.
    Ingestion,
    /// The causal graph over drivers. Row 2.
    CausalGraph,
    /// Episodic memory and analogical retrieval. Row 3.
    EpisodicMemory,
    /// The belief state, whose confidence drives size. Row 4.
    BeliefState,
    /// Counterfactual scoring of paths not taken. Row 5.
    CounterfactualScoring,
}

impl Capability {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Ingestion => "ingestion",
            Self::CausalGraph => "causal_graph",
            Self::EpisodicMemory => "episodic_memory",
            Self::BeliefState => "belief_state",
            Self::CounterfactualScoring => "counterfactual_scoring",
        }
    }

    pub const fn all() -> [Self; 5] {
        [
            Self::Ingestion,
            Self::CausalGraph,
            Self::EpisodicMemory,
            Self::BeliefState,
            Self::CounterfactualScoring,
        ]
    }

    /// Whether losing this capability may change a trading decision at all.
    ///
    /// Blueprint §6.2 is explicit that counterfactual scoring has "no trading
    /// impact whatsoever" — it is entirely a warm-path function. Stating that
    /// as a method rather than as a comment is what stops a later change from
    /// quietly giving the learning path a veto over the trading path.
    pub const fn affects_trading(&self) -> bool {
        !matches!(self, Self::CounterfactualScoring)
    }
}

/// How current a capability's state is.
///
/// Deliberately three-valued rather than a boolean. "Stale" and "unavailable"
/// narrow by different amounts in §6.2 — a stale belief falls back to a fixed
/// multiplier while an absent one is the same thing only because we choose to
/// treat it that way — and collapsing them would lose the distinction the
/// table draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    /// Current, within its TTL.
    Fresh,
    /// Past its TTL but still readable.
    Stale,
    /// Not readable at all.
    Unavailable,
}

impl Freshness {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }

    /// Parse a freshness token, refusing anything it does not recognise.
    ///
    /// Refuses rather than defaulting, on the standing rule that a value
    /// silently corrected is a caller bug that survives. The refusal names the
    /// permitted tokens because an error that does not say what to do instead
    /// is a dead end.
    ///
    /// The match is on the whole string, not a prefix or a substring: `"stale"`
    /// is a substring of a great many things, and a policy file with a typo
    /// that happened to contain it would otherwise be read as a deliberate
    /// degradation.
    pub fn parse(token: &str) -> Result<Self> {
        match token {
            "fresh" => Ok(Self::Fresh),
            "stale" => Ok(Self::Stale),
            "unavailable" => Ok(Self::Unavailable),
            other => Err(Error::invalid(format!(
                "unknown freshness {other:?}; expected one of fresh, stale, unavailable"
            ))),
        }
    }

    /// Whether the capability is usable at full strength.
    pub const fn is_fresh(&self) -> bool {
        matches!(self, Self::Fresh)
    }
}

/// The classes of strategy §6.2 treats differently when a capability is lost.
///
/// The table's rows are not "everything pauses" — they are careful about which
/// strategies keep running, and that care is the useful part. A price-only
/// strategy does not consult the world model, so an ingestion stall is not its
/// problem, and pausing it would be an outage the platform inflicted on itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyClass {
    /// Depends on prices and books alone.
    PriceOnly,
    /// Depends on world events — filings, releases, news.
    EventDriven,
    /// Depends on resolution of a real-world outcome.
    PredictionMarket,
    /// Depends on recognising a situation it has seen before.
    SituationalRecognition,
}

impl StrategyClass {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::PriceOnly => "price_only",
            Self::EventDriven => "event_driven",
            Self::PredictionMarket => "prediction_market",
            Self::SituationalRecognition => "situational_recognition",
        }
    }

    pub const fn all() -> [Self; 4] {
        [
            Self::PriceOnly,
            Self::EventDriven,
            Self::PredictionMarket,
            Self::SituationalRecognition,
        ]
    }
}

/// How capital is spread across families.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationMode {
    /// Conditioned on the detected regime — the full-capability mode.
    RegimeConditional,
    /// The fallback when the causal graph can no longer be reasoned about.
    Unconditional,
}

/// The multiplier applied to size when the causal graph cannot be trusted.
///
/// §6.2: "Sizing becomes more conservative because relationships can no longer
/// be reasoned about." The number is a policy choice and this is the one place
/// it is written down; what the type system holds is that it is below one and
/// that it compounds.
const CAUSAL_STALE_MULTIPLIER: (i128, u32) = (75, 2);

/// The fixed conservative multiplier §6.2 row 4 falls back to when the belief
/// state is stale beyond its TTL.
///
/// Fixed on purpose: the point of the fallback is that it does not depend on a
/// confidence estimate, because the confidence estimate is the thing that has
/// gone stale.
const BELIEF_STALE_MULTIPLIER: (i128, u32) = (50, 2);

/// What the platform currently believes about each capability, and what that
/// implies.
///
/// Construct with [`DegradationState::fully_available`] and record what is
/// known; anything not recorded reads as [`Freshness::Unavailable`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DegradationState {
    freshness: BTreeMap<Capability, Freshness>,
}

impl DegradationState {
    /// Every capability fresh. The state a healthy platform reports.
    pub fn fully_available() -> Self {
        let mut freshness = BTreeMap::new();
        for capability in Capability::all() {
            freshness.insert(capability, Freshness::Fresh);
        }
        Self { freshness }
    }

    /// Nothing known about anything, which fails closed to fully degraded.
    pub fn nothing_known() -> Self {
        Self {
            freshness: BTreeMap::new(),
        }
    }

    /// Record what is known about one capability.
    pub fn observe(&mut self, capability: Capability, freshness: Freshness) {
        self.freshness.insert(capability, freshness);
    }

    /// What is known about one capability.
    ///
    /// **Fails closed.** A capability nobody has reported on is `Unavailable`,
    /// not `Fresh`. The alternative makes a dead reporter look identical to a
    /// healthy subsystem, and the platform would size as though it still knew
    /// something it had merely stopped being told.
    pub fn freshness(&self, capability: Capability) -> Freshness {
        self.freshness
            .get(&capability)
            .copied()
            .unwrap_or(Freshness::Unavailable)
    }

    /// Whether analogical retrieval is available.
    ///
    /// §6.2 row 3 has two clauses — "analogical retrieval unavailable" and
    /// "strategies depending on situational recognition pause" — and only the
    /// second had an accessor. A caller that wants to know whether it may
    /// *retrieve* an analogue, rather than whether it must stop, had to infer
    /// it from a strategy class it may not have.
    pub fn analogical_retrieval_available(&self) -> bool {
        self.freshness(Capability::EpisodicMemory).is_fresh()
    }

    /// Whether a strategy of this class pauses.
    ///
    /// Straight from §6.2, and the negative half matters as much as the
    /// positive: a price-only strategy pauses for nothing in this table,
    /// because nothing in this table is an input to it.
    pub fn pauses(&self, class: StrategyClass) -> bool {
        let ingestion_lost = !self.freshness(Capability::Ingestion).is_fresh();
        let episodic_lost = !self.freshness(Capability::EpisodicMemory).is_fresh();
        match class {
            // Not a default arm. Written out so that adding a class forces a
            // decision here rather than inheriting "never pauses" by accident.
            StrategyClass::PriceOnly => false,
            StrategyClass::EventDriven | StrategyClass::PredictionMarket => ingestion_lost,
            StrategyClass::SituationalRecognition => episodic_lost,
        }
    }

    /// The multiplier applied to position size under the current degradation.
    ///
    /// `Decimal` and never `f64`: this scales a position, so it is money
    /// arithmetic however statistical its inputs were.
    ///
    /// Compounding rather than competing. Two independent reasons to distrust
    /// a size are two reasons, and a scheme that took the most conservative
    /// single rule would let the second one cost nothing.
    pub fn sizing_multiplier(&self) -> Decimal {
        let mut multiplier = Decimal::from_int(1);
        if !self.freshness(Capability::CausalGraph).is_fresh() {
            multiplier = Self::scaled(CAUSAL_STALE_MULTIPLIER, multiplier);
        }
        if !self.freshness(Capability::BeliefState).is_fresh() {
            multiplier = Self::scaled(BELIEF_STALE_MULTIPLIER, multiplier);
        }
        multiplier
    }

    /// Apply a policy constant, narrowing further if it cannot be represented.
    ///
    /// The fallback is deliberately asymmetric. If the constant or the product
    /// cannot be represented we return zero rather than the un-narrowed
    /// multiplier, because the instruction that governs every branch here is
    /// that an unrepresentable state narrows further and never less.
    fn scaled(constant: (i128, u32), current: Decimal) -> Decimal {
        let zero = Decimal::from_int(0);
        let Some(factor) = Decimal::from_scaled(constant.0, constant.1) else {
            return zero;
        };
        current.checked_mul(factor).unwrap_or(zero)
    }

    /// `scaled`, reachable from a test.
    ///
    /// Both fallible branches above are unreachable through
    /// [`Self::sizing_multiplier`], because the only constants it applies are
    /// 0.75 and 0.5 against a starting 1 and neither can fail. That made the
    /// module's central safety claim — that an unrepresentable state narrows
    /// further and never less — an assertion in prose with nothing exercising
    /// it: inverting `unwrap_or(zero)` to `unwrap_or(current)`, so that a
    /// failed multiply *widens*, left the whole suite green.
    ///
    /// This exists so the claim is checked rather than merely made.
    #[cfg(test)]
    fn scaled_for_test(constant: (i128, u32), current: Decimal) -> Decimal {
        Self::scaled(constant, current)
    }

    /// How capital is spread, given what can still be reasoned about.
    pub fn allocation_mode(&self) -> AllocationMode {
        if self.freshness(Capability::CausalGraph).is_fresh() {
            AllocationMode::RegimeConditional
        } else {
            AllocationMode::Unconditional
        }
    }

    /// Whether the platform halts.
    ///
    /// Always false, and it is a method rather than an absence so that the
    /// property is testable and so that a future change that wants to halt has
    /// to come through here and explain itself. §6.2's entire premise is that
    /// the platform narrows rather than stopping; halting belongs to the kill
    /// switch, which is a control an operator holds, not a consequence of a
    /// warm-path job being late.
    pub const fn halts(&self) -> bool {
        false
    }

    /// Every capability that is not fresh, in a deterministic order.
    pub fn narrowed(&self) -> BTreeMap<Capability, Freshness> {
        Capability::all()
            .into_iter()
            .map(|capability| (capability, self.freshness(capability)))
            .filter(|(_, freshness)| !freshness.is_fresh())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multiplier_that_cannot_be_represented_narrows_to_nothing() {
        // The fail-closed arm, which no path through `sizing_multiplier` can
        // reach. The asymmetry is the point: when a constant or a product
        // cannot be represented we return zero, not the un-narrowed
        // multiplier, because every branch here narrows further and never
        // less.
        let one = Decimal::from_int(1);

        // Premise: a representable constant does *not* return zero, so the
        // assertions below are about the failure and not about the function
        // always returning zero.
        let ordinary = DegradationState::scaled_for_test((75, 2), one);
        assert!(
            !ordinary.is_zero(),
            "a representable constant returned zero"
        );

        // An exponent no decimal of this scale can carry.
        let unrepresentable = DegradationState::scaled_for_test((1, u32::MAX), one);
        assert!(
            unrepresentable.is_zero(),
            "an unrepresentable constant widened instead of narrowing: \
             {unrepresentable:?}"
        );

        // And the multiply itself, driven past what the type can hold. Both
        // operands must be *representable* or the `from_scaled` branch above
        // catches it first and this arm stays untested — which is exactly what
        // happened on the first attempt: `from_scaled(i128::MAX, 0)` overflows
        // its own scaling and returns `None`, so the case was skipped and
        // inverting `unwrap_or(zero)` to `unwrap_or(current)` left the suite
        // green.
        //
        // `Decimal` scales by 10^9, so a mantissa near 10^28 is representable
        // while the square of one is not.
        let big = 10i128.pow(28);
        let representable =
            Decimal::from_scaled(big, 0).expect("10^28 is within the decimal's range");
        assert!(
            !representable.is_zero(),
            "the premise failed: the operand is not representable, so the \
             multiply below is not the thing being tested"
        );
        let overflowed = DegradationState::scaled_for_test((big, 0), representable);
        assert!(
            overflowed.is_zero(),
            "an overflowing multiply widened instead of narrowing: {overflowed:?}"
        );
    }
}
