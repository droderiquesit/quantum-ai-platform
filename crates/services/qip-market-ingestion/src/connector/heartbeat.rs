//! Heartbeat, and telling a dead connector from a dead source.
//!
//! Two failures look identical on a dashboard that only counts records, and
//! they have different runbooks:
//!
//! * **Silent** — nothing has answered. The connector, the network or the
//!   source is down, and the fix is on our side of the socket first.
//! * **Stale** — answers keep arriving and the newest event in them is old.
//!   The source is up and has stopped producing, and the fix is a call to the
//!   provider.
//!
//! A monitor that measured only "records per minute" reports both as zero and
//! sends the on-call engineer to the wrong place. So this records the two
//! instants separately: when the source last *answered*, and when the newest
//! event it answered with *occurred*.
//!
//! Both are measured against the manifest's freshness SLA and both take the
//! instant from the caller, so a replay of a log produces the same verdict the
//! monitor reached live.

use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// What a feed is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "liveness", rename_all = "snake_case")]
pub enum Liveness {
    /// Answering, with events inside the SLA.
    Live,
    /// Nothing has answered yet. Not the same as silence: a feed that has
    /// never started has no last success to be silent since, and reporting it
    /// as stale would page somebody for a deployment that has not rolled out.
    NeverStarted,
    /// No successful answer within the SLA.
    Silent {
        since: Timestamp,
        quiet_for: Duration,
    },
    /// Answering, but the newest event is older than the SLA.
    Stale {
        newest_event_at: Timestamp,
        behind_by: Duration,
    },
}

impl Liveness {
    pub const fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }

    /// Whether this state should raise an alarm. `NeverStarted` does not:
    /// a feed that has not begun is a rollout, not an incident.
    pub const fn is_alarming(&self) -> bool {
        matches!(self, Self::Silent { .. } | Self::Stale { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Live => "live".to_string(),
            Self::NeverStarted => "never started: no successful fetch yet".to_string(),
            Self::Silent { since, quiet_for } => format!(
                "silent: nothing has answered since {since}, {quiet_for:?} ago. The connector, \
                 the network or the source is down"
            ),
            Self::Stale {
                newest_event_at,
                behind_by,
            } => format!(
                "stale: the source is answering and its newest event is from {newest_event_at}, \
                 {behind_by:?} behind. The source has stopped producing"
            ),
        }
    }
}

/// One feed's liveness state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedHeartbeat {
    source_id: String,
    sla: Duration,
    last_answer: Option<Timestamp>,
    newest_event: Option<Timestamp>,
    beats: u64,
    empty_beats: u64,
}

impl FeedHeartbeat {
    pub fn new(source_id: impl Into<String>, sla: Duration) -> Self {
        Self {
            source_id: source_id.into(),
            sla,
            last_answer: None,
            newest_event: None,
            beats: 0,
            empty_beats: 0,
        }
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub const fn sla(&self) -> Duration {
        self.sla
    }

    pub const fn beats(&self) -> u64 {
        self.beats
    }

    /// Answers that carried no event. Counted because a source answering
    /// `[]` forever is stale in a way "requests succeeded" will not show.
    pub const fn empty_beats(&self) -> u64 {
        self.empty_beats
    }

    pub const fn last_answer(&self) -> Option<Timestamp> {
        self.last_answer
    }

    pub const fn newest_event(&self) -> Option<Timestamp> {
        self.newest_event
    }

    /// Record a successful answer at `at`, carrying an event that occurred at
    /// `newest_event_at` if it carried one at all.
    ///
    /// The newest event only ever moves forward. A source that re-serves an
    /// old page must not be able to make the feed look *more* stale than it
    /// is, and a source replaying history must not make it look fresher.
    pub fn answered(&mut self, at: Timestamp, newest_event_at: Option<Timestamp>) {
        self.beats = self.beats.saturating_add(1);
        self.last_answer = Some(match self.last_answer {
            Some(previous) if previous > at => previous,
            _ => at,
        });
        match newest_event_at {
            None => self.empty_beats = self.empty_beats.saturating_add(1),
            Some(event_at) => {
                self.newest_event = Some(match self.newest_event {
                    Some(previous) if previous > event_at => previous,
                    _ => event_at,
                });
            }
        }
    }

    /// The verdict at `at`.
    ///
    /// Silence is checked before staleness: a feed that has not answered has
    /// no newest event worth judging, and reporting it as stale would name the
    /// wrong runbook.
    pub fn liveness(&self, at: Timestamp) -> Liveness {
        let Some(last_answer) = self.last_answer else {
            return Liveness::NeverStarted;
        };
        let quiet_for = at.since(last_answer);
        if quiet_for > self.sla {
            return Liveness::Silent {
                since: last_answer,
                quiet_for,
            };
        }
        let Some(newest_event) = self.newest_event else {
            // Answering, and every answer has been empty. That is the source
            // producing nothing, which is staleness measured from the first
            // answer rather than from an event that never arrived.
            return if quiet_for > self.sla {
                Liveness::Silent {
                    since: last_answer,
                    quiet_for,
                }
            } else {
                Liveness::Live
            };
        };
        let behind_by = at.since(newest_event);
        if behind_by > self.sla {
            return Liveness::Stale {
                newest_event_at: newest_event,
                behind_by,
            };
        }
        Liveness::Live
    }
}
