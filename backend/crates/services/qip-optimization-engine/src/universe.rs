//! The blueprint's §19 strategy universe: ten alpha families, each with a
//! distinct alpha source, horizon, evaluation tier and capacity profile.
//!
//! # This is the third thing in this workspace called a "family", and the
//! three are not the same
//!
//! Read this before using [`AlphaFamily`] anywhere, because conflating it
//! with either of the other two silently produces a decision keyed on a
//! subject nobody measured.
//!
//! * [`crate::families::FamilyId`] is a **correlation cluster**, recomputed
//!   every cycle from stress correlation. Its membership moves as the
//!   correlation moves, which is the whole point of it, and it carries no
//!   name a person chose.
//! * `qip_lifecycle::trials::StrategyFamily` is a **provenance key** naming
//!   the sweep a strategy was enrolled from, fixed at enrolment precisely so
//!   a trial count cannot be laundered by renaming.
//! * [`AlphaFamily`] — here — is the **taxonomy**: what kind of edge the
//!   strategy claims to harvest. It is a property of the strategy's design
//!   rather than of its returns, so it neither moves with correlation nor
//!   depends on which sweep produced it.
//!
//! A regime favours or disfavours an *alpha source* (§23.3), and an
//! evaluation tier is a property of an alpha source's *horizon* (§19.2).
//! Neither question can be answered by a correlation cluster or by a sweep
//! name, which is why this third thing exists rather than being folded into
//! one of the first two.
//!
//! # Nothing here infers a family from a string
//!
//! [`AlphaFamily::parse`] matches the whole token and refuses anything else.
//! There is deliberately no fuzzy or prefix match: a sweep named
//! `momentum-v3` is not evidence that its strategies harvest continuation,
//! and a taxonomy assigned by guessing at a name would put a strategy in a
//! regime stance and an evaluation tier that nobody chose for it. The
//! callers that cannot attribute a family say so by passing `None`, and
//! every consumer here treats `None` as the most conservative answer it has.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// One of the blueprint's ten alpha families (§19).
///
/// Ordered as the blueprint's own table orders them, hottest tier first, so
/// [`AlphaFamily::ALL`] reads against the source. The derived `Ord` follows
/// that declaration order and reaches output through `BTreeSet`s, so it is
/// part of the replay contract: reordering these arms reorders a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AlphaFamily {
    /// Bid-ask spread, minus adverse selection.
    MarketMaking,
    /// Price inconsistency across venues, assets, representations.
    Arbitrage,
    /// Order book imbalance, queue dynamics, trade flow.
    Microstructure,
    /// Overreaction to order flow.
    ShortHorizonReversion,
    /// Continuation across horizons.
    MomentumAndTrend,
    /// Mean reversion in spreads, pairs, baskets.
    StatisticalArbitrage,
    /// Funding, basis, roll, term structure.
    Carry,
    /// Scheduled and unscheduled information events.
    EventDriven,
    /// Implied versus realised, surface shape, dispersion.
    Volatility,
    /// Improving the fills of every other family.
    ExecutionAlpha,
}

impl AlphaFamily {
    /// Every family, in the blueprint's table order.
    pub const ALL: [Self; 10] = [
        Self::MarketMaking,
        Self::Arbitrage,
        Self::Microstructure,
        Self::ShortHorizonReversion,
        Self::MomentumAndTrend,
        Self::StatisticalArbitrage,
        Self::Carry,
        Self::EventDriven,
        Self::Volatility,
        Self::ExecutionAlpha,
    ];

    /// The stable key this family is recorded and matched under.
    ///
    /// A fixed spelling rather than a serialisation, so a serde attribute
    /// changed elsewhere cannot silently re-key a journalled stance.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MarketMaking => "market_making",
            Self::Arbitrage => "arbitrage",
            Self::Microstructure => "microstructure",
            Self::ShortHorizonReversion => "short_horizon_reversion",
            Self::MomentumAndTrend => "momentum_and_trend",
            Self::StatisticalArbitrage => "statistical_arbitrage",
            Self::Carry => "carry",
            Self::EventDriven => "event_driven",
            Self::Volatility => "volatility",
            Self::ExecutionAlpha => "execution_alpha",
        }
    }

    /// The family named by exactly this key.
    ///
    /// Whole-token equality, never a prefix or a substring: `arbitrage` and
    /// `statistical_arbitrage` are different alpha sources with different
    /// regime stances, and a `contains` here would file every statistical
    /// arbitrage strategy under the one family §23.3 calls regime-agnostic —
    /// which is the one that is never reduced. The refusal names the ten
    /// keys so a caller that mis-spelled one can see which it meant.
    pub fn parse(key: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|family| family.as_str() == key)
            .ok_or_else(|| {
                let known: Vec<&str> = Self::ALL.iter().map(|family| family.as_str()).collect();
                Error::invalid(format!(
                    "{key:?} names no alpha family; supply one of {}",
                    known.join(", ")
                ))
            })
    }

    /// Whether this family's edge survives not knowing the regime.
    ///
    /// §23.3's closing sentence, as a property of the family rather than as
    /// a special case inside the stance table: "Arbitrage is regime-agnostic;
    /// momentum is not." A price inconsistency between two venues is an
    /// inconsistency whatever the tape is doing; every other family here
    /// prices a view about what the tape will do next.
    ///
    /// [`AlphaFamily::ExecutionAlpha`] is deliberately *not* regime-agnostic
    /// even though it looks like plumbing: it improves the fills of whatever
    /// the other families are doing, so it inherits their exposure to the
    /// regime and has no standalone edge to fall back on.
    pub const fn is_regime_agnostic(self) -> bool {
        matches!(self, Self::Arbitrage)
    }
}
