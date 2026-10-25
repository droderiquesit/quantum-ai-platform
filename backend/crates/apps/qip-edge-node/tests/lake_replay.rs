//! A replay of retained tick-lake history yields the decisions the
//! original session made (R12-TICK-9006).
//!
//! `tick_replay.rs` and the lake suites prove that the lake reproduces
//! events, books and fills. None of them proves the claim a desk relies on
//! when it replays a session to explain what the cell did: that a cell fed
//! the retained history decides as the original did. A book that rebuilds
//! correctly is necessary for that and not sufficient. The cell decides
//! from its decoder's output, its sequencer, its features and its own fills,
//! and a replay that skipped any of them would rebuild the book and still
//! explain nothing.
//!
//! So the session runs twice through `run_pass`'s own decide-act-reconcile
//! step. The original run takes its market view from the simulated venue
//! through `SimulatedFeed`, and every pass's applied ticks are sealed by the
//! drain's production `LakeSink`. The replay runs a fresh cell, built
//! identically, that sees the market only as `qip_storage::lake_history`
//! reads it back. Those ticks go through `ReplayFeed` and the feed's own
//! decoder. The venue the replay sends to is a second simulator with the
//! same seed and the same exogenous seeding. That is the fill model, and it
//! is not a market view: the replayed cell never reads it.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::MarketMessage;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_core::ids::ObjectId;
use qip_core::lineage::Lineage;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, EventId, ManualClock, dec};
use qip_edge::cell::{Cell, CellConfig, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::assemble;
use qip_edge_node::event_fabric::drain::{LakeSink, Sink, default_retention};
use qip_edge_node::feed::{ReplayFeed, SIMULATED_DATASET, SimulatedFeed};
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass, run_replay_pass};
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
use qip_storage::lake::Lake;
use qip_storage::lake_codec::SegmentCodec;
use qip_storage::lake_history::HistoryQuery;
use qip_storage::{BlobStore, MemoryBlobStore};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const CELL: &str = "replay-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const ENVELOPE_KEY: &[u8] = b"lake-replay-envelope-key";
/// The simulator's seed, the same for the original venue and the replay's.
const SEED: u64 = 7;
/// The session's passes, one second apart.
const PASSES: std::ops::Range<i64> = 10..40;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

/// A grant bounded as the centre's issuer bounds every grant, signed.
fn grant() -> Result<VerifiedEnvelope> {
    let unsigned = CapitalEnvelope::new(
        StrategyId::new(STRATEGY),
        CELL,
        dec!("1000000"),
        dec!("100000"),
        dec!("50000"),
        vec![venue()],
        t(0),
        t(36_000),
        "alice@example.com",
        "unsigned",
    )?;
    let capital = unsigned.gross_limit();
    let unsigned = unsigned.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        2_500,
        Decimal::ONE,
        capital,
    )?)?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), ENVELOPE_KEY, CELL, t(1))
}

/// The cell, built the same way every time it is called: a strategy whose
/// one rule always holds and rests at mid, so every pass decides something
/// and the price it decides depends on the book. The clock is a fixed
/// manual one, so nothing the cell stamps can differ between two builds.
fn cell() -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let node = assemble(
        config,
        features,
        Arc::new(ManualClock::new(t(0))),
        allocation,
        None,
    )?;
    let mut cell = node.cell;
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("1")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    cell.deploy_with_pricing(
        compiled,
        compiler.into_program(),
        grant()?,
        PricingPolicy::rest_at_mid(Duration::from_secs(5))?,
    )?;
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![grant()?.signature().to_string()],
        },
        t(5),
    );
    let policy = VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, CELL, t(5))?;
    cell.apply_policy(policy, t(5))?;
    Ok(cell)
}

/// The simulated venue, permitted to list and seed the instrument, with
/// the touch every session opens on.
fn simulated_venue() -> Result<SimulatedGateway> {
    let mut permissions = CreationPermissions::new();
    for action in [
        CreationAction::Create,
        CreationAction::List,
        CreationAction::Seed,
    ] {
        let request = CreationRequest::new(action, VENUE, object().as_str(), "paper");
        for authority in CreationAuthority::ALL {
            permissions.record(authority, &request, "test fixture")?;
        }
    }
    let mut gateway = SimulatedGateway::new(venue(), SEED, t(0))?
        .with_listing_permissions(permissions, "paper")?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    Ok(gateway)
}

/// The exogenous seeding of pass `second`, applied to whichever venue the
/// run sends to: the touch moves every pass and an ask comes and goes, so
/// the book the replay must reproduce is not the first one.
fn seed(gateway: &mut SimulatedGateway, second: i64) -> Result<()> {
    let bid = dec!("99") - Decimal::from(second % 3);
    gateway.seed_touch(&object(), Side::Buy, bid, dec!("500"), t(second))?;
    if second % 4 == 0 {
        gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("300"), t(second))?;
    }
    Ok(())
}

/// What a pass decided, rendered whole: every order, fill, refusal, signal
/// and cancellation in the cell's report, every requote and every break.
/// The feed's own tally is left out on purpose. The live feed counts the
/// instruments the venue lists and the replay counts the instruments that
/// ticked, and that tally is not a decision.
fn decided(outcome: &PassOutcome) -> String {
    match outcome {
        PassOutcome::Ran {
            report,
            requotes,
            breaks,
            ..
        } => format!("ran {report:?} requotes {requotes:?} breaks {breaks:?}"),
        PassOutcome::Halted { fills, .. } => format!("halted fills {fills:?}"),
    }
}

/// One pass's applied ticks as the spool carries them to the drain.
fn batch_of(events: &[MarketEvent], pass: i64) -> Result<Batch> {
    let records = events
        .iter()
        .map(|event| {
            let payload = serde_json::to_value(event)?;
            let payload_hash = sha256_hex(qip_core::canonical::canonical_json(&payload).as_bytes());
            let id = format!("{CELL}/{pass}/{}", event.origin().sequence);
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

/// What a run decided, and the journal it left.
struct Run {
    decisions: Vec<String>,
    orders: usize,
    /// Every journal entry's sequence, instant and decision. The journal
    /// holds decisions a pass report does not carry, such as the venue
    /// chosen for each signal and the candidates it was chosen from.
    journal: Vec<String>,
    journal_digest: String,
}

impl Run {
    fn of(cell: &Cell, decisions: Vec<String>, orders: usize) -> Self {
        let entries = cell.journal().entries();
        Self {
            decisions,
            orders,
            journal: entries
                .iter()
                .map(|entry| {
                    format!(
                        "{} {} {:?}",
                        entry.sequence,
                        entry.at.as_nanos(),
                        entry.decision
                    )
                })
                .collect(),
            journal_digest: entries
                .last()
                .map(|entry| entry.digest.clone())
                .unwrap_or_default(),
        }
    }
}

/// The original session: the cell's market view from the simulated venue,
/// every pass's applied ticks sealed into `lake` by the drain's `LakeSink`.
/// Returns what it decided and how many ticks it sealed.
fn original_session(lake: &Arc<MemoryBlobStore>) -> Result<(Run, usize)> {
    let mut cell = cell()?;
    let mut gateway = simulated_venue()?;
    let mut feed = SimulatedFeed::new(venue());
    feed.attach(&mut cell)?;
    let mut sink = LakeSink::new(
        Arc::clone(lake) as Arc<dyn BlobStore>,
        CELL,
        REGION,
        default_retention(),
        SegmentCodec::Identity,
    )?;
    let mut stats = PassStats::default();
    let mut decisions = Vec::new();
    let mut orders = 0usize;
    let mut sealed = 0usize;
    for second in PASSES {
        seed(&mut gateway, second)?;
        let outcome = run_pass(
            &mut cell,
            &mut gateway,
            &mut feed,
            None,
            &mut stats,
            t(second),
        )?;
        if let PassOutcome::Ran { report, .. } = &outcome {
            orders += report.orders.len();
        }
        decisions.push(decided(&outcome));
        let applied = feed.take_captured();
        if !applied.is_empty() {
            sealed += applied.len();
            sink.deliver(
                second as u64,
                QosClass::P2MarketJournal,
                &batch_of(&applied, second)?,
            )?;
        }
    }
    Ok((Run::of(&cell, decisions, orders), sealed))
}

/// Every tick `lake` retains, read back through `qip_storage::lake_history`
/// (pinned, checksums verified) and grouped by the pass that received it.
fn retained_history(
    lake: &MemoryBlobStore,
) -> Result<(BTreeMap<Timestamp, Vec<MarketMessage>>, usize)> {
    let granted = BTreeSet::from([SIMULATED_DATASET.to_string()]);
    let reader = Lake::new(lake);
    reader.pin_market_history(
        "tick-9006",
        &HistoryQuery {
            from_date: "2000-01-01".to_string(),
            to_date: "2100-01-01".to_string(),
            venues: BTreeSet::new(),
            instruments: BTreeSet::new(),
            granted: granted.clone(),
            max_segments: 10_000,
            max_ticks: 100_000,
        },
    )?;
    let history = reader.read_market_history("tick-9006", &granted, 100_000)?;
    assert_eq!(history.unparsed, 0, "every retained line parses");
    let count = history.ticks.len();
    let mut by_pass: BTreeMap<Timestamp, Vec<MarketMessage>> = BTreeMap::new();
    for tick in history.ticks {
        by_pass
            .entry(tick.message.capture_time)
            .or_default()
            .push(tick.message);
    }
    Ok((by_pass, count))
}

/// The replay: a fresh cell built identically, whose only market view is
/// the retained history, sending to a fresh simulator under the same seed
/// and the same exogenous seeding.
fn replayed_session(history: &BTreeMap<Timestamp, Vec<MarketMessage>>) -> Result<Run> {
    let mut cell = cell()?;
    let mut gateway = simulated_venue()?;
    let mut feed = ReplayFeed::new(venue());
    feed.attach(&mut cell)?;
    let mut stats = PassStats::default();
    let mut decisions = Vec::new();
    let mut orders = 0usize;
    for second in PASSES {
        seed(&mut gateway, second)?;
        let ticks = history.get(&t(second)).map_or(&[][..], Vec::as_slice);
        let outcome = run_replay_pass(
            &mut cell,
            &mut gateway,
            &mut feed,
            ticks,
            None,
            &mut stats,
            t(second),
        )?;
        if let PassOutcome::Ran { report, .. } = &outcome {
            orders += report.orders.len();
        }
        decisions.push(decided(&outcome));
    }
    Ok(Run::of(&cell, decisions, orders))
}

/// R12-TICK-9006: replaying retained lake history through a fresh cell
/// yields the decisions the original session made: each pass's report, and
/// then every journal entry and the hash-chained journal's tail.
///
/// The journal is compared entry by entry as well as the reports, because
/// it holds decisions no report carries. Under the mutation below every
/// report matched while the venue choice's journaled candidates did not.
/// A test that compared reports alone would have passed a replay that
/// decided differently.
///
/// Mutation: in `ReplayFeed::publish` (src/feed.rs), add one to the price
/// of the first level of each replayed frame (perturb one replayed tick per
/// pass). The journal comparison fails at entry 132, a `VenueChosen` whose
/// candidates read ("XLON", "1") where the original read ("XLON", "2").
#[test]
fn replaying_retained_lake_history_through_a_fresh_cell_yields_the_same_decisions() -> Result<()> {
    let lake = Arc::new(MemoryBlobStore::new());
    let (original, sealed) = original_session(&lake)?;
    assert!(
        original.orders > 0,
        "premise: the original session placed orders, so there are decisions to reproduce"
    );
    assert!(sealed > 10, "premise: the passes sealed ticks ({sealed})");

    let (history, retained) = retained_history(lake.as_ref())?;
    assert_eq!(
        retained, sealed,
        "premise: the lake retains every tick the passes applied"
    );
    assert!(
        history.len() > 1,
        "premise: more than one pass has ticks to replay ({})",
        history.len()
    );

    let replay = replayed_session(&history)?;
    assert_eq!(replay.decisions.len(), original.decisions.len());
    for (pass, (replayed, recorded)) in replay.decisions.iter().zip(&original.decisions).enumerate()
    {
        assert_eq!(
            replayed, recorded,
            "pass {pass}: the replay decided differently from the session it replays"
        );
    }
    assert_eq!(replay.orders, original.orders);
    assert!(
        original
            .journal
            .iter()
            .any(|entry| entry.contains("OrderSent")),
        "premise: the original cell journaled orders it sent"
    );
    assert_eq!(replay.journal.len(), original.journal.len());
    for (replayed, recorded) in replay.journal.iter().zip(&original.journal) {
        assert_eq!(
            replayed, recorded,
            "the replayed cell journaled a decision the original did not"
        );
    }
    assert_eq!(
        replay.journal_digest, original.journal_digest,
        "the replayed cell's hash-chained journal is not the original's"
    );
    Ok(())
}

/// A recorded tick another venue published is refused by the replay before
/// any of it reaches the cell. The feed's decoder stamps every line it reads
/// with its own venue, so a tick of XPAR's written onto XLON's wire would
/// decode as XLON's and replay a session that never happened.
///
/// Mutation: drop the venue comparison in `ReplayFeed::publish`. The
/// publication succeeds, the cell decodes the line as XLON's and tracks a
/// book for it, and the first assertion fails.
#[test]
fn a_recorded_tick_another_venue_published_is_refused_before_it_reaches_the_cell() -> Result<()> {
    use qip_contracts::message::{BookSide, MessageBody};
    use qip_contracts::venue::Origin;

    let mut cell = cell()?;
    let mut feed = ReplayFeed::new(venue());
    feed.attach(&mut cell)?;
    let tracked_before = cell.liquidity().len();
    let elsewhere = MarketMessage::new(
        object(),
        Origin::new(VenueId::new("XPAR"), qip_edge_node::feed::FEED_NAME, 0, 1),
        MessageBody::LevelSet {
            side: BookSide::Bid,
            price: dec!("99"),
            quantity: dec!("5"),
            order_count: None,
        },
        t(10),
        t(10),
    );
    let refused = feed
        .publish(&mut cell, std::slice::from_ref(&elsewhere), t(10))
        .expect_err("a tick of another venue was replayed as this one's");
    assert_eq!(refused.code(), "invalid", "{}", refused.message());
    assert_eq!(
        cell.liquidity().len(),
        tracked_before,
        "the refused tick still reached the cell"
    );
    // Premise: the same tick under this feed's venue is replayable, so the
    // refusal was about the venue and nothing else.
    let mut here = elsewhere;
    here.origin.venue = venue();
    assert_eq!(feed.publish(&mut cell, &[here], t(10))?.messages, 1);
    Ok(())
}
