//! Arbitration between redundant feeds.
//!
//! Venues publish the same stream twice, on independent network paths, precisely
//! because one path will drop packets. Taking one line and ignoring the other
//! throws that away and leaves the cell with a single point of failure that the
//! venue already paid to remove. Arbitration takes whichever copy of each
//! sequence arrives first and discards the rest.
//!
//! Two decisions are worth stating.
//!
//! **The output is stamped with a canonical feed name.** If the A and B copies
//! reached consumers labelled `itch-a` and `itch-b`, they would be two streams
//! by [`qip_contracts::Origin::stream_key`], each missing whatever the other
//! delivered, and every gap detector downstream would see two half-streams full
//! of holes. The arbitrated stream is one stream and is named as one. The
//! message's identifier still records which physical copy was used, which is
//! what an operator needs when reconciling against a capture.
//!
//! **The seen-set is bounded.** It holds a window of recent sequences; anything
//! below the window is treated as already delivered. An unbounded set would grow
//! for the life of the session, and the alternative — no set at all — would let
//! a slow line re-deliver a whole morning.

use qip_contracts::MarketMessage;
use qip_core::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How a line performed, as far as arbitration can tell.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LineHealth {
    pub line: String,
    /// Units this line delivered, whether or not it delivered them first.
    pub delivered: u64,
    /// Units this line delivered first, and which were therefore published.
    pub won: u64,
    /// Units delivered by another line first.
    pub lost: u64,
    /// Units that left the window without this line ever delivering them.
    pub missed: u64,
    /// Missed as a fraction of what the line should have carried. A statistic.
    pub loss_rate_f64: f64,
    /// Mean lag behind the winning line, in nanoseconds, over the units this
    /// line lost. A statistic.
    pub mean_lag_nanos_f64: f64,
    lag_total_nanos: i128,
}

impl LineHealth {
    fn new(line: &str) -> Self {
        Self {
            line: line.to_string(),
            ..Self::default()
        }
    }

    fn recompute(&mut self) {
        let accountable = self.delivered + self.missed;
        self.loss_rate_f64 = if accountable == 0 {
            0.0
        } else {
            self.missed as f64 / accountable as f64
        };
        self.mean_lag_nanos_f64 = if self.lost == 0 {
            0.0
        } else {
            self.lag_total_nanos as f64 / self.lost as f64
        };
    }
}

/// What arbitration decided about a batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ArbitrationEvent {
    /// A line delivered a unit first; this is the copy that was published.
    Published { line: String, sequence: u64 },
    /// A line delivered a unit another line had already published.
    Duplicate {
        line: String,
        sequence: u64,
        lag_nanos: i64,
    },
    /// A unit left the window without this line ever delivering it.
    Missed { line: String, sequence: u64 },
    /// A line delivered something so old the window no longer remembers it.
    ///
    /// Distinguished from a duplicate because it means the line is further
    /// behind than the window is wide, which is a fault rather than jitter.
    BeyondWindow { line: String, sequence: u64 },
}

/// What one line delivered for one sequence.
#[derive(Clone, Debug)]
struct Delivery {
    winner: String,
    at: Timestamp,
    seen_by: BTreeSet<String>,
}

/// The result of offering a batch to the arbiter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArbitrationOutcome {
    /// The copies that were published, relabelled to the canonical feed.
    pub released: Vec<MarketMessage>,
    pub events: Vec<ArbitrationEvent>,
}

/// Merges two or more redundant lines into one stream.
#[derive(Debug)]
pub struct LineArbiter {
    canonical_feed: String,
    window: usize,
    recent: BTreeMap<u64, Delivery>,
    /// Sequences at or below this have left the window and count as delivered.
    floor: Option<u64>,
    lines: BTreeMap<String, LineHealth>,
}

impl LineArbiter {
    /// `canonical_feed` is the feed name the merged stream is published under.
    pub fn new(canonical_feed: impl Into<String>, lines: &[&str], window: usize) -> Self {
        Self {
            canonical_feed: canonical_feed.into(),
            window: window.max(1),
            recent: BTreeMap::new(),
            floor: None,
            lines: lines
                .iter()
                .map(|line| ((*line).to_string(), LineHealth::new(line)))
                .collect(),
        }
    }

    /// Health of every line the arbiter has been told about or has seen.
    pub fn health(&self) -> Vec<LineHealth> {
        self.lines.values().cloned().collect()
    }

    pub fn line_health(&self, line: &str) -> Option<&LineHealth> {
        self.lines.get(line)
    }

    /// How many sequences the window is holding.
    ///
    /// Exposed because the bound is a property worth asserting rather than
    /// trusting: this is the structure that would otherwise grow for the life of
    /// the session.
    pub fn tracked(&self) -> usize {
        self.recent.len()
    }

    /// Which line's copy of `sequence` was published, while it is still in the
    /// window. The question asked whenever a decoded message is reconciled
    /// against a packet capture.
    pub fn winner_of(&self, sequence: u64) -> Option<&str> {
        self.recent
            .get(&sequence)
            .map(|delivery| delivery.winner.as_str())
    }

    /// Offer one line's decoded output.
    pub fn accept(
        &mut self,
        line: &str,
        messages: Vec<MarketMessage>,
        now: Timestamp,
    ) -> ArbitrationOutcome {
        let mut outcome = ArbitrationOutcome::default();
        self.lines
            .entry(line.to_string())
            .or_insert_with(|| LineHealth::new(line));

        for (_, sequence, unit) in crate::tracker::delivery_units(messages) {
            self.accept_unit(line, sequence, unit, now, &mut outcome);
        }
        for health in self.lines.values_mut() {
            health.recompute();
        }
        outcome
    }

    fn accept_unit(
        &mut self,
        line: &str,
        sequence: u64,
        unit: Vec<MarketMessage>,
        now: Timestamp,
        outcome: &mut ArbitrationOutcome,
    ) {
        if let Some(health) = self.lines.get_mut(line) {
            health.delivered += 1;
        }

        if self.floor.is_some_and(|floor| sequence <= floor) {
            if let Some(health) = self.lines.get_mut(line) {
                health.lost += 1;
            }
            outcome.events.push(ArbitrationEvent::BeyondWindow {
                line: line.to_string(),
                sequence,
            });
            return;
        }

        if let Some(delivery) = self.recent.get_mut(&sequence) {
            let lag = now.since(delivery.at);
            delivery.seen_by.insert(line.to_string());
            if let Some(health) = self.lines.get_mut(line) {
                health.lost += 1;
                health.lag_total_nanos += i128::from(lag.as_nanos());
            }
            outcome.events.push(ArbitrationEvent::Duplicate {
                line: line.to_string(),
                sequence,
                lag_nanos: lag.as_nanos(),
            });
            return;
        }

        let mut seen_by = BTreeSet::new();
        seen_by.insert(line.to_string());
        self.recent.insert(
            sequence,
            Delivery {
                winner: line.to_string(),
                at: now,
                seen_by,
            },
        );
        if let Some(health) = self.lines.get_mut(line) {
            health.won += 1;
        }
        outcome.events.push(ArbitrationEvent::Published {
            line: line.to_string(),
            sequence,
        });

        for mut message in unit {
            // One stream in, one stream out. See the module note: leaving the
            // physical line name on the message splits the stream in two.
            message.origin.feed = self.canonical_feed.clone();
            outcome.released.push(message);
        }

        self.evict(outcome);
    }

    /// Drop the oldest deliveries, attributing what each line never sent.
    fn evict(&mut self, outcome: &mut ArbitrationOutcome) {
        while self.recent.len() > self.window {
            let Some((&sequence, _)) = self.recent.iter().next() else {
                break;
            };
            let Some(delivery) = self.recent.remove(&sequence) else {
                break;
            };
            self.floor = Some(sequence);
            for (name, health) in self.lines.iter_mut() {
                if !delivery.seen_by.contains(name) {
                    health.missed += 1;
                    outcome.events.push(ArbitrationEvent::Missed {
                        line: name.clone(),
                        sequence,
                    });
                }
            }
        }
    }
}
