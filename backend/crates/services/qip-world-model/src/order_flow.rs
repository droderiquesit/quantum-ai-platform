//! Order flow tracking: imbalances and directional biases in market activity.
//!
//! Order flow is the aggregate of all buy and sell orders reaching the market.
//! Imbalances (where buy flow exceeds sell flow or vice versa) are predictive of
//! short-term price movement and are measured separately from trade counts to
//! account for size bias.

use qip_contracts::venue::VenueId;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum number of order flow observations per venue per instrument.
const FLOW_HISTORY_PER_VENUE: usize = 256;

/// One venue's order flow imbalance at one instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlowObservation {
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// Timestamp when the flow was observed
    pub observed_at: Timestamp,
    /// Buy-initiated volume
    pub buy_volume: Decimal,
    /// Sell-initiated volume
    pub sell_volume: Decimal,
    /// Buy count (number of buy orders/trades)
    pub buy_count: u64,
    /// Sell count (number of sell orders/trades)
    pub sell_count: u64,
}

impl FlowObservation {
    /// Imbalance ratio: (buy - sell) / (buy + sell). Range: [-1, 1].
    pub fn imbalance_ratio(&self) -> f64 {
        let buy = self.buy_volume.to_f64();
        let sell = self.sell_volume.to_f64();
        let total = buy + sell;
        if total == 0.0 {
            0.0
        } else {
            (buy - sell) / total
        }
    }

    /// Volume-weighted directional bias
    pub fn flow_direction(&self) -> FlowDirection {
        let ratio = self.imbalance_ratio();
        if ratio > 0.1 {
            FlowDirection::BuyBiased
        } else if ratio < -0.1 {
            FlowDirection::SellBiased
        } else {
            FlowDirection::Balanced
        }
    }
}

/// Direction of order flow imbalance
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowDirection {
    BuyBiased,
    SellBiased,
    Balanced,
}

/// One venue's latest order flow state
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueFlow {
    pub venue: VenueId,
    pub observation: FlowObservation,
    /// Age of this observation
    pub staleness: Duration,
}

/// Aggregate order flow for an instrument across venues
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlowMap {
    pub valid_at: Timestamp,
    pub known_at: Timestamp,
    pub flows: BTreeMap<VenueId, VenueFlow>,
}

impl FlowMap {
    /// Aggregate imbalance across all venues
    pub fn aggregate_imbalance(&self) -> f64 {
        let total_buy: Decimal = self.flows.values().map(|f| f.observation.buy_volume).sum();
        let total_sell: Decimal = self.flows.values().map(|f| f.observation.sell_volume).sum();
        let buy = total_buy.to_f64();
        let sell = total_sell.to_f64();
        let total = buy + sell;
        if total == 0.0 {
            0.0
        } else {
            (buy - sell) / total
        }
    }

    /// Count venues with buy bias
    pub fn venues_buy_biased(&self) -> usize {
        self.flows
            .values()
            .filter(|f| f.observation.flow_direction() == FlowDirection::BuyBiased)
            .count()
    }

    /// Count venues with sell bias
    pub fn venues_sell_biased(&self) -> usize {
        self.flows
            .values()
            .filter(|f| f.observation.flow_direction() == FlowDirection::SellBiased)
            .count()
    }
}

/// Order flow state tracker for one instrument
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderFlowState {
    /// History per venue, bounded to prevent unbounded growth
    history: BTreeMap<VenueId, Vec<FlowObservation>>,
}

impl Default for OrderFlowState {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderFlowState {
    pub fn new() -> Self {
        Self {
            history: BTreeMap::new(),
        }
    }

    /// Absorb a new flow observation
    pub fn absorb(&mut self, observation: FlowObservation) {
        let venues = self.history.entry(observation.venue.clone()).or_default();
        venues.push(observation);
        if venues.len() > FLOW_HISTORY_PER_VENUE {
            venues.remove(0);
        }
    }

    /// Get current flow map
    pub fn flow_map(&self, valid_at: Timestamp, known_at: Timestamp) -> Option<FlowMap> {
        if self.history.is_empty() {
            return None;
        }

        let mut flows = BTreeMap::new();
        for (venue_id, venue_history) in &self.history {
            if let Some(latest) = venue_history.last() {
                let staleness = valid_at.since(latest.observed_at);
                flows.insert(
                    venue_id.clone(),
                    VenueFlow {
                        venue: venue_id.clone(),
                        observation: latest.clone(),
                        staleness,
                    },
                );
            }
        }

        if flows.is_empty() {
            None
        } else {
            Some(FlowMap {
                valid_at,
                known_at,
                flows,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_imbalance_ratio_calculation() {
        let obs = FlowObservation {
            object_id: ObjectId::from_string("flow-obs-1"),
            venue: VenueId::new("NASDAQ"),
            observed_at: Timestamp::from_secs(0),
            buy_volume: Decimal::from(100),
            sell_volume: Decimal::from(50),
            buy_count: 20,
            sell_count: 10,
        };
        let ratio = obs.imbalance_ratio();
        assert!((ratio - (1.0 / 3.0)).abs() < 0.001);
    }

    #[test]
    fn test_flow_direction_classification() {
        let buy_biased = FlowObservation {
            object_id: ObjectId::from_string("flow-obs-2"),
            venue: VenueId::new("NYSE"),
            observed_at: Timestamp::from_secs(0),
            buy_volume: Decimal::from(100),
            sell_volume: Decimal::from(20),
            buy_count: 50,
            sell_count: 10,
        };
        assert_eq!(buy_biased.flow_direction(), FlowDirection::BuyBiased);

        let balanced = FlowObservation {
            object_id: ObjectId::from_string("flow-obs-3"),
            venue: VenueId::new("NYSE"),
            observed_at: Timestamp::from_secs(0),
            buy_volume: Decimal::from(100),
            sell_volume: Decimal::from(95),
            buy_count: 50,
            sell_count: 48,
        };
        assert_eq!(balanced.flow_direction(), FlowDirection::Balanced);
    }

    #[test]
    fn test_order_flow_state_absorbs_observations() {
        let mut state = OrderFlowState::new();
        let obs = FlowObservation {
            object_id: ObjectId::from_string("flow-obs-4"),
            venue: VenueId::new("NASDAQ"),
            observed_at: Timestamp::from_secs(0),
            buy_volume: Decimal::from(100),
            sell_volume: Decimal::from(50),
            buy_count: 20,
            sell_count: 10,
        };
        state.absorb(obs.clone());
        let map = state.flow_map(Timestamp::from_secs(0), Timestamp::from_secs(0));
        assert!(map.is_some());
        assert_eq!(map.unwrap().flows.len(), 1);
    }
}
