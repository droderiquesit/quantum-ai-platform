//! The bound on what one cell may commit in total, driven through the cell.
//!
//! Before this existed every capital bound the cell held was per *strategy* —
//! one signed envelope each — and nothing summed them. Four deployed
//! strategies gave a cell four envelopes' worth of authority bounded by no
//! single number, and seven cells deciding alone while partitioned could each
//! spend to their own total against a budget the centre had promised once.
//!
//! Every test here runs a real pass. None asserts that a ledger method
//! returns what it was told: the unit tests in `src/reservation.rs` do that,
//! and a ledger nothing calls would pass them forever.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::reservation::RegionTable;
use qip_edge::telemetry::{EDGE_REGION_ALLOCATION_CONFIGURED, EDGE_REGION_ALLOCATION_FREE};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Labels, Metrics, labels};
use qip_orderbook::venue::VenueState;
use qip_risk_engine::autonomy::{AutonomyLevel, OperatorIdentity};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const SYMBOL: &str = "ACME";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
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

/// A strategy whose one rule always holds, so a pass raises exactly the
/// signal kind under test.
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

#[derive(Debug, Default)]
struct RecordingGateway {
    placed: Vec<(BookSide, Decimal)>,
}

impl Placer for RecordingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push((side, quantity));
        Ok(())
    }
}

/// A cell holding the book, the given strategies and — when one is asked for
/// — a region allocation.
fn trading_cell(
    strategies: &[(&str, SignalKind, &str)],
    allocation: Option<Decimal>,
    metrics: Option<Arc<Metrics>>,
) -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    if let Some(metrics) = metrics {
        cell = cell.with_metrics(metrics);
    }
    if let Some(amount) = allocation {
        cell = cell.with_region_allocation(amount)?;
    }
    cell.track(book()?);
    for (id, kind, size) in strategies {
        let (compiled, program) = firing_strategy(id, *kind, size)?;
        cell.deploy_with_pricing(
            compiled,
            program,
            signed_envelope(id)?,
            PricingPolicy::Marketable,
        )?;
    }
    Ok(cell)
}

/// What one pass of `strategies` actually holds against the region.
///
/// Measured by running the pass against an allocation far larger than it
/// could need, rather than restated as a literal: the degradation floor
/// scales every size, so a literal here would be a number this file believed
/// and the cell did not.
fn held_by(strategies: &[(&str, SignalKind, &str)]) -> Result<Decimal> {
    let opening = dec!("100000000");
    let mut cell = trading_cell(strategies, Some(opening), None)?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        report.orders.len(),
        1,
        "the probe premise failed: the pass placed no order: {:?}",
        report.refusals
    );
    let free = cell
        .region_allocation_free()
        .expect("the probe cell was given an allocation");
    Ok(opening - free)
}

fn refused_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        // Delimited equality, not `contains`: `region_reservation_abandoned`
        // has `region_reservation` as a prefix, and a substring match would
        // read the backstop's journal entry as the gate firing.
        .filter(|(recorded, _)| recorded == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

#[test]
fn a_cell_given_no_region_allocation_sends_exactly_what_it_sent_before() -> Result<()> {
    // The failure this prevents is the slice itself: a bound whose absent
    // case defaulted to zero would silently stop every deployed cell, since
    // no composition root hands one over yet.
    let mut cell = trading_cell(&[("alpha", SignalKind::Enter, "100")], None, None)?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;

    assert_eq!(
        report.signals.len(),
        1,
        "the premise failed: the strategy did not fire: {:?}",
        report.refusals
    );
    assert_eq!(
        report.orders.len(),
        1,
        "a cell with no region allocation stopped sending: {:?}",
        report.refusals
    );
    assert_eq!(gateway.placed.len(), 1);
    assert!(
        refused_under(&report, "region_reservation").is_empty(),
        "a cell with no allocation refused at a gate it does not hold"
    );
    assert_eq!(
        cell.region_allocation_free(),
        None,
        "a cell with no allocation reported a balance, conflating none with nothing left"
    );
    Ok(())
}

#[test]
fn a_second_strategy_is_refused_once_the_region_allocation_is_spent_even_though_its_own_envelope_would_admit_it()
-> Result<()> {
    // The headline. Both strategies hold a signed envelope of a million, so
    // nothing per-strategy stops the second: only the cell-wide bound does.
    let one = held_by(&[("alpha", SignalKind::Enter, "100")])?;
    let mut cell = trading_cell(
        &[
            ("alpha", SignalKind::Enter, "100"),
            ("beta", SignalKind::Enter, "100"),
        ],
        Some(one),
        None,
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;

    // Premise: both strategies fired. Without this the test would pass
    // against a cell where beta never raised a signal at all, which is a
    // different fact entirely.
    assert_eq!(
        report.signals.len(),
        2,
        "the premise failed: both strategies must fire for the region gate to be what stops \
         the second: {:?}",
        report.refusals
    );

    assert_eq!(
        report.orders.len(),
        1,
        "the region allocation admitted more than it holds: {:?}",
        report.orders
    );
    let order = &report.orders[0];
    assert_eq!(
        order.contributors.len(),
        1,
        "the second strategy contributed to the net despite the region being spent: {:?}",
        order.contributors
    );
    assert_eq!(order.contributors[0].strategy.as_str(), "alpha");

    let refusals = refused_under(&report, "region_reservation");
    assert_eq!(
        refusals.len(),
        1,
        "the region gate did not refuse exactly once: {:?}",
        report.refusals
    );
    // The refusal names what was asked for and what was left, because an
    // operator reading the journal needs to know how short the region was
    // and not merely that something was short. Matched on the phrase around
    // the number: `one.to_string()` on its own is a digit string that would
    // appear in almost any message.
    assert!(
        refusals[0].contains(&format!("holding {one} for ")),
        "the refusal did not name the notional it turned away: {}",
        refusals[0]
    );
    assert!(
        refusals[0].contains("the 0 the region allocation has left"),
        "the refusal did not name what the allocation had left: {}",
        refusals[0]
    );
    Ok(())
}

#[test]
fn a_net_that_cancels_to_zero_returns_its_holds_to_the_region_allocation() -> Result<()> {
    let opening = dec!("100000000");
    let mut cell = trading_cell(
        &[
            ("alpha", SignalKind::Enter, "100"),
            ("beta", SignalKind::Exit, "100"),
        ],
        Some(opening),
        None,
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;

    // Premise: the two intents did cancel, and nothing went out.
    assert_eq!(
        report.signals.len(),
        2,
        "the premise failed: both strategies must fire: {:?}",
        report.refusals
    );
    assert_eq!(
        report.cancelled.len(),
        1,
        "the premise failed: the two intents did not cancel: {:?}",
        report.refusals
    );
    assert!(report.orders.is_empty(), "{:?}", report.orders);

    assert_eq!(
        cell.region_allocation_free(),
        Some(opening),
        "a net that reached no venue kept the region's capital for the pass"
    );
    Ok(())
}

#[test]
fn an_intent_the_feasibility_gate_drops_returns_its_region_hold() -> Result<()> {
    // Four hundred rest on the ask. Two thousand, scaled by the degradation
    // floor, is still well over four hundred and well inside the envelope's
    // hundred-thousand order limit, so the envelope takes it whole and the
    // depth rule is what refuses it. This asked for a hundred thousand until
    // the envelope stopped reducing an order to fit (CAPITAL-025): that size
    // only reached the depth gate because the envelope cut it down first.
    let opening = dec!("100000000");
    let mut cell = trading_cell(&[("alpha", SignalKind::Enter, "2000")], Some(opening), None)?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;

    // Premise: the pass reached the feasibility gate and that gate is what
    // refused — not the envelope, and not the region bound, which is large.
    assert_eq!(
        report.signals.len(),
        1,
        "the premise failed: the strategy did not fire: {:?}",
        report.refusals
    );
    assert!(
        refused_under(&report, "capital").is_empty(),
        "the premise failed: the envelope, not the depth gate, refused: {:?}",
        report.refusals
    );
    assert!(
        !refused_under(&report, "feasibility_depth").is_empty(),
        "the premise failed: the depth gate did not refuse: {:?}",
        report.refusals
    );
    assert!(report.orders.is_empty());

    assert_eq!(
        cell.region_allocation_free(),
        Some(opening),
        "an intent the feasibility gate dropped kept its region hold, so a cell whose \
         strategies are all infeasible would exhaust its region and send nothing"
    );
    Ok(())
}

#[test]
fn an_order_the_cell_sent_spends_the_region_allocation_and_does_not_get_it_back() -> Result<()> {
    let opening = dec!("100000000");
    let mut cell = trading_cell(&[("alpha", SignalKind::Enter, "100")], Some(opening), None)?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        report.orders.len(),
        1,
        "the premise failed: no order was sent: {:?}",
        report.refusals
    );

    let after_send = cell
        .region_allocation_free()
        .expect("the cell was given an allocation");
    assert!(
        after_send < opening,
        "an order that reached the venue spent none of the region's capital"
    );

    // And the next pass's sweep does not undo the commit: a committed hold is
    // gone, not merely out of scope.
    let second = cell.work(t(60), &mut gateway)?;
    assert!(
        cell.region_allocation_free().expect("still allocated") < after_send,
        "the second pass returned capital the first had already spent: {second:?}"
    );
    Ok(())
}

#[test]
fn a_cell_at_observation_holds_none_of_the_region_allocation() -> Result<()> {
    // The hold sits *after* the autonomy gate, so a cell that sends nothing
    // takes nothing. Put before it, an observing cell would pin its region's
    // whole budget every pass for orders it was never going to send.
    let opening = dec!("100000000");
    let mut cell = trading_cell(&[("alpha", SignalKind::Enter, "100")], Some(opening), None)?;
    let operator = OperatorIdentity::verified("alice@example.com", "oidc", t(40));
    cell.autonomy_mut().request_change(
        AutonomyLevel::Observation,
        &operator,
        "a drill that proves an observing cell reserves nothing",
        None,
        t(41),
    )?;

    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;

    // Premise: the autonomy gate is what refused.
    assert!(
        !refused_under(&report, "autonomy").is_empty(),
        "the premise failed: the cell was not at observation: {:?}",
        report.refusals
    );
    assert!(report.orders.is_empty());
    assert_eq!(
        cell.region_allocation_free(),
        Some(opening),
        "an observing cell held region capital against an order it never sends"
    );
    Ok(())
}

#[test]
fn the_region_allocation_series_report_what_the_cell_has_left() -> Result<()> {
    let opening = dec!("100000000");
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut cell = trading_cell(
        &[("alpha", SignalKind::Enter, "100")],
        Some(opening),
        Some(Arc::clone(&metrics)),
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        report.orders.len(),
        1,
        "the premise failed: no order was sent, so the balance would not have moved: {:?}",
        report.refusals
    );

    let free = cell
        .region_allocation_free()
        .expect("the cell was given an allocation");
    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.gauge(EDGE_REGION_ALLOCATION_CONFIGURED, &base()),
        Some(1.0),
        "a cell holding an allocation did not say so"
    );
    // The gauge is published at the top of the pass and the pass then spent
    // some of it, so the series carries what the *previous* pass left. A
    // second pass makes the published number the post-send balance, which is
    // the fact an operator reads.
    cell.work(t(60), &mut gateway)?;
    assert_eq!(
        metrics
            .snapshot()
            .gauge(EDGE_REGION_ALLOCATION_FREE, &base()),
        Some(free.to_f64()),
        "the free-balance series is not what the allocation has left"
    );
    Ok(())
}

#[test]
fn a_cell_holding_no_region_allocation_says_so_in_its_series() -> Result<()> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut cell = trading_cell(
        &[("alpha", SignalKind::Enter, "100")],
        None,
        Some(Arc::clone(&metrics)),
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;
    // Premise: the pass ran. A cell that never ran publishes nothing either.
    assert_eq!(
        report.orders.len(),
        1,
        "the premise failed: the pass placed no order: {:?}",
        report.refusals
    );

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.gauge(EDGE_REGION_ALLOCATION_CONFIGURED, &base()),
        Some(0.0),
        "a cell with no allocation read as one that has it"
    );
    assert_eq!(
        snapshot.gauge(EDGE_REGION_ALLOCATION_FREE, &base()),
        None,
        "a cell with no allocation published a free balance, which reads on a chart as a cell \
         that has spent everything"
    );
    Ok(())
}

#[test]
fn a_marketable_order_holds_of_the_region_exactly_the_notional_it_is_sent_at() -> Result<()> {
    // Round 2 found the region hold and the envelope admission charged at the
    // mid while a marketable order went out at the touch, which the Risk Gate
    // measures. A buy therefore committed half a spread more than the region
    // held for it, and an allocation sized to its holds was overspent by the
    // sum of those half-spreads. Both sides are driven: a buy takes the ask
    // above the mid, a sale the bid below it, so a hold at the mid is wrong in
    // opposite directions and a fix right for only one side still fails.
    for (kind, touch) in [
        (SignalKind::Enter, dec!("101")),
        (SignalKind::Exit, dec!("99")),
    ] {
        let opening = dec!("100000000");
        let mut cell = trading_cell(&[("alpha", kind, "100")], Some(opening), None)?;
        let mut gateway = RecordingGateway::default();
        let report = cell.work(t(50), &mut gateway)?;
        assert_eq!(
            report.orders.len(),
            1,
            "the premise failed: the {kind:?} pass placed no order: {:?}",
            report.refusals
        );
        let order = &report.orders[0];
        // Premise: the order went out at the touch, and the touch is not the
        // mid. Against a book whose touch were the mid, a hold priced at the
        // mid would agree with the order and this test would prove nothing.
        assert_eq!(
            order.price, touch,
            "the {kind:?} order did not go out at the touch"
        );
        assert_ne!(
            touch,
            dec!("100"),
            "the premise failed: the touch is the mid"
        );

        let held = opening
            - cell
                .region_allocation_free()
                .expect("the cell was given an allocation");
        assert_eq!(
            held,
            order.quantity * order.price,
            "the {kind:?} order held a different notional of the region from the one it was \
             sent at"
        );
    }
    Ok(())
}

/// What a region table held at the instant an order reached the gateway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AtPlacement {
    /// The notional of the order arriving.
    notional: Decimal,
    free: Decimal,
    held: Decimal,
    committed: Decimal,
}

/// A gateway that reads the cell's region table at the instant an order
/// reaches it.
///
/// That instant is the first at which an order object exists. `Cell::send`
/// numbers the order and hands it to the placer in one step, and until then
/// the cell holds a price and a quantity and nothing more (the comment above
/// the quote-budget gate in `Cell::place_net` says so). A hold this gateway
/// sees existed before the order did.
#[derive(Debug)]
struct TableReadingGateway {
    table: RegionTable,
    seen: Vec<AtPlacement>,
}

impl TableReadingGateway {
    fn new(table: &RegionTable) -> Self {
        Self {
            table: table.clone(),
            seen: Vec::new(),
        }
    }
}

impl Placer for TableReadingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.seen.push(AtPlacement {
            notional: quantity * price,
            free: self.table.free(),
            held: self.table.held_total(),
            committed: self.table.committed_total(),
        });
        Ok(())
    }
}

/// A cell built as [`trading_cell`] builds one, holding against `table`: a
/// handle the test keeps as well, so the table can be read from outside the
/// cell while the cell works.
fn cell_on_table(strategies: &[(&str, SignalKind, &str)], table: &RegionTable) -> Result<Cell> {
    Ok(trading_cell(strategies, None, None)?.with_region_table(table.clone()))
}

/// RM-P3-03: the capital an order commits is reserved before the order
/// object exists, and an order whose reservation cannot be taken is never
/// built.
///
/// The failure this prevents is a limit applied after the fact. A region
/// bound taken once the order is already numbered and on its way, or
/// checked against the balance without holding it, leaves every end-of-pass
/// balance the tests above assert exactly as it should be, and is still a
/// race: a second order built in that window is checked against capital the
/// first is about to spend. `src/reservation.rs` states the discipline,
/// "held before an order exists, refused — never reduced — when the hold
/// cannot be taken", and until this test nothing observed the "before".
/// Here the gateway shares the cell's region table and reads it at the
/// instant the order arrives.
///
/// Mutations: in `Cell`, take the hold after `send` instead of before the
/// intent (`hold_region_capital` checks the balance without holding, and
/// `place_net` reserves and commits after the send, so the end-of-pass
/// balance is unchanged) and the placement assertion fires; in
/// `RegionAllocation::reserve`, reduce a hold to what the region has left
/// instead of refusing it, and the refusal half fires.
#[test]
fn capital_is_reserved_before_an_order_object_exists() -> Result<()> {
    let strategies = [("alpha", SignalKind::Enter, "100")];

    // The first half: a region with room for the order.
    let opening = dec!("100000000");
    let table = RegionTable::new(opening)?;
    let mut cell = cell_on_table(&strategies, &table)?;
    let mut gateway = TableReadingGateway::new(&table);
    // Premise: nothing is held or spent before the pass.
    assert_eq!(
        (table.free(), table.held_total(), table.committed_total()),
        (opening, Decimal::ZERO, Decimal::ZERO),
        "premise: the table was not fresh before the pass"
    );
    let report = cell.work(t(50), &mut gateway)?;
    // Premise: one order went out, and the order the gateway saw is the one
    // the report names.
    assert_eq!(
        report.orders.len(),
        1,
        "premise: the pass placed no order: {:?}",
        report.refusals
    );
    assert_eq!(
        gateway.seen.len(),
        1,
        "premise: the gateway saw {} placements for one order",
        gateway.seen.len()
    );
    let order = &report.orders[0];
    let notional = order.quantity * order.price;
    let at_placement = gateway.seen[0];
    assert!(
        notional.is_positive(),
        "premise: the order carries no notional to reserve"
    );
    assert_eq!(
        at_placement.notional, notional,
        "premise: the gateway saw a different order from the one the report names"
    );
    // The property: when the order came into existence its whole notional
    // was already out of the region's free balance, held or committed, and
    // nothing else was.
    assert_eq!(
        opening - at_placement.free,
        notional,
        "when the order reached the gateway the region had not yet given up its notional, so \
         the hold was taken after the order existed or not at all: {at_placement:?}"
    );
    assert_eq!(
        at_placement.held + at_placement.committed,
        notional,
        "when the order reached the gateway the table held something other than its \
         notional: {at_placement:?}"
    );

    // The second half: one unit less than that notional. The hold cannot be
    // taken, so the order is never built: nothing reaches the gateway, not
    // even a smaller order, and none of the region's capital moves.
    let short = notional - dec!("1");
    let table = RegionTable::new(short)?;
    let mut cell = cell_on_table(&strategies, &table)?;
    let mut gateway = TableReadingGateway::new(&table);
    let report = cell.work(t(50), &mut gateway)?;
    // Premise: the strategy fired and its envelope admitted it, so the region
    // hold is the only thing left that could stop it.
    assert_eq!(
        report.signals.len(),
        1,
        "premise: the strategy did not fire: {:?}",
        report.refusals
    );
    assert!(
        refused_under(&report, "capital").is_empty(),
        "premise: the envelope refused, not the region: {:?}",
        report.refusals
    );
    assert!(
        gateway.seen.is_empty(),
        "an order reached the gateway although its region hold could not be taken: {:?}",
        gateway.seen
    );
    assert!(
        report.orders.is_empty(),
        "the report names an order whose region hold could not be taken: {:?}",
        report.orders
    );
    assert_eq!(
        refused_under(&report, "region_reservation").len(),
        1,
        "the region gate did not refuse exactly once: {:?}",
        report.refusals
    );
    assert_eq!(
        (table.free(), table.held_total(), table.committed_total()),
        (short, Decimal::ZERO, Decimal::ZERO),
        "a hold that could not be taken still moved the region's capital"
    );
    Ok(())
}
