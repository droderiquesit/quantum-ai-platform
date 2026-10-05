//! Before, after and causal ordering between events (REASON-009).
//!
//! Until this module the ordering guarantee was implicit: timestamps compare,
//! and a causal lag is an unsigned duration. That holds, but nothing could be
//! *asked* whether one event came before another, and nothing refused a claim
//! that an effect preceded its cause — it would simply have been stored.
//!
//! A [`Timeline`] answers the question. `a` is before `b` when it occurred
//! strictly earlier, or when a chain of causal links leads from `a` to `b`.
//! The second arm is what orders two events with the same timestamp, and it is
//! also the only way the relation could contradict itself, so
//! [`Timeline::link`] refuses a link whose effect is already ordered before
//! its cause. With that refusal the relation is a strict partial order:
//! irreflexive, asymmetric and transitive.
//!
//! **Point in time.** Every query names the instant it is asked as of, and
//! reads only events and links knowable by then. An event not yet knowable and
//! an event never recorded both answer [`Order::Undecided`]: telling them
//! apart would tell the asker that something is coming.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};

/// Something that happened, with the instant it became knowable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub occurred_at: Timestamp,
    pub knowable_at: Timestamp,
}

/// How two events are ordered, as of the instant asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Order {
    Before,
    After,
    /// Both are known and neither precedes the other.
    Concurrent,
    /// The same event.
    Same,
    /// At least one is not knowable as of the instant asked.
    Undecided,
}

/// Events and the causal links between them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timeline {
    events: BTreeMap<String, Event>,
    /// Cause to `(effect, the instant the link became knowable)`.
    links: BTreeMap<String, BTreeSet<(String, Timestamp)>>,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an event. Refuses a second event under one id, because every
    /// answer already given about the first would silently change.
    pub fn record(&mut self, event: Event) -> Result<()> {
        if self.events.contains_key(&event.id) {
            return Err(Error::invalid(format!(
                "event '{}' is already recorded; record a correction as a new event",
                event.id
            )));
        }
        self.events.insert(event.id.clone(), event);
        Ok(())
    }

    /// Claim that `cause` caused `effect`, knowable from `knowable_at`.
    ///
    /// Refuses a link whose effect is already ordered before its cause — by
    /// time, or through links already held. Judged against everything
    /// recorded rather than as of an instant, so no later query can find the
    /// relation inconsistent.
    pub fn link(&mut self, cause: &str, effect: &str, knowable_at: Timestamp) -> Result<()> {
        for id in [cause, effect] {
            if !self.events.contains_key(id) {
                return Err(Error::invalid(format!(
                    "event '{id}' is not recorded; record it before linking it"
                )));
            }
        }
        if cause == effect || self.precedes(effect, cause, None) {
            return Err(Error::denied(format!(
                "'{effect}' is already ordered before '{cause}', so it cannot be its effect; \
                 correct the timestamps or reverse the link"
            )));
        }
        self.links
            .entry(cause.to_string())
            .or_default()
            .insert((effect.to_string(), knowable_at));
        Ok(())
    }

    /// How `a` and `b` are ordered, using only what was knowable at `as_of`.
    pub fn order(&self, a: &str, b: &str, as_of: Timestamp) -> Order {
        let known = |id: &str| self.events.get(id).is_some_and(|e| e.knowable_at <= as_of);
        if !known(a) || !known(b) {
            Order::Undecided
        } else if a == b {
            Order::Same
        } else if self.precedes(a, b, Some(as_of)) {
            Order::Before
        } else if self.precedes(b, a, Some(as_of)) {
            Order::After
        } else {
            Order::Concurrent
        }
    }

    /// Whether `a` is strictly before `b`. `as_of` of `None` reads everything
    /// recorded; both ids must be recorded.
    fn precedes(&self, a: &str, b: &str, as_of: Option<Timestamp>) -> bool {
        let (Some(first), Some(second)) = (self.events.get(a), self.events.get(b)) else {
            return false;
        };
        if first.occurred_at != second.occurred_at {
            // A causal chain cannot run against time: `link` refused it.
            return first.occurred_at < second.occurred_at;
        }
        // Same instant: only a causal chain orders them.
        let visible = |at: Timestamp| as_of.is_none_or(|t| at <= t);
        let mut seen = BTreeSet::from([a]);
        let mut frontier = vec![a];
        while let Some(current) = frontier.pop() {
            for (next, knowable_at) in self.links.get(current).into_iter().flatten() {
                let reachable = visible(*knowable_at)
                    && self
                        .events
                        .get(next)
                        .is_some_and(|e| visible(e.knowable_at));
                if !reachable {
                    continue;
                }
                if next == b {
                    return true;
                }
                if seen.insert(next) {
                    frontier.push(next);
                }
            }
        }
        false
    }
}
