//! Step 1 of a tick-replay campaign: notice that a microstructure residual has
//! grown past its threshold, or that a venue nobody has seen before has
//! appeared, and open a [`TickCampaign`] scoped to what was affected
//! (EXPAND-017).
//!
//! A residual that grows without anyone investigating it becomes the loss
//! nobody could explain. This module only decides that a campaign is owed; it
//! replays nothing and schedules nothing.
//!
//! Status: library only. Nothing in the tree feeds the detector yet — the
//! central plane's `qip_venue_fill_error_bps` histogram is diagnostic and read
//! by nothing that decides — so the register row this serves stays PARTIAL
//! with `integrated = false`.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A campaign to replay and study tick-level microstructure after a residual
/// anomaly or a new venue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickCampaign {
    /// Identifier, issued in sequence by the detector that opened it, so the
    /// same observations replayed through a fresh detector name the same
    /// campaigns. A wall-clock identifier would make a replay disagree with
    /// the record it is replaying.
    pub campaign_id: String,
    /// Why this campaign was opened.
    pub trigger: TickCampaignTrigger,
    /// Instruments affected by the residual or the new venue.
    pub affected_instruments: BTreeSet<String>,
    /// The venue affected, if venue-specific.
    pub venue: Option<String>,
    /// The observed residual that opened the campaign; `None` for a new venue.
    pub residual_value: Option<Decimal>,
    /// The threshold the residual's magnitude exceeded; `None` for a new
    /// venue, which has no threshold rather than a threshold of zero.
    pub threshold: Option<Decimal>,
}

/// The reason a [`TickCampaign`] was opened.
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
    /// World-event reaction residuals grow.
    WorldEventReaction,
    /// Divergence between paper fills and replayed fills.
    PaperReplayDivergence,
    /// A venue not previously seen.
    NewVenue,
}

/// Opens tick-replay campaigns from residual observations and venue sightings.
#[derive(Debug, Clone)]
pub struct TickCampaignDetector {
    slippage_threshold: Decimal,
    known_venues: BTreeSet<String>,
    issued: u64,
}

impl TickCampaignDetector {
    /// The default slippage threshold, 0.01 (one per cent). The blueprint
    /// does not define the threshold for "grow"; this is the house default
    /// until a calibrated one exists.
    pub const DEFAULT_SLIPPAGE_THRESHOLD: Decimal = Decimal::from_raw(10_000_000);

    /// A detector with the default slippage threshold and no known venues.
    pub fn new() -> Result<Self> {
        Self::with_slippage_threshold(Self::DEFAULT_SLIPPAGE_THRESHOLD)
    }

    /// A detector with the given slippage threshold.
    ///
    /// Refuses a negative threshold: residuals are compared by magnitude, so a
    /// negative threshold would open a campaign on every observation,
    /// including a residual of zero, and drown the real ones.
    pub fn with_slippage_threshold(threshold: Decimal) -> Result<Self> {
        if threshold < Decimal::ZERO {
            return Err(Error::invalid(format!(
                "slippage threshold {threshold} is negative; residuals are compared by magnitude, so pass a threshold of zero or more"
            )));
        }
        Ok(Self {
            slippage_threshold: threshold,
            known_venues: BTreeSet::new(),
            issued: 0,
        })
    }

    /// The threshold a slippage residual's magnitude must exceed.
    pub fn slippage_threshold(&self) -> Decimal {
        self.slippage_threshold
    }

    /// Record a venue sighting. Opens a [`TickCampaignTrigger::NewVenue`]
    /// campaign the first time a venue is seen, scoped to the instruments it
    /// was seen trading, and nothing on every later sighting.
    pub fn observe_venue(
        &mut self,
        venue: &str,
        instruments: BTreeSet<String>,
    ) -> Option<TickCampaign> {
        if !self.known_venues.insert(venue.to_string()) {
            return None;
        }
        Some(TickCampaign {
            campaign_id: self.next_id(),
            trigger: TickCampaignTrigger::NewVenue,
            affected_instruments: instruments,
            venue: Some(venue.to_string()),
            residual_value: None,
            threshold: None,
        })
    }

    /// Record an unexplained-slippage residual. Opens a campaign when its
    /// magnitude strictly exceeds the threshold — a negative residual matters
    /// as much as a positive one of the same size — and nothing otherwise.
    pub fn observe_slippage(
        &mut self,
        residual: Decimal,
        venue: Option<&str>,
        instruments: BTreeSet<String>,
    ) -> Option<TickCampaign> {
        if residual.abs() <= self.slippage_threshold {
            return None;
        }
        Some(TickCampaign {
            campaign_id: self.next_id(),
            trigger: TickCampaignTrigger::UnexplainedSlippage,
            affected_instruments: instruments,
            venue: venue.map(str::to_string),
            residual_value: Some(residual),
            threshold: Some(self.slippage_threshold),
        })
    }

    fn next_id(&mut self) -> String {
        self.issued += 1;
        format!("tick-campaign-{:06}", self.issued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    /// `n` hundredths of a per cent, built from the raw scaled integer so no
    /// fallible constructor is needed in a test.
    fn pct(hundredths_of_a_percent: i128) -> Decimal {
        Decimal::from_raw(hundredths_of_a_percent * 100_000)
    }

    fn detector() -> TickCampaignDetector {
        match TickCampaignDetector::new() {
            Ok(d) => d,
            Err(e) => panic!("the default detector must construct: {e}"),
        }
    }

    /// One recorded window: slippage that grows from 0.5% to 2% at NYSE, then
    /// a venue the detector has never seen. The row's verification asks for
    /// exactly this window.
    fn replay(detector: &mut TickCampaignDetector) -> Vec<TickCampaign> {
        let mut opened = Vec::new();
        for residual in [pct(50), pct(90), pct(200)] {
            opened.extend(detector.observe_venue("NYSE", set(&["AAPL"])));
            opened.extend(detector.observe_slippage(residual, Some("NYSE"), set(&["AAPL"])));
        }
        opened.extend(detector.observe_venue("IEX", set(&["MSFT", "GOOG"])));
        opened
    }

    #[test]
    fn a_window_where_slippage_grows_and_a_venue_appears_opens_one_scoped_campaign_for_each() {
        let mut detector = detector();
        // NYSE is known before the window starts, so the only new venue in
        // the window is IEX.
        assert!(detector.observe_venue("NYSE", set(&["AAPL"])).is_some());

        let opened = replay(&mut detector);

        assert_eq!(
            opened.len(),
            2,
            "expected one slippage and one venue campaign: {opened:?}"
        );
        let slippage = &opened[0];
        assert_eq!(slippage.trigger, TickCampaignTrigger::UnexplainedSlippage);
        assert_eq!(slippage.residual_value, Some(pct(200)));
        assert_eq!(slippage.threshold, Some(pct(100)));
        assert_eq!(slippage.venue.as_deref(), Some("NYSE"));
        assert_eq!(slippage.affected_instruments, set(&["AAPL"]));

        let venue = &opened[1];
        assert_eq!(venue.trigger, TickCampaignTrigger::NewVenue);
        assert_eq!(venue.venue.as_deref(), Some("IEX"));
        assert_eq!(venue.affected_instruments, set(&["GOOG", "MSFT"]));
        assert_eq!(venue.residual_value, None);
        assert_eq!(venue.threshold, None);
    }

    #[test]
    fn replaying_the_same_window_through_a_fresh_detector_opens_identical_campaigns() {
        let first = replay(&mut detector());
        let second = replay(&mut detector());
        // Premise: the window opened something (NYSE, the 2% slippage, IEX),
        // so equality is not two empty lists agreeing.
        assert_eq!(first.len(), 3, "{first:?}");
        assert_eq!(first, second);
        let ids: BTreeSet<&str> = first.iter().map(|c| c.campaign_id.as_str()).collect();
        assert_eq!(
            ids.len(),
            first.len(),
            "campaign ids must be distinct: {first:?}"
        );
    }

    #[test]
    fn a_residual_exactly_at_the_threshold_opens_nothing_and_one_just_above_does() {
        let mut detector = detector();
        let at = detector.slippage_threshold();
        assert_eq!(at, pct(100));
        assert_eq!(detector.observe_slippage(at, None, set(&["AAPL"])), None);
        let above = at + Decimal::from_raw(1);
        assert!(
            detector
                .observe_slippage(above, None, set(&["AAPL"]))
                .is_some()
        );
    }

    #[test]
    fn a_negative_residual_opens_a_campaign_at_the_same_magnitude_as_a_positive_one() {
        let mut detector = detector();
        let negative = Decimal::ZERO - pct(200);
        let opened = detector.observe_slippage(negative, Some("NYSE"), set(&["GOOG"]));
        assert_eq!(opened.map(|c| c.residual_value), Some(Some(negative)));
    }

    #[test]
    fn a_venue_seen_twice_opens_a_campaign_only_the_first_time() {
        let mut detector = detector();
        assert!(detector.observe_venue("NYSE", set(&["AAPL"])).is_some());
        assert_eq!(detector.observe_venue("NYSE", set(&["AAPL"])), None);
        assert!(detector.observe_venue("NASDAQ", set(&["AAPL"])).is_some());
    }

    #[test]
    fn a_negative_threshold_is_refused_rather_than_opening_a_campaign_on_everything() {
        match TickCampaignDetector::with_slippage_threshold(Decimal::ZERO - pct(1)) {
            Err(e) => assert!(e.to_string().contains("is negative"), "{e}"),
            Ok(d) => panic!("a negative threshold was admitted: {d:?}"),
        }
        assert!(TickCampaignDetector::with_slippage_threshold(Decimal::ZERO).is_ok());
    }
}
