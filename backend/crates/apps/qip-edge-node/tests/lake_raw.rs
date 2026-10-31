//! The tick lake keeps the simulated venue's bytes exactly as published
//! (R12-TICK-2042, ADR 0181).
//!
//! The lake kept decoded events only, and no captured session could be
//! re-materialised from it as the venue sent it. ADR 0181 grants the
//! simulated depth dataset raw retention (its terms are the platform's own)
//! and has `LakeSink` seal the venue's bytes in each segment's raw slot:
//! each line re-encoded with the feed's own encoder and kept only when it
//! hashes to what the feed recorded at publication.
//!
//! The proof here is independent of that encoder: every pass, the frame the
//! feed actually put on the wire (`SimulatedFeed::last_frame`) is compared
//! byte for byte with the raw slot of the segment that pass's events were
//! sealed into.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_core::ids::ObjectId;
use qip_core::lineage::Lineage;
use qip_core::time::Timestamp;
use qip_core::{Duration, EventId, SystemClock, dec};
use qip_edge::cell::CellConfig;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::assemble;
use qip_edge_node::event_fabric::drain::{LakeSink, Sink, default_retention};
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_events::AnyEvent;
use qip_events::event_fabric::bindings::MARKET_EVENT_APPLIED;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::QosClass;
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_storage::lake_codec::SegmentCodec;
use qip_storage::{BlobStore, MemoryBlobStore};
use std::sync::Arc;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn permitting_to_seed(
    gateway: SimulatedGateway,
    objects: &[&ObjectId],
) -> Result<SimulatedGateway> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for object in objects {
        for action in [
            CreationAction::Create,
            CreationAction::List,
            CreationAction::Seed,
        ] {
            let request = CreationRequest::new(action, venue.as_str(), object.as_str(), "paper");
            for authority in CreationAuthority::ALL {
                permissions.record(authority, &request, "test fixture")?;
            }
        }
    }
    gateway.with_listing_permissions(permissions, "paper")
}

fn batch_of(events: &[qip_contracts::market_event::MarketEvent], pass: u64) -> Result<Batch> {
    let records = events
        .iter()
        .map(|event| {
            let payload = serde_json::to_value(event)?;
            let payload_hash = sha256_hex(qip_core::canonical::canonical_json(&payload).as_bytes());
            let id = format!("raw-1/{pass}/{}", event.origin().sequence);
            Record::from_any_event(
                &AnyEvent {
                    event_id: EventId::from_string(id.clone()),
                    topic: MARKET_EVENT_APPLIED.topic,
                    schema_version: MARKET_EVENT_APPLIED.schema_version,
                    occurred_at: event.event_time(),
                    recorded_at: event.receive_time(),
                    sequence: 0,
                    lineage: Lineage::root(
                        qip_core::lineage::CorrelationId::from_string("c-1"),
                        "test",
                    ),
                    idempotency_key: Some(id),
                    payload,
                    payload_hash,
                },
                PayloadCodec::CanonicalJson,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    Batch::new(
        MessageType::Data,
        QosClass::P2MarketJournal.batch_schema_id(),
        1,
        PayloadCodec::CanonicalJson,
        records,
    )
}

/// Mutation: in `LakeSink::deliver`, drop the native line of every
/// even-sequenced tick from the partition's raw bytes — a raw slot no
/// longer equals the frame its pass published.
#[test]
fn every_passs_frame_re_materialises_byte_for_byte_from_the_lakes_raw_slot() -> Result<()> {
    let venue = VenueId::new("XLON");
    let config = CellConfig::new("raw-1", "europe-west2").with_venue(venue.clone());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let object = ObjectId::from_string("obj-BOOK");
    let mut gateway =
        permitting_to_seed(SimulatedGateway::new(venue.clone(), 7, t(0))?, &[&object])?;
    let mut feed = SimulatedFeed::new(venue);
    feed.attach(&mut node.cell)?;

    let lake = Arc::new(MemoryBlobStore::new());
    let mut sink = LakeSink::new(
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        "raw-1",
        "europe-west2",
        default_retention(),
        SegmentCodec::Identity,
    )?;

    let mut frames: Vec<(u64, String)> = Vec::new();
    for pass in 1..=12u64 {
        let at = t(pass as i64);
        gateway.advance_to(at)?;
        let bid = dec!("99") - qip_core::Decimal::from((pass % 3) as i64);
        gateway.seed_touch(&object, Side::Buy, bid, dec!("500"), at)?;
        if pass % 4 == 0 {
            gateway.seed_touch(&object, Side::Sell, dec!("101"), dec!("300"), at)?;
        }
        feed.publish(&gateway, &mut node.cell, at)?;
        let events = feed.take_captured();
        if events.is_empty() {
            continue;
        }
        frames.push((pass, feed.last_frame().to_string()));
        sink.deliver(pass, QosClass::P2MarketJournal, &batch_of(&events, pass)?)?;
    }
    assert!(
        frames.len() > 5,
        "premise: the passes published frames ({})",
        frames.len()
    );
    assert_eq!(
        sink.raw_unverified(),
        0,
        "every line re-encoded to its published bytes"
    );

    let raw_keys: Vec<String> = lake
        .list("lake/class=market/")?
        .into_iter()
        .filter(|key| key.ends_with(".raw"))
        .collect();
    assert_eq!(
        raw_keys.len(),
        frames.len(),
        "one raw slot per published frame: {raw_keys:?}"
    );
    for (pass, frame) in &frames {
        let suffix = format!("raw-1-o{pass:020}.raw");
        let key = raw_keys
            .iter()
            .find(|key| key.ends_with(&suffix))
            .unwrap_or_else(|| panic!("no raw slot for pass {pass}"));
        let raw = lake.get(key)?.expect("a listed raw slot is readable");
        assert_eq!(
            String::from_utf8(raw).expect("the simulated wire is text"),
            *frame,
            "pass {pass}: the lake's raw bytes are not the frame the venue published"
        );
    }
    Ok(())
}
