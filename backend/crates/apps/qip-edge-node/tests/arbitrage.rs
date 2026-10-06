//! The arbitrage desk, installed into the assembled cell from the payload's
//! whitelist.
//!
//! The scanner was wired into the cell and no composition root gave a cell a
//! desk, because the whitelist carried strings. These tests drive the node's
//! installer with a signed payload whose whitelist carries conversions, a
//! verified grant for the desk's strategy, and the assembled cell — and
//! prove what installs and what is refused. That a payload signed before the
//! structured whitelist existed still verifies is held beside the verifier,
//! in `qip-edge/tests/whitelist.rs`.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{
    BeliefPriors, CausalDigest, CycleWhitelist, EpisodicDigest, PolicyPayload, Slot,
    WhitelistedConversion,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, SystemClock, Timestamp, dec};
use qip_edge::cell::{CellConfig, Placer, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge_node::allocation::RegionCapital;
use qip_edge_node::arbitrage::{
    ArbitrageInstaller, Installation, MAX_LEGS_VARIABLE, graph_from_whitelist, parse_max_legs,
    registry_from_books,
};
use qip_edge_node::{NodeAssembly, assemble};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use std::collections::BTreeMap;
use std::sync::Arc;

const KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "CX";
const DESK: &str = "arbitrage-desk";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn assembled() -> NodeAssembly {
    let config = CellConfig::new(CELL, REGION).with_venue(VenueId::new(VENUE));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    // Far above any grant this suite signs, so the desk's own gates decide.
    let allocation = RegionCapital::read(Some("1000000000")).expect("a positive amount");
    assemble(config, features, Arc::new(SystemClock), allocation, None)
        .expect("a well-formed cell assembles")
}

fn signed_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
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
    let unsigned = build("unsigned")?;
    let signature = sign_payload(KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, KEY, CELL, t(1))
}

fn conversion(
    venue: &str,
    market: &str,
    from: &str,
    to: &str,
    side: BookSide,
) -> WhitelistedConversion {
    WhitelistedConversion {
        venue: venue.to_string(),
        venue_class: VenueClass::CryptoExchange,
        market: market.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        side,
        cost_fraction: dec!("0.0004"),
    }
}

/// The ETH/BTC/USDT triangle at one venue, sized at every start.
fn triangle(venue: &str) -> CycleWhitelist {
    CycleWhitelist {
        cycles: BTreeMap::from([("eth-triangle".to_string(), "1".to_string())]),
        conversions: vec![
            conversion(venue, "ETHUSDT", "USDT", "ETH", BookSide::Ask),
            conversion(venue, "ETHBTC", "ETH", "BTC", BookSide::Bid),
            conversion(venue, "BTCUSDT", "BTC", "USDT", BookSide::Bid),
        ],
        start_sizes: BTreeMap::from([
            ("USDT".to_string(), dec!("10000")),
            ("ETH".to_string(), dec!("3.3")),
            ("BTC".to_string(), dec!("0.16")),
        ]),
    }
}

/// A payload whose capability slots are fresh (so the cell is not degraded)
/// and whose whitelist is `whitelist`, when one is given.
fn policy(
    sequence: u64,
    issued_at: Timestamp,
    fresh_capabilities: bool,
    whitelist: Option<CycleWhitelist>,
) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, issued_at);
    if fresh_capabilities {
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
    }
    if let Some(whitelist) = whitelist {
        payload.cycle_whitelist = Slot::produced(whitelist, issued_at);
    }
    VerifiedPolicy::verify(payload.signed(KEY)?, KEY, CELL, issued_at)
}

fn installer() -> ArbitrageInstaller {
    ArbitrageInstaller::new(StrategyId::new(DESK), vec![VenueId::new(VENUE)])
}

#[test]
fn the_node_installs_a_desk_from_the_payloads_whitelist_once_capital_for_it_has_arrived()
-> Result<()> {
    let mut node = assembled();
    let mut installer = installer();
    assert!(
        node.cell.arbitrage().is_none(),
        "the premise is a cell with no desk"
    );

    // Neither input yet: nothing installs, and the reason is named.
    assert_eq!(
        installer.install(&mut node.cell, t(10)),
        Installation::NoWhitelist
    );

    node.cell
        .apply_policy(policy(1, t(10), true, Some(triangle(VENUE)))?, t(10))?;
    assert_eq!(
        installer.install(&mut node.cell, t(11)),
        Installation::NoEnvelope,
        "a whitelist with no grant behind it installed a desk with no capital"
    );

    installer.offer(signed_envelope(DESK)?)?;
    assert_eq!(
        installer.install(&mut node.cell, t(12)),
        Installation::Installed(3)
    );
    let desk = node.cell.arbitrage().expect("the desk is installed");
    assert_eq!(desk.strategy().as_str(), DESK);
    assert_eq!(
        desk.graph().edge_count(),
        3,
        "the three conversions are the three edges"
    );
    assert!(
        desk.graph()
            .edges()
            .iter()
            .all(|edge| edge.from.venue.as_str() == VENUE && edge.to.venue.as_str() == VENUE),
        "an edge reaches a venue other than the whitelist's"
    );
    // Spent, and a second attempt does not build a second desk.
    assert!(
        !installer.holds_envelope(),
        "the grant was kept after the desk spent it"
    );
    assert_eq!(
        installer.install(&mut node.cell, t(13)),
        Installation::AlreadyInstalled
    );
    Ok(())
}

#[test]
fn a_whitelist_naming_a_venue_outside_the_configured_list_is_refused_and_installs_nothing()
-> Result<()> {
    let mut node = assembled();
    let mut installer = installer();
    installer.offer(signed_envelope(DESK)?)?;
    // Premise: the same whitelist at the configured venue would install, so
    // what refuses below is the venue and not the shape.
    assert!(
        graph_from_whitelist(&triangle(VENUE), &[VenueId::new(VENUE)]).is_ok(),
        "the premise failed: the fixture whitelist does not build at its own venue"
    );

    let mut foreign = triangle(VENUE);
    foreign.conversions[1].venue = "ZZZ".to_string();
    node.cell
        .apply_policy(policy(1, t(10), true, Some(foreign))?, t(10))?;
    let outcome = installer.install(&mut node.cell, t(11));
    match &outcome {
        Installation::Refused(reason) => assert!(
            reason.contains("ZZZ") && reason.contains("conversion 1"),
            "the refusal names neither the venue nor the entry: {reason}"
        ),
        other => panic!("a whitelist naming an unknown venue was not refused: {other:?}"),
    }
    assert!(
        node.cell.arbitrage().is_none(),
        "a desk was installed through a refused whitelist"
    );
    // The grant is kept for a whitelist that does not name a venue this cell
    // cannot reach; refusing the whitelist is not refusing the capital.
    assert!(
        installer.holds_envelope(),
        "a refused whitelist discarded the desk's grant"
    );
    Ok(())
}

#[test]
fn a_degraded_cell_and_an_empty_whitelist_install_no_desk() -> Result<()> {
    let mut node = assembled();
    let mut installer = installer();
    installer.offer(signed_envelope(DESK)?)?;

    // A whitelist with nothing else produced: every capability is
    // unavailable, the multiplier is at its floor, and a desk would refuse
    // to scan on every pass.
    node.cell
        .apply_policy(policy(1, t(10), false, Some(triangle(VENUE)))?, t(10))?;
    assert!(
        node.cell.narrowing(t(11)).sizing_multiplier() < Decimal::ONE,
        "the premise failed: the cell is not degraded"
    );
    assert_eq!(
        installer.install(&mut node.cell, t(11)),
        Installation::Degraded
    );
    assert!(node.cell.arbitrage().is_none());

    // Fresh capabilities but a whitelist with conversions stripped: the
    // string map alone is not a graph.
    let mut bare = triangle(VENUE);
    bare.conversions.clear();
    node.cell
        .apply_policy(policy(2, t(20), true, Some(bare))?, t(20))?;
    assert_eq!(
        installer.install(&mut node.cell, t(21)),
        Installation::EmptyWhitelist
    );
    assert!(node.cell.arbitrage().is_none());

    // And a whitelist whose slot has gone stale reads as none: a desk built
    // from a whitelist the centre stopped republishing prices a graph it
    // may have withdrawn.
    node.cell
        .apply_policy(policy(3, t(30), true, Some(triangle(VENUE)))?, t(30))?;
    assert_eq!(
        installer.install(&mut node.cell, t(30 + 3600)),
        Installation::NoWhitelist,
        "a stale whitelist was read as fresh"
    );
    Ok(())
}

#[test]
fn a_grant_for_another_strategy_is_refused_by_the_installer_rather_than_held() -> Result<()> {
    let mut installer = installer();
    let outcome = installer.offer(signed_envelope("momentum-9")?);
    assert!(
        outcome.is_err(),
        "a grant for another strategy was held for the desk"
    );
    assert!(!installer.holds_envelope());
    Ok(())
}

/// REFLEX-027's chain in one run: the signed whitelist's conversions become
/// the graph, the desk re-quotes it from books, and the scanner returns an
/// opportunity. The node tests above stop at installation and the scanner's
/// own tests never use a whitelist, so a whitelist the scanner could not
/// walk would have passed both.
#[test]
fn a_whitelisted_triangle_becomes_an_opportunity_once_its_books_are_read() -> Result<()> {
    use qip_arbitrage::liquidity::StaticLiquidity;
    use qip_arbitrage::scan::RejectionStage;
    use qip_core::ObjectId;
    use qip_edge_node::arbitrage::desk_from_whitelist;

    let venue = VenueId::new(VENUE);
    let mut books = StaticLiquidity::new();
    for (market, bid, ask, size) in [
        ("ETHUSDT", dec!("3000.0"), dec!("3000.1"), dec!("200")),
        ("ETHBTC", dec!("0.0505"), dec!("0.05051"), dec!("200")),
        ("BTCUSDT", dec!("60000"), dec!("60001"), dec!("10")),
    ] {
        books = books.with_quote(
            venue.clone(),
            ObjectId::from_string(market),
            t(0),
            bid,
            size,
            ask,
            size,
            20,
        );
    }

    let mut desk = desk_from_whitelist(
        &triangle(VENUE),
        std::slice::from_ref(&venue),
        StrategyId::new(DESK),
        signed_envelope(DESK)?,
    )?;
    // Premise: the whitelist produced a graph, and before any book is read
    // the placeholder rates are no opportunity (a rate of one all round).
    assert_eq!(desk.graph().edge_count(), 3);
    assert!(desk.scan(&books, t(1)).opportunities.is_empty());

    assert_eq!(desk.refresh(&books)?.repriced, 3);
    let report = desk.scan(&books, t(1));
    assert_eq!(report.opportunities.len(), 1, "{:?}", report.rejections);
    let opportunity = &report.opportunities[0];
    assert_eq!(opportunity.planned.plan.len(), 3);
    assert!(opportunity.net() > Decimal::ZERO);
    assert!(report.rejected_at(RejectionStage::NetEdge).is_empty());
    Ok(())
}

// --- MESH-010: the maximum leg count in force is configuration ---------------

/// A two-sided book for one market at the venue, built from feed messages,
/// because the cell has no setter that bypasses the feed path.
fn book(market: &str, bid: (&str, &str), ask: (&str, &str)) -> Result<VenueState> {
    let at = VenueId::new(VENUE);
    let object = ObjectId::from_string(market);
    let mut state = VenueState::aggregated(object.clone(), at.clone(), VenueStatus::Open);
    for (index, (side, (price, size))) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            object.clone(),
            Origin::new(at.clone(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side,
                price: Decimal::parse(price).expect("a decimal literal"),
                quantity: Decimal::parse(size).expect("a decimal literal"),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    Ok(state)
}

/// Books on which the whitelisted triangle pays: the ETH/BTC cross is a
/// percent away from the two dollar legs that imply it.
fn dislocated_books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book("ETHUSDT", ("3000.0", "200"), ("3000.1", "200"))?,
        book("ETHBTC", ("0.0505", "200"), ("0.05051", "200"))?,
        book("BTCUSDT", ("60000", "10"), ("60001", "10"))?,
    ])
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

/// A node whose desk was installed under the leg limit `configured`, the way
/// the composition root installs it, with the triangle's books tracked.
fn node_with_max_legs(configured: Option<&str>) -> Result<NodeAssembly> {
    let mut node = assembled();
    let mut installer = installer().with_search(parse_max_legs(configured)?);
    node.cell
        .apply_policy(policy(1, t(10), true, Some(triangle(VENUE)))?, t(10))?;
    installer.offer(signed_envelope(DESK)?)?;
    assert_eq!(
        installer.install(&mut node.cell, t(12)),
        Installation::Installed(3),
        "the premise is an installed desk"
    );
    for state in dislocated_books()? {
        node.cell.track(state);
    }
    Ok(node)
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
fn the_configured_maximum_leg_count_is_refused_outside_two_to_twenty_and_unset_is_the_engine_default()
 {
    // Refused, never lowered to the ceiling: a node asked for thirty legs
    // that quietly ran twenty would be running a limit nobody chose.
    assert_eq!(
        parse_max_legs(None).unwrap(),
        qip_arbitrage::SearchSettings::default()
    );
    assert_eq!(
        parse_max_legs(Some("  ")).unwrap(),
        qip_arbitrage::SearchSettings::default()
    );
    for (value, legs) in [("2", 2), ("5", 5), (" 20 ", 20)] {
        assert_eq!(parse_max_legs(Some(value)).unwrap().max_cycle_edges, legs);
    }
    for value in ["0", "1", "21", "30", "five", "-3", "4.5"] {
        let refused = parse_max_legs(Some(value)).unwrap_err();
        assert!(
            refused.message().contains(MAX_LEGS_VARIABLE),
            "the refusal of {value} does not name the variable: {}",
            refused.message()
        );
    }
    let over = parse_max_legs(Some("21")).unwrap_err();
    assert!(
        over.message().contains("2 to 20"),
        "the refusal does not name the range: {}",
        over.message()
    );
}

#[test]
fn a_node_configured_for_two_legs_refuses_the_three_leg_cycle_its_desk_finds_naming_the_limit()
-> Result<()> {
    // What this holds is the wiring: the value the composition root read is
    // the maximum the installed desk scans under. That an over-length cycle
    // sends nothing is held where a fixture can send one,
    // `qip-edge/tests/path_assignment.rs`; this assembled node's region
    // table is unfunded, so no cycle is sent here whatever its length and an
    // empty gateway would prove nothing.
    const GATE_SCAN_LENGTH: &str = "arbitrage_scan_length";

    // Premise: the same node configured for three legs finds the triangle
    // and assigns it a path. So what stops the cycle below at the scan is
    // the configured maximum, not the books, the grant or the whitelist.
    let mut within = node_with_max_legs(Some("3"))?;
    let report = within.cell.work(t(20), &mut RecordingGateway::default())?;
    assert!(
        refusals_under(&report, GATE_SCAN_LENGTH).is_empty(),
        "a three-leg cycle was refused as too long at a maximum of three: {:?}",
        report.refusals
    );
    assert_eq!(
        report.paths.len(),
        1,
        "the premise failed, the triangle was not found and routed: {:?}",
        report.refusals
    );

    let mut node = node_with_max_legs(Some("2"))?;
    let mut gateway = RecordingGateway::default();
    let report = node.cell.work(t(20), &mut gateway)?;
    let refused = refusals_under(&report, GATE_SCAN_LENGTH);
    assert_eq!(
        refused.len(),
        1,
        "the three-leg cycle was not refused under {GATE_SCAN_LENGTH}: {:?}",
        report.refusals
    );
    assert!(
        refused[0].contains("3 legs") && refused[0].contains("maximum of 2"),
        "the refusal does not name the limit: {}",
        refused[0]
    );
    assert!(
        report.paths.is_empty(),
        "a refused cycle was assigned a path"
    );
    assert!(gateway.placed.is_empty());

    // Unset is the engine's default of four, which admits the triangle: a
    // node deployed before the variable existed behaves as it did.
    let mut unset = node_with_max_legs(None)?;
    let report = unset.cell.work(t(20), &mut RecordingGateway::default())?;
    assert!(refusals_under(&report, GATE_SCAN_LENGTH).is_empty());
    assert_eq!(report.paths.len(), 1, "{:?}", report.refusals);
    Ok(())
}

// --- MESH-023: a tradable conversion with no edge is reported, not skipped ---

#[test]
fn a_book_side_the_whitelist_gives_no_edge_is_reported_at_installation_and_a_whitelist_that_covers_every_side_reports_none()
-> Result<()> {
    // The failure this prevents has no symptom: a market the cell holds a
    // book for and the whitelist does not mention is one the desk can find
    // no cycle through, and the scan says nothing about it. Before this the
    // installation said "installed with 3 trade edge(s)" whether those three
    // were the whole market or a corner of it.
    let mut node = assembled();
    let mut installer = installer();
    for state in dislocated_books()? {
        node.cell.track(state);
    }
    // A fourth book the whitelist never mentions.
    node.cell
        .track(book("SOLUSDT", ("150.0", "100"), ("150.1", "100"))?);

    // Premise: the cell's own registry is four books on both sides, eight
    // tradable conversions, and the whitelist names three of them.
    let registry = registry_from_books(&node.cell);
    assert_eq!(registry.len(), 8, "premise: four books, two sides each");
    let whitelist = triangle(VENUE);
    assert_eq!(whitelist.conversions.len(), 3);

    node.cell
        .apply_policy(policy(1, t(10), true, Some(whitelist))?, t(10))?;
    installer.offer(signed_envelope(DESK)?)?;
    let outcome = installer.install(&mut node.cell, t(12));
    let Installation::InstalledWithGaps { edges, gaps } = &outcome else {
        panic!("five tradable book sides have no edge and the installation said: {outcome:?}");
    };
    assert_eq!(*edges, 3);
    // Exactly the five sides with no edge, in a stable order: the other
    // side of each of the triangle's three books, and both sides of the
    // book the whitelist left out. The three whitelisted sides are absent.
    assert_eq!(
        gaps,
        &vec![
            "BTCUSDT@CX/ask".to_string(),
            "ETHBTC@CX/ask".to_string(),
            "ETHUSDT@CX/bid".to_string(),
            "SOLUSDT@CX/bid".to_string(),
            "SOLUSDT@CX/ask".to_string(),
        ]
    );
    // It is installed all the same: a gap is a report, not a refusal.
    assert_eq!(
        node.cell
            .arbitrage()
            .expect("the desk is installed")
            .graph()
            .edge_count(),
        3
    );
    // And it reaches the operator: the outcome is not quiet, and the line
    // the tick logs counts the gaps and names them.
    assert!(!outcome.is_quiet());
    let line = outcome.describe();
    assert!(
        line.starts_with("installed with 3 trade edge(s); 5 tradable conversion(s)")
            && gaps.iter().all(|gap| line.contains(gap.as_str())),
        "{line}"
    );

    // The other half: a whitelist naming both sides of every book the cell
    // holds leaves nothing to report. Without this the test above passes
    // against an installer that reports a gap for everything.
    let mut node = assembled();
    let mut covering = self::installer();
    for state in dislocated_books()? {
        node.cell.track(state);
    }
    let mut both_ways = triangle(VENUE);
    both_ways.conversions.extend([
        conversion(VENUE, "ETHUSDT", "ETH", "USDT", BookSide::Bid),
        conversion(VENUE, "ETHBTC", "BTC", "ETH", BookSide::Ask),
        conversion(VENUE, "BTCUSDT", "USDT", "BTC", BookSide::Ask),
    ]);
    assert_eq!(registry_from_books(&node.cell).len(), 6);
    node.cell
        .apply_policy(policy(1, t(10), true, Some(both_ways))?, t(10))?;
    covering.offer(signed_envelope(DESK)?)?;
    assert_eq!(
        covering.install(&mut node.cell, t(12)),
        Installation::Installed(6)
    );
    Ok(())
}
