//! MESH-010: a cycle has two to twenty legs, the maximum in force is
//! configuration, and a cycle past it is refused whole.
//!
//! Before this suite the search dropped a cycle longer than
//! `max_cycle_edges` without a word. That is not truncation, and it is not a
//! refusal either: a market whose only cycle was one leg too long produced
//! the same empty report as a market with no cycle in it, and nobody
//! reading the report could tell that a limit they had set was the reason.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_arbitrage::graph::{ArbitrageGraph, Node, VenueFacts};
use qip_arbitrage::liquidity::StaticLiquidity;
use qip_arbitrage::netedge::EdgeAssumptions;
use qip_arbitrage::plan::PlanSettings;
use qip_arbitrage::scan::{OpportunityScanner, RejectionStage, ScanReport, SizePolicy};
use qip_arbitrage::search::{MAX_CYCLE_EDGES, MIN_CYCLE_EDGES, SearchSettings, search};
use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook};

const VENUE: &str = "CX";

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn d(value: &str) -> Decimal {
    Decimal::parse(value).expect("test fixture decimal")
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

/// A ring of `legs` conversions at one venue, every hop its own market.
///
/// One hop is offered at nine tenths and the rest at parity, on books a
/// hundredth of a percent wide, so the ring pays after costs however long it
/// is. The length, and only the length, is then the thing under test.
fn ring(legs: usize) -> Result<(ArbitrageGraph, StaticLiquidity)> {
    let venue = VenueId::new(VENUE);
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        venue.clone(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    let mut depth = StaticLiquidity::new();
    for hop in 0..legs {
        let market = format!("M{hop}");
        let (bid, ask, rate) = if hop == 0 {
            ("0.8999", "0.9", "1.1111")
        } else {
            ("0.9999", "1", "1")
        };
        graph.add_trade(
            Node::new(object(&format!("O{hop}")), venue.clone()),
            Node::new(object(&format!("O{}", (hop + 1) % legs)), venue.clone()),
            d(rate),
            Decimal::ZERO,
            object(&market),
            BookSide::Ask,
            at(),
            20,
        )?;
        depth = depth.with_book(
            venue.clone(),
            OrderBook::from_levels(
                object(&market),
                VENUE,
                at(),
                vec![BookLevel::new(d(bid), d("1000000"))],
                vec![BookLevel::new(d(ask), d("1000000"))],
            ),
            20,
        );
    }
    Ok((graph, depth))
}

fn settings(max_cycle_edges: usize) -> SearchSettings {
    SearchSettings {
        max_cycle_edges,
        ..SearchSettings::default()
    }
}

fn scan(legs: usize, max_cycle_edges: usize) -> Result<ScanReport> {
    let (graph, depth) = ring(legs)?;
    let scanner = OpportunityScanner::new(
        settings(max_cycle_edges),
        EdgeAssumptions::default(),
        PlanSettings::with_budget(d("5000000")),
    );
    Ok(scanner.scan(&graph, &depth, &SizePolicy::uniform(d("1000")), at()))
}

#[test]
fn with_the_maximum_at_twenty_cycles_of_two_and_twenty_legs_are_accepted_and_one_and_twenty_one_are_refused()
-> Result<()> {
    assert_eq!((MIN_CYCLE_EDGES, MAX_CYCLE_EDGES), (2, 20));
    let twenty = settings(20);
    twenty.validate()?;

    // The rule itself, at both of its edges.
    twenty.admit_length(2)?;
    twenty.admit_length(20)?;
    let one = twenty.admit_length(1).unwrap_err();
    assert!(one.message().contains("cannot close"), "{}", one.message());
    let over = twenty.admit_length(21).unwrap_err();
    assert!(
        over.message().contains("21 legs") && over.message().contains("maximum of 20"),
        "{}",
        over.message()
    );

    // And the rule as the engine applies it. Premise: every ring is a real
    // profitable cycle of exactly the length asked for, so what separates
    // the accepted from the refused below is the length alone.
    for legs in [2, 20, 21] {
        let (graph, _) = ring(legs)?;
        let found = search(&graph, &settings(MAX_CYCLE_EDGES + 5));
        assert_eq!(
            found.candidates.len(),
            1,
            "premise: a {legs}-leg ring is found"
        );
        assert_eq!(found.candidates[0].len(), legs);
    }

    for legs in [2, 20] {
        let report = scan(legs, 20)?;
        assert!(
            report.rejections.is_empty(),
            "a {legs}-leg cycle was refused at a maximum of 20: {:?}",
            report.rejections
        );
        assert_eq!(report.opportunities.len(), 1, "{legs} legs");
        assert_eq!(report.opportunities[0].candidate.len(), legs);
        assert_eq!(report.opportunities[0].planned.plan.len(), legs);
    }

    let report = scan(21, 20)?;
    assert!(
        report.opportunities.is_empty(),
        "a 21-leg cycle was accepted"
    );
    let refused = report.rejected_at(RejectionStage::Length);
    assert_eq!(refused.len(), 1, "{:?}", report.rejections);
    assert!(
        refused[0].detail.contains("21 legs") && refused[0].detail.contains("maximum of 20"),
        "{}",
        refused[0].detail
    );
    Ok(())
}

#[test]
fn with_the_maximum_at_five_a_six_leg_candidate_is_refused_naming_the_limit_and_is_not_shortened_to_fit()
-> Result<()> {
    // Premise: five legs at a maximum of five is an opportunity, so the
    // refusal below is caused by the sixth leg and by nothing else about
    // the fixture.
    let within = scan(5, 5)?;
    assert_eq!(within.opportunities.len(), 1, "{:?}", within.rejections);
    assert!(within.rejections.is_empty());

    let report = scan(6, 5)?;
    assert!(
        report.opportunities.is_empty(),
        "a six-leg cycle produced an opportunity at a maximum of five: {:?}",
        report
            .opportunities
            .iter()
            .map(|o| o.candidate.edges.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(report.rejections.len(), 1, "{:?}", report.rejections);
    let refusal = &report.rejections[0];
    assert_eq!(refusal.stage, RejectionStage::Length);
    assert_eq!(refusal.stage.as_str(), "length");
    assert!(
        refusal.detail.contains("6 legs") && refusal.detail.contains("maximum of 5"),
        "the refusal does not name the limit: {}",
        refusal.detail
    );
    // Refused, not truncated: the cycle the refusal carries is the whole
    // six-leg ring, every edge once, and not a five-leg prefix of it.
    let mut edges = refusal.candidate.edges.clone();
    edges.sort_unstable();
    assert_eq!(edges, vec![0, 1, 2, 3, 4, 5]);
    Ok(())
}

#[test]
fn a_configured_maximum_outside_two_to_twenty_is_refused_and_never_lowered_to_the_ceiling() {
    for max in [MIN_CYCLE_EDGES, 5, MAX_CYCLE_EDGES] {
        assert!(settings(max).validate().is_ok(), "premise: {max} is valid");
    }
    for max in [0, 1, MAX_CYCLE_EDGES + 1, 30] {
        let refused = settings(max).validate().unwrap_err();
        assert!(
            refused.message().contains(&format!("{max} legs"))
                && refused.message().contains("2 to 20"),
            "{}",
            refused.message()
        );
    }
}
