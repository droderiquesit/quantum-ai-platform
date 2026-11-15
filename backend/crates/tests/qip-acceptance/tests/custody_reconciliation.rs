//! Venue custody against the ledger of record, and the halt a break issues
//! (ADR 0130 decision 4, ADR 0442, R12-LEDGER-8139).
//!
//! ```text
//! SimulatedFeed -> Cell::work -> SimulatedGateway (paper venue) -> fill
//!   -> Cell journal -> FabricMirror -> SpoolWriter -> P1 records
//!   -> qip-ledgerd LedgerStore::apply (the real store)
//! SimulatedGateway::positions -> CustodyStatement at the chain position
//!   -> LedgerStore::reconcile_custody -> custody_break note + counter
//!   -> HaltCommand (unsigned, deterministic) -> signed -> P0 halt frame
//!   -> PolicyDownlink -> Cell::apply_halt -> the next pass is Halted
//! ```
//!
//! Every hop is the production type except one, and the test names it: the
//! carrier from `qip-ledgerd` to the centre's halt signer. Here the test
//! signs the store's unsigned command with the cell's key and frames it on
//! the halt topic; in a deployment that is `qip-api`'s `broadcast_halt`.
//!
//! Since ADR 0494 the node's pass journals the venue's statement on the
//! cell's chain, and `LedgerStore::apply` compares it as it applies it:
//!
//! ```text
//! run_pass -> SimulatedGateway::custody_holdings
//!   -> Cell::record_custody_statement -> Decision::CustodyStatement on P1
//!   -> LedgerStore::apply -> Disposition::CustodyCompared
//! ```
//!
//! What is still missing is the carrier from a `custody_break` to the
//! centre's halt signer.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::ledger::custody::{CustodyDirection, CustodyStatement};
use qip_contracts::policy::{GrantManifest, HaltCommand, PolicyPayload, Slot};
use qip_contracts::reflex::Decision as Recorded;
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, CorrelationId, Decimal, Id, Lineage, ManualClock, SystemClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::mesh::{HaltTopic, PolicyDownlink, ScriptedFrames};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::event_fabric::mirror::FabricMirror;
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{self, RecordKind, SpoolWriter, Step, WriterConfig};
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::{NodeAssembly, assemble};
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_ledgerd::store::{CustodyReconciliation, Delivery, LedgerStore, P1Record};
use qip_ledgerd::telemetry::LedgerTelemetry;
use qip_observability::metrics::{MetricValue, Metrics, names};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const KEY: &[u8] = b"custody-reconciliation-envelope-key";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

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

fn grant() -> Result<VerifiedEnvelope> {
    let unsigned = CapitalEnvelope::new(
        StrategyId::new(STRATEGY),
        CELL,
        dec!("1000000"),
        dec!("100000"),
        dec!("50000"),
        vec![venue()],
        t(0),
        t(3600),
        "alice@example.com",
        "",
    )?;
    let capital = unsigned.gross_limit();
    let unsigned = unsigned.with_exposure(qip_contracts::capital::ExposureBounds::bounded(
        2_500,
        Decimal::ONE,
        capital,
    )?)?;
    let loss_limit = unsigned.loss_limit();
    let unsigned = unsigned.with_drawdown(qip_contracts::DrawdownBounds::bounded(
        loss_limit, None, None,
    )?)?;
    let signature = sign_payload(KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(unsigned.signed(signature), KEY, CELL, t(1))
}

fn share_policy(grants: Vec<String>) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: grants,
        },
        t(5),
    );
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, t(5))
}

fn permitting_to_seed(gateway: SimulatedGateway) -> Result<SimulatedGateway> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for action in [
        CreationAction::Create,
        CreationAction::List,
        CreationAction::Seed,
    ] {
        let request = CreationRequest::new(action, venue.as_str(), object().as_str(), "paper");
        for authority in CreationAuthority::ALL {
            permissions.record(authority, &request, "test fixture")?;
        }
    }
    gateway.with_listing_permissions(permissions, "paper")
}

fn funded_node() -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = permitting_to_seed(SimulatedGateway::new(venue(), 7, t(0))?)?;
    gateway.seed_touch(&object(), Side::Buy, dec!("99"), dec!("500"), t(1))?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), t(1))?;
    let feed = SimulatedFeed::new(venue());
    feed.attach(&mut node.cell)?;
    let (compiled, program) = firing_strategy()?;
    let grant = grant()?;
    let named = grant.signature().to_string();
    node.cell
        .deploy_with_pricing(compiled, program, grant, PricingPolicy::Marketable)?;
    node.cell.apply_policy(share_policy(vec![named])?, t(5))?;
    Ok((node, gateway, feed))
}

fn pass(
    node: &mut NodeAssembly,
    gateway: &mut SimulatedGateway,
    feed: &mut SimulatedFeed,
    now: Timestamp,
) -> Result<PassOutcome> {
    run_pass(
        &mut node.cell,
        gateway,
        feed,
        None,
        &mut PassStats::default(),
        now,
    )
}

// --- the spool and the ledger ------------------------------------------------

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn scratch(label: &str) -> PathBuf {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-custody-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The node's P1 records after one pass that filled, as the spool holds them.
fn p1_after_a_fill(node: &mut NodeAssembly, label: &str) -> Result<Vec<P1Record>> {
    let dir = scratch(label);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let spool = SegmentLog::open(&dir, SegmentLogConfig::new(Arc::clone(&clock)))?;
    let thresholds = Thresholds::new(1 << 40, dec!("0.5"), dec!("0.9"))?;
    let (_gauge, publisher, _drain) =
        PressureGauge::new(thresholds, clock, Duration::from_secs(5))?;
    let telemetry = Arc::new(OutboxTelemetry::new(Arc::new(Metrics::new(
        "qip-edge-node",
    ))));
    let (sender, receiver) = writer::channel(16)?;
    let config = WriterConfig::new(CELL, t(0), std::time::Duration::from_millis(5), 64)?;
    let mut spool = SpoolWriter::open(spool, receiver, publisher, telemetry, config)?;
    let mut mirror = FabricMirror::new(CELL, sender)?;
    node.cell.flush(&mut mirror, t(11))?;
    for _ in 0..64 {
        if spool.step()? == Step::Idle {
            break;
        }
    }
    let outcome = RecordKind::OutcomeRecorded.binding().topic;
    let span = RecordKind::ChainSpan.binding().topic;
    Ok(writer::records(spool.spool())?
        .into_iter()
        .filter(|record| record.event.topic == outcome || record.event.topic == span)
        .map(|record| {
            if record.event.topic == outcome {
                P1Record::Outcome(Box::new(
                    serde_json::from_value(record.event.payload.clone()).expect("an outcome"),
                ))
            } else {
                P1Record::Span(writer::chain_span(&record.event).expect("a span"))
            }
        })
        .collect())
}

/// The chain up to, and not including, the custody statement the node
/// journaled after the fill (ADR 0494).
///
/// The tests that drive `reconcile_custody` out of band compare their own
/// statement at the shipped position. Since the pass journals the venue's
/// statement on the chain, the ledger compares that one when it applies it
/// and marks the position, so an out-of-band statement at or behind it is
/// `Stale`. The prefix is the same chain the node shipped up to the instant
/// before the venue's statement — a consistent chain, not a doctored one —
/// and the in-stream comparison has its own test below.
fn before_the_journaled_statement(records: &[P1Record]) -> Vec<P1Record> {
    let at = records
        .iter()
        .position(|r| {
            matches!(r, P1Record::Outcome(o) if matches!(o.entry.decision, Recorded::CustodyStatement { .. }))
        })
        .expect("premise: the pass journaled the venue's custody statement");
    assert_eq!(
        at + 1,
        records.len(),
        "premise: the statement is the last record the pass shipped"
    );
    records[..at].to_vec()
}

fn ledger(records: &[P1Record], label: &str) -> Result<(LedgerStore, Arc<Metrics>)> {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let metrics = Arc::new(Metrics::new("qip-ledgerd"));
    let store = LedgerStore::open(
        scratch(label),
        qip_storage::EngineConfig::new(clock),
        LedgerTelemetry::new(Arc::clone(&metrics)),
    )?;
    for (offset, record) in records.iter().enumerate() {
        store.apply(&Delivery {
            partition: "p0".to_string(),
            offset: offset as u64,
            record_hash: qip_core::sha256_hex(&serde_json::to_vec(record).expect("serialises")),
            record: record.clone(),
        })?;
    }
    Ok((store, metrics))
}

/// The last chain position the node shipped: what the venue's statement is
/// stamped with when it is taken after that shipment.
fn shipped_position(records: &[P1Record]) -> (u64, u64) {
    records
        .iter()
        .map(|record| match record {
            P1Record::Outcome(o) => (o.session, o.journal_sequence),
            P1Record::Span(s) => (s.session, s.last_seq),
        })
        .max()
        .expect("premise: the node shipped something")
}

/// The venue's own custody statement for the cell: its clearing ledger's
/// positions, at the shipped chain position. `skew` is added to the venue's
/// figure for the traded instrument — the injected break.
fn statement(
    gateway: &SimulatedGateway,
    (session, sequence): (u64, u64),
    skew: Decimal,
) -> Result<CustodyStatement> {
    let mut statement =
        CustodyStatement::new(CELL, VENUE, session, sequence, t(12), true)?.cash("USD")?;
    for position in gateway.positions() {
        let quantity = if position.object_id == object() {
            position.quantity + skew
        } else {
            position.quantity
        };
        statement = statement.holding(position.object_id.as_str(), quantity)?;
    }
    Ok(statement)
}

fn custody_breaks(metrics: &Metrics, direction: &str) -> u64 {
    metrics
        .snapshot()
        .series
        .iter()
        .filter(|s| {
            s.name == names::LEDGER_CUSTODY_BREAKS
                && s.labels.get("direction").map(String::as_str) == Some(direction)
        })
        .map(|s| match &s.value {
            MetricValue::Counter(n) => *n,
            other => panic!("not a counter: {other:?}"),
        })
        .sum()
}

// --- the P0 halt frame -------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct HaltBody(HaltCommand);

impl EventBody for HaltBody {
    const TOPIC: Topic = HaltTopic::TOPIC;
    const SCHEMA_VERSION: u32 = HaltTopic::SCHEMA_VERSION;
}

fn halt_frame(command: HaltCommand) -> Result<AnyEvent> {
    Envelope::new(
        Id::from_string("EVT-CUSTODY-HALT".to_string()),
        command.issued_at,
        command.issued_at,
        Lineage::root(
            CorrelationId::from_string("COR-CUSTODY-HALT".to_string()),
            "qip-acceptance-custody",
        ),
        HaltBody(command),
    )
    .erase()
}

// --- the property -------------------------------------------------------------

/// The row's verification: an injected break emits a P0 halt and the cell
/// stops; an agreeing statement emits nothing and the cell keeps trading.
///
/// Two worlds from one fixture, so the agreeing half is the premise for the
/// broken half: the same pass, the same fill, the same store, and the only
/// difference is one unit on the venue's statement.
///
/// Mutation (ADR 0442 decision 3, the trip): make `halt_for` return `None`
/// on a break — `reconcile_custody` answers `Agrees`, the `Broken` match
/// fails. Mutation (the ledger side): drop the trading-account filter — the
/// venue's own contra legs cancel the trading legs, the ledger reads zero,
/// and the agreeing statement reads as a break.
#[test]
fn a_custody_break_halts_the_cell_through_a_signed_p0_halt_and_an_agreeing_statement_does_not()
-> Result<()> {
    // --- the agreeing world ---
    let (mut node, mut gateway, mut feed) = funded_node()?;
    let PassOutcome::Ran { report, .. } = pass(&mut node, &mut gateway, &mut feed, t(10))? else {
        panic!("premise: the node ran its pass");
    };
    assert_eq!(report.fills.len(), 1, "premise: the cell filled");
    let records = before_the_journaled_statement(&p1_after_a_fill(&mut node, "agree")?);
    assert!(
        records.iter().any(|r| matches!(r, P1Record::Outcome(o) if matches!(o.entry.decision, Recorded::Filled { .. }))),
        "premise: the fill is on P1"
    );
    let at = shipped_position(&records);
    let (store, _metrics) = ledger(&records, "agree-ledger")?;

    // A statement past what the ledger has applied is not compared.
    let ahead = statement(&gateway, (at.0, at.1 + 1), Decimal::ZERO)?;
    assert_eq!(
        store.reconcile_custody(&ahead)?,
        CustodyReconciliation::Pending
    );

    let clean = statement(&gateway, at, Decimal::ZERO)?;
    assert!(
        clean
            .holdings
            .get(object().as_str())
            .is_some_and(|q| !q.is_zero()),
        "premise: the venue holds the traded units: {clean:?}"
    );
    assert_eq!(
        store.reconcile_custody(&clean)?,
        CustodyReconciliation::Agrees
    );
    assert_eq!(
        store.reconcile_custody(&clean)?,
        CustodyReconciliation::Stale
    );
    assert!(
        store.journal()?.iter().all(|n| n.event != "custody_break"),
        "an agreeing statement journaled a break"
    );
    assert!(
        matches!(
            pass(&mut node, &mut gateway, &mut feed, t(20))?,
            PassOutcome::Ran { .. }
        ),
        "premise: with no break the cell keeps trading"
    );

    // --- the broken world: the venue states one unit fewer ---
    let (mut node, mut gateway, mut feed) = funded_node()?;
    let PassOutcome::Ran { .. } = pass(&mut node, &mut gateway, &mut feed, t(10))? else {
        panic!("premise: the node ran its pass");
    };
    let records = before_the_journaled_statement(&p1_after_a_fill(&mut node, "break")?);
    let at = shipped_position(&records);
    let (store, metrics) = ledger(&records, "break-ledger")?;
    let submitted = gateway.submitted_count();

    let broken = statement(&gateway, at, -Decimal::ONE)?;
    let CustodyReconciliation::Broken { breaks, halt } = store.reconcile_custody(&broken)? else {
        panic!("a one-unit custody difference was not a break");
    };
    assert_eq!(breaks.len(), 1, "{breaks:?}");
    assert_eq!(breaks[0].unit, object().as_str());
    assert_eq!(breaks[0].direction, CustodyDirection::LedgerOverVenue);
    assert_eq!(custody_breaks(&metrics, "ledger_over_venue"), 1);
    assert_eq!(
        store
            .journal()?
            .iter()
            .filter(|n| n.event == "custody_break" && n.cell == CELL)
            .count(),
        1,
        "the break was not journaled once"
    );
    assert_eq!(halt.cell, CELL);
    assert_eq!(
        halt.issued_at,
        t(12),
        "issued at the statement, not the clock"
    );
    // The same statement again journals nothing twice.
    assert_eq!(
        store.reconcile_custody(&broken)?,
        CustodyReconciliation::Stale
    );
    assert_eq!(custody_breaks(&metrics, "ledger_over_venue"), 1);

    // The P0 fabric event: signed, framed on the halt topic, received by the
    // cell's own policy downlink and applied.
    let mut frames = ScriptedFrames::new(4)?;
    frames.push(halt_frame(halt.signed(KEY)?)?)?;
    let mut downlink = PolicyDownlink::from_source(CELL, KEY, Box::new(frames))?;
    let batch = downlink.poll(t(13))?;
    assert_eq!(
        batch.halts.len(),
        1,
        "the cell did not verify the halt: {batch:?}"
    );
    for verified in batch.halts {
        node.cell.apply_halt(verified, t(13));
    }
    assert!(
        node.cell.is_halted(),
        "the custody break did not halt the cell"
    );
    assert!(
        matches!(
            pass(&mut node, &mut gateway, &mut feed, t(20))?,
            PassOutcome::Halted { .. }
        ),
        "a halted cell ran its pass"
    );
    assert_eq!(
        gateway.submitted_count(),
        submitted,
        "a halted cell sent an order"
    );
    Ok(())
}

/// The paper fence (ADR 0130 decision 6): a statement not marked simulated
/// is refused and compares nothing.
///
/// Mutation: delete the `simulated` refusal — the call returns `Ok` and the
/// assertion fails.
#[test]
fn a_custody_statement_not_marked_simulated_is_refused() -> Result<()> {
    let (mut node, mut gateway, mut feed) = funded_node()?;
    let PassOutcome::Ran { .. } = pass(&mut node, &mut gateway, &mut feed, t(10))? else {
        panic!("premise: the node ran its pass");
    };
    let records = before_the_journaled_statement(&p1_after_a_fill(&mut node, "live")?);
    let at = shipped_position(&records);
    let (store, _metrics) = ledger(&records, "live-ledger")?;
    let mut live = statement(&gateway, at, Decimal::ZERO)?;
    assert_eq!(
        store.reconcile_custody(&live)?,
        CustodyReconciliation::Agrees,
        "premise"
    );
    live.simulated = false;
    live.sequence += 1;
    let refusal = store
        .reconcile_custody(&live)
        .expect_err("a live statement was compared");
    assert!(
        refusal.to_string().contains("not marked simulated"),
        "{refusal}"
    );
    Ok(())
}

/// The production path end to end (ADR 0494, R12-LEDGER-10185): a node pass
/// that fills journals the simulated venue's custody statement after the
/// fill, the statement ships on P1, and the ledger compares it with the fill
/// it booked as it applies it — and they agree, because both are the same
/// venue's account of the same pass. A second pass in which nothing filled
/// journals no second statement.
///
/// Mutations, each run and restored: drop the `record_custody_statement`
/// call from the pass — no statement reaches P1 ("journaled no custody
/// statement"); leave both cash-unit inserts out of `custody_holdings` — the
/// ledger reads the fill's USD leg as a holding the venue denies and
/// `compared` is `[1]`; remove the unchanged-statement check in
/// `Cell::record_custody_statement` — "an unchanged statement was journaled
/// again"; remove its configured-venue check — the foreign venue is
/// accepted; journal `simulated: false` — the paper assertion fails.
#[test]
fn a_node_pass_that_fills_journals_the_venues_custody_statement_on_p1_and_the_ledger_finds_it_agrees()
-> Result<()> {
    use qip_ledgerd::store::Disposition;
    let (mut node, mut gateway, mut feed) = funded_node()?;
    let PassOutcome::Ran { report, .. } = pass(&mut node, &mut gateway, &mut feed, t(10))? else {
        panic!("premise: the node ran its pass");
    };
    assert_eq!(report.fills.len(), 1, "premise: the cell filled");
    let records = p1_after_a_fill(&mut node, "in-stream")?;
    let fill_at = records
        .iter()
        .position(|r| matches!(r, P1Record::Outcome(o) if matches!(o.entry.decision, Recorded::Filled { .. })))
        .expect("premise: the fill is on P1");
    let statement_at = records
        .iter()
        .position(|r| matches!(r, P1Record::Outcome(o) if matches!(o.entry.decision, Recorded::CustodyStatement { .. })))
        .expect("the pass journaled no custody statement on P1");
    assert!(
        statement_at > fill_at,
        "the statement must follow the fill it covers"
    );
    let P1Record::Outcome(outcome) = &records[statement_at] else {
        unreachable!()
    };
    let Recorded::CustodyStatement {
        holdings,
        simulated,
        ..
    } = &outcome.entry.decision
    else {
        unreachable!()
    };
    assert!(
        *simulated,
        "a cell's custody statement is paper by construction"
    );
    assert!(
        holdings
            .iter()
            .any(|(unit, quantity)| unit == object().as_str() && quantity != "0"),
        "premise: the venue states the traded units: {holdings:?}"
    );

    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let metrics = Arc::new(Metrics::new("qip-ledgerd"));
    let store = LedgerStore::open(
        scratch("in-stream-ledger"),
        qip_storage::EngineConfig::new(clock),
        LedgerTelemetry::new(Arc::clone(&metrics)),
    )?;
    let mut compared = Vec::new();
    for (offset, record) in records.iter().enumerate() {
        let disposition = store.apply(&Delivery {
            partition: "p0".to_string(),
            offset: offset as u64,
            record_hash: qip_core::sha256_hex(&serde_json::to_vec(record).expect("serialises")),
            record: record.clone(),
        })?;
        if let Disposition::CustodyCompared { breaks } = disposition {
            compared.push(breaks);
        }
    }
    assert_eq!(compared, vec![0], "one statement, compared once, agreeing");
    assert_eq!(custody_breaks(&metrics, "ledger_over_venue"), 0);
    assert_eq!(custody_breaks(&metrics, "venue_over_ledger"), 0);
    assert!(store.journal()?.iter().all(|n| n.event != "custody_break"));

    // Nothing moved at the venue since the pass: the same statement is not
    // journaled again. A statement that differs is, at the next sequence.
    let (holdings, cash) = gateway.custody_holdings();
    assert_eq!(
        node.cell
            .record_custody_statement(&venue(), holdings.clone(), cash.clone(), t(11))?,
        None,
        "an unchanged statement was journaled again"
    );
    let mut moved = holdings;
    moved.insert(object().as_str().to_string(), Decimal::ZERO);
    assert!(
        node.cell
            .record_custody_statement(&venue(), moved, cash.clone(), t(11))?
            .is_some(),
        "a changed statement was not journaled"
    );
    // A venue the cell does not trade at holds nothing for it to state.
    assert!(
        node.cell
            .record_custody_statement(&VenueId::new("sim-elsewhere"), BTreeMap::new(), cash, t(11))
            .is_err()
    );
    Ok(())
}

/// A venue that has stated nothing and holds nothing has not moved, and one
/// that held something and now holds nothing has.
///
/// This has already failed once: the first statement was journaled whatever
/// it named, so a pass the capital gate refused outright still put a P1
/// outcome on the stream, and `vertical_slice_cell_ledger` went red on two
/// tests in CI. The second half is the guard on the fix: reading *every*
/// empty statement as "nothing moved" would hide a venue that stopped holding
/// what the ledger still books.
///
/// Mutations, each run and restored: journal the first statement whatever it
/// names (the `None` arm answering `false`) — "a venue holding nothing was
/// journaled"; treat any statement with no holdings as unmoved — "a venue
/// that stopped holding was not journaled".
#[test]
fn a_venue_that_never_held_anything_journals_no_statement_and_one_that_stopped_holding_does()
-> Result<()> {
    let (mut node, gateway, _feed) = funded_node()?;
    let (holdings, cash) = gateway.custody_holdings();
    assert!(
        holdings.is_empty(),
        "premise: no pass has run, so the venue holds nothing: {holdings:?}"
    );
    assert!(!cash.is_empty(), "premise: the venue names a cash unit");
    assert_eq!(
        node.cell
            .record_custody_statement(&venue(), BTreeMap::new(), cash.clone(), t(1))?,
        None,
        "a venue holding nothing was journaled"
    );
    // Asked again, the answer is the same: the unjournaled statement was not
    // remembered as if it had been journaled.
    assert_eq!(
        node.cell
            .record_custody_statement(&venue(), BTreeMap::new(), cash.clone(), t(2))?,
        None,
        "a venue holding nothing was journaled on the second call"
    );
    let mut held = BTreeMap::new();
    held.insert(object().as_str().to_string(), dec!("5"));
    let first = node
        .cell
        .record_custody_statement(&venue(), held, cash.clone(), t(3))?
        .expect("the first statement naming a holding was not journaled");
    let emptied = node
        .cell
        .record_custody_statement(&venue(), BTreeMap::new(), cash.clone(), t(4))?
        .expect("a venue that stopped holding was not journaled");
    assert!(emptied > first, "{emptied} does not follow {first}");
    assert_eq!(
        node.cell
            .record_custody_statement(&venue(), BTreeMap::new(), cash, t(5))?,
        None,
        "an unchanged empty statement was journaled again"
    );
    Ok(())
}
