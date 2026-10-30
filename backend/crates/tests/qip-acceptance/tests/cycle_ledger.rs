//! A completed arbitrage cycle, leg by leg, on the global ledger
//! (R12-MESH-5066).
//!
//! ```text
//! Cell::work (desk finds the cross-venue cycle, sends both legs)
//!   -> Placer (paper venue) reports each leg filled
//!   -> Cell::confirm_execution_reports -> Cell journal (hash chain)
//!   -> Cell::flush -> FabricMirror -> SpoolWriter -> SegmentLog -> P1 records
//!   -> LedgerStore::apply, one balanced LedgerEvent per filled leg
//! ```
//!
//! ADR 0153 decision 6 declines a cycle-completion record: the P1 outcome row
//! keeps its six kinds, `CycleCommitted` stays on P2, and "every leg of the
//! cycle filled" is a property of the booked fills rather than a tally the
//! cell would keep. This suite is the proof that decision names. The cycle
//! here starts and ends in the same unit at the same venue — buy at one venue,
//! move, sell at the other, move the cash home — so the property reads
//! directly off the ledger's own accounts, with no mapping the test supplies:
//! the desk's holding of the instrument nets to zero across the two legs, and
//! the cash it ends with is exactly what the two fills' arithmetic says. A
//! triangle on one venue would not do, because the ledger keys a position by
//! the market the fill named (`ETHUSDT`, `ETHBTC`), and nothing in the ledger
//! knows one market's base is another's quote.
//!
//! Every hop is the production type. The broker between the spool and
//! `qip-ledgerd` is the one thing not reached, as in
//! `vertical_slice_cell_ledger.rs`; `apps/qip-ledgerd/tests/fabric.rs` covers
//! that leg, and the overlap is the P1 record.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_arbitrage::{
    ArbitrageGraph, EdgeAssumptions, Node, OpportunityScanner, PlanSettings, SearchSettings,
    SizePolicy, VenueFacts,
};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::ledger::Account;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{BeliefPriors, CausalDigest, EpisodicDigest, PolicyPayload, Slot};
use qip_contracts::reflex::{Decision as Recorded, OutcomeRecord};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::money::Currency;
use qip_core::{Clock, Decimal, Duration, ManualClock, ObjectId, Timestamp, dec};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer, QuoteTerms};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::{Decision, Mirror, MirrorBatch};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::event_fabric::mirror::FabricMirror;
use qip_edge_node::event_fabric::pressure::{PressureGauge, Thresholds};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{
    self, RecordKind, SpoolWriter, SpooledRecord, Step, WriterConfig,
};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_ledgerd::store::{Delivery, Disposition, LedgerStore, P1Record};
use qip_ledgerd::telemetry::LedgerTelemetry;
use qip_observability::metrics::Metrics;
use qip_orderbook::venue::VenueState;
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE_A: &str = "CXA";
const VENUE_B: &str = "CXB";
const DESK: &str = "arb-desk";
const MARKET: &str = "BTCUSDT";
const CASH: &str = "USDT";
const ENVELOPE_KEY: &[u8] = b"a-cycle-ledger-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cycle-ledger-policy-key-for-tests";
const TRANSFER_OBSERVATIONS: u32 = 512;

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

fn venue_a() -> VenueId {
    VenueId::new(VENUE_A)
}

fn venue_b() -> VenueId {
    VenueId::new(VENUE_B)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

/// A two-sided book for one market at one venue, built from feed messages.
fn book_at(at: &VenueId, bid: (&str, &str), ask: (&str, &str)) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(MARKET), at.clone(), VenueStatus::Open);
    for (index, (side, (price, size))) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            object(MARKET),
            Origin::new(at.clone(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side,
                price: d(price),
                quantity: d(size),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    Ok(state)
}

/// BTC is ten per cent dearer at the second venue: a dislocation no cost
/// model prices away.
fn books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book_at(&venue_a(), ("59999", "10"), ("60000", "10"))?,
        book_at(&venue_b(), ("66000", "10"), ("66001", "10"))?,
    ])
}

/// Buy at A, move, sell at B, move the cash home. Both venues are in the
/// cell's own region, so the transfers are plain transport edges.
fn graph() -> Result<ArbitrageGraph> {
    let mut graph = ArbitrageGraph::new();
    for at in [venue_a(), venue_b()] {
        graph.register_venue(
            at,
            VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
        );
    }
    graph.add_trade(
        Node::new(object(CASH), venue_a()),
        Node::new(object("BTC"), venue_a()),
        Decimal::ONE,
        Decimal::ZERO,
        object(MARKET),
        BookSide::Ask,
        t(0),
        0,
    )?;
    graph.add_transfer(
        object("BTC"),
        venue_a(),
        venue_b(),
        Decimal::ZERO,
        t(1),
        TRANSFER_OBSERVATIONS,
    )?;
    graph.add_trade(
        Node::new(object("BTC"), venue_b()),
        Node::new(object(CASH), venue_b()),
        Decimal::ONE,
        Decimal::ZERO,
        object(MARKET),
        BookSide::Bid,
        t(0),
        0,
    )?;
    graph.add_transfer(
        object(CASH),
        venue_b(),
        venue_a(),
        Decimal::ZERO,
        t(1),
        TRANSFER_OBSERVATIONS,
    )?;
    Ok(graph)
}

fn signed_envelope() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(DESK),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue_a(), venue_b()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))
}

fn desk() -> Result<ArbitrageDesk> {
    ArbitrageDesk::new(
        StrategyId::new(DESK),
        OpportunityScanner::new(
            SearchSettings {
                max_cycle_edges: 4,
                ..SearchSettings::default()
            },
            EdgeAssumptions::default(),
            PlanSettings::with_budget(d("500000")),
        ),
        graph()?,
        SizePolicy::uniform(d("10000")).with(object("BTC"), d("0.16")),
        signed_envelope()?,
        8,
        Duration::from_secs(30),
    )
}

/// A payload whose capability slots are fresh, so the desk scans at full size.
fn fresh_policy(issued_at: Timestamp) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, issued_at);
    payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::new(),
        },
        issued_at,
    );
    payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: Vec::new(),
        },
        issued_at,
    );
    payload.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "d".to_string(),
            episodes: 0,
        },
        issued_at,
    );
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, issued_at)
}

fn cell() -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION)
        .with_venue(venue_a())
        .with_venue(venue_b());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?
        .with_metrics(Arc::new(Metrics::new("qip-edge-node")))
        .with_arbitrage(desk()?)?;
    cell.apply_policy(fresh_policy(t(5))?, t(5))?;
    for state in books()? {
        cell.track(state);
    }
    Ok(cell)
}

/// A paper venue that accepts every leg and reports each one filled in full
/// at the price it was placed at, in the listing's stated quote unit. It never
/// infers a fill the test did not ask it to report.
#[derive(Debug, Default)]
struct FillingGateway {
    placed: Vec<(String, VenueId, BookSide, Decimal, Decimal)>,
    reports: Vec<ExecutionReport>,
}

impl FillingGateway {
    fn fill_everything(&mut self, at: Timestamp) {
        for (order_id, venue, _, quantity, price) in &self.placed {
            self.reports.push(ExecutionReport {
                order_id: order_id.clone(),
                venue: venue.clone(),
                quantity: *quantity,
                price: *price,
                at,
            });
        }
    }
}

impl Placer for FillingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed
            .push((order_id.to_string(), venue.clone(), side, quantity, price));
        Ok(())
    }

    fn quote_terms(&self, _object_id: &ObjectId, _venue: &VenueId) -> Option<QuoteTerms> {
        Some(QuoteTerms {
            quote_unit: Currency::parse(CASH).expect("a currency code"),
        })
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }
}

fn scratch(label: &str) -> PathBuf {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-cycle-ledger-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The writer the binary runs, over a real `SegmentLog`.
struct Fabric {
    writer: SpoolWriter<SegmentLog>,
    mirror: Forwarding,
    // Held so the writer's heartbeat has a gauge to beat.
    _gauge: PressureGauge,
}

#[derive(Debug)]
struct Forwarding {
    inner: FabricMirror,
}

impl Mirror for Forwarding {
    fn ship(&mut self, batch: MirrorBatch) -> Result<()> {
        self.inner.ship(batch)
    }
}

fn fabric(label: &str) -> Fabric {
    let dir = scratch(label);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let spool = SegmentLog::open(&dir, SegmentLogConfig::new(Arc::clone(&clock))).expect("spool");
    let thresholds = Thresholds::new(1 << 40, d("0.5"), d("0.9")).expect("thresholds");
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
        mirror: Forwarding {
            inner: FabricMirror::new(CELL, sender).expect("mirror"),
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

    /// The spool's P1 records, in spool order, as the broker would deliver
    /// them to `qip-ledgerd`.
    fn p1_deliveries(&self) -> Vec<Delivery> {
        let outcome_topic = RecordKind::OutcomeRecorded.binding().topic;
        let span_topic = RecordKind::ChainSpan.binding().topic;
        self.records()
            .into_iter()
            .filter(|record| {
                record.event.topic == outcome_topic || record.event.topic == span_topic
            })
            .map(|record| {
                if record.event.topic == outcome_topic {
                    let outcome: OutcomeRecord =
                        serde_json::from_value(record.event.payload.clone()).expect("an outcome");
                    P1Record::Outcome(Box::new(outcome))
                } else {
                    P1Record::Span(writer::chain_span(&record.event).expect("a span"))
                }
            })
            .enumerate()
            .map(|(offset, record)| Delivery {
                partition: "p0".to_string(),
                offset: offset as u64,
                record_hash: qip_core::sha256_hex(
                    &serde_json::to_vec(&record).expect("serialises"),
                ),
                record,
            })
            .collect()
    }
}

fn store(label: &str) -> Result<LedgerStore> {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    LedgerStore::open(
        scratch(label),
        qip_storage::EngineConfig::new(clock),
        LedgerTelemetry::new(Arc::new(Metrics::new("qip-ledgerd"))),
    )
}

/// The requirement's own wording, as the mutation it names reads it: every
/// filled leg of a completed cycle is one balanced ledger event, and the
/// cycle's cash nets — the desk ends the cycle holding none of the instrument
/// and exactly the cash the two fills' arithmetic leaves.
///
/// Mutations (each run, each failed here, each restored byte for byte, the
/// file's digest checked before and after):
/// * make `qip_portfolio::ledger::post` write no postings for an `Ask` fill —
///   the buy leg's event is refused by the store as a fill it cannot book
///   exactly, the cell is parked at that sequence, and "exactly the cycle's
///   two legs posted" fails with 0;
/// * drop the two cash postings from the `Bid` arm of `post` — the sell leg
///   books its instrument and no proceeds, and "the cycle's cash is not what
///   its two fills net to" fails, the trading account showing only the buy's
///   cost.
#[test]
fn every_filled_leg_of_a_completed_cycle_posts_balanced_and_the_cycle_cash_nets_to_zero()
-> Result<()> {
    let mut cell = cell()?;
    let mut gateway = FillingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;

    // Premise: the desk found the one cycle and sent both of its legs — a buy
    // at the cheap venue and a sell at the dear one, in that order, for the
    // same quantity. Everything after this is about the record, not the scan.
    assert_eq!(
        report.orders.len(),
        2,
        "the cycle's legs did not both go out: {report:?}"
    );
    assert_eq!(gateway.placed.len(), 2);
    let (buy, sell) = (&gateway.placed[0], &gateway.placed[1]);
    // A leg's side is the book side it crosses: the buy lifts A's ask, the
    // sell hits B's bid.
    assert_eq!((&buy.1, buy.2), (&venue_a(), BookSide::Ask), "{buy:?}");
    assert_eq!((&sell.1, sell.2), (&venue_b(), BookSide::Bid), "{sell:?}");
    assert_eq!(buy.3, sell.3, "the two legs carry one quantity");
    assert!(buy.3.is_positive());
    assert!(
        cell.journal()
            .entries()
            .iter()
            .any(|entry| matches!(entry.decision, Decision::CycleCommitted { .. })),
        "the cycle was not committed on the chain"
    );

    // The venue answers: both legs filled in full at their placed prices.
    gateway.fill_everything(t(11));
    let confirmed = cell.confirm_execution_reports(&mut gateway, t(11));
    assert_eq!(confirmed.len(), 2, "both legs confirmed: {confirmed:?}");
    assert_eq!(
        cell.position(&venue_a(), &object(MARKET)) + cell.position(&venue_b(), &object(MARKET)),
        Decimal::ZERO,
        "premise: the cell itself holds none of the instrument after the cycle"
    );

    // The chain reaches the spool, and the spool's P1 records reach the store.
    let mut fabric = fabric("spool");
    cell.flush(&mut fabric.mirror, t(12))?;
    fabric.drain();
    let deliveries = fabric.p1_deliveries();
    let filled_on_p1 = deliveries
        .iter()
        .filter(|delivery| {
            matches!(&delivery.record, P1Record::Outcome(o) if matches!(o.entry.decision, Recorded::Filled { .. }))
        })
        .count();
    assert_eq!(filled_on_p1, 2, "premise: both fills are on P1");

    let store = store("ledger")?;
    let mut posted = 0;
    for delivery in &deliveries {
        if matches!(store.apply(delivery)?, Disposition::Posted { .. }) {
            posted += 1;
        }
    }
    assert_eq!(posted, 2, "exactly the cycle's two legs posted");
    assert_eq!(store.fill_events()?.len(), 2);

    // Every leg is one balanced event: the store's trial balance is zero in
    // every unit, and each event's own postings sum to zero per unit.
    assert!(
        store.trial_balance()?.values().all(|t| t.is_zero()),
        "{:?}",
        store.trial_balance()?
    );
    for event in store.fill_events()? {
        let mut by_unit: BTreeMap<String, Decimal> = BTreeMap::new();
        for posting in event.postings() {
            let entry = by_unit.entry(posting.unit.clone()).or_insert(Decimal::ZERO);
            match posting.direction {
                qip_contracts::ledger::Direction::Debit => *entry += posting.amount,
                qip_contracts::ledger::Direction::Credit => *entry -= posting.amount,
            }
        }
        assert!(
            by_unit.values().all(|v| v.is_zero()),
            "a leg's event does not balance: {by_unit:?}"
        );
    }

    // The cycle nets: the desk holds none of the instrument, and its cash is
    // the sell's proceeds less the buy's cost — what the two fills say, and
    // nothing the scanner priced.
    let trading = Account::Trading {
        cell: CELL.to_string(),
        strategy: DESK.to_string(),
    };
    assert_eq!(
        store.balance(&trading, MARKET)?,
        Decimal::ZERO,
        "the cycle's instrument does not net to zero on the ledger: {:?}",
        store.balances()?
    );
    let (bought, sold) = (&confirmed[0], &confirmed[1]);
    let expected_cash = sold.quantity * sold.price - bought.quantity * bought.price;
    assert!(
        expected_cash.is_positive(),
        "premise: the dislocation pays: {expected_cash}"
    );
    assert_eq!(
        store.balance(&trading, CASH)?,
        expected_cash,
        "the cycle's cash is not what its two fills net to"
    );
    // The venues hold the mirror image, per venue: A is owed the cost, B
    // gave up the proceeds.
    let at = |venue: &str| Account::Venue {
        venue: venue.to_string(),
    };
    assert_eq!(
        store.balance(&at(VENUE_A), CASH)?,
        bought.quantity * bought.price
    );
    assert_eq!(
        store.balance(&at(VENUE_B), CASH)?,
        -(sold.quantity * sold.price)
    );
    Ok(())
}
