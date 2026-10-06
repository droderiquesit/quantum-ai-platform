//! Arbitrage opportunity tracking: cross-venue price discrepancies.
//!
//! Tracks mispricings between venues that represent potential arbitrage
//! opportunities and captures when those opportunities are transient vs
//! structural.

use qip_contracts::venue::VenueId;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Spread threshold (bps) above which we track an opportunity
const MIN_SPREAD_BPS: f64 = 2.0;

/// Maximum number of opportunities to track
const MAX_OPPORTUNITIES: usize = 1000;

/// One arbitrage opportunity
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArbitrageOpportunity {
    pub object_id: ObjectId,
    pub observed_at: Timestamp,
    /// Venue with better bid (buy opportunity)
    pub bid_venue: VenueId,
    /// Venue with better ask (sell opportunity)
    pub ask_venue: VenueId,
    /// Bid price
    pub bid_price: Decimal,
    /// Ask price
    pub ask_price: Decimal,
    /// Spread in basis points
    pub spread_bps: f64,
    /// Estimated profit per unit (before costs)
    pub profit_per_unit: Decimal,
}

impl ArbitrageOpportunity {
    /// Calculate profit as % of mid
    pub fn profit_percentage(&self) -> f64 {
        let mid = (self.bid_price.to_f64() + self.ask_price.to_f64()) / 2.0;
        if mid == 0.0 {
            0.0
        } else {
            (self.profit_per_unit.to_f64() / mid) * 100.0
        }
    }
}

/// Arbitrage opportunity classification
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpportunityClass {
    Transient,  // Appears and disappears quickly
    Structural, // Persistent across multiple observations
    Fleeting,   // Exists for only one observation
}

/// Tracked opportunity with history
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackedOpportunity {
    pub opportunity: ArbitrageOpportunity,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
    pub observation_count: u32,
    pub class: OpportunityClass,
}

impl TrackedOpportunity {
    /// Duration this opportunity has persisted
    pub fn persistence(&self) -> Duration {
        self.last_seen.since(self.first_seen)
    }

    /// Update classification based on persistence
    fn update_class(&mut self) {
        let duration_ms = self.persistence().as_millis();
        self.class = if duration_ms < 1000 {
            OpportunityClass::Fleeting
        } else if duration_ms < 10000 {
            OpportunityClass::Transient
        } else {
            OpportunityClass::Structural
        };
    }
}

/// Arbitrage state for one instrument
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArbitrageState {
    /// Active opportunities being tracked (bounded)
    opportunities: BTreeMap<(VenueId, VenueId), TrackedOpportunity>,
    /// Statistics
    total_observed: u64,
    largest_opportunity: Option<ArbitrageOpportunity>,
}

impl Default for ArbitrageState {
    fn default() -> Self {
        Self::new()
    }
}

impl ArbitrageState {
    pub fn new() -> Self {
        Self {
            opportunities: BTreeMap::new(),
            total_observed: 0,
            largest_opportunity: None,
        }
    }

    /// Record an arbitrage observation
    pub fn observe_prices(
        &mut self,
        bid_venue: VenueId,
        ask_venue: VenueId,
        bid_price: Decimal,
        ask_price: Decimal,
    ) {
        // Only track if profitable
        if bid_price <= ask_price {
            return; // No arbitrage if bids <= asks
        }

        let profit_per_unit = bid_price - ask_price;
        let spread_bps = ((bid_price.to_f64() - ask_price.to_f64())
            / ((bid_price.to_f64() + ask_price.to_f64()) / 2.0))
            * -10000.0;

        if spread_bps.abs() < MIN_SPREAD_BPS {
            return; // Below threshold
        }

        let opportunity = ArbitrageOpportunity {
            object_id: ObjectId::from_string(format!("arb-{}-{}", bid_venue, ask_venue)),
            observed_at: Timestamp::from_secs(0),
            bid_venue: bid_venue.clone(),
            ask_venue: ask_venue.clone(),
            bid_price,
            ask_price,
            spread_bps: spread_bps.abs(),
            profit_per_unit,
        };

        self.total_observed += 1;

        // Track largest
        if let Some(ref largest) = self.largest_opportunity {
            if opportunity.spread_bps > largest.spread_bps {
                self.largest_opportunity = Some(opportunity.clone());
            }
        } else {
            self.largest_opportunity = Some(opportunity.clone());
        }

        // Update or create tracked opportunity
        let key = (bid_venue.clone(), ask_venue.clone());
        match self.opportunities.get_mut(&key) {
            Some(tracked) => {
                tracked.opportunity = opportunity;
                tracked.last_seen = Timestamp::from_secs(0);
                tracked.observation_count += 1;
                tracked.update_class();
            }
            None => {
                if self.opportunities.len() < MAX_OPPORTUNITIES {
                    let now = Timestamp::from_secs(0);
                    let mut tracked = TrackedOpportunity {
                        opportunity,
                        first_seen: now,
                        last_seen: now,
                        observation_count: 1,
                        class: OpportunityClass::Fleeting,
                    };
                    tracked.update_class();
                    self.opportunities.insert(key, tracked);
                }
            }
        }
    }

    /// Get all current opportunities
    pub fn current_opportunities(&self) -> Vec<TrackedOpportunity> {
        self.opportunities.values().cloned().collect()
    }

    /// Count structural opportunities
    pub fn structural_count(&self) -> usize {
        self.opportunities
            .values()
            .filter(|o| o.class == OpportunityClass::Structural)
            .count()
    }

    /// Count transient opportunities
    pub fn transient_count(&self) -> usize {
        self.opportunities
            .values()
            .filter(|o| o.class == OpportunityClass::Transient)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arbitrage_profit_detection() {
        let arb = ArbitrageOpportunity {
            object_id: ObjectId::from_string("test-arb-1"),
            observed_at: Timestamp::from_secs(0),
            bid_venue: VenueId::new("NYSE"),
            ask_venue: VenueId::new("NASDAQ"),
            bid_price: Decimal::parse("100.10").unwrap(),
            ask_price: Decimal::parse("100.00").unwrap(),
            spread_bps: 10.0,
            profit_per_unit: Decimal::parse("0.10").unwrap(),
        };
        assert!(arb.profit_percentage() > 0.0);
    }

    #[test]
    fn test_arbitrage_state_tracking() {
        let mut state = ArbitrageState::new();
        state.observe_prices(
            VenueId::new("NYSE"),
            VenueId::new("NASDAQ"),
            Decimal::parse("100.10").unwrap(),
            Decimal::parse("100.00").unwrap(),
        );
        assert_eq!(state.current_opportunities().len(), 1);
        assert!(state.largest_opportunity.is_some());
    }

    #[test]
    fn test_ignores_unprofitable_spreads() {
        let mut state = ArbitrageState::new();
        state.observe_prices(
            VenueId::new("NYSE"),
            VenueId::new("NASDAQ"),
            Decimal::parse("100.00").unwrap(),
            Decimal::parse("100.10").unwrap(),
        );
        assert_eq!(state.current_opportunities().len(), 0);
    }
}
