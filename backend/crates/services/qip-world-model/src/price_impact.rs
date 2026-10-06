//! Price impact model: how market depth affects execution price.
//!
//! The impact model quantifies how executed size affects the realized price
//! relative to the mid-price at order entry, a key input to execution logic.

use qip_contracts::venue::VenueId;
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum historical samples for impact calculation
const IMPACT_HISTORY_LIMIT: usize = 500;

/// One observation of realized impact
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImpactObservation {
    pub object_id: ObjectId,
    pub venue: VenueId,
    pub observed_at: Timestamp,
    /// Size executed
    pub size: Decimal,
    /// Mid-price at order entry
    pub entry_mid: Decimal,
    /// Realized execution price
    pub realized_price: Decimal,
    /// Whether this was a buy (true) or sell (false)
    pub is_buy: bool,
}

impl ImpactObservation {
    /// Impact in basis points
    pub fn impact_bps(&self) -> f64 {
        let expected = self.entry_mid.to_f64();
        let realized = self.realized_price.to_f64();
        if self.is_buy {
            // Buy impact is positive (paid more)
            ((realized - expected) / expected) * 10000.0
        } else {
            // Sell impact is negative (received less)
            ((expected - realized) / expected) * 10000.0
        }
    }

    /// Impact per unit of size
    pub fn impact_per_unit(&self) -> f64 {
        let impact = self.impact_bps().abs();
        impact / self.size.to_f64()
    }
}

/// Impact model for one venue
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueImpactModel {
    pub venue: VenueId,
    /// Linear impact coefficient: impact = coeff * sqrt(size / depth)
    pub impact_coefficient: f64,
    /// Exponent for size dependence (typically 0.5 to 1.0)
    pub size_exponent: f64,
    /// Confidence in this model (0-1)
    pub confidence: f64,
}

/// Impact estimates for an instrument at one instant
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImpactMap {
    pub valid_at: Timestamp,
    pub known_at: Timestamp,
    pub models: BTreeMap<VenueId, VenueImpactModel>,
}

/// Impact state tracker for one instrument
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImpactState {
    /// Historical observations per venue (bounded)
    history: BTreeMap<VenueId, Vec<ImpactObservation>>,
    /// Fitted models per venue
    models: BTreeMap<VenueId, VenueImpactModel>,
}

impl Default for ImpactState {
    fn default() -> Self {
        Self::new()
    }
}

impl ImpactState {
    pub fn new() -> Self {
        Self {
            history: BTreeMap::new(),
            models: BTreeMap::new(),
        }
    }

    /// Record an impact observation
    pub fn absorb(&mut self, observation: ImpactObservation) {
        let venue_history = self
            .history
            .entry(observation.venue.clone())
            .or_insert_with(Vec::new);
        venue_history.push(observation.clone());
        if venue_history.len() > IMPACT_HISTORY_LIMIT {
            venue_history.remove(0);
        }

        // Update model for this venue
        self.fit_model(observation.venue);
    }

    /// Fit impact model from historical observations
    fn fit_model(&mut self, venue: VenueId) {
        if let Some(observations) = self.history.get(&venue) {
            if observations.len() < 10 {
                return; // Not enough data
            }

            // Simple linear fit: mean impact coefficient
            let impacts: Vec<f64> = observations.iter().map(|o| o.impact_per_unit()).collect();
            let mean_impact = impacts.iter().sum::<f64>() / impacts.len() as f64;

            // Confidence based on variance and sample size
            let variance = impacts
                .iter()
                .map(|x| (x - mean_impact).powi(2))
                .sum::<f64>()
                / impacts.len() as f64;
            let std_dev = variance.sqrt();
            let coefficient_of_variation = if mean_impact != 0.0 {
                std_dev / mean_impact
            } else {
                1.0
            };

            let confidence = 1.0 / (1.0 + coefficient_of_variation);

            self.models.insert(
                venue.clone(),
                VenueImpactModel {
                    venue,
                    impact_coefficient: mean_impact,
                    size_exponent: 0.5, // Standard assumption
                    confidence: confidence.min(0.95),
                },
            );
        }
    }

    /// Estimate impact for a hypothetical trade
    pub fn estimate_impact(&self, venue: VenueId, size: Decimal) -> Option<f64> {
        self.models
            .get(&venue)
            .map(|model| model.impact_coefficient * size.to_f64().powf(model.size_exponent))
    }

    /// Get current impact map
    pub fn impact_map(&self, valid_at: Timestamp, known_at: Timestamp) -> Option<ImpactMap> {
        if self.models.is_empty() {
            return None;
        }

        Some(ImpactMap {
            valid_at,
            known_at,
            models: self.models.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_impact_bps_calculation() {
        let obs = ImpactObservation {
            object_id: ObjectId::from_string("test-impact-1"),
            venue: VenueId::new("NYSE"),
            observed_at: Timestamp::from_secs(0),
            size: Decimal::from(100),
            entry_mid: Decimal::parse("100.00").unwrap(),
            realized_price: Decimal::parse("100.50").unwrap(),
            is_buy: true,
        };
        let impact = obs.impact_bps();
        assert!((impact - 50.0).abs() < 0.1);
    }

    #[test]
    fn test_impact_state_absorbs_observations() {
        let mut state = ImpactState::new();
        for i in 0..15 {
            let obs = ImpactObservation {
                object_id: ObjectId::from_string(format!("test-impact-{}", i)),
                venue: VenueId::new("NYSE"),
                observed_at: Timestamp::from_secs(0),
                size: Decimal::from(100 + i),
                entry_mid: Decimal::parse("100.00").unwrap(),
                realized_price: Decimal::parse("100.50").unwrap(),
                is_buy: true,
            };
            state.absorb(obs);
        }
        let model = state.models.get(&VenueId::new("NYSE"));
        assert!(model.is_some());
    }

    #[test]
    fn test_impact_estimate() {
        let mut state = ImpactState::new();
        for i in 0..20 {
            let obs = ImpactObservation {
                object_id: ObjectId::from_string(format!("test-impact-{}", i)),
                venue: VenueId::new("NASDAQ"),
                observed_at: Timestamp::from_secs(0),
                size: Decimal::from(1000),
                entry_mid: Decimal::parse("50.00").unwrap(),
                realized_price: Decimal::parse("50.25").unwrap(),
                is_buy: true,
            };
            state.absorb(obs);
        }
        let estimate = state.estimate_impact(VenueId::new("NASDAQ"), Decimal::from(500));
        assert!(estimate.is_some());
    }
}
