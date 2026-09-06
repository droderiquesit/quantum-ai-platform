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
use qip_contracts::policy::{GrantManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
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

// --- ADR 0039: the cell's share of its region's grant ------------------------

/// An envelope sized exactly: `gross` in total, and per order, and to lose.
/// The share the cell derives from a manifest naming this envelope is `gross`.
fn envelope_of(
    cell: &str,
    strategy: &str,
    gross: Decimal,
    expires: Timestamp,
) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            cell,
            gross,
            gross,
            gross,
            vec![venue()],
            t(0),
            expires,
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, cell, t(1))
}

/// A cell of `cell_id` over its own table opened `unfunded`, holding one
/// always-firing `Enter` strategy under `envelope` — the deployment's shape:
/// one cell, one process, one private table.
fn unfunded_cell(cell_id: &str, envelope: VerifiedEnvelope) -> Result<(Cell, RegionTable)> {
    unfunded_cell_under(cell_id, envelope, dec!("100000000"))
}

/// As [`unfunded_cell`], with the operator's ceiling stated: the most the
/// table will ever be bounded to, whatever share the centre names.
fn unfunded_cell_under(
    cell_id: &str,
    envelope: VerifiedEnvelope,
    ceiling: Decimal,
) -> Result<(Cell, RegionTable)> {
    let table = RegionTable::unfunded(ceiling)?;
    let config = CellConfig::new(cell_id, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_region_table(table.clone());
    cell.track(book()?);
    let (compiled, program) = firing_strategy("alpha", SignalKind::Enter, "100")?;
    cell.deploy_with_pricing(compiled, program, envelope, PricingPolicy::Marketable)?;
    Ok((cell, table))
}

/// A verified payload for `cell` whose `capital_grants` slot is produced and
/// names `grants` — or unproduced, when `None`.
fn share_policy(
    cell: &str,
    sequence: u64,
    issued_at: Timestamp,
    grants: Option<Vec<String>>,
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, cell, issued_at);
    if let Some(live_grants) = grants {
        payload.capital_grants = Slot::produced(GrantManifest { live_grants }, issued_at);
    }
    VerifiedPolicy::verify(payload.signed(ENVELOPE_KEY)?, ENVELOPE_KEY, cell, issued_at)
}

fn share_entries(cell: &Cell) -> Vec<&Decision> {
    cell.journal()
        .entries()
        .iter()
        .map(|entry| &entry.decision)
        .filter(|decision| matches!(decision, Decision::RegionShareApplied { .. }))
        .collect()
}

#[test]
fn two_cells_in_two_processes_under_disjoint_shares_cannot_together_exceed_the_regions_grant()
-> Result<()> {
    // The deployment's shape, which the shared-table test above cannot
    // reach: two cells over two *separate* tables, each re-based from its
    // own signed payload. The region's grant is one order's worth. The
    // centre shared all of it to the first cell and nothing to the second,
    // so the second's manifest names no grant of its own.
    let grant = one_pass_holds(PricingPolicy::Marketable)?;
    let (mut first, first_table) = unfunded_cell(
        FIRST_CELL,
        envelope_of(FIRST_CELL, "alpha", grant, t(3600))?,
    )?;
    let (mut second, second_table) = unfunded_cell(
        SECOND_CELL,
        envelope_of(SECOND_CELL, "alpha", grant, t(3600))?,
    )?;
    assert!(
        !first_table.shares_with(&second_table),
        "the premise failed: the two cells share a table, which is not the deployment's shape"
    );
    // The signature is deterministic over the envelope's fields, so signing
    // the same terms again names the grant the first cell holds.
    let first_signature = envelope_of(FIRST_CELL, "alpha", grant, t(3600))?
        .signature()
        .to_string();
    first.apply_policy(
        share_policy(FIRST_CELL, 1, t(10), Some(vec![first_signature]))?,
        t(10),
    )?;
    second.apply_policy(share_policy(SECOND_CELL, 1, t(10), Some(vec![]))?, t(10))?;
    assert_eq!(
        first.region_allocation_bound(),
        Some(grant),
        "the premise failed: the first cell's table was not re-based to its share"
    );
    assert_eq!(
        second.region_allocation_bound(),
        Some(Decimal::ZERO),
        "the premise failed: the second cell's table was not re-based to nothing"
    );

    // Contrast, first: shares that together exceed the grant let both send,
    // so what refuses below is the partition and not the amount.
    let (mut over_first, _) = unfunded_cell(
        FIRST_CELL,
        envelope_of(FIRST_CELL, "alpha", grant, t(3600))?,
    )?;
    let (mut over_second, _) = unfunded_cell(
        SECOND_CELL,
        envelope_of(SECOND_CELL, "alpha", grant, t(3600))?,
    )?;
    let over_first_signature = envelope_of(FIRST_CELL, "alpha", grant, t(3600))?
        .signature()
        .to_string();
    let over_second_signature = envelope_of(SECOND_CELL, "alpha", grant, t(3600))?
        .signature()
        .to_string();
    over_first.apply_policy(
        share_policy(FIRST_CELL, 1, t(10), Some(vec![over_first_signature]))?,
        t(10),
    )?;
    over_second.apply_policy(
        share_policy(SECOND_CELL, 1, t(10), Some(vec![over_second_signature]))?,
        t(10),
    )?;
    let mut gateway = VenueGateway::default();
    assert_eq!(
        over_first.work(t(50), &mut gateway)?.orders.len(),
        1,
        "the contrast premise failed: the first cell cannot send even under a share of the whole grant"
    );
    assert_eq!(
        over_second.work(t(50), &mut gateway)?.orders.len(),
        1,
        "the contrast premise failed: the second cell cannot send even under a share of the whole grant"
    );

    // The property: under disjoint shares, what the two cells send together
    // is at most the grant.
    let first_report = first.work(t(50), &mut gateway)?;
    assert_eq!(
        first_report.orders.len(),
        1,
        "the first cell did not send within its share: {:?}",
        first_report.refusals
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
        "the second cell sent against a region whose grant the first cell's share had \
         exhausted: {:?}",
        second_report.orders
    );
    assert_eq!(
        refused_under(&second_report, GATE).len(),
        1,
        "the second cell was not refused exactly once under `{GATE}`: {:?}",
        second_report.refusals
    );
    let sent_together = first_table.committed_total() + second_table.committed_total();
    assert_eq!(
        sent_together, grant,
        "the two cells together committed more than the grant"
    );
    Ok(())
}

#[test]
fn a_replayed_lower_sequence_cannot_widen_a_cells_share() -> Result<()> {
    // The un-widenable property: a wide share under sequence 5, narrowed to
    // nothing by sequence 6, and sequence 5 played again. Without the
    // discipline a captured payload re-widens a cell the centre has just
    // narrowed, which is the replay ADR 0008 exists to make impossible.
    let grant = one_pass_holds(PricingPolicy::Marketable)?;
    let envelope = envelope_of(FIRST_CELL, "alpha", grant, t(3600))?;
    let signature = envelope.signature().to_string();
    let (mut cell, table) = unfunded_cell(FIRST_CELL, envelope)?;
    let wide = share_policy(FIRST_CELL, 5, t(10), Some(vec![signature]))?;
    cell.apply_policy(wide.clone(), t(10))?;
    assert_eq!(
        cell.region_allocation_bound(),
        Some(grant),
        "the premise failed: sequence 5 did not widen the table"
    );
    cell.apply_policy(share_policy(FIRST_CELL, 6, t(20), Some(vec![]))?, t(20))?;
    assert_eq!(
        cell.region_allocation_bound(),
        Some(Decimal::ZERO),
        "the premise failed: sequence 6 did not narrow the table"
    );

    let refused = cell.apply_policy(wide, t(30));
    assert!(refused.is_err(), "the replayed sequence 5 was applied");
    assert_eq!(
        cell.region_allocation_bound(),
        Some(Decimal::ZERO),
        "the replayed payload re-widened the cell's share"
    );
    assert_eq!(cell.region_share_sequence(), Some(6));
    assert_eq!(table.share_sequence(), Some(6));
    let mut gateway = VenueGateway::default();
    let report = cell.work(t(50), &mut gateway)?;
    assert!(
        report.orders.is_empty(),
        "the cell sent after the replay: {:?}",
        report.orders
    );
    assert_eq!(
        refused_under(&report, GATE).len(),
        1,
        "{:?}",
        report.refusals
    );
    Ok(())
}

#[test]
fn a_cell_absent_from_the_shares_books_nothing() -> Result<()> {
    let grant = one_pass_holds(PricingPolicy::Marketable)?;
    // Contrast first: the same cell, named in the shares, sends.
    let named = envelope_of(FIRST_CELL, "alpha", grant, t(3600))?;
    let named_signature = named.signature().to_string();
    let (mut funded, _) = unfunded_cell(FIRST_CELL, named)?;
    funded.apply_policy(
        share_policy(FIRST_CELL, 1, t(10), Some(vec![named_signature]))?,
        t(10),
    )?;
    let mut gateway = VenueGateway::default();
    assert_eq!(
        funded.work(t(50), &mut gateway)?.orders.len(),
        1,
        "the contrast premise failed: a cell named in the shares cannot send"
    );

    // Never granted to at all: an unfunded table and no payload.
    let (mut never, _) = unfunded_cell(
        FIRST_CELL,
        envelope_of(FIRST_CELL, "alpha", grant, t(3600))?,
    )?;
    let report = never.work(t(50), &mut gateway)?;
    assert_eq!(
        report.signals.len(),
        1,
        "the premise failed: the strategy did not propose: {:?}",
        report.refusals
    );
    assert!(
        report.orders.is_empty(),
        "a cell nobody granted to sent: {:?}",
        report.orders
    );
    assert_eq!(
        refused_under(&report, GATE).len(),
        1,
        "{:?}",
        report.refusals
    );

    // Granted to, but the manifest names a sibling's grant and none of this
    // cell's: a share of nothing.
    let (mut absent, table) = unfunded_cell(
        FIRST_CELL,
        envelope_of(FIRST_CELL, "alpha", grant, t(3600))?,
    )?;
    let siblings = envelope_of(SECOND_CELL, "alpha", grant, t(3600))?
        .signature()
        .to_string();
    absent.apply_policy(
        share_policy(FIRST_CELL, 1, t(10), Some(vec![siblings]))?,
        t(10),
    )?;
    assert_eq!(absent.region_allocation_bound(), Some(Decimal::ZERO));
    let report = absent.work(t(50), &mut gateway)?;
    assert!(
        report.orders.is_empty(),
        "a cell absent from the shares sent: {:?}",
        report.orders
    );
    assert_eq!(
        refused_under(&report, GATE).len(),
        1,
        "{:?}",
        report.refusals
    );
    assert_eq!(table.committed_total(), Decimal::ZERO);
    let entries = share_entries(&absent);
    assert_eq!(entries.len(), 1, "the share was not journaled exactly once");
    assert!(
        matches!(entries[0], Decision::RegionShareApplied { grants: 0, .. }),
        "the journal did not record that no grant of this cell's was named: {:?}",
        entries[0]
    );
    Ok(())
}

#[test]
fn a_share_below_what_the_cell_already_committed_narrows_free_to_zero_and_journals_the_deficit()
-> Result<()> {
    let grant = one_pass_holds(PricingPolicy::Marketable)?;
    let envelope = envelope_of(FIRST_CELL, "alpha", grant, t(3600))?;
    let signature = envelope.signature().to_string();
    let (mut cell, table) = unfunded_cell(FIRST_CELL, envelope)?;
    cell.apply_policy(
        share_policy(FIRST_CELL, 1, t(10), Some(vec![signature]))?,
        t(10),
    )?;
    let mut gateway = VenueGateway::default();
    assert_eq!(
        cell.work(t(50), &mut gateway)?.orders.len(),
        1,
        "the premise failed: the cell did not send"
    );
    assert_eq!(
        table.committed_total(),
        grant,
        "the premise failed: the order did not commit the share"
    );

    // The centre narrows the cell to nothing while its order is out.
    cell.apply_policy(share_policy(FIRST_CELL, 2, t(60), Some(vec![]))?, t(60))?;
    assert_eq!(
        table.free(),
        Decimal::ZERO,
        "free went somewhere other than zero under a deficit"
    );
    assert!(!table.free().is_negative());
    assert_eq!(table.committed_total(), grant, "narrowing un-sent an order");
    let entries = share_entries(&cell);
    assert_eq!(entries.len(), 2);
    match entries[1] {
        Decision::RegionShareApplied { deficit, free, .. } => {
            assert_eq!(
                deficit,
                &grant.to_string(),
                "the deficit was not journaled as the shortfall"
            );
            assert_eq!(free, "0");
        }
        other => panic!("not a share entry: {other:?}"),
    }
    Ok(())
}

#[test]
fn a_partitioned_cell_keeps_spending_within_its_last_share_until_its_envelopes_expire() -> Result<()>
{
    // The ADR 0008 conformance test. One payload, then silence: the cell
    // sends within its share, refuses past it, keeps the share when the
    // slot goes stale, and stops only when its envelope expires on its own
    // clock. A second expiry on the share would double the partition
    // failure mode without bounding anything the envelope does not.
    let one = one_pass_holds(PricingPolicy::Marketable)?;
    // The envelope admits three orders; the operator's ceiling bounds the
    // share at two, so the region gate — not the envelope — is what refuses
    // the third. The share the centre named is the envelope's gross; the
    // bound is `min(share, ceiling)`.
    let share = one + one;
    let envelope = envelope_of(FIRST_CELL, "alpha", one + one + one, t(7200))?;
    let signature = envelope.signature().to_string();
    let (mut cell, table) = unfunded_cell_under(FIRST_CELL, envelope, share)?;
    cell.apply_policy(
        share_policy(FIRST_CELL, 1, t(10), Some(vec![signature]))?,
        t(10),
    )?;
    assert_eq!(
        cell.region_allocation_bound(),
        Some(share),
        "the premise failed: the share was not applied"
    );
    let mut gateway = VenueGateway::default();
    assert_eq!(
        cell.work(t(50), &mut gateway)?.orders.len(),
        1,
        "the first pass did not send"
    );
    // The grant manifest's slot is stale past an hour; the envelope is not.
    let stale = cell.work(t(3700), &mut gateway)?;
    assert_eq!(
        stale.orders.len(),
        1,
        "the cell stopped spending within its share when the slot went stale: {:?}",
        stale.refusals
    );
    // Within the share, not necessarily to the unit: the envelope's own
    // utilisation may trim the second order, which is the envelope bounding
    // and not the share.
    assert!(
        table.committed_total() > one && table.committed_total() <= share,
        "two passes committed {} against a share of {share}",
        table.committed_total()
    );
    let past = cell.work(t(3800), &mut gateway)?;
    assert!(
        past.orders.is_empty(),
        "the cell sent past its share: {:?}",
        past.orders
    );
    assert_eq!(refused_under(&past, GATE).len(), 1, "{:?}", past.refusals);
    assert_eq!(
        cell.region_allocation_bound(),
        Some(share),
        "the share was zeroed on staleness"
    );
    // The envelope's own clock is what stops the cell.
    let expired = cell.work(t(7300), &mut gateway)?;
    assert!(expired.orders.is_empty());
    assert!(
        !refused_under(&expired, "envelope_expiry").is_empty(),
        "the expired envelope was not what refused: {:?}",
        expired.refusals
    );
    Ok(())
}
