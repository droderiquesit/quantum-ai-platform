//! Gap detection over envelopes, by composing `qip-sequencing`.
//!
//! There is no gap detection in this file. `qip_sequencing::Sequencer` already
//! reorders within a bounded window, drops duplicates, opens and closes holes,
//! gives up on a deadline or a full buffer, emits a reset when it does, and
//! publishes a watermark that is the highest *contiguous* position rather than
//! the highest seen. A second implementation of that would be a second set of
//! off-by-one errors, and the two would disagree the first time either was
//! fixed.
//!
//! What this file does is adapt. The tracker speaks `MarketMessage`; the spine
//! speaks [`StreamEnvelope`]. So each sequenced envelope is offered to the
//! tracker as a **carrier** — a `MarketMessage` bearing the envelope's
//! `qip_contracts::Origin` and the envelope's own identity — and whatever the
//! tracker releases is translated straight back into the envelope it stood for.
//!
//! # About the carrier body
//!
//! The tracker reads only `MarketMessage::origin`; the body is never
//! inspected. A body is still required, and the choice is deliberate:
//! `MessageBody::Reset` is the only variant that makes no claim about a price,
//! a size or a venue's state. Carriers never leave this module — every released
//! carrier is replaced by the envelope it identifies before anything else sees
//! it — but if that invariant were ever broken, the worst a leaked carrier
//! could do is tell a consumer to resynchronise. A carrier claiming a price
//! could do considerably more.
//!
//! A carrier is distinguished from the tracker's *own* synthesised reset by
//! identity rather than by body: a carrier's object id is the envelope's event
//! id, and `qip_sequencing::synthetic_id` cannot produce one of those.
//!
//! # Unsequenced sources bypass this entirely
//!
//! An envelope with no `Origin` is released immediately. Running a sequence
//! tracker over a source that numbers nothing would manufacture gaps out of
//! arrival order — the same warning `qip_sequencing::Sequencer` gives about
//! mixing two venues' sequence spaces.

use qip_contracts::time::Watermark;
use qip_contracts::{MarketMessage, MessageBody};
use qip_core::Timestamp;
use qip_core::ids::ObjectKind;
use qip_sequencing::{ReorderPolicy, SequenceEvent, Sequencer};
use std::collections::BTreeMap;

use crate::envelope::StreamEnvelope;

/// The tracker gave up on a hole and told consumers to resynchronise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamReset {
    pub stream: String,
    pub reason: String,
}

/// What one pass through the coordinator produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SequencedEnvelopes {
    /// Envelopes in contiguous order, safe to hand on.
    pub released: Vec<StreamEnvelope>,
    /// Exactly what `qip_sequencing` observed, unmodified.
    pub events: Vec<SequenceEvent>,
    /// Every touched stream's watermark, at its highest contiguous position.
    pub watermarks: Vec<Watermark>,
    /// Resynchronisation notices for streams whose holes were abandoned.
    pub resets: Vec<StreamReset>,
}

impl SequencedEnvelopes {
    pub fn is_empty(&self) -> bool {
        self.released.is_empty() && self.events.is_empty() && self.resets.is_empty()
    }
}

/// Holds envelopes while `qip_sequencing` decides whether they are in order.
///
/// Memory is bounded by the tracker's own bound: every admitted envelope is
/// either released, dropped as a duplicate, or held by the tracker, and the
/// tracker holds at most `ReorderPolicy::max_buffered_messages` before
/// declaring the gap unrecoverable.
#[derive(Debug)]
pub struct SequenceCoordinator {
    sequencer: Sequencer,
    /// Envelopes awaiting release, keyed by event id.
    pending: BTreeMap<String, StreamEnvelope>,
}

impl SequenceCoordinator {
    pub fn new(policy: ReorderPolicy) -> Self {
        Self {
            sequencer: Sequencer::new(policy),
            pending: BTreeMap::new(),
        }
    }

    /// Every stream's watermark, at its highest contiguous position.
    pub fn watermarks(&self) -> Vec<Watermark> {
        self.sequencer.watermarks()
    }

    /// Envelopes currently held behind a hole.
    pub fn held(&self) -> usize {
        self.pending.len()
    }

    /// Offer envelopes in arrival order.
    pub fn admit(&mut self, envelopes: Vec<StreamEnvelope>, now: Timestamp) -> SequencedEnvelopes {
        let mut outcome = SequencedEnvelopes::default();
        let mut carriers = Vec::new();
        // (event id, stream, sequence) for everything offered in this call, so
        // a duplicate can be matched to the copy that was rejected rather than
        // to the copy the tracker is already holding.
        let mut arrivals: Vec<(String, String, u64)> = Vec::new();

        for envelope in envelopes {
            match carrier(&envelope) {
                Some(message) => {
                    let stream = message.origin.stream_key();
                    let sequence = message.origin.sequence;
                    let id = envelope.event_id().as_str().to_string();
                    arrivals.push((id.clone(), stream, sequence));
                    self.pending.insert(id, envelope);
                    carriers.push(message);
                }
                // Unsequenced: nothing to detect a gap in.
                None => outcome.released.push(envelope),
            }
        }

        if !carriers.is_empty() {
            let batch = self.sequencer.accept(carriers, now);
            self.absorb(batch, &mut arrivals, &mut outcome);
        }
        outcome
    }

    /// Let a deadline pass without a message arriving to carry it.
    ///
    /// A stream that goes silent immediately after a gap would otherwise stay
    /// blocked forever, and going silent is exactly what a gap often precedes.
    /// `until` rather than an ambient clock, for the same reason
    /// `qip_market_ingestion::adapter::DataAdapter::poll` takes one.
    pub fn poll(&mut self, until: Timestamp) -> SequencedEnvelopes {
        let mut outcome = SequencedEnvelopes::default();
        let batch = self.sequencer.poll(until);
        let mut arrivals = Vec::new();
        self.absorb(batch, &mut arrivals, &mut outcome);
        outcome
    }

    /// Translate a tracker batch back into envelopes.
    fn absorb(
        &mut self,
        batch: qip_sequencing::SequencedBatch,
        arrivals: &mut Vec<(String, String, u64)>,
        outcome: &mut SequencedEnvelopes,
    ) {
        for message in batch.released {
            let id = message.object_id.as_str();
            match self.pending.remove(id) {
                Some(envelope) => {
                    arrivals.retain(|(arrival, _, _)| arrival != id);
                    outcome.released.push(envelope);
                }
                // Not one of ours: the tracker synthesised this reset itself
                // because a hole would not fill.
                None => {
                    if let MessageBody::Reset { reason } = &message.body {
                        outcome.resets.push(StreamReset {
                            stream: message.origin.stream_key(),
                            reason: reason.clone(),
                        });
                    }
                }
            }
        }

        for event in &batch.events {
            // The *last* offered copy, not the first. Within one call the
            // tracker releases or holds the earliest copy of a sequence and
            // drops the later ones, so matching forwards charged the duplicate
            // to the copy the tracker was still holding: that envelope left
            // `pending`, and when the hole filled the tracker released a
            // carrier nothing could be matched to. The envelope was lost, its
            // carrier's `Reset` body was reported to consumers as an order to
            // resynchronise a stream that had lost nothing, and the copy that
            // really was dropped stayed in `pending` for the life of the
            // process — an unbounded working set on a feed with a redundant
            // line. Released arrivals are retained out above, so the last
            // remaining match is always a copy the tracker refused.
            if let SequenceEvent::Duplicate { stream, sequence } = event
                && let Some(position) = arrivals
                    .iter()
                    .rposition(|(_, s, q)| s == stream && q == sequence)
            {
                let (id, _, _) = arrivals.remove(position);
                self.pending.remove(&id);
            }
        }

        outcome.events.extend(batch.events);
        for watermark in batch.watermarks {
            match outcome
                .watermarks
                .iter_mut()
                .find(|existing| existing.stream == watermark.stream)
            {
                Some(existing) => *existing = watermark,
                None => outcome.watermarks.push(watermark),
            }
        }
    }
}

/// The `MarketMessage` that stands in for an envelope inside the tracker.
///
/// `None` for an envelope whose source does not number its output.
fn carrier(envelope: &StreamEnvelope) -> Option<MarketMessage> {
    let origin = envelope.subject().origin.clone()?;
    Some(MarketMessage::new(
        // The carrier's identity *is* the envelope's, which is what makes the
        // translation back exact and makes a synthesised reset unmistakable.
        envelope.event_id().retype::<ObjectKind>(),
        origin,
        MessageBody::Reset {
            reason: format!(
                "sequencing carrier for {} on {}",
                envelope.event_id(),
                envelope.event_type().name()
            ),
        },
        envelope.event_timestamp(),
        envelope.ingest_timestamp(),
    ))
}
