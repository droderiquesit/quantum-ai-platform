//! The vertical slice, cell half: a simulated venue fill to a double-entry
//! ledger event.
//!
//! ```text
//! SimulatedFeed -> Cell::work (signal, netting, grant + region share, order gates)
//!   -> SimulatedGateway (paper venue) -> confirmed fill -> Cell journal (hash chain)
//!   -> Cell::flush -> FabricMirror -> SpoolWriter -> SegmentLog (the regional
//!      fabric's local durable spool) -> P1 OutcomeRecord
//!   -> PaperFill::try_from (paper fence) -> ledger::post -> LedgerEvent
//! ```
//!
//! Every hop is the production type. The spool is the fabric seam in process:
//! the same `SpoolWriter` the `qip-edge-node` binary runs, over the same
//! `SegmentLog`. What is NOT reached here is the broker that delivers the spool
//! to `qip-ledgerd` and the ledger store itself, because `qip-acceptance`
//! does not depend on `qip-ledgerd` (a manifest and lockfile change this file
//! does not own). `apps/qip-ledgerd/tests/fabric.rs` proves that last leg with
//! P1 records shaped as the writer shapes them, so the chain is covered in two
//! overlapping pieces and the overlap is the `OutcomeRecord`.
//!
//! Correlation: the cell's journal carries `order_id` and the chain digest, not
//! a correlation id. The writer stamps ONE correlation id per writer session on
//! every record it spools. The join key across this half is therefore the
//! order id plus the journal digest, and the test asserts exactly that, rather
//! than a trace id the production types do not carry.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::ledger::{Account, Direction, LedgerEvent, Settlement};
use qip_contracts::message::BookSide;
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::reflex::{Decision as Recorded, OutcomeRecord};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Decimal, ManualClock, SystemClock, dec};
use qip_edge::cell::{CellConfig, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::{Journal, Mirror, MirrorBatch, ship};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::event_fabric::mirror::FabricMirror;
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{
    self, RecordKind, SpoolWriter, SpooledRecord, Step, WriterConfig,
};
use qip_edge_node::feed::SimulatedFeed;
use qip_edge_node::gateway::SimulatedGateway;
use qip_edge_node::pass::{PassOutcome, PassStats, run_pass};
use qip_edge_node::{NodeAssembly, assemble};
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
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const STRATEGY: &str = "always-enter";
const ENVELOPE_KEY: &[u8] = b"vertical-slice-envelope-key";
const RESERVATION_GATE: &str = "region_reservation";

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
fn node(
    grant: VerifiedEnvelope,
    funded: bool,
) -> Result<(NodeAssembly, SimulatedGateway, SimulatedFeed)> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let allocation = RegionCapital::read(Some("1000000000"))?;
    let mut node = assemble(config, features, Arc::new(SystemClock), allocation, None)?;
    let mut gateway = SimulatedGateway::new(venue(), 7, t(0))?;
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

    /// Every P1 outcome in the spool, decoded.
    fn outcomes(&self) -> Vec<(SpooledRecord, OutcomeRecord)> {
        let topic = RecordKind::OutcomeRecorded.binding().topic;
        self.records()
            .into_iter()
            .filter(|record| record.event.topic == topic)
            .map(|record| {
                let outcome: OutcomeRecord =
                    serde_json::from_value(record.event.payload.clone()).expect("an outcome");
                (record, outcome)
            })
            .collect()
    }
}

fn filled_outcomes(fabric: &Fabric) -> Vec<OutcomeRecord> {
    fabric
        .outcomes()
        .into_iter()
        .map(|(_, outcome)| outcome)
        .filter(|outcome| matches!(outcome.entry.decision, Recorded::Filled { .. }))
        .collect()
}

/// Per unit, debits minus credits across every posting: zero for a balanced
/// event.
fn imbalance(event: &LedgerEvent) -> BTreeMap<String, Decimal> {
    let mut by_unit: BTreeMap<String, Decimal> = BTreeMap::new();
    for posting in event.postings() {
        let entry = by_unit.entry(posting.unit.clone()).or_insert(Decimal::ZERO);
        match posting.direction {
            Direction::Debit => *entry += posting.amount,
            Direction::Credit => *entry -= posting.amount,
        }
    }
    by_unit
}

// --- the happy path ----------------------------------------------------------

#[test]
fn a_simulated_fill_reaches_the_fabric_spool_and_is_booked_as_one_balanced_ledger_event()
-> Result<()> {
    let (mut node, mut gateway, mut feed) = funded_node()?;
    let mut fabric = fabric("happy");

    // HOP: signal, capital gates, order, paper venue, confirmed fill.
    assert_eq!(gateway.class(), "simulated", "the venue is the simulator");
    let report = one_pass(&mut node, &mut gateway, &mut feed, t(10))?;
    assert_eq!(report.signals.len(), 1, "the strategy fired");
    assert_eq!(
        report.orders.len(),
        1,
        "one order was sent: {:?}",
        report.refusals
    );
    assert_eq!(
        report.fills.len(),
        1,
        "the paper venue filled it on the pass"
    );
    let order = &report.orders[0];
    assert_eq!(gateway.submitted_count(), 1);
    assert_eq!(order.venue, venue());
    assert_eq!(report.fills[0].quantity, order.quantity);

    // HOP: cell position equals the venue's own record.
    let position = node.cell.position(&venue(), &object());
    assert_eq!(position, order.quantity);
    let venue_side: Decimal = gateway
        .positions()
        .iter()
        .map(|snapshot| snapshot.quantity)
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(position, venue_side, "the cell and the venue disagree");

    // HOP: journal -> fabric mirror -> spool. A second flush ships nothing.
    let shipped = node.cell.flush(&mut fabric.mirror, t(11))?;
    assert!(shipped > 0, "premise: the journal had entries to ship");
    fabric.drain();
    assert_eq!(
        node.cell.flush(&mut fabric.mirror, t(12))?,
        0,
        "nothing is shipped twice"
    );
    fabric.drain();

    // HOP: the outcome is on P1 with the journal's own digest.
    let outcomes = filled_outcomes(&fabric);
    assert_eq!(outcomes.len(), 1, "exactly one Filled outcome on P1");
    let outcome = &outcomes[0];
    let Recorded::Filled {
        order_id,
        simulated,
        side,
        quote_unit,
        ..
    } = &outcome.entry.decision
    else {
        panic!("filtered to Filled");
    };
    assert_eq!(order_id, &order.order_id, "the order id is the join key");
    assert!(*simulated);
    assert_eq!(
        *side,
        Some(BookSide::Ask),
        "a buy is booked on the venue's ask"
    );
    assert!(
        quote_unit.is_some(),
        "the cell stated the quote unit the ledger needs"
    );
    let journaled = node
        .cell
        .journal()
        .entries()
        .iter()
        .find(|entry| entry.sequence == outcome.journal_sequence)
        .expect("the journal entry the outcome points at");
    assert_eq!(journaled.digest, outcome.journal_digest);
    assert_eq!(journaled, &outcome.entry);

    // One correlation id per writer session, shared by every record. It is a
    // session id, not a trace of this fill (finding C2 in the report).
    let correlations: BTreeSet<String> = fabric
        .records()
        .iter()
        .map(|record| record.event.lineage.correlation_id.as_str().to_string())
        .collect();
    assert_eq!(correlations.len(), 1, "{correlations:?}");

    // HOP: the paper fence, then the double-entry posting.
    let fill = PaperFill::try_from(outcome)?;
    let event = post(&fill)?;
    assert_eq!(event.settlement(), Settlement::Simulated);
    assert_eq!(event.source().order_id, order.order_id);
    assert_eq!(event.source().cell, CELL);
    assert_eq!(event.source().journal_digest, outcome.journal_digest);
    assert!(
        imbalance(&event).values().all(|net| net.is_zero()),
        "the ledger event does not balance per unit: {:?}",
        imbalance(&event)
    );
    let object_unit = object().as_str().to_string();
    let trading = Account::Trading {
        cell: CELL.to_string(),
        strategy: STRATEGY.to_string(),
    };
    let held: Decimal = event
        .postings()
        .iter()
        .filter(|posting| posting.account == trading && posting.unit == object_unit)
        .map(|posting| match posting.direction {
            Direction::Debit => posting.amount,
            Direction::Credit => -posting.amount,
        })
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(
        held, position,
        "the ledger holds what the cell and the venue hold"
    );
    let cash: Decimal = event
        .postings()
        .iter()
        .filter(|posting| posting.account == trading && posting.unit != object_unit)
        .map(|posting| match posting.direction {
            Direction::Debit => -posting.amount,
            Direction::Credit => posting.amount,
        })
        .fold(Decimal::ZERO, |a, b| a + b);
    assert_eq!(
        cash,
        report.fills[0].quantity * report.fills[0].price,
        "the cash leg is quantity times the venue's fill price"
    );
    Ok(())
}

// --- negatives ---------------------------------------------------------------

/// What a refused pass must leave: no order at the venue, no position, and a
/// journal that says the pass happened. (That nothing outcome-shaped reached P1
/// is asserted by each caller, from the spool.)
fn assert_nothing_reached_the_venue(node: &NodeAssembly, gateway: &SimulatedGateway) {
    assert_eq!(gateway.submitted_count(), 0, "the venue saw an order");
    assert_eq!(node.cell.position(&venue(), &object()), Decimal::ZERO);
    assert!(
        !node.cell.journal().entries().is_empty(),
        "premise: the refused pass was journaled"
    );
}

#[test]
fn an_unfunded_cell_is_refused_at_capital_authorisation_and_books_nothing() -> Result<()> {
    let (mut node, mut gateway, mut feed) = node(
        grant_with(dec!("1000000"), dec!("100000"), dec!("50000"))?,
        false,
    )?;
    let mut fabric = fabric("unfunded");
    let report = one_pass(&mut node, &mut gateway, &mut feed, t(10))?;
    assert_eq!(report.signals.len(), 1, "premise: the strategy fired");
    assert!(report.orders.is_empty());
    assert_eq!(
        report
            .refusals
            .iter()
            .filter(|(gate, _)| gate == RESERVATION_GATE)
            .count(),
        1,
        "{:?}",
        report.refusals
    );
    node.cell.flush(&mut fabric.mirror, t(11))?;
    fabric.drain();
    assert!(
        filled_outcomes(&fabric).is_empty(),
        "a refused pass produced a fill outcome"
    );
    assert!(
        fabric.outcomes().is_empty(),
        "a refused pass produced any P1 outcome"
    );
    assert_nothing_reached_the_venue(&node, &gateway);
    Ok(())
}

#[test]
fn an_order_past_the_grants_per_order_limit_is_refused_at_the_capital_gate_before_the_venue()
-> Result<()> {
    // Premise: a grant whose per-order limit clears the strategy's order sends
    // it, so the refusal below is the limit and not a cell that cannot trade.
    let (mut roomy, mut roomy_gateway, mut roomy_feed) = node(
        grant_with(dec!("1000000"), dec!("500"), dec!("50000"))?,
        true,
    )?;
    let sent = one_pass(&mut roomy, &mut roomy_gateway, &mut roomy_feed, t(10))?;
    assert_eq!(sent.orders.len(), 1, "{:?}", sent.refusals);
    let notional = sent.orders[0].quantity * sent.orders[0].price;
    assert!(
        notional > dec!("200") && notional <= dec!("500"),
        "{notional}"
    );

    let (mut node, mut gateway, mut feed) = node(
        grant_with(dec!("1000000"), dec!("200"), dec!("50000"))?,
        true,
    )?;
    let mut fabric = fabric("order-limit");
    let report = one_pass(&mut node, &mut gateway, &mut feed, t(10))?;
    assert_eq!(report.signals.len(), 1, "premise: the strategy fired");
    assert!(report.orders.is_empty(), "{:?}", report.orders);
    assert_eq!(report.refusals.len(), 1, "{:?}", report.refusals);
    let (gate, reason) = &report.refusals[0];
    assert_eq!(gate, "capital");
    assert!(reason.contains("200 order limit"), "{reason}");
    node.cell.flush(&mut fabric.mirror, t(11))?;
    fabric.drain();
    assert!(
        fabric.outcomes().is_empty(),
        "a refused order produced a P1 outcome"
    );
    assert_nothing_reached_the_venue(&node, &gateway);
    Ok(())
}

#[test]
fn a_halted_cell_sends_nothing_and_the_ledger_has_nothing_to_book() -> Result<()> {
    let (mut node, mut gateway, mut feed) = funded_node()?;
    let mut fabric = fabric("halted");
    node.cell.apply_polled_halt(
        qip_edge::cell::PolledHalt::Engaged("drill".to_string()),
        t(5),
    );
    assert!(node.cell.is_halted(), "premise: the cell is halted");
    let outcome = run_pass(
        &mut node.cell,
        &mut gateway,
        &mut feed,
        None,
        &mut PassStats::default(),
        t(10),
    )?;
    assert!(matches!(outcome, PassOutcome::Halted { .. }), "{outcome:?}");
    assert_eq!(gateway.submitted_count(), 0);
    node.cell.flush(&mut fabric.mirror, t(11))?;
    fabric.drain();
    assert!(filled_outcomes(&fabric).is_empty());
    Ok(())
}

/// Ship a hand-built journal of one decision through the real mirror, writer
/// and spool, and hand back the outcome records that reached P1.
fn spool_of(decision: Recorded) -> Vec<OutcomeRecord> {
    let mut fabric = fabric("hand-built");
    let mut journal = Journal::new();
    journal.record(decision, t(1));
    ship(&mut journal, &mut fabric.mirror, CELL, Vec::new(), t(2)).expect("room in the channel");
    fabric.drain();
    fabric
        .outcomes()
        .into_iter()
        .map(|(_, outcome)| outcome)
        .collect()
}

fn filled(simulated: bool, shares: Vec<(String, String)>, side: Option<BookSide>) -> Recorded {
    Recorded::Filled {
        order_id: "ord-hand-built".to_string(),
        venue: VENUE.to_string(),
        object: "obj-ACME".to_string(),
        quantity: "5".to_string(),
        price: "101".to_string(),
        simulated,
        shares,
        side,
        quote_unit: Some("GBP".to_string()),
        fee: None,
    }
}

#[test]
fn the_ledger_refuses_a_live_fill_and_a_fill_it_cannot_book_exactly() -> Result<()> {
    let share = vec![("alpha".to_string(), "5".to_string())];

    // Premise: the same fill, simulated and complete, books. Without it the
    // refusals below could be a ledger that books nothing.
    let good = spool_of(filled(true, share.clone(), Some(BookSide::Ask)));
    assert_eq!(good.len(), 1);
    post(&PaperFill::try_from(&good[0])?)?;

    // ADR 0100 section 9's fourth paper fence: a fill not flagged simulated
    // has no type to be booked with.
    let live = spool_of(filled(false, share.clone(), Some(BookSide::Ask)));
    assert_eq!(live.len(), 1, "the writer still records the fact");
    let refusal = PaperFill::try_from(&live[0]).unwrap_err();
    assert!(
        refusal.message().contains("simulated"),
        "{}",
        refusal.message()
    );

    // A fill with no side cannot say debit from credit.
    let sideless = spool_of(filled(true, share, None));
    assert!(PaperFill::try_from(&sideless[0]).is_err());

    // Shares that do not sum to the fill are refused, not re-split.
    let uneven = spool_of(filled(
        true,
        vec![("alpha".to_string(), "4".to_string())],
        Some(BookSide::Ask),
    ));
    let refusal = PaperFill::try_from(&uneven[0]).unwrap_err();
    assert!(refusal.message().contains("sum"), "{}", refusal.message());
    Ok(())
}

#[test]
fn a_replayed_journal_batch_cannot_become_a_second_outcome_and_a_replayed_posting_is_identical()
-> Result<()> {
    let (mut node, mut gateway, mut feed) = funded_node()?;
    let mut fabric = fabric("replay");
    one_pass(&mut node, &mut gateway, &mut feed, t(10))?;
    node.cell.flush(&mut fabric.mirror, t(11))?;
    fabric.drain();
    assert_eq!(
        filled_outcomes(&fabric).len(),
        1,
        "premise: one fill is booked"
    );
    let before = fabric.records().len();
    let batch = fabric
        .mirror
        .seen
        .first()
        .cloned()
        .expect("a batch was shipped");

    // The same batch delivered again does not chain onto the session's tail, so
    // the writer declares a gap rather than writing the fill a second time.
    fabric.mirror.ship(batch)?;
    fabric.drain();
    assert_eq!(
        filled_outcomes(&fabric).len(),
        1,
        "a replayed batch produced a second Filled outcome"
    );
    assert!(fabric.records().len() >= before, "the spool only grows");

    // And posting the one outcome twice yields byte-identical events carrying
    // the same source key, which is what lets the ledger store deduplicate.
    let outcome = &filled_outcomes(&fabric)[0];
    let first = post(&PaperFill::try_from(outcome)?)?;
    let second = post(&PaperFill::try_from(outcome)?)?;
    assert_eq!(first, second);
    assert_eq!(first.source(), second.source());
    Ok(())
}
