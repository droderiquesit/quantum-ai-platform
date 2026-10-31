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
use qip_core::ids::OrderId;
use qip_core::{Clock, Context, Decimal, Duration, ManualClock, ObjectId, Timestamp};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::status::NodeStatus;
use qip_deepbrain::tick_desk::{
    ENTITLEMENTS_VARIABLE, EVERY_VARIABLE, MEASURE_CODEC_VARIABLE, OUTPUT_NAMESPACE, TickDesk,
    TickDeskConfig, skill_gate,
};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance as Quality;
use qip_financial::universe::Universe;
use qip_kernel::platform::FillScore;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_simulation_engine::tick_replay::{OrderSide, ReplayOrder};
use qip_storage::lake::{Entitlement, Lake, Partition, RecordClass};
use qip_storage::lake_codec::SegmentCodec;
use qip_storage::{BlobStore, ChainArchive, MemoryBlobStore, MemoryKeyValueStore};
use qip_training::microstructure::{FillModel, Scaling};
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
/// One tick received `received_after_ms` after the venue stamped it — a
/// negative value is a tick the platform says it knew before it happened.
fn stamped_line(
    sequence: u64,
    ms: i64,
    received_after_ms: i64,
    uncertainty: Duration,
    body: MessageBody,
    dataset: &str,
) -> Result<String> {
    let message = MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        Origin::new(VenueId::new("XLON"), "itch-a", 0, sequence),
        body,
        at(ms),
        at(ms),
    );
    let hash = MarketEvent::hash_payload(&message)?;
    // Admission refuses an event received before it happened by more than
    // its doubt (ADR 0132, `qip_contracts::admission`), so a contaminated
    // line cannot be built through `MarketEvent::new`. The fixture builds
    // the admitted event and then writes the earlier receipt into its lake
    // line by hand: the line a producer without admission could have left
    // on the lake, which is what the replay's own exclusion exists for.
    let received = at(ms + received_after_ms);
    let event = MarketEvent::new(
        message,
        at(ms),
        at(ms + received_after_ms.max(0)),
        at(ms + received_after_ms.max(0)),
        uncertainty,
        hash,
        Licence::Granted {
            dataset: dataset.into(),
            usage: Usage::Trade,
            expires_at: Timestamp::MAX,
        },
    )?;
    let mut event = serde_json::to_value(&event)?;
    if received_after_ms < 0 {
        event["receive_time"] = serde_json::to_value(received)?;
        event["normalized_time"] = serde_json::to_value(received)?;
    }
    Ok(qip_core::canonical::canonical_json(&serde_json::json!({
        "exchange_time_ns": at(ms).as_nanos(),
        "capture_time_ns": received.as_nanos(),
        "feed": "itch-a",
        "partition": 0,
        "sequence": sequence,
        "event": event,
    })))
}

/// Five minutes of a book whose next move follows its touch imbalance, as
/// the edge node would have sealed it: a quote each second, and half a
/// second later a print through the bid after a thin bid or at the ask after
/// a thick one.
fn seal_session(store: &MemoryBlobStore, dataset: &str) -> Result<usize> {
    seal_session_uncertain(store, dataset, Duration::ZERO)
}

/// [`seal_session`], every tick captured with `uncertainty` of clock doubt.
fn seal_session_uncertain(
    store: &MemoryBlobStore,
    dataset: &str,
    uncertainty: Duration,
) -> Result<usize> {
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
        lines.push(stamped_line(
            sequence,
            t,
            3,
            uncertainty,
            MessageBody::Quote {
                bid: Some((d(mid - 0.01), d(bid_size))),
                ask: Some((d(mid + 0.01), d(ask_size))),
            },
            dataset,
        )?);
        sequence += 1;
        lines.push(stamped_line(
            sequence,
            t + 500,
            3,
            uncertainty,
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
    // R12-CONTRACT-6110: the live branches cross as WorldBranch contracts, and
    // none is silently absent: every live branch was either written or refused
    // by the contract (the baseline is its parent's own future).
    assert!(
        round.world_branches > 0 && round.world_branches_refused > 0,
        "premise: some live branch is stateable and the baseline is not ({} written, {} refused)",
        round.world_branches,
        round.world_branches_refused
    );
    assert_eq!(
        round.world_branches + round.world_branches_refused,
        round.branches_live
    );
    let written = outputs.list(&format!("{OUTPUT_NAMESPACE}/world-branches/"))?;
    assert_eq!(written.len(), 1, "one file per round");
    let contracts: Vec<qip_contracts::WorldBranch> =
        serde_json::from_slice(&outputs.get(&written[0])?.expect("the file was listed"))?;
    assert_eq!(contracts.len(), round.world_branches);
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
            desk: None,
            research: None,
            operations: None,
            tick: Some(&mut desk),
            permissions: None,
            ambient: None,
            agency: None,
            warehouse: None,
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

/// R12-TICK-1049, 1050, 1051: every training row the desk builds from
/// replayed history carries the venue, the venue's own sequence and the
/// entitlement of the message it was drawn at.
///
/// Mutation: set `sequence: 0` or `entitlement: String::new()` where
/// `build_examples` builds a `MicroExample`, or `venue` to the instrument —
/// the row no longer names its message, and the matching assertion fails.
#[test]
fn every_training_row_carries_its_venue_sequence_and_entitlement() -> Result<()> {
    let lake = Arc::new(MemoryBlobStore::new());
    let ticks = seal_session(&lake, "vendor-depth")?;
    let mut desk = TickDesk::new(
        config(),
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        Arc::new(MemoryBlobStore::new()),
    );
    let round = desk.round(&graph()?, at(400_000), Vec::new())?;
    assert_eq!(round.ticks, ticks, "premise: the history was read");
    assert!(
        round.labelled.len() > 100,
        "premise: rows were labelled: {}",
        round.labelled.len()
    );
    assert_eq!(round.labelled.len(), round.examples);
    // `seal_session` stamps sequence 2i+1 (a quote) at i seconds and 2i+2
    // (a print) half a second later, each received 3 ms after; so a row's
    // sequence fixes the instant it must have been drawn at.
    let drawn_at = |sequence: u64| {
        let i = (sequence - 1) / 2;
        at(i as i64 * 1_000 + ((sequence - 1) % 2) as i64 * 500 + 3)
    };
    assert!(
        round.labelled.iter().all(|r| r.sequence > 0),
        "a row names no sequence"
    );
    if let Some(row) = round.labelled.iter().find(|r| drawn_at(r.sequence) != r.at) {
        panic!(
            "a row at {} names sequence {}, which was knowable at {}",
            row.at,
            row.sequence,
            drawn_at(row.sequence)
        );
    }
    let sequences: BTreeSet<u64> = round.labelled.iter().map(|r| r.sequence).collect();
    assert!(
        sequences.len() > 50,
        "rows share sequences: {} distinct over {} rows",
        sequences.len(),
        round.labelled.len()
    );
    assert!(round.labelled.iter().all(|r| r.venue == "XLON"));
    assert!(
        round
            .labelled
            .iter()
            .all(|r| r.entitlement == "vendor-depth"),
        "a row does not name the entitlement it was obtained under"
    );
    Ok(())
}

/// R12-TICK-1053: a tick stamped knowable before the venue says it happened
/// is excluded from replay and training and counted, never read on either
/// clock.
///
/// Mutation: drop the `knowable_at < event_time` exclusion in
/// `TickDesk::round` and the contaminated tick is replayed.
#[test]
fn a_tick_received_before_it_happened_is_excluded_from_replay_and_counted() -> Result<()> {
    let lake = Arc::new(MemoryBlobStore::new());
    let clean = seal_session(&lake, "vendor-depth")?;
    // A print the platform claims to have received two seconds before the
    // venue stamped it.
    let contaminated = stamped_line(
        10_000,
        200_000,
        -2_000,
        Duration::ZERO,
        MessageBody::Trade {
            price: d(100.0),
            quantity: d(1.0),
            condition: TradeCondition::Regular,
            aggressor: None,
        },
        "vendor-depth",
    )?;
    let partition = Partition::new(
        RecordClass::Market,
        "XLON",
        "2026-10-11",
        "obj-ACME",
        Entitlement {
            id: "vendor-depth".into(),
            raw_retention_permitted: false,
        },
    )?;
    let mut body = contaminated.into_bytes();
    body.push(b'\n');
    Lake::new(lake.as_ref()).write_segment(
        &partition,
        "edge-1-o00000000000000000002",
        body,
        None,
    )?;
    let mut desk = TickDesk::new(
        config(),
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        Arc::new(MemoryBlobStore::new()),
    );
    let round = desk.round(&graph()?, at(400_000), Vec::new())?;
    assert_eq!(round.segments, 2, "premise: both segments were pinned");
    assert_eq!(round.unparsed, 0, "premise: the contaminated tick parsed");
    assert_eq!(round.contaminated, 1);
    assert_eq!(round.ticks, clean, "the contaminated tick was replayed");
    Ok(())
}

/// A fill model whose holdout skill does not beat the base rate is refused
/// a card (R12-TICK-2092), and the same model with a better holdout Brier
/// passes, so the gate is the comparison and not a refusal of everything.
/// The gate is the one `TickDesk::round` runs before `ModelCard::new`, so a
/// refusal here is a round with `card_refused` set and no map authorised.
/// Mutation: make `skill_gate` return `Ok(())` unconditionally — the first
/// assertion fails.
#[test]
fn a_fill_model_that_does_not_beat_its_baseline_issues_no_card() {
    let model = |holdout_brier: f64| FillModel {
        features: vec!["spread".to_string()],
        scaling: Scaling {
            means: vec![0.0],
            scales: vec![1.0],
        },
        intercept: 0.0,
        weights: vec![0.1],
        base_rate: 0.4,
        train_examples: 40,
        holdout_examples: 10,
        holdout_brier,
        baseline_brier: 0.24,
    };
    let unskilled = model(0.25);
    assert!(
        !unskilled.beats_baseline(),
        "premise: 0.25 does not beat 0.24"
    );
    let refused = skill_gate(&unskilled).expect_err("no card for a model with no skill");
    assert!(
        refused
            .message()
            .contains("does not beat its base-rate baseline"),
        "{refused}"
    );
    let skilled = model(0.20);
    assert!(skilled.beats_baseline(), "premise: 0.20 beats 0.24");
    skill_gate(&skilled).expect("a skilled model passes the gate");
}

fn fill(venue: &str, ms: i64, error: Option<f64>) -> FillScore {
    FillScore {
        order_id: OrderId::from_string(format!("ord-{venue}-{ms}")),
        object_id: ObjectId::from_string("obj-ACME".to_string()),
        venue: venue.to_string(),
        filled_at: at(ms),
        scored_at: at(ms + 1),
        smaller_favoured: false,
        larger_favoured: false,
        trade_error_bps: error,
        unfilled: Decimal::ZERO,
        realised_slippage_bps: None,
        predicted_slippage_bps: None,
        predicted_outcome_bps: None,
        realised_outcome_bps: None,
        outcome_error_bps: None,
    }
}

/// R12-TICK-6014: a venue fill whose simulated entry was off becomes a
/// training row, joined to the labelled instant it followed.
///
/// The failure this prevents: `qip_venue_fill_error_bps` charted how wrong
/// the twin was about every fill and no microstructure model was ever fitted
/// or judged on those fills, so the number a desk looked at never reached
/// the data that could correct it. Mutation: passing `&[]` for the fills in
/// `round_with_fills`'s join, or dropping the join, empties the rows and the
/// first assertion after the premise fails.
#[test]
fn a_mispriced_fill_becomes_a_microstructure_training_row() -> Result<()> {
    let lake = Arc::new(MemoryBlobStore::new());
    seal_session(&lake, "vendor-depth")?;
    let mut desk = TickDesk::new(
        config(),
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        Arc::new(MemoryBlobStore::new()),
    );
    let now = at(400_000);
    let fills = vec![
        // Inside the replayed history, on a venue it covers: a row.
        fill("XLON", 250_000, Some(7.5)),
        // On a venue the history does not cover: counted, never matched.
        fill("XNYS", 250_000, Some(3.0)),
        // Before the first replayed instant: no example to follow.
        fill("XLON", -5_000, Some(1.0)),
        // The twin never filled, so there is no error to learn from.
        fill("XLON", 260_000, None),
    ];
    let round = desk.round_with_fills(&graph()?, now, Vec::new(), &fills)?;

    assert!(
        round.examples > 100,
        "premise: the history was labelled: {}",
        round.examples
    );
    assert_eq!(round.residuals_offered, 3, "a fill with no error is no row");
    let [row] = round.residual_examples.as_slice() else {
        panic!("expected one residual row: {:?}", round.residual_examples);
    };
    assert_eq!(round.residuals_unmatched, 2);
    assert_eq!(row.example.venue, "XLON");
    assert_eq!(row.example.instrument, "obj-ACME");
    assert!((row.error_bps - 7.5).abs() < 1e-12);
    assert!(
        row.example.at <= at(250_000),
        "the row was drawn after the fill it is joined to: {:?}",
        row.example.at
    );
    assert!(
        row.example.at > at(240_000),
        "the row is not the nearest instant before the fill: {:?}",
        row.example.at
    );
    assert!(
        round.describe().contains("1 of 3 venue fill(s) joined"),
        "{}",
        round.describe()
    );
    Ok(())
}

/// R12-RES-7103: a window captured under a clock too imprecise for the
/// shortest label horizon is refused whole by name, and nothing is labelled
/// or trained from it. The same session captured exactly is labelled (the
/// premise).
///
/// Mutation: admit every window in `TickDesk::round_with_fills`.
#[test]
fn a_window_captured_under_a_degraded_clock_is_not_labelled_or_trained_on() -> Result<()> {
    let exact = Arc::new(MemoryBlobStore::new());
    seal_session(&exact, "vendor-depth")?;
    let mut desk = TickDesk::new(
        config(),
        Arc::clone(&exact) as Arc<dyn BlobStore>,
        Arc::new(MemoryBlobStore::new()),
    );
    let round = desk.round(&graph()?, at(400_000), Vec::new())?;
    assert!(round.examples > 100, "premise: an exact window is labelled");

    let doubtful = Arc::new(MemoryBlobStore::new());
    let ticks = seal_session_uncertain(&doubtful, "vendor-depth", Duration::from_secs(2))?;
    let mut desk = TickDesk::new(
        config(),
        Arc::clone(&doubtful) as Arc<dyn BlobStore>,
        Arc::new(MemoryBlobStore::new()),
    );
    let round = desk.round(&graph()?, at(400_000), Vec::new())?;
    assert_eq!(round.ticks, ticks, "premise: the history was read");
    assert_eq!(
        round.examples, 0,
        "a window with two seconds of clock doubt was labelled"
    );
    assert!(round.labelled.is_empty());
    assert!(
        round.fill_model.is_none(),
        "a model was trained on an imprecise window"
    );
    assert!(
        round
            .training_refused
            .iter()
            .any(|r| r.starts_with("labels: ") && r.contains(&round.manifest)),
        "the refusal does not name the window: {:?}",
        round.training_refused
    );
    Ok(())
}

/// An order the platform sent, as the kernel's order manager holds it.
fn platform_order(
    id: &str,
    order_type: qip_execution_engine::order::OrderType,
    created_ms: i64,
    filled_at: Option<f64>,
) -> qip_execution_engine::order::Order {
    use qip_execution_engine::order::{Fill, Order, Side};
    let mut order = Order::new(
        OrderId::from_string(id),
        ObjectId::from_string("obj-ACME"),
        Side::Buy,
        d(5.0),
        order_type,
        d(100.0),
        "proposal-1",
        vec!["hypothesis-1".to_string()],
        "desk",
        at(created_ms),
    );
    if let Some(price) = filled_at {
        order.fills.push(Fill {
            fill_id: qip_core::ids::FillId::from_string(format!("fill-{id}")),
            order_id: OrderId::from_string(id),
            at: at(created_ms + 10),
            quantity: d(5.0),
            price: d(price),
            costs: Decimal::ZERO,
            venue: "XLON".to_string(),
            simulated: true,
        });
    }
    order
}

/// R12-TICK-2073: the round the run loop calls replays the platform's own
/// orders against the recorded market and records, per order, the replayed
/// fill beside the venue's. A scheduled algorithm the replay cannot express
/// is counted, not replayed as a market order nobody sent, and an order
/// from outside the window is not replayed at all. Mutation: hand
/// `round_with_fills` `Vec::new()` in `maybe_round_with_orders` again — no
/// order row is recorded and the first assertion fails.
#[test]
fn a_tick_round_replays_the_platforms_own_orders_and_records_fill_versus_assumption_rows()
-> Result<()> {
    use qip_execution_engine::order::OrderType;
    let lake = Arc::new(MemoryBlobStore::new());
    seal_session(&lake, "vendor-depth")?;
    let outputs = Arc::new(MemoryBlobStore::new());
    let mut desk = TickDesk::new(
        config(),
        Arc::clone(&lake) as Arc<dyn BlobStore>,
        Arc::clone(&outputs) as Arc<dyn BlobStore>,
    );
    let now = at(400_000);
    let orders = vec![
        platform_order("market-1", OrderType::Market, 150_000, Some(100.02)),
        platform_order(
            "limit-1",
            OrderType::Limit { price: d(99.0) },
            160_000,
            None,
        ),
        platform_order(
            "twap-1",
            OrderType::TimeWeighted { minutes: 5 },
            170_000,
            None,
        ),
        // Two days before `now`: outside the desk's one-day lookback.
        platform_order("stale-1", OrderType::Market, -172_800_000, Some(100.0)),
    ];

    let round = desk
        .maybe_round_with_orders(&graph()?, 1, now, &orders, &[])?
        .expect("cadence 1 runs every cycle");

    let ids: Vec<&str> = round
        .order_rows
        .iter()
        .map(|row| row.order_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["market-1", "limit-1"],
        "the replay did not take the platform's own orders: {:?}",
        round.order_rows
    );
    assert_eq!(
        round.unreplayable_orders, 1,
        "the scheduled algorithm is counted"
    );
    let market = &round.order_rows[0];
    assert!(
        market.replayed_price.is_some(),
        "the recorded market filled the buy"
    );
    assert_eq!(market.actual_price, Some(d(100.02)));
    assert!(
        market.gap_bps.is_some_and(f64::is_finite),
        "the replayed fill sits beside the venue's: {market:?}"
    );
    let limit = &round.order_rows[1];
    assert_eq!(
        limit.actual_price, None,
        "an unfilled order has no actual price"
    );
    assert_eq!(limit.gap_bps, None);
    Ok(())
}

/// R12-TICK-2073: the run loop hands the tick desk the platform's own
/// orders, so a round replays what the desk actually sent against the market
/// that existed rather than an empty decision list. `node.rs` is the only
/// caller of `maybe_round_with_orders`, and until this test the handoff was
/// pinned by nothing: the run-loop test above holds no orders, so passing
/// `Vec::new()` again would have left every test green.
///
/// Mutation: hand the desk an empty order list in `node.rs` and every round
/// reports no order row and nothing unreplayable; the test fails.
#[test]
fn the_run_loop_hands_the_platforms_own_orders_to_the_tick_desk() -> Result<()> {
    let start = at(400_000);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start));
    let platform_config = PlatformConfig::default();
    let context = Context::new(clock.clone(), platform_config.seed);
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string("obj-ACME"),
            "obj-ACME",
            InstrumentType::CommonStock,
            LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue("XLON")
        .sector(Sector::InformationTechnology)
        .price(d(100.0))
        .provenance(Quality::synthetic("tick-desk-test", start))
        .build(start)?,
    )?;
    let mut platform = Platform::new(
        platform_config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )?;
    let order = platform.order_for(
        ObjectId::from_string("obj-ACME"),
        qip_execution_engine::order::Side::Buy,
        d(5.0),
        d(100.0),
        "proposal-1",
        vec!["hypothesis-1".to_string()],
        start,
    )?;
    platform.submit_order(order, start)?;
    let held = platform.orders().orders().count();
    assert!(held >= 1, "premise: the platform holds an order of its own");

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
            desk: None,
            research: None,
            operations: None,
            tick: Some(&mut desk),
            permissions: None,
            ambient: None,
            agency: None,
            warehouse: None,
        },
        |outcome| {
            assert!(
                outcome.side_failures.is_empty(),
                "{:?}",
                outcome.side_failures
            );
            if let Some(round) = &outcome.tick {
                rounds.push((round.order_rows.len(), round.unreplayable_orders));
            }
        },
    )?;
    assert_eq!(rounds.len(), 2, "a round on each of the two cycles");
    assert!(
        rounds
            .iter()
            .all(|(rows, unreplayable)| rows + unreplayable >= held),
        "the run loop handed the desk none of the platform's {held} order(s): {rounds:?}"
    );
    Ok(())
}
