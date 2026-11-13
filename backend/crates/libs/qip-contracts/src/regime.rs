//! The market regime a decision is made under (ADR 0464).
//!
//! It lived in `qip-cost-router::context`, and the confidence governor reads
//! it to discount a calibration graded in other regimes. The router's
//! closure holds `qip-agents` and `qip-ai`, so the governor naming the type
//! through the router put an alpha crate in the closure of the code that
//! narrows capital grants. A regime is a measurement label, not a model:
//! it belongs with the other contracts between brains.

use serde::{Deserialize, Serialize};

/// What the market is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketRegime {
    /// Prices are going somewhere and continuing to.
    Trending,
    /// Prices come back. Most of the time, until they do not.
    MeanReverting,
    /// Correlations go to one and liquidity goes to nothing.
    Crisis,
    /// The book is thin enough that the price is an opinion.
    Illiquid,
    /// Nothing is happening, which is its own regime and not the absence of
    /// one: a model calibrated in quiet markets is a model with no experience
    /// of the day that matters.
    Quiet,
}

impl MarketRegime {
    pub const ALL: [Self; 5] = [
        Self::Trending,
        Self::MeanReverting,
        Self::Crisis,
        Self::Illiquid,
        Self::Quiet,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Trending => "trending",
            Self::MeanReverting => "mean_reverting",
            Self::Crisis => "crisis",
            Self::Illiquid => "illiquid",
            Self::Quiet => "quiet",
        }
    }
}
