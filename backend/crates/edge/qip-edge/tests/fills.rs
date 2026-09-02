//! A fill is a venue fact.
//!
//! The cell once wrote a fill the moment a venue *accepted* an order, so an
//! order that rested was a position the cell believed in and the venue did
//! not. The reconciler, doing its job, halted the cell on the first strategy
//! that fired against a real two-sided book — and every consumer of the
//! cell's record (the ledger, attribution, the central aggregate) had by then
//! been handed a trade that never happened. Each test here drives the cell
//! through the order-entry channel and asserts what the cell may believe at
//! each step: nothing on acceptance, the reported quantity on a report,
//! attributed pro rata, and a halt when a channel names something the cell
//! never sent.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, PlacedOrder, Placer, WorkReport};
use qip_edge::dropcopy::{CellFill, Discrepancy, DropCopyFill, DropCopyReconciler};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::telemetry::EDGE_FILLS_CONFIRMED;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Labels, Metrics, labels, names};
use qip_orderbook::venue::VenueState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const SYMBOL: &str = "ACME";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-fill-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn by(key: &str, value: &str) -> Labels {
    labels([("cell", CELL), ("region", REGION), (key, value)])
}

fn base() -> Labels {
    labels([("cell", CELL), ("region", REGION)])
}

fn level(sequence: u64, side: BookSide, price: &str, size: &str, when: Timestamp) -> MarketMessage {
    MarketMessage::new(
        object(),
        Origin::new(venue(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: d(price),
            quantity: d(size),
            order_count: None,
        },
        when,
        when,
    )
}

/// A two-sided book with a mid of 100.
fn book() -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "500"), (BookSide::Ask, "101", "400")]
            .iter()
            .enumerate()
    {
        state.apply(&level(index as u64, *side, price, size, t(index as i64)))?;
    }
    Ok(state)
}

fn firing_strategy(id: &str, kind: SignalKind, size: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            kind,
            Expr::Flag(true),
            Expr::Exact(d(size)),
            Expr::Statistic(0.5),
            10,
        ),
    );
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn signed_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
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

/// A gateway that accepts every order and reports only what the test tells
/// it the venue did. It never infers a fill from an order: that is the
/// property under test, and a fixture that did would hide its absence.
#[derive(Debug, Default)]
struct ReportingGateway {
    placed: Vec<(String, Decimal, Decimal)>,
    reports: Vec<ExecutionReport>,
}

impl ReportingGateway {
    fn report(&mut self, order_id: &str, quantity: Decimal, price: Decimal, at: Timestamp) {
        self.reports.push(ExecutionReport {
            order_id: order_id.to_string(),
            venue: venue(),
            quantity,
            price,
            at,
        });
    }
}

impl Placer for ReportingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push((order_id.to_string(), quantity, price));
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }
}

fn trading_cell(strategies: &[(&str, SignalKind, &str)]) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));
    cell.track(book()?);
    for (id, kind, size) in strategies {
        let (compiled, program) = firing_strategy(id, *kind, size)?;
        cell.deploy(compiled, program, signed_envelope(id)?)?;
    }
    Ok((cell, metrics))
}

/// One pass that sends exactly one order, returned with its report.
fn send_one(
    cell: &mut Cell,
    gateway: &mut ReportingGateway,
    at: Timestamp,
) -> Result<(WorkReport, PlacedOrder)> {
    let report = cell.work(at, gateway)?;
    assert!(
        report.refusals.is_empty(),
        "the premise is a pass that refuses nothing: {:?}",
        report.refusals
    );
    let order = report
        .orders
        .first()
        .cloned()
        .ok_or_else(|| Error::not_found("an order from a cell that signalled"))?;
    assert_eq!(
        gateway.placed.len(),
        1,
        "the premise is one order at the venue"
    );
    Ok((report, order))
}

fn drop_copy(order: &PlacedOrder, quantity: Decimal, at: Timestamp) -> DropCopyFill {
    DropCopyFill {
        order_id: order.order_id.clone(),
        venue: order.venue.clone(),
        quantity,
        price: order.price,
        at,
    }
}

/// One journaled fill: order id, quantity, and the shares as journaled.
type JournaledFill = (String, String, Vec<(String, String)>);

fn filled_entries(cell: &Cell) -> Vec<JournaledFill> {
    cell.journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::Filled {
                order_id,
                quantity,
                shares,
                ..
            } => Some((order_id.clone(), quantity.clone(), shares.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn an_order_the_venue_accepted_is_not_a_fill_until_the_order_entry_channel_reports_one()
-> Result<()> {
    let (mut cell, metrics) = trading_cell(&[("alpha", SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    let (report, order) = send_one(&mut cell, &mut gateway, t(50))?;
    assert!(
        order.quantity.is_positive(),
        "the premise is an order with size"
    );

    // Accepted, and nothing more. The order is open at its full size, the
    // instrument has no position, no fill was booked or journaled, and the
    // reconciler — which used to halt here — finds nothing to disagree about.
    assert_eq!(
        cell.position(&venue(), &object()),
        Decimal::ZERO,
        "an accepted order was booked as a position before the venue reported a fill"
    );
    assert!(
        cell.fills().is_empty(),
        "an accepted order was recorded as a fill: {:?}",
        cell.fills()
    );
    assert!(
        report.fills.is_empty(),
        "the pass reported a fill nobody confirmed"
    );
    let open = cell.open_orders();
    assert_eq!(open.len(), 1, "the accepted order is not held open");
    assert_eq!(open[0].order_id, order.order_id);
    assert_eq!(open[0].quantity, order.quantity);
    assert_eq!(open[0].filled, Decimal::ZERO);
    assert_eq!(open[0].closed, None, "an unfilled order was closed");
    assert!(
        filled_entries(&cell).is_empty(),
        "the journal carries a fill nobody reported"
    );
    assert!(
        cell.journal()
            .entries()
            .iter()
            .any(|entry| matches!(&entry.decision, Decision::OrderSent { .. })),
        "the premise is an order the journal knows was sent"
    );
    let breaks = cell.reconcile(t(51));
    assert!(
        breaks.is_empty(),
        "a resting order the venue has not filled reconciled as a break: {breaks:?}"
    );
    assert!(!cell.is_halted(), "a resting order halted the cell");
    assert_eq!(
        metrics
            .snapshot()
            .counter(EDGE_FILLS_CONFIRMED, &by("venue", VENUE)),
        0,
        "a fill was counted before the venue reported one"
    );

    // The venue reports half. Now, and only now, the cell holds half.
    let half = order.quantity / dec!("2");
    gateway.report(&order.order_id, half, order.price, t(55));
    let confirmed = cell.confirm_execution_reports(&mut gateway, t(56));
    assert_eq!(confirmed.len(), 1, "the report was not confirmed");
    assert_eq!(confirmed[0].quantity, half);
    assert_eq!(confirmed[0].side, BookSide::Ask, "an Enter is a buy");
    assert_eq!(
        cell.position(&venue(), &object()),
        half,
        "the position is not what filled"
    );
    assert_eq!(cell.fills().len(), 1);
    let open = cell.open_orders();
    assert_eq!(open[0].filled, half);
    assert_eq!(open[0].remaining(), order.quantity - half);
    assert_eq!(open[0].closed, None, "a half-filled order was closed");
    let journaled = filled_entries(&cell);
    assert_eq!(journaled.len(), 1, "the fill did not reach the journal");
    assert_eq!(journaled[0].1, half.to_string());
    assert_eq!(
        metrics
            .snapshot()
            .counter(EDGE_FILLS_CONFIRMED, &by("venue", VENUE)),
        1,
        "the confirmed fill was not counted"
    );

    // The venue's own channel agrees on the half: clean, and the order is
    // still open because it is not finished.
    cell.observe_drop_copy(drop_copy(&order, half, t(55)));
    let breaks = cell.reconcile(t(57));
    assert!(
        breaks.is_empty(),
        "a confirmed half fill broke against the venue's half: {breaks:?}"
    );
    assert_eq!(
        cell.open_orders().len(),
        1,
        "a half-filled order was settled"
    );

    // The rest fills. The order closes, both channels agree, and the order
    // settles: gone from the working set and the confirmed list, still in
    // the journal, and the position stands.
    gateway.report(&order.order_id, order.quantity - half, order.price, t(60));
    let confirmed = cell.confirm_execution_reports(&mut gateway, t(61));
    assert_eq!(confirmed.len(), 1);
    assert_eq!(
        cell.open_orders()[0].closed.as_deref(),
        Some("filled"),
        "a fully filled order was not closed"
    );
    cell.observe_drop_copy(drop_copy(&order, order.quantity - half, t(60)));
    let breaks = cell.reconcile(t(62));
    assert!(breaks.is_empty(), "{breaks:?}");
    assert!(
        cell.open_orders().is_empty(),
        "a settled order is still open"
    );
    assert!(
        cell.fills().is_empty(),
        "a settled order's fills are still held"
    );
    assert_eq!(
        cell.position(&venue(), &object()),
        order.quantity,
        "settlement lost the position"
    );
    assert_eq!(
        filled_entries(&cell).len(),
        2,
        "the journal lost a settled fill"
    );
    assert!(!cell.is_halted());
    Ok(())
}

#[test]
fn a_fill_is_attributed_to_every_contributor_pro_rata_and_the_shares_close_to_the_fill()
-> Result<()> {
    // Two strategies buying the same instrument are one order; a partial
    // fill on it belongs to both, in proportion to what each wanted, and the
    // shares sum to the fill exactly — not to the order, which is what an
    // attribution done at acceptance would have summed to.
    let (mut cell, _metrics) = trading_cell(&[
        ("alpha", SignalKind::Enter, "100"),
        ("beta", SignalKind::Enter, "50"),
    ])?;
    let mut gateway = ReportingGateway::default();
    let (_, order) = send_one(&mut cell, &mut gateway, t(50))?;
    assert_eq!(
        order.contributors.len(),
        2,
        "the premise is one order carrying two strategies"
    );
    let partial = order.quantity / dec!("3");
    assert!(partial < order.quantity, "the premise is a partial fill");

    gateway.report(&order.order_id, partial, order.price, t(55));
    let confirmed = cell.confirm_execution_reports(&mut gateway, t(56));
    assert_eq!(confirmed.len(), 1);
    let fill = &confirmed[0];
    let total: Decimal = fill.shares.iter().map(|(_, share)| *share).sum();
    assert_eq!(
        total, partial,
        "the shares do not sum to the fill: {:?}",
        fill.shares
    );
    let share_of = |id: &str| {
        fill.shares
            .iter()
            .find(|(strategy, _)| strategy.as_str() == id)
            .map(|(_, share)| *share)
    };
    let alpha = share_of("alpha").ok_or_else(|| Error::not_found("alpha's share"))?;
    let beta = share_of("beta").ok_or_else(|| Error::not_found("beta's share"))?;
    assert!(
        alpha.is_positive() && beta.is_positive(),
        "a contributor received nothing"
    );
    assert_eq!(
        alpha,
        beta * dec!("2"),
        "alpha wanted twice what beta wanted and was not attributed twice the fill"
    );
    // The journal carries the same split, as strings, so the chain alone
    // answers who traded what.
    let journaled = filled_entries(&cell);
    assert_eq!(journaled.len(), 1);
    let journaled_total: Decimal = journaled[0].2.iter().map(|(_, share)| d(share)).sum();
    assert_eq!(
        journaled_total, partial,
        "the journaled shares do not close"
    );
    Ok(())
}

#[test]
fn a_report_naming_an_order_the_cell_never_sent_is_a_break_and_halts_the_cell() -> Result<()> {
    // The order-entry channel saying the cell traded something it has no
    // record of is the drop-copy failure arriving on the other channel:
    // a position nobody is watching.
    let (mut cell, metrics) = trading_cell(&[("alpha", SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    assert!(!cell.is_halted(), "the premise is a running cell");
    gateway.report("ghost-1", dec!("10"), dec!("100"), t(5));
    let confirmed = cell.confirm_execution_reports(&mut gateway, t(6));
    assert!(
        confirmed.is_empty(),
        "a fill on an unknown order was confirmed: {confirmed:?}"
    );
    assert!(
        cell.is_halted(),
        "a fill on an order the cell never sent left it running"
    );
    assert!(
        cell.reconciliation_breaks()
            .iter()
            .any(|detail| detail.contains("ghost-1") && detail.contains("no open order")),
        "the break does not name the order: {:?}",
        cell.reconciliation_breaks()
    );
    assert_eq!(
        metrics
            .snapshot()
            .counter(names::EDGE_RECONCILIATION_BREAKS, &base()),
        1
    );
    Ok(())
}

#[test]
fn a_report_past_the_order_s_size_is_booked_and_halts_the_cell() -> Result<()> {
    // The venue says more traded than was sent. The position is real — the
    // venue is the fact — so it is booked; and the cell's record has been
    // shown wrong, so it stops.
    let (mut cell, _metrics) = trading_cell(&[("alpha", SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    let (_, order) = send_one(&mut cell, &mut gateway, t(50))?;
    let over = order.quantity * dec!("2");
    gateway.report(&order.order_id, over, order.price, t(55));
    let confirmed = cell.confirm_execution_reports(&mut gateway, t(56));
    assert_eq!(
        confirmed.len(),
        1,
        "an over-fill the venue reported was not booked"
    );
    assert_eq!(
        cell.position(&venue(), &object()),
        over,
        "the booked position is not what the venue reported"
    );
    assert!(cell.is_halted(), "an over-fill left the cell running");
    Ok(())
}

#[test]
fn two_partial_drop_copies_on_one_order_accumulate_and_a_redelivery_does_not() -> Result<()> {
    // A venue that fills an order in parts reports each part. Replacing the
    // first with the second — which is what the reconciler did — made a
    // filled order look half filled, and a redelivered part must still not
    // count twice.
    let mut reconciler = DropCopyReconciler::new();
    let first = DropCopyFill {
        order_id: "in-parts".to_string(),
        venue: venue(),
        quantity: dec!("40"),
        price: dec!("50"),
        at: t(1),
    };
    let second = DropCopyFill {
        quantity: dec!("60"),
        at: t(2),
        ..first.clone()
    };
    reconciler.observe(first.clone());
    reconciler.observe(second.clone());
    reconciler.observe(second);
    assert_eq!(reconciler.observed(), 1, "one order, however many parts");
    assert_eq!(
        reconciler.venue_quantity("in-parts"),
        Some(dec!("100")),
        "the parts did not accumulate, or the redelivery counted"
    );
    let cell_fills = vec![CellFill {
        order_id: "in-parts".to_string(),
        venue: venue(),
        quantity: dec!("100"),
        price: dec!("50"),
    }];
    let breaks = reconciler.reconcile(&cell_fills);
    assert!(breaks.is_empty(), "{breaks:?}");

    // And the comparison is on what traded, not on how it was sliced: the
    // cell confirming the same hundred as one fill and the venue reporting
    // it as two is agreement.
    reconciler.observe(first);
    let breaks = reconciler.reconcile(&cell_fills);
    assert!(
        breaks.is_empty(),
        "a redelivered part broke a settled comparison: {breaks:?}"
    );
    Ok(())
}

#[test]
fn a_settled_order_s_redelivered_drop_copy_is_recognised_and_a_new_fill_on_it_is_not() -> Result<()>
{
    // After settlement the venue keeps redelivering, which is the channel's
    // habit and not a fact. A fill it has not delivered before is a fact —
    // one the cell has no record of — and stops the cell.
    let (mut cell, _metrics) = trading_cell(&[("alpha", SignalKind::Enter, "100")])?;
    let mut gateway = ReportingGateway::default();
    let (_, order) = send_one(&mut cell, &mut gateway, t(50))?;
    gateway.report(&order.order_id, order.quantity, order.price, t(55));
    cell.confirm_execution_reports(&mut gateway, t(56));
    let copy = drop_copy(&order, order.quantity, t(55));
    cell.observe_drop_copy(copy.clone());
    assert!(cell.reconcile(t(57)).is_empty());
    assert!(
        cell.open_orders().is_empty(),
        "the premise is a settled order"
    );

    cell.observe_drop_copy(copy);
    let breaks = cell.reconcile(t(58));
    assert!(
        breaks.is_empty(),
        "a redelivery after settlement was a break: {breaks:?}"
    );
    assert!(!cell.is_halted());

    cell.observe_drop_copy(drop_copy(&order, dec!("1"), t(70)));
    let breaks = cell.reconcile(t(71));
    assert_eq!(
        breaks.len(),
        1,
        "a fresh fill on a settled order was not a break: {breaks:?}"
    );
    assert!(
        matches!(&breaks[0], Discrepancy::UnknownToCell { order_id, .. } if *order_id == order.order_id),
        "the break is not the venue knowing more than the cell: {breaks:?}"
    );
    assert!(
        cell.is_halted(),
        "the cell kept running on a fill it has no record of"
    );
    Ok(())
}
