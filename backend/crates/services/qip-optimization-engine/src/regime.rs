//! Blueprint §23.3, regime-conditional allocation — the narrowing half, and
//! only the narrowing half.
//!
//! The section's table says which alpha families a regime favours. Read
//! literally it asks for two directions at once: give momentum more when the
//! tape trends, give it less when the tape turns. **Only the second is built
//! here, and the type system holds that line rather than a convention:**
//! [`Stance::multiplier`] has no arm above one, so no composition of this
//! module's outputs can raise a bound. The favouring direction leaves
//! [`favoured`] as a set of family names carrying no number at all, for a
//! caller to journal as a finding.
//!
//! That asymmetry is ADR 0061's and ADR 0063's, applied to a third row. A
//! platform that can widen its own bounds because a model said the regime
//! turned has an escalation path whose authority is a classifier, and the
//! classifier this reads — `Platform::market_regime` — is four branches over
//! a drawdown, a spread and a sign-persistence count. That is enough evidence
//! to take risk off. It is not enough to put risk on.
//!
//! # One classifier, one answer
//!
//! [`AllocationRegime`] is a *transport* of the regime the platform already
//! decides, not a second opinion about it. The kernel maps
//! `qip_cost_router::MarketRegime` onto it with an exhaustive `match`, so a
//! sixth regime arm added there fails the build rather than falling into a
//! default that sizes as though nothing had changed. Nothing in this module
//! looks at a price, a spread or a drawdown.
//!
//! # What the unattributed case does, and why it is the conservative one
//!
//! No production path in this platform attributes an alpha family to a sized
//! instrument: a thesis carries an object and a conviction, and the two
//! existing "family" notions are a correlation cluster and a sweep name
//! (see [`crate::universe`]). So the case that actually runs is
//! [`unattributed_multiplier`], which is the **narrowest** stance any family
//! would receive in that regime.
//!
//! That is the fail-closed direction, and it is deliberately not free: an
//! instrument whose alpha source nobody recorded is sized as the family the
//! regime suits least. The alternative — treating an unknown family as
//! unaffected — would make §23.3 a control that cannot fire, because the
//! only input it ever receives in production is "unknown". This repository
//! keeps the receipt for that shape under `MaxExpectedShortfall`.
//!
//! # Units
//!
//! Every number here is a multiplier on a *weight bound* — a fraction of a
//! fraction of equity, never money — so `f64` is the right type and is the
//! type `qip_portfolio_engine`'s bounds are already expressed in. The
//! crossing point into `Decimal` is at the kernel's seam, where these caps
//! meet a budget, and it is stated there.

use crate::universe::AlphaFamily;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The regime the platform has classified, as this module reads it.
///
/// The five arms are `qip_cost_router::MarketRegime`'s five arms and exist
/// so this crate need not depend on that one — services meet in the kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AllocationRegime {
    /// Prices are going somewhere and continuing to.
    Trending,
    /// Prices come back.
    MeanReverting,
    /// Correlations go to one and liquidity goes to nothing.
    Crisis,
    /// The book is thin enough that the price is an opinion.
    Illiquid,
    /// Nothing is happening — and, in the classifier this reads, also the
    /// arm returned when there is too little tape to tell. See
    /// [`AllocationRegime::belief_is_wide`].
    Quiet,
}

impl AllocationRegime {
    pub const ALL: [Self; 5] = [
        Self::Trending,
        Self::MeanReverting,
        Self::Crisis,
        Self::Illiquid,
        Self::Quiet,
    ];

    /// The key the classifier itself spells, so a regime that travels as a
    /// string and comes back is the same regime.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trending => "trending",
            Self::MeanReverting => "mean_reverting",
            Self::Crisis => "crisis",
            Self::Illiquid => "illiquid",
            Self::Quiet => "quiet",
        }
    }

    /// The regime named by exactly this key, refusing anything else.
    ///
    /// Whole-token equality. `mean_reverting` contains `reverting` and
    /// nothing here may match on a fragment: a regime read wrongly off a
    /// label is a stance applied to the wrong tape.
    pub fn parse(key: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|regime| regime.as_str() == key)
            .ok_or_else(|| {
                let known: Vec<&str> = Self::ALL.iter().map(|regime| regime.as_str()).collect();
                Error::invalid(format!(
                    "{key:?} names no market regime; the classifier spells one of {}",
                    known.join(", ")
                ))
            })
    }

    /// Whether the platform's belief about this regime is wide enough that
    /// §23.3's last row applies: "reduce everything; favour arbitrage whose
    /// edge does not depend on regime".
    ///
    /// Three arms qualify and the argument differs for each:
    ///
    /// * `Crisis` is the row's own subject — correlations at one means the
    ///   family boundaries an allocation is drawn on have stopped holding.
    /// * `Illiquid` qualifies because a price that is an opinion is not
    ///   evidence about a regime; every classification downstream of it is
    ///   drawn on a quote nobody can trade.
    /// * `Quiet` qualifies because the classifier returns it both for a
    ///   genuinely still tape **and** for a subject with fewer than three
    ///   usable returns — it is the arm that means "I could not tell". The
    ///   two are indistinguishable to a reader, so the reader takes the
    ///   conservative one. That is also the arm a freshly started process
    ///   sees, which is exactly when the platform knows least.
    pub const fn belief_is_wide(self) -> bool {
        matches!(self, Self::Crisis | Self::Illiquid | Self::Quiet)
    }
}

/// What a regime does to one family's weight bound.
///
/// Four arms, two of which mean "change nothing". That is not redundancy:
/// `Favoured` and `Unaffected` produce the same multiplier and say different
/// things, and the difference is what [`favoured`] reports and what a
/// journalled record must be able to distinguish. A favouring that produced
/// a number would be the escalation this module refuses to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Stance {
    /// The regime's own table names this family. The bound does not move:
    /// favouring is a finding, never a multiplier.
    Favoured,
    /// The regime says nothing about this family.
    Unaffected,
    /// This family's edge works against this regime.
    Narrowed,
    /// §23.3's last row: the platform does not know what regime it is in, so
    /// everything without a structural edge comes down.
    Reduced,
}

/// What a family whose edge fights the regime keeps of its bound.
///
/// Three quarters: one auditable number rather than a curve fitted after the
/// fact, in the spirit of ADR 0063's single halving. Distinct from
/// [`REGIME_REDUCED_MULTIPLIER`] on purpose — a stance taken because the
/// platform *knows* the regime is a weaker claim against a position than one
/// taken because it does not.
pub const REGIME_NARROWED_MULTIPLIER: f64 = 0.75;

/// What a family keeps when the platform cannot say what regime it is in.
///
/// A half, the same number ADR 0063's sizing cap uses, and for the same
/// reason: one number a person can name and check against the record.
pub const REGIME_REDUCED_MULTIPLIER: f64 = 0.5;

impl Stance {
    /// The multiplier on the mandate's position cap for this name, in
    /// `(0, 1]`.
    ///
    /// **There is no arm above one.** That is the guarantee of this module
    /// and it is structural: an edit that wanted to reward a favoured family
    /// would have to change this function, and every regime and every family
    /// is asserted against it below.
    pub const fn multiplier(self) -> f64 {
        match self {
            Self::Favoured | Self::Unaffected => 1.0,
            Self::Narrowed => REGIME_NARROWED_MULTIPLIER,
            Self::Reduced => REGIME_REDUCED_MULTIPLIER,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Favoured => "favoured",
            Self::Unaffected => "unaffected",
            Self::Narrowed => "narrowed",
            Self::Reduced => "reduced",
        }
    }
}

/// §23.3's table: what `regime` does to `family`.
///
/// The two directional regimes narrow the families whose alpha source runs
/// against them and leave the rest alone; the three regimes under which the
/// platform's belief is wide reduce everything except the one family whose
/// edge does not depend on the regime at all.
pub const fn stance(regime: AllocationRegime, family: AlphaFamily) -> Stance {
    if regime.belief_is_wide() {
        // "Reduce everything; favour arbitrage whose edge does not depend on
        // regime." Favoured here means *not reduced* — the bound is
        // unchanged, and no arm of this function can hand arbitrage more.
        return if family.is_regime_agnostic() {
            Stance::Favoured
        } else {
            Stance::Reduced
        };
    }
    match (regime, family) {
        // Breadth expanding, volatility contained → momentum and trend.
        (AllocationRegime::Trending, AlphaFamily::MomentumAndTrend) => Stance::Favoured,
        // Both of these harvest a price coming back, which is the bet a
        // trend is taking the other side of.
        (
            AllocationRegime::Trending,
            AlphaFamily::ShortHorizonReversion | AlphaFamily::StatisticalArbitrage,
        ) => Stance::Narrowed,
        // Volatility elevated, breadth narrow → short reversion, market
        // making at wider spreads.
        (
            AllocationRegime::MeanReverting,
            AlphaFamily::ShortHorizonReversion | AlphaFamily::MarketMaking,
        ) => Stance::Favoured,
        // Continuation is the one claim a tape that comes back refutes.
        (AllocationRegime::MeanReverting, AlphaFamily::MomentumAndTrend) => Stance::Narrowed,
        _ => Stance::Unaffected,
    }
}

/// The multiplier on `family`'s weight bound under `regime`, in `(0, 1]`.
pub const fn multiplier(regime: AllocationRegime, family: AlphaFamily) -> f64 {
    stance(regime, family).multiplier()
}

/// The multiplier for an instrument whose alpha family nobody recorded: the
/// narrowest stance any family takes under `regime`.
///
/// This is the function production actually calls, because no production
/// path attributes a family. It is the conservative answer by construction —
/// a minimum over a table with no arm above one — so an unattributed name is
/// sized as the family the regime suits least.
pub fn unattributed_multiplier(regime: AllocationRegime) -> f64 {
    AlphaFamily::ALL
        .into_iter()
        .map(|family| multiplier(regime, family))
        // Every value is a finite constant out of `Stance::multiplier`, so
        // there is no NaN here for a `partial_cmp` to fail on; `f64::min`
        // states that rather than relying on a total order f64 lacks.
        .fold(1.0_f64, f64::min)
}

/// The families `regime`'s own table names, carrying no number.
///
/// The loosening direction of §23.3, and the whole of it. A caller journals
/// this as a finding — "the tape favours these, and the platform did not act
/// on that" — the way `sizing_review::larger_size_finding` journals the
/// larger-size direction. `BTreeSet` because it reaches a record and a
/// replay that reorders is not a replay.
pub fn favoured(regime: AllocationRegime) -> BTreeSet<AlphaFamily> {
    AlphaFamily::ALL
        .into_iter()
        .filter(|family| stance(regime, *family) == Stance::Favoured)
        .collect()
}

/// One line naming what the regime did to an unattributed name, for the
/// proposal's own compromises.
pub fn describe(regime: AllocationRegime) -> String {
    let multiplier = unattributed_multiplier(regime);
    let named: Vec<&str> = favoured(regime)
        .into_iter()
        .map(AlphaFamily::as_str)
        .collect();
    format!(
        "regime {} narrows an unattributed name's bound to {multiplier} of the mandate's cap; \
         the regime favours {} and nothing was widened for it",
        regime.as_str(),
        if named.is_empty() {
            "no family".to_string()
        } else {
            named.join(", ")
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact equality is what every assertion below means: each value in the
    /// table is a binary fraction written as a literal, so nothing here is
    /// the accumulated error `float_cmp` warns about. The epsilon satisfies
    /// the lint without suppressing it, because an `allow` on a whole test
    /// module would also cover a future assertion that genuinely is comparing
    /// two computed reals.
    fn same(left: f64, right: f64) -> bool {
        (left - right).abs() < f64::EPSILON
    }

    #[test]
    fn no_regime_and_no_family_can_widen_a_weight_bound() {
        // §12.4's guardrail as arithmetic over the whole table rather than
        // as care taken at a call site. The premise first: the table is not
        // empty and it is not uniform — if every cell were one, this test
        // would pass while guarding nothing, which is the exact shape of
        // failure this repository records under `MaxExpectedShortfall`.
        let mut cells = 0usize;
        let mut narrowing = 0usize;
        for regime in AllocationRegime::ALL {
            for family in AlphaFamily::ALL {
                cells += 1;
                let multiplier = multiplier(regime, family);
                assert!(
                    multiplier.is_finite() && multiplier > 0.0,
                    "{} × {} is {multiplier}, which is not a usable bound",
                    regime.as_str(),
                    family.as_str()
                );
                assert!(
                    multiplier <= 1.0,
                    "{} × {} widens a bound to {multiplier}",
                    regime.as_str(),
                    family.as_str()
                );
                if multiplier < 1.0 {
                    narrowing += 1;
                }
            }
        }
        assert_eq!(cells, 50, "the table is five regimes by ten families");
        assert!(
            narrowing > 0,
            "no cell narrows anything, so the whole table is a control that cannot fire"
        );
    }

    #[test]
    fn a_regime_the_platform_cannot_name_reduces_every_family_but_arbitrage() {
        // §23.3's closing paragraph, which is the row the platform actually
        // reaches: the classifier answers `quiet` for a subject with too
        // little tape to tell, which is every subject on a freshly started
        // process.
        let wide: Vec<AllocationRegime> = AllocationRegime::ALL
            .into_iter()
            .filter(|regime| regime.belief_is_wide())
            .collect();
        assert_eq!(wide.len(), 3, "the premise is three wide-belief regimes");
        for regime in wide {
            assert_eq!(
                stance(regime, AlphaFamily::Arbitrage),
                Stance::Favoured,
                "{} reduced the one family whose edge does not depend on it",
                regime.as_str()
            );
            assert!(same(multiplier(regime, AlphaFamily::Arbitrage), 1.0));
            let reduced: Vec<&str> = AlphaFamily::ALL
                .into_iter()
                .filter(|family| stance(regime, *family) == Stance::Reduced)
                .map(AlphaFamily::as_str)
                .collect();
            assert_eq!(
                reduced.len(),
                9,
                "{} reduced {reduced:?}, not the other nine families",
                regime.as_str()
            );
            assert!(same(
                unattributed_multiplier(regime),
                REGIME_REDUCED_MULTIPLIER
            ));
        }
        // And the premise that this is a distinction rather than a
        // tautology: the two directional regimes reduce nothing, and an
        // unattributed name under them keeps more than under a wide belief.
        for regime in [AllocationRegime::Trending, AllocationRegime::MeanReverting] {
            assert!(!regime.belief_is_wide());
            assert!(
                AlphaFamily::ALL
                    .into_iter()
                    .all(|family| stance(regime, family) != Stance::Reduced),
                "{} took the uncertain row",
                regime.as_str()
            );
            assert!(same(
                unattributed_multiplier(regime),
                REGIME_NARROWED_MULTIPLIER
            ));
        }
        assert!(
            unattributed_multiplier(AllocationRegime::Crisis)
                < unattributed_multiplier(AllocationRegime::Trending),
            "not knowing the regime must cost more than knowing it, or the regime is not an \
             input to the size at all"
        );
    }

    #[test]
    fn a_favoured_family_is_a_finding_and_never_a_number() {
        // The trending tape favours momentum, and momentum's bound is
        // unchanged rather than widened — the asymmetry ADR 0061 and ADR
        // 0063 hold for their own rows.
        let favoured = favoured(AllocationRegime::Trending);
        assert!(
            favoured.contains(&AlphaFamily::MomentumAndTrend),
            "the trending table names momentum and the finding did not"
        );
        assert!(
            same(
                multiplier(AllocationRegime::Trending, AlphaFamily::MomentumAndTrend),
                1.0
            ),
            "a favoured family was handed more than its mandate's cap"
        );
        // And the regime that favours it narrows the families that fight it,
        // so the finding is not the only thing the table says.
        assert!(same(
            multiplier(
                AllocationRegime::Trending,
                AlphaFamily::ShortHorizonReversion
            ),
            REGIME_NARROWED_MULTIPLIER
        ));
        assert!(
            same(
                multiplier(
                    AllocationRegime::MeanReverting,
                    AlphaFamily::MomentumAndTrend
                ),
                REGIME_NARROWED_MULTIPLIER
            ),
            "the mirror row does not narrow continuation when the tape comes back"
        );
    }

    #[test]
    fn a_regime_key_and_a_family_key_are_matched_whole() {
        // Substring matching is the trap this repository has already paid
        // for: `arbitrage` is a substring of `statistical_arbitrage`, and
        // the two take opposite stances under a wide belief — one of them is
        // the family that is never reduced.
        assert_eq!(
            AlphaFamily::parse("statistical_arbitrage").expect("a known key"),
            AlphaFamily::StatisticalArbitrage
        );
        assert_eq!(
            stance(AllocationRegime::Crisis, AlphaFamily::StatisticalArbitrage),
            Stance::Reduced
        );
        assert_eq!(
            stance(AllocationRegime::Crisis, AlphaFamily::Arbitrage),
            Stance::Favoured
        );
        for family in AlphaFamily::ALL {
            assert_eq!(
                AlphaFamily::parse(family.as_str()).expect("round trip"),
                family
            );
        }
        for regime in AllocationRegime::ALL {
            assert_eq!(
                AllocationRegime::parse(regime.as_str()).expect("round trip"),
                regime
            );
        }
        let refusal = AlphaFamily::parse("arbitrag").expect_err("a near miss is refused");
        assert!(
            refusal.message().contains("names no alpha family"),
            "the refusal does not say what was wrong: {}",
            refusal.message()
        );
        assert!(
            AllocationRegime::parse("reverting").is_err(),
            "a fragment of mean_reverting was accepted as a regime"
        );
    }
}
