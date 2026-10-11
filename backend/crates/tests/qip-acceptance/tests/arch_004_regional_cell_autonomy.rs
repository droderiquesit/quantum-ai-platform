//! ARCH-004 / ADR 0008: cells decide alone.
//!
//! A cell that cannot reach the centre keeps working within its envelope
//! rather than stopping. The previous version of this file searched source
//! text: it asserted `cell.rs` contained "ledger" *or* "work" *or* "pass",
//! that `mesh.rs` contained "CircuitBreaker", that `main.rs` contained "main",
//! and that `cell.rs` contained "order". Every one of those words survives a
//! cell that halts the moment its uplink fails. These tests build a cell and
//! a real uplink to a centre that is not listening, and watch what the cell
//! does.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Clock, Decimal, Duration, ManualClock, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::mesh::{CellUplink, UplinkConfig};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_transport::breaker::{BreakerPolicy, BreakerState};
use qip_transport::{
    ClientLimits, MemoryDeadLetters, MeshConfig, RecordingSleeper, RetryPolicy, Sleeper,
};
use std::net::TcpListener;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const KEY: &[u8] = b"a-cell-envelope-key-for-arch-004";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn book() -> VenueState {
    let venue = VenueId::new(VENUE);
    let mut state = VenueState::aggregated(object(), venue.clone(), VenueStatus::Open);
    for (sequence, side, price, size) in [
        (0, BookSide::Bid, "99.95", "500"),
        (1, BookSide::Ask, "100.05", "400"),
    ] {
        state
            .apply(&MarketMessage::new(
                object(),
                Origin::new(venue.clone(), "feed-a", 0, sequence),
                MessageBody::LevelSet {
                    side,
                    price: d(price),
                    quantity: d(size),
                    order_count: None,
                },
                t(1),
                t(1),
            ))
            .unwrap();
    }
    state
}

fn envelope() -> VerifiedEnvelope {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("alpha"),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![VenueId::new(VENUE)],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned").unwrap();
    let signature = sign_payload(KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature).unwrap(), KEY, CELL, t(1)).unwrap()
}

/// A cell holding one strategy that wants to buy on every pass, and — when
/// `with_book` — a two-sided book at its one venue.
fn cell(with_book: bool) -> Cell {
    let config = CellConfig::new(CELL, REGION).with_venue(VenueId::new(VENUE));
    let mut cell = Cell::new(
        config,
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
    )
    .unwrap();
    if with_book {
        cell.track(book());
    }
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new("alpha"), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(d("10")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec).unwrap();
    cell.deploy_with_pricing(
        compiled,
        compiler.into_program(),
        envelope(),
        PricingPolicy::Marketable,
    )
    .unwrap();
    cell
}

/// A paper gateway: simulated, accepts everything, remembers it.
#[derive(Debug, Default)]
struct PaperGateway {
    placed: Vec<(String, VenueId, Decimal)>,
}

impl Placer for PaperGateway {
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
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed
            .push((order_id.to_string(), venue.clone(), quantity));
        Ok(())
    }
}

/// An address nothing listens on: bound, then dropped.
fn dead_centre() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    format!("http://{address}")
}

fn uplink_to(peer: &str) -> CellUplink {
    let mesh = MeshConfig::new("uplink:london-1", peer)
        .with_retry(RetryPolicy {
            max_attempts: 2,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
            multiplier: 2,
            jitter_basis_points: 0,
        })
        .with_limits(ClientLimits {
            read_timeout: std::time::Duration::from_millis(300),
            connect_timeout: std::time::Duration::from_millis(300),
            ..ClientLimits::default()
        });
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    let sleeper: Arc<dyn Sleeper> = Arc::new(RecordingSleeper::new());
    CellUplink::connect(
        UplinkConfig::new(CELL, REGION, mesh).with_breaker(
            BreakerPolicy {
                failure_threshold: 1,
                ..BreakerPolicy::default()
            },
            7,
        ),
        clock,
        sleeper,
        Box::new(MemoryDeadLetters::new(16)),
    )
    .unwrap()
}

/// The centre is unreachable before the cell's first pass and stays so. The
/// cell places its order anyway, at the venue it holds a book for, and the
/// uplink reports the outage as a value — dead-lettered, then refused by the
/// open circuit without touching the network — rather than as an error that
/// would unwind the work loop.
///
/// Mutation: in `Cell::work`, refuse every intent while the cell has applied
/// no policy payload from the centre (e.g. return early when
/// `self.policy_sequence() == 0`) — `report.orders` is empty and the
/// order assertion fires. Mutation: make `CellUplink::publish` return
/// `Err` instead of `Dispatch::DeadLettered` — the `?`-free `.unwrap()` on the
/// publish panics, which is the cascade this guards.
#[test]
fn a_cell_that_has_never_reached_the_centre_places_orders_within_its_envelope_and_survives_the_outage()
 {
    let mut uplink = uplink_to(&dead_centre());
    let mut cell = cell(true);
    let mut gateway = PaperGateway::default();

    // Premise: the centre really is unreachable, and the breaker has opened.
    let probe = uplink
        .publish(cell.state_delta(&Default::default(), t(1)), t(1))
        .unwrap();
    assert_eq!(probe.code(), "dead_lettered");
    assert_eq!(uplink.circuit(), BreakerState::Open);

    let report = cell.work(t(2), &mut gateway).unwrap();
    assert!(
        !report.halted,
        "the cell halted because it could not see the centre"
    );
    assert!(!cell.is_halted());
    assert_eq!(
        report.orders.len(),
        1,
        "the cell placed nothing without the centre: refusals {:?}",
        report.refusals
    );
    let order = &report.orders[0];
    assert_eq!(order.venue, VenueId::new(VENUE));
    assert!(order.simulated, "paper gateway, paper order");
    assert_eq!(gateway.placed.len(), 1);
    assert_eq!(gateway.placed[0].0, order.order_id);

    // The post-pass delta to the dead centre costs no network attempt and is
    // a value, not an error.
    let after = uplink
        .publish(cell.state_delta(&report, t(2)), t(2))
        .unwrap();
    assert_eq!(after.code(), "circuit_open");
    assert_eq!(uplink.stats().circuit_refusals, 1);
    assert!(!cell.is_halted());
}

/// The cell decides from the book it holds, not from anything the centre
/// tells it: the same cell without its local book places nothing, and with
/// it places at that book's venue. Neither case involves a centre.
///
/// Mutation: in `Cell::venue_for`, return `self.config.venues.first().cloned()`
/// when no tracked state exists — the book-less cell then routes an order and
/// the emptiness assertion fires.
#[test]
fn the_cell_routes_from_its_own_book_and_without_one_places_nothing() {
    let mut with = cell(true);
    let mut gateway = PaperGateway::default();
    let report = with.work(t(2), &mut gateway).unwrap();
    // Premise: the strategy fires and the book makes it routable.
    assert_eq!(
        report.orders.len(),
        1,
        "premise: refusals {:?}",
        report.refusals
    );
    assert_eq!(report.orders[0].venue, VenueId::new(VENUE));

    let mut without = cell(false);
    let mut gateway = PaperGateway::default();
    let report = without.work(t(2), &mut gateway).unwrap();
    assert!(report.orders.is_empty(), "{:?}", report.orders);
    assert!(gateway.placed.is_empty());
}
