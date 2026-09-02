//! How an intent is priced when it reaches a venue is a stated choice.
//!
//! Every order used to be a limit at the mid. Against a real two-sided book
//! that order rests, and with nothing to withdraw it, it rests forever: a
//! position the cell has promised to take at a price the market may have
//! left. Pricing is now a policy the strategy is deployed with — take the
//! touch, or rest at the mid for a stated time and then withdraw — and a
//! strategy that stated none has its intents refused, never priced for it.
//! Each test here drives a cell through one pricing outcome and asserts
//! what reached the venue, at what price, and what was withdrawn when.

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
use qip_edge::telemetry::EDGE_ORDERS_EXPIRED;
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
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-pricing-tests";

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

/// A two-sided book, 99 bid / 101 ask, with the sizes given: a mid of 100
/// that is neither touch.
fn book(bid_size: &str, ask_size: &str) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    for (index, (side, price, size)) in [
        (BookSide::Bid, "99", bid_size),
        (BookSide::Ask, "101", ask_size),
    ]
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

/// What the fixture venue does with a cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CancelPath {
    /// Withdraws, reporting the whole order as still open.
    Works,
    /// Has no cancel at all, and says so.
    Absent,
    /// Claims a cancel path and refuses every cancel.
    Refuses,
}

/// A gateway that accepts every order, fills nothing on its own, and
/// withdraws — or not — as the test configured.
#[derive(Debug)]
struct VenueGateway {
    cancel_path: CancelPath,
    placed: Vec<(String, BookSide, Decimal, Decimal)>,
    cancelled: Vec<String>,
    reports: Vec<ExecutionReport>,
}

impl VenueGateway {
    fn with(cancel_path: CancelPath) -> Self {
        Self {
            cancel_path,
            placed: Vec::new(),
            cancelled: Vec::new(),
            reports: Vec::new(),
        }
    }
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
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed
            .push((order_id.to_string(), side, quantity, price));
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }

    fn can_cancel(&self) -> bool {
        self.cancel_path != CancelPath::Absent
    }

    fn cancel(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _at: Timestamp,
    ) -> Result<Decimal> {
        match self.cancel_path {
            CancelPath::Works => {
                let remaining = self
                    .placed
                    .iter()
                    .find(|(id, ..)| id == order_id)
                    .map(|(_, _, quantity, _)| *quantity)
                    .ok_or_else(|| Error::not_found(format!("no order {order_id}")))?;
                self.cancelled.push(order_id.to_string());
                Ok(remaining)
            }
            CancelPath::Absent => Err(Error::denied("this gateway has no cancel path")),
            CancelPath::Refuses => {
                Err(Error::io(format!("the venue refused to cancel {order_id}")))
            }
        }
    }
}

fn wired_cell(book: VenueState) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));
    cell.track(book);
    Ok((cell, metrics))
}

fn deploy(
    cell: &mut Cell,
    id: &str,
    kind: SignalKind,
    size: &str,
    pricing: Option<PricingPolicy>,
) -> Result<()> {
    let (compiled, program) = firing_strategy(id, kind, size)?;
    match pricing {
        Some(pricing) => cell.deploy_with_pricing(compiled, program, signed_envelope(id)?, pricing),
        None => cell.deploy(compiled, program, signed_envelope(id)?),
    }
}

fn rest(secs: i64) -> Result<PricingPolicy> {
    PricingPolicy::rest_at_mid(Duration::from_secs(secs))
}

fn refusal<'a>(report: &'a WorkReport, gate: &str) -> Option<&'a str> {
    report
        .refusals
        .iter()
        .find(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
}

#[test]
fn an_intent_with_no_stated_pricing_is_refused_and_nothing_reaches_the_venue() -> Result<()> {
    // The safe default. A cell that priced an unstated intent would be
    // deciding for the strategy whether to pay the spread, and "market" is
    // the guess that costs money on every fill.
    let (mut cell, metrics) = wired_cell(book("500", "400")?)?;
    deploy(&mut cell, "unpriced", SignalKind::Enter, "100", None)?;
    let mut gateway = VenueGateway::with(CancelPath::Works);

    let report = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        report.signals.len(),
        1,
        "the premise is a strategy that fires"
    );
    let reason = refusal(&report, "pricing").unwrap_or_else(|| {
        panic!(
            "an unpriced intent was not refused under pricing: {:?}",
            report.refusals
        )
    });
    assert!(
        reason.contains("no pricing policy") && reason.contains("deploy_with_pricing"),
        "the refusal does not say what to do instead: {reason}"
    );
    assert!(
        gateway.placed.is_empty(),
        "an intent with no stated pricing reached the venue: {:?}",
        gateway.placed
    );
    assert_eq!(
        metrics
            .snapshot()
            .counter(names::EDGE_REFUSALS, &by("gate", "pricing")),
        1,
        "the refusal was not counted under its gate"
    );
    Ok(())
}

#[test]
fn a_marketable_buy_takes_the_ask_and_a_marketable_sell_hits_the_bid_and_neither_is_the_mid()
-> Result<()> {
    for (kind, expected, side) in [
        (SignalKind::Enter, "101", BookSide::Ask),
        (SignalKind::Exit, "99", BookSide::Bid),
    ] {
        let (mut cell, _metrics) = wired_cell(book("500", "400")?)?;
        deploy(
            &mut cell,
            "taker",
            kind,
            "100",
            Some(PricingPolicy::Marketable),
        )?;
        let mid = cell
            .liquidity()
            .get(&venue(), &object())
            .and_then(VenueState::mid)
            .ok_or_else(|| Error::not_found("the fixture book's mid"))?;
        assert_ne!(
            mid,
            d(expected),
            "the fixture cannot tell the touch from the mid"
        );
        let mut gateway = VenueGateway::with(CancelPath::Works);

        let report = cell.work(t(50), &mut gateway)?;
        assert!(
            report.refusals.is_empty(),
            "{kind:?}: refused: {:?}",
            report.refusals
        );
        assert_eq!(report.orders.len(), 1, "{kind:?}: no order was sent");
        assert_eq!(report.orders[0].side, side);
        assert_eq!(
            report.orders[0].price,
            d(expected),
            "{kind:?}: a marketable order was not sent at the touch"
        );
        assert_eq!(gateway.placed.len(), 1);
        assert_eq!(
            gateway.placed[0].3,
            d(expected),
            "the venue received a different price"
        );
        let open = cell.open_orders();
        assert_eq!(open.len(), 1);
        assert_eq!(
            open[0].expires_at, None,
            "a marketable order was given a time to live"
        );
    }
    Ok(())
}

#[test]
fn a_marketable_net_larger_than_the_touch_is_refused_rather_than_walked_deeper() -> Result<()> {
    // Two contributors each within the touch net to more than it holds.
    // The feasibility gate judged each intent alone and admitted both; the
    // order that would go out is the net, and it is judged again at the
    // size that goes out.
    let (mut cell, _metrics) = wired_cell(book("500", "50")?)?;
    deploy(
        &mut cell,
        "alpha",
        SignalKind::Enter,
        "100",
        Some(PricingPolicy::Marketable),
    )?;
    deploy(
        &mut cell,
        "beta",
        SignalKind::Enter,
        "100",
        Some(PricingPolicy::Marketable),
    )?;
    let mut gateway = VenueGateway::with(CancelPath::Works);

    let report = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        report.signals.len(),
        2,
        "the premise is two firing strategies"
    );
    let reason = refusal(&report, "feasibility_depth")
        .unwrap_or_else(|| panic!("the net was not refused for depth: {:?}", report.refusals));
    // Each intent alone is 37.5 against a touch of 50 — admitted — and the
    // net is 75. The refusal must be about the net, or it is the per-intent
    // gate firing on a fixture that was meant to pass it.
    assert!(
        reason.contains("the net of 75") && reason.contains("exceeds the 50"),
        "the refusal is not about the net's size against the touch: {reason}"
    );
    assert_eq!(
        report.refusals.len(),
        1,
        "the premise is that each intent alone passed the gate: {:?}",
        report.refusals
    );
    assert!(
        gateway.placed.is_empty(),
        "a net larger than the touch reached the venue"
    );
    Ok(())
}

#[test]
fn a_resting_order_rests_at_the_mid_and_is_withdrawn_when_its_time_to_live_elapses() -> Result<()> {
    let (mut cell, metrics) = wired_cell(book("500", "400")?)?;
    deploy(
        &mut cell,
        "rester",
        SignalKind::Enter,
        "100",
        Some(rest(10)?),
    )?;
    let mut gateway = VenueGateway::with(CancelPath::Works);

    let first = cell.work(t(50), &mut gateway)?;
    assert!(first.refusals.is_empty(), "{:?}", first.refusals);
    assert_eq!(first.orders.len(), 1, "the premise is a resting order sent");
    let order_id = first.orders[0].order_id.clone();
    assert_eq!(
        first.orders[0].price,
        d("100"),
        "a resting order was not sent at the mid"
    );
    let open = cell.open_orders();
    assert_eq!(
        open[0].expires_at,
        Some(t(60)),
        "the time to live was not stamped on the order"
    );

    // Before the time to live: still resting, nothing withdrawn. The pass
    // sends another resting order on the same side, which is allowed —
    // only the other side would be a self-trade.
    cell.work(t(55), &mut gateway)?;
    assert!(
        gateway.cancelled.is_empty(),
        "an order was withdrawn before its time to live: {:?}",
        gateway.cancelled
    );
    assert!(
        cell.open_orders()
            .iter()
            .any(|o| o.order_id == order_id && o.closed.is_none()),
        "the resting order was closed early"
    );

    // At the time to live: withdrawn through the venue, closed as expired,
    // journaled and counted — and, with nothing filled on either side, it
    // settles on the next clean comparison.
    cell.work(t(60), &mut gateway)?;
    assert_eq!(
        gateway.cancelled,
        vec![order_id.clone()],
        "the expired order was not withdrawn at the venue, or something else was"
    );
    let expired = cell
        .open_orders()
        .into_iter()
        .find(|o| o.order_id == order_id)
        .ok_or_else(|| Error::not_found("the expired order before settlement"))?;
    assert_eq!(expired.closed.as_deref(), Some("expired"));
    assert!(
        cell.journal().entries().iter().any(|entry| matches!(
            &entry.decision,
            Decision::OrderExpired { order_id: id, withdrawn, .. }
                if *id == order_id && *withdrawn == first.orders[0].quantity.to_string()
        )),
        "the withdrawal did not reach the journal with the venue's own remaining"
    );
    assert_eq!(
        metrics
            .snapshot()
            .counter(EDGE_ORDERS_EXPIRED, &by("venue", VENUE)),
        1,
        "the withdrawal was not counted"
    );
    let breaks = cell.reconcile(t(61));
    assert!(breaks.is_empty(), "{breaks:?}");
    assert!(
        cell.open_orders().iter().all(|o| o.order_id != order_id),
        "an expired order both sides agree on was not settled"
    );
    assert!(!cell.is_halted());
    Ok(())
}

#[test]
fn a_resting_policy_is_refused_on_a_gateway_that_cannot_withdraw() -> Result<()> {
    // An order nothing can withdraw is a promise the cell cannot take back.
    // Refused before the venue sees it, naming the reason.
    let (mut cell, _metrics) = wired_cell(book("500", "400")?)?;
    deploy(
        &mut cell,
        "rester",
        SignalKind::Enter,
        "100",
        Some(rest(10)?),
    )?;
    let mut gateway = VenueGateway::with(CancelPath::Absent);
    assert!(
        !gateway.can_cancel(),
        "the premise is a gateway with no cancel path"
    );

    let report = cell.work(t(50), &mut gateway)?;
    assert_eq!(report.signals.len(), 1);
    let reason = refusal(&report, "pricing").unwrap_or_else(|| {
        panic!(
            "a resting order was sent to a gateway that cannot withdraw it: {:?}",
            report.refusals
        )
    });
    assert!(reason.contains("cannot withdraw"), "{reason}");
    assert!(
        gateway.placed.is_empty(),
        "the order reached the venue: {:?}",
        gateway.placed
    );
    Ok(())
}

#[test]
fn contributors_that_disagree_on_pricing_refuse_the_net_rather_than_choose() -> Result<()> {
    let (mut cell, _metrics) = wired_cell(book("500", "400")?)?;
    deploy(
        &mut cell,
        "taker",
        SignalKind::Enter,
        "100",
        Some(PricingPolicy::Marketable),
    )?;
    deploy(
        &mut cell,
        "rester",
        SignalKind::Enter,
        "50",
        Some(rest(10)?),
    )?;
    let mut gateway = VenueGateway::with(CancelPath::Works);

    let report = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        report.signals.len(),
        2,
        "the premise is two contributors to one net"
    );
    let reason = refusal(&report, "pricing_conflict")
        .unwrap_or_else(|| panic!("a net with two pricings was sent: {:?}", report.refusals));
    assert!(
        reason.contains("marketable") && reason.contains("rest_at_mid"),
        "the refusal does not name the two policies: {reason}"
    );
    assert!(
        gateway.placed.is_empty(),
        "the cell chose a price for the net: {:?}",
        gateway.placed
    );
    Ok(())
}

#[test]
fn a_net_opposite_the_cell_s_own_resting_order_is_refused_as_a_self_trade() -> Result<()> {
    // Netting stops two strategies crossing each other within a pass. A
    // buy resting from the last pass and a sell this pass is the same
    // self-trade with a pass between, and the venue would match it.
    let (mut cell, _metrics) = wired_cell(book("500", "400")?)?;
    deploy(
        &mut cell,
        "buyer",
        SignalKind::Enter,
        "100",
        Some(rest(60)?),
    )?;
    let mut gateway = VenueGateway::with(CancelPath::Works);
    let first = cell.work(t(50), &mut gateway)?;
    assert_eq!(first.orders.len(), 1, "the premise is a resting buy");
    assert_eq!(first.orders[0].side, BookSide::Ask);
    assert!(
        cell.open_orders()[0].remaining().is_positive(),
        "the premise is a buy still working"
    );

    // A seller twice the buyer's size, so the net this pass is a sell.
    deploy(
        &mut cell,
        "seller",
        SignalKind::Exit,
        "200",
        Some(rest(60)?),
    )?;
    let second = cell.work(t(51), &mut gateway)?;
    assert_eq!(second.signals.len(), 2);
    let reason = refusal(&second, "self_trade").unwrap_or_else(|| {
        panic!(
            "a sell was sent against the cell's own resting buy: {:?}",
            second.refusals
        )
    });
    assert!(reason.contains("resting on the other side"), "{reason}");
    assert_eq!(
        gateway.placed.len(),
        1,
        "the second pass sent an order the venue would have matched with the first"
    );
    Ok(())
}

#[test]
fn a_cancel_the_venue_refuses_is_a_break_and_halts_the_cell() -> Result<()> {
    // The order's state is now unknown: it may still be working at a price
    // the market has left. Unknown is the one state the cell does not trade
    // through.
    let (mut cell, _metrics) = wired_cell(book("500", "400")?)?;
    deploy(
        &mut cell,
        "rester",
        SignalKind::Enter,
        "100",
        Some(rest(10)?),
    )?;
    let mut gateway = VenueGateway::with(CancelPath::Refuses);
    let first = cell.work(t(50), &mut gateway)?;
    assert_eq!(first.orders.len(), 1, "the premise is a resting order");
    assert!(!cell.is_halted());

    cell.work(t(60), &mut gateway)?;
    assert!(
        cell.is_halted(),
        "a cancel the venue refused left the cell running"
    );
    assert!(
        cell.reconciliation_breaks()
            .iter()
            .any(|detail| detail.contains("refused to withdraw")),
        "{:?}",
        cell.reconciliation_breaks()
    );
    Ok(())
}

#[test]
fn a_time_to_live_that_could_not_elapse_is_refused_at_deployment() -> Result<()> {
    assert!(PricingPolicy::rest_at_mid(Duration::from_secs(0)).is_err());
    assert!(PricingPolicy::rest_at_mid(Duration::from_secs(-1)).is_err());
    let (mut cell, _metrics) = wired_cell(book("500", "400")?)?;
    // A literal around the constructor is judged at the same seam.
    let outcome = deploy(
        &mut cell,
        "rester",
        SignalKind::Enter,
        "100",
        Some(PricingPolicy::RestAtMid {
            time_to_live: Duration::from_secs(0),
        }),
    );
    assert!(outcome.is_err(), "a zero time to live was deployed");
    assert!(cell.deployed_strategies().is_empty());
    Ok(())
}
