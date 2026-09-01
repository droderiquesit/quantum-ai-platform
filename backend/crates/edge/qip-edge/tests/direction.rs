//! Which way an order leaves the cell, asserted rather than inferred.
//!
//! Until this file existed nothing asserted the side of a placed order
//! against the signal that raised it. The netting and crossing suites count
//! orders, sum contributor shares through `abs()`, and check that a cross
//! names *a* buyer and *a* seller — every one of them passes with the two
//! sides swapped. They did pass that way: `intent_for` signed a buy negative
//! and `place_net` read a negative net as a buy, so an `Enter` still reached
//! the venue as a buy while the cross ledger recorded the entering strategy
//! under `sold`. Two inversions that cancel at the venue are invisible to a
//! test that only looks at the venue.
//!
//! The convention these tests pin: an order's `BookSide` is the side of the
//! book it takes, so `Ask` is a buy and `Bid` is a sell; a contributor's
//! signed share is positive for a buy. Both the node's gateways and the
//! orderbook's sweep read `BookSide` this way.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
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

/// A gateway that keeps what it was told, so the side the *venue* was asked
/// for can be asserted independently of the side the cell reports.
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

fn trading_cell(strategies: &[(&str, SignalKind, &str)]) -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.track(book()?);
    for (id, kind, size) in strategies {
        let (compiled, program) = firing_strategy(id, *kind, size)?;
        cell.deploy(compiled, program, signed_envelope(id)?)?;
    }
    Ok(cell)
}

/// One pass of a single strategy, with the premise checked: the signal was
/// the kind asked for and exactly one order reached the gateway.
fn one_order(kind: SignalKind) -> Result<(WorkReport, RecordingGateway)> {
    let mut cell = trading_cell(&[("alpha", kind, "100")])?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        report.signals.len(),
        1,
        "the premise failed: the strategy did not fire once: {:?}",
        report.refusals
    );
    assert_eq!(
        report.signals[0].kind, kind,
        "the premise failed: the signal was not {kind:?}"
    );
    assert_eq!(
        report.orders.len(),
        1,
        "the premise failed: no order was placed: {:?}",
        report.refusals
    );
    assert_eq!(
        gateway.placed.len(),
        1,
        "the premise failed: the gateway was not asked for exactly one order"
    );
    Ok((report, gateway))
}

#[test]
fn an_enter_signal_leaves_the_cell_taking_the_ask_which_is_a_buy() -> Result<()> {
    let (report, gateway) = one_order(SignalKind::Enter)?;
    let order = &report.orders[0];

    // The side the venue was asked for, and the side the cell reports, are
    // the same object read from two places.
    assert_eq!(
        gateway.placed[0].0,
        BookSide::Ask,
        "an Enter was sent to the venue hitting the bid, which is a sell"
    );
    assert_eq!(order.side, BookSide::Ask);
    // The size is whatever the degradation floor left of the hundred asked
    // for — a cell with no policy sizes conservatively — so it is read off
    // the order rather than restated, and only required to be a real size.
    assert!(order.quantity.is_positive());
    assert_eq!(gateway.placed[0].1, order.quantity);

    // The contributor's share is positive: a buy, by the sign convention
    // `Intent::signed_size` documents. This is the value the cross ledger
    // and the centre's attribution read, and the one that was inverted.
    assert_eq!(order.contributors.len(), 1);
    assert_eq!(
        order.contributors[0].signed_size, order.quantity,
        "an Enter's share was carried with a sell's sign"
    );
    Ok(())
}

#[test]
fn an_exit_signal_leaves_the_cell_hitting_the_bid_which_is_a_sell() -> Result<()> {
    let (report, gateway) = one_order(SignalKind::Exit)?;
    let order = &report.orders[0];

    assert_eq!(
        gateway.placed[0].0,
        BookSide::Bid,
        "an Exit was sent to the venue taking the ask, which is a buy"
    );
    assert_eq!(order.side, BookSide::Bid);
    assert!(order.quantity.is_positive());
    assert_eq!(gateway.placed[0].1, order.quantity);
    assert_eq!(order.contributors.len(), 1);
    assert_eq!(
        order.contributors[0].signed_size, -order.quantity,
        "an Exit's share was carried with a buy's sign"
    );
    Ok(())
}

#[test]
fn a_cross_names_the_entering_strategy_as_the_buyer_and_the_exiting_one_as_the_seller() -> Result<()>
{
    // The ledger entry §27.1 calls a regulatory expectation. Before the sign
    // was corrected this recorded the strategy that wanted to buy under
    // `sold`, and every crossing test passed, because each only checked that
    // one name appeared on each side.
    let mut cell = trading_cell(&[
        ("alpha", SignalKind::Enter, "100"),
        ("beta", SignalKind::Exit, "40"),
    ])?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;

    // Premise: both fired, in the kinds named, and the offset was booked.
    assert_eq!(report.signals.len(), 2, "the fixture needs two signals");
    assert!(
        report
            .signals
            .iter()
            .any(|s| s.strategy.as_str() == "alpha" && s.kind == SignalKind::Enter),
        "alpha did not raise an Enter"
    );
    assert!(
        report
            .signals
            .iter()
            .any(|s| s.strategy.as_str() == "beta" && s.kind == SignalKind::Exit),
        "beta did not raise an Exit"
    );
    assert_eq!(
        report.crosses.len(),
        1,
        "the premise failed: no cross was booked: {:?}",
        report.refusals
    );

    let cross = &report.crosses[0];
    assert_eq!(
        cross
            .bought
            .iter()
            .map(StrategyId::as_str)
            .collect::<Vec<_>>(),
        vec!["alpha"],
        "the entering strategy was not recorded as the buyer"
    );
    assert_eq!(
        cross
            .sold
            .iter()
            .map(StrategyId::as_str)
            .collect::<Vec<_>>(),
        vec!["beta"],
        "the exiting strategy was not recorded as the seller"
    );

    // The residual is alpha's remaining buy, sent as one. Sizes are read off
    // the contributors because the degradation floor scales both asks, and
    // the relation — not the literal — is what says which way each went:
    // alpha's share positive, beta's negative, the cross the smaller of the
    // two, the order what is left of the larger.
    assert_eq!(report.orders.len(), 1);
    let order = &report.orders[0];
    let share = |name: &str| -> Decimal {
        order
            .contributors
            .iter()
            .find(|contributor| contributor.strategy.as_str() == name)
            .map(|contributor| contributor.signed_size)
            .expect("both strategies contributed to the one order")
    };
    let alpha = share("alpha");
    let beta = share("beta");
    assert!(alpha.is_positive(), "alpha's Enter was carried as a sell");
    assert!(beta.is_negative(), "beta's Exit was carried as a buy");
    assert!(
        alpha > -beta,
        "the premise failed: alpha did not outsize beta"
    );
    assert_eq!(cross.quantity, -beta, "the cross is not the smaller side");
    assert_eq!(order.side, BookSide::Ask);
    assert_eq!(order.quantity, alpha + beta);
    assert_eq!(gateway.placed, vec![(BookSide::Ask, alpha + beta)]);

    // And the hash-chained journal — the record an examiner reads — agrees
    // with the report rather than with the old sign.
    let entry = cell
        .journal()
        .entries()
        .iter()
        .find(|entry| entry.decision.kind() == "crossed_internally")
        .expect("a booked cross is journaled");
    match &entry.decision {
        Decision::CrossedInternally { bought, sold, .. } => {
            assert_eq!(bought, &vec!["alpha".to_string()]);
            assert_eq!(sold, &vec!["beta".to_string()]);
        }
        other => panic!("the journal recorded {other:?} rather than a cross"),
    }
    Ok(())
}
