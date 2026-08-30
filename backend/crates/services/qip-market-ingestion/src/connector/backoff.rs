//! Bounded exponential backoff with jitter, and the decision to stop.
//!
//! Built on `qip_transport::RetryPolicy` rather than beside it. Two backoff
//! implementations in one process are two ladders that disagree during the
//! outage they both exist for, and the manifest's retry stanza is already that
//! policy in JSON.
//!
//! What this module adds is the part a connector needs and the transport's
//! publisher does not: a source that answers `429 retry-after: 30` has told us
//! when it will serve again, and a computed ladder that ignores that is a
//! client arguing with a rate limiter it is going to lose to. So the *longer*
//! of the two waits wins.
//!
//! # Why the jitter subtracts, restated because it is load-bearing here
//!
//! [`qip_transport::RetryPolicy::backoff`] jitters downward. Adding jitter on
//! top of a capped exponential means the cap is not a cap, and a maximum
//! backoff that can be exceeded is a number that gets quoted in a runbook and
//! is then wrong. [`a_backoff_never_exceeds_the_manifest_ceiling`] in
//! `tests/connector_runtime.rs` is what holds that.
//!
//! # Why an attempt count is bounded
//!
//! An unbounded retry is a poll loop that never reports a dead source. The
//! ladder ends, the batch is dead-lettered through
//! [`crate::connector::quarantine`], and the failure becomes visible instead
//! of becoming a queue.

use qip_core::Duration;
use qip_core::error::Result;
use qip_core::rng::Xoshiro256;
use qip_transport::RetryPolicy;

/// Why an attempt failed, which decides whether another one is worth making.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// The source is up and could serve later — a 5xx, a dropped connection,
    /// a read timeout.
    Transient { detail: String },
    /// The source is asking for less traffic, and may have said for how long.
    RateLimited {
        retry_after: Option<Duration>,
        detail: String,
    },
    /// Retrying cannot help — a rejected credential, a 404, a body this
    /// connector cannot decode. Retrying one of these is a client hammering a
    /// source over a configuration mistake.
    Permanent { detail: String },
}

impl FailureKind {
    pub fn detail(&self) -> &str {
        match self {
            Self::Transient { detail }
            | Self::Permanent { detail }
            | Self::RateLimited { detail, .. } => detail,
        }
    }

    pub const fn is_retryable(&self) -> bool {
        !matches!(self, Self::Permanent { .. })
    }
}

/// What to do after a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryDecision {
    /// Wait this long, then make attempt number `attempt`.
    Retry { after: Duration, attempt: u32 },
    /// Stop. The reason is carried so the dead letter can quote it rather than
    /// recording "retries exhausted", which says nothing about the source.
    GiveUp { attempts: u32, reason: String },
}

impl RetryDecision {
    pub const fn is_retry(&self) -> bool {
        matches!(self, Self::Retry { .. })
    }

    pub fn wait(&self) -> Option<Duration> {
        match self {
            Self::Retry { after, .. } => Some(*after),
            Self::GiveUp { .. } => None,
        }
    }
}

/// One source's retry state: the policy, a seeded RNG, and how far up the
/// ladder this failure sequence has climbed.
///
/// The RNG is seeded and held rather than drawn from the environment, so "the
/// third attempt waited 3.4 seconds" is a testable statement rather than a
/// thing that was true once. Two connectors with different seeds spread; the
/// same connector replayed reproduces its schedule exactly.
#[derive(Debug)]
pub struct BackoffLadder {
    policy: RetryPolicy,
    rng: Xoshiro256,
    attempt: u32,
}

impl BackoffLadder {
    pub fn new(policy: RetryPolicy, seed: u64) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            policy,
            rng: Xoshiro256::seeded(seed),
            attempt: 0,
        })
    }

    /// Failures recorded since the last success.
    pub const fn attempts(&self) -> u32 {
        self.attempt
    }

    pub const fn policy(&self) -> RetryPolicy {
        self.policy
    }

    /// A success clears the ladder, so the next outage starts at the bottom.
    ///
    /// Without this a source that fails once an hour would eventually be
    /// waiting the maximum backoff after a single failure, which looks from
    /// the outside like a source that has become slow.
    pub fn succeeded(&mut self) {
        self.attempt = 0;
    }

    /// Record a failure and say what happens next.
    pub fn failed(&mut self, failure: &FailureKind) -> RetryDecision {
        self.attempt = self.attempt.saturating_add(1);
        if !failure.is_retryable() {
            return RetryDecision::GiveUp {
                attempts: self.attempt,
                reason: format!(
                    "not retryable, and retrying would be this client hammering the source over \
                     a mistake of ours: {}",
                    failure.detail()
                ),
            };
        }
        if !self.policy.may_retry(self.attempt) {
            return RetryDecision::GiveUp {
                attempts: self.attempt,
                reason: format!(
                    "{} attempts is the manifest's limit and the source still fails: {}",
                    self.policy.max_attempts,
                    failure.detail()
                ),
            };
        }
        let computed = self.policy.backoff(self.attempt, &mut self.rng);
        // The source's own `retry-after` wins when it is longer. Our ladder is
        // a guess about when the source will serve again; the source's header
        // is not.
        let after = match failure {
            FailureKind::RateLimited {
                retry_after: Some(asked),
                ..
            } if *asked > computed => *asked,
            _ => computed,
        };
        RetryDecision::Retry {
            after,
            attempt: self.attempt + 1,
        }
    }
}
