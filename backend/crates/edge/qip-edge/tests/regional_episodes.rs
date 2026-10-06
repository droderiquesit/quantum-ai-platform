//! A reflex cell records its own regional episodes (EXPAND-009).
//!
//! Every test here drives the cell through the order-entry channel — a pass
//! that sends an order, then the venue's report on it — and asserts which
//! episodes the cell recorded, of which kind, naming which cell and region.
//! None calls a recorder directly. The first version of this suite did
//! exactly that: it inserted an episode by hand and asserted that it was
//! there, so every recording site in the cell could be deleted and the suite
//! still passed.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::regional_episode::{EpisodeKind, RegionalEpisode};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{
    Cell, CellConfig, ExecutionReport, MAX_RETAINED_EPISODES, PlacedOrder, Placer, PricingPolicy,
};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const SYMBOL: &str = "ACME";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-episode-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn level(sequence: u64, side: BookSide, price: Decimal, size: Decimal) -> MarketMessage {
    let when = t(sequence as i64);
    MarketMessage::new(
        object(),
        Origin::new(venue(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price,
            quantity: size,
            order_count: None,
        },
        when,
        when,
    )
}

/// A two-sided book with a mid of 100.
fn book() -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    state.apply(&level(0, BookSide::Bid, dec!("99"), dec!("500")))?;
    state.apply(&level(1, BookSide::Ask, dec!("101"), dec!("400")))?;
    Ok(state)
}

fn firing_strategy(id: &str) -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(
        Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("100")),
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
/// it the venue did.
#[derive(Debug, Default)]
struct ReportingGateway {
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
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }
}

fn trading_cell() -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.track(book()?);
    let (compiled, program) = firing_strategy("alpha")?;
    cell.deploy_with_pricing(
        compiled,
        program,
        signed_envelope("alpha")?,
        PricingPolicy::Marketable,
    )?;
    Ok(cell)
}

/// One pass at `at` that sends exactly one order, a buy.
fn send_one(cell: &mut Cell, gateway: &mut ReportingGateway, at: Timestamp) -> Result<PlacedOrder> {
    let report = cell.work(at, gateway)?;
    assert!(
        report.refusals.is_empty(),
        "the premise is a pass that refuses nothing: {:?}",
        report.refusals
    );
    assert_eq!(report.orders.len(), 1, "the premise is one order");
    let order = report
        .orders
        .first()
        .cloned()
        .ok_or_else(|| Error::not_found("an order from a cell that signalled"))?;
    assert_eq!(order.side, BookSide::Ask, "the premise is a buy");
    assert!(
        cell.episodes().is_empty(),
        "sending an order recorded an episode: {:?}",
        cell.episodes()
    );
    Ok(order)
}

fn kinds(episodes: &[RegionalEpisode]) -> Vec<EpisodeKind> {
    episodes.iter().map(|e| e.kind.clone()).collect()
}

fn assert_names_this_cell(episodes: &[RegionalEpisode]) {
    for episode in episodes {
        assert_eq!(episode.cell, CELL, "{episode:?}");
        assert_eq!(episode.region, REGION, "{episode:?}");
    }
}

fn bps(adverse: Decimal, limit: Decimal) -> Result<Decimal> {
    adverse
        .checked_div(limit)
        .map(|ratio| ratio * Decimal::from_int(10_000))
        .ok_or_else(|| Error::numeric("a zero limit in a test"))
}

#[test]
fn a_fill_at_its_limit_inside_the_threshold_records_no_episode() -> Result<()> {
    // The quiet case first, so the cases below are not passing on a cell
    // that records an episode for every fill.
    let mut cell = trading_cell()?;
    let mut gateway = ReportingGateway::default();
    let order = send_one(&mut cell, &mut gateway, t(50))?;
    gateway.report(&order.order_id, order.quantity, order.price, t(50));
    let confirmed = cell.confirm_execution_reports(&mut gateway, t(50));
    assert_eq!(confirmed.len(), 1, "the premise is a booked fill");
    assert!(!cell.is_halted(), "the premise is a clean fill");
    assert_eq!(cell.episodes(), &[] as &[RegionalEpisode]);
    Ok(())
}

#[test]
fn an_order_completing_past_the_threshold_records_one_latency_spike_naming_the_cell_and_region()
-> Result<()> {
    let mut cell = trading_cell()?;
    let mut gateway = ReportingGateway::default();
    let order = send_one(&mut cell, &mut gateway, t(50))?;
    // Five seconds after release, at the limit, so latency is the only fact.
    gateway.report(&order.order_id, order.quantity, order.price, t(55));
    let confirmed = cell.confirm_execution_reports(&mut gateway, t(56));
    assert_eq!(confirmed.len(), 1, "the premise is a booked fill");

    let episodes = cell.episodes();
    assert_eq!(kinds(episodes), vec![EpisodeKind::LatencySpike]);
    assert_names_this_cell(episodes);
    assert_eq!(
        episodes[0].occurred_at,
        Some(t(55)),
        "when the venue filled"
    );
    assert_eq!(
        episodes[0].recorded_at,
        t(56),
        "when the cell learned of it"
    );
    assert!(
        episodes[0].detail.contains(" 5000 ms after its release"),
        "{}",
        episodes[0].detail
    );
    Ok(())
}

#[test]
fn a_buy_filled_above_its_limit_is_recorded_as_adverse_slippage() -> Result<()> {
    let mut cell = trading_cell()?;
    let mut gateway = ReportingGateway::default();
    let order = send_one(&mut cell, &mut gateway, t(50))?;
    let paid = order.price + dec!("1");
    gateway.report(&order.order_id, order.quantity, paid, t(50));
    assert_eq!(cell.confirm_execution_reports(&mut gateway, t(50)).len(), 1);

    let episodes = cell.episodes();
    assert_eq!(kinds(episodes), vec![EpisodeKind::FillSlippage]);
    assert_names_this_cell(episodes);
    let expected = format!(": {} bps adverse", bps(dec!("1"), order.price)?);
    assert!(
        episodes[0].detail.ends_with(&expected),
        "expected the detail to end {expected:?}: {}",
        episodes[0].detail
    );
    Ok(())
}

#[test]
fn a_buy_filled_below_its_limit_is_recorded_as_price_improvement_not_as_a_cost() -> Result<()> {
    // Unsigned, the first version filed this as slippage: a venue that
    // improved on the limit read as a venue that cost money.
    let mut cell = trading_cell()?;
    let mut gateway = ReportingGateway::default();
    let order = send_one(&mut cell, &mut gateway, t(50))?;
    let paid = order.price - dec!("1");
    gateway.report(&order.order_id, order.quantity, paid, t(50));
    assert_eq!(cell.confirm_execution_reports(&mut gateway, t(50)).len(), 1);

    let episodes = cell.episodes();
    assert_eq!(kinds(episodes), vec![EpisodeKind::FillSlippage]);
    let expected = format!(": {} bps price improvement", bps(dec!("1"), order.price)?);
    assert!(
        episodes[0].detail.ends_with(&expected),
        "expected the detail to end {expected:?}: {}",
        episodes[0].detail
    );
    Ok(())
}

#[test]
fn a_report_on_an_order_never_sent_records_an_anomaly_then_one_failure() -> Result<()> {
    // The anomaly is what arrived; the failure is the cell halting on it.
    let mut cell = trading_cell()?;
    let mut gateway = ReportingGateway::default();
    gateway.report("ghost-1", dec!("10"), dec!("100"), t(5));
    assert!(
        cell.confirm_execution_reports(&mut gateway, t(6))
            .is_empty()
    );
    assert!(cell.is_halted(), "the premise is a break");

    let episodes = cell.episodes();
    assert_eq!(
        kinds(episodes),
        vec![EpisodeKind::Anomaly, EpisodeKind::Failure]
    );
    assert_names_this_cell(episodes);
    assert_eq!(episodes[0].occurred_at, Some(t(5)));
    assert!(
        episodes[0].detail.contains("on order ghost-1 at XLON"),
        "{}",
        episodes[0].detail
    );
    Ok(())
}

#[test]
fn an_over_fill_records_exactly_one_failure() -> Result<()> {
    // The first version recorded a Failure for the over-fill and another
    // inside the break it raised, so one event was filed twice.
    let mut cell = trading_cell()?;
    let mut gateway = ReportingGateway::default();
    let order = send_one(&mut cell, &mut gateway, t(50))?;
    gateway.report(
        &order.order_id,
        order.quantity * dec!("2"),
        order.price,
        t(50),
    );
    assert_eq!(cell.confirm_execution_reports(&mut gateway, t(50)).len(), 1);
    assert!(cell.is_halted(), "the premise is a break");

    assert_eq!(kinds(cell.episodes()), vec![EpisodeKind::Failure]);
    assert_names_this_cell(cell.episodes());
    Ok(())
}

#[test]
fn episodes_past_the_bound_drop_the_oldest_first() -> Result<()> {
    // Each unknown-order report is two episodes, so one more report than
    // half the bound overflows it by exactly two: the first report's pair.
    let mut cell = trading_cell()?;
    let mut gateway = ReportingGateway::default();
    let reports = MAX_RETAINED_EPISODES / 2 + 1;
    for n in 0..reports {
        gateway.report(&format!("ghost-{n}"), dec!("10"), dec!("100"), t(5));
    }
    assert!(
        cell.confirm_execution_reports(&mut gateway, t(6))
            .is_empty()
    );

    let episodes = cell.episodes();
    assert_eq!(episodes.len(), MAX_RETAINED_EPISODES);
    assert_eq!(episodes[0].kind, EpisodeKind::Anomaly);
    assert!(
        episodes[0].detail.contains("on order ghost-1 at "),
        "the oldest retained should be the second report's: {}",
        episodes[0].detail
    );
    let last = &episodes[MAX_RETAINED_EPISODES - 1];
    assert_eq!(last.kind, EpisodeKind::Failure);
    assert!(
        last.detail
            .contains(&format!("on order ghost-{} at ", reports - 1)),
        "{}",
        last.detail
    );
    Ok(())
}
