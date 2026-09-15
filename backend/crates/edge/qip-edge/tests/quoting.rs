//! §29.2's quote-rate management, driven through the cell.
//!
//! Three controls meet here and they are not independent. The token bucket
//! stops the cell producing more messages than a venue session will take; the
//! withdrawal reserve keeps part of that budget for cancels, so the cell can
//! always stop being exposed even after it has quoted its rate away; and the
//! mass cancel is what spends that reserve when a halt takes the cell out of
//! the market.
//!
//! Every test drives a `Cell` through the event rather than calling the
//! budget directly — the budget's own arithmetic is unit-tested beside it.
//! What is under test here is that the control is *reached*: a limit checked
//! by nothing is the failure mode this repository already shipped once.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{
    Cell, CellConfig, ExecutionReport, GATE_MASS_CANCEL, GATE_QUOTE_BUDGET, Placer, PricingPolicy,
    WorkReport,
};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::quoting::{MessageKind, RateLimits};
use qip_edge::telemetry::{
    EDGE_MESSAGES_SENT, EDGE_ORDERS_EXPIRED, EDGE_ORDERS_MASS_CANCELLED, EDGE_QUOTE_BUDGET_TOKENS,
    EDGE_QUOTE_NARROWED,
};
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

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn base() -> Labels {
    labels([("cell", CELL), ("region", REGION)])
}

fn by(key: &str, value: &str) -> Labels {
    labels([("cell", CELL), ("region", REGION), (key, value)])
}

fn message_labels(kind: MessageKind) -> Labels {
    labels([
        ("cell", CELL),
        ("kind", kind.as_str()),
        ("region", REGION),
        ("venue", VENUE),
    ])
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

/// A two-sided book with a mid of 100, built from messages because there is
/// deliberately no setter that bypasses the feed.
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

/// A strategy whose one rule always holds, so every pass raises a signal and
/// what stops an order is a gate rather than a quiet market.
fn firing_strategy(id: &str, size: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            SignalKind::Enter,
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

/// A cell wired the way `qip-edge-node` wires it, under the given limits and
/// pricing.
fn cell_under(limits: RateLimits, pricing: PricingPolicy) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut config = CellConfig::new(CELL, REGION).with_venue(venue());
    config.quote_limits = limits;
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));
    cell.track(book()?);
    let (compiled, program) = firing_strategy("alpha", "10")?;
    cell.deploy_with_pricing(compiled, program, signed_envelope("alpha")?, pricing)?;
    Ok((cell, metrics))
}

/// A venue that accepts everything, never reports a fill, and can withdraw.
///
/// It reports no fill on purpose: an order that rests is what the mass cancel
/// has to withdraw, and a gateway that filled everything would leave nothing
/// for the control under test to do.
#[derive(Debug, Default)]
struct RestingVenue {
    placed: Vec<String>,
    cancelled: Vec<String>,
    /// The quantity each accepted order is still open for.
    open: Vec<(String, Decimal)>,
}

impl Placer for RestingVenue {
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
        self.placed.push(order_id.to_string());
        self.open.push((order_id.to_string(), quantity));
        Ok(())
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
        self.cancelled.push(order_id.to_string());
        Ok(self
            .open
            .iter()
            .find(|(id, _)| id == order_id)
            .map_or(Decimal::ZERO, |(_, quantity)| *quantity))
    }
}

/// The same venue without a cancel path, which is what a gateway that has no
/// way to withdraw an order must say about itself.
#[derive(Debug, Default)]
struct OneWayVenue {
    placed: Vec<String>,
}

impl Placer for OneWayVenue {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push(order_id.to_string());
        Ok(())
    }
}

fn refusals_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

fn journal_kinds(cell: &Cell) -> Vec<&'static str> {
    cell.journal()
        .entries()
        .iter()
        .map(|entry| entry.decision.kind())
        .collect()
}

/// Limits with exactly `spendable` placements in them and `reserve` messages
/// kept back for cancels, refilling too slowly for a test's passes to matter.
///
/// The narrowed floor is the withdrawal reserve, so narrowing changes nothing
/// here: these tests are about the bucket and the reserve, and the
/// message-to-trade monitor is proven on its own arithmetic beside it. A
/// fixture that let narrowing move the floor would leave every refusal below
/// attributable to either control.
fn limits(spendable: u32, reserve: u32) -> Result<RateLimits> {
    RateLimits::new(spendable + reserve, 1, reserve, reserve, 4, 64)
}

#[test]
fn a_net_is_refused_before_it_reaches_the_venue_once_the_quote_budget_is_spent() -> Result<()> {
    // The failure this prevents: producing messages until the venue's own
    // limiter throttles or disconnects the session, which happens at a moment
    // nobody chose and leaves resting orders the cell can no longer withdraw.
    // One placement in the budget, so the second pass is the control firing.
    let (mut cell, metrics) = cell_under(limits(1, 0)?, PricingPolicy::Marketable)?;
    let mut gateway = RestingVenue::default();

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        1,
        "the premise failed: the first pass sent nothing, so nothing spent the budget: {:?}",
        first.refusals
    );
    assert!(
        refusals_under(&first, GATE_QUOTE_BUDGET).is_empty(),
        "the first pass was refused although the budget held a message for it: {:?}",
        first.refusals
    );

    // The same instant, so nothing refills: the bucket is the only thing that
    // has changed between the two passes.
    let second = cell.work(t(10), &mut gateway)?;
    let refused = refusals_under(&second, GATE_QUOTE_BUDGET);
    assert_eq!(
        refused.len(),
        1,
        "a net was formed with no message budget to send it: {:?}",
        second.refusals
    );
    assert!(
        refused[0].contains(VENUE),
        "the refusal did not name the venue whose budget was spent: {}",
        refused[0]
    );
    assert_eq!(
        second.orders.len(),
        0,
        "the refused net reached the venue anyway"
    );
    assert_eq!(
        gateway.placed.len(),
        1,
        "the gateway was called after the budget refused"
    );

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::EDGE_REFUSALS, &by("gate", GATE_QUOTE_BUDGET)),
        1,
        "the budget refusal did not reach the refusal series"
    );
    assert_eq!(
        snapshot.counter(EDGE_MESSAGES_SENT, &message_labels(MessageKind::Placement)),
        1,
        "the one message that was sent was not counted as a placement"
    );
    assert_eq!(
        snapshot.gauge(EDGE_QUOTE_BUDGET_TOKENS, &by("venue", VENUE)),
        Some(0.0),
        "the spent budget was not published, so an operator cannot see why the cell went quiet"
    );
    Ok(())
}

#[test]
fn a_cell_that_has_sent_nothing_publishes_a_full_budget_rather_than_no_series() -> Result<()> {
    // The silent-when-idle failure. A budget that is only published when it is
    // spent reaches no chart in the state a deployment is almost always in,
    // and an absent series reads as good news on every dashboard ever built.
    // A halted cell publishes it too: the gauge is written before the halt
    // check, like the region allocation beside it.
    let (mut cell, metrics) = cell_under(limits(4, 2)?, PricingPolicy::Marketable)?;
    let mut gateway = RestingVenue::default();

    cell.autonomy_mut()
        .kill_switch_mut()
        .trip_global(t(9), "drill", "an idle-state measurement");
    assert!(cell.is_halted(), "the premise is a halted cell");

    let report = cell.work(t(10), &mut gateway)?;
    assert!(report.halted, "the premise failed: the pass was not halted");
    assert!(
        gateway.placed.is_empty(),
        "the premise failed: a halted cell sent"
    );

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.gauge(EDGE_QUOTE_BUDGET_TOKENS, &by("venue", VENUE)),
        Some(6.0),
        "a cell that has sent nothing published no budget at all, so a full bucket and a spent \
         one are the same picture"
    );
    assert_eq!(
        snapshot.gauge(EDGE_QUOTE_NARROWED, &by("venue", VENUE)),
        Some(0.0),
        "the message-to-trade monitor published nothing, so a narrowed venue and an idle one are \
         indistinguishable"
    );
    assert_eq!(
        snapshot.counter(EDGE_MESSAGES_SENT, &message_labels(MessageKind::Placement)),
        0,
        "the premise failed: a message was counted on a pass that sent none"
    );
    Ok(())
}

#[test]
fn a_halted_cell_withdraws_every_order_it_had_resting_and_charts_it_as_a_mass_cancel() -> Result<()>
{
    // The failure this prevents: a kill switch that stops the cell adding
    // exposure and leaves the exposure it already has resting at a venue the
    // cell has stopped watching. §29.2 wires the mass cancel to the halt, and
    // `qip-edge-node` reaches it through `withdraw_expired` — the call it
    // makes on every pass, halted or not, before `Cell::work` is reached at
    // all.
    let resting = PricingPolicy::rest_at_mid(Duration::from_secs(600))?;
    let (mut cell, metrics) = cell_under(limits(4, 2)?, resting)?;
    let mut gateway = RestingVenue::default();

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        1,
        "the premise failed: nothing was left resting to withdraw: {:?}",
        first.refusals
    );
    assert!(
        cell.open_orders()
            .iter()
            .all(|order| order.closed.is_none()),
        "the premise failed: the order did not rest"
    );

    cell.autonomy_mut().kill_switch_mut().trip_global(
        t(11),
        "drill",
        "a halt with an order resting",
    );
    assert!(cell.is_halted(), "the premise is a halted cell");

    // Long before the order's ten-minute time to live: the halt is the reason,
    // and nothing else could be.
    let withdrawn = cell.withdraw_expired(&mut gateway, t(12));
    assert_eq!(
        withdrawn.len(),
        1,
        "the halted cell left its resting order at the venue"
    );
    assert_eq!(
        gateway.cancelled, gateway.placed,
        "the venue was not asked to withdraw the order the cell had sent"
    );

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(EDGE_ORDERS_MASS_CANCELLED, &by("venue", VENUE)),
        1,
        "the mass cancel was not charted"
    );
    assert_eq!(
        snapshot.counter(EDGE_ORDERS_EXPIRED, &by("venue", VENUE)),
        0,
        "a halt was filed as an expiry, which hides a kill switch inside routine housekeeping"
    );
    assert_eq!(
        snapshot.counter(EDGE_MESSAGES_SENT, &message_labels(MessageKind::Withdrawal)),
        1,
        "the cancel was not counted as a message, so the venue's rate limit sees traffic the \
         cell's own budget does not"
    );
    assert!(
        journal_kinds(&cell).contains(&"mass_cancelled"),
        "the chain does not say the order was pulled by a halt: {:?}",
        journal_kinds(&cell)
    );
    assert!(
        !journal_kinds(&cell).contains(&"order_expired"),
        "the chain calls a halt an expiry: {:?}",
        journal_kinds(&cell)
    );
    Ok(())
}

#[test]
fn a_cancel_is_funded_from_the_reserve_after_placements_have_spent_the_rest() -> Result<()> {
    // The reserve's whole purpose, end to end. Without it a cell that has
    // quoted its session's rate away cannot withdraw, and the rate limit — a
    // control meant to keep the session up — becomes the reason an order the
    // cell wanted gone stays at the venue. One placement and one cancel in
    // the budget, and the placement may not touch the cancel's.
    let resting = PricingPolicy::rest_at_mid(Duration::from_secs(600))?;
    let (mut cell, metrics) = cell_under(limits(1, 1)?, resting)?;
    let mut gateway = RestingVenue::default();

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        1,
        "the premise failed: the first pass sent nothing: {:?}",
        first.refusals
    );

    // The same instant, so the bucket holds exactly the reserve.
    let second = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        refusals_under(&second, GATE_QUOTE_BUDGET).len(),
        1,
        "the premise failed: a second placement was admitted out of the reserve: {:?}",
        second.refusals
    );
    assert_eq!(
        metrics
            .snapshot()
            .gauge(EDGE_QUOTE_BUDGET_TOKENS, &by("venue", VENUE)),
        Some(1.0),
        "the premise failed: the bucket does not hold exactly the reserve"
    );

    cell.autonomy_mut().kill_switch_mut().trip_global(
        t(11),
        "drill",
        "a halt after the budget is spent",
    );
    let withdrawn = cell.withdraw_expired(&mut gateway, t(11));
    assert_eq!(
        withdrawn.len(),
        1,
        "the cancel the reserve exists for was refused: the cell cannot stop being exposed"
    );
    assert_eq!(
        metrics
            .snapshot()
            .counter(EDGE_MESSAGES_SENT, &message_labels(MessageKind::Withdrawal)),
        1,
        "the cancel was not billed against the budget it spent"
    );
    Ok(())
}

#[test]
fn a_halted_cell_that_cannot_withdraw_says_so_in_the_chain_rather_than_withdrawing_nothing_quietly()
-> Result<()> {
    // A halted cell holding orders it has no way to cancel is the single fact
    // an operator most needs from it, and a mass cancel that simply found no
    // cancel path and returned would be silence. Calling `cancel` anyway is
    // the other wrong answer: the gateway refuses, every halted pass books a
    // reconciliation break, and the real finding is buried under repetition.
    let (mut cell, metrics) = cell_under(limits(4, 2)?, PricingPolicy::Marketable)?;
    let mut gateway = OneWayVenue::default();

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        1,
        "the premise failed: nothing was left open: {:?}",
        first.refusals
    );

    cell.autonomy_mut().kill_switch_mut().trip_global(
        t(11),
        "drill",
        "a halt on a one-way gateway",
    );
    let withdrawn = cell.withdraw_expired(&mut gateway, t(12));
    assert!(
        withdrawn.is_empty(),
        "a gateway with no cancel path reported withdrawals"
    );
    assert!(
        cell.reconciliation_breaks().is_empty(),
        "the mass cancel booked a break for a gateway that never had a cancel path: {:?}",
        cell.reconciliation_breaks()
    );

    let refusals: Vec<&str> = cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            qip_edge::journal::Decision::Refused { gate, reason } if gate == GATE_MASS_CANCEL => {
                Some(reason.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        refusals.len(),
        1,
        "the chain does not record that the halted cell could not withdraw"
    );
    assert!(
        refusals[0].contains("1 resting order"),
        "the record does not say how much exposure was left standing: {}",
        refusals[0]
    );
    assert_eq!(
        metrics
            .snapshot()
            .counter(EDGE_ORDERS_MASS_CANCELLED, &base()),
        0,
        "a withdrawal that never happened was charted"
    );
    Ok(())
}

/// A venue that fills everything in full the moment it accepts it.
#[derive(Debug, Default)]
struct FillingVenue {
    pending: Vec<ExecutionReport>,
}

impl Placer for FillingVenue {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        self.pending.push(ExecutionReport {
            order_id: order_id.to_string(),
            venue: venue.clone(),
            quantity,
            price,
            at,
        });
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.pending)
    }
}

#[test]
fn only_a_fill_the_venue_reported_counts_as_a_trade_against_the_message_to_trade_ratio()
-> Result<()> {
    // The denominator has to come from the venue. If an order the cell sent
    // counted as its own trade, every message-to-trade ratio would read as
    // healthy by construction and the monitor would narrow nothing, ever —
    // a control that cannot fire, which is the failure this repository names
    // by name.
    let (mut cell, _) = cell_under(limits(8, 2)?, PricingPolicy::Marketable)?;
    let mut resting = RestingVenue::default();

    let quiet = cell.work(t(10), &mut resting)?;
    assert_eq!(
        quiet.orders.len(),
        1,
        "the premise failed: nothing was sent: {:?}",
        quiet.refusals
    );
    let after_sending = cell.quote_budget();
    assert_eq!(
        after_sending[0].placements, 1,
        "the premise failed: the message was not billed"
    );
    assert_eq!(
        after_sending[0].trades, 0,
        "an order the venue never reported filled was counted as a trade"
    );

    let mut filling = FillingVenue::default();
    let traded = cell.work(t(11), &mut filling)?;
    assert_eq!(
        traded.fills.len(),
        1,
        "the premise failed: the venue reported no fill: {:?}",
        traded.refusals
    );
    assert_eq!(
        cell.quote_budget()[0].trades,
        1,
        "the venue's own fill did not reach the message-to-trade monitor, so the ratio is \
         measured against a denominator that never moves"
    );
    Ok(())
}
