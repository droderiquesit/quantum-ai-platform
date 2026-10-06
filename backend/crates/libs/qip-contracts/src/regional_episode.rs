//! Regional episode artifacts (EXPAND-009): typed events recorded by edge cells.
//!
//! Each reflex cell records local episodes as typed artifacts, naming the cell
//! and region where the event occurred. Episodes include latency spikes,
//! fill/slippage events, anomalies, failures, microstructure observations,
//! and venue-behaviour changes.
//!
//! Episodes are held in the cell's memory, bounded, and are **not** journaled:
//! the facts they classify (fills, breaks) are in the cell's hash-chained
//! journal, but the classification itself is lost when the process stops.
//! The row's `ReflexDelta` — what the cell changed in response to an episode
//! — has no type here, because nothing in the cell yet decides anything from
//! an episode, and a type nobody constructs would read as a control.

use qip_core::Timestamp;
use serde::{Deserialize, Serialize};

/// The category of a regional episode, identifying what event was observed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeKind {
    /// A fill arrived slower than expected or expected latency was exceeded.
    /// `detail` names the venue and measured latency in milliseconds.
    LatencySpike,

    /// A fill executed at a worse price than expected, or unexpected slippage
    /// was observed. `detail` names the venue, expected price, and actual price.
    FillSlippage,

    /// An anomalous market condition or data integrity issue was detected.
    /// `detail` describes the anomaly and impact on the cell's sizing.
    Anomaly,

    /// A critical failure occurred (venue down, feed stall, reconciliation
    /// break). `detail` names the failure, affected venue(s), and recovery action.
    Failure,

    /// Microstructure observation: order book imbalance, layer imbalance,
    /// or spread widening. `detail` contains the observation and timing.
    Microstructure,

    /// Venue behaviour change detected: new latency profile, new fees,
    /// new order validation rules. `detail` names the venue and change.
    VenueBehaviour,
}

impl EpisodeKind {
    /// The string representation of this episode kind.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::LatencySpike => "latency_spike",
            Self::FillSlippage => "fill_slippage",
            Self::Anomaly => "anomaly",
            Self::Failure => "failure",
            Self::Microstructure => "microstructure",
            Self::VenueBehaviour => "venue_behaviour",
        }
    }
}

/// One typed episode a regional cell observed and recorded.
///
/// Each episode names the cell and region where it occurred, along with the
/// kind of event and structured details that allow replay and analysis.
/// Episodes are recorded at the moment the event becomes known, so they can
/// be used to explain decisions made in the same pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionalEpisode {
    /// The cell that recorded this episode (e.g., "london-1", "tokyo-2").
    pub cell: String,

    /// The region the cell belongs to (e.g., "emea", "apac").
    pub region: String,

    /// What kind of episode this is.
    pub kind: EpisodeKind,

    /// When the episode was recorded (the instant it became known).
    pub recorded_at: Timestamp,

    /// The instant the event itself occurred, which may differ from recording
    /// time (e.g., a delayed fill report). `None` for events with no clear
    /// occurrence time.
    pub occurred_at: Option<Timestamp>,

    /// Details structured by episode kind:
    /// - LatencySpike: venue name, measured latency in ms
    /// - FillSlippage: venue name, expected price, actual price, quantity
    /// - Anomaly: description of anomaly, impact on sizing
    /// - Failure: failure name, affected venue(s), recovery action
    /// - Microstructure: observation type, venue, levels affected
    /// - VenueBehaviour: venue name, behaviour change description
    pub detail: String,
}

impl RegionalEpisode {
    /// Create a new regional episode.
    pub fn new(
        cell: String,
        region: String,
        kind: EpisodeKind,
        recorded_at: Timestamp,
        detail: String,
    ) -> Self {
        Self {
            cell,
            region,
            kind,
            recorded_at,
            occurred_at: None,
            detail,
        }
    }

    /// Set the occurrence time (when the event actually happened, if known).
    pub fn with_occurred_at(mut self, occurred_at: Timestamp) -> Self {
        self.occurred_at = Some(occurred_at);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_regional_episode_has_cell_and_region() {
        let episode = RegionalEpisode::new(
            "london-1".to_string(),
            "emea".to_string(),
            EpisodeKind::LatencySpike,
            Timestamp::from_secs(1_700_000_000),
            "XLON: 145ms".to_string(),
        );

        assert_eq!(episode.cell, "london-1");
        assert_eq!(episode.region, "emea");
        assert_eq!(episode.kind, EpisodeKind::LatencySpike);
    }

    #[test]
    fn an_episode_can_have_an_occurrence_time() {
        let episode = RegionalEpisode::new(
            "london-1".to_string(),
            "emea".to_string(),
            EpisodeKind::FillSlippage,
            Timestamp::from_secs(1_700_000_100),
            "XLON: expected 100.50, got 100.25".to_string(),
        )
        .with_occurred_at(Timestamp::from_secs(1_700_000_050));

        assert_eq!(
            episode.occurred_at,
            Some(Timestamp::from_secs(1_700_000_050))
        );
    }

    #[test]
    fn each_episode_kind_s_label_is_the_name_it_serialises_under() {
        // `as_str` and the serde rename are two spellings of one fact; a
        // label that drifts from the wire name would chart one kind under
        // a name the journal never carries.
        for kind in [
            EpisodeKind::LatencySpike,
            EpisodeKind::FillSlippage,
            EpisodeKind::Anomaly,
            EpisodeKind::Failure,
            EpisodeKind::Microstructure,
            EpisodeKind::VenueBehaviour,
        ] {
            let wire = match serde_json::to_string(&kind) {
                Ok(wire) => wire,
                Err(e) => panic!("{kind:?} did not serialise: {e}"),
            };
            assert_eq!(wire, format!("\"{}\"", kind.as_str()), "{kind:?}");
        }
    }
}
