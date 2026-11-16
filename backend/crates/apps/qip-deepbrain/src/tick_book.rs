//! The platform's book as the tick desk's future tree reads it
//! (R12-TICK-10121).
//!
//! `FutureTree::portfolio_outcomes` turns a branch's per-instrument impact
//! into a move of the whole portfolio, and for a long time nothing but its own
//! test called it: the desk grew worlds, and no world was ever priced against
//! what the platform actually held, so a forecast could not inform sizing.
//! This module is the seam that supplies the positions.
//!
//! The book holds signed quantity at average cost, because the platform holds
//! no marks (`Platform::equity` says so). A weight here is therefore
//! `quantity * average_cost / equity`, signed, a fraction of net asset value at
//! cost. That is the honest figure available, and it is stated on the record
//! so nobody reads it as a mark-to-market exposure.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use qip_twin::futures::PortfolioOutcome;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Signed weights of the open book, as a fraction of equity.
///
/// Refuses non-positive equity rather than dividing by it or flooring it: a
/// book whose equity is gone has no meaningful weight, and a clamped figure
/// would size the next round against a portfolio that does not exist. A flat
/// book is not an error; it has no positions and so no outcome.
pub fn book_weights(
    positions: &BTreeMap<String, (Decimal, Decimal)>,
    equity: Decimal,
) -> Result<BTreeMap<String, f64>> {
    if positions.is_empty() {
        return Ok(BTreeMap::new());
    }
    if !equity.is_positive() {
        return Err(Error::invalid(format!(
            "the book has {} open position(s) but equity {equity}, so no weight can be stated; \
             a portfolio outcome is not computed against an exhausted book",
            positions.len()
        )));
    }
    let mut weights = BTreeMap::new();
    for (instrument, (quantity, average_cost)) in positions {
        if quantity.is_zero() {
            continue;
        }
        let notional = quantity
            .checked_mul(*average_cost)
            .ok_or_else(|| Error::numeric(format!("position {instrument} notional overflows")))?;
        let weight = notional
            .checked_div(equity)
            .ok_or_else(|| Error::numeric(format!("position {instrument} weight overflows")))?;
        // The money-to-statistic crossing: a weight is a ratio fed to the
        // twin's f64 arithmetic, and no amount crosses back.
        weights.insert(instrument.clone(), weight.to_f64());
    }
    Ok(weights)
}

/// What one round stated about the book, written beside the world branches so
/// the distribution can be reproduced from the output store alone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PortfolioOutcomeRecord {
    pub manifest: String,
    pub as_of_nanos: i64,
    /// The weights the outcomes were computed from.
    pub positions: BTreeMap<String, f64>,
    pub outcomes: Vec<PortfolioOutcome>,
}

impl PortfolioOutcomeRecord {
    pub fn new(
        manifest: &str,
        as_of: Timestamp,
        positions: &BTreeMap<String, f64>,
        outcomes: Vec<PortfolioOutcome>,
    ) -> Self {
        Self {
            manifest: manifest.to_string(),
            as_of_nanos: as_of.as_nanos(),
            positions: positions.clone(),
            outcomes,
        }
    }
}
