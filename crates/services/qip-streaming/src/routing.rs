//! Which path an event takes, and why it is not a free choice.
//!
//! Two transports exist and they are not interchangeable:
//!
//! * The **local** path is an in-process handoff. No hashing, no append, no
//!   buffer beyond the queue the caller drains in the same tick. It is bounded
//!   and it drops the oldest entry when it fills.
//! * The **durable** path is `qip_events::EventLog` — append-only, hash-chained,
//!   optionally file-backed. Nothing is lost and everything is replayable, at
//!   the cost of a hash and a write per event.
//!
//! A routing class is therefore a claim about what an event can survive, and
//! two of those claims are checked rather than trusted:
//!
//! 1. **A venue-critical market-data event must not traverse the durable
//!    buffer.** The entire reason the edge exists is that the interval between
//!    a quote changing and a decision being made is measured in microseconds;
//!    a hash and an append in that interval is a self-inflicted latency budget.
//!    The tick is also replaceable — `qip_events::Topic::is_lossy_tolerable`
//!    already says so — so the loss the local path risks is a loss that costs
//!    nothing.
//! 2. **An event that cannot be lost must not take the local path.** The same
//!    predicate, read the other way: an order fill, a risk decision or a
//!    hypothesis is not replaceable, and a bounded queue that drops its oldest
//!    entry under load would drop exactly those under exactly the conditions
//!    that produce them.
//!
//! Both rules are enforced in [`RoutingClass::check`], which
//! [`crate::StreamEnvelope::seal`] calls. An envelope in hand is therefore one
//! whose routing class does not contradict its content — the type is the
//! evidence, not a convention someone has to remember.
//!
//! This is deliberately *not* `qip_data_finder::RoutingClass`, which decides
//! how eagerly to poll a **source**. This one decides which wire a single
//! **event** goes down. They share three words and nothing else.

use qip_core::error::{Error, Result};
use qip_events::Topic;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::provenance::SourceType;

/// Which transport an event actually travels on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportPath {
    /// In-process, bounded, lossy under overload, no durability.
    Local,
    /// Append-only, hash-chained, replayable.
    Durable,
}

impl TransportPath {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Durable => "durable",
        }
    }

    /// Whether an event on this path survives the process that carried it.
    pub const fn is_durable(&self) -> bool {
        matches!(self, Self::Durable)
    }
}

impl fmt::Display for TransportPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How urgently an event must move, and therefore where it may move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingClass {
    /// Microseconds matter and the event is replaceable. Local path only.
    Hot,
    /// Must not be lost; seconds of buffering are acceptable. Durable path.
    Warm,
    /// Kept for history and research. Durable path, and nothing waits on it.
    Cold,
}

impl RoutingClass {
    pub const ALL: [Self; 3] = [Self::Hot, Self::Warm, Self::Cold];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Warm => "warm",
            Self::Cold => "cold",
        }
    }

    /// The transport this class travels on.
    ///
    /// Total and `const`: there is exactly one path per class, so a caller
    /// cannot route a hot event durably by picking a different publisher.
    pub const fn path(&self) -> TransportPath {
        match self {
            Self::Hot => TransportPath::Local,
            Self::Warm | Self::Cold => TransportPath::Durable,
        }
    }

    /// Whether a consumer may sit behind a buffer for this class.
    pub const fn tolerates_buffering(&self) -> bool {
        !matches!(self, Self::Hot)
    }

    /// The class an event of this shape must carry.
    ///
    /// Derived rather than chosen, so two call sites cannot classify the same
    /// event differently. `Cold` is never derived: whether history is worth
    /// keeping warm is a retention decision that this function has no way to
    /// make, so a caller who wants it says so and [`Self::check`] agrees.
    pub fn derive(source_type: SourceType, topic: Topic) -> Self {
        if source_type.is_venue_critical()
            && topic.group().is_latency_critical()
            && topic.is_lossy_tolerable()
        {
            Self::Hot
        } else {
            Self::Warm
        }
    }

    /// Refuse a routing class that contradicts the event it is attached to.
    ///
    /// The two rules never overlap: the first applies only to topics that
    /// cannot be lost, the second only to topics that can.
    pub fn check(&self, source_type: SourceType, topic: Topic) -> Result<()> {
        if !topic.is_lossy_tolerable() && matches!(self, Self::Hot) {
            return Err(Error::invalid(format!(
                "{} may not be routed hot: the local path is bounded and drops its oldest entry \
                 under load, and losing one of these corrupts the record it belongs to",
                topic.name()
            )));
        }
        if source_type.is_venue_critical()
            && topic.group().is_latency_critical()
            && topic.is_lossy_tolerable()
            && !matches!(self, Self::Hot)
        {
            return Err(Error::invalid(format!(
                "{} from a venue feed may not be routed {}: a durable append on the venue-critical \
                 path spends the latency budget the edge exists to protect, and the event is \
                 replaceable within milliseconds",
                topic.name(),
                self.as_str()
            )));
        }
        Ok(())
    }
}

impl fmt::Display for RoutingClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
