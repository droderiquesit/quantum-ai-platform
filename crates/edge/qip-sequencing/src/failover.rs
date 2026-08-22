//! Switching sources without dropping or double-applying a message.
//!
//! A cell fails over to a backup feed mid-session — the primary went silent, the
//! network path degraded, an operator moved traffic. The backup does not resume
//! where the primary stopped: it is somewhere else in the same sequence space,
//! and both mistakes are easy to make and hard to see.
//!
//! * **Behind.** The backup replays messages already applied. Applying them
//!   again adds size to the book that no order ever placed, and nothing later
//!   removes it. Those are dropped, by sequence, not by content.
//! * **Ahead.** The backup starts past where the primary stopped, so the
//!   messages in between are simply gone. Continuing quietly leaves a book that
//!   is wrong in a way no subsequent message corrects, so the reconciler emits a
//!   [`qip_contracts::MessageBody::Reset`] for the affected stream — the same
//!   response as an unrecoverable gap, because it is the same failure.
//!
//! The reconciler is deliberately about *position*, not about ordering: it
//! decides what has already been applied and what has been lost. Reordering
//! within a source is [`crate::tracker::SequenceTracker`]'s job, and keeping the
//! two apart is what stops a switch from being mistaken for jitter.

use crate::identity::reset_message;
use qip_contracts::{MarketMessage, Origin};
use qip_core::Timestamp;
use serde::{Deserialize, Serialize};

/// What the reconciler did with a unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum FailoverEvent {
    /// A switch was requested; both sources are accepted until it completes.
    SwitchBegun { from: String, to: String },
    /// The backup delivered a message that advanced the position, so it is now
    /// the active source.
    SwitchCompleted {
        from: String,
        to: String,
        at_sequence: u64,
    },
    /// The source re-delivered something already applied.
    AlreadyApplied { source: String, sequence: u64 },
    /// A source that is neither active nor the switch target sent something.
    Ignored { source: String, sequence: u64 },
    /// The new source starts past where the old one stopped. A reset has been
    /// emitted for the stream.
    ResyncRequired {
        source: String,
        missing_from: u64,
        missing_to: u64,
    },
}

/// What one call to [`FailoverReconciler::admit`] produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FailoverOutcome {
    /// Messages safe to apply, in order, with any reset already in place.
    pub applied: Vec<MarketMessage>,
    /// What the reconciler decided, in order.
    pub events: Vec<FailoverEvent>,
}

/// Counters for a stream's failover history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailoverStats {
    /// Switches that completed.
    pub switches: u64,
    /// Delivery units applied.
    pub units_applied: u64,
    /// Units dropped because their sequence was already applied.
    pub units_already_applied: u64,
    /// Units from a source that was neither active nor the switch target.
    pub units_ignored: u64,
    /// Times a source resumed past the last applied position.
    pub resyncs: u64,
    /// Messages a resync established were lost.
    pub messages_lost: u64,
}

/// Keeps one stream's position across a change of source.
#[derive(Debug)]
pub struct FailoverReconciler {
    stream: String,
    active: String,
    /// The source being switched to, accepted alongside the active one until it
    /// delivers something. Both are accepted during the switch because the
    /// primary usually keeps sending right up to the moment it stops, and
    /// refusing it would drop messages the backup has not reached yet.
    target: Option<String>,
    position: Option<u64>,
    stats: FailoverStats,
}

impl FailoverReconciler {
    /// A reconciler for one stream, starting on `active`.
    pub fn new(stream: impl Into<String>, active: impl Into<String>) -> Self {
        Self {
            stream: stream.into(),
            active: active.into(),
            target: None,
            position: None,
            stats: FailoverStats::default(),
        }
    }

    /// Start from a known position, when resuming against a durable log.
    pub fn resuming_at(mut self, position: u64) -> Self {
        self.position = Some(position);
        self
    }

    /// The stream this reconciler is responsible for.
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// The source currently being applied.
    pub fn active(&self) -> &str {
        &self.active
    }

    /// The highest sequence applied, or `None` before the first message.
    pub fn position(&self) -> Option<u64> {
        self.position
    }

    /// Counters for this stream's failover history.
    pub fn stats(&self) -> FailoverStats {
        self.stats
    }

    /// Whether a switch has been requested and not yet completed.
    pub fn is_switching(&self) -> bool {
        self.target.is_some()
    }

    /// Request a switch to `to`.
    ///
    /// Nothing changes until `to` delivers a message that advances the position.
    /// A switch that completed on the request alone would strand the stream if
    /// the backup were also down, which is not a rare combination — the two
    /// sources usually fail for related reasons.
    pub fn begin_switch(&mut self, to: impl Into<String>) -> FailoverEvent {
        let to = to.into();
        self.target = Some(to.clone());
        FailoverEvent::SwitchBegun {
            from: self.active.clone(),
            to,
        }
    }

    /// Offer a source's decoded output.
    pub fn admit(
        &mut self,
        source: &str,
        messages: Vec<MarketMessage>,
        now: Timestamp,
    ) -> FailoverOutcome {
        let mut outcome = FailoverOutcome::default();
        for (_, sequence, unit) in crate::tracker::delivery_units(messages) {
            self.admit_unit(source, sequence, unit, now, &mut outcome);
        }
        outcome
    }

    fn admit_unit(
        &mut self,
        source: &str,
        sequence: u64,
        unit: Vec<MarketMessage>,
        now: Timestamp,
        outcome: &mut FailoverOutcome,
    ) {
        let is_active = source == self.active;
        let is_target = self.target.as_deref() == Some(source);
        if !is_active && !is_target {
            self.stats.units_ignored += 1;
            outcome.events.push(FailoverEvent::Ignored {
                source: source.to_string(),
                sequence,
            });
            return;
        }

        if self.position.is_some_and(|position| sequence <= position) {
            self.stats.units_already_applied += 1;
            outcome.events.push(FailoverEvent::AlreadyApplied {
                source: source.to_string(),
                sequence,
            });
            return;
        }

        if let (Some(position), Some(origin)) = (
            self.position,
            unit.first().map(|message| message.origin.clone()),
        ) && sequence > position + 1
        {
            let missing_from = position + 1;
            let missing_to = sequence - 1;
            self.stats.resyncs += 1;
            self.stats.messages_lost += missing_to - missing_from + 1;
            outcome.applied.push(reset_message(
                Origin::new(
                    origin.venue.clone(),
                    origin.feed.clone(),
                    origin.partition,
                    missing_from,
                ),
                format!(
                    "{source} resumed {} at {sequence}, past the last applied position {position}",
                    self.stream
                ),
                now,
            ));
            outcome.events.push(FailoverEvent::ResyncRequired {
                source: source.to_string(),
                missing_from,
                missing_to,
            });
        }

        self.position = Some(sequence);
        self.stats.units_applied += 1;
        outcome.applied.extend(unit);

        if is_target && !is_active {
            let from = std::mem::replace(&mut self.active, source.to_string());
            self.target = None;
            self.stats.switches += 1;
            outcome.events.push(FailoverEvent::SwitchCompleted {
                from,
                to: source.to_string(),
                at_sequence: sequence,
            });
        }
    }
}
