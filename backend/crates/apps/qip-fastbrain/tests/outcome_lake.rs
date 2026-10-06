//! The node's own loop seals what the platform did into the Tick/Internal
//! Lake (blueprint TICK-065).
//!
//! `qip-storage`'s own suite proves the archive seals outcomes when it is
//! handed records. That proves nothing about a binary: the lake had a tested
//! writer and no caller for as long as no loop handed it anything. This drives
//! the loop `main.rs` runs — the real platform, the real feed, the real
//! hand-over between cycles and the flush on the way out — over an archive
//! built the way `main.rs` builds it, and reads the lake afterwards.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_fastbrain::config::FastBrainConfig;
use qip_fastbrain::feed::Feed;
use qip_fastbrain::node;
use qip_fastbrain::status::NodeStatus;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::lake::LAKE_NAMESPACE;
use qip_storage::settings::StorageSettings;
use qip_storage::{BlobStore, ChainArchive, MemoryKeyValueStore};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// Every record in the lake's internal class, by its own record hash. Read
/// as JSON rather than as the log's record type, which this crate does not
/// name: the comparison below is against the log's record serialised the same
/// way, so nothing is lost by it.
fn sealed(blobs: &dyn BlobStore) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    for key in blobs.list("lake/class=internal/").expect("the lake lists") {
        let bytes = blobs
            .get(&key)
            .expect("reads")
            .expect("a listed key exists");
        for line in String::from_utf8(bytes).expect("utf-8").lines() {
            let record: serde_json::Value = serde_json::from_str(line).expect("a JSON line");
            let hash = record["record_hash"].as_str().expect("a record hash");
            out.insert(hash.to_string(), record);
        }
    }
    out
}

#[test]
fn the_run_loop_seals_every_outcome_the_cycles_recorded_into_the_internal_lake_and_no_market_event()
{
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start()));
    let config = PlatformConfig::default();
    let context = qip_core::Context::new(clock.clone(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .expect("the platform assembles");
    let mut feed = Feed::synthetic(
        5,
        Duration::from_secs(60),
        start().saturating_sub(Duration::from_mins(30)),
    );

    // Built as `main.rs` builds it: the lake is whatever blob store the
    // configured storage resolves the lake namespace to.
    let storage = StorageSettings::in_memory();
    let blobs = storage.blobs(LAKE_NAMESPACE).expect("the lake opens");
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
        .expect("an empty archive opens")
        .with_outcome_lake(blobs.clone());

    let config = FastBrainConfig {
        cycle_interval: Duration::from_millis(1),
        max_cycles: Some(3),
        archive_every: 1,
        ..FastBrainConfig::default()
    };
    let cleared = qip_fastbrain::roster::clear(start()).expect("the roster clears");
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &cleared,
        &config,
        "synthetic-exchange",
        false,
        start(),
    )));
    let stop = Arc::new(AtomicBool::new(false));

    let summary = node::run(
        &mut platform,
        &mut feed,
        &archive,
        &config,
        &status,
        &stop,
        &clock,
        |_| {},
    )
    .expect("the loop runs");
    assert_eq!(summary.cycles, 3);
    node::flush(&platform, &archive, false, Duration::from_secs(5)).expect("the flush runs");

    // Premise: the cycles recorded both kinds. Without an outcome there is
    // nothing the lake could have missed, and without a market event there is
    // nothing it could have wrongly kept.
    let records = platform.event_log().records();
    let own: Vec<_> = records
        .iter()
        .filter(|record| record.event.topic.requires_permanent_retention())
        .collect();
    let world = records.len() - own.len();
    assert!(!own.is_empty(), "premise: three cycles recorded no outcome");
    assert!(world > 0, "premise: three cycles recorded no market event");

    let lake = sealed(blobs.as_ref());
    for record in &own {
        assert_eq!(
            lake.get(&record.record_hash),
            Some(&serde_json::to_value(record).expect("a record serialises")),
            "{:?} at sequence {} is in the log and not in the lake",
            record.event.topic,
            record.sequence
        );
    }
    assert_eq!(
        lake.len(),
        own.len(),
        "the lake holds a record that is not one of the platform's own outcomes"
    );
}
