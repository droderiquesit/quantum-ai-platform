//! The deep brain's tick desk, reached from its run loop: recorded market
//! history pinned out of the lake, replayed tick by tick, labelled, trained
//! on, fused with the world model, distilled into a map shipped only where
//! the licence permits, and explored with synthetic futures (R12-TICK-2059,
//! 2061, 2065, 2067, 2068, 2069, 2076, 2081–2086, 2091, 7199, 2057;
//! R12-DATA-1039).
//!
//! Each test asserts its premise first: that the lake holds history under
//! the granted entitlement, so an empty lake cannot pass for a quiet one.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::governance::{Entitlement as Licence, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::{MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Clock, Context, Decimal, Duration, ManualClock, ObjectId, Timestamp};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::status::NodeStatus;
use qip_deepbrain::tick_desk::{
    ENTITLEMENTS_VARIABLE, EVERY_VARIABLE, MEASURE_CODEC_VARIABLE, OUTPUT_NAMESPACE, TickDesk,
    TickDeskConfig,
};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_simulation_engine::tick_replay::{OrderSide, ReplayOrder};
use qip_storage::lake::{Entitlement, Lake, Partition, RecordClass};
use qip_storage::lake_codec::SegmentCodec;
use qip_storage::{BlobStore, ChainArchive, MemoryBlobStore, MemoryKeyValueStore};
use qip_twin::futures::Provenance;
use qip_world_model::graph::{EntityKind, Fact, KnowledgeGraph, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// 2026-10-11T00:00:00Z plus `ms`.
fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

fn d(v: f64) -> Decimal {
    Decimal::from_f64(v).unwrap_or_else(|| panic!("{v} is representable"))
}

/// One tick in the edge node's lake line shape.
fn line(sequence: u64, ms: i64, body: MessageBody, dataset: &str) -> Result<String> {
    let message = MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        Origin::new(VenueId::new("XLON"), "itch-a", 0, sequence),
        body,
        at(ms),
        at(ms),
    );
    let hash = MarketEvent::hash_payload(&message)?;
    let event = MarketEvent::new(
        message,
        at(ms),
        at(ms + 3),
        at(ms + 3),
        Duration::ZERO,
        hash,
        Licence::Granted {
            dataset: dataset.into(),
            usage: Usage::Trade,
            expires_at: Timestamp::MAX,
        },
    )?;
    Ok(qip_core::canonical::canonical_json(&serde_json::json!({
        "exchange_time_ns": at(ms).as_nanos(),
        "capture_time_ns": at(ms + 3).as_nanos(),
        "feed": "itch-a",
        "partition": 0,
        "sequence": sequence,
        "event": serde_json::to_value(&event)?,
    })))
}

/// Five minutes of a book whose next move follows its touch imbalance, as
/// the edge node would have sealed it: a quote each second, and half a
/// second later a print through the bid after a thin bid or at the ask after
/// a thick one.
fn seal_session(store: &MemoryBlobStore, dataset: &str) -> Result<usize> {
    let mut lines = Vec::new();
    let mut mid = 100.0;
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut sequence = 0;
    for i in 0..300 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let thin = state.is_multiple_of(2);
        let (bid_size, ask_size) = if thin { (2.0, 8.0) } else { (8.0, 2.0) };
        let t = i * 1_000;
        sequence += 1;
        lines.push(line(
            sequence,
            t,
            MessageBody::Quote {
                bid: Some((d(mid - 0.01), d(bid_size))),
                ask: Some((d(mid + 0.01), d(ask_size))),
            },
            dataset,
        )?);
        sequence += 1;
        lines.push(line(
            sequence,
            t + 500,
            MessageBody::Trade {
                price: d(if thin { mid - 0.02 } else { mid + 0.01 }),
                quantity: d(1.0),
                condition: TradeCondition::Regular,
                aggressor: None,
            },
            dataset,
        )?);
        mid += if thin { -0.01 } else { 0.01 };
    }
    let partition = Partition::new(
        RecordClass::Market,
        "XLON",
        "2026-10-11",
        "obj-ACME",
        Entitlement {
            id: dataset.into(),
            raw_retention_permitted: false,
        },
    )?;
    let mut body = Vec::new();
    for l in &lines {
        body.extend_from_slice(l.as_bytes());
        body.push(b'\n');
    }
    Lake::new(store).write_segment(&partition, "edge-1-o00000000000000000001", body, None)?;
    Ok(lines.len())
}

/// The world model's view: a strike at ACME, learned at 100 s, expected to
/// push the price down.
fn graph() -> Result<KnowledgeGraph> {
    let mut graph = KnowledgeGraph::new();
    graph.add_node(
        Node::event("evt-strike", "strike", at(90_000), at(100_000))
            .with_attribute("expected_direction", "down")
            .with_attribute("evidence_quality", "0.9"),
    )?;
    graph.add_node(Node::entity("acme", EntityKind::Company, "Acme", at(0)))?;
    graph.add_node(
        Node::new("inst-acme", NodeKind::FinancialObject, "ACME", at(0))
            .with_attribute("object_id", "obj-ACME"),
    )?;
    graph.assert_fact(Fact::new(
        Relationship::new(
            "evt-strike",
            "acme",
            RelationshipKind::ConcernsEntity,
            1.0,
            "wire:1",
        ),
        at(90_000),
        at(100_000),
        0.9,
    )?);
    graph.assert_fact(Fact::new(
        Relationship::new(
            "acme",
            "inst-acme",
            RelationshipKind::Issues,
            1.0,
            "registry:1",
        ),
        at(0),
        at(0),
        1.0,
    )?);
    Ok(graph)
}

fn config() -> TickDeskConfig {
    let mut config = TickDeskConfig::off();
    config.every_cycles = 1;
    config.granted = BTreeSet::from(["vendor-depth".to_string()]);
    config.entitlement_regions = BTreeMap::from([(
        "vendor-depth".to_string(),
        BTreeSet::from(["europe-west2".to_string()]),
    )]);
    config.map_regions = BTreeSet::from(["europe-west2".to_string(), "us-east4".to_string()]);
    config.measure_codec = Some(SegmentCodec::Zstd { level: 3 });
    config
}

/// Mutation: make `TickDeskConfig::from_lookup` default the cadence to 1,
/// or accept an empty grant — the desk runs on configuration nobody gave it.
#[test]
fn the_desk_is_off_by_default_and_refuses_to_start_on_with_no_entitlement_grant() -> Result<()> {
    let none = TickDeskConfig::from_lookup(&|_| None)?;
    assert!(!none.enabled());
    let on_without_grant =
        TickDeskConfig::from_lookup(&|name| (name == EVERY_VARIABLE).then(|| "3".to_string()));
    assert!(
        on_without_grant.is_err(),
        "an enabled desk with nothing granted is refused"
    );
    let bad_codec = TickDeskConfig::from_lookup(&|name| match name {
        n if n == EVERY_VARIABLE => Some("3".into()),
        n if n == ENTITLEMENTS_VARIABLE => Some("vendor-depth".into()),
        n if n == MEASURE_CODEC_VARIABLE => Some("lz4".into()),
        _ => None,
    });
    assert!(
        bad_codec.is_err(),
        "a codec this lake does not write is refused by name"
    );
    Ok(())
}

/// Mutation: in `TickDesk::round`, skip `pin_market_history` (read the
/// segments directly), drop the replay, skip the label job, or ship the map
/// without `authorise_for` — the respective assertion fails.
#[test]
fn a_round_replays_the_lake_trains_on_its_labels_and_ships_the_map_only_where_the_licence_permits()
-> Result<()> {
    let lake = Arc::new(MemoryBlobStore::new());
    let ticks = seal_session(&lake, "vendor-depth")?;
    seal_session(&lake, "research-only")?;
    let outputs = Arc::new(MemoryBlobStore::new());
    let mut desk = TickDesk::new(
        config(),
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        Arc::clone(&outputs) as Arc<dyn BlobStore>,
    );
    let now = at(400_000);
    let decisions = vec![ReplayOrder {
        id: "decision-1".into(),
        instrument: "obj-ACME".into(),
        side: OrderSide::Buy,
        quantity: d(5.0),
        limit: None,
        decided_at: at(150_000),
    }];
    let round = desk.round(&graph()?, now, decisions)?;

    assert_eq!(round.segments, 1, "premise: the granted segment was pinned");
    assert_eq!(
        round.withheld, 1,
        "the research-only segment was withheld and never read"
    );
    assert_eq!(round.ticks, ticks);
    assert_eq!(round.unparsed, 0);
    assert_eq!(
        round.entitlements_read,
        BTreeSet::from(["vendor-depth".to_string()])
    );
    let replay = round.replay.as_ref().expect("the history was replayed");
    assert_eq!(
        replay.steps as usize, ticks,
        "every recorded tick was replayed"
    );
    let execution = &replay.executions[0];
    assert!(execution.filled.is_positive() && execution.fees.is_positive());
    assert_eq!(
        execution.arrived_at,
        Some(at(150_005)),
        "the decision met the book after latency"
    );

    assert!(
        round.examples > 100,
        "labels were built from the replayed books: {}",
        round.examples
    );
    let model = round.fill_model.as_ref().expect("a fill model was trained");
    assert!(
        model.beats_baseline(),
        "{} vs {}",
        model.holdout_brier,
        model.baseline_brier
    );
    assert!(round.benchmark.as_ref().is_some_and(|b| !b.rows.is_empty()));

    let card = round.model_card.as_ref().expect("a card was issued");
    assert_eq!(
        card.data_entitlements,
        BTreeSet::from(["vendor-depth".to_string()])
    );
    assert_eq!(
        card.permitted_locations,
        BTreeSet::from(["europe-west2".to_string()])
    );
    assert!(
        outputs
            .list(&format!("{OUTPUT_NAMESPACE}/model-cards/"))?
            .len()
            == 1
    );

    assert_eq!(
        round.fused_events, 1,
        "the strike was fused with the replayed market"
    );
    assert_eq!(
        round.shipped_to,
        BTreeSet::from(["europe-west2".to_string()])
    );
    assert!(round.refused_regions.contains_key("us-east4"));
    assert_eq!(
        outputs
            .list(&format!(
                "{OUTPUT_NAMESPACE}/opportunity-maps/region=europe-west2/"
            ))?
            .len(),
        1
    );
    assert!(
        outputs
            .list(&format!(
                "{OUTPUT_NAMESPACE}/opportunity-maps/region=us-east4/"
            ))?
            .is_empty()
    );

    assert!(round.branches_grown > 1, "a future tree was grown");
    let tree = desk.tree().expect("the desk holds its tree");
    assert!(
        tree.branches
            .values()
            .all(|b| b.provenance == Provenance::Synthetic)
    );
    let compression = round.compression.as_ref().expect("the codec was measured");
    assert!(compression.total().ratio().is_some_and(|r| r < 1.0));

    // A later round pins the same bytes, replays the same market, and scores
    // the previous round's futures against what happened since.
    let again = desk.round(&graph()?, at(460_000), Vec::new())?;
    assert_eq!(again.ticks, ticks);
    assert!(
        again.tree_log_score.is_some(),
        "the previous tree was scored against reality"
    );
    Ok(())
}

/// Mutation: drop the `tick` arm from `node::run_with` — no cycle carries a
/// tick round, though the desk is configured and the lake holds history.
#[test]
fn the_run_loop_runs_the_tick_desk_on_its_cadence() -> Result<()> {
    let start = at(400_000);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start));
    let platform_config = PlatformConfig::default();
    let context = Context::new(clock.clone(), platform_config.seed);
    let mut platform = Platform::new(
        platform_config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;
    let node_config = DeepBrainConfig {
        cycle_interval: Duration::from_millis(1),
        max_cycles: Some(2),
        ..DeepBrainConfig::default()
    };
    let roster = qip_deepbrain::roster::clear(start)?;
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &roster,
        &node_config,
        start,
    )));
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))?;
    let lake = Arc::new(MemoryBlobStore::new());
    let ticks = seal_session(&lake, "vendor-depth")?;
    assert!(ticks > 0, "premise: the lake holds history");
    let mut desk = TickDesk::new(
        config(),
        lake as Arc<dyn BlobStore>,
        Arc::new(MemoryBlobStore::new()),
    );
    let mut rounds = Vec::new();
    node::run_with(
        &mut platform,
        &archive,
        &node_config,
        &status,
        &Arc::new(AtomicBool::new(false)),
        &clock,
        0,
        None,
        Loops {
            research: None,
            operations: None,
            tick: Some(&mut desk),
        },
        |outcome| {
            assert!(
                outcome.side_failures.is_empty(),
                "{:?}",
                outcome.side_failures
            );
            if let Some(round) = &outcome.tick {
                rounds.push((round.segments, round.ticks));
            }
        },
    )?;
    assert_eq!(
        rounds,
        vec![(1, ticks), (1, ticks)],
        "a round on each of the two cycles"
    );
    Ok(())
}
