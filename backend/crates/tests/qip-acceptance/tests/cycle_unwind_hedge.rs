//! A broken two-leg cycle, from the cell that unwinds it to the centre that
//! learns what it left behind (R12-MESH-7143, ADR 0208).
//!
//! ```text
//! Cell::work (desk sends leg 1, the venue refuses leg 2)
//!   -> Cell::break_cycle: one offsetting order per filled leg, journaled as a
//!      hedge, and the cell halts
//!   -> the venue fills leg 1 and never the offset: a position is left over
//!   -> Cell::state_delta (the cell's statement of its book, ADR 0208)
//!   -> CellStateDelta::to_frame -> qip_mesh::delta::decode_cell_delta
//!   -> CellReport -> Platform::ingest_cell_report
//!   -> CentralPlane::ingest: the fill cannot settle (its order never reached
//!      the wire), so the settled book holds nothing, and the cell's
//!      statement disagrees with it by exactly the leftover leg
//! ```
//!
//! What this proves, and what it finds. The unwind half is the cell's:
//! the offset goes out and the cell stops. The centre's half is where the
//! finding is: on the node's own pass a refused leg fails `Cell::work`, and
//! `qip-edge-node`'s scheduler keeps no report for a failed pass and none
//! for a halted one, so the orders sent before the refusal never ride the
//! delta, the fill that follows arrives on an order the centre never saw
//! sent, and nothing is charged into the risk aggregate the Hedge Brain
//! plans against. ADR 0208's statement is what makes that visible: the cell
//! says it holds the leg, the centre's settled book says it holds nothing,
//! and the disagreement is on the record. Until the failed pass's orders
//! ride the wire, "hedged end to end" cannot be asserted, and this suite
//! says so rather than asserting it.
//!
//! Every hop is the production type; the socket and the API's sink are
//! covered over a real listener in `qip-api/tests/mesh.rs`.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_arbitrage::{
    ArbitrageGraph, EdgeAssumptions, Node, OpportunityScanner, PlanSettings, SearchSettings,
    SizePolicy, VenueFacts,
};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{BeliefPriors, CausalDigest, EpisodicDigest, PolicyPayload, Slot};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_financial::universe::Universe;
use qip_kernel::central::BreakOrigin;
use qip_kernel::config::{PlatformConfig, paper_hedge_candidates};
use qip_kernel::platform::Platform;
use qip_kernel::{CellReport, CentralPlane};
use qip_observability::Telemetry;
use qip_observability::metrics::Metrics;
use qip_orderbook::venue::VenueState;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::KillSwitch;
use std::collections::BTreeMap;
use std::sync::Arc;

const CELL: &str = "newyork-1";
const REGION: &str = "us-east4";
const VENUE_A: &str = "CXA";
const VENUE_B: &str = "CXB";
const DESK: &str = "arb-desk";
/// A name in the committed catalogue, so the centre knows the instrument.
const EXPOSED: &str = "OBJ00000000000000000NWSC";
const CASH: &str = "USD";
const ENVELOPE_KEY: &[u8] = b"a-cycle-unwind-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cycle-unwind-policy-key-for-tests";
const TRANSFER_OBSERVATIONS: u32 = 512;
const COMMITTED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/universe.json"
);

/// After the committed catalogue's own instants, which a record refuses to
/// be read before.
fn t(secs: i64) -> Timestamp {
    Timestamp::from_civil(2026, 10, 1).saturating_add(Duration::from_secs(secs))
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

fn book_at(at: &VenueId, bid: (&str, &str), ask: (&str, &str)) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(EXPOSED), at.clone(), VenueStatus::Open);
    for (index, (side, (price, size))) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            object(EXPOSED),
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

/// Ten per cent dearer at the second venue, deep enough on both sides for
/// the whole cycle.
fn books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book_at(&venue_a(), ("99", "100000"), ("100", "100000"))?,
        book_at(&venue_b(), ("110", "100000"), ("111", "100000"))?,
    ])
}

fn graph() -> Result<ArbitrageGraph> {
    let mut graph = ArbitrageGraph::new();
    for at in [venue_a(), venue_b()] {
        graph.register_venue(at, VenueFacts::new(VenueClass::Exchange, VenueStatus::Open));
    }
    graph.add_trade(
        Node::new(object(CASH), venue_a()),
        Node::new(object(EXPOSED), venue_a()),
        Decimal::ONE,
        Decimal::ZERO,
        object(EXPOSED),
        BookSide::Ask,
        t(0),
        0,
    )?;
    graph.add_transfer(
        object(EXPOSED),
        venue_a(),
        venue_b(),
        Decimal::ZERO,
        t(1),
        TRANSFER_OBSERVATIONS,
    )?;
    graph.add_trade(
        Node::new(object(EXPOSED), venue_b()),
        Node::new(object(CASH), venue_b()),
        Decimal::ONE,
        Decimal::ZERO,
        object(EXPOSED),
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
            dec!("5000000"),
            dec!("1000000"),
            dec!("200000"),
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
            PlanSettings::with_budget(d("2000000")),
        ),
        graph()?,
        // Seven hundred and twenty thousand dollars of the start instrument:
        // 7,200 shares at the touch, the size the catalogue test needs a book
        // to fail a library world at.
        SizePolicy::uniform(d("720000")).with(object(EXPOSED), d("7200")),
        signed_envelope()?,
        8,
        Duration::from_secs(30),
    )
}

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

/// A venue that refuses the placement attempt with this ordinal, fills the
/// first `fills_through` accepted orders in full at their placed price, and
/// leaves the rest resting — which is how an offset is sent and not filled.
#[derive(Debug, Default)]
struct BreakingGateway {
    attempts: usize,
    refuse_attempt: usize,
    fills_through: usize,
    placed: Vec<(String, VenueId, BookSide, Decimal, Decimal)>,
    pending: Vec<ExecutionReport>,
}

impl Placer for BreakingGateway {
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
        at: Timestamp,
    ) -> Result<()> {
        self.attempts += 1;
        if self.attempts == self.refuse_attempt {
            return Err(Error::io("the venue refused the order"));
        }
        self.placed
            .push((order_id.to_string(), venue.clone(), side, quantity, price));
        if self.placed.len() <= self.fills_through {
            self.pending.push(ExecutionReport {
                order_id: order_id.to_string(),
                venue: venue.clone(),
                quantity,
                price,
                at,
            });
        }
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.pending)
    }
}

fn catalogue() -> Universe {
    let text = std::fs::read_to_string(COMMITTED).expect("the committed catalogue is readable");
    qip_financial::catalogue::load(&text, t(0))
        .expect("the committed catalogue loads")
        .universe
}

fn platform() -> Result<Platform> {
    let universe = catalogue();
    let config = PlatformConfig {
        initial_equity: dec!("1000000"),
        ..PlatformConfig::default()
    }
    .with_hedge_candidates(paper_hedge_candidates(&universe));
    let (context, _clock) = Context::deterministic(t(0), config.seed);
    let limits = LimitSet::new("cycle-unwind-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 3.0 })
            .with_rationale("gross exposure is capped at 3x equity"),
    );
    Platform::new(config, context, Telemetry::silent(), universe, limits)
}

/// Mutations (each run, each failed here, each restored byte for byte):
/// * `Cell::state_delta` shipping `positions: Vec::new()` — the statement
///   carries nothing, the centre raises no disagreement, and "the centre
///   learns the leg the cell holds" fails;
/// * `CentralPlane::advisory_disagreements_of` comparing the stated side
///   against itself — the same assertion fails with no disagreement raised.
#[test]
fn a_broken_cycle_is_unwound_and_hedged_end_to_end() -> Result<()> {
    // --- the cell unwinds -------------------------------------------------
    let mut cell = cell()?;
    let mut gateway = BreakingGateway {
        refuse_attempt: 2,
        fills_through: 1,
        ..BreakingGateway::default()
    };
    assert!(
        cell.work(t(10), &mut gateway).is_err(),
        "premise: the refused second leg did not fail the pass"
    );
    // Premise: the buy went out at the cheap venue, the sell was refused, and
    // exactly one offset — the same book, the other side, the same size —
    // followed, journaled as a hedge, and the cell stopped.
    assert_eq!(gateway.placed.len(), 2, "{:?}", gateway.placed);
    let (leg, offset) = (gateway.placed[0].clone(), gateway.placed[1].clone());
    assert_eq!((&leg.1, leg.2), (&venue_a(), BookSide::Ask), "{leg:?}");
    assert_eq!(
        (&offset.1, offset.2),
        (&venue_a(), BookSide::Bid),
        "{offset:?}"
    );
    assert_eq!(leg.3, offset.3, "the offset is not the filled leg's size");
    assert!(
        cell.journal()
            .entries()
            .iter()
            .any(|entry| matches!(&entry.decision, Decision::Hedged { order_id, .. } if *order_id == offset.0)),
        "the offset was not journaled as a hedge"
    );
    assert!(
        cell.is_halted(),
        "the cell kept trading after a broken cycle"
    );

    // The venue filled the leg and never the offset: the cell confirmed the
    // fill inside the break (the offset is sized by what filled), confirms
    // nothing more now, and is left long.
    assert!(
        cell.confirm_execution_reports(&mut gateway, t(11))
            .is_empty(),
        "premise: nothing was left to confirm after the break"
    );
    let confirmed = cell.fills().to_vec();
    assert_eq!(confirmed.len(), 1, "{confirmed:?}");
    let leftover = cell.position(&venue_a(), &object(EXPOSED));
    assert_eq!(
        leftover, leg.3,
        "premise: the leg is held, the offset is not filled"
    );
    assert!(leftover.is_positive());

    // --- the statement rides the wire -------------------------------------
    // What the node's scheduler keeps for a failed pass is nothing, and for
    // a halted one nothing, so this is the report the next delta is built
    // from: the fills the cell confirmed, and no orders. The finding this
    // suite records starts here.
    let report = WorkReport {
        fills: confirmed.clone(),
        ..WorkReport::default()
    };
    let delta = cell.state_delta(&report, t(11));
    assert_eq!(delta.positions.len(), 1, "{:?}", delta.positions);
    assert_eq!(delta.positions[0].object_id, object(EXPOSED));
    assert_eq!(delta.positions[0].quantity, leftover);
    assert!(
        delta.positions_as_of > 0,
        "the statement carries no sequence"
    );
    assert!(
        delta.orders.is_empty(),
        "premise: the failed pass's orders are not on the report"
    );
    let frame = delta.to_frame()?;
    let decoded = qip_mesh::delta::decode_cell_delta(&frame)?;
    assert_eq!(decoded.standing.positions, delta.positions);
    assert_eq!(decoded.standing.positions_as_of, delta.positions_as_of);

    // --- the centre learns what the cell holds -----------------------------
    let mut platform = platform()?;
    assert!(
        platform.central().advisory_book(CELL).is_none(),
        "premise: the centre holds no statement yet"
    );
    let cell_report = CellReport::new(decoded.standing.cell.clone(), decoded.standing.at)
        .with_region(decoded.standing.region.clone())
        .with_orders(decoded.interval.orders.clone())
        .with_fills(decoded.interval.fills.clone())
        .with_advisory_positions(
            decoded.standing.positions.clone(),
            decoded.standing.positions_as_of,
        );
    let ingestion = platform.ingest_cell_report(cell_report, t(12))?;

    // The fill arrived on an order the centre never saw sent — the node kept
    // no report for the failed pass — so it could not settle, nothing was
    // charged into the aggregate, and the settled book for the cell is
    // empty. That is the finding, recorded as the break it is.
    assert!(
        ingestion
            .settlement
            .breaks
            .iter()
            .any(|found| found.origin == BreakOrigin::UnsentFill),
        "premise: the fill on an unreported order was not found unsent: {:?}",
        ingestion.settlement.breaks
    );
    assert_eq!(ingestion.settlement.fills_settled, 0);

    // ADR 0208: the cell's statement disagrees with the settled book by
    // exactly the leftover leg, and the centre says so.
    assert_eq!(
        ingestion.advisory_disagreements.len(),
        1,
        "the centre did not learn the leg the cell holds: {:?}",
        ingestion.advisory_disagreements
    );
    let disagreement = &ingestion.advisory_disagreements[0];
    assert!(
        disagreement.contains(&format!("states {leftover} of {EXPOSED}"))
            && disagreement.contains("holds 0"),
        "{disagreement}"
    );
    let held = platform
        .central()
        .advisory_book(CELL)
        .expect("the statement is held");
    assert_eq!(held.as_of, delta.positions_as_of);
    assert_eq!(held.positions, delta.positions);
    assert_eq!(held.disagreements.len(), 1);

    // And the same statement on a plane with no platform around it: the
    // refusal of a statement that went backwards is the plane's own.
    let mut plane = CentralPlane::new(&[7u8; 32], qip_kernel::central::CentralConfig::default())?;
    let mut kill_switch = KillSwitch::new();
    plane.ingest(
        CellReport::new(CELL, t(12))
            .with_advisory_positions(delta.positions.clone(), delta.positions_as_of),
        &mut kill_switch,
        t(12),
    )?;
    let older =
        CellReport::new(CELL, t(13)).with_advisory_positions(Vec::new(), delta.positions_as_of - 1);
    assert!(
        plane.ingest(older, &mut kill_switch, t(13)).is_err(),
        "a statement that went backwards was absorbed"
    );
    Ok(())
}
