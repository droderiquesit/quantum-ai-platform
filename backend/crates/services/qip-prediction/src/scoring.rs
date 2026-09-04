//! Does the platform's probability beat the market's, on the same contract,
//! at the same instant?
//!
//! This is the whole of the Phase 6 gate, and until this module existed the
//! platform could produce a calibrated probability and a venue-implied one and
//! had no arithmetic that put them side by side. Two things make the naive
//! version of that arithmetic worthless, and both are refused here rather than
//! documented:
//!
//! * **A later quote is leakage.** The market probability scored against must
//!   be the one knowable when the platform formed its own. A quote that became
//!   known afterwards has already absorbed information the platform did not
//!   have, and a comparison against it says nothing about forecasting skill —
//!   it says the venue's later price was closer to the outcome, which is
//!   always true and never interesting. [`ScoredForecast::new`] takes both
//!   probabilities as bitemporal stamps and refuses a market stamp whose
//!   known-time is later than the platform's.
//! * **A score without a baseline is a number, not a comparison.** Brier
//!   alone says how wrong the platform was; it does not say whether anyone
//!   could have done better. [`compare`] scores both forecasters on the same
//!   contracts and reports the difference with a paired standard error, so
//!   "better" carries the error bar a desk needs before believing it.
//!
//! An unresolved contract cannot be scored — there is no outcome to score
//! against — and a disputed or proposed resolution is a claim rather than a
//! fact, so [`ScoredForecast::new`] accepts only a final resolution, for the
//! same reason [`MarketResolution::settle`] does.

use qip_contracts::Stamped;
use qip_core::ObjectId;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::market::OutcomeId;
use crate::oracle::{MarketResolution, ResolutionState};
use crate::pricing::Probability;

/// One resolved contract with the two probabilities that were held on it at
/// one instant, and what actually happened.
///
/// Both stamps are kept, not just the values: the leakage refusal in
/// [`ScoredForecast::new`] is only as strong as the caller's inability to
/// construct one of these without it, and a value with its stamp discarded
/// could not be re-checked later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoredForecast {
    market_id: ObjectId,
    outcome: OutcomeId,
    platform: Stamped<Probability>,
    market: Stamped<Probability>,
    resolved_yes: bool,
}

impl ScoredForecast {
    /// Pair the platform's probability for `outcome` with the venue's implied
    /// probability for the same outcome, against a final resolution.
    ///
    /// Refuses, in order of how badly each would mislead:
    ///
    /// * a market stamp known later than the platform's — that is a quote the
    ///   platform could not have seen, and scoring against it is leakage;
    /// * a resolution for a different market — a score against the wrong
    ///   contract's outcome is a fabrication that looks exactly like a result;
    /// * a resolution that is not final — a proposed or disputed outcome may
    ///   still be overturned, and a void one has no outcome at all.
    pub fn new(
        market_id: ObjectId,
        outcome: OutcomeId,
        platform: Stamped<Probability>,
        market: Stamped<Probability>,
        resolution: &MarketResolution,
    ) -> Result<Self> {
        if market.known_at() > platform.known_at() {
            return Err(Error::invalid(format!(
                "the market quote for {market_id}/{outcome} became known at {}, after the platform's probability at {}; score against the quote that was knowable then, not a later one",
                market.known_at().to_rfc3339(),
                platform.known_at().to_rfc3339()
            )));
        }
        if resolution.market_id != market_id {
            return Err(Error::invalid(format!(
                "the resolution is for market {}, not {market_id}; score a contract against its own resolution",
                resolution.market_id
            )));
        }
        let ResolutionState::Final { outcome: won, .. } = resolution.state() else {
            return Err(Error::invalid(format!(
                "market {market_id} is {} and has no outcome to score against; wait for a final resolution",
                resolution.state().as_str()
            )));
        };
        Ok(Self {
            resolved_yes: won == &outcome,
            market_id,
            outcome,
            platform,
            market,
        })
    }

    pub const fn market_id(&self) -> &ObjectId {
        &self.market_id
    }

    pub const fn outcome(&self) -> &OutcomeId {
        &self.outcome
    }

    pub const fn platform(&self) -> &Stamped<Probability> {
        &self.platform
    }

    pub const fn market(&self) -> &Stamped<Probability> {
        &self.market
    }

    /// Whether the scored outcome is the one that won.
    pub const fn resolved_yes(&self) -> bool {
        self.resolved_yes
    }

    /// The outcome as the Brier score needs it: one if it happened, zero if
    /// it did not.
    fn realised(&self) -> f64 {
        if self.resolved_yes { 1.0 } else { 0.0 }
    }

    /// Squared error of the platform's probability against the outcome.
    ///
    /// This is where a `Decimal` probability becomes an `f64`: a Brier score
    /// is a statistic, not money, and its standard error needs a square root
    /// that `Decimal` does not have.
    pub fn platform_brier(&self) -> f64 {
        (self.platform.value().as_f64() - self.realised()).powi(2)
    }

    /// Squared error of the market's implied probability against the outcome.
    /// The same `Decimal` to `f64` crossing as [`Self::platform_brier`].
    pub fn market_brier(&self) -> f64 {
        (self.market.value().as_f64() - self.realised()).powi(2)
    }

    /// Platform minus market, for this contract. Negative is the platform
    /// being closer to what happened.
    pub fn brier_difference(&self) -> f64 {
        self.platform_brier() - self.market_brier()
    }
}

/// Mean squared error of the platform's probabilities over a resolved set.
///
/// Refuses an empty set: a mean of nothing is not zero, and zero is a perfect
/// score, so returning it would report a forecaster that has never forecast
/// as one that has never been wrong.
pub fn brier_score(forecasts: &[ScoredForecast]) -> Result<f64> {
    mean(
        forecasts.iter().map(ScoredForecast::platform_brier),
        forecasts.len(),
    )
}

/// Mean squared error of the market's implied probabilities over the same set,
/// with the same refusal as [`brier_score`].
pub fn market_brier_score(forecasts: &[ScoredForecast]) -> Result<f64> {
    mean(
        forecasts.iter().map(ScoredForecast::market_brier),
        forecasts.len(),
    )
}

fn mean(terms: impl Iterator<Item = f64>, count: usize) -> Result<f64> {
    if count == 0 {
        return Err(Error::invalid(
            "no resolved contracts to score; a Brier score over nothing is not zero",
        ));
    }
    // The count is small and exact; the widening is lossless far past any
    // number of contracts a desk will resolve.
    Ok(terms.sum::<f64>() / count as f64)
}

/// The platform scored against the market on the same contracts, with the
/// uncertainty of the difference.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrierComparison {
    /// Mean squared error of the platform's probabilities. Lower is better.
    pub platform: f64,
    /// Mean squared error of the market's implied probabilities on the same
    /// contracts at the same instants.
    pub market: f64,
    /// `platform - market`. Negative means the platform was closer to what
    /// happened; positive means the market was.
    pub difference: f64,
    /// Standard error of `difference`, from the per-contract differences
    /// paired by contract. Paired, because the two forecasters faced the same
    /// events: an easy set flatters both, and the pairing removes that shared
    /// term rather than counting it as disagreement.
    pub standard_error: f64,
    /// How many resolved contracts the comparison rests on.
    pub count: usize,
}

impl BrierComparison {
    /// The difference in units of its standard error. Negative favours the
    /// platform. Zero when the two forecasters agreed on every contract,
    /// because a difference of nothing needs no error bar to be believed.
    pub fn z_score(&self) -> f64 {
        if self.standard_error == 0.0 {
            0.0
        } else {
            self.difference / self.standard_error
        }
    }

    /// Whether the platform beat the market by more than `sigmas` standard
    /// errors. This is the gate's question, and the error bar is what keeps
    /// three lucky contracts from answering it.
    pub fn platform_beats_market_by(&self, sigmas: f64) -> bool {
        self.difference < 0.0 && self.difference.abs() > sigmas * self.standard_error
    }
}

/// Score both forecasters on the same resolved contracts and report the
/// difference with its paired standard error.
///
/// Refuses fewer than two contracts. The empty case has no score at all, and a
/// single contract has a difference but no variance to put an error bar on —
/// returning one with a standard error of zero would present one contract's
/// luck as a certainty.
pub fn compare(forecasts: &[ScoredForecast]) -> Result<BrierComparison> {
    let count = forecasts.len();
    if count < 2 {
        return Err(Error::invalid(format!(
            "a paired Brier comparison needs at least two resolved contracts and was given {count}; a difference over fewer has no standard error"
        )));
    }
    let platform = brier_score(forecasts)?;
    let market = market_brier_score(forecasts)?;
    let difference = platform - market;
    // The same exact widening as in `mean`.
    let n = count as f64;
    let variance = forecasts
        .iter()
        .map(|forecast| (forecast.brier_difference() - difference).powi(2))
        .sum::<f64>()
        / (n - 1.0);
    let standard_error = (variance / n).sqrt();
    Ok(BrierComparison {
        platform,
        market,
        difference,
        standard_error,
        count,
    })
}
