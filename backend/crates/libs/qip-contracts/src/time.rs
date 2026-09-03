//! Bitemporal stamping and stream watermarks.

use qip_core::Timestamp;
use serde::{Deserialize, Serialize};

/// A value with both of its times.
///
/// `valid_at` is when the fact was true in the market. `known_at` is when this
/// platform could first have acted on it. Every read that reasons "as of" a
/// moment filters on `known_at`, never on `valid_at` — filtering on valid-time
/// is exactly the mistake that makes a backtest profitable and a live run not.
///
/// The constructor refuses a value known before it was true, because that
/// combination has no physical meaning and always indicates a clock or a
/// parsing bug rather than a genuinely prescient feed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamped<T> {
    value: T,
    valid_at: Timestamp,
    known_at: Timestamp,
    /// Whether `known_at` was moved forward by [`Stamped::new`] because a feed
    /// claimed a known-time before its valid-time.
    ///
    /// Kept as its own field rather than derived from `known_at == valid_at`:
    /// a fact that was genuinely known the instant it became true —
    /// [`Stamped::immediate`], or a `new` call whose known-time already
    /// equalled valid-time — has that same equality and is not evidence of a
    /// clock or parsing bug. `was_clamped` read that equality directly before
    /// this field existed, so every immediate stamp reported itself as
    /// clamped, and nothing distinguished "arrived instantaneously" from
    /// "arrived late and was corrected".
    #[serde(default)]
    clamped: bool,
}

impl<T> Stamped<T> {
    /// Stamp a value, clamping known-time forward if a feed claims to have
    /// delivered a fact before it happened.
    ///
    /// Clamping rather than refusing: a single bad timestamp must not drop a
    /// message on the floor, and the clamp is visible through
    /// [`Stamped::was_clamped`].
    pub fn new(value: T, valid_at: Timestamp, known_at: Timestamp) -> Self {
        let clamped = known_at < valid_at;
        Self {
            value,
            valid_at,
            known_at: if clamped { valid_at } else { known_at },
            clamped,
        }
    }

    /// Stamp a value that became known at the moment it became true.
    pub fn immediate(value: T, at: Timestamp) -> Self {
        Self {
            value,
            valid_at: at,
            known_at: at,
            clamped: false,
        }
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn into_value(self) -> T {
        self.value
    }

    pub const fn valid_at(&self) -> Timestamp {
        self.valid_at
    }

    pub const fn known_at(&self) -> Timestamp {
        self.known_at
    }

    /// Whether this fact was knowable at `as_of`.
    ///
    /// The only correct predicate for a point-in-time read.
    pub fn was_known_by(&self, as_of: Timestamp) -> bool {
        self.known_at <= as_of
    }

    /// How late the platform learned of this fact.
    pub fn latency(&self) -> qip_core::Duration {
        self.known_at.since(self.valid_at)
    }

    /// Whether known-time had to be clamped to valid-time on construction.
    ///
    /// Not `known_at == valid_at`: a fact stamped [`Stamped::immediate`], or
    /// constructed with an already-equal known-time, has that same equality
    /// without a clamp ever happening, and reporting it as clamped would name
    /// a clock or parsing bug that did not occur.
    pub fn was_clamped(&self) -> bool {
        self.clamped
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Stamped<U> {
        Stamped {
            value: f(self.value),
            valid_at: self.valid_at,
            known_at: self.known_at,
            clamped: self.clamped,
        }
    }
}

/// How far a stream has been consumed.
///
/// A watermark is a promise: everything at or before `position` has been seen,
/// so a consumer may act on the interval without waiting. It is what lets a
/// durable buffer sit in front of the loop without the loop losing its clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watermark {
    pub stream: String,
    /// The highest contiguous sequence consumed. Contiguous, not highest seen:
    /// a watermark past a gap is a promise that was not kept.
    pub position: u64,
    /// The known-time of the message at `position`.
    pub at: Timestamp,
}

impl Watermark {
    pub fn new(stream: impl Into<String>, position: u64, at: Timestamp) -> Self {
        Self {
            stream: stream.into(),
            position,
            at,
        }
    }

    /// Advance to a later position, refusing to move backwards.
    ///
    /// A watermark that can retreat is not a promise, and a consumer that
    /// trusted the earlier value has already acted.
    pub fn advance_to(&mut self, position: u64, at: Timestamp) -> bool {
        if position <= self.position {
            return false;
        }
        self.position = position;
        self.at = at;
        true
    }
}
