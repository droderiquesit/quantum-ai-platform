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
//! # The schema/version hash
//!
//! The Blueprint's field list (v12.0 §27, "FabricEnvelope") says "schema/version
//! hash", and `AnyEvent::schema_version` alone is an integer a publisher can
//! forget to bump. [`FabricEnvelope::schema_hash`] is a
//! [`SchemaId`](super::schema_id::SchemaId) — SHA-256 over topic, version and
//! the payload's recursive shape, the derivation the schema registry already
//! uses — recomputed from the wrapped event every time an envelope is
//! assembled. A wire document whose `schema_version` was edited without the
//! hash (or the hash without the version) is therefore refused, which an
//! integer alone cannot do. It attests the shape of *this* payload; it is not
//! a registry lookup, so two instances differing only in an optional field
//! being absent can hash differently, exactly as the registry's own
//! sample-based ids can. Compatibility is decided by version against the
//! reader's bindings in [`FabricEnvelope::open`], not by hash equality.
//!
//! # How an envelope crosses the fabric
//!
//! The unit on the wire is the existing [`Batch`] of [`Record`]s (ADR 0100 §1:
//! one format for the spool, the wire and broker segments); there is no
//! second framing. A record carries the `AnyEvent`; the batch header carries
//! what the drain (producer id, epoch, base sequence) and the broker (base
//! offset, leader epoch, HLC) stamped. [`FabricEnvelope::open`] is the one
//! place those are joined back into the envelope, and the producer's drain and
//! the ledger's consumer both call it, so a record the consumer would refuse is
//! refused before it is sent instead of stalling a partition for ever.
//!
//! # What the type refuses
//!
//!
//! Every field above that identifies *who* or *where* — the stream, the
//! region, the ordering key, the producer, the auth context — must be
//! non-empty. CONTRACT-036's whole argument is that ordering, fencing,
//! idempotency, replay and audit are each decided from one of these fields:
//! an empty ordering key is not "no preference", it is every such record
//! silently sharing one (wrong) partition key, which is why a missing one is
//! refused rather than defaulted.

use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{EventId, Lineage, Timestamp};
use serde::{Deserialize, Serialize};

use crate::envelope::{AnyEvent, Envelope, EventBody, canonical_json};
use crate::topic::Topic;

use super::bindings::TopicBinding;
use super::codec::{Batch, PayloadCodec, Record};
use super::hlc::HlcTimestamp;
use super::policy::QosClass;
use super::schema_id::{SchemaId, Shape};

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

/// What the reader of a batch knows: which stream and partition it read the
/// batch from, and — for a batch no broker has stamped yet, which only its
/// own producer opens — the producer's own region, identity and name.
///
/// Since ADR 0129 the broker stamps region, partition and the authenticated
/// principal into the batch ([`super::codec::BrokerOrigin`]) and the writer
/// stamps its [`super::codec::Provenance`]. Where the batch carries them they
/// are what the envelope says, and a reader whose own claim disagrees is
/// refused rather than believed: a consumer that names the region a record
/// came from is fabricating it. A consumer states `region: None`; a batch
/// that then carries no broker origin is refused, never given the reader's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricContext {
    pub stream: String,
    /// The producer's own region, for its own preflight of a batch it has not
    /// sent; `None` for a consumer, which knows only what the broker stamped.
    pub region: Option<String>,
    pub partition: u32,
    /// The stream's class; a record whose topic is bound to another class is
    /// refused (a P2 journal entry on the P1 outcomes stream is a routing
    /// fault, not data).
    pub qos_class: QosClass,
    pub auth_context: String,
    pub provenance: String,
}

/// The region and authenticated identity an envelope states, from the
/// broker's stamp when the batch carries one (ADR 0129 decision 3).
///
/// Refuses a reader whose region or partition disagrees with the broker's
/// stamp — the consumer-fabricated region this decision exists to stop — and
/// a batch with no stamp opened by a reader that states no region of its
/// own: that is a consumer, and a consumer has no region to give it.
///
/// The partition check runs before the origin arms and independently of
/// them, because the broker stamps the partition on every append
/// (R12-FABRIC-8084) while it stamps an origin only for an authenticated
/// session: a batch produced in-process carries a partition and no origin,
/// and a reader of it as another partition is still refused.
fn origin_facts(batch: &Batch, context: &FabricContext) -> Result<(String, String)> {
    if let Some(partition) = batch.partition
        && partition != context.partition
    {
        return Err(Error::denied(format!(
            "the batch at offset {} was appended to partition {partition} and is being read as \
             partition {}; the broker's stamp and the read disagree",
            batch.base_offset, context.partition
        )));
    }
    match (&batch.origin, &context.region) {
        (Some(origin), claimed) => {
            if let Some(claimed) = claimed
                && *claimed != origin.region
            {
                return Err(Error::denied(format!(
                    "the batch at offset {} was stamped in region {} by the broker that \
                     accepted it, and its reader claims {claimed}; a reader does not name the \
                     region a record was produced in",
                    batch.base_offset, origin.region
                )));
            }
            Ok((origin.region.clone(), origin.principal.clone()))
        }
        (None, Some(own)) => Ok((own.clone(), context.auth_context.clone())),
        (None, None) => Err(Error::denied(format!(
            "the batch at offset {} carries no broker-stamped region, partition or principal \
             (format version 1, or never accepted by a broker), and its reader states none; \
             the region is not filled in by whoever reads it",
            batch.base_offset
        ))),
    }
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
    schema_hash: SchemaId,
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
        Self::assemble(event, logical_timestamp, facts, None)
    }

    /// Join one record of a fetched or about-to-be-sent batch with the batch
    /// header and the reader's context into the envelope that record is.
    ///
    /// Refuses, with nothing returned:
    /// * a record whose duplicated header (event id, trace id, source
    ///   timestamp) disagrees with the event in its payload — the header is
    ///   what a broker ACL reads and the payload is what a consumer acts on,
    ///   so a mismatch is a record that says two things;
    /// * a payload whose `payload_hash` is not the hash of its payload;
    /// * a topic with no binding in `bindings`, a `schema_version` of zero
    ///   or newer than the binding's (upgrade the reader before the
    ///   producer — never read a partly understood event), and a topic bound
    ///   to a class other than `context.qos_class`;
    /// * a batch with no producer id (not yet stamped by a drain) or any
    ///   other empty identity field, via [`Self::assemble`].
    pub fn open(
        batch: &Batch,
        index: usize,
        context: &FabricContext,
        bindings: &[TopicBinding],
    ) -> Result<Self> {
        let record = Self::record_at(batch, index)?;
        let event = record.decode_payload(batch.encoding)?;
        Self::open_decoded(batch, index, event, context, bindings)
    }

    fn record_at(batch: &Batch, index: usize) -> Result<&Record> {
        batch.records.get(index).ok_or_else(|| {
            Error::invalid(format!(
                "batch at offset {} holds {} records; there is no record {index}",
                batch.base_offset,
                batch.records.len()
            ))
        })
    }

    /// [`Self::open`] for a caller that has already decoded record `index`'s
    /// payload (to route on its topic, say) and should not pay to decode it
    /// twice. `event` must be that record's own decoded payload; the header
    /// cross-checks compare it against the record, so a swapped event is
    /// refused rather than trusted.
    pub fn open_decoded(
        batch: &Batch,
        index: usize,
        event: AnyEvent,
        context: &FabricContext,
        bindings: &[TopicBinding],
    ) -> Result<Self> {
        if batch.producer_id.trim().is_empty() {
            return Err(Error::invalid(
                "the batch carries no producer id: the drain stamps one before a batch is sent, \
                 and an unstamped batch has no identity to attribute its records to"
                    .to_string(),
            ));
        }
        let record = Self::record_at(batch, index)?;
        if record.event_id != event.event_id.as_str() {
            return Err(Error::schema(format!(
                "record {index} is framed as event {} but its payload is event {}; the header                  and the payload disagree, so neither is trusted",
                record.event_id, event.event_id
            )));
        }
        let payload_trace = event.lineage.trace_id.as_ref().map(|t| t.as_str());
        if record.trace_id.as_deref() != payload_trace {
            return Err(Error::schema(format!(
                "record {index} (event {}) is framed with trace id {:?} but its payload carries                  {:?}",
                event.event_id, record.trace_id, payload_trace
            )));
        }
        if record.source_timestamp_ns != event.occurred_at.as_nanos() {
            return Err(Error::schema(format!(
                "record {index} (event {}) is framed with source timestamp {} ns but its \
                 payload says {} ns",
                event.event_id,
                record.source_timestamp_ns,
                event.occurred_at.as_nanos()
            )));
        }
        let binding = bindings
            .iter()
            .find(|b| b.topic == event.topic)
            .ok_or_else(|| {
                Error::schema(format!(
                    "no reader is bound to topic {}; an event on a topic this build has no \
                     binding for is refused rather than passed on unread",
                    event.topic
                ))
            })?;
        if event.schema_version == 0 || event.schema_version > binding.schema_version {
            return Err(Error::schema(format!(
                "{} schema version {} is not one this reader supports (1..={}); upgrade the \
                 reader before the producer",
                event.topic, event.schema_version, binding.schema_version
            )));
        }
        if binding.qos_class != context.qos_class {
            return Err(Error::schema(format!(
                "{} is bound to class {} but arrived on stream {} of class {}",
                event.topic,
                binding.qos_class.as_str(),
                context.stream,
                context.qos_class.as_str()
            )));
        }
        let (region, auth_context) = origin_facts(batch, context)?;
        // The writer's provenance, by the same rule as the region (ADR 0129
        // decision 3): a producer's own preflight — the reader that states
        // a region — may name its own, and a consumer may not. Until
        // 2026-10-25 every reader fell back to its own string, so a session
        // batch that carried no provenance reached the ledger labelled with
        // whatever the ledger chose to call it.
        let provenance = match (batch.provenance, &context.region) {
            (Some(provenance), _) => provenance.as_str().to_string(),
            (None, Some(_)) => context.provenance.clone(),
            (None, None) => {
                return Err(Error::denied(format!(
                    "the batch at offset {} carries no writer-stamped provenance (format \
                     version 1, or a writer that stamped none), and its reader is a consumer; \
                     the provenance is not filled in by whoever reads it",
                    batch.base_offset
                )));
            }
        };
        let index_u64 = u64::try_from(index)
            .map_err(|_| Error::numeric(format!("record index {index} does not fit a u64")))?;
        let producer_sequence = batch.base_sequence.checked_add(index_u64).ok_or_else(|| {
            Error::numeric(format!(
                "producer sequence {} + {index} overflows",
                batch.base_sequence
            ))
        })?;
        Self::assemble(
            event,
            HlcTimestamp::new(
                Timestamp::from_nanos(batch.logical_timestamp.physical_ns),
                u64::from(batch.logical_timestamp.logical),
            ),
            FabricFacts {
                stream: context.stream.clone(),
                region,
                partition: context.partition,
                // The catalogue's partition key is `cell` and a cell is its
                // own producer id, so the producer id is the ordering key.
                ordering_key: batch.producer_id.clone(),
                producer_id: batch.producer_id.clone(),
                producer_epoch: batch.producer_epoch,
                producer_sequence,
                leader_epoch: batch.leader_epoch,
                offset: batch.base_offset,
                qos_class: binding.qos_class,
                auth_context,
                provenance,
            },
            None,
        )
    }

    /// Open every record of `batch`, all or none. The first refusal is
    /// returned and nothing is delivered from a batch any of whose records is
    /// refused: a partial open would let a consumer apply a prefix of a batch
    /// whose tail it then stalls on.
    pub fn open_all(
        batch: &Batch,
        context: &FabricContext,
        bindings: &[TopicBinding],
    ) -> Result<Vec<Self>> {
        (0..batch.records.len())
            .map(|index| Self::open(batch, index, context, bindings))
            .collect()
    }

    /// The record this envelope's event travels as — the producer-side half
    /// of [`Self::open`], through the one codec. The facts the envelope adds
    /// (stream, producer, offset, …) are stamped on the batch by the drain
    /// and the broker, not on the record.
    pub fn to_record(&self, encoding: PayloadCodec) -> Result<Record> {
        Record::from_any_event(&self.event, encoding)
    }

    /// The identity under which a redelivery is recognised: the same
    /// producer's same sequence in the same epoch on the same partition is
    /// the same envelope however many times it arrives. Epoch is part of it
    /// because a fenced-off incarnation's sequence 7 is not the new
    /// incarnation's.
    pub fn identity(&self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            self.stream,
            self.partition,
            self.producer_id,
            self.producer_epoch,
            self.producer_sequence
        )
    }

    /// Apply every CONTRACT-036 field's validation and assemble. The one
    /// path both [`Self::seal`] and the wire `TryFrom` below go through, so
    /// a record built fresh and one recovered off the wire are held to the
    /// same rule.
    fn assemble(
        event: AnyEvent,
        logical_timestamp: HlcTimestamp,
        facts: FabricFacts,
        claimed_schema_hash: Option<SchemaId>,
    ) -> Result<Self> {
        let expected_hash = sha256_hex(canonical_json(&event.payload).as_bytes());
        if event.payload_hash != expected_hash {
            return Err(Error::schema(format!(
                "event {} carries payload hash {} but its payload hashes to {expected_hash}; the \
                 payload was altered after it was sealed",
                event.event_id, event.payload_hash
            )));
        }
        let schema_hash = SchemaId::new(
            event.topic.name(),
            event.schema_version,
            &Shape::from_json(&event.payload),
        );
        if let Some(claimed) = claimed_schema_hash
            && claimed != schema_hash
        {
            return Err(Error::schema(format!(
                "event {} claims schema hash {claimed} but topic {} version {} over this \
                 payload hashes to {schema_hash}; its version or its shape was edited",
                event.event_id, event.topic, event.schema_version
            )));
        }
        // R12-CONTRACT-7089: the trace id and the idempotency key are two of
        // the contract's required fields, so an envelope without either is
        // refused here — built or decoded — rather than carried with a hole.
        // A trace nobody stamped joins nothing, and a record with no
        // idempotency key cannot be told from its own redelivery downstream.
        if event
            .lineage
            .trace_id
            .as_ref()
            .is_none_or(|trace| trace.as_str().trim().is_empty())
        {
            return Err(Error::invalid(format!(
                "event {} carries no trace id; a fabric envelope's trace id is required, and the \
                 writer stamps it",
                event.event_id
            )));
        }
        if event
            .idempotency_key
            .as_deref()
            .is_none_or(|key| key.trim().is_empty())
        {
            return Err(Error::invalid(format!(
                "event {} carries no idempotency key; a fabric envelope's idempotency key is \
                 required, and the writer stamps it",
                event.event_id
            )));
        }
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
            provenance: require_non_empty(facts.provenance, "provenance")?,
            schema_hash,
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
    /// The schema/version hash; see the module doc.
    pub fn schema_hash(&self) -> &SchemaId {
        &self.schema_hash
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
    schema_hash: SchemaId,
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
            schema_hash: e.schema_hash,
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
            Some(wire.schema_hash),
        )
    }
}
