//! The universal event envelope.
//!
//! # What this is, and what it is not
//!
//! It is **not** a second envelope. `qip_events::AnyEvent` already carries
//! identity, event type, both timestamps, lineage, an idempotency key and a
//! payload hash, and every one of those is used here rather than restated. A
//! [`StreamEnvelope`] *wraps* an `AnyEvent` and adds the four things ingestion
//! knows and the in-process bus does not: which upstream source the fact
//! entered through, what it is about, how much it cost, and which wire it may
//! travel on.
//!
//! The mapping from the target field names to what already existed:
//!
//! | field | where it lives |
//! |---|---|
//! | `event_id` | `AnyEvent::event_id` |
//! | `event_type` | `AnyEvent::topic` — `Topic` *is* the closed set of event types |
//! | `event_timestamp` | `AnyEvent::occurred_at` — valid-time |
//! | `ingest_timestamp` | `AnyEvent::recorded_at` — known-time |
//! | `schema_version` | `AnyEvent::schema_version` (the body's; the envelope's is [`crate::schema::ENVELOPE_SCHEMA_VERSION`]) |
//! | `payload_hash` | `AnyEvent::payload_hash`, computed by `Envelope::erase` |
//! | `lineage` | `AnyEvent::lineage` |
//! | `venue`, `sequence_number` | `Subject::origin`, a `qip_contracts::Origin` |
//! | `source_id`, `source_type`, `region` | [`crate::provenance::SourceIdentity`] |
//! | `asset_class`, `instrument` | [`crate::provenance::Subject`] |
//! | `confidence`, `cost_metadata` | [`crate::provenance`], both optional |
//! | `routing_class` | [`crate::routing::RoutingClass`] |
//! | `freshness` | **not a field** — see below |
//!
//! # Three invariants the type carries, rather than documents
//!
//! **The payload hash cannot disagree with the payload.** The inner event is
//! private and [`StreamEnvelope::seal`] is the only way to build one from a
//! body. `seal` computes the hash through `qip_events::Envelope::erase`, so a
//! caller has no opportunity to supply one. Deserialisation goes through
//! [`crate::schema::EnvelopeWire`] and recomputes the hash before accepting,
//! so a doctored JSON document is refused rather than trusted.
//!
//! **Ingest never precedes the event.** The pair is clamped forward by
//! `qip_contracts::Stamped`, which owns that rule for the whole platform. The
//! raw value the source reported is kept, so the clamp is visible through
//! [`StreamEnvelope::clock_correction`] instead of being silently swallowed —
//! a feed claiming to have delivered a fact before it happened is a clock
//! fault, and the evidence for it is exactly that difference.
//!
//! **Freshness is derived, never stored.** [`StreamEnvelope::freshness`] takes
//! an as-of and computes from the two timestamps. A stored freshness would be
//! a number that was true once, would drift from the timestamps it was computed
//! from, and — worst — would be a value the platform could read without
//! supplying a clock, which is precisely the ambient-time dependency the whole
//! codebase is built to avoid.

use qip_contracts::time::Stamped;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{Duration, EventId, Lineage, ObjectId, Timestamp};
use qip_events::envelope::canonical_json;
use qip_events::{AnyEvent, EventBody, Topic};
use serde::{Deserialize, Serialize};

use crate::provenance::{
    Confidence, CostMetadata, Region, SourceId, SourceIdentity, SourceType, Subject,
};
use crate::routing::{RoutingClass, TransportPath};
use crate::schema::{ENVELOPE_SCHEMA_VERSION, EnvelopeWire};

/// Everything about an event that is not its payload.
///
/// A parameter object rather than six more arguments to [`StreamEnvelope::seal`]:
/// the fields are all optional-looking strings and enums, and a positional list
/// of them is a list two of which will eventually be swapped.
#[derive(Clone, Debug, PartialEq)]
pub struct EventFacts {
    pub source: SourceIdentity,
    pub subject: Subject,
    pub routing_class: RoutingClass,
    /// `None` means the source did not state one. See [`crate::schema`].
    pub confidence: Option<Confidence>,
    /// `None` means nobody metered it. It does not mean free.
    pub cost: Option<CostMetadata>,
}

impl EventFacts {
    /// Facts for an event whose routing class follows from its shape.
    ///
    /// Prefer this to naming the class by hand: [`RoutingClass::derive`] is
    /// the one place the choice is made, so two call sites cannot classify the
    /// same event differently.
    pub fn derived(source: SourceIdentity, subject: Subject, topic: Topic) -> Self {
        let routing_class = RoutingClass::derive(source.source_type, topic);
        Self {
            source,
            subject,
            routing_class,
            confidence: None,
            cost: None,
        }
    }

    pub fn with_confidence(mut self, confidence: Confidence) -> Self {
        self.confidence = Some(confidence);
        self
    }

    pub fn with_cost(mut self, cost: CostMetadata) -> Self {
        self.cost = Some(cost);
        self
    }

    pub fn with_routing_class(mut self, routing_class: RoutingClass) -> Self {
        self.routing_class = routing_class;
        self
    }
}

/// How old a fact is, computed against an explicit as-of.
///
/// Three numbers rather than one, because "stale" has two different causes and
/// the response differs: a large `ingest_lag` is the source being slow, a large
/// `delivery_lag` is the platform being slow, and only the sum is visible in a
/// single staleness figure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freshness {
    /// As-of minus the event time. Never negative for an event already seen.
    pub age: Duration,
    /// Ingest minus event: how late the source was.
    pub ingest_lag: Duration,
    /// As-of minus ingest: how long the platform has been sitting on it.
    pub delivery_lag: Duration,
}

impl Freshness {
    /// Freshness of an event with these timestamps, seen at `as_of`.
    pub fn between(
        event_timestamp: Timestamp,
        ingest_timestamp: Timestamp,
        as_of: Timestamp,
    ) -> Self {
        Self {
            age: as_of.since(event_timestamp),
            ingest_lag: ingest_timestamp.since(event_timestamp),
            delivery_lag: as_of.since(ingest_timestamp),
        }
    }

    /// Whether the fact is young enough to act on under `budget`.
    pub fn is_within(&self, budget: Duration) -> bool {
        self.age <= budget
    }
}

/// An event, everything known about where it came from, and where it may go.
///
/// The inner `AnyEvent` is private. That is the whole mechanism behind the hash
/// invariant: `AnyEvent`'s fields are public, so an exposed one could be edited
/// out from under its own `payload_hash`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(into = "EnvelopeWire", try_from = "EnvelopeWire")]
pub struct StreamEnvelope {
    event: AnyEvent,
    source: SourceIdentity,
    subject: Subject,
    routing_class: RoutingClass,
    confidence: Option<Confidence>,
    cost: Option<CostMetadata>,
    /// The ingest time as reported, before the forward clamp.
    reported_ingest_timestamp: Timestamp,
}

impl StreamEnvelope {
    /// Build an envelope around a typed body.
    ///
    /// The name says what is irreversible about it: the payload hash is
    /// computed here from the body and there is no argument for supplying one.
    ///
    /// Refuses a routing class that contradicts the event — see
    /// [`RoutingClass::check`]. Clamps `ingest_timestamp` forward to
    /// `event_timestamp` when a source claims to have delivered a fact before
    /// it happened, and keeps the reported value so the clamp is visible.
    pub fn seal<T: EventBody>(
        event_id: EventId,
        lineage: Lineage,
        body: T,
        event_timestamp: Timestamp,
        ingest_timestamp: Timestamp,
        facts: EventFacts,
    ) -> Result<Self> {
        facts
            .routing_class
            .check(facts.source.source_type, T::TOPIC)?;

        // `erase` is where the payload hash is computed, and it is the only
        // place in the platform that computes one. Reusing it means a
        // `StreamEnvelope` and the `AnyEvent` on the bus agree by construction
        // rather than by two implementations happening to match.
        let erased =
            qip_events::Envelope::new(event_id, event_timestamp, ingest_timestamp, lineage, body)
                .erase()?;

        Ok(Self::assemble(
            erased,
            facts,
            ingest_timestamp,
            event_timestamp,
        ))
    }

    /// Apply the clamp and assemble, given an already-erased event.
    fn assemble(
        event: AnyEvent,
        facts: EventFacts,
        reported_ingest_timestamp: Timestamp,
        event_timestamp: Timestamp,
    ) -> Self {
        // `Stamped` owns the "known-time never precedes valid-time" rule for
        // the platform. Applying it here rather than re-deriving it keeps the
        // envelope and every bitemporal read in the mesh on the same rule.
        let stamped = Stamped::new(event, event_timestamp, reported_ingest_timestamp);
        let (valid_at, known_at) = (stamped.valid_at(), stamped.known_at());
        let mut event = stamped.into_value();
        event.occurred_at = valid_at;
        event.recorded_at = known_at;
        Self {
            event,
            source: facts.source,
            subject: facts.subject,
            routing_class: facts.routing_class,
            confidence: facts.confidence,
            cost: facts.cost,
            reported_ingest_timestamp,
        }
    }

    // --- identity and classification ---------------------------------------

    pub fn event_id(&self) -> &EventId {
        &self.event.event_id
    }

    /// The event type. `qip_events::Topic` is the platform's closed set of
    /// them, so there is no second vocabulary here to keep in step with it.
    pub fn event_type(&self) -> Topic {
        self.event.topic
    }

    pub fn source_id(&self) -> &SourceId {
        &self.source.source_id
    }

    pub fn source_type(&self) -> SourceType {
        self.source.source_type
    }

    pub fn region(&self) -> &Region {
        &self.source.region
    }

    pub fn source(&self) -> &SourceIdentity {
        &self.source
    }

    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    pub fn asset_class(&self) -> Option<qip_financial::asset_class::AssetClass> {
        self.subject.asset_class
    }

    pub fn venue(&self) -> Option<&qip_contracts::venue::VenueId> {
        self.subject.venue()
    }

    pub fn instrument(&self) -> Option<&ObjectId> {
        self.subject.instrument.as_ref()
    }

    /// The publisher's own sequence number, where the publisher numbers its
    /// output.
    ///
    /// Deliberately *not* `AnyEvent::sequence`, which is the position the
    /// durable log assigned on append. The two answer different questions —
    /// "did the source skip one" and "where is this in our history" — and a
    /// single field would answer neither.
    pub fn sequence_number(&self) -> Option<u64> {
        self.subject.sequence_number()
    }

    /// The body's schema version.
    pub fn schema_version(&self) -> u32 {
        self.event.schema_version
    }

    /// The envelope schema this build writes.
    pub const fn envelope_version() -> u32 {
        ENVELOPE_SCHEMA_VERSION
    }

    pub fn confidence(&self) -> Option<Confidence> {
        self.confidence
    }

    pub fn cost(&self) -> Option<&CostMetadata> {
        self.cost.as_ref()
    }

    pub fn routing_class(&self) -> RoutingClass {
        self.routing_class
    }

    pub fn transport_path(&self) -> TransportPath {
        self.routing_class.path()
    }

    pub fn lineage(&self) -> &Lineage {
        &self.event.lineage
    }

    // --- time ---------------------------------------------------------------

    /// When the fact was true. `AnyEvent::occurred_at`.
    pub fn event_timestamp(&self) -> Timestamp {
        self.event.occurred_at
    }

    /// When the platform could first have acted on it. `AnyEvent::recorded_at`,
    /// clamped so it never precedes [`Self::event_timestamp`].
    pub fn ingest_timestamp(&self) -> Timestamp {
        self.event.recorded_at
    }

    /// What the source claimed, before the clamp.
    pub fn reported_ingest_timestamp(&self) -> Timestamp {
        self.reported_ingest_timestamp
    }

    /// How far ingest had to be moved forward. Zero when the source's clock
    /// was sane.
    ///
    /// Non-zero means the source dated a delivery before the event it was
    /// delivering, which is physically impossible and therefore a clock or a
    /// parsing fault at the source.
    pub fn clock_correction(&self) -> Duration {
        self.ingest_timestamp()
            .since(self.reported_ingest_timestamp)
    }

    /// Whether the ingest timestamp was clamped forward.
    pub fn was_clamped(&self) -> bool {
        self.reported_ingest_timestamp < self.event_timestamp()
    }

    /// The bitemporal pair, in the platform's own contract type.
    pub fn stamped(&self) -> Stamped<&AnyEvent> {
        Stamped::new(&self.event, self.event_timestamp(), self.ingest_timestamp())
    }

    /// Freshness against an explicit as-of. Derived, never stored.
    pub fn freshness(&self, as_of: Timestamp) -> Freshness {
        Freshness::between(self.event_timestamp(), self.ingest_timestamp(), as_of)
    }

    /// Whether this event was knowable at `as_of` — the only correct predicate
    /// for a point-in-time read.
    pub fn was_known_by(&self, as_of: Timestamp) -> bool {
        self.ingest_timestamp() <= as_of
    }

    // --- payload ------------------------------------------------------------

    /// Content hash of the canonicalised payload, computed by
    /// `qip_events::Envelope::erase` and never supplied.
    pub fn payload_hash(&self) -> &str {
        &self.event.payload_hash
    }

    pub fn payload(&self) -> &serde_json::Value {
        &self.event.payload
    }

    /// The erased event, for code that already speaks `qip_events`.
    pub fn event(&self) -> &AnyEvent {
        &self.event
    }

    /// Recover the typed body, with `AnyEvent::decode`'s policy: a topic
    /// mismatch or a newer *body* schema is refused.
    pub fn decode<T: EventBody>(&self) -> Result<qip_events::Envelope<T>> {
        self.event.decode()
    }

    /// The deduplication key: the body's own idempotency key where it has one,
    /// otherwise the topic and payload hash.
    ///
    /// `AnyEvent::dedup_key` already makes that choice, and making it a second
    /// time here would let the bus and the transport disagree about whether two
    /// deliveries are the same event.
    pub fn idempotency_key(&self) -> String {
        self.event.dedup_key()
    }

    /// Recompute the payload hash and refuse a disagreement.
    ///
    /// Called on every decode. An envelope whose hash does not match its body
    /// is either corrupt in transit or edited on purpose, and neither is worth
    /// distinguishing at this layer.
    pub fn verify_payload_hash(&self) -> Result<()> {
        let recomputed = sha256_hex(canonical_json(&self.event.payload).as_bytes());
        if recomputed != self.event.payload_hash {
            return Err(Error::schema(format!(
                "payload hash mismatch on {}: the envelope claims {} and the payload hashes to {}",
                self.event.event_id, self.event.payload_hash, recomputed
            )));
        }
        Ok(())
    }

    // --- transport framing --------------------------------------------------

    /// The envelope as one `AnyEvent`, ready to append to a durable log.
    ///
    /// Two hashes with two jobs, and they are not the same number. The frame's
    /// `payload_hash` covers the whole serialised envelope, which is what the
    /// log's chain commits to. [`Self::payload_hash`] covers only the business
    /// payload, which is what deduplication and audit compare. Keeping both
    /// honest means neither has to be qualified when it is quoted.
    ///
    /// The frame copies identity, topic, timestamps and lineage from the
    /// envelope, so `EventLog`'s existing indexes — by correlation, by topic,
    /// by event id — and `EventFilter` all keep working on it unchanged.
    pub fn to_frame(&self) -> Result<AnyEvent> {
        let wire = EnvelopeWire::from(self.clone());
        let payload = serde_json::to_value(&wire)?;
        let payload_hash = sha256_hex(canonical_json(&payload).as_bytes());
        Ok(AnyEvent {
            event_id: self.event.event_id.clone(),
            topic: self.event.topic,
            schema_version: self.event.schema_version,
            occurred_at: self.event.occurred_at,
            recorded_at: self.event.recorded_at,
            // Assigned by the log on append; zero until then.
            sequence: 0,
            lineage: self.event.lineage.clone(),
            idempotency_key: Some(self.idempotency_key()),
            payload,
            payload_hash,
        })
    }

    /// Recover an envelope from a transport frame, re-verifying its hash.
    pub fn from_frame(frame: &AnyEvent) -> Result<Self> {
        let wire: EnvelopeWire = serde_json::from_value(frame.payload.clone())?;
        Self::try_from(wire)
    }
}

impl From<StreamEnvelope> for EnvelopeWire {
    fn from(envelope: StreamEnvelope) -> Self {
        Self {
            envelope_version: ENVELOPE_SCHEMA_VERSION,
            source: envelope.source,
            subject: envelope.subject,
            routing_class: envelope.routing_class,
            confidence: envelope.confidence,
            cost: envelope.cost,
            reported_ingest_timestamp: Some(envelope.reported_ingest_timestamp),
            event: envelope.event,
        }
    }
}

impl TryFrom<EnvelopeWire> for StreamEnvelope {
    type Error = Error;

    /// The only way in from the wire, and it checks everything the type claims.
    ///
    /// A newer publisher's extra fields have already been dropped by serde at
    /// this point; see [`crate::schema`] for why that is the right policy for
    /// the envelope and the wrong one for the body.
    fn try_from(wire: EnvelopeWire) -> Result<Self> {
        let event_timestamp = wire.event.occurred_at;
        let reported = wire
            .reported_ingest_timestamp
            .unwrap_or(wire.event.recorded_at);
        let facts = EventFacts {
            source: wire.source,
            subject: wire.subject,
            routing_class: wire.routing_class,
            confidence: wire.confidence,
            cost: wire.cost,
        };
        facts
            .routing_class
            .check(facts.source.source_type, wire.event.topic)?;

        let envelope = Self::assemble(wire.event, facts, reported, event_timestamp);
        envelope.verify_payload_hash()?;
        Ok(envelope)
    }
}
