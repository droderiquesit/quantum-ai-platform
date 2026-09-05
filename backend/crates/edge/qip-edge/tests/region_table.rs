//! The per-region reservation table, consulted at the cell (blueprint §26/§33,
//! traceability row F6).
//!
//! The property is placement: a cell that has lost the centre must refuse its
//! own second proposal against what its first still holds, and two cells
//! under one grant must not each spend the whole grant — and both must hold
//! with no call leaving the process. Every test here drives real passes
//! through `Cell::work`; none asserts that a ledger method returns what it
//! was told, because a ledger nothing calls would pass that forever.
//!
//! "Disconnected" is literal: no cell here has a mesh at all. The grant is
//! the signed envelope each strategy was deployed with and the table the root
//! handed over, both fixed before the first pass, which is exactly what a
//! partitioned cell has to work from (ADR 0008).

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::reservation::RegionTable;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Labels, Metrics, labels, names};
use qip_orderbook::venue::VenueState;
use qip_risk_engine::autonomy::{AutonomyLevel, OperatorIdentity};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::sync::Arc;

const FIRST_CELL: &str = "london-1";
const SECOND_CELL: &str = "london-2";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const SYMBOL: &str = "ACME";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
/// The gate literal `Cell::hold_region_capital` refuses under. Spelled once
/// here and matched by delimited equality everywhere, because
/// `region_reservation_abandoned` and `region_reservation_return` both carry
/// it as a prefix.
const GATE: &str = "region_reservation";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn by(cell: &str, key: &str, value: &str) -> Labels {
    labels([("cell", cell), ("region", REGION), (key, value)])
}

fn level(sequence: u64, side: BookSide, price: &str, size: &str, when: Timestamp) -> MarketMessage {
    MarketMessage::new(
        object(),
        Origin::new(venue(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: Decimal::parse(price).expect("a decimal literal"),
            quantity: Decimal::parse(size).expect("a decimal literal"),
            order_count: None,
        },
        when,
        when,
    )
}

/// A two-sided book with a mid of 100 and four hundred resting on the ask.
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

/// A strategy whose one rule always holds, so every pass proposes.
fn firing_strategy(id: &str, kind: SignalKind, size: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            kind,
            Expr::Flag(true),
            Expr::Exact(Decimal::parse(size).expect("a decimal literal")),
            Expr::Statistic(0.5),
            10,
        ),
    );
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// An envelope wide enough that the strategy's own grant is never what
/// refuses: a million gross, a hundred thousand per order.
fn signed_envelope(cell: &str, strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            cell,
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
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, cell, t(1))
}

/// A venue that accepts every order, fills nothing on its own, and withdraws
/// a resting order whole when asked — so an order can expire unfilled, which
/// is the case that gives region capital back.
#[derive(Debug, Default)]
struct VenueGateway {
    placed: Vec<(String, Decimal)>,
    cancelled: Vec<String>,
}

impl Placer for VenueGateway {
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
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push((order_id.to_string(), quantity));
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        Vec::new()
    }

    fn can_cancel(&self) -> bool {
        true
    }

    fn cancel(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _at: Timestamp,
    ) -> Result<Decimal> {
        let remaining = self
            .placed
            .iter()
            .find(|(id, _)| id == order_id)
            .map(|(_, quantity)| *quantity)
            .ok_or_else(|| Error::not_found(format!("no order {order_id}")))?;
        self.cancelled.push(order_id.to_string());
        Ok(remaining)
    }
}

fn rest(secs: i64) -> Result<PricingPolicy> {
    PricingPolicy::rest_at_mid(Duration::from_secs(secs))
}

/// A cell of `cell_id` holding the book, one always-firing `Enter` strategy
/// per entry in `strategies`, the given table, and the given registry.
fn cell_under(
    cell_id: &str,
    strategies: &[(&str, SignalKind, PricingPolicy)],
    table: Option<&RegionTable>,
    metrics: Option<&Arc<Metrics>>,
) -> Result<Cell> {
    let config = CellConfig::new(cell_id, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    if let Some(metrics) = metrics {
        cell = cell.with_metrics(Arc::clone(metrics));
    }
    if let Some(table) = table {
        cell = cell.with_region_table(table.clone());
    }
    cell.track(book()?);
    for (id, kind, pricing) in strategies {
        let (compiled, program) = firing_strategy(id, *kind, "100")?;
        cell.deploy_with_pricing(compiled, program, signed_envelope(cell_id, id)?, *pricing)?;
    }
    Ok(cell)
}

/// What one pass of one `Enter` strategy under `pricing` commits against the
/// region, measured by running it against a table far larger than it could
/// need. Measured rather than restated as a literal: the degradation floor
/// scales every size, so a literal here would be a number this file believed
/// and the cell did not.
fn one_pass_holds(pricing: PricingPolicy) -> Result<Decimal> {
    let opening = dec!("100000000");
    let table = RegionTable::new(opening)?;
    let mut cell = cell_under(
        FIRST_CELL,
        &[("alpha", SignalKind::Enter, pricing)],
        Some(&table),
        None,
    )?;
    let mut gateway = VenueGateway::default();
    let report = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        report.orders.len(),
        1,
        "the probe premise failed: the pass placed no order: {:?}",
        report.refusals
    );
    let held = opening - table.free();
    assert!(held.is_positive(), "the probe pass committed nothing");
    assert_eq!(table.committed_total(), held);
    Ok(held)
}

fn refused_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(recorded, _)| recorded == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

fn abandoned_entries(cell: &Cell) -> usize {
    cell.journal()
        .entries()
        .iter()
        .filter(|entry| {
            matches!(
                &entry.decision,
                Decision::Refused { gate, .. } if gate == "region_reservation_abandoned"
            )
        })
        .count()
}

fn expired_entries(cell: &Cell) -> usize {
    cell.journal()
        .entries()
        .iter()
        .filter(|entry| matches!(&entry.decision, Decision::OrderExpired { .. }))
        .count()
}

/// Proven at compile time: a table a root can hand to a cell on another
/// thread. Nothing calls this; a `RegionTable` that stopped being `Send +
/// Sync` would stop this file compiling, which is the point.
#[allow(dead_code)]
fn a_region_table_can_be_shared_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RegionTable>();
}

#[test]
fn a_disconnected_cells_second_proposal_is_refused_against_what_its_first_still_holds_until_that_order_expires()
-> Result<()> {
    // One cell, no mesh, one strategy that proposes every pass, resting its
    // orders for ten seconds; a table of exactly one pass's worth. The
    // failure this prevents: a partitioned cell whose first order is resting
    // at the venue proposing again, and the only thing that could refuse it
    // — the centre's ledger — being on the far side of the partition.
    let policy = rest(10)?;
    let one = one_pass_holds(policy)?;
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let table = RegionTable::new(one)?;
    let mut cell = cell_under(
        FIRST_CELL,
        &[("alpha", SignalKind::Enter, policy)],
        Some(&table),
        Some(&metrics),
    )?;
    let mut gateway = VenueGateway::default();

    let first = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        1,
        "the premise failed: the first proposal was not sent: {:?}",
        first.refusals
    );
    let first_order = first.orders[0].order_id.clone();
    assert_eq!(
        table.free(),
        Decimal::ZERO,
        "the premise failed: the first order did not spend the whole table"
    );

    // Second pass, the order still resting. The strategy fires again — the
    // premise, without which the test would pass against a strategy that
    // simply went quiet — and the region gate, by its literal, is what stops
    // it. No other gate refuses: the envelope of a million would admit it.
    let second = cell.work(t(55), &mut gateway)?;
    assert_eq!(
        second.signals.len(),
        1,
        "the premise failed: the strategy did not propose again: {:?}",
        second.refusals
    );
    assert!(
        second.orders.is_empty(),
        "the second proposal was sent against capital the first still holds: {:?}",
        second.orders
    );
    let refusals = refused_under(&second, GATE);
    assert_eq!(
        refusals.len(),
        1,
        "the region gate did not refuse exactly once under `{GATE}`: {:?}",
        second.refusals
    );
    assert_eq!(
        second.refusals.len(),
        1,
        "something other than the region gate refused, so the region was not what held: {:?}",
        second.refusals
    );
    assert!(
        refusals[0].contains("the 0 the region allocation has left"),
        "the refusal did not name what the region had left: {}",
        refusals[0]
    );
    assert_eq!(
        metrics
            .snapshot()
            .counter(names::EDGE_REFUSALS, &by(FIRST_CELL, "gate", GATE)),
        1,
        "the refusal did not move the refusals series under the region gate"
    );
    assert!(
        gateway.cancelled.is_empty(),
        "the resting order was withdrawn before its time to live"
    );

    // At the time to live the venue withdraws the first order whole. Nothing
    // of it filled, so it never ran, and its capital returns — in the same
    // pass, before the strategy proposes, so the third proposal is admitted
    // against the capital the first gave back and spends it in turn.
    let third = cell.work(t(60), &mut gateway)?;
    assert_eq!(
        gateway.cancelled,
        vec![first_order],
        "the premise failed: the first order was not withdrawn at its time to live"
    );
    assert_eq!(
        third.orders.len(),
        1,
        "the expired order's capital did not return, so the cell starved on an order that \
         never ran: {:?}",
        third.refusals
    );
    assert_eq!(
        table.free(),
        Decimal::ZERO,
        "the third order did not spend what the first returned"
    );
    assert_eq!(
        table.committed_total(),
        one,
        "the table counts more than one order's capital as spent, so the expiry did not \
         return the first's"
    );
    Ok(())
}

#[test]
fn two_cells_under_one_region_table_cannot_each_spend_the_whole_grant() -> Result<()> {
    // The blueprint's per-region shape, and the case the F6 row said nothing
    // covered: two cells, each with its own signed envelope wide enough to
    // admit its order, sharing one table of one order's worth. Neither has a
    // mesh; the centre sees neither. Before the table existed each cell
    // owned its own ledger, so both would have sent.
    let policy = PricingPolicy::Marketable;
    let one = one_pass_holds(policy)?;
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let table = RegionTable::new(one)?;
    let mut first = cell_under(
        FIRST_CELL,
        &[("alpha", SignalKind::Enter, policy)],
        Some(&table),
        Some(&metrics),
    )?;
    let mut second = cell_under(
        SECOND_CELL,
        &[("alpha", SignalKind::Enter, policy)],
        Some(&table),
        Some(&metrics),
    )?;

    // Contrast, first: the same two cells over two *separate* tables of the
    // same amount both send. Without this the refusal below could be the
    // amount being too small for either cell, not the table being shared.
    let mut separate_first = cell_under(
        FIRST_CELL,
        &[("alpha", SignalKind::Enter, policy)],
        Some(&RegionTable::new(one)?),
        None,
    )?;
    let mut separate_second = cell_under(
        SECOND_CELL,
        &[("alpha", SignalKind::Enter, policy)],
        Some(&RegionTable::new(one)?),
        None,
    )?;
    let mut gateway = VenueGateway::default();
    assert_eq!(
        separate_first.work(t(50), &mut gateway)?.orders.len(),
        1,
        "the premise failed: the first cell cannot send even alone"
    );
    assert_eq!(
        separate_second.work(t(50), &mut gateway)?.orders.len(),
        1,
        "the premise failed: the second cell cannot send even alone"
    );

    let first_report = first.work(t(50), &mut gateway)?;
    assert_eq!(
        first_report.orders.len(),
        1,
        "the premise failed: the first cell did not send: {:?}",
        first_report.refusals
    );
    assert_eq!(
        table.free(),
        Decimal::ZERO,
        "the premise failed: the first cell's order did not spend the whole table"
    );

    let second_report = second.work(t(50), &mut gateway)?;
    assert_eq!(
        second_report.signals.len(),
        1,
        "the premise failed: the second cell's strategy did not propose: {:?}",
        second_report.refusals
    );
    assert!(
        second_report.orders.is_empty(),
        "the second cell sent against capital the first cell spent — both cells spent the \
         whole grant: {:?}",
        second_report.orders
    );
    let refusals = refused_under(&second_report, GATE);
    assert_eq!(
        refusals.len(),
        1,
        "the second cell was not refused exactly once under `{GATE}`: {:?}",
        second_report.refusals
    );
    assert_eq!(
        second_report.refusals.len(),
        1,
        "something other than the region gate refused the second cell: {:?}",
        second_report.refusals
    );
    assert!(
        refusals[0].contains("the 0 the region allocation has left"),
        "the refusal did not name what the region had left: {}",
        refusals[0]
    );
    assert_eq!(
        metrics
            .snapshot()
            .counter(names::EDGE_REFUSALS, &by(SECOND_CELL, "gate", GATE)),
        1,
        "the second cell's refusal did not chart under its own cell label"
    );
    assert_eq!(
        metrics
            .snapshot()
            .counter(names::EDGE_REFUSALS, &by(FIRST_CELL, "gate", GATE)),
        0,
        "the first cell was charged a refusal it did not make"
    );
    // Both cells read the same balance: it is one table, not two that agree.
    assert_eq!(first.region_allocation_free(), Some(Decimal::ZERO));
    assert_eq!(second.region_allocation_free(), Some(Decimal::ZERO));
    assert_eq!(table.committed_total(), one);
    Ok(())
}

#[test]
fn a_committed_reservation_survives_the_cells_halt_and_a_halted_pass_neither_sweeps_nor_returns_it()
-> Result<()> {
    // A cell that halts with an order resting must go on counting that
    // order's capital as spent: the order is still at the venue and may
    // still fill. The pass-start sweep runs before the halt check, which is
    // right for abandoned holds and would be wrong for commits — so the test
    // pins that a halted pass returns nothing that is committed.
    let opening = dec!("100000000");
    let table = RegionTable::new(opening)?;
    let mut cell = cell_under(
        FIRST_CELL,
        &[("alpha", SignalKind::Enter, rest(10)?)],
        Some(&table),
        None,
    )?;
    let mut gateway = VenueGateway::default();
    let first = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        1,
        "the premise failed: no order was sent: {:?}",
        first.refusals
    );
    let after_send = table.free();
    assert!(
        after_send < opening,
        "the premise failed: the order committed nothing"
    );

    cell.autonomy_mut().kill_switch_mut().trip_global(
        t(52),
        "drill",
        "a halt with an order resting",
    );
    assert!(cell.is_halted(), "the premise is a halted cell");

    let halted = cell.work(t(55), &mut gateway)?;
    assert!(halted.halted);
    assert!(
        !refused_under(&halted, "kill_switch").is_empty(),
        "the premise failed: the pass was not refused under the kill switch: {:?}",
        halted.refusals
    );
    assert_eq!(
        table.free(),
        after_send,
        "a halted pass returned capital an order still resting at the venue had committed"
    );
    assert_eq!(
        abandoned_entries(&cell),
        0,
        "the halted pass's sweep treated a committed order's capital as an abandoned hold"
    );
    assert!(
        gateway.cancelled.is_empty(),
        "the halt withdrew an order before its time to live"
    );

    // Still halted at the time to live: withdrawing is not sending, so the
    // venue withdraws it whole and — nothing having filled — the capital
    // returns while the cell stays halted. The cell then holds all of it
    // again and still sends nothing.
    let expiry = cell.work(t(60), &mut gateway)?;
    assert!(expiry.halted);
    assert_eq!(
        gateway.cancelled.len(),
        1,
        "the premise failed: the resting order was not withdrawn at its time to live"
    );
    assert_eq!(
        table.free(),
        opening,
        "an order withdrawn unfilled while halted kept its capital spent"
    );
    assert!(expiry.orders.is_empty(), "a halted cell sent");
    Ok(())
}

#[test]
fn an_expired_orders_capital_returns_once_and_no_later_pass_returns_it_again() -> Result<()> {
    // Two resting orders of the same size, sent five seconds apart, and the
    // cell put at observation so nothing further is proposed. When the first
    // expires its capital returns; the pass after that must not return it a
    // second time — the second order's commit is the same amount, so a
    // repeated return would be admitted by the ledger's bound and the region
    // would read as untouched while an order still rested at the venue.
    let opening = dec!("100000000");
    let table = RegionTable::new(opening)?;
    let mut cell = cell_under(
        FIRST_CELL,
        &[("alpha", SignalKind::Enter, rest(10)?)],
        Some(&table),
        None,
    )?;
    let mut gateway = VenueGateway::default();
    let first = cell.work(t(50), &mut gateway)?;
    assert_eq!(first.orders.len(), 1, "{:?}", first.refusals);
    let one = opening - table.free();
    assert!(
        one.is_positive(),
        "the premise failed: nothing was committed"
    );
    let second = cell.work(t(55), &mut gateway)?;
    assert_eq!(second.orders.len(), 1, "{:?}", second.refusals);
    assert_eq!(
        table.free(),
        opening - one - one,
        "the premise failed: the two orders did not commit the same amount"
    );

    let operator = OperatorIdentity::verified("alice@example.com", "oidc", t(56));
    cell.autonomy_mut().request_change(
        AutonomyLevel::Observation,
        &operator,
        "so the passes below propose nothing and the balance moves only on expiry",
        t(56),
    )?;

    // The first order expires; the second, sent at 55, has until 65.
    cell.work(t(60), &mut gateway)?;
    assert_eq!(
        gateway.cancelled.len(),
        1,
        "the premise failed: exactly one order should have expired"
    );
    assert_eq!(
        table.free(),
        opening - one,
        "the first order's expiry did not return exactly its own capital"
    );

    // A pass in between: nothing is due, nothing returns.
    cell.work(t(62), &mut gateway)?;
    assert_eq!(
        table.free(),
        opening - one,
        "the expired order's capital was returned a second time"
    );
    assert_eq!(
        gateway.cancelled.len(),
        1,
        "an order already withdrawn was withdrawn again"
    );
    assert_eq!(
        expired_entries(&cell),
        1,
        "the expiry was journaled more than once"
    );

    cell.work(t(65), &mut gateway)?;
    assert_eq!(gateway.cancelled.len(), 2);
    assert_eq!(
        table.free(),
        opening,
        "the second order's expiry did not return its capital"
    );
    assert_eq!(table.committed_total(), Decimal::ZERO);
    Ok(())
}

#[test]
fn a_net_that_cancelled_releases_its_holds_once_and_the_next_passs_sweep_finds_nothing()
-> Result<()> {
    // Cancelled internally, the two intents' holds go back in the pass that
    // took them. The sweep at the top of the next pass is the backstop for a
    // hold nothing released; if a released hold were still on the table it
    // would return a second time and the region would grow past what its
    // operator allocated.
    let opening = dec!("100000000");
    let table = RegionTable::new(opening)?;
    let mut cell = cell_under(
        FIRST_CELL,
        &[
            ("alpha", SignalKind::Enter, PricingPolicy::Marketable),
            ("beta", SignalKind::Exit, PricingPolicy::Marketable),
        ],
        Some(&table),
        None,
    )?;
    let mut gateway = VenueGateway::default();
    let first = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        first.signals.len(),
        2,
        "the premise failed: both strategies must fire: {:?}",
        first.refusals
    );
    assert_eq!(
        first.cancelled.len(),
        1,
        "the premise failed: the two intents did not cancel: {:?}",
        first.refusals
    );
    assert_eq!(
        table.free(),
        opening,
        "a net that reached no venue kept the region's capital"
    );
    assert_eq!(
        table.held_total(),
        Decimal::ZERO,
        "a released hold is still on the table"
    );

    // The next pass cancels again and, above that, sweeps. Nothing to sweep.
    let second = cell.work(t(60), &mut gateway)?;
    assert_eq!(second.cancelled.len(), 1, "{:?}", second.refusals);
    assert_eq!(
        table.free(),
        opening,
        "the region grew past its opening amount: a hold was returned twice"
    );
    assert_eq!(
        abandoned_entries(&cell),
        0,
        "the sweep found a hold that phase three had already released"
    );
    Ok(())
}
