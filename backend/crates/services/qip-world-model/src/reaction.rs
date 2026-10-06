//! World events and their observable market reactions — WORLD-044, WORLD-045.
//!
//! A [`ReactionEpisode`] pairs a material world event with a labelled market reaction.
//! Unlike statistical discovery of temporal precedence (Granger causality), a reaction
//! episode *names* the lag and provides observed evidence that a specific cause preceded
//! a specific effect. These serve three purposes:
//!
//! 1. **Evidence for causal graph updates** (WORLD-045): labelled reaction episodes guide
//!    temporal precedence discovery toward pairs likely to causally connect, speeding
//!    establishment beyond pure statistical scanning.
//! 2. **Feedback for mechanism discovery**: when the platform's causal model predicts
//!    no lag or a different lag, the recorded lag updates the model.
//! 3. **Falsification**: if a recorded reaction contradicts a held causal edge, that
//!    edge's confidence is reduced and the edge is marked for re-examination.

use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// A material event in the world and the market's observable reaction to it.
///
/// The event is recorded as:
/// - **What happened** (event_name, e.g. "FOMC_decision", "earnings_miss", "sanctions")
/// - **When it happened** (event_time)
/// - **Market consequence** (affected_instruments, reaction_magnitude_bps)
/// - **Observable lag** (time_to_reaction: how long before the market moved)
/// - **Confidence** (how sure we are of the causal connection)
///
/// The reaction lag is the time from the event to the first material market movement.
/// "Material" is defined as exceeding a basis-point threshold (typically 5-10 bps for
/// major price moves, depending on asset class volatility). The lag is measured in the
/// bars' own cadence (intraday, daily, etc.) — it is not a wall-clock duration the
/// platform guesses at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReactionEpisode {
    /// Unique identifier for this reaction episode.
    pub episode_id: String,

    /// Human-readable name of the world event (e.g., "FOMC_decision_2026-10-06").
    pub event_name: String,

    /// When the event occurred (the cause).
    pub event_time: Timestamp,

    /// When the market reaction began (the effect observed).
    pub reaction_time: Timestamp,

    /// List of instruments affected by the reaction.
    /// Each entry is (instrument_id, reaction_magnitude_bps, direction).
    /// Direction: +1.0 for price increase, -1.0 for decrease.
    pub affected_instruments: Vec<(String, f64, f64)>,

    /// The lag from event to observable market reaction, in bars of the measurement cadence.
    /// This is the core evidence for temporal precedence: a cause must precede its effect.
    pub lag_bars: usize,

    /// The confidence in the causal connection: [0, 1].
    /// 1.0 = certain, 0.5 = moderate, 0.0 = uncertain.
    /// This is a platform-assigned confidence, not the statistical p-value from Granger.
    pub confidence: f64,

    /// Human explanation for why this event was material and the reaction was real.
    /// E.g., "Fed rate cut → risk-on inflows to equities".
    pub rationale: String,

    /// When this reaction episode was recorded (created/observed).
    pub recorded_at: Timestamp,
}

impl ReactionEpisode {
    /// Create a new reaction episode, validating the parameters.
    ///
    /// # Refusals
    ///
    /// - Event time >= reaction time (reaction must follow event).
    /// - Confidence outside [0, 1].
    /// - Empty affected_instruments list.
    /// - Any reaction magnitude is NaN or infinite.
    /// - lag_bars is 0.
    pub fn new(
        episode_id: String,
        event_name: String,
        event_time: Timestamp,
        reaction_time: Timestamp,
        affected_instruments: Vec<(String, f64, f64)>,
        lag_bars: usize,
        confidence: f64,
        rationale: String,
        recorded_at: Timestamp,
    ) -> Result<Self> {
        // Event must precede reaction.
        if event_time >= reaction_time {
            return Err(Error::invalid(format!(
                "reaction episode {episode_id}: event at {event_time} must precede reaction at \
                 {reaction_time}; a cause cannot happen after its effect"
            )));
        }

        // Confidence must be a valid probability.
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(Error::invalid(format!(
                "reaction episode {episode_id}: confidence {confidence} outside [0, 1] or \
                 non-finite; must be a valid probability"
            )));
        }

        // Must have at least one affected instrument.
        if affected_instruments.is_empty() {
            return Err(Error::invalid(format!(
                "reaction episode {episode_id}: no affected instruments recorded; a reaction \
                 with no observable market move is not a reaction episode"
            )));
        }

        // Validate each instrument's reaction magnitude.
        for (instrument, magnitude, direction) in &affected_instruments {
            if !magnitude.is_finite() {
                return Err(Error::invalid(format!(
                    "reaction episode {episode_id}: {instrument} has non-finite reaction \
                     magnitude {magnitude}; magnitude must be a real number in basis points"
                )));
            }
            if !direction.is_finite() || !([-1.0, 0.0, 1.0].contains(direction)) {
                return Err(Error::invalid(format!(
                    "reaction episode {episode_id}: {instrument} has invalid direction {direction}; \
                     must be -1.0, 0.0, or +1.0"
                )));
            }
        }

        // Lag must be at least one bar.
        if lag_bars == 0 {
            return Err(Error::invalid(format!(
                "reaction episode {episode_id}: lag_bars is 0; a reaction in the same bar as \
                 the event is not a lead-lag relationship"
            )));
        }

        Ok(ReactionEpisode {
            episode_id,
            event_name,
            event_time,
            reaction_time,
            affected_instruments,
            lag_bars,
            confidence,
            rationale,
            recorded_at,
        })
    }

    /// The wall-clock duration from event to reaction.
    pub fn duration(&self) -> Duration {
        self.reaction_time.since(self.event_time)
    }

    /// The number of instruments affected by this reaction.
    pub fn affected_count(&self) -> usize {
        self.affected_instruments.len()
    }

    /// The average absolute reaction magnitude across all affected instruments.
    pub fn average_reaction_magnitude(&self) -> f64 {
        if self.affected_instruments.is_empty() {
            return 0.0;
        }
        let sum: f64 = self
            .affected_instruments
            .iter()
            .map(|(_, mag, _)| mag.abs())
            .sum();
        sum / self.affected_instruments.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reaction_episode_requires_event_before_reaction() {
        let now = Timestamp::from_secs(1000);
        let later = now.saturating_add(Duration::from_secs(60));

        let result = ReactionEpisode::new(
            "r001".into(),
            "test_event".into(),
            later, // event after reaction → fail
            now,   // reaction
            vec![("BTC".into(), 100.0, 1.0)],
            1,
            0.7,
            "test".into(),
            now,
        );

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("event at") && err_msg.contains("precede"));
    }

    #[test]
    fn a_reaction_episode_refuses_invalid_confidence() {
        let now = Timestamp::from_secs(1000);
        let later = now.saturating_add(Duration::from_secs(60));

        let result = ReactionEpisode::new(
            "r001".into(),
            "test_event".into(),
            now,
            later,
            vec![("BTC".into(), 100.0, 1.0)],
            1,
            1.5, // confidence > 1.0 → fail
            "test".into(),
            now,
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("confidence"));
    }

    #[test]
    fn a_reaction_episode_requires_affected_instruments() {
        let now = Timestamp::from_secs(1000);
        let later = now.saturating_add(Duration::from_secs(60));

        let result = ReactionEpisode::new(
            "r001".into(),
            "test_event".into(),
            now,
            later,
            vec![], // empty instruments → fail
            1,
            0.7,
            "test".into(),
            now,
        );

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no affected instruments")
        );
    }

    #[test]
    fn a_reaction_episode_refuses_zero_lag() {
        let now = Timestamp::from_secs(1000);
        let later = now.saturating_add(Duration::from_secs(60));

        let result = ReactionEpisode::new(
            "r001".into(),
            "test_event".into(),
            now,
            later,
            vec![("BTC".into(), 100.0, 1.0)],
            0, // zero lag → fail
            0.7,
            "test".into(),
            now,
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("lag_bars is 0"));
    }

    #[test]
    fn a_valid_reaction_episode_is_created() {
        let now = Timestamp::from_secs(1000);
        let later = now.saturating_add(Duration::from_secs(3600));

        let episode = ReactionEpisode::new(
            "r001".into(),
            "FOMC_decision".into(),
            now,
            later,
            vec![("BTC".into(), 250.5, 1.0), ("ETH".into(), 180.0, 1.0)],
            4,
            0.8,
            "Fed cut rates → crypto rally".into(),
            now,
        );

        assert!(episode.is_ok());
        let ep = episode.unwrap();
        assert_eq!(ep.affected_count(), 2);
        assert!((ep.average_reaction_magnitude() - 215.25).abs() < 0.01);
    }

    #[test]
    fn affected_instruments_are_validated_for_non_finite_magnitudes() {
        let now = Timestamp::from_secs(1000);
        let later = now.saturating_add(Duration::from_secs(60));

        let result = ReactionEpisode::new(
            "r001".into(),
            "test_event".into(),
            now,
            later,
            vec![("BTC".into(), f64::NAN, 1.0)], // NaN magnitude → fail
            1,
            0.7,
            "test".into(),
            now,
        );

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("non-finite reaction")
        );
    }

    #[test]
    fn affected_instruments_direction_must_be_unit_signed() {
        let now = Timestamp::from_secs(1000);
        let later = now.saturating_add(Duration::from_secs(60));

        let result = ReactionEpisode::new(
            "r001".into(),
            "test_event".into(),
            now,
            later,
            vec![("BTC".into(), 100.0, 0.5)], // invalid direction → fail
            1,
            0.7,
            "test".into(),
            now,
        );

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid direction")
        );
    }
}
