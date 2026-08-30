//! Builders shared by the `qip-streaming` tests.
//!
//! Every timestamp here is a literal. Nothing in these tests reads a wall
//! clock, so a failure reproduces exactly and a run in June behaves like a run
//! in December.

// Each test binary compiles this module and uses a different part of it.
#![allow(dead_code)]

use qip_contracts::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{CorrelationId, Decimal, EventId, Id, Lineage, ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use qip_financial::asset_class::AssetClass;
use qip_market::quote::Tick;
use qip_streaming::{
    Confidence, CostMetadata, EventFacts, Region, RoutingClass, SourceId, SourceIdentity,
    SourceType, StreamEnvelope, Subject,
};
use serde::{Deserialize, Serialize};

pub(crate) const VENUE: &str = "XNAS";
pub(crate) const FEED: &str = "itch-a";
pub(crate) const PARTITION: u32 = 1;

/// 2024-01-02T14:24:05Z, so a rendered timestamp in a failure is legible.
const BASE_NANOS: i64 = 1_704_205_445_000_000_000;

/// A timestamp `millis` after the fixed base.
pub(crate) fn at(millis: i64) -> Timestamp {
    Timestamp::from_nanos(BASE_NANOS + millis * 1_000_000)
}

pub(crate) fn origin(sequence: u64) -> Origin {
    Origin::new(VenueId::new(VENUE), FEED, PARTITION, sequence)
}

pub(crate) fn event_id(label: &str) -> EventId {
    Id::from_string(format!("EVT{label:0>23}"))
}

pub(crate) fn lineage(label: &str) -> Lineage {
    Lineage::root(
        CorrelationId::from_string(format!("COR{label:0>23}")),
        "qip-streaming-tests",
    )
}

pub(crate) fn instrument() -> ObjectId {
    Id::from_string("OBJ00000000000000000000AAA")
}

/// A venue feed: the only source type that is venue-critical.
pub(crate) fn venue_source() -> SourceIdentity {
    SourceIdentity::new(
        SourceId::new("itch-primary"),
        SourceType::VenueFeed,
        Region::new("us-east"),
    )
}

/// A source that publishes text and numbers nothing.
pub(crate) fn research_source() -> SourceIdentity {
    SourceIdentity::new(
        SourceId::new("filings-scraper"),
        SourceType::AlternativeData,
        Region::new("us-east"),
    )
}

pub(crate) fn tick(price: i64, when: Timestamp) -> Tick {
    Tick {
        object_id: instrument(),
        venue: VENUE.to_string(),
        at: when,
        price: Decimal::from_int(price),
        volume: Decimal::from_int(100),
        quality: qip_financial::quality::DataQuality::clean(),
    }
}

/// A body with no idempotency key, so the payload hash has to serve as one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct AuditNote {
    pub(crate) note: String,
}

impl EventBody for AuditNote {
    // A topic `Topic::is_lossy_tolerable` calls unreplaceable, so it can never
    // be routed hot.
    const TOPIC: Topic = Topic::OrderFilled;
    const SCHEMA_VERSION: u32 = 1;
}

/// A hot, venue-critical market tick.
pub(crate) fn hot_tick(
    sequence: u64,
    event_at: Timestamp,
    ingest_at: Timestamp,
) -> Result<StreamEnvelope> {
    hot_tick_labelled(&format!("T{sequence}"), sequence, event_at, ingest_at)
}

/// The same tick under a chosen event id, so a re-delivery of one fact under a
/// new event id can be told apart from a re-delivery of the same envelope.
pub(crate) fn hot_tick_labelled(
    label: &str,
    sequence: u64,
    event_at: Timestamp,
    ingest_at: Timestamp,
) -> Result<StreamEnvelope> {
    let facts = EventFacts::derived(
        venue_source(),
        Subject::default()
            .with_asset_class(AssetClass::Equity)
            .with_instrument(instrument())
            .with_origin(origin(sequence)),
        Topic::MarketTick,
    )
    .with_confidence(Confidence::CERTAIN)
    .with_cost(CostMetadata::free(qip_core::Currency::USD, 96));

    StreamEnvelope::seal(
        event_id(label),
        lineage(label),
        tick(100 + sequence as i64, event_at),
        event_at,
        ingest_at,
        facts,
    )
}

/// A tick from a venue feed that forgot to number its output.
pub(crate) fn unsequenced_tick(
    event_at: Timestamp,
    ingest_at: Timestamp,
) -> Result<StreamEnvelope> {
    let facts = EventFacts::derived(venue_source(), Subject::unattributed(), Topic::MarketTick);
    StreamEnvelope::seal(
        event_id("U1"),
        lineage("U1"),
        tick(100, event_at),
        event_at,
        ingest_at,
        facts,
    )
}

/// A durable event from an unsequenced source.
pub(crate) fn warm_note(
    label: &str,
    event_at: Timestamp,
    ingest_at: Timestamp,
) -> Result<StreamEnvelope> {
    let facts = EventFacts::derived(
        research_source(),
        Subject::unattributed(),
        Topic::OrderFilled,
    )
    .with_routing_class(RoutingClass::Warm);

    StreamEnvelope::seal(
        event_id(label),
        lineage(label),
        AuditNote {
            note: format!("note {label}"),
        },
        event_at,
        ingest_at,
        facts,
    )
}

/// The wire form of an envelope, as JSON.
pub(crate) fn to_wire(envelope: &StreamEnvelope) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(envelope)?)
}
