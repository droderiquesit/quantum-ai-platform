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

    /// Check if a residual value exceeds the threshold and should trigger
    /// a campaign. Returns true only when residual > threshold.
    pub fn should_trigger(&self) -> bool {
        match self.residual_value {
            Some(residual) => {
                // Compare absolute values; a negative residual at the same
                // magnitude as a positive one matters equally.
                residual.abs() > self.threshold
            }
            None => false, // New venue campaigns trigger unconditionally
        }
    }
}

/// Detects when to create tick-replay campaigns from residual anomalies.
#[derive(Debug, Clone)]
pub struct TickCampaignDetector {
    /// Threshold for slippage residuals as a fraction (e.g., 0.01 = 1%).
    pub slippage_threshold: Decimal,
    /// Known venues to track for new ones appearing.
    pub known_venues: BTreeSet<String>,
}

impl TickCampaignDetector {
    /// Create a detector with default thresholds.
    pub fn new() -> qip_core::error::Result<Self> {
        Ok(Self {
            // Default: 1% slippage triggers a campaign
            slippage_threshold: Decimal::from_scaled(1, 2).ok_or_else(|| {
                qip_core::error::Error::numeric("failed to construct default slippage threshold")
            })?,
            known_venues: BTreeSet::new(),
        })
    }

    /// Detect if a new venue should trigger a campaign. Returns the venue
    /// name if it's new, or None if it's already known.
    pub fn detect_new_venue(&mut self, venue: &str) -> Option<String> {
        if !self.known_venues.contains(venue) {
            self.known_venues.insert(venue.to_string());
            Some(venue.to_string())
        } else {
            None
        }
    }

    /// Detect if slippage residual exceeds threshold. Returns the campaign
    /// if triggered, or None if below threshold.
    pub fn detect_slippage(
        &self,
        residual: Decimal,
        venue: Option<&str>,
        instruments: BTreeSet<String>,
    ) -> Option<TickCampaign> {
        if residual.abs() > self.slippage_threshold {
            let campaign = TickCampaign::for_residual(
                format!("tick_{}", uuid_stub()),
                TickCampaignTrigger::UnexplainedSlippage,
                instruments,
                residual,
                self.slippage_threshold,
                venue.map(|s| s.to_string()),
            );
            Some(campaign)
        } else {
            None
        }
    }
}

/// Generate a stub UUID for testing (not for production use).
fn uuid_stub() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:08x}", nanos)
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

    #[test]
    fn a_residual_below_threshold_does_not_trigger_a_campaign() {
        let instruments = vec!["BTC-USD".to_string()].into_iter().collect();
        let residual = Decimal::from_scaled(50, 4).unwrap(); // 0.005 = 0.5%
        let threshold = Decimal::from_scaled(1, 2).unwrap(); // 0.01 = 1%
        let campaign = TickCampaign::for_residual(
            "tick_below".to_string(),
            TickCampaignTrigger::UnexplainedSlippage,
            instruments,
            residual,
            threshold,
            Some("KRAKEN".to_string()),
        );

        assert!(!campaign.should_trigger());
    }

    #[test]
    fn a_residual_above_threshold_triggers_a_campaign() {
        let instruments = vec!["ETH-USD".to_string()].into_iter().collect();
        let residual = Decimal::from_scaled(15, 3).unwrap(); // 0.015 = 1.5%
        let threshold = Decimal::from_scaled(1, 2).unwrap(); // 0.01 = 1%
        let campaign = TickCampaign::for_residual(
            "tick_above".to_string(),
            TickCampaignTrigger::UnexplainedSlippage,
            instruments,
            residual,
            threshold,
            Some("BINANCE".to_string()),
        );

        assert!(campaign.should_trigger());
    }

    #[test]
    fn detector_discovers_new_venues_and_records_them() {
        let mut detector = TickCampaignDetector::new().expect("detector must construct");

        // First venue: should be detected as new
        let venue1_new = detector.detect_new_venue("NYSE");
        assert_eq!(venue1_new, Some("NYSE".to_string()));

        // Same venue again: not new anymore
        let venue1_again = detector.detect_new_venue("NYSE");
        assert_eq!(venue1_again, None);

        // Different venue: should be new
        let venue2_new = detector.detect_new_venue("NASDAQ");
        assert_eq!(venue2_new, Some("NASDAQ".to_string()));
    }

    #[test]
    fn detector_triggers_campaigns_when_slippage_exceeds_threshold() {
        let detector = TickCampaignDetector::new().expect("detector must construct");
        let instruments: BTreeSet<String> = vec!["AAPL".to_string()].into_iter().collect();

        // Below threshold: no campaign
        let below = Decimal::from_scaled(50, 4).unwrap(); // 0.5%
        let campaign_below = detector.detect_slippage(below, Some("NYSE"), instruments.clone());
        assert!(campaign_below.is_none());

        // Above threshold: campaign triggered
        let above = Decimal::from_scaled(2, 2).unwrap(); // 2%
        let campaign_above = detector.detect_slippage(above, Some("NYSE"), instruments);
        assert!(campaign_above.is_some());
        let c = campaign_above.unwrap();
        assert_eq!(c.trigger, TickCampaignTrigger::UnexplainedSlippage);
        assert_eq!(c.venue, Some("NYSE".to_string()));
    }

    #[test]
    fn negative_residuals_trigger_campaigns_at_the_same_threshold() {
        let detector = TickCampaignDetector::new().expect("detector must construct");
        let instruments = vec!["GOOG".to_string()].into_iter().collect();

        // Negative residual above threshold magnitude
        let negative = Decimal::from_scaled(-2, 2).unwrap(); // -2%
        let campaign = detector.detect_slippage(negative, Some("NYSE"), instruments);
        assert!(
            campaign.is_some(),
            "negative residuals should trigger at the same threshold"
        );
        let c = campaign.unwrap();
        assert_eq!(c.residual_value, Some(negative));
    }
}
