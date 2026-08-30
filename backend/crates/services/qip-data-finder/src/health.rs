//! Whether a registered source is still working.
//!
//! Health is computed over an explicit window of observations the caller
//! supplies, with the window's end passed in. There is no ambient clock here
//! for the same reason there is none anywhere else: a monitor that reads the
//! wall clock cannot be replayed, and the first time that matters is the
//! incident review where nobody can reproduce what the monitor saw.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// What one attempt to read a source produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ObservationOutcome {
    /// The source answered.
    Served {
        latency: Duration,
        /// When the newest record in the answer was true in the world. This
        /// is what staleness is measured from — an endpoint that answers
        /// instantly with yesterday's numbers is available and useless.
        payload_at: Timestamp,
    },
    /// The source answered with an error status.
    Failed { status: u16 },
    /// The source did not answer within the deadline.
    TimedOut,
}

impl ObservationOutcome {
    pub fn succeeded(&self) -> bool {
        matches!(self, Self::Served { .. })
    }

    pub fn latency(&self) -> Option<Duration> {
        match self {
            Self::Served { latency, .. } => Some(*latency),
            Self::Failed { .. } | Self::TimedOut => None,
        }
    }
}

/// One attempt, at a stated time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthObservation {
    pub at: Timestamp,
    pub outcome: ObservationOutcome,
}

impl HealthObservation {
    pub fn served(at: Timestamp, latency: Duration, payload_at: Timestamp) -> Self {
        Self {
            at,
            outcome: ObservationOutcome::Served {
                latency,
                payload_at,
            },
        }
    }

    pub fn failed(at: Timestamp, status: u16) -> Self {
        Self {
            at,
            outcome: ObservationOutcome::Failed { status },
        }
    }

    pub fn timed_out(at: Timestamp) -> Self {
        Self {
            at,
            outcome: ObservationOutcome::TimedOut,
        }
    }
}

/// A source's behaviour over a window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceHealth {
    window: Duration,
    window_ends: Timestamp,
    samples: usize,
    availability: f64,
    error_rate: f64,
    mean_latency: Duration,
    worst_latency: Duration,
    staleness: Duration,
    last_success: Option<Timestamp>,
}

impl SourceHealth {
    /// Compute health over the observations falling inside the window ending
    /// at `window_ends`.
    ///
    /// An empty window is an error rather than a perfect score. "Nobody
    /// looked" and "everything worked" are the same number in most monitoring
    /// systems, and telling them apart is the whole reason a source stops
    /// being trusted before it stops being read.
    pub fn over(
        observations: &[HealthObservation],
        window_ends: Timestamp,
        window: Duration,
    ) -> Result<Self> {
        if window.as_nanos() <= 0 {
            return Err(Error::invalid("a health window must be a positive span"));
        }
        let window_starts = window_ends.saturating_sub(window);
        let inside: Vec<&HealthObservation> = observations
            .iter()
            .filter(|observation| observation.at > window_starts && observation.at <= window_ends)
            .collect();
        if inside.is_empty() {
            return Err(Error::not_found(format!(
                "no observations of this source fall in the {window:?} window ending {window_ends}; \
                 an unobserved source is not a healthy source"
            )));
        }

        let samples = inside.len();
        let served = inside
            .iter()
            .filter(|observation| observation.outcome.succeeded())
            .count();
        let availability = served as f64 / samples as f64;

        let latencies: Vec<Duration> = inside
            .iter()
            .filter_map(|observation| observation.outcome.latency())
            .collect();
        let mean_latency = if latencies.is_empty() {
            Duration::ZERO
        } else {
            let total: i64 = latencies
                .iter()
                .map(|latency| latency.as_nanos())
                .fold(0i64, i64::saturating_add);
            Duration::from_nanos(total / latencies.len() as i64)
        };
        let worst_latency = latencies.iter().copied().max().unwrap_or(Duration::ZERO);

        let newest_payload = inside
            .iter()
            .filter_map(|observation| match observation.outcome {
                ObservationOutcome::Served { payload_at, .. } => Some(payload_at),
                ObservationOutcome::Failed { .. } | ObservationOutcome::TimedOut => None,
            })
            .max();
        let staleness = match newest_payload {
            Some(payload_at) => window_ends.since(payload_at),
            // Nothing was served, so the source is as stale as the window is
            // long — not zero, which is what an unset field would read as.
            None => window,
        };
        let last_success = inside
            .iter()
            .filter(|observation| observation.outcome.succeeded())
            .map(|observation| observation.at)
            .max();

        Ok(Self {
            window,
            window_ends,
            samples,
            availability,
            error_rate: 1.0 - availability,
            mean_latency,
            worst_latency,
            staleness,
            last_success,
        })
    }

    pub fn window(&self) -> Duration {
        self.window
    }

    pub fn window_ends(&self) -> Timestamp {
        self.window_ends
    }

    pub fn samples(&self) -> usize {
        self.samples
    }

    /// Fraction of attempts that were served, in `[0, 1]`.
    pub fn availability(&self) -> f64 {
        self.availability
    }

    /// Fraction of attempts that failed or timed out, in `[0, 1]`.
    pub fn error_rate(&self) -> f64 {
        self.error_rate
    }

    pub fn mean_latency(&self) -> Duration {
        self.mean_latency
    }

    pub fn worst_latency(&self) -> Duration {
        self.worst_latency
    }

    /// How old the newest record served in the window was, at the window's
    /// end.
    pub fn staleness(&self) -> Duration {
        self.staleness
    }

    pub fn last_success(&self) -> Option<Timestamp> {
        self.last_success
    }

    /// Minimum attempts before an all-failing window is called death rather
    /// than a blip.
    pub const DEATH_SAMPLE_FLOOR: usize = 3;

    /// Whether the source answered nothing across a window big enough to
    /// mean it.
    pub fn is_dead(&self) -> bool {
        self.samples >= Self::DEATH_SAMPLE_FLOOR && self.last_success.is_none()
    }

    /// Whether the source is working but below what it promised.
    pub fn is_degraded(&self, expected_freshness: Duration) -> bool {
        !self.is_dead() && (self.availability < 0.99 || self.staleness > expected_freshness)
    }

    pub fn describe(&self) -> String {
        format!(
            "{:.1}% available over {} sample(s), mean latency {:?}, staleness {:?}",
            self.availability * 100.0,
            self.samples,
            self.mean_latency,
            self.staleness
        )
    }
}
