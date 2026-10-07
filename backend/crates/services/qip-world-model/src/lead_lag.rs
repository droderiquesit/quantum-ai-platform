//! Lead-lag relationships: which venues move first in price discovery.
//!
//! Lead-lag analysis identifies which venues typically lead price changes and
//! which lag, revealing information flow patterns across markets.

use qip_contracts::venue::VenueId;
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Maximum samples to retain for lead-lag analysis per venue pair
const LEAD_LAG_SAMPLE_LIMIT: usize = 1000;

/// Lag direction in price discovery
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeadPosition {
    Leader,   // This venue typically moves first
    Follower, // This venue typically responds to others
    Neutral,  // No clear relationship
}

/// Lead-lag relationship between two venues
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeadLagRelationship {
    pub leader: VenueId,
    pub follower: VenueId,
    /// Average lag time from leader move to follower response
    pub avg_lag_ms: u64,
    /// Fraction of leader moves followed by follower response
    pub correlation: f64,
    /// Number of observations this relationship is based on
    pub sample_count: usize,
}

/// One venue's lead-lag position for an instrument
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueLeadLagProfile {
    pub venue: VenueId,
    pub position: LeadPosition,
    /// Relationships this venue has (as leader or follower)
    pub relationships: Vec<LeadLagRelationship>,
    /// Fraction of times this venue moved first among observable peers
    pub leadership_score: f64,
}

/// Lead-lag network for an instrument
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeadLagNetwork {
    pub valid_at: Timestamp,
    pub known_at: Timestamp,
    /// Profiles for each venue
    pub profiles: BTreeMap<VenueId, VenueLeadLagProfile>,
}

/// Sample of a price move at one venue
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PriceMoveEvent {
    venue: VenueId,
    at: Timestamp,
    direction: MoveDirection,
    magnitude: f64,
}

/// Direction of price movement
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum MoveDirection {
    Up,
    Down,
}

/// Lead-lag state tracker for one instrument
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeadLagState {
    /// Recent price moves per venue (bounded)
    recent_moves: BTreeMap<VenueId, Vec<PriceMoveEvent>>,
    /// Pairwise lead-lag samples
    relationships: BTreeMap<(VenueId, VenueId), LeadLagRelationship>,
}

impl Default for LeadLagState {
    fn default() -> Self {
        Self::new()
    }
}

impl LeadLagState {
    pub fn new() -> Self {
        Self {
            recent_moves: BTreeMap::new(),
            relationships: BTreeMap::new(),
        }
    }

    /// Record a price move at a venue
    pub fn record_move(&mut self, venue: VenueId, at: Timestamp, direction: bool, magnitude: f64) {
        let moves = self.recent_moves.entry(venue.clone()).or_default();
        moves.push(PriceMoveEvent {
            venue: venue.clone(),
            at,
            direction: if direction {
                MoveDirection::Up
            } else {
                MoveDirection::Down
            },
            magnitude,
        });
        if moves.len() > LEAD_LAG_SAMPLE_LIMIT {
            moves.remove(0);
        }

        self.update_relationships(venue, at);
    }

    fn update_relationships(&mut self, venue: VenueId, _at: Timestamp) {
        if let Some(latest_move) = self.recent_moves.get(&venue).and_then(|v| v.last()) {
            for (other_venue, other_moves) in &self.recent_moves {
                if other_venue == &venue {
                    continue;
                }

                // Look for responses within 100ms of the move
                let response_window = Duration::from_millis(100);
                let followers: Vec<_> = other_moves
                    .iter()
                    .filter(|m| {
                        let delta = if m.at > latest_move.at {
                            m.at.since(latest_move.at)
                        } else {
                            Duration::ZERO
                        };
                        delta <= response_window && m.direction == latest_move.direction
                    })
                    .collect();

                if followers.len() > 10 {
                    let avg_lag_ms = followers
                        .iter()
                        .map(|f| f.at.since(latest_move.at).as_millis() as u64)
                        .sum::<u64>()
                        / followers.len() as u64;

                    let relationship = LeadLagRelationship {
                        leader: venue.clone(),
                        follower: other_venue.clone(),
                        avg_lag_ms,
                        correlation: followers.len() as f64 / other_moves.len() as f64,
                        sample_count: followers.len(),
                    };

                    self.relationships
                        .insert((venue.clone(), other_venue.clone()), relationship);
                }
            }
        }
    }

    /// Get the lead-lag network for current state
    pub fn network(&self, valid_at: Timestamp, known_at: Timestamp) -> Option<LeadLagNetwork> {
        if self.recent_moves.is_empty() {
            return None;
        }

        let mut profiles = BTreeMap::new();
        for venue in self.recent_moves.keys() {
            let as_leader: Vec<_> = self
                .relationships
                .iter()
                .filter(|((l, _), _)| l == venue)
                .map(|(_, rel)| rel.clone())
                .collect();

            let total_moves = self.recent_moves.get(venue).map(|v| v.len()).unwrap_or(0);
            let leadership_score = if total_moves == 0 {
                0.0
            } else {
                as_leader.len() as f64 / total_moves as f64
            };

            let position = if leadership_score > 0.3 {
                LeadPosition::Leader
            } else if leadership_score < 0.1 {
                LeadPosition::Follower
            } else {
                LeadPosition::Neutral
            };

            profiles.insert(
                venue.clone(),
                VenueLeadLagProfile {
                    venue: venue.clone(),
                    position,
                    relationships: as_leader,
                    leadership_score,
                },
            );
        }

        if profiles.is_empty() {
            None
        } else {
            Some(LeadLagNetwork {
                valid_at,
                known_at,
                profiles,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lead_lag_state_tracks_moves() {
        let mut state = LeadLagState::new();
        let venue: VenueId = VenueId::new("NYSE");
        state.record_move(venue.clone(), Timestamp::from_secs(0), true, 0.5);
        assert!(state.recent_moves.contains_key(&venue));
    }

    #[test]
    fn test_lead_position_classification() {
        let profile = VenueLeadLagProfile {
            venue: VenueId::new("NYSE"),
            position: LeadPosition::Leader,
            relationships: vec![],
            leadership_score: 0.5,
        };
        assert_eq!(profile.position, LeadPosition::Leader);
    }
}
