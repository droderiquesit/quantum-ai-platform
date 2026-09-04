//! `qip-prediction` — prediction and event markets.
//!
//! These markets look like the easiest instruments the platform trades: a
//! contract that pays one unit if a thing happens, nothing if it does not, and
//! a price that reads directly as a probability. Every part of that summary is
//! a trap, and the crate is organised around the four that cost money.
//!
//! * **The proposition is the instrument.** Resolution criteria are structured
//!   data ([`ResolutionCriteria`]) evaluated against named observations from a
//!   named source, not a sentence. Two markets are the same proposition when
//!   their criteria hash to the same digest, which is why
//!   [`CrossMarketPair::new`] can refuse a spread between two markets that
//!   merely sound alike.
//! * **The price is not the probability.** [`implied_from_ask`] and
//!   [`implied_from_bid`] back out the fee and the spread and return a band;
//!   its width is how much better than the market a forecast has to be before
//!   it is worth anything.
//! * **Resolved is not settled.** An oracle can be late, under-confident, or
//!   wrong, and a resolution inside its dispute window is a claim rather than a
//!   payment. [`MarketResolution::settle`] refuses everything that is not
//!   final, because settling on a disputed resolution is how a position bought
//!   at 0.97 loses a whole unit.
//! * **An arbitrage is a size, not a price.** A complete set costing less than
//!   its payoff is a locked profit only for as much of it as the book holds,
//!   so [`set_arbitrage`] walks the depth and reports the quantity it can
//!   actually fill.
//! * **Better than the market is a paired measurement, not a feeling.**
//!   [`compare`] Brier-scores the platform's probability and the venue's
//!   implied one on the same resolved contracts and reports the difference
//!   with its standard error; [`ScoredForecast::new`] refuses a market quote
//!   known later than the platform's probability, because scoring against a
//!   quote the platform could not have seen is leakage dressed as skill.

pub mod adapter;
pub mod arbitrage;
pub mod cross;
pub mod market;
pub mod oracle;
pub mod pricing;
pub mod resolution;
pub mod scoring;

pub use adapter::{
    PredictionAdapter, PredictionSource, PredictionUpdate, SyntheticPredictionVenue,
    SyntheticVenueConfig, VenueApiAdapter, VenueApiConfig, demo_market,
};
pub use arbitrage::{SetArbitrage, SetArbitrageKind, implied_sum, set_arbitrage};
pub use cross::{CrossMarketPair, CrossVenueArbitrage};
pub use market::{
    EventMarket, FeeSchedule, MarketKind, MarketVerdict, Outcome, OutcomeId, ScalarBucket,
};
pub use oracle::{
    Dispute, MarketResolution, Oracle, OracleIdentity, OracleKind, OracleReport, ResolutionState,
    ScriptedOracle, Settlement,
};
pub use pricing::{
    Probability, ProbabilityBand, SumDeviation, implied_band, implied_from_ask, implied_from_bid,
    naive_probability,
};
pub use resolution::{
    Comparison, Observation, Observations, Proposition, PropositionDifference, ResolutionCriteria,
    ResolutionSource, SettlementRule, SourceKind, UndeterminedRule, Verdict,
};
pub use scoring::{BrierComparison, ScoredForecast, brier_score, compare, market_brier_score};
