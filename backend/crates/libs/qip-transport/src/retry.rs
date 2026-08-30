//! Bounded exponential backoff, computed rather than felt.
//!
//! # Why the jitter is seeded and the sleep is injected
//!
//! Retry logic is where a distributed system's worst behaviour lives, and it is
//! also the hardest thing to test, because the two things it is made of —
//! randomness and waiting — are exactly the two ambient dependencies this
//! platform forbids everywhere else. So neither is ambient here either:
//!
//! * The jitter comes from a seeded [`Xoshiro256`] held by the publisher. Two
//!   runs from the same seed produce the same schedule, which is what makes
//!   "the third attempt waited 3.4 seconds" a testable statement rather than a
//!   thing that was true once.
//! * The waiting goes through [`Sleeper`]. In production that is
//!   [`ThreadSleeper`] and it really sleeps; under test it is
//!   [`RecordingSleeper`], which records what it was asked to wait and returns
//!   immediately. A test suite that had to spend the backoff in real seconds
//!   would be a test suite that stops asserting on the backoff.
//!
//! # Why jitter subtracts
//!
//! [`RetryPolicy::backoff`] jitters *downward* from the exponential value,
//! never upward. Adding jitter on top of a capped exponential means the cap is
//! not a cap, and a maximum backoff that can be exceeded is a number that will
//! be quoted in a runbook and then be wrong. Subtracting keeps
//! [`RetryPolicy::max_backoff`] a real ceiling while still spreading the herd:
//! nine regional cells reconnecting to the same central plane after a rollout
//! must not all retry on the same millisecond.
//!
//! # What backoff does not fix
//!
//! Backoff spreads retries; it does not create capacity. If the peer is down
//! for longer than `max_attempts` ladders take, messages go to the dead-letter
//! path — see [`crate::deadletter`]. That is the honest failure, and it is why
//! the attempt count is bounded: an unbounded retry is a queue that grows until
//! the process dies, with no record of what was in it.

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Duration, Error, Result};
use std::sync::Mutex;
use std::time::Duration as StdDuration;

/// Somewhere to spend a backoff interval.
pub trait Sleeper: std::fmt::Debug + Send + Sync {
    fn sleep(&self, duration: Duration);
}

/// The production sleeper: it blocks the calling thread.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, duration: Duration) {
        let nanos = duration.as_nanos();
        if nanos > 0 {
            std::thread::sleep(StdDuration::from_nanos(nanos as u64));
        }
    }
}

/// A sleeper that records instead of waiting.
///
/// Shipped rather than confined to `#[cfg(test)]` because the tests that assert
/// on the retry ladder live in `tests/`, which links the crate as a consumer
/// would. A test that cannot see the schedule ends up asserting only that a
/// retry happened, which is the part that was never in doubt.
#[derive(Debug, Default)]
pub struct RecordingSleeper {
    slept: Mutex<Vec<Duration>>,
}

impl RecordingSleeper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every interval this sleeper was asked to wait, in order.
    pub fn recorded(&self) -> Vec<Duration> {
        self.slept
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// What the wall clock would have spent.
    pub fn total(&self) -> Duration {
        self.recorded()
            .into_iter()
            .fold(Duration::ZERO, |total, next| total + next)
    }
}

impl Sleeper for RecordingSleeper {
    fn sleep(&self, duration: Duration) {
        self.slept
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(duration);
    }
}

/// How many times to try, how long to wait, and how much to spread it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total sends, including the first. `1` means no retry at all.
    pub max_attempts: u32,
    /// The wait after the first failure.
    pub initial_backoff: Duration,
    /// The ceiling, which [`Self::backoff`] never exceeds.
    pub max_backoff: Duration,
    /// What each successive backoff is multiplied by.
    pub multiplier: u32,
    /// How far below the exponential value the jitter may pull, in basis
    /// points. `2_500` means "up to 25% earlier".
    ///
    /// Basis points rather than a fraction so the whole computation is integer
    /// arithmetic: a float here would make the schedule depend on rounding, and
    /// a replay that diverges in its retry timing is a replay that diverges.
    pub jitter_basis_points: u32,
}

impl Default for RetryPolicy {
    /// Five attempts over roughly six seconds.
    ///
    /// Tuned for a peer inside the same VPC: a rolling pod restart is seconds,
    /// so a ladder that gives up in under one is a dead letter for an outage
    /// that had already ended, and one that gives up in minutes is a queue
    /// holding stale state deltas nobody wants any more.
    fn default() -> Self {
        Self {
            max_attempts: 5,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_secs(5),
            multiplier: 4,
            jitter_basis_points: 2_500,
        }
    }
}

impl RetryPolicy {
    /// Refuse a policy that cannot do what it says.
    ///
    /// Checked at construction rather than at the first failure: a zero
    /// attempt count or a negative backoff is a configuration mistake that
    /// would otherwise surface as messages silently never being sent, during
    /// the incident the retry existed for.
    pub fn validate(&self) -> Result<()> {
        if self.max_attempts == 0 {
            return Err(Error::invalid(
                "a retry policy with zero attempts never sends anything",
            ));
        }
        if self.multiplier == 0 {
            return Err(Error::invalid(
                "a retry multiplier of zero collapses every backoff to nothing",
            ));
        }
        if self.initial_backoff.as_nanos() < 0 || self.max_backoff.as_nanos() < 0 {
            return Err(Error::invalid("a backoff interval cannot be negative"));
        }
        if self.initial_backoff > self.max_backoff {
            return Err(Error::invalid(
                "the initial backoff is longer than the maximum, so the maximum is not a maximum",
            ));
        }
        if self.jitter_basis_points > 10_000 {
            return Err(Error::invalid(
                "jitter above 10000 basis points would pull a backoff below zero",
            ));
        }
        Ok(())
    }

    /// How long to wait after `attempt` failed. `attempt` is 1-based.
    ///
    /// Never exceeds [`Self::max_backoff`], never negative, and depends only on
    /// the policy, the attempt number and the RNG's state.
    pub fn backoff(&self, attempt: u32, rng: &mut Xoshiro256) -> Duration {
        let ceiling = self.max_backoff.as_nanos().max(0) as u128;
        let mut nanos = self.initial_backoff.as_nanos().max(0) as u128;
        for _ in 1..attempt.max(1) {
            nanos = nanos.saturating_mul(u128::from(self.multiplier));
            if nanos >= ceiling {
                break;
            }
        }
        let capped = nanos.min(ceiling);
        let span = capped * u128::from(self.jitter_basis_points) / 10_000;
        // `below(n)` draws from `[0, n)`, so `span + 1` makes the full span
        // reachable and a zero span still draws zero.
        let drawn = u128::from(rng.below((span + 1).min(u128::from(u64::MAX)) as u64));
        Duration::from_nanos((capped - drawn.min(capped)) as i64)
    }

    /// Whether another send is permitted after `attempt` failed.
    pub const fn may_retry(&self, attempt: u32) -> bool {
        attempt < self.max_attempts
    }

    /// The longest this policy can spend before giving up, ignoring the time
    /// the sends themselves take. For a runbook, and for a caller deciding
    /// whether a synchronous publish can block for that long.
    pub fn worst_case_backoff(&self) -> Duration {
        let mut total = Duration::ZERO;
        let mut nanos = self.initial_backoff.as_nanos().max(0) as u128;
        let ceiling = self.max_backoff.as_nanos().max(0) as u128;
        for _ in 1..self.max_attempts {
            total = total + Duration::from_nanos(nanos.min(ceiling) as i64);
            nanos = nanos.saturating_mul(u128::from(self.multiplier));
        }
        total
    }
}
