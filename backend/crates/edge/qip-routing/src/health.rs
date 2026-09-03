//! Watching venues behave, and pricing what it costs when they do not.
//!
//! A venue that rejects one order in twenty is not one twentieth broken; it is
//! a venue that costs an extra round trip on one order in twenty, and the round
//! trip is paid in a market that has moved. The same is true of latency. So
//! health is not a flag the router consults after choosing — it is a cost that
//! goes into the choice, which is what makes routing away from a degrading
//! venue automatic rather than something an operator has to notice.
//!
//! Two thresholds sit on top of that cost. Below the first, a venue is healthy
//! and pays only for what it actually does. Past the second it is taken out of
//! rotation entirely, because at some point the right answer is not a bigger
//! number, it is to stop sending.
//!
//! Every verdict carries its reason. "The router stopped using the venue" is a
//! sentence somebody has to be able to finish.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Weight given to the newest latency sample.
///
/// High enough that a venue going slow is visible within a handful of orders,
/// low enough that one slow acknowledgement does not empty the venue.
const LATENCY_SMOOTHING: f64 = 0.2;

/// Where the lines are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthPolicy {
    /// Reject rate above which a venue is reported as degrading.
    pub degraded_reject_rate_f64: f64,
    /// Reject rate above which it stops being sent orders at all.
    pub quarantine_reject_rate_f64: f64,
    /// What a rejected order costs to place again, in basis points.
    ///
    /// The mechanism by which a reject rate becomes a routing decision: a venue
    /// rejecting one in twenty carries a twentieth of this on every order.
    pub requote_cost_bps_f64: f64,
    /// Multiple of a venue's own typical latency that counts as degrading.
    pub latency_multiple_f64: f64,
    /// Basis points charged per multiple of excess latency.
    pub latency_penalty_bps_f64: f64,
    /// Orders that must have been sent before any of this is acted on.
    ///
    /// One reject out of one is a reject rate of a hundred percent and no
    /// evidence at all.
    pub min_samples: u64,
    /// How long a quarantine lasts before the venue is tried again.
    pub quarantine_for: Duration,
}

impl Default for HealthPolicy {
    fn default() -> Self {
        Self {
            degraded_reject_rate_f64: 0.02,
            quarantine_reject_rate_f64: 0.20,
            requote_cost_bps_f64: 5.0,
            latency_multiple_f64: 3.0,
            latency_penalty_bps_f64: 1.0,
            min_samples: 10,
            quarantine_for: Duration::from_secs(300),
        }
    }
}

impl HealthPolicy {
    /// Refuse a policy whose numbers cannot mean what the type claims.
    ///
    /// Every sibling policy in this crate ([`crate::reprice::RepricePolicy`],
    /// [`crate::venue::VenueProfile`]) validates its own invariants at
    /// construction rather than trusting the caller; this one had not, and a
    /// caller-supplied policy could silently make a threshold unreachable
    /// instead of failing loudly. Two failures this catches by name:
    ///
    /// * a `quarantine_reject_rate_f64` set *below* `degraded_reject_rate_f64`
    ///   makes the degraded verdict dead code — `VenueHealth::assess` checks
    ///   quarantine first, so any reject rate that would degrade a venue
    ///   would already have quarantined it, and the milder verdict can never
    ///   fire. That is the same defect class as a risk limit that cannot
    ///   trigger.
    /// * a `latency_multiple_f64` at or below one makes the degrading
    ///   threshold (`multiple - 1.0`) zero or negative, so any latency at
    ///   all — even none — reads as degraded forever.
    pub fn validate(&self) -> Result<()> {
        if !(0.0..=1.0).contains(&self.degraded_reject_rate_f64) {
            return Err(Error::invalid(format!(
                "a degraded reject rate of {} is not a probability",
                self.degraded_reject_rate_f64
            )));
        }
        if !(0.0..=1.0).contains(&self.quarantine_reject_rate_f64) {
            return Err(Error::invalid(format!(
                "a quarantine reject rate of {} is not a probability",
                self.quarantine_reject_rate_f64
            )));
        }
        if self.quarantine_reject_rate_f64 < self.degraded_reject_rate_f64 {
            return Err(Error::invalid(format!(
                "the quarantine reject rate ({}) is below the degraded reject rate ({}); \
                 assessment checks quarantine first, so the degraded verdict could never fire",
                self.quarantine_reject_rate_f64, self.degraded_reject_rate_f64
            )));
        }
        if !self.requote_cost_bps_f64.is_finite() || self.requote_cost_bps_f64 < 0.0 {
            return Err(Error::invalid(
                "the requote cost must be a non-negative finite number of basis points",
            ));
        }
        if !self.latency_multiple_f64.is_finite() || self.latency_multiple_f64 <= 1.0 {
            return Err(Error::invalid(format!(
                "a latency multiple of {} is at or below one, which would mark any latency at \
                 all as degrading; the multiple has to be greater than one to mean anything",
                self.latency_multiple_f64
            )));
        }
        if !self.latency_penalty_bps_f64.is_finite() || self.latency_penalty_bps_f64 < 0.0 {
            return Err(Error::invalid(
                "the latency penalty must be a non-negative finite number of basis points",
            ));
        }
        if self.min_samples == 0 {
            return Err(Error::invalid(
                "a minimum sample count of zero would act on a venue's first order, which is no \
                 evidence at all — the reason this field exists",
            ));
        }
        if self.quarantine_for.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a quarantine that lasts zero time is not a quarantine",
            ));
        }
        Ok(())
    }
}

/// What the router should do about a venue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum HealthVerdict {
    /// Behaving, or not yet observed enough to say otherwise.
    Healthy,
    /// Still usable, and paying for its behaviour.
    Degraded { reason: String },
    /// Out of rotation until `until`.
    Quarantined { until: Timestamp, reason: String },
}

impl HealthVerdict {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded { .. } => "degraded",
            Self::Quarantined { .. } => "quarantined",
        }
    }

    pub const fn is_usable(&self) -> bool {
        !matches!(self, Self::Quarantined { .. })
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::Healthy => "no adverse behaviour observed",
            Self::Degraded { reason } | Self::Quarantined { reason, .. } => reason,
        }
    }
}

/// The verdict, the cost it implies, and the evidence behind both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthAssessment {
    pub venue: VenueId,
    pub verdict: HealthVerdict,
    /// What this venue's behaviour adds to the cost of an order, in basis
    /// points. Zero until there is enough evidence to charge for.
    pub cost_bps_f64: f64,
    pub reject_rate_f64: f64,
    pub observed_latency: Duration,
    pub samples: u64,
}

/// What one venue has actually done.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueHealth {
    venue: VenueId,
    sent: u64,
    rejected: u64,
    latency_ewma_nanos_f64: f64,
    latency_samples: u64,
    last_reject_at: Option<Timestamp>,
}

impl VenueHealth {
    pub fn new(venue: VenueId) -> Self {
        Self {
            venue,
            sent: 0,
            rejected: 0,
            latency_ewma_nanos_f64: 0.0,
            latency_samples: 0,
            last_reject_at: None,
        }
    }

    pub fn venue(&self) -> &VenueId {
        &self.venue
    }

    pub fn sent(&self) -> u64 {
        self.sent
    }

    pub fn rejected(&self) -> u64 {
        self.rejected
    }

    pub fn record_sent(&mut self) {
        self.sent = self.sent.saturating_add(1);
    }

    /// Record an acknowledgement and how long it took.
    pub fn record_ack(&mut self, latency: Duration) {
        let sample = latency.as_nanos().max(0) as f64;
        self.latency_ewma_nanos_f64 = if self.latency_samples == 0 {
            sample
        } else {
            LATENCY_SMOOTHING * sample + (1.0 - LATENCY_SMOOTHING) * self.latency_ewma_nanos_f64
        };
        self.latency_samples = self.latency_samples.saturating_add(1);
    }

    /// Record a rejection. Counts as a send too, so a venue cannot improve its
    /// rate by rejecting orders the tracker never saw go out.
    pub fn record_reject(&mut self, at: Timestamp) {
        self.rejected = self.rejected.saturating_add(1);
        self.last_reject_at = Some(at);
    }

    pub fn reject_rate_f64(&self) -> f64 {
        if self.sent == 0 {
            return 0.0;
        }
        self.rejected as f64 / self.sent as f64
    }

    pub fn observed_latency(&self) -> Duration {
        Duration::from_nanos(self.latency_ewma_nanos_f64 as i64)
    }

    /// Assess the venue against a policy and its own stated latency.
    pub fn assess(
        &self,
        policy: &HealthPolicy,
        typical_latency: Duration,
        at: Timestamp,
    ) -> HealthAssessment {
        let reject_rate_f64 = self.reject_rate_f64();
        let observed_latency = self.observed_latency();

        if self.sent < policy.min_samples {
            return HealthAssessment {
                venue: self.venue.clone(),
                verdict: HealthVerdict::Healthy,
                cost_bps_f64: 0.0,
                reject_rate_f64,
                observed_latency,
                samples: self.sent,
            };
        }

        let latency_excess_f64 = latency_excess(observed_latency, typical_latency);
        let cost_bps_f64 = reject_rate_f64 * policy.requote_cost_bps_f64
            + latency_excess_f64 * policy.latency_penalty_bps_f64;

        let quarantine_until = self
            .last_reject_at
            .map(|last| last.saturating_add(policy.quarantine_for));
        let still_quarantined = quarantine_until.is_some_and(|until| at < until);

        let verdict = if reject_rate_f64 >= policy.quarantine_reject_rate_f64 && still_quarantined {
            HealthVerdict::Quarantined {
                until: quarantine_until.unwrap_or(at),
                reason: format!(
                    "{} rejected {} of {} orders; at that rate the right answer is to stop sending, not to pay more",
                    self.venue.as_str(),
                    self.rejected,
                    self.sent
                ),
            }
        } else if reject_rate_f64 >= policy.degraded_reject_rate_f64 {
            HealthVerdict::Degraded {
                reason: format!(
                    "{} rejected {} of {} orders, which costs {cost_bps_f64}bp per order in re-routing",
                    self.venue.as_str(),
                    self.rejected,
                    self.sent
                ),
            }
        } else if latency_excess_f64 >= policy.latency_multiple_f64 - 1.0 {
            HealthVerdict::Degraded {
                reason: format!(
                    "{} is acknowledging in {observed_latency:?} against a typical {typical_latency:?}",
                    self.venue.as_str()
                ),
            }
        } else {
            HealthVerdict::Healthy
        };

        HealthAssessment {
            venue: self.venue.clone(),
            verdict,
            cost_bps_f64,
            reject_rate_f64,
            observed_latency,
            samples: self.sent,
        }
    }
}

/// Health for every venue the router knows about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HealthTracker {
    policy: HealthPolicy,
    venues: BTreeMap<VenueId, VenueHealth>,
}

impl HealthTracker {
    /// Refuses a policy whose own numbers contradict each other — see
    /// [`HealthPolicy::validate`] for the two shapes that fail here.
    pub fn new(policy: HealthPolicy) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            policy,
            venues: BTreeMap::new(),
        })
    }

    pub fn policy(&self) -> &HealthPolicy {
        &self.policy
    }

    pub fn record_sent(&mut self, venue: &VenueId) {
        self.entry(venue).record_sent();
    }

    pub fn record_ack(&mut self, venue: &VenueId, latency: Duration) {
        self.entry(venue).record_ack(latency);
    }

    pub fn record_reject(&mut self, venue: &VenueId, at: Timestamp) {
        self.entry(venue).record_reject(at);
    }

    pub fn health(&self, venue: &VenueId) -> Option<&VenueHealth> {
        self.venues.get(venue)
    }

    /// Assess a venue, treating one never seen as healthy but unproven.
    pub fn assess(
        &self,
        venue: &VenueId,
        typical_latency: Duration,
        at: Timestamp,
    ) -> HealthAssessment {
        match self.venues.get(venue) {
            Some(health) => health.assess(&self.policy, typical_latency, at),
            None => HealthAssessment {
                venue: venue.clone(),
                verdict: HealthVerdict::Healthy,
                cost_bps_f64: 0.0,
                reject_rate_f64: 0.0,
                observed_latency: typical_latency,
                samples: 0,
            },
        }
    }

    fn entry(&mut self, venue: &VenueId) -> &mut VenueHealth {
        self.venues
            .entry(venue.clone())
            .or_insert_with(|| VenueHealth::new(venue.clone()))
    }
}

/// How many multiples slower than expected a venue is running.
fn latency_excess(observed: Duration, typical: Duration) -> f64 {
    let typical_nanos = typical.as_nanos();
    if typical_nanos <= 0 {
        return 0.0;
    }
    let ratio = observed.as_nanos() as f64 / typical_nanos as f64;
    (ratio - 1.0).max(0.0)
}
