//! The arbitrage desk as the cell runs it: a cycle on the cell's own books
//! becomes its legs as orders in one pass, never through `net`; an
//! infeasible leg vetoes the whole cycle; the cap refuses and counts the
//! excess; a cycle that breaks between legs stops the cell.
//!
//! Every fixture seeds books through the feed path — there is no setter
//! that bypasses it — and the graph the desk is handed carries no rates
//! worth anything: the desk re-quotes it from the books on every pass, so
//! what the cell finds is a fact about the books and not about the
//! template.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

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
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::{Cell, CellConfig, Placer, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::feasibility::{Granularity, VenueModel};
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Metrics, labels, names};
use qip_orderbook::venue::VenueState;
use std::collections::BTreeMap;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "CX";
const DESK: &str = "arb-desk";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn node(object_name: &str) -> Node {
    Node::new(object(object_name), venue())
}

/// A two-sided book for one market, built from feed messages.
fn book(market: &str, bid: (&str, &str), ask: (&str, &str)) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(market), venue(), VenueStatus::Open);
    for (index, (side, (price, size))) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            object(market),
            Origin::new(venue(), "feed-a", 0, index as u64),
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

/// The books of a real triangular dislocation: ETH/BTC is a percent away
/// from what the two dollar legs imply, and the spreads are real.
fn ethereum_books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book("ETHUSDT", ("3000", "200"), ("3000.1", "200"))?,
        book("ETHBTC", ("0.0505", "200"), ("0.05051", "200"))?,
        book("BTCUSDT", ("60000", "10"), ("60001", "10"))?,
    ])
}

/// A second, edge-disjoint dislocation on the same venue.
fn solana_books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book("SOLUSDC", ("150", "200"), ("150.01", "200"))?,
        book("SOLBTC", ("0.002525", "200"), ("0.00253", "200"))?,
        book("BTCUSDC", ("60000", "10"), ("60001", "10"))?,
    ])
}

/// One triangle's three conversions, quoted at a placeholder of one. The
/// graph refuses a template rate of zero, and the desk re-quotes every trade
/// edge from the books before every scan, so the placeholder is a value the
/// scan never reads — the premise assertions below hold that.
fn triangle(
    graph: &mut ArbitrageGraph,
    quote: &str,
    base: &str,
    via: &str,
    base_market: &str,
    cross_market: &str,
    via_market: &str,
) -> Result<()> {
    let unquoted = Decimal::ONE;
    let fee = d("0.0004");
    graph.add_trade(
        node(quote),
        node(base),
        unquoted,
        fee,
        object(base_market),
        BookSide::Ask,
        t(0),
        0,
    )?;
    graph.add_trade(
        node(base),
        node(via),
        unquoted,
        fee,
        object(cross_market),
        BookSide::Bid,
        t(0),
        0,
    )?;
    graph.add_trade(
        node(via),
        node(quote),
        unquoted,
        fee,
        object(via_market),
        BookSide::Bid,
        t(0),
        0,
    )?;
    Ok(())
}

fn ethereum_graph() -> Result<ArbitrageGraph> {
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        venue(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    triangle(
        &mut graph, "USDT", "ETH", "BTC", "ETHUSDT", "ETHBTC", "BTCUSDT",
    )?;
    Ok(graph)
}

fn two_triangle_graph() -> Result<ArbitrageGraph> {
    let mut graph = ethereum_graph()?;
    triangle(
        &mut graph, "USDC", "SOL", "BTC", "SOLUSDC", "SOLBTC", "BTCUSDC",
    )?;
    Ok(graph)
}

/// About ten thousand dollars' worth of whichever instrument a cycle
/// happens to start from.
fn sizes() -> SizePolicy {
    SizePolicy::uniform(d("10000"))
        .with(object("ETH"), d("3.3"))
        .with(object("BTC"), d("0.16"))
        .with(object("SOL"), d("66"))
}

fn scanner() -> OpportunityScanner {
    OpportunityScanner::new(
        SearchSettings::default(),
        EdgeAssumptions::default(),
        PlanSettings::with_budget(d("50000")),
    )
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

fn desk(graph: ArbitrageGraph, cap: usize) -> Result<ArbitrageDesk> {
    ArbitrageDesk::new(
        StrategyId::new(DESK),
        scanner(),
        graph,
        sizes(),
        signed_envelope(DESK)?,
        cap,
        Duration::from_secs(30),
    )
}

/// A payload whose capability slots are fresh, so the sizing multiplier is
/// one and the desk scans rather than refusing to.
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

/// A wired cell holding the given books, a fresh policy, and the desk.
fn cell_with(
    books: Vec<VenueState>,
    desk: ArbitrageDesk,
    model: Option<VenueModel>,
) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut config = CellConfig::new(CELL, REGION).with_venue(venue());
    if let Some(model) = model {
        config = config.with_feasibility(&venue(), model);
    }
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?
        .with_metrics(Arc::clone(&metrics))
        .with_arbitrage(desk)?;
    cell.apply_policy(fresh_policy(t(5))?, t(5))?;
    for state in books {
        cell.track(state);
    }
    Ok((cell, metrics))
}

/// A gateway that records what it was asked to place.
#[derive(Debug, Default)]
struct RecordingGateway {
    placed: Vec<(String, ObjectId, BookSide, Decimal, Decimal)>,
    /// Refuse the placement with this ordinal (1-based), if any.
    refuse_at: Option<usize>,
}

impl Placer for RecordingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        _venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        if self.refuse_at == Some(self.placed.len() + 1) {
            return Err(Error::io("the venue refused the order"));
        }
        self.placed.push((
            order_id.to_string(),
            object_id.clone(),
            side,
            quantity,
            price,
        ));
        Ok(())
    }
}

fn kinds(cell: &Cell) -> Vec<&'static str> {
    cell.journal()
        .entries()
        .iter()
        .map(|entry| entry.decision.kind())
        .collect()
}

fn refusals_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

#[test]
fn a_cycle_on_the_cells_own_books_becomes_its_legs_as_orders_in_one_pass() -> Result<()> {
    // The failure this closes: a scanner constructed by no composition root
    // and a cell that consulted no graph, so every cycle in the tree was
    // found by a test and taken by nobody.
    let (mut cell, _) = cell_with(ethereum_books()?, desk(ethereum_graph()?, 4)?, None)?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;

    // Premise: the desk, scanning the same books after the pass re-quoted
    // its graph, finds exactly the one cycle — so what follows is about the
    // wiring and not about the scanner.
    let desk = cell.arbitrage().expect("the desk was installed");
    let scanned = desk.scan(cell.liquidity(), t(10));
    assert_eq!(
        scanned.opportunities.len(),
        1,
        "the premise failed: the seeded books hold {} cycles: {scanned:?}",
        scanned.opportunities.len()
    );
    let expected = &scanned.opportunities[0];
    let steps = expected.planned.plan.steps();
    assert_eq!(steps.len(), 3, "a triangle has three legs");

    assert_eq!(
        report.orders.len(),
        3,
        "the cycle's legs did not all go out: {report:?}"
    );
    assert_eq!(gateway.placed.len(), 3);
    for (step, (placed, order)) in steps.iter().zip(gateway.placed.iter().zip(&report.orders)) {
        assert_eq!(
            placed.1, step.object_id,
            "a leg went out on the wrong market"
        );
        assert_eq!(placed.2, step.side, "a leg went out on the wrong side");
        assert_eq!(placed.3, step.quantity, "a leg went out at the wrong size");
        assert_eq!(
            placed.4, step.reference_price,
            "a leg went out at the wrong price"
        );
        assert_eq!(order.strategy.as_str(), DESK);
        assert_eq!(
            order.contributors.len(),
            1,
            "a leg carried more than one contributor, which is what netting produces"
        );
        assert_eq!(order.contributors[0].strategy.as_str(), DESK);
    }
    // Nothing directional was in the pass, so nothing was netted: the ratio
    // is unstated rather than 1.0, and nothing cancelled. A leg that had
    // gone through `net` would have produced a ratio.
    assert_eq!(report.netting_ratio, None);
    assert!(report.cancelled.is_empty());

    // The chain: the cycle priced, three orders, and the set committed by
    // name — with the order ids that make it up.
    let recorded = kinds(&cell);
    assert!(recorded.contains(&"edge_priced"), "{recorded:?}");
    assert_eq!(
        recorded.iter().filter(|k| **k == "order_sent").count(),
        3,
        "{recorded:?}"
    );
    let committed = cell
        .journal()
        .entries()
        .iter()
        .find_map(|entry| match &entry.decision {
            qip_edge::journal::Decision::CycleCommitted {
                cycle_id,
                orders,
                net,
            } => Some((cycle_id.clone(), orders.clone(), net.clone())),
            _ => None,
        })
        .expect("the cycle was not committed in the chain");
    assert_eq!(committed.0, expected.cycle_id(t(10)));
    assert_eq!(
        committed.1,
        report
            .orders
            .iter()
            .map(|o| o.order_id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(committed.2, expected.net().to_string());

    // And the desk spent its own envelope, three orders' worth.
    assert_eq!(
        cell.arbitrage()
            .expect("installed")
            .utilisation()
            .orders_sent,
        3
    );
    Ok(())
}

#[test]
fn an_infeasible_leg_vetoes_the_whole_cycle_and_no_leg_goes_out() -> Result<()> {
    // A cycle short one leg is a position, not a smaller cycle. The ETH/BTC
    // leg is worth about a sixth of a bitcoin; a venue minimum of one unit
    // of the quote refuses it, and the two dollar legs — each well above the
    // minimum — must not go out on their own.
    let minimum = VenueModel::new(
        VenueClass::CryptoExchange,
        Granularity::new(d("0.000000001"), d("0.000000001"), Decimal::ZERO)?,
        Decimal::ZERO,
        None,
    )?
    .with_minimum_notional(d("1"))?;

    // Premise: without the model the same books yield the three legs.
    let (mut cell, _) = cell_with(ethereum_books()?, desk(ethereum_graph()?, 4)?, None)?;
    let unmodelled = cell.work(t(10), &mut RecordingGateway::default())?;
    assert_eq!(
        unmodelled.orders.len(),
        3,
        "the premise failed: {unmodelled:?}"
    );

    let (mut cell, metrics) = cell_with(
        ethereum_books()?,
        desk(ethereum_graph()?, 4)?,
        Some(minimum),
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;

    assert!(
        gateway.placed.is_empty(),
        "legs of a vetoed cycle reached the venue: {:?}",
        gateway.placed
    );
    assert!(report.orders.is_empty());
    let leg = refusals_under(&report, "feasibility_minimum_notional");
    assert_eq!(
        leg.len(),
        1,
        "the leg was not refused by the notional rule: {report:?}"
    );
    assert!(leg[0].contains("ETHBTC"), "{}", leg[0]);
    let cycle = refusals_under(&report, "arbitrage_cycle");
    assert_eq!(cycle.len(), 1, "the cycle was not vetoed whole: {report:?}");
    assert!(
        cycle[0].contains("refused whole") && cycle[0].contains("ETHBTC"),
        "{}",
        cycle[0]
    );
    assert!(
        !kinds(&cell).contains(&"cycle_committed"),
        "a vetoed cycle was recorded as committed"
    );
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", "arbitrage_cycle")
            ])
        ),
        1
    );
    Ok(())
}

#[test]
fn the_cap_refuses_the_excess_and_counts_it() -> Result<()> {
    let books = || -> Result<Vec<VenueState>> {
        let mut books = ethereum_books()?;
        books.extend(solana_books()?);
        Ok(books)
    };
    // Premise: with room for both, both cycles go out — six legs.
    let (mut cell, _) = cell_with(books()?, desk(two_triangle_graph()?, 2)?, None)?;
    let roomy = cell.work(t(10), &mut RecordingGateway::default())?;
    assert_eq!(
        roomy.orders.len(),
        6,
        "the premise failed: the two dislocations did not both execute: {roomy:?}"
    );
    assert!(refusals_under(&roomy, "arbitrage_cap").is_empty());

    let (mut cell, metrics) = cell_with(books()?, desk(two_triangle_graph()?, 1)?, None)?;
    let report = cell.work(t(10), &mut RecordingGateway::default())?;
    assert_eq!(
        report.orders.len(),
        3,
        "the cap did not hold at one cycle: {report:?}"
    );
    let capped = refusals_under(&report, "arbitrage_cap");
    assert_eq!(
        capped.len(),
        1,
        "the excess was dropped rather than refused: {report:?}"
    );
    assert!(capped[0].contains("cap is 1"), "{}", capped[0]);
    // Both were priced and journaled; only one was committed.
    let recorded = kinds(&cell);
    assert_eq!(
        recorded.iter().filter(|k| **k == "edge_priced").count(),
        2,
        "{recorded:?}"
    );
    assert_eq!(
        recorded.iter().filter(|k| **k == "cycle_committed").count(),
        1,
        "{recorded:?}"
    );
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", "arbitrage_cap")
            ])
        ),
        1,
        "the cap refusal did not reach the series"
    );
    Ok(())
}

#[test]
fn the_orders_series_moves_by_the_venue_the_legs_went_to() -> Result<()> {
    // The series a scraper reads. Asserted from a snapshot of the registry
    // the composition root would hand over, not from the report.
    let (mut cell, metrics) = cell_with(ethereum_books()?, desk(ethereum_graph()?, 4)?, None)?;
    let before = metrics.snapshot().counter(
        names::EDGE_ORDERS_PLACED,
        &labels([("cell", CELL), ("region", REGION), ("venue", VENUE)]),
    );
    assert_eq!(
        before, 0,
        "the premise failed: orders were counted before any pass"
    );
    let report = cell.work(t(10), &mut RecordingGateway::default())?;
    assert_eq!(report.orders.len(), 3, "the premise failed: {report:?}");
    let after = metrics.snapshot().counter(
        names::EDGE_ORDERS_PLACED,
        &labels([("cell", CELL), ("region", REGION), ("venue", VENUE)]),
    );
    assert_eq!(
        after, 3,
        "three legs went out and the series did not move by three"
    );
    Ok(())
}

#[test]
fn a_cycle_that_breaks_between_legs_halts_the_cell_and_records_the_break() -> Result<()> {
    // The cell cannot cancel, so it cannot unwind. What it can do is refuse
    // to keep trading while it holds a position it did not decide to take.
    let (mut cell, metrics) = cell_with(ethereum_books()?, desk(ethereum_graph()?, 4)?, None)?;
    let mut gateway = RecordingGateway {
        refuse_at: Some(2),
        ..RecordingGateway::default()
    };
    let outcome = cell.work(t(10), &mut gateway);
    assert!(
        outcome.is_err(),
        "the premise failed: the refusing venue did not fail the pass"
    );
    assert_eq!(
        gateway.placed.len(),
        1,
        "the premise failed: the first leg did not go out before the second was refused"
    );
    assert!(
        cell.is_halted(),
        "the cell kept trading with a cycle half on"
    );
    let recorded = kinds(&cell);
    assert!(recorded.contains(&"halt_changed"), "{recorded:?}");
    assert!(
        !recorded.contains(&"cycle_committed"),
        "a broken cycle was recorded as committed: {recorded:?}"
    );
    let broken = cell
        .journal()
        .entries()
        .iter()
        .find_map(|entry| match &entry.decision {
            qip_edge::journal::Decision::Refused { gate, reason }
                if gate == "arbitrage_cycle_broken" =>
            {
                Some(reason.clone())
            }
            _ => None,
        })
        .expect("the break was not journaled");
    assert!(
        broken.contains("after 1 of 3 legs"),
        "the break does not say how far the cycle got: {broken}"
    );
    assert_eq!(
        metrics.snapshot().gauge(
            names::EDGE_HALTED,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("source", "kill_switch")
            ])
        ),
        Some(1.0),
        "the halt did not reach the gauge"
    );

    // Halted is halted: the next pass sends nothing, whatever the books say.
    let mut quiet = RecordingGateway::default();
    let next = cell.work(t(11), &mut quiet)?;
    assert!(next.halted);
    assert!(quiet.placed.is_empty());
    Ok(())
}

#[test]
fn a_desk_whose_graph_reaches_a_venue_the_cell_cannot_is_refused_at_installation() -> Result<()> {
    let mut graph = ArbitrageGraph::new();
    let elsewhere = VenueId::new("XNYS");
    graph.register_venue(
        elsewhere.clone(),
        VenueFacts::new(VenueClass::Exchange, VenueStatus::Open),
    );
    graph.add_trade(
        Node::new(object("USD"), elsewhere.clone()),
        Node::new(object("ACME"), elsewhere.clone()),
        Decimal::ONE,
        Decimal::ZERO,
        object("ACME"),
        BookSide::Ask,
        t(0),
        0,
    )?;
    graph.add_trade(
        Node::new(object("ACME"), elsewhere.clone()),
        Node::new(object("USD"), elsewhere),
        Decimal::ONE,
        Decimal::ZERO,
        object("ACME"),
        BookSide::Bid,
        t(0),
        0,
    )?;
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let refusal = Cell::new(config, features)?
        .with_arbitrage(desk(graph, 1)?)
        .expect_err("a graph through XNYS was installed in a cell that trades only CX");
    assert!(refusal.message().contains("XNYS"), "{}", refusal.message());

    // And the desk itself refuses a cap that admits nothing.
    assert!(
        desk(ethereum_graph()?, 0).is_err(),
        "a cap of zero built a desk that refuses everything it finds"
    );
    Ok(())
}
