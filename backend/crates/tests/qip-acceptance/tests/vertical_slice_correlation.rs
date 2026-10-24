//! The vertical slice, correlation: one id joins a decision chain from the
//! signal that began it to the ledger event that booked it.
//!
//! ```text
//! SimulatedFeed -> Cell::work -> SimulatedGateway -> journal (OrderSent,
//! ChainOpened, Filled) -> Cell::flush -> SpoolWriter -> P2 journal records and
//! the P1 OutcomeRecord -> ledger::post -> LedgerEvent
//! ```
//!
//! Until this suite the writer stamped one id per session on every record, so a
//! fill at the ledger could be joined to the decision that produced it by order
//! id alone, and a second fill in the session was indistinguishable from the
//! first by correlation. The id is now `qip_contracts::reflex::chain_correlation`
//! over the cell, the session and the order id: derived, so a replay yields the
//! same id, and recomputable from the ledger event's own source key.
//!
//! The legs after the spool (the broker, `qip-ledgerd`) are covered by
//! `apps/qip-ledgerd/tests/fabric.rs`; `qip-acceptance` does not depend on it.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::reflex::{Decision as Recorded, JournalEntry, OutcomeRecord, chain_correlation};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, CorrelationId, Decimal, ManualClock, SystemClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::{Mirror, MirrorBatch};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::event_fabric::mirror::FabricMirror;
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{
    self, RecordKind, SpoolWriter, SpooledRecord, Step, WriterConfig, session_correlation,
};
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::{NodeAssembly, assemble};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::Metrics;
use qip_portfolio::ledger::{PaperFill, post};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const ENVELOPE_KEY: &[u8] = b"vertical-slice-envelope-key";

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

// --- the node, assembled the way the binary assembles it ---------------------

fn firing_strategy() -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("10")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn grant_with(gross: Decimal, order: Decimal, loss: Decimal) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            gross,
            order,
            loss,
            vec![venue()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("")?;
    let capital = unsigned.gross_limit();
    let unsigned = unsigned.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        2_500,
        qip_core::Decimal::ONE,
        capital,
    )?)?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), ENVELOPE_KEY, CELL, t(1))
}

fn share_policy(grants: Vec<String>) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: grants,
        },
        t(5),
    );
    VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, CELL, t(5))
}

/// A node holding one firing strategy under `grant`, with or without the
/// centre's region share, and a venue whose touch is seeded on both sides.
/// The venue may list `object()` on demand, the one instrument seeded and
/// traded here, and nothing else.
fn node(
    grant: VerifiedEnvelope,
    funded: bool,
) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = permitting(SimulatedGateway::new(venue(), 7, t(0))?, &[&object()])?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    let named = grant.signature().to_string();
    node.cell
        .deploy_with_pricing(compiled, program, grant, PricingPolicy::Marketable)?;
    if funded {
        node.cell.apply_policy(share_policy(vec![named])?, t(5))?;
    }
    Ok((node, gateway, feed))
}

fn funded_node() -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    node(
        grant_with(dec!("1000000"), dec!("100000"), dec!("50000"))?,
        true,
    )
}

fn one_pass(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    now: Timestamp,
) -> Result<WorkReport> {
    match run_pass(
        &mut node.cell,
        gateway,
        feed,
        None,
        &mut PassStats::default(),
        now,
    )? {
        PassOutcome::Ran { report, .. } => Ok(*report),
        other => panic!("the node did not run its pass: {other:?}"),
    }
}

// --- the spool: the regional fabric's durable seam ---------------------------

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn scratch(label: &str) -> PathBuf {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-vertical-slice-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The writer the binary runs, over a real `SegmentLog`, plus a mirror that
/// also remembers every batch it was handed so a test can replay one.
struct Fabric {
    writer: SpoolWriter<SegmentLog>,
    mirror: Recording,
    // Held so the writer's heartbeat has a gauge to beat.
    _gauge: PressureGauge,
}

#[derive(Debug)]
struct Recording {
    inner: FabricMirror,
    seen: Vec<MirrorBatch>,
}

impl Mirror for Recording {
    fn ship(&mut self, batch: MirrorBatch) -> Result<()> {
        self.seen.push(batch.clone());
        self.inner.ship(batch)
    }
}

fn fabric(label: &str) -> Fabric {
    let dir = scratch(label);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let spool = SegmentLog::open(&dir, SegmentLogConfig::new(Arc::clone(&clock))).expect("spool");
    let thresholds = Thresholds::new(
        1 << 40,
        Decimal::parse("0.5").expect("literal"),
        Decimal::parse("0.9").expect("literal"),
    )
    .expect("thresholds");
    let (gauge, publisher, _drain) =
        PressureGauge::new(thresholds, clock, Duration::from_secs(5)).expect("gauge");
    let telemetry = Arc::new(OutboxTelemetry::new(Arc::new(Metrics::new(
        "qip-edge-node",
    ))));
    let (sender, receiver) = writer::channel(16).expect("channel");
    let config = WriterConfig::new(CELL, t(0), std::time::Duration::from_millis(5), 64)
        .expect("writer config");
    let writer = SpoolWriter::open(spool, receiver, publisher, telemetry, config).expect("writer");
    Fabric {
        writer,
        mirror: Recording {
            inner: FabricMirror::new(CELL, sender).expect("mirror"),
            seen: Vec::new(),
        },
        _gauge: gauge,
    }
}

impl Fabric {
    fn drain(&mut self) {
        for _ in 0..64 {
            if self.writer.step().expect("the writer beats") == Step::Idle {
                return;
            }
        }
        panic!("the writer never went idle");
    }

    fn records(&self) -> Vec<SpooledRecord> {
        writer::records(self.writer.spool()).expect("the spool reads back")
    }
}

// --- the chain, read back off the spool ----------------------------------------

/// A funded node whose cell has been told the session the writer committed.
fn wired(label: &str) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed, Fabric, u64)> {
    let (mut node, gateway, feed) = funded_node()?;
    let fabric = fabric(label);
    let session = fabric.writer.session();
    node.cell.set_chain_epoch(session)?;
    Ok((node, gateway, feed, fabric, session))
}

fn ship_and_drain(node: &mut NodeAssembly, fabric: &mut Fabric, at: i64) -> Result<()> {
    let shipped = node.cell.flush(&mut fabric.mirror, t(at))?;
    assert!(shipped > 0, "premise: the journal had entries to ship");
    fabric.drain();
    Ok(())
}

/// Every P2 journal record decoded, with the correlation its lineage carries.
fn journal_records(fabric: &Fabric) -> Vec<(CorrelationId, JournalEntry)> {
    let topic = RecordKind::JournalRecorded.binding().topic;
    fabric
        .records()
        .into_iter()
        .filter(|record| record.event.topic == topic)
        .map(|record| {
            let entry = writer::journal_entry(&record.event).expect("a journal entry");
            (record.event.lineage.correlation_id.clone(), entry)
        })
        .collect()
}

fn outcome_records(fabric: &Fabric) -> Vec<(SpooledRecord, OutcomeRecord)> {
    let topic = RecordKind::OutcomeRecorded.binding().topic;
    fabric
        .records()
        .into_iter()
        .filter(|record| record.event.topic == topic)
        .map(|record| {
            let outcome = writer::outcome(&record.event).expect("an outcome the writer stamped");
            (record, outcome)
        })
        .collect()
}

fn filled_order(outcomes: &[(SpooledRecord, OutcomeRecord)]) -> (SpooledRecord, OutcomeRecord) {
    let mut filled = outcomes
        .iter()
        .filter(|(_, outcome)| matches!(outcome.entry.decision, Recorded::Filled { .. }))
        .cloned();
    let first = filled.next().expect("premise: a Filled outcome is on P1");
    assert!(filled.next().is_none(), "premise: exactly one Filled");
    first
}

#[test]
fn one_correlation_id_joins_every_hop_from_the_signal_to_the_ledger_event() -> Result<()> {
    let (mut node, mut gateway, mut feed, mut fabric, session) = wired("chain")?;
    let report = one_pass(&mut node, &mut gateway, &mut feed, t(10))?;
    assert_eq!(report.orders.len(), 1, "{:?}", report.refusals);
    assert_eq!(report.fills.len(), 1, "premise: the paper venue filled it");
    let order_id = report.orders[0].order_id.clone();
    let chain = chain_correlation(CELL, session, &order_id);
    ship_and_drain(&mut node, &mut fabric, 11)?;

    // HOP: every journal entry that names the order, on P2, under the chain.
    let journal = journal_records(&fabric);
    let hops: Vec<&(CorrelationId, JournalEntry)> = journal
        .iter()
        .filter(|(_, entry)| entry.decision.order_id() == Some(order_id.as_str()))
        .collect();
    let kinds: Vec<&str> = hops.iter().map(|(_, e)| e.decision.kind()).collect();
    assert_eq!(
        kinds,
        ["order_sent", "chain_opened", "filled"],
        "premise: the order's hops, in journal order"
    );
    for (correlation, entry) in &hops {
        assert_eq!(
            correlation,
            &chain,
            "a {} hop carries another chain's id",
            entry.decision.kind()
        );
    }

    // HOP: entries that name no order are not stamped with the chain.
    let unrelated: Vec<&(CorrelationId, JournalEntry)> = journal
        .iter()
        .filter(|(_, entry)| entry.decision.order_id().is_none())
        .collect();
    assert!(!unrelated.is_empty(), "premise: the pass journaled more");
    for (correlation, entry) in &unrelated {
        assert_eq!(
            correlation,
            &session_correlation(CELL, session),
            "a {} entry names no order and belongs to the session",
            entry.decision.kind()
        );
        assert_ne!(correlation, &chain);
    }

    // HOP: the P1 outcome, by the writer's own checked decoder.
    let (record, outcome) = filled_order(&outcome_records(&fabric));
    assert_eq!(record.event.lineage.correlation_id, chain);
    assert_eq!(outcome.correlation(), Some(chain.clone()));

    // HOP: the origin. ChainOpened names the correlation and the signal.
    let opened = hops
        .iter()
        .find_map(|(_, entry)| match &entry.decision {
            Recorded::ChainOpened {
                correlation,
                signals,
                pass,
                ..
            } => Some((correlation.clone(), signals.clone(), *pass)),
            _ => None,
        })
        .expect("a ChainOpened hop");
    assert_eq!(opened.0, chain.as_str());
    assert_eq!(opened.1.len(), 1, "one strategy raised the signal");
    assert_eq!(opened.1[0].0, STRATEGY);
    assert_eq!(opened.2, 1, "the order was sent on the cell's first pass");
    let signal = journal
        .iter()
        .map(|(_, entry)| entry)
        .find(|entry| entry.sequence == opened.1[0].1)
        .expect("the signal the chain names is in the journal");
    assert!(
        matches!(&signal.decision, Recorded::SignalRaised { strategy, .. } if strategy == STRATEGY),
        "the sequence ChainOpened names is not the strategy's SignalRaised: {:?}",
        signal.decision
    );

    // HOP: the ledger event's own source key names the same chain.
    let event = post(&PaperFill::try_from(&outcome)?)?;
    let source = event.source();
    assert_eq!(source.order_id, order_id);
    assert_eq!(
        chain_correlation(&source.cell, source.session, &source.order_id),
        chain,
        "ledger event -> correlation does not land on the chain"
    );
    Ok(())
}

#[test]
fn two_decision_chains_in_one_session_get_distinct_ids() -> Result<()> {
    let (mut node, mut gateway, mut feed, mut fabric, session) = wired("distinct")?;
    let first = one_pass(&mut node, &mut gateway, &mut feed, t(10))?;
    let second = one_pass(&mut node, &mut gateway, &mut feed, t(20))?;
    assert_eq!(first.orders.len(), 1, "{:?}", first.refusals);
    assert_eq!(second.orders.len(), 1, "{:?}", second.refusals);
    let (one, two) = (&first.orders[0].order_id, &second.orders[0].order_id);
    assert_ne!(one, two, "premise: two orders");
    ship_and_drain(&mut node, &mut fabric, 21)?;

    let chains = [
        chain_correlation(CELL, session, one),
        chain_correlation(CELL, session, two),
    ];
    assert_ne!(chains[0], chains[1]);
    let journal = journal_records(&fabric);
    for (order_id, chain) in [one, two].into_iter().zip(&chains) {
        let stamped = journal
            .iter()
            .filter(|(_, entry)| entry.decision.order_id() == Some(order_id.as_str()))
            .count();
        assert!(stamped >= 3, "premise: {order_id} has its hops: {stamped}");
        assert!(
            journal
                .iter()
                .filter(|(_, entry)| entry.decision.order_id() == Some(order_id.as_str()))
                .all(|(correlation, _)| correlation == chain),
            "{order_id} has a hop under another id"
        );
    }
    let outcomes = outcome_records(&fabric);
    let fills: Vec<CorrelationId> = outcomes
        .iter()
        .filter(|(_, outcome)| matches!(outcome.entry.decision, Recorded::Filled { .. }))
        .map(|(record, _)| record.event.lineage.correlation_id.clone())
        .collect();
    assert_eq!(fills.len(), 2, "premise: two fills reached P1");
    assert_ne!(fills[0], fills[1], "two fills share one correlation id");
    Ok(())
}

#[test]
fn a_replayed_batch_keeps_every_id_and_books_no_second_effect() -> Result<()> {
    let (mut node, mut gateway, mut feed, mut fabric, session) = wired("replay")?;
    let report = one_pass(&mut node, &mut gateway, &mut feed, t(10))?;
    let chain = chain_correlation(CELL, session, &report.orders[0].order_id);
    ship_and_drain(&mut node, &mut fabric, 11)?;
    let before: BTreeMap<String, String> = fabric
        .records()
        .iter()
        .map(|r| {
            (
                r.event.event_id.as_str().to_string(),
                r.event.lineage.correlation_id.as_str().to_string(),
            )
        })
        .collect();
    let chained_before = before.values().filter(|c| *c == chain.as_str()).count();
    assert!(
        chained_before >= 4,
        "premise: the chain has records: {chained_before}"
    );

    let batch = fabric
        .mirror
        .seen
        .first()
        .cloned()
        .expect("a batch shipped");
    fabric.mirror.ship(batch)?;
    fabric.drain();

    let after = fabric.records();
    for record in &after {
        let id = record.event.event_id.as_str();
        if let Some(was) = before.get(id) {
            assert_eq!(
                record.event.lineage.correlation_id.as_str(),
                was,
                "redelivery changed the correlation of {id}"
            );
        }
    }
    let chained_after = after
        .iter()
        .filter(|r| r.event.lineage.correlation_id == chain)
        .count();
    assert_eq!(
        chained_after, chained_before,
        "a replayed batch wrote a second record under the chain"
    );
    assert_eq!(
        outcome_records(&fabric)
            .iter()
            .filter(|(_, o)| matches!(o.entry.decision, Recorded::Filled { .. }))
            .count(),
        1,
        "a replayed batch booked a second fill"
    );
    Ok(())
}

/// The correlation of every record, in spool order, with the record's own id.
fn id_trace(fabric: &Fabric) -> Vec<(String, String)> {
    fabric
        .records()
        .iter()
        .map(|r| {
            (
                r.event.event_id.as_str().to_string(),
                r.event.lineage.correlation_id.as_str().to_string(),
            )
        })
        .collect()
}

#[test]
fn the_ids_are_the_same_across_two_runs_of_the_same_seed() -> Result<()> {
    let run = |label: &str| -> Result<(Vec<(String, String)>, String)> {
        let (mut node, mut gateway, mut feed, mut fabric, _) = wired(label)?;
        let report = one_pass(&mut node, &mut gateway, &mut feed, t(10))?;
        ship_and_drain(&mut node, &mut fabric, 11)?;
        Ok((id_trace(&fabric), report.orders[0].order_id.clone()))
    };
    let (first, first_order) = run("seed-a")?;
    let (second, second_order) = run("seed-b")?;
    assert_eq!(first_order, second_order, "premise: the same order");
    assert!(first.len() > 4, "premise: a non-trivial trace");
    assert_eq!(first, second, "two runs of one seed minted different ids");
    Ok(())
}

#[test]
fn the_session_is_part_of_the_id_so_a_restart_cannot_reuse_an_order_number() {
    // The cell numbers orders from one at every start: `london-1-1` exists in
    // every session. Without the session in the preimage a ledger joining on
    // the id would attach a fill to the wrong day's decision.
    let a = chain_correlation(CELL, 1, "london-1-1");
    assert_ne!(a, chain_correlation(CELL, 2, "london-1-1"));
    assert_ne!(a, chain_correlation("paris-1", 1, "london-1-1"));
    assert_ne!(a, chain_correlation(CELL, 1, "london-1-2"));
    assert_eq!(a, chain_correlation(CELL, 1, "london-1-1"));
}

#[test]
fn an_outcome_whose_correlation_was_edited_or_stripped_is_refused() -> Result<()> {
    let (mut node, mut gateway, mut feed, mut fabric, session) = wired("tamper")?;
    one_pass(&mut node, &mut gateway, &mut feed, t(10))?;
    ship_and_drain(&mut node, &mut fabric, 11)?;
    let (record, outcome) = filled_order(&outcome_records(&fabric));
    assert!(writer::outcome(&record.event).is_ok(), "premise: it reads");

    // Stripped to the session id, which is what every record carried before.
    let mut stripped = record.event.clone();
    stripped.lineage.correlation_id = session_correlation(CELL, session);
    let refusal = writer::outcome(&stripped).unwrap_err();
    assert!(
        refusal.message().contains("lineage"),
        "{}",
        refusal.message()
    );

    // Grafted onto another chain.
    let mut grafted = record.event.clone();
    grafted.lineage.correlation_id = chain_correlation(CELL, session, "london-1-99");
    assert!(writer::outcome(&grafted).is_err());

    // Grafted onto the same order of another session.
    let mut resession = record.event.clone();
    resession.lineage.correlation_id = chain_correlation(
        CELL,
        session + 1,
        outcome.entry.decision.order_id().expect("an order"),
    );
    assert!(writer::outcome(&resession).is_err());
    Ok(())
}

#[test]
fn a_cell_scoped_to_another_session_than_the_writer_is_refused_at_the_spool() -> Result<()> {
    let (mut node, mut gateway, mut feed) = funded_node()?;
    let mut fabric = fabric("mismatch");
    node.cell.set_chain_epoch(fabric.writer.session() + 1)?;
    let report = one_pass(&mut node, &mut gateway, &mut feed, t(10))?;
    assert_eq!(report.orders.len(), 1, "{:?}", report.refusals);
    node.cell.flush(&mut fabric.mirror, t(11))?;
    // A refused writer reports `Failed` on every step rather than going idle,
    // so it is stepped a bounded number of times and not drained.
    for _ in 0..4 {
        fabric.writer.step()?;
    }
    let failure = fabric
        .writer
        .failure()
        .expect("the writer refused the batch");
    assert!(
        failure.message().contains("set_chain_epoch"),
        "{}",
        failure.message()
    );
    assert!(
        outcome_records(&fabric).is_empty(),
        "an outcome under a chain the writer cannot derive reached P1"
    );
    Ok(())
}

#[test]
fn a_cell_with_no_session_opens_no_chain_link_and_the_epoch_is_set_once_and_early() -> Result<()> {
    // An unwired cell journals what it always did: no ChainOpened.
    let (mut bare, mut gateway, mut feed) = funded_node()?;
    let report = one_pass(&mut bare, &mut gateway, &mut feed, t(10))?;
    assert_eq!(report.orders.len(), 1, "premise: it traded");
    assert!(
        bare.cell
            .journal()
            .entries()
            .iter()
            .all(|entry| !matches!(entry.decision, Recorded::ChainOpened { .. })),
        "an unwired cell opened a chain"
    );
    assert!(bare.cell.chain_of("london-1-1").is_none());
    // Too late: an order has been numbered.
    assert!(bare.cell.set_chain_epoch(1).is_err());

    let (mut wired_node, _, _) = funded_node()?;
    assert!(
        wired_node.cell.set_chain_epoch(0).is_err(),
        "zero is no session"
    );
    wired_node.cell.set_chain_epoch(3)?;
    assert!(
        wired_node.cell.set_chain_epoch(4).is_err(),
        "twice splits ids"
    );
    assert_eq!(
        wired_node.cell.chain_of("london-1-1"),
        Some(chain_correlation(CELL, 3, "london-1-1"))
    );
    Ok(())
}

/// `gateway`, permitted to list each of `objects` on demand: a recorded
/// venue, product and jurisdiction permission for each (R12-COVERAGE-5107).
/// A fixture; the refusal without one is proven in qip-edge-node's
/// tests/gateway.rs.
fn permitting(gateway: SimulatedGateway, objects: &[&ObjectId]) -> Result<SimulatedGateway> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for object in objects {
        let request = CreationRequest::new(
            CreationAction::List,
            venue.as_str(),
            object.as_str(),
            "paper",
        );
        for authority in CreationAuthority::ALL {
            permissions.record(authority, &request, "test fixture")?;
        }
    }
    gateway.with_listing_permissions(permissions, "paper")
}
