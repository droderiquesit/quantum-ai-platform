//! Staying inside what a source will tolerate being asked.
//!
//! A token bucket over an *injected* instant. There is no clock here for the
//! same reason there is none anywhere else in this platform: a limiter that
//! read the wall clock could not be replayed, and the first time that matters
//! is the incident review where nobody can reproduce how many requests went
//! out.
//!
//! # Why integer arithmetic
//!
//! Tokens are fixed-point over `i128`, not `f64`. A limiter whose state
//! depends on floating-point rounding is a limiter that admits a different
//! number of requests on a replay than it did live, and the failure that
//! prevents is a rate-limit ban that cannot be reproduced from the log.
//!
//! # What this does not do
//!
//! It does not sleep. [`RateLimiter::admit`] returns
//! [`Admission::Deferred`] naming the instant the next token exists, and the
//! caller decides whether to wait, to skip the poll or to shed. A limiter that
//! blocked would hide backpressure inside a function call.

use super::manifest::RateLimitSpec;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};

/// Fixed-point scale for a token. One token is [`TOKEN`] units.
const TOKEN: i128 = 1_000_000;

/// Whether a request may go out now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Spend a token and make the request.
    Allowed,
    /// Not yet. `until` is the earliest instant a token exists, and `wait` is
    /// how far away that is from the instant asked about.
    Deferred { until: Timestamp, wait: Duration },
}

impl Admission {
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    pub fn wait(&self) -> Duration {
        match self {
            Self::Allowed => Duration::ZERO,
            Self::Deferred { wait, .. } => *wait,
        }
    }
}

/// A token bucket sized by a manifest's rate-limit stanza.
#[derive(Clone, Debug)]
pub struct RateLimiter {
    spec: RateLimitSpec,
    /// Tokens available, scaled by [`TOKEN`].
    tokens: i128,
    /// The instant [`Self::tokens`] was true at. `None` before the first
    /// question, so a limiter starts full rather than pinned to an epoch.
    last: Option<Timestamp>,
    /// A floor the source itself imposed, from a `retry-after`. Independent
    /// of the bucket: a source can ask for a pause longer than its own
    /// published rate implies, and honouring only the bucket would ignore it.
    barrier: Option<Timestamp>,
    admitted: u64,
    deferred: u64,
}

impl RateLimiter {
    pub fn new(spec: RateLimitSpec) -> Result<Self> {
        spec.validate()?;
        Ok(Self {
            tokens: i128::from(spec.burst) * TOKEN,
            spec,
            last: None,
            barrier: None,
            admitted: 0,
            deferred: 0,
        })
    }

    pub const fn spec(&self) -> RateLimitSpec {
        self.spec
    }

    pub const fn admitted(&self) -> u64 {
        self.admitted
    }

    pub const fn deferred(&self) -> u64 {
        self.deferred
    }

    /// Whole tokens available at `at`, without spending one.
    pub fn available(&self, at: Timestamp) -> u32 {
        let tokens = self.refilled(at);
        u32::try_from(tokens / TOKEN).unwrap_or(u32::MAX)
    }

    /// The source asked for a pause. Nothing goes out before `until`.
    ///
    /// Recorded separately from the bucket so that a `retry-after` longer than
    /// the bucket's own recovery is honoured rather than averaged away.
    pub fn pause_until(&mut self, until: Timestamp) {
        self.barrier = Some(match self.barrier {
            Some(existing) if existing > until => existing,
            _ => until,
        });
    }

    /// Ask whether a request may go out at `at`, spending a token if so.
    pub fn admit(&mut self, at: Timestamp) -> Admission {
        if let Some(barrier) = self.barrier {
            if at < barrier {
                self.deferred = self.deferred.saturating_add(1);
                return Admission::Deferred {
                    until: barrier,
                    wait: barrier.since(at),
                };
            }
            self.barrier = None;
        }
        let tokens = self.refilled(at);
        if tokens >= TOKEN {
            self.tokens = tokens - TOKEN;
            self.last = Some(at);
            self.admitted = self.admitted.saturating_add(1);
            return Admission::Allowed;
        }
        self.tokens = tokens;
        self.last = Some(at);
        self.deferred = self.deferred.saturating_add(1);
        let deficit = TOKEN - tokens;
        let wait = self.nanos_for(deficit);
        let until = at.saturating_add(wait);
        Admission::Deferred { until, wait }
    }

    /// Tokens at `at`, capped at the burst. Never spends.
    fn refilled(&self, at: Timestamp) -> i128 {
        let ceiling = i128::from(self.spec.burst) * TOKEN;
        let Some(last) = self.last else {
            return ceiling;
        };
        let elapsed = i128::from(at.since(last).as_nanos().max(0));
        let window = i128::from(self.spec.per().as_nanos().max(1));
        let gained = elapsed * i128::from(self.spec.requests) * TOKEN / window;
        (self.tokens + gained).min(ceiling).max(0)
    }

    /// How long it takes to accrue `tokens` scaled units.
    fn nanos_for(&self, tokens: i128) -> Duration {
        let window = i128::from(self.spec.per().as_nanos().max(1));
        let rate = i128::from(self.spec.requests).max(1);
        // Round up: rounding down would return an instant at which the token
        // still does not exist, and the caller would spin.
        let nanos = (tokens * window + (rate * TOKEN - 1)) / (rate * TOKEN);
        Duration::from_nanos(i64::try_from(nanos).unwrap_or(i64::MAX))
    }
}
