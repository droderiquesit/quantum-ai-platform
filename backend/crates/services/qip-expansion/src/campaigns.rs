//! Step 1 of a tick-replay campaign: detect when microstructure residuals grow
//! past a threshold or a new venue appears, and create a TickCampaign to study
//! the anomaly (EXPAND-017).

use qip_core::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A campaign to replay and study tick-level microstructure after a residual
/// anomaly or new venue discovery.
///
/// Residuals that trigger campaigns: unexplained slippage, fill/queue
/// prediction errors, new venue mechanics, abnormal lead/lag, world-event
/// reaction residuals, and divergence between live (paper) and replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickCampaign {
    /// A unique identifier for this campaign.
    pub campaign_id: String,
    /// The reason this campaign was created.
    pub trigger: TickCampaignTrigger,
    /// Instruments affected by the residual or new venue.
    pub affected_instruments: BTreeSet<String>,
    /// The venue affected, if venue-specific.
    pub venue: Option<String>,
    /// The observed residual value that triggered the campaign.
    pub residual_value: Option<Decimal>,
    /// The threshold that was exceeded.
    pub threshold: Decimal,
}

/// The reason a TickCampaign was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TickCampaignTrigger {
    /// Unexplained slippage exceeds the threshold.
    UnexplainedSlippage,
    /// Fill prediction error exceeds the threshold.
    FillPredictionError,
    /// Queue prediction error exceeds the threshold.
    QueuePredictionError,
    /// Lead/lag relationship anomaly detected.
    AbnormalLeadLag,
    /// World event reaction residuals grow.
    WorldEventReaction,
    /// Divergence between paper and replay fills.
    PaperReplayDivergence,
    /// A new venue was discovered.
    NewVenue,
}

impl TickCampaign {
    /// Create a new tick campaign for a residual anomaly.
    pub fn for_residual(
        campaign_id: String,
        trigger: TickCampaignTrigger,
        instruments: BTreeSet<String>,
        residual_value: Decimal,
        threshold: Decimal,
        venue: Option<String>,
    ) -> Self {
        Self {
            campaign_id,
            trigger,
            affected_instruments: instruments,
            venue,
            residual_value: Some(residual_value),
            threshold,
        }
    }

    /// Create a new tick campaign for a new venue discovery.
    pub fn for_new_venue(
        campaign_id: String,
        venue: String,
        instruments: BTreeSet<String>,
    ) -> Self {
        Self {
            campaign_id,
            trigger: TickCampaignTrigger::NewVenue,
            affected_instruments: instruments,
            venue: Some(venue),
            residual_value: None,
            threshold: Decimal::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slippage_campaign_names_its_residual_and_threshold() {
        let instruments = vec!["AAPL".to_string(), "GOOG".to_string()]
            .into_iter()
            .collect();
        let residual = Decimal::from_scaled(50, 4).unwrap(); // 0.005 = 0.5 bps
        let threshold = Decimal::from_scaled(100, 4).unwrap(); // 0.01 = 1 bps
        let campaign = TickCampaign::for_residual(
            "tick_001".to_string(),
            TickCampaignTrigger::UnexplainedSlippage,
            instruments,
            residual,
            threshold,
            Some("NYSE".to_string()),
        );

        assert_eq!(campaign.campaign_id, "tick_001");
        assert_eq!(campaign.trigger, TickCampaignTrigger::UnexplainedSlippage);
        assert_eq!(campaign.residual_value, Some(residual));
        assert_eq!(campaign.threshold, threshold);
        assert_eq!(campaign.venue, Some("NYSE".to_string()));
        assert_eq!(campaign.affected_instruments.len(), 2);
    }

    #[test]
    fn a_new_venue_campaign_names_the_venue_and_instruments() {
        let instruments = vec!["BTC-USD".to_string()].into_iter().collect();
        let campaign =
            TickCampaign::for_new_venue("tick_002".to_string(), "KRAKEN".to_string(), instruments);

        assert_eq!(campaign.campaign_id, "tick_002");
        assert_eq!(campaign.trigger, TickCampaignTrigger::NewVenue);
        assert_eq!(campaign.venue, Some("KRAKEN".to_string()));
        assert!(campaign.residual_value.is_none());
        assert_eq!(campaign.affected_instruments.len(), 1);
        assert!(campaign.affected_instruments.contains("BTC-USD"));
    }

    #[test]
    fn tick_campaign_trigger_variants_are_distinct() {
        let variants = vec![
            TickCampaignTrigger::UnexplainedSlippage,
            TickCampaignTrigger::FillPredictionError,
            TickCampaignTrigger::QueuePredictionError,
            TickCampaignTrigger::AbnormalLeadLag,
            TickCampaignTrigger::WorldEventReaction,
            TickCampaignTrigger::PaperReplayDivergence,
            TickCampaignTrigger::NewVenue,
        ];

        // Ensure all variants are distinct by comparing to themselves and others
        for (i, v1) in variants.iter().enumerate() {
            for (j, v2) in variants.iter().enumerate() {
                if i == j {
                    assert_eq!(v1, v2, "trigger variant should equal itself");
                } else {
                    assert_ne!(v1, v2, "different trigger variants should not be equal");
                }
            }
        }
    }
}
