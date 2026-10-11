//! What the edge node's tick lake keeps is decided per dataset by a
//! retention policy, not by a constant (R12-DATA-1039), and the codec it
//! seals with is a setting that is off by default (R12-TICK-2057).

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::governance::{Entitlement as Licence, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_core::lineage::Lineage;
use qip_core::{Decimal, Duration, EventId, ObjectId, Timestamp};
use qip_edge_node::event_fabric::drain::{LakeSink, Sink, default_retention, parse_retention};
use qip_edge_node::feed::SIMULATED_DATASET;
use qip_events::AnyEvent;
use qip_events::event_fabric::bindings::MARKET_EVENT_APPLIED;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::QosClass;
use qip_storage::lake_codec::SegmentCodec;
use qip_storage::{BlobStore, MemoryBlobStore};
use std::sync::Arc;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

fn record(dataset: &str, instrument: &str, sequence: u64) -> Result<Record> {
    let message = MarketMessage::new(
        ObjectId::from_string(instrument),
        Origin::new(VenueId::new("XLON"), "itch-a", 0, sequence),
        MessageBody::LevelSet {
            side: BookSide::Bid,
            price: Decimal::from_int(100),
            quantity: Decimal::from_int(5),
            order_count: None,
        },
        at(0),
        at(1),
    );
    let hash = MarketEvent::hash_payload(&message)?;
    let event = MarketEvent::new(
        message,
        at(0),
        at(1),
        at(1),
        Duration::ZERO,
        hash,
        Licence::Granted {
            dataset: dataset.into(),
            usage: Usage::Trade,
            expires_at: Timestamp::MAX,
        },
    )?;
    let payload = serde_json::to_value(&event)?;
    let payload_hash = sha256_hex(qip_core::canonical::canonical_json(&payload).as_bytes());
    let id = format!("edge-1/{dataset}/{sequence}");
    let any = AnyEvent {
        event_id: EventId::from_string(id.clone()),
        topic: MARKET_EVENT_APPLIED.topic,
        schema_version: MARKET_EVENT_APPLIED.schema_version,
        occurred_at: at(0),
        recorded_at: at(1),
        sequence: 0,
        lineage: Lineage::root(qip_core::lineage::CorrelationId::from_string("c-1"), "test"),
        idempotency_key: Some(id),
        payload,
        payload_hash,
    };
    Record::from_any_event(&any, PayloadCodec::CanonicalJson)
}

fn batch(records: Vec<Record>) -> Result<Batch> {
    Batch::new(
        MessageType::Data,
        QosClass::P2MarketJournal.batch_schema_id(),
        1,
        PayloadCodec::CanonicalJson,
        records,
    )
}

/// Mutation: drop the `permitted` check in `LakeSink::deliver` — the
/// licensed dataset nobody granted retention to is written to the lake.
#[test]
fn a_licensed_dataset_with_no_retention_right_is_used_live_and_not_kept() -> Result<()> {
    let lake = Arc::new(MemoryBlobStore::new());
    let mut sink = LakeSink::new(
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        "edge-1",
        default_retention(),
        SegmentCodec::Identity,
    )?;
    sink.deliver(
        7,
        QosClass::P2MarketJournal,
        &batch(vec![
            record(SIMULATED_DATASET, "obj-ACME", 1)?,
            record("lse-level2", "obj-ACME", 2)?,
        ])?,
    )?;
    let keys = lake.list("lake/class=market/")?;
    assert_eq!(
        keys.len(),
        1,
        "premise: the simulator's own depth was kept: {keys:?}"
    );
    assert!(keys[0].contains(&format!("entitlement={SIMULATED_DATASET}/")));
    assert_eq!(
        sink.withheld(),
        1,
        "the licensed tick nobody granted retention to is counted, not kept"
    );

    let granted = parse_retention(Some("lse-level2=canonical"))?;
    let mut sink = LakeSink::new(
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        "edge-2",
        granted,
        SegmentCodec::Identity,
    )?;
    sink.deliver(
        8,
        QosClass::P2MarketJournal,
        &batch(vec![record("lse-level2", "obj-ACME", 3)?])?,
    )?;
    assert!(
        lake.list("lake/class=market/")?
            .iter()
            .any(|k| k.contains("entitlement=lse-level2/")),
        "once its terms are stated, the dataset is kept"
    );
    assert_eq!(sink.withheld(), 0);
    Ok(())
}

/// Mutation: make `parse_retention` ignore an unknown right instead of
/// refusing it — the typo reads as "retain nothing".
#[test]
fn a_retention_setting_that_cannot_be_read_is_refused_by_name() -> Result<()> {
    assert_eq!(parse_retention(None)?, default_retention());
    assert!(parse_retention(Some("lse-level2=canonicl")).is_err());
    assert!(parse_retention(Some("lse-level2")).is_err());
    assert!(
        parse_retention(Some("lse-level2=raw")).is_err(),
        "raw without its record is refused"
    );
    let policy = parse_retention(Some("lse-level2=canonical|raw; xpar=none"))?;
    assert!(policy["lse-level2"].raw && policy["lse-level2"].canonical);
    assert!(!policy["xpar"].canonical);
    Ok(())
}

/// Mutation: ignore the sink's codec in `deliver` (always `Lake::new`) — the
/// segment is stored under `.canonical` though zstd was configured.
#[test]
fn a_sink_configured_with_zstd_seals_compressed_segments() -> Result<()> {
    let lake = Arc::new(MemoryBlobStore::new());
    let mut sink = LakeSink::new(
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        "edge-1",
        default_retention(),
        SegmentCodec::zstd(3)?,
    )?;
    sink.deliver(
        1,
        QosClass::P2MarketJournal,
        &batch(vec![record(SIMULATED_DATASET, "obj-ACME", 1)?])?,
    )?;
    let keys = lake.list("lake/class=market/")?;
    assert_eq!(keys.len(), 1);
    assert!(keys[0].ends_with(".canonical.zst"), "{keys:?}");
    Ok(())
}
