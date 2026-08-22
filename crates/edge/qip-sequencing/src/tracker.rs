//! Sequence tracking, reordering and gap handling.
//!
//! A feed delivers messages numbered by the venue. They can arrive out of order,
//! twice, or not at all, and each of those has a different correct response:
//!
//! * **Out of order** — hold the later message until its predecessors arrive.
//!   Applying it immediately would build a book from operations applied in the
//!   wrong order, which no later message corrects.
//! * **Twice** — drop the second copy. A redundant line, a retransmission and a
//!   reconnect all deliver messages the consumer has already applied, and
//!   applying an order-add twice puts size on the book that does not exist.
//! * **Never** — say so. This is the case the whole module exists for. A
//!   consumer trading off a book with a silent hole in it is the failure being
//!   prevented, so an unrecoverable gap produces [`MessageBody::Reset`] for the
//!   stream and the watermark only moves past the hole behind that reset.
//!
//! **The reorder buffer is bounded, and the bound is not a detail.** The event
//! that fills it is a permanent gap — exactly the feed failure this module
//! exists to survive — so an unbounded buffer would turn a recoverable data loss
//! into an out-of-memory kill. When the bound is reached the gap is declared
//! unrecoverable immediately rather than at the deadline; the response is the
//! same, only sooner.
//!
//! **A sequence number identifies a delivery unit, not a message.** One wire
//! message often decodes into several facts — a FIX snapshot into a reset and
//! its levels, an ITCH execution into a reduction and a print — and they share
//! the position of the packet that carried them. Messages are therefore tracked
//! in units of consecutive equal sequence numbers, which is precisely what a
//! decoder's output already is.

use crate::identity::reset_message;
use qip_contracts::{MarketMessage, Origin, Watermark};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How long to wait for a hole to fill, and how much to hold while waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReorderPolicy {
    /// Messages held out of order before the gap is declared unrecoverable.
    pub max_buffered_messages: usize,
    /// How long a gap may stay open before it is declared unrecoverable.
    ///
    /// This is a latency budget, not a hope: everything behind the gap is
    /// invisible to consumers until it closes, so a generous deadline is a
    /// deliberate decision to be blind for that long.
    pub gap_timeout: Duration,
}

impl Default for ReorderPolicy {
    fn default() -> Self {
        Self {
            max_buffered_messages: 1_024,
            gap_timeout: Duration::from_millis(50),
        }
    }
}

impl ReorderPolicy {
    /// A policy holding at most `max_buffered_messages` for `gap_timeout`.
    pub fn new(max_buffered_messages: usize, gap_timeout: Duration) -> Self {
        Self {
            max_buffered_messages,
            gap_timeout,
        }
    }
}

/// Why a gap was given up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    /// The deadline passed with the hole still open.
    Deadline,
    /// The reorder buffer filled while waiting.
    BufferFull,
}

impl GapReason {
    /// A stable label for metrics and log grouping.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Deadline => "deadline",
            Self::BufferFull => "buffer_full",
        }
    }
}

/// What the tracker observed. Every one of these is worth an operator's
/// attention; none of them is worth stopping for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SequenceEvent {
    /// The first message seen on a stream, which sets the starting position.
    StreamStarted { stream: String, sequence: u64 },
    /// A sequence already released, or already held, arrived again.
    Duplicate { stream: String, sequence: u64 },
    /// A message arrived ahead of its predecessors.
    GapOpened {
        stream: String,
        missing_from: u64,
        missing_to: u64,
    },
    /// The hole filled and everything held has been released in order.
    GapFilled {
        stream: String,
        recovered_through: u64,
    },
    /// The hole will not fill. A reset has been emitted for the stream.
    GapAbandoned {
        stream: String,
        missing_from: u64,
        missing_to: u64,
        reason: GapReason,
    },
}

/// Counters for one stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamStats {
    /// Messages handed to consumers.
    pub released: u64,
    /// Delivery units dropped because they had already been released or held.
    pub duplicates: u64,
    /// Holes that opened.
    pub gaps_opened: u64,
    /// Holes that filled before their deadline.
    pub gaps_filled: u64,
    /// Holes given up on, each of which produced a reset.
    pub gaps_abandoned: u64,
    /// Sequences an abandoned gap established were lost.
    pub messages_lost: u64,
    /// The most messages ever held out of order at once.
    pub peak_buffered: usize,
}

/// What one call to the sequencer produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SequencedBatch {
    /// Messages in contiguous order, safe to apply as they stand.
    pub released: Vec<MarketMessage>,
    /// What the tracker observed while producing this batch.
    pub events: Vec<SequenceEvent>,
    /// The updated watermark of every stream this call touched.
    pub watermarks: Vec<Watermark>,
}

impl SequencedBatch {
    /// Whether the call produced nothing at all.
    pub fn is_empty(&self) -> bool {
        self.released.is_empty() && self.events.is_empty()
    }

    fn absorb(&mut self, other: SequencedBatch) {
        self.released.extend(other.released);
        self.events.extend(other.events);
        for watermark in other.watermarks {
            match self
                .watermarks
                .iter_mut()
                .find(|existing| existing.stream == watermark.stream)
            {
                Some(existing) => *existing = watermark,
                None => self.watermarks.push(watermark),
            }
        }
    }
}

/// Tracks one ordered stream, keyed by [`Origin::stream_key`].
#[derive(Debug)]
pub struct SequenceTracker {
    stream: String,
    policy: ReorderPolicy,
    /// The highest sequence released with nothing missing before it.
    contiguous: Option<u64>,
    /// The known-time of the message at `contiguous`, which the watermark
    /// publishes. Known-time rather than venue-time: a watermark is a claim
    /// about what this cell has seen, not about what the market did.
    contiguous_at: Timestamp,
    buffer: BTreeMap<u64, Vec<MarketMessage>>,
    buffered_messages: usize,
    /// When the current hole opened, for the deadline.
    gap_opened_at: Option<Timestamp>,
    /// Kept so a reset can be attributed to the right venue, feed and partition
    /// when there is no message to copy it from.
    template_origin: Option<Origin>,
    stats: StreamStats,
}

impl SequenceTracker {
    /// A tracker for a stream whose starting position is not yet known.
    pub fn new(stream: impl Into<String>, policy: ReorderPolicy) -> Self {
        Self {
            stream: stream.into(),
            policy,
            contiguous: None,
            contiguous_at: Timestamp::EPOCH,
            buffer: BTreeMap::new(),
            buffered_messages: 0,
            gap_opened_at: None,
            template_origin: None,
            stats: StreamStats::default(),
        }
    }

    /// A tracker that already knows where the stream should start.
    ///
    /// For a cell resuming against a durable log: without it the first message
    /// to arrive defines the position, so anything lost while the cell was down
    /// would be invisible instead of being the gap it is.
    pub fn expecting(
        stream: impl Into<String>,
        policy: ReorderPolicy,
        first_sequence: u64,
    ) -> Self {
        let mut tracker = Self::new(stream, policy);
        tracker.contiguous = first_sequence.checked_sub(1);
        tracker
    }

    /// The stream this tracker is responsible for.
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// Counters for this stream.
    pub fn stats(&self) -> StreamStats {
        self.stats
    }

    /// Messages currently held out of order.
    pub fn buffered(&self) -> usize {
        self.buffered_messages
    }

    /// The highest contiguous position, or `None` before the first message.
    pub fn position(&self) -> Option<u64> {
        self.contiguous
    }

    /// Whether a hole is currently open.
    pub fn has_open_gap(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// How far this stream has been consumed.
    ///
    /// The highest *contiguous* position. A watermark past a hole is a promise
    /// that was not kept, and the consumer that believed it has already traded.
    pub fn watermark(&self) -> Option<Watermark> {
        self.contiguous
            .map(|position| Watermark::new(self.stream.clone(), position, self.contiguous_at))
    }

    /// Offer one delivery unit — the messages that shared a sequence number.
    pub fn accept_unit(
        &mut self,
        sequence: u64,
        messages: Vec<MarketMessage>,
        now: Timestamp,
    ) -> SequencedBatch {
        let mut batch = SequencedBatch::default();
        if let Some(origin) = messages.first().map(|message| message.origin.clone()) {
            self.template_origin = Some(origin);
        }

        let Some(contiguous) = self.contiguous else {
            // The first message seen defines the starting position. There is
            // nothing to compare it against, and refusing to start until some
            // configured sequence arrives would mean a cell joining a running
            // feed never starts at all.
            self.contiguous = Some(sequence);
            self.contiguous_at = now;
            self.stats.released += messages.len() as u64;
            batch.released.extend(messages);
            batch.events.push(SequenceEvent::StreamStarted {
                stream: self.stream.clone(),
                sequence,
            });
            self.publish_watermark(&mut batch);
            return batch;
        };

        if sequence <= contiguous || self.buffer.contains_key(&sequence) {
            self.stats.duplicates += 1;
            batch.events.push(SequenceEvent::Duplicate {
                stream: self.stream.clone(),
                sequence,
            });
            return batch;
        }

        if sequence == contiguous + 1 {
            let was_holding = !self.buffer.is_empty();
            self.release(sequence, messages, now, &mut batch);
            self.drain(now, &mut batch);
            if was_holding && self.buffer.is_empty() {
                self.gap_opened_at = None;
                self.stats.gaps_filled += 1;
                batch.events.push(SequenceEvent::GapFilled {
                    stream: self.stream.clone(),
                    recovered_through: self.contiguous.unwrap_or_default(),
                });
            }
        } else {
            let opening = self.buffer.is_empty();
            self.buffered_messages += messages.len();
            self.buffer.insert(sequence, messages);
            self.stats.peak_buffered = self.stats.peak_buffered.max(self.buffered_messages);
            if opening {
                self.gap_opened_at = Some(now);
                self.stats.gaps_opened += 1;
                batch.events.push(SequenceEvent::GapOpened {
                    stream: self.stream.clone(),
                    missing_from: contiguous + 1,
                    missing_to: sequence - 1,
                });
            }
            if self.buffered_messages > self.policy.max_buffered_messages {
                self.abandon(GapReason::BufferFull, now, &mut batch);
            }
        }

        self.publish_watermark(&mut batch);
        batch
    }

    /// Give the tracker the current time so a deadline can pass without any
    /// message arriving to carry it.
    ///
    /// Without this, a stream that goes silent immediately after a gap stays
    /// blocked forever — and a stream going silent is exactly what a gap often
    /// precedes.
    pub fn poll(&mut self, now: Timestamp) -> SequencedBatch {
        let mut batch = SequencedBatch::default();
        let expired = self
            .gap_opened_at
            .is_some_and(|opened| now.since(opened) >= self.policy.gap_timeout);
        if expired && !self.buffer.is_empty() {
            self.abandon(GapReason::Deadline, now, &mut batch);
            self.publish_watermark(&mut batch);
        }
        batch
    }

    fn release(
        &mut self,
        sequence: u64,
        messages: Vec<MarketMessage>,
        now: Timestamp,
        batch: &mut SequencedBatch,
    ) {
        self.contiguous = Some(sequence);
        self.contiguous_at = now;
        self.stats.released += messages.len() as u64;
        batch.released.extend(messages);
    }

    /// Release everything the newly arrived message unblocked.
    ///
    /// Silent about whether the hole closed: the caller knows whether this was a
    /// recovery or a resynchronisation after giving up, and those must not be
    /// reported to an operator as the same thing.
    fn drain(&mut self, now: Timestamp, batch: &mut SequencedBatch) {
        while let Some(next) = self.contiguous.map(|position| position + 1) {
            let Some(messages) = self.buffer.remove(&next) else {
                break;
            };
            self.buffered_messages -= messages.len();
            self.release(next, messages, now, batch);
        }
    }

    /// Declare the hole unrecoverable: warn the consumers, then resynchronise.
    ///
    /// The reset is emitted *before* the held messages are released, and the
    /// watermark only moves afterwards. That ordering is the whole guarantee: a
    /// consumer applying this batch in order is told its book is stale before it
    /// is handed anything that assumes otherwise.
    fn abandon(&mut self, reason: GapReason, now: Timestamp, batch: &mut SequencedBatch) {
        let Some(contiguous) = self.contiguous else {
            return;
        };
        let Some(&resume_at) = self.buffer.keys().next() else {
            return;
        };
        let missing_from = contiguous + 1;
        let missing_to = resume_at - 1;

        if let Some(template) = &self.template_origin {
            let origin = Origin::new(
                template.venue.clone(),
                template.feed.clone(),
                template.partition,
                missing_from,
            );
            batch.released.push(reset_message(
                origin,
                format!(
                    "sequence gap {missing_from}..={missing_to} on {} was not recovered ({})",
                    self.stream,
                    reason.as_str()
                ),
                now,
            ));
        }

        self.stats.gaps_abandoned += 1;
        self.stats.messages_lost += missing_to.saturating_sub(missing_from) + 1;
        batch.events.push(SequenceEvent::GapAbandoned {
            stream: self.stream.clone(),
            missing_from,
            missing_to,
            reason,
        });

        // Resume at the earliest message still held, then release everything
        // that is contiguous from there. Anything still missing after that is a
        // new gap, treated as one.
        self.contiguous = Some(resume_at - 1);
        self.gap_opened_at = None;
        self.drain(now, batch);
        if !self.buffer.is_empty() {
            self.gap_opened_at = Some(now);
        }
    }

    fn publish_watermark(&self, batch: &mut SequencedBatch) {
        if let Some(watermark) = self.watermark() {
            match batch
                .watermarks
                .iter_mut()
                .find(|existing| existing.stream == watermark.stream)
            {
                Some(existing) => *existing = watermark,
                None => batch.watermarks.push(watermark),
            }
        }
    }
}

/// One tracker per stream, and the routing that keeps them apart.
///
/// Sequence numbers are only comparable within a stream: two venues, two feeds
/// of one venue, or two partitions of one feed each number from their own
/// origin. Mixing them would manufacture gaps out of nothing.
#[derive(Debug)]
pub struct Sequencer {
    policy: ReorderPolicy,
    trackers: BTreeMap<String, SequenceTracker>,
}

impl Sequencer {
    /// A sequencer with no streams, applying `policy` to each it discovers.
    pub fn new(policy: ReorderPolicy) -> Self {
        Self {
            policy,
            trackers: BTreeMap::new(),
        }
    }

    /// Every stream seen so far.
    pub fn streams(&self) -> Vec<&str> {
        self.trackers.keys().map(String::as_str).collect()
    }

    /// One stream's tracker, if it has been seen.
    pub fn tracker(&self, stream: &str) -> Option<&SequenceTracker> {
        self.trackers.get(stream)
    }

    /// Every stream's watermark.
    pub fn watermarks(&self) -> Vec<Watermark> {
        self.trackers
            .values()
            .filter_map(SequenceTracker::watermark)
            .collect()
    }

    /// Offer a decoder's output, in the order the decoder produced it.
    pub fn accept(&mut self, messages: Vec<MarketMessage>, now: Timestamp) -> SequencedBatch {
        let mut batch = SequencedBatch::default();
        for (stream, sequence, unit) in delivery_units(messages) {
            let policy = self.policy;
            let tracker = self
                .trackers
                .entry(stream.clone())
                .or_insert_with(|| SequenceTracker::new(stream, policy));
            batch.absorb(tracker.accept_unit(sequence, unit, now));
        }
        batch
    }

    /// Advance every stream's deadline.
    pub fn poll(&mut self, now: Timestamp) -> SequencedBatch {
        let mut batch = SequencedBatch::default();
        for tracker in self.trackers.values_mut() {
            batch.absorb(tracker.poll(now));
        }
        batch
    }
}

/// Split a decoder's output into runs that share a stream and a sequence.
///
/// Consecutive runs rather than a grouping by key: the same sequence appearing
/// twice with something else in between is a re-delivery, and collapsing the two
/// into one unit would hide it from the duplicate check.
pub fn delivery_units(messages: Vec<MarketMessage>) -> Vec<(String, u64, Vec<MarketMessage>)> {
    let mut units: Vec<(String, u64, Vec<MarketMessage>)> = Vec::new();
    for message in messages {
        let stream = message.origin.stream_key();
        let sequence = message.origin.sequence;
        match units.last_mut() {
            Some((last_stream, last_sequence, unit))
                if *last_stream == stream && *last_sequence == sequence =>
            {
                unit.push(message);
            }
            _ => units.push((stream, sequence, vec![message])),
        }
    }
    units
}
