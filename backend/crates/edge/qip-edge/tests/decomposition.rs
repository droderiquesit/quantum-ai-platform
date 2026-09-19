//! §32.1's size decomposition, driven through the cell rather than asserted
//! on the arithmetic.
//!
//! The failure every test here is about: `Cell::place_cycle` sent every leg
//! of an admitted cycle at the size the scanner priced, whatever the venue
//! had already said about the legs that went out. A first leg that filled
//! six tenths was followed by a second leg at ten tenths, and the difference
//! was an outright position taken at a price chosen for an arbitrage that did
//! not exist at that size.
//!
//! The fixture is the one `tests/arbitrage.rs` uses — a real triangular
//! dislocation seeded through the feed path, with no setter that bypasses it
//! — and the gateways differ only in what the venue answers. Nothing here
//! reaches a deployed process: `qip-edge-node` runs `Cell::work` only under
//! `QIP_VENUE_FEED=simulated`, and `execution_nodes = {}` in all four
//! environments.

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
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer};
use qip_edge::decomposition::DecompositionPolicy;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge::telemetry::EDGE_CYCLE_LEGS;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Labels, Metrics};
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

fn ethereum_graph() -> Result<ArbitrageGraph> {
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        venue(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    let unquoted = Decimal::ONE;
    let fee = d("0.0004");
    for (from, to, market, side) in [
        ("USDT", "ETH", "ETHUSDT", BookSide::Ask),
        ("ETH", "BTC", "ETHBTC", BookSide::Bid),
        ("BTC", "USDT", "BTCUSDT", BookSide::Bid),
    ] {
        graph.add_trade(
            node(from),
            node(to),
            unquoted,
            fee,
            object(market),
            side,
            t(0),
            0,
        )?;
    }
    Ok(graph)
}

fn sizes() -> SizePolicy {
    SizePolicy::uniform(d("10000"))
        .with(object("ETH"), d("3.3"))
        .with(object("BTC"), d("0.16"))
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

fn desk() -> Result<ArbitrageDesk> {
    ArbitrageDesk::new(
        StrategyId::new(DESK),
        OpportunityScanner::new(
            SearchSettings::default(),
            EdgeAssumptions::default(),
            PlanSettings::with_budget(d("50000")),
        ),
        ethereum_graph()?,
        sizes(),
        signed_envelope(DESK)?,
        4,
        Duration::from_secs(30),
    )
}

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

/// A wired cell holding the triangular books, the desk, and `policy`.
///
/// No `VenueModel`, so no lot grid narrows a decomposed leg: these tests are
/// about the fraction, and the grid has its own tests beside the arithmetic.
fn cell_with(policy: DecompositionPolicy) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut config = CellConfig::new(CELL, REGION).with_venue(venue());
    config.decomposition = policy;
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?
        .with_metrics(Arc::clone(&metrics))
        .with_arbitrage(desk()?)?;
    cell.apply_policy(fresh_policy(t(5))?, t(5))?;
    for state in ethereum_books()? {
        cell.track(state);
    }
    Ok((cell, metrics))
}

/// A venue that answers on acceptance, filling a stated numerator over four
/// of whatever it was asked for.
///
/// Quarters rather than an arbitrary decimal so the ratio the test brackets
/// is one a reader can check by eye. A leg with no entry fills whole, which
/// is what a healthy venue does and what the unchanged-behaviour tests need.
#[derive(Debug, Default)]
struct AnsweringGateway {
    placed: Vec<(String, ObjectId, BookSide, Decimal, Decimal)>,
    /// Placement ordinal, 1-based, to quarters filled.
    quarters: BTreeMap<usize, i64>,
    /// Whether the venue answers at all. `false` is the honest behaviour of
    /// a gateway with no order-entry channel, which is the default of
    /// `Placer::execution_reports`.
    answers: bool,
    reports: Vec<ExecutionReport>,
}

impl AnsweringGateway {
    fn answering() -> Self {
        Self {
            answers: true,
            ..Self::default()
        }
    }

    fn filling(mut self, ordinal: usize, quarters: i64) -> Self {
        self.quarters.insert(ordinal, quarters);
        self
    }

    fn silent() -> Self {
        Self::default()
    }
}

impl Placer for AnsweringGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        let ordinal = self.placed.len() + 1;
        self.placed.push((
            order_id.to_string(),
            object_id.clone(),
            side,
            quantity,
            price,
        ));
        if !self.answers {
            return Ok(());
        }
        let quarters = self.quarters.get(&ordinal).copied().unwrap_or(4);
        let filled = quantity
            .checked_mul(Decimal::from_int(quarters))
            .and_then(|scaled| scaled.checked_div(Decimal::from_int(4)))
            .unwrap_or(Decimal::ZERO);
        if filled.is_positive() {
            self.reports.push(ExecutionReport {
                order_id: order_id.to_string(),
                venue: venue.clone(),
                quantity: filled,
                price,
                at,
            });
        }
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }
}

fn legs_counted(metrics: &Metrics, completion: &str) -> u64 {
    let mut labels = Labels::new();
    labels.insert("cell".to_string(), CELL.to_string());
    labels.insert("region".to_string(), REGION.to_string());
    labels.insert("completion".to_string(), completion.to_string());
    metrics.snapshot().counter(EDGE_CYCLE_LEGS, &labels)
}

/// What the gateway was asked to send, in placement order.
fn sent(gateway: &AnsweringGateway) -> Vec<Decimal> {
    gateway.placed.iter().map(|placed| placed.3).collect()
}

#[test]
fn a_cycle_whose_venues_fill_every_leg_whole_is_sent_exactly_as_it_was_admitted() -> Result<()> {
    // The half that distinguishes a mechanism from a new limit. A control
    // that shrank every cycle would pass every test about shrinking and be
    // useless; this is the one that says it does nothing when nothing is
    // wrong.
    let (mut cell, metrics) = cell_with(DecompositionPolicy::default())?;
    let mut gateway = AnsweringGateway::answering();
    let report = cell.work(t(10), &mut gateway)?;

    let scanned = cell
        .arbitrage()
        .expect("the desk was installed")
        .scan(cell.liquidity(), t(10));
    assert_eq!(
        scanned.opportunities.len(),
        1,
        "the premise failed: the seeded books hold {} cycles",
        scanned.opportunities.len()
    );
    let planned: Vec<Decimal> = scanned.opportunities[0]
        .planned
        .plan
        .steps()
        .iter()
        .map(|step| step.quantity)
        .collect();
    assert_eq!(planned.len(), 3, "a triangle has three legs");
    assert_eq!(
        report.orders.len(),
        3,
        "the premise failed: the cycle's legs did not all go out: {report:?}"
    );
    assert_eq!(
        sent(&gateway),
        planned,
        "a cycle every venue filled whole was not sent at the sizes the scanner priced"
    );
    assert_eq!(
        legs_counted(&metrics, "whole"),
        3,
        "three whole legs were not counted as whole"
    );
    assert_eq!(
        legs_counted(&metrics, "short"),
        0,
        "a cycle every venue filled whole reported a leg that filled short"
    );
    Ok(())
}

#[test]
fn a_cycle_whose_first_leg_fills_three_quarters_sends_its_later_legs_at_three_quarters()
-> Result<()> {
    // The mechanism itself. Before it, the second and third legs went out at
    // the sizes the scanner priced, and the quarter of each that the first
    // leg could not support was an outright position.
    let (mut cell, metrics) = cell_with(DecompositionPolicy::default())?;
    let mut gateway = AnsweringGateway::answering().filling(1, 3);
    let report = cell.work(t(10), &mut gateway)?;

    let scanned = cell
        .arbitrage()
        .expect("the desk was installed")
        .scan(cell.liquidity(), t(10));
    let planned: Vec<Decimal> = scanned.opportunities[0]
        .planned
        .plan
        .steps()
        .iter()
        .map(|step| step.quantity)
        .collect();
    assert_eq!(planned.len(), 3, "a triangle has three legs");
    let sent = sent(&gateway);
    assert_eq!(
        sent.len(),
        3,
        "the premise failed: the cycle did not reach the venue three times: {report:?}"
    );
    assert_eq!(
        sent[0], planned[0],
        "the first leg was reduced, and nothing had filled short of it yet"
    );

    // Bracketed rather than recomputed: a test that repeated the
    // implementation's own `planned * filled / sent` would pass against any
    // arithmetic the implementation happened to do, including none.
    for index in 1..3 {
        let low = planned[index]
            .checked_mul(d("0.749"))
            .expect("a representable bound");
        let high = planned[index]
            .checked_mul(d("0.751"))
            .expect("a representable bound");
        assert!(
            sent[index] > low && sent[index] < high,
            "leg {index} went out at {}, and the leg in front of it completed three quarters of \
             {} — it should be about three quarters of its planned {}",
            sent[index],
            planned[0],
            planned[index]
        );
    }

    // The attribution follows the size that was sent, not the planned one:
    // a contributor claiming a size the venue was never asked for would
    // attribute a decomposed leg's fills to a cycle that was never sent.
    for (index, order) in report.orders.iter().enumerate() {
        assert_eq!(
            order.contributors.len(),
            1,
            "a cycle leg carried more than one contributor"
        );
        assert_eq!(
            order.contributors[0].signed_size.abs(),
            sent[index],
            "leg {index} was attributed at a size the venue was never asked for"
        );
    }

    // The series counts a leg by what that leg completed, so one short leg
    // and two that filled everything asked of their reduced size. A label
    // naming the consequence instead would put all three under one word and
    // lose which leg the venue actually came up short on.
    assert_eq!(
        legs_counted(&metrics, "short"),
        1,
        "the leg the venue filled three quarters of was not counted as short"
    );
    assert_eq!(
        legs_counted(&metrics, "whole"),
        2,
        "the two reduced legs the venue filled entirely were not counted as whole"
    );
    let decomposed: Vec<&str> = cell
        .journal()
        .entries()
        .iter()
        .map(|entry| entry.decision.kind())
        .filter(|kind| *kind == "cycle_decomposed")
        .collect();
    assert_eq!(
        decomposed.len(),
        2,
        "the chain does not say why two legs are not the size the cycle was admitted at"
    );
    Ok(())
}

#[test]
fn a_cycle_whose_first_leg_fills_under_the_minimum_viable_fraction_sends_no_further_leg()
-> Result<()> {
    // A quarter is below the default half, so there is no size at which the
    // rest of the cycle is worth completing. The legs already out are a
    // position nobody chose, which is the state the cell halts on.
    let (mut cell, metrics) = cell_with(DecompositionPolicy::default())?;
    let mut gateway = AnsweringGateway::answering().filling(1, 1);
    let outcome = cell.work(t(10), &mut gateway);

    assert!(
        outcome.is_err(),
        "a cycle that could not be completed reported a pass that finished"
    );
    assert_eq!(
        sent(&gateway).len(),
        1,
        "a later leg went out behind a first leg that filled a quarter"
    );
    assert!(
        cell.is_halted(),
        "the cell carried on with a position it did not decide to take"
    );
    assert_eq!(
        legs_counted(&metrics, "unviable"),
        1,
        "the leg that completed too short to continue the cycle was not counted as unviable"
    );
    assert_eq!(
        legs_counted(&metrics, "short"),
        0,
        "a leg below the minimum viable fraction was counted as a viable short fill"
    );
    // The leg that never went out is counted where every other refusal is,
    // and not a second time on the leg series: one stopped cycle must not
    // read as two.
    assert_eq!(
        legs_counted(&metrics, "unviable") as usize,
        sent(&gateway).len(),
        "the leg the cell declined to send was counted on the series for legs it sent"
    );
    Ok(())
}

#[test]
fn a_cycle_at_a_gateway_that_answers_nothing_is_sent_whole_and_says_so_on_the_series() -> Result<()>
{
    // Reading silence as a zero fill would stop every cycle at every gateway
    // with no order-entry channel — which is the documented default of
    // `Placer::execution_reports` and the behaviour of every such gateway in
    // this tree. The silence is counted instead, so a cell that decomposes
    // nothing because nothing ever answers does not read like a cell whose
    // cycles always complete whole.
    let (mut cell, metrics) = cell_with(DecompositionPolicy::default())?;
    let mut gateway = AnsweringGateway::silent();
    let report = cell.work(t(10), &mut gateway)?;

    let scanned = cell
        .arbitrage()
        .expect("the desk was installed")
        .scan(cell.liquidity(), t(10));
    let planned: Vec<Decimal> = scanned.opportunities[0]
        .planned
        .plan
        .steps()
        .iter()
        .map(|step| step.quantity)
        .collect();
    assert_eq!(
        report.orders.len(),
        3,
        "the premise failed: the cycle's legs did not all go out: {report:?}"
    );
    assert_eq!(
        sent(&gateway),
        planned,
        "a cycle at a venue that said nothing was sized down on a fill nobody reported"
    );
    assert_eq!(
        legs_counted(&metrics, "unanswered"),
        3,
        "a cell whose venue answers nothing reads identically to one whose cycles complete whole"
    );
    assert_eq!(
        legs_counted(&metrics, "whole"),
        0,
        "silence was counted as a leg that filled"
    );
    Ok(())
}

#[test]
fn a_desk_that_will_only_carry_a_whole_cycle_stops_on_any_short_leg_at_all() -> Result<()> {
    // The policy is the operator's, and a minimum viable fraction of one is
    // a legitimate setting rather than a misconfiguration: it says this desk
    // completes cycles whole or not at all. Three quarters is admitted under
    // the default and refused under this, on the same books and the same
    // venue answers, which is what proves the policy is read rather than
    // hard-coded.
    let (mut cell, _) = cell_with(DecompositionPolicy::new(Decimal::ONE)?)?;
    let mut gateway = AnsweringGateway::answering().filling(1, 3);
    let outcome = cell.work(t(10), &mut gateway);

    assert!(
        outcome.is_err(),
        "a desk that carries only whole cycles completed one at three quarters"
    );
    assert_eq!(
        sent(&gateway).len(),
        1,
        "a later leg went out at a desk that carries only whole cycles"
    );
    Ok(())
}
