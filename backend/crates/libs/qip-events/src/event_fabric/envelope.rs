//! Event fabric envelope wrapping `AnyEvent` with fabric-specific metadata.
//!
//! # What this is, and what it is not
//!
//! [`FabricEnvelope`] is CONTRACT-036's one internal event model — the red
//! team's m7: "the envelope wraps `AnyEvent`, no second event model." It
//! **wraps** [`crate::envelope::AnyEvent`] rather than restating it, in the
//! same discipline `qip_streaming::envelope::StreamEnvelope` already uses;
//! see that module's own doc comment for why a second envelope would be the
//! worst possible outcome for a platform whose whole audit story is that
//! there is one event history.
//!
//! CONTRACT-036's field list, and where each one already lives:
//!
//! | field | where it lives |
//! |---|---|
//! | event ID | `AnyEvent::event_id` |
//! | schema ID | `AnyEvent::topic` — `Topic` is the closed set of schema identities |
//! | schema version | `AnyEvent::schema_version` |
//! | source timestamp | `AnyEvent::occurred_at` |
//! | receive timestamp | `AnyEvent::recorded_at` |
//! | idempotency key | `AnyEvent::idempotency_key` |
//! | payload hash / integrity checksum | `AnyEvent::payload_hash` |
//! | trace ID | `AnyEvent::lineage.trace_id` |
//! | stream namespace | [`FabricEnvelope::stream`] |
//! | region | [`FabricEnvelope::region`] |
//! | partition | [`FabricEnvelope::partition`] |
//! | ordering key | [`FabricEnvelope::ordering_key`] |
//! | producer ID, epoch, sequence | [`FabricEnvelope::producer_id`], [`FabricEnvelope::producer_epoch`], [`FabricEnvelope::producer_sequence`] |
//! | leader epoch | [`FabricEnvelope::leader_epoch`] |
//! | offset | [`FabricEnvelope::offset`] |
//! | logical (HLC) timestamp | [`FabricEnvelope::logical_timestamp`], `super::hlc::HlcTimestamp` |
//! | priority/QoS class | [`FabricEnvelope::qos_class`], `super::policy::QosClass` |
//! | provenance metadata | [`FabricEnvelope::provenance`] |
//! | auth context | [`FabricEnvelope::auth_context`] |
//!
//! # What the type refuses
//!
//! Every field above that identifies *who* or *where* — the stream, the
//! region, the ordering key, the producer, the auth context — must be
//! non-empty. CONTRACT-036's whole argument is that ordering, fencing,
//! idempotency, replay and audit are each decided from one of these fields:
//! an empty ordering key is not "no preference", it is every such record
//! silently sharing one (wrong) partition key, which is why a missing one is
//! refused rather than defaulted.

use qip_core::error::{Error, Result};
use qip_core::{EventId, Lineage, Timestamp};
use serde::{Deserialize, Serialize};

use crate::envelope::{AnyEvent, Envelope, EventBody};
use crate::topic::Topic;

use super::hlc::HlcTimestamp;
use super::policy::QosClass;

/// Everything [`FabricEnvelope::seal`] needs beyond the wrapped event and its
/// logical timestamp, gathered so the constructor takes one argument that
/// names its fields rather than a dozen positional ones two of which would
/// eventually be swapped.
#[derive(Clone, Debug, PartialEq)]
pub struct FabricFacts {
    pub stream: String,
    pub region: String,
    pub partition: u32,
    pub ordering_key: String,
    pub producer_id: String,
    pub producer_epoch: u64,
    pub producer_sequence: u64,
    pub leader_epoch: u64,
    pub offset: u64,
    pub qos_class: QosClass,
    pub auth_context: String,
    pub provenance: String,
}

/// CONTRACT-036's `FabricEnvelope`. See the module doc for the full field
/// mapping onto `AnyEvent`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(into = "FabricEnvelopeWire", try_from = "FabricEnvelopeWire")]
pub struct FabricEnvelope {
    event: AnyEvent,
    stream: String,
    region: String,
    partition: u32,
    ordering_key: String,
    producer_id: String,
    producer_epoch: u64,
    producer_sequence: u64,
    leader_epoch: u64,
    offset: u64,
    logical_timestamp: HlcTimestamp,
    qos_class: QosClass,
    auth_context: String,
    provenance: String,
}

impl FabricEnvelope {
    /// Build a `FabricEnvelope` around a typed body.
    ///
    /// Mirrors `StreamEnvelope::seal`: `Envelope::erase` is the one place a
    /// payload hash is computed, so wrapping it here rather than
    /// recomputing means this type and the bus's own `AnyEvent` cannot
    /// disagree about what the hash covers.
    pub fn seal<T: EventBody>(
        event_id: EventId,
        lineage: Lineage,
        body: T,
        occurred_at: Timestamp,
        recorded_at: Timestamp,
        logical_timestamp: HlcTimestamp,
        facts: FabricFacts,
    ) -> Result<Self> {
        let event = Envelope::new(event_id, occurred_at, recorded_at, lineage, body).erase()?;
        Self::assemble(event, logical_timestamp, facts)
    }

    /// Apply every CONTRACT-036 field's validation and assemble. The one
    /// path both [`Self::seal`] and the wire `TryFrom` below go through, so
    /// a record built fresh and one recovered off the wire are held to the
    /// same rule.
    fn assemble(
        event: AnyEvent,
        logical_timestamp: HlcTimestamp,
        facts: FabricFacts,
    ) -> Result<Self> {
        Ok(Self {
            event,
            stream: require_non_empty(facts.stream, "stream namespace")?,
            region: require_non_empty(facts.region, "region")?,
            partition: facts.partition,
            ordering_key: require_non_empty(facts.ordering_key, "ordering key")?,
            producer_id: require_non_empty(facts.producer_id, "producer id")?,
            producer_epoch: facts.producer_epoch,
            producer_sequence: facts.producer_sequence,
            leader_epoch: facts.leader_epoch,
            offset: facts.offset,
            logical_timestamp,
            qos_class: facts.qos_class,
            auth_context: require_non_empty(facts.auth_context, "auth context")?,
            provenance: facts.provenance,
        })
    }

    // --- CONTRACT-036 fields this type adds ---------------------------------

    pub fn stream(&self) -> &str {
        &self.stream
    }
    pub fn region(&self) -> &str {
        &self.region
    }
    pub fn partition(&self) -> u32 {
        self.partition
    }
    pub fn ordering_key(&self) -> &str {
        &self.ordering_key
    }
    pub fn producer_id(&self) -> &str {
        &self.producer_id
    }
    pub fn producer_epoch(&self) -> u64 {
        self.producer_epoch
    }
    pub fn producer_sequence(&self) -> u64 {
        self.producer_sequence
    }
    pub fn leader_epoch(&self) -> u64 {
        self.leader_epoch
    }
    pub fn offset(&self) -> u64 {
        self.offset
    }
    pub fn logical_timestamp(&self) -> HlcTimestamp {
        self.logical_timestamp
    }
    pub fn qos_class(&self) -> QosClass {
        self.qos_class
    }
    pub fn auth_context(&self) -> &str {
        &self.auth_context
    }
    pub fn provenance(&self) -> &str {
        &self.provenance
    }

    // --- CONTRACT-036 fields `AnyEvent` already carries ---------------------

    pub fn event_id(&self) -> &EventId {
        &self.event.event_id
    }
    pub fn topic(&self) -> Topic {
        self.event.topic
    }
    pub fn schema_version(&self) -> u32 {
        self.event.schema_version
    }
    pub fn occurred_at(&self) -> Timestamp {
        self.event.occurred_at
    }
    pub fn recorded_at(&self) -> Timestamp {
        self.event.recorded_at
    }
    pub fn lineage(&self) -> &Lineage {
        &self.event.lineage
    }
    pub fn idempotency_key(&self) -> Option<&str> {
        self.event.idempotency_key.as_deref()
    }
    pub fn payload_hash(&self) -> &str {
        &self.event.payload_hash
    }
    pub fn payload(&self) -> &serde_json::Value {
        &self.event.payload
    }

    /// The wrapped event, for code that already speaks `qip_events`.
    pub fn event(&self) -> &AnyEvent {
        &self.event
    }

    /// Recover the typed body, with `AnyEvent::decode`'s policy: a topic
    /// mismatch or a newer body schema is refused.
    pub fn decode<T: EventBody>(&self) -> Result<Envelope<T>> {
        self.event.decode()
    }
}

fn require_non_empty(value: String, field: &str) -> Result<String> {
    if value.trim().is_empty() {
        return Err(Error::invalid(format!(
            "a fabric envelope must carry a non-empty {field}"
        )));
    }
    Ok(value)
}

/// The serde wire form. Every field is public because the validation that
/// matters lives in [`FabricEnvelope::assemble`], reached by every path onto
/// or off the wire — this type only carries bytes between them.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct FabricEnvelopeWire {
    event: AnyEvent,
    stream: String,
    region: String,
    partition: u32,
    ordering_key: String,
    producer_id: String,
    producer_epoch: u64,
    producer_sequence: u64,
    leader_epoch: u64,
    offset: u64,
    logical_timestamp: HlcTimestamp,
    qos_class: QosClass,
    auth_context: String,
    provenance: String,
}

impl From<FabricEnvelope> for FabricEnvelopeWire {
    fn from(e: FabricEnvelope) -> Self {
        Self {
            event: e.event,
            stream: e.stream,
            region: e.region,
            partition: e.partition,
            ordering_key: e.ordering_key,
            producer_id: e.producer_id,
            producer_epoch: e.producer_epoch,
            producer_sequence: e.producer_sequence,
            leader_epoch: e.leader_epoch,
            offset: e.offset,
            logical_timestamp: e.logical_timestamp,
            qos_class: e.qos_class,
            auth_context: e.auth_context,
            provenance: e.provenance,
        }
    }
}

impl TryFrom<FabricEnvelopeWire> for FabricEnvelope {
    type Error = Error;

    /// The only way in from the wire, and it checks everything the type
    /// claims: a doctored or truncated document with, say, an emptied
    /// ordering key is refused here rather than accepted with a field that
    /// silently means nothing.
    fn try_from(wire: FabricEnvelopeWire) -> Result<Self> {
        Self::assemble(
            wire.event,
            wire.logical_timestamp,
            FabricFacts {
                stream: wire.stream,
                region: wire.region,
                partition: wire.partition,
                ordering_key: wire.ordering_key,
                producer_id: wire.producer_id,
                producer_epoch: wire.producer_epoch,
                producer_sequence: wire.producer_sequence,
                leader_epoch: wire.leader_epoch,
                offset: wire.offset,
                qos_class: wire.qos_class,
                auth_context: wire.auth_context,
                provenance: wire.provenance,
            },
        )
    }
}
