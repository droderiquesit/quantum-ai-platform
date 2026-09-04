//! What happens to an event between arriving and being trusted.
//!
//! Eight things, in a fixed order, and the order matters:
//!
//! 1. **Decode and verify.** A wire form that will not parse, or whose payload
//!    hash disagrees with its payload, is rejected. So is one whose routing
//!    class contradicts its topic.
//! 2. **Replay protection.** An event whose ingest time is older than the
//!    replay window is refused before anything else looks at it.
//! 3. **Future rejection.** The mirror image: an event stamped meaningfully
//!    after the caller's clock is a clock fault or an injection, not data.
//! 4. **Deduplication.** By `qip_events::AnyEvent::dedup_key` — the body's own
//!    idempotency key where it has one, the payload hash where it does not.
//! 5. **Timestamp correction.** Already applied by the envelope, which clamps
//!    ingest forward past event; here it is *reported*, because a clamp is
//!    evidence of a broken clock upstream.
//! 6. **Clock-skew detection.** A large but legal ingest lag is observed, not
//!    rejected. Dropping the data loses the data and keeps the clock problem.
//! 7. **Sequence validation.** A source that is supposed to number its output
//!    and did not cannot be gap-checked, and a stream with no gap detection is
//!    a book that can be silently wrong. That is refused loudly.
//! 8. **Gap detection.** Delegated whole to `qip-sequencing` through
//!    [`crate::sequencing::SequenceCoordinator`]. A hole stops the watermark.
//!
//! # One bad event never stops the batch
//!
//! [`StreamProcessor::admit_wire`] returns `BatchOutcome`, not `Result`. A
//! malformed event is a rejection with a named reason and an index, and the
//! rest of the batch is processed. The alternative — failing the call — means
//! one corrupt message from one source stops every other source in the same
//! poll, which is how a single bad publisher takes down an ingestion tier.
//!
//! # Nothing here reads a clock
//!
//! Every entry point takes the time as a parameter. A replay of the same wire
//! forms with the same timestamps produces the same acceptances, the same
//! rejections and the same watermarks as the live run did.

use qip_contracts::time::Watermark;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_sequencing::{ReorderPolicy, SequenceEvent};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};

use crate::envelope::StreamEnvelope;
use crate::provenance::SourceId;
use crate::schema::{self, SchemaCompatibility};
use crate::sequencing::{SequenceCoordinator, SequencedEnvelopes, StreamReset};

/// The tolerances a processor applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessingPolicy {
    /// How far back an event's ingest time may be before it is treated as a
    /// replay rather than as late data.
    ///
    /// A deliberate backfill runs with this widened; the default is sized for
    /// a live tier, where an event a day old is a re-delivery of something
    /// already acted on.
    pub replay_window: Duration,
    /// How far ahead of the caller's clock an event may be stamped.
    pub future_tolerance: Duration,
    /// Ingest lag beyond which the source's clock is reported as skewed.
    pub max_clock_skew: Duration,
    /// How many distinct idempotency keys are remembered.
    ///
    /// Bounded, because an unbounded set grows with the feed and the process
    /// dies during the busiest hour. A duplicate arriving after this many
    /// distinct events is not caught here, which is why the replay window is
    /// the second line of defence and not a nicety.
    ///
    /// Zero is refused by [`StreamProcessor::new`] rather than widened to one.
    pub dedup_capacity: usize,
    /// Passed straight to `qip_sequencing`.
    pub reorder: ReorderPolicy,
}

impl Default for ProcessingPolicy {
    fn default() -> Self {
        Self {
            replay_window: Duration::from_hours(24),
            future_tolerance: Duration::from_secs(1),
            max_clock_skew: Duration::from_secs(5),
            dedup_capacity: 65_536,
            reorder: ReorderPolicy::default(),
        }
    }
}

/// Why an event was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    /// Would not decode, or its payload hash disagreed with its payload.
    Malformed,
    /// Decoded, but its routing class contradicts its topic or its source.
    Unroutable,
    /// Already seen, by idempotency key.
    Duplicate,
    /// Older than the replay window.
    Replayed,
    /// Stamped further ahead of the caller's clock than tolerated.
    FutureDated,
    /// From a source that numbers its output, but carrying no sequence.
    Unsequenced,
}

impl RejectionReason {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::Unroutable => "unroutable",
            Self::Duplicate => "duplicate",
            Self::Replayed => "replayed",
            Self::FutureDated => "future_dated",
            Self::Unsequenced => "unsequenced",
        }
    }
}

/// One rejected event, with enough to find it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejection {
    /// Position in the batch as offered, so the caller can correlate.
    pub index: usize,
    pub reason: RejectionReason,
    /// What was wrong, in words.
    pub detail: String,
    /// The event id, where the event decoded far enough to have one.
    pub event_id: Option<String>,
}

/// Something worth an operator's attention that is not a rejection.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamObservation {
    /// A source dated a delivery before the event it was delivering. The
    /// envelope clamped it forward; this is how much.
    IngestClamped {
        event_id: String,
        correction: Duration,
    },
    /// A source's ingest lag exceeded the tolerance. Not a rejection: dropping
    /// the data loses the data and keeps the clock problem.
    ClockSkew {
        source_id: SourceId,
        skew: Duration,
        tolerance: Duration,
    },
    /// The publisher writes a different envelope version than this build reads.
    SchemaSkew {
        event_id: String,
        compatibility: SchemaCompatibility,
    },
    /// A hole would not fill. Consumers of the stream must resynchronise.
    StreamResynchronised { stream: String, reason: String },
}

/// Counters for everything the processor has seen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessingStats {
    /// Events handed to the processor.
    pub offered: u64,
    /// Events that passed every screen.
    ///
    /// Not the same as events released: one that passed may still be held
    /// behind a hole and released on a later call, which is exactly what a
    /// watermark that has stopped moving means.
    pub accepted: u64,
    pub malformed: u64,
    pub unroutable: u64,
    pub duplicates: u64,
    pub replayed: u64,
    pub future_dated: u64,
    pub unsequenced: u64,
    /// Events whose ingest time had to be clamped forward.
    pub clamped: u64,
    /// Events whose ingest lag exceeded the tolerance.
    pub skewed: u64,
    /// Events decoded from a different envelope version than this build writes.
    pub schema_skew: u64,
    /// Holes given up on, each of which told consumers to resynchronise.
    pub resets: u64,
}

/// What one batch produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatchOutcome {
    /// Envelopes in contiguous order, safe to act on.
    pub accepted: Vec<StreamEnvelope>,
    /// Everything refused, each with a named reason.
    pub rejected: Vec<Rejection>,
    /// Exactly what `qip_sequencing` observed.
    pub sequence_events: Vec<SequenceEvent>,
    pub observations: Vec<StreamObservation>,
    /// Each touched stream's highest contiguous position.
    pub watermarks: Vec<Watermark>,
}

impl BatchOutcome {
    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty() && self.rejected.is_empty() && self.observations.is_empty()
    }

    /// Rejections of one kind, for a caller that only cares about some.
    pub fn rejections_because(&self, reason: RejectionReason) -> Vec<&Rejection> {
        self.rejected
            .iter()
            .filter(|rejection| rejection.reason == reason)
            .collect()
    }
}

/// A bounded set of idempotency keys already seen.
#[derive(Debug)]
struct DedupWindow {
    capacity: usize,
    seen: BTreeSet<String>,
    order: VecDeque<String>,
}

impl DedupWindow {
    /// A window remembering `capacity` keys, refusing a capacity of zero.
    ///
    /// The refusal rather than a widening to one: a window silently corrected
    /// upwards is a configuration mistake that survives, because the operator
    /// reads back the policy they wrote while the process runs a different one.
    /// `qip_market_ingestion::DedupWindow::new` and `EventBus::dedup_capacity`
    /// already refuse the same value; a third window that clamped instead would
    /// be the one place a zero could be written down and not noticed.
    fn new(capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a dedup window of zero remembers no idempotency key, so every redelivery would \
                 be admitted as a new event; give dedup_capacity at least 1",
            ));
        }
        Ok(Self {
            capacity,
            seen: BTreeSet::new(),
            order: VecDeque::new(),
        })
    }

    /// Record a key. `false` when it was already present.
    fn admit(&mut self, key: &str) -> bool {
        if self.seen.contains(key) {
            return false;
        }
        if self.order.len() >= self.capacity
            && let Some(evicted) = self.order.pop_front()
        {
            self.seen.remove(&evicted);
        }
        self.seen.insert(key.to_string());
        self.order.push_back(key.to_string());
        true
    }
}

/// Screens, deduplicates and orders a stream of envelopes.
#[derive(Debug)]
pub struct StreamProcessor {
    policy: ProcessingPolicy,
    dedup: DedupWindow,
    coordinator: SequenceCoordinator,
    stats: ProcessingStats,
}

impl StreamProcessor {
    /// A processor applying `policy`, refusing a policy whose bounds cannot do
    /// the job they are configured for.
    pub fn new(policy: ProcessingPolicy) -> Result<Self> {
        Ok(Self {
            dedup: DedupWindow::new(policy.dedup_capacity)?,
            coordinator: SequenceCoordinator::new(policy.reorder),
            policy,
            stats: ProcessingStats::default(),
        })
    }

    pub fn policy(&self) -> ProcessingPolicy {
        self.policy
    }

    pub fn stats(&self) -> ProcessingStats {
        self.stats
    }

    /// Every stream's watermark, at its highest contiguous position.
    pub fn watermarks(&self) -> Vec<Watermark> {
        self.coordinator.watermarks()
    }

    /// Admit raw wire forms, reporting schema skew as it decodes.
    ///
    /// The entry point a transport uses. Never fails as a whole: an
    /// undecodable frame is one rejection and the batch continues.
    pub fn admit_wire(&mut self, frames: Vec<serde_json::Value>, now: Timestamp) -> BatchOutcome {
        let mut outcome = BatchOutcome::default();
        let mut decoded = Vec::with_capacity(frames.len());

        for (index, frame) in frames.iter().enumerate() {
            self.stats.offered += 1;
            match schema::decode(frame) {
                Ok((envelope, compatibility)) => {
                    if compatibility != SchemaCompatibility::Exact {
                        self.stats.schema_skew += 1;
                        outcome.observations.push(StreamObservation::SchemaSkew {
                            event_id: envelope.event_id().as_str().to_string(),
                            compatibility,
                        });
                    }
                    decoded.push((index, envelope));
                }
                Err(error) => self.reject_decode(index, frame, &error, &mut outcome),
            }
        }

        self.screen_and_order(decoded, now, &mut outcome);
        outcome
    }

    /// Admit envelopes that are already decoded and verified.
    pub fn admit(&mut self, envelopes: Vec<StreamEnvelope>, now: Timestamp) -> BatchOutcome {
        let mut outcome = BatchOutcome::default();
        let decoded: Vec<(usize, StreamEnvelope)> = envelopes.into_iter().enumerate().collect();
        self.stats.offered += decoded.len() as u64;
        self.screen_and_order(decoded, now, &mut outcome);
        outcome
    }

    /// Advance gap deadlines without anything arriving to carry them.
    ///
    /// A stream that goes silent right after a hole opens would otherwise stay
    /// blocked forever, and going silent is what a hole often precedes.
    pub fn poll(&mut self, until: Timestamp) -> BatchOutcome {
        let mut outcome = BatchOutcome::default();
        let sequenced = self.coordinator.poll(until);
        self.absorb(sequenced, &mut outcome);
        outcome
    }

    /// Turn a decode failure into a named rejection.
    ///
    /// `Error::Invalid` comes from `RoutingClass::check` and nothing else on
    /// this path, so it is reported as its own reason: an event that decoded
    /// fine but is addressed to the wrong wire is an entirely different fault
    /// from a corrupt one, and an operator should not have to read the prose
    /// to tell them apart.
    fn reject_decode(
        &mut self,
        index: usize,
        frame: &serde_json::Value,
        error: &Error,
        outcome: &mut BatchOutcome,
    ) {
        let reason = match error {
            Error::Invalid(_) => RejectionReason::Unroutable,
            _ => RejectionReason::Malformed,
        };
        match reason {
            RejectionReason::Unroutable => self.stats.unroutable += 1,
            _ => self.stats.malformed += 1,
        }
        outcome.rejected.push(Rejection {
            index,
            reason,
            detail: error.to_string(),
            event_id: frame
                .get("event")
                .and_then(|event| event.get("event_id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        });
    }

    /// Screen survivors, then hand them to the sequencer.
    fn screen_and_order(
        &mut self,
        decoded: Vec<(usize, StreamEnvelope)>,
        now: Timestamp,
        outcome: &mut BatchOutcome,
    ) {
        let mut survivors = Vec::with_capacity(decoded.len());
        for (index, envelope) in decoded {
            if let Some(envelope) = self.screen(index, envelope, now, outcome) {
                survivors.push(envelope);
            }
        }
        if survivors.is_empty() {
            // Still publish whatever the sequencer already knows, so a caller
            // polling a quiet tick sees the current watermarks.
            outcome.watermarks = self.coordinator.watermarks();
            return;
        }
        let sequenced = self.coordinator.admit(survivors, now);
        self.absorb(sequenced, outcome);
    }

    /// Apply every per-event check. `None` means rejected.
    fn screen(
        &mut self,
        index: usize,
        envelope: StreamEnvelope,
        now: Timestamp,
        outcome: &mut BatchOutcome,
    ) -> Option<StreamEnvelope> {
        let event_id = envelope.event_id().as_str().to_string();

        let age = now.since(envelope.ingest_timestamp());
        if age > self.policy.replay_window {
            self.stats.replayed += 1;
            outcome.rejected.push(Rejection {
                index,
                reason: RejectionReason::Replayed,
                detail: format!(
                    "ingested {age:?} ago, which is outside the {:?} replay window",
                    self.policy.replay_window
                ),
                event_id: Some(event_id),
            });
            return None;
        }

        let ahead = envelope.ingest_timestamp().since(now);
        if ahead > self.policy.future_tolerance {
            self.stats.future_dated += 1;
            outcome.rejected.push(Rejection {
                index,
                reason: RejectionReason::FutureDated,
                detail: format!(
                    "ingest time is {ahead:?} ahead of the caller's clock, beyond the {:?} \
                     tolerance",
                    self.policy.future_tolerance
                ),
                event_id: Some(event_id),
            });
            return None;
        }

        if envelope.source_type().is_sequenced() && envelope.sequence_number().is_none() {
            self.stats.unsequenced += 1;
            outcome.rejected.push(Rejection {
                index,
                reason: RejectionReason::Unsequenced,
                detail: format!(
                    "a {} source must number its output; without a sequence this stream cannot \
                     be gap-checked, and a stream with no gap detection is one that can be \
                     silently wrong",
                    envelope.source_type()
                ),
                event_id: Some(event_id),
            });
            return None;
        }

        let key = envelope.idempotency_key();
        if !self.dedup.admit(&key) {
            self.stats.duplicates += 1;
            outcome.rejected.push(Rejection {
                index,
                reason: RejectionReason::Duplicate,
                detail: format!("idempotency key {key} has already been processed"),
                event_id: Some(event_id),
            });
            return None;
        }

        if envelope.was_clamped() {
            self.stats.clamped += 1;
            outcome.observations.push(StreamObservation::IngestClamped {
                event_id: event_id.clone(),
                correction: envelope.clock_correction(),
            });
        }

        let skew = envelope
            .ingest_timestamp()
            .since(envelope.event_timestamp());
        if skew > self.policy.max_clock_skew {
            self.stats.skewed += 1;
            outcome.observations.push(StreamObservation::ClockSkew {
                source_id: envelope.source_id().clone(),
                skew,
                tolerance: self.policy.max_clock_skew,
            });
        }

        self.stats.accepted += 1;
        Some(envelope)
    }

    /// Fold a sequencer outcome into the batch outcome.
    fn absorb(&mut self, sequenced: SequencedEnvelopes, outcome: &mut BatchOutcome) {
        let SequencedEnvelopes {
            released,
            events,
            watermarks,
            resets,
        } = sequenced;
        outcome.accepted.extend(released);
        outcome.sequence_events.extend(events);
        for StreamReset { stream, reason } in resets {
            self.stats.resets += 1;
            outcome
                .observations
                .push(StreamObservation::StreamResynchronised { stream, reason });
        }
        for watermark in watermarks {
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
