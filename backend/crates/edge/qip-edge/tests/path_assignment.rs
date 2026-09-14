//! Blueprint §30.2's path router as the cell actually runs it (ADR 0068).
//!
//! The router landed with no production caller at all. A capability nobody
//! calls is one nobody can rely on, and the shape it fails in is specific:
//! the crate's own tests keep passing, the acceptance suite keeps proving the
//! vocabulary matches the blueprint, and the platform never assigns a path to
//! anything. These tests are about the seam, not the vocabulary — every one
//! of them drives a real `Cell::work` pass over real books and asserts that
//! what the router decided reached a surface somebody can read.
//!
//! Three surfaces, and each is asserted separately because they fail
//! separately: the pass report, the hash-chained journal, and — for a refusal
//! — `qip_edge_refusals_total{gate="path_router"}`.
//!
//! What is deliberately **not** asserted here is that the assignment changes
//! what the cell sends. It does not, and it must not: §30.2's assignment is a
//! classification, `Cell::send` remains the one place a `Placer` is called,
//! and the router names no venue. The one thing an assignment can do to an
//! order is stop it existing, which is what the refusal test below holds.

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
use qip_edge::cell::{Cell, CellConfig, GATE_PATH_ROUTER, Placer, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Metrics, labels, names};
use qip_orderbook::venue::VenueState;
use qip_routing::path::ExecutionPath;
use std::collections::BTreeMap;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "CX";
/// The second venue, in the **same** region as the first. That it is the same
/// region is the whole point of the two-venue test: §30.2 tells row 2 from
/// rows 3 to 6 by comparing regions, and a cell places every venue it may
/// trade in its own.
const VENUE_TWO: &str = "DX";
const DESK: &str = "arb-desk";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-tests";

/// How many observations a transfer edge declares.
///
/// A transfer's rate is one by definition — the same asset moved, not
/// converted — so there is no book to observe it from and `ArbitrageDesk`
/// deliberately never re-quotes one. The net-edge calculator haircuts a cycle
/// by its *fewest*-observed edge, so a transfer left at zero would haircut
/// every cross-venue cycle to nothing on uncertainty about a rate that is not
/// uncertain. Stated here rather than buried as a literal because it is the
/// reason this fixture finds a cycle at all.
const TRANSFER_OBSERVATIONS: u32 = 512;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn venue_two() -> VenueId {
    VenueId::new(VENUE_TWO)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

/// A two-sided book for one market at one venue, built from feed messages.
/// There is no setter that bypasses the feed path, deliberately.
fn book_at(at: &VenueId, market: &str, bid: (&str, &str), ask: (&str, &str)) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(market), at.clone(), VenueStatus::Open);
    for (index, (side, (price, size))) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            object(market),
            Origin::new(at.clone(), "feed-a", 0, index as u64),
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

fn book(market: &str, bid: (&str, &str), ask: (&str, &str)) -> Result<VenueState> {
    book_at(&venue(), market, bid, ask)
}

/// The books of a real triangular dislocation at one venue: ETH/BTC is a
/// percent away from what the two dollar legs imply.
fn one_venue_books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book("ETHUSDT", ("3000", "200"), ("3000.1", "200"))?,
        book("ETHBTC", ("0.0505", "200"), ("0.05051", "200"))?,
        book("BTCUSDT", ("60000", "10"), ("60001", "10"))?,
    ])
}

/// A triangle's three conversions at one venue, quoted at a placeholder of
/// one. The desk re-quotes every trade edge from the books before every scan,
/// so the placeholder is a number the scan never reads.
fn one_venue_graph() -> Result<ArbitrageGraph> {
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        venue(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    let node = |name: &str| Node::new(object(name), venue());
    let fee = d("0.0004");
    graph.add_trade(
        node("USDT"),
        node("ETH"),
        Decimal::ONE,
        fee,
        object("ETHUSDT"),
        BookSide::Ask,
        t(0),
        0,
    )?;
    graph.add_trade(
        node("ETH"),
        node("BTC"),
        Decimal::ONE,
        fee,
        object("ETHBTC"),
        BookSide::Bid,
        t(0),
        0,
    )?;
    graph.add_trade(
        node("BTC"),
        node("USDT"),
        Decimal::ONE,
        fee,
        object("BTCUSDT"),
        BookSide::Bid,
        t(0),
        0,
    )?;
    Ok(graph)
}

/// Books for the same instrument at two venues, dislocated: the second venue
/// bids well above what the first offers.
fn two_venue_books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book_at(&venue(), "BTCUSDT", ("59999", "10"), ("60000", "10"))?,
        book_at(&venue_two(), "BTCUSDT", ("66000", "10"), ("66001", "10"))?,
    ])
}

/// Buy BTC at one venue, move it, sell it at the other, move the cash back.
/// Two trade edges and two transfer edges; the transfers are what make this
/// §30.2's row 2 rather than its row 1.
fn two_venue_graph() -> Result<ArbitrageGraph> {
    let mut graph = ArbitrageGraph::new();
    for at in [venue(), venue_two()] {
        graph.register_venue(
            at,
            VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
        );
    }
    graph.add_trade(
        Node::new(object("USDT"), venue()),
        Node::new(object("BTC"), venue()),
        Decimal::ONE,
        Decimal::ZERO,
        object("BTCUSDT"),
        BookSide::Ask,
        t(0),
        0,
    )?;
    graph.add_transfer(
        object("BTC"),
        venue(),
        venue_two(),
        Decimal::ZERO,
        t(1),
        TRANSFER_OBSERVATIONS,
    )?;
    graph.add_trade(
        Node::new(object("BTC"), venue_two()),
        Node::new(object("USDT"), venue_two()),
        Decimal::ONE,
        Decimal::ZERO,
        object("BTCUSDT"),
        BookSide::Bid,
        t(0),
        0,
    )?;
    graph.add_transfer(
        object("USDT"),
        venue_two(),
        venue(),
        Decimal::ZERO,
        t(1),
        TRANSFER_OBSERVATIONS,
    )?;
    Ok(graph)
}

/// A ring of `edges` conversions at one venue, every hop its own market.
///
/// One hop is offered at nine tenths and the rest at parity, so the ring is
/// profitable however long it is — which is what lets the length, and only
/// the length, be the thing under test.
fn ring(edges: usize) -> Result<(ArbitrageGraph, Vec<VenueState>)> {
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        venue(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    let mut books = Vec::with_capacity(edges);
    for hop in 0..edges {
        let market = format!("M{hop}");
        let (bid, ask) = if hop == 0 {
            ("0.89", "0.9")
        } else {
            ("0.99", "1")
        };
        books.push(book(&market, (bid, "1000000"), (ask, "1000000"))?);
        graph.add_trade(
            Node::new(object(&format!("O{hop}")), venue()),
            Node::new(object(&format!("O{}", (hop + 1) % edges)), venue()),
            Decimal::ONE,
            Decimal::ZERO,
            object(&market),
            BookSide::Ask,
            t(0),
            0,
        )?;
    }
    Ok((graph, books))
}

fn sizes() -> SizePolicy {
    SizePolicy::uniform(d("10000"))
        .with(object("ETH"), d("3.3"))
        .with(object("BTC"), d("0.16"))
}

fn scanner(max_cycle_edges: usize) -> OpportunityScanner {
    OpportunityScanner::new(
        SearchSettings {
            max_cycle_edges,
            ..SearchSettings::default()
        },
        EdgeAssumptions::default(),
        PlanSettings::with_budget(d("500000")),
    )
}

fn signed_envelope(venues: Vec<VenueId>) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(DESK),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            venues.clone(),
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

fn desk(
    graph: ArbitrageGraph,
    venues: Vec<VenueId>,
    max_cycle_edges: usize,
) -> Result<ArbitrageDesk> {
    ArbitrageDesk::new(
        StrategyId::new(DESK),
        scanner(max_cycle_edges),
        graph,
        sizes(),
        signed_envelope(venues)?,
        8,
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

fn cell_with(
    books: Vec<VenueState>,
    desk: ArbitrageDesk,
    venues: Vec<VenueId>,
) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut config = CellConfig::new(CELL, REGION);
    for at in venues {
        config = config.with_venue(at);
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

#[derive(Debug, Default)]
struct RecordingGateway {
    placed: Vec<String>,
}

impl Placer for RecordingGateway {
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

#[test]
fn a_cycle_the_cell_finds_is_assigned_an_execution_path_and_the_pass_report_carries_it()
-> Result<()> {
    // The failure this closes: §30.2's router built, tested, and called by
    // nothing, so every path in the tree was assigned by a test and by no
    // pass. If the call site in `Cell::scan_cycles` is deleted, this fails.
    let venues = vec![venue()];
    let (mut cell, _) = cell_with(
        one_venue_books()?,
        desk(one_venue_graph()?, venues.clone(), 4)?,
        venues,
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;

    // Premise, in two halves. The scan really found a cycle — otherwise an
    // empty `paths` would be indistinguishable from a router nobody called —
    // and the cycle really was taken, so this is the live path and not a
    // classification of something the cell then refused.
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
    assert_eq!(
        report.orders.len(),
        3,
        "the premise failed: the cycle's legs did not go out, so this is not the live path"
    );

    assert_eq!(
        report.paths.len(),
        1,
        "the pass found a cycle and reported no path for it: {:?}",
        report.paths
    );
    let routed = &report.paths[0];
    // A triangle at one venue is three conversion edges and nothing else,
    // which is §30.2's row 1 exactly. Asserted as the enum *and* the row
    // number: the number is what an operator reads beside the blueprint, and
    // the two have already been allowed to disagree once in this tree.
    assert_eq!(routed.path(), ExecutionPath::IntraVenue);
    assert_eq!(routed.assignment.assigned().number(), 1);
    // And nothing else was eligible, so the assignment is the table's and not
    // the preference's. A test that only asserted the assignment would pass
    // just as well if the router had found four paths eligible and the
    // default ranking had happened to pick this one.
    assert_eq!(
        routed.assignment.eligible().len(),
        1,
        "a one-venue triangle admitted more than the one row §30.2 gives it: {:?}",
        routed.assignment.eligible()
    );
    assert!(
        routed
            .assignment
            .eligible()
            .contains(&ExecutionPath::IntraVenue)
    );

    // The cycle the report names is the cycle the cell acted on, not some
    // other one: the journal's own priced entry carries the same identifier.
    let priced: Vec<&str> = cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::EdgePriced { opportunity, .. } => Some(opportunity.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(priced, vec![routed.cycle_id.as_str()]);

    // The boundary, stated as an assertion rather than a comment: assigning a
    // path is a classification and changes nothing about what was sent.
    assert_eq!(gateway.placed.len(), 3);
    assert!(
        refusals_under(&report, GATE_PATH_ROUTER).is_empty(),
        "a routable cycle was refused by the router"
    );
    Ok(())
}

#[test]
fn the_path_the_cell_assigned_is_hash_chained_into_its_own_journal() -> Result<()> {
    // The report is returned to the caller and dropped with the pass. "Why
    // was this cycle executed the way it was" is an incident question, asked
    // afterwards, and the journal is the only thing that can answer it — so
    // the assignment goes on the chain and not only into the report.
    let venues = vec![venue()];
    let (mut cell, _) = cell_with(
        one_venue_books()?,
        desk(one_venue_graph()?, venues.clone(), 4)?,
        venues,
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    // Premise: a path really was assigned this pass.
    assert_eq!(report.paths.len(), 1);

    let assigned: Vec<(&str, u8, &str, &Vec<String>, &str)> = cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::CyclePathAssigned {
                cycle_id,
                path,
                path_name,
                eligible,
                rationale,
            } => Some((
                cycle_id.as_str(),
                *path,
                path_name.as_str(),
                eligible,
                rationale.as_str(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        assigned.len(),
        1,
        "the pass assigned a path and the chain does not hold it"
    );
    let (cycle_id, number, name, eligible, rationale) = assigned[0];
    assert_eq!(cycle_id, report.paths[0].cycle_id);
    assert_eq!(number, 1);
    assert_eq!(name, "intra_venue");
    assert_eq!(eligible, &vec!["intra_venue".to_string()]);
    // The rationale is the router's own, not a string this cell composed, and
    // it must name the coordination model §31 gives the row — that is the
    // fact an operator reading the chain beside the blueprint needs.
    assert!(
        rationale.contains("coordination unilateral"),
        "the chained rationale does not carry §31's coordination column: {rationale}"
    );

    // And the chain still verifies with the new entry in it. A decision
    // variant that broke the digest would make every cell's record
    // unreplayable, which is a worse failure than the one this record closes.
    assert_eq!(
        cell.journal().verify(),
        Ok(()),
        "the chain does not verify with a path assignment in it"
    );
    Ok(())
}

#[test]
fn a_cycle_across_two_venues_of_one_region_is_assigned_path_two_and_never_a_cross_region_path()
-> Result<()> {
    // ADR 0068's central claim is that the arbitrage graph cannot tell a
    // venue hop inside one region from the same hop across an ocean, and
    // that the difference decides between row 2 — parallel dispatch inside
    // one process, on a 5-to-15 millisecond budget — and rows 3 to 6, whose
    // budgets run to minutes. This is the cell supplying the missing fact.
    //
    // It is also the honest bound on what a cell can reach: `VenueRegions`
    // is built from the cell's own region and its own venue list, so every
    // venue is local and a transfer is always a transport edge. Rows 3 to 6
    // need a cross-region whitelist, which is §31.1's work and does not
    // exist. That is asserted below rather than left as prose.
    let venues = vec![venue(), venue_two()];
    let (mut cell, _) = cell_with(
        two_venue_books()?,
        desk(two_venue_graph()?, venues.clone(), 4)?,
        venues,
    )?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;

    // Premise: the two-venue dislocation really was found.
    let scanned = cell
        .arbitrage()
        .expect("the desk was installed")
        .scan(cell.liquidity(), t(10));
    assert_eq!(
        scanned.opportunities.len(),
        1,
        "the premise failed: the two-venue books hold {} cycles",
        scanned.opportunities.len()
    );
    assert_eq!(
        report.paths.len(),
        1,
        "a two-venue cycle was found and assigned no path: {:?}",
        report.refusals
    );

    let routed = &report.paths[0];
    assert_eq!(routed.path(), ExecutionPath::CrossVenue);
    assert_eq!(routed.assignment.assigned().number(), 2);
    // The half that carries the meaning: none of rows 3 to 6 was even
    // eligible. A cell whose region map had made the second venue foreign
    // would have produced mirror edges here, and a mirror edge with no facts
    // is refused rather than assigned — so this asserts both that the cell
    // supplied the region and that it supplied the right one.
    for remote in [
        ExecutionPath::MirroredInventory,
        ExecutionPath::HedgedBridging,
        ExecutionPath::PassiveAnchoring,
        ExecutionPath::FirmQuoteBridging,
    ] {
        assert!(
            !routed.assignment.eligible().contains(&remote),
            "a cycle inside one region was eligible for the cross-region path {remote:?}"
        );
    }
    Ok(())
}

#[test]
fn a_cycle_longer_than_the_routers_bound_is_refused_whole_and_no_leg_of_it_reaches_the_gateway()
-> Result<()> {
    // The gate has to be able to fire, or it is the `MaxExpectedShortfall`
    // shape: a control that reads as protection and cannot. This is the one
    // input a cell's own configuration can present that §30.2 assigns no
    // path to — a cycle past `MAX_COMPOSITION_EDGES`, which a desk reaches
    // by setting the search's `max_cycle_edges` above eight.
    //
    // Nine edges rather than eight, and the eight-edge ring below is the
    // other half: a bound that refused everything would pass the first half
    // of this test and mean nothing.
    let venues = vec![venue()];
    let (graph, books) = ring(9)?;
    let (mut cell, metrics) = cell_with(books, desk(graph, venues.clone(), 9)?, venues.clone())?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;

    // Premise: the scan really surfaced the nine-edge cycle, so the refusal
    // below is the router's and not the scanner finding nothing.
    let scanned = cell
        .arbitrage()
        .expect("the desk was installed")
        .scan(cell.liquidity(), t(10));
    assert_eq!(
        scanned.opportunities.len(),
        1,
        "the premise failed: the nine-hop ring produced {} opportunities",
        scanned.opportunities.len()
    );
    assert_eq!(scanned.opportunities[0].candidate.edges.len(), 9);

    let refused = refusals_under(&report, GATE_PATH_ROUTER);
    assert_eq!(
        refused.len(),
        1,
        "a cycle the router cannot assign a path to was not refused under {GATE_PATH_ROUTER}: \
         {:?}",
        report.refusals
    );
    assert!(
        refused[0].contains("exceeds the 8"),
        "the refusal does not say what was wrong: {}",
        refused[0]
    );
    assert!(
        report.paths.is_empty(),
        "a refused cycle was reported as assigned a path"
    );
    // The consequence, which is the point: nothing of that cycle was sent.
    assert!(
        gateway.placed.is_empty(),
        "legs of an unassignable cycle reached the venue: {:?}",
        gateway.placed
    );
    // And the refusal went through `Cell::refuse` like every other gate, so
    // it is counted at the one pass-time recording site and adds no second
    // seam to `qip_edge_refusals_total{gate}`.
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", GATE_PATH_ROUTER)
            ])
        ),
        1
    );

    // The other half of the gate: eight edges is admitted. Without this the
    // test above would pass against a router that refused every cycle.
    let (graph, books) = ring(8)?;
    let (mut cell, _) = cell_with(books, desk(graph, venues.clone(), 8)?, venues)?;
    let report = cell.work(t(10), &mut RecordingGateway::default())?;
    assert!(
        refusals_under(&report, GATE_PATH_ROUTER).is_empty(),
        "an eight-edge cycle was refused by a router bounded at eight: {:?}",
        report.refusals
    );
    assert_eq!(
        report.paths.len(),
        1,
        "an eight-edge ring at one venue was assigned no path: {:?}",
        report.refusals
    );
    assert_eq!(report.paths[0].path(), ExecutionPath::IntraVenue);
    Ok(())
}
