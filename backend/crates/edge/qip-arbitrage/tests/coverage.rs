//! MESH-023: every conversion a registry marks tradable has its directed
//! edge, or is reported as a gap. Never neither.
//!
//! The failure this prevents has no symptom. A conversion with no edge is
//! one the search cannot propose a cycle through, so the scan refuses
//! nothing for it and reports nothing about it: a desk blind to a market
//! and a desk watching a quiet one produce the same empty report. The only
//! way to see the difference is to compare the graph with something that is
//! not the graph, which is what `ArbitrageGraph::coverage` does and what
//! these generated registries hold it to.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_arbitrage::coverage::{Tradable, TradableRegistry};
use qip_arbitrage::graph::{ArbitrageGraph, EdgeKind, Node, VenueFacts};
use qip_contracts::intent::{Representation, SettlementStage};
use qip_contracts::message::BookSide;
use qip_contracts::venue::{Region, VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, ObjectId, Timestamp};
use std::collections::BTreeSet;

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn venue(index: u64) -> VenueId {
    VenueId::new(format!("V{index}"))
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

/// The base and quote a generated market converts between. The registry
/// names a market and a side; the graph needs the two instruments, and the
/// fixture is where that correspondence lives.
fn base_and_quote(market: &ObjectId) -> (ObjectId, ObjectId) {
    (
        object(&format!("{}-base", market.as_str())),
        object(&format!("{}-quote", market.as_str())),
    )
}

/// Add the one directed edge that represents `tradable`, through the same
/// constructors every production graph is built with.
fn represent(graph: &mut ArbitrageGraph, tradable: &Tradable) -> Result<()> {
    match tradable {
        Tradable::Trade {
            venue,
            market,
            side,
        } => {
            graph.register_venue(
                venue.clone(),
                VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
            );
            let (base, quote) = base_and_quote(market);
            // Consuming bids sells the base for the quote; consuming offers
            // buys the base with the quote.
            let (from, to) = match side {
                BookSide::Bid => (base, quote),
                BookSide::Ask => (quote, base),
            };
            graph.add_trade(
                Node::new(
                    from,
                    venue.clone(),
                    Representation::Spot,
                    SettlementStage::T0,
                    Region::Global,
                ),
                Node::new(
                    to,
                    venue.clone(),
                    Representation::Spot,
                    SettlementStage::T0,
                    Region::Global,
                ),
                Decimal::ONE,
                Decimal::ZERO,
                market.clone(),
                *side,
                at(),
                1,
            )?;
        }
        Tradable::Transfer { object, from, to } => {
            graph.add_transfer(
                object.clone(),
                from.clone(),
                to.clone(),
                Decimal::ZERO,
                at(),
                1,
            )?;
        }
    }
    Ok(())
}

/// A registry of a few venues, each with markets tradable on one side or
/// both, and a few transfers allowed in one direction or both.
fn generated_registry(rng: &mut Xoshiro256) -> Result<TradableRegistry> {
    let venues = 1 + rng.below(3);
    let mut registry = TradableRegistry::new();
    for v in 0..venues {
        for m in 0..1 + rng.below(5) {
            let market = object(&format!("M{v}-{m}"));
            registry = match rng.below(3) {
                0 => registry.with_side(venue(v), market, BookSide::Bid),
                1 => registry.with_side(venue(v), market, BookSide::Ask),
                _ => registry.with_market(venue(v), market),
            };
        }
    }
    if venues > 1 {
        for t in 0..rng.below(4) {
            let from = rng.below(venues);
            let to = (from + 1 + rng.below(venues - 1)) % venues;
            let moved = object(&format!("T{t}"));
            registry = registry.with_transfer(moved.clone(), venue(from), venue(to))?;
            if rng.bernoulli(0.5) {
                registry = registry.with_transfer(moved, venue(to), venue(from))?;
            }
        }
    }
    Ok(registry)
}

/// Whether the graph holds an edge in exactly `tradable`'s direction, asked
/// of the edges directly rather than through `coverage`, so the property
/// below is checked against something other than the code under test.
fn has_directed_edge(graph: &ArbitrageGraph, tradable: &Tradable) -> bool {
    graph.edges().iter().any(|edge| match tradable {
        Tradable::Trade {
            venue,
            market,
            side,
        } => {
            let (base, quote) = base_and_quote(market);
            let (from, to) = match side {
                BookSide::Bid => (base, quote),
                BookSide::Ask => (quote, base),
            };
            matches!(&edge.kind, EdgeKind::Trade { .. })
                && edge.from
                    == Node::new(
                        from,
                        venue.clone(),
                        Representation::Spot,
                        SettlementStage::T0,
                        Region::Global,
                    )
                && edge.to
                    == Node::new(
                        to,
                        venue.clone(),
                        Representation::Spot,
                        SettlementStage::T0,
                        Region::Global,
                    )
        }
        Tradable::Transfer { object, from, to } => {
            matches!(edge.kind, EdgeKind::Transfer)
                && edge.from
                    == Node::new(
                        object.clone(),
                        from.clone(),
                        Representation::Spot,
                        SettlementStage::T0,
                        Region::Global,
                    )
                && edge.to
                    == Node::new(
                        object.clone(),
                        to.clone(),
                        Representation::Spot,
                        SettlementStage::T0,
                        Region::Global,
                    )
        }
    })
}

#[test]
fn every_conversion_a_registry_marks_tradable_has_its_directed_edge_or_is_reported_as_a_gap_and_none_is_skipped()
-> Result<()> {
    let mut rng = Xoshiro256::seeded(23);
    let (mut registries, mut conversions, mut gaps_seen, mut one_way_gaps, mut transfers) =
        (0u32, 0usize, 0usize, 0u32, 0usize);

    for _ in 0..300 {
        let registry = generated_registry(&mut rng)?;
        registries += 1;
        conversions += registry.len();
        transfers += registry
            .iter()
            .filter(|t| matches!(t, Tradable::Transfer { .. }))
            .count();

        // Built from the whole registry: every tradable conversion appears
        // as a directed edge in each direction it can be traded, and there
        // is nothing to report.
        let mut whole = ArbitrageGraph::new();
        for tradable in registry.iter() {
            represent(&mut whole, tradable)?;
        }
        let coverage = whole.coverage(&registry);
        assert!(coverage.is_complete(), "{:?}", coverage.gaps);
        assert_eq!(coverage.covered.len(), registry.len());
        for tradable in registry.iter() {
            assert!(
                has_directed_edge(&whole, tradable),
                "{} is tradable and has no edge in its direction",
                tradable.label()
            );
        }

        // Built from part of it: the conversions left out are the gaps,
        // exactly, and every conversion is in one list or the other.
        let represented: BTreeSet<Tradable> = registry
            .iter()
            .filter(|_| rng.bernoulli(0.6))
            .cloned()
            .collect();
        let mut partial = ArbitrageGraph::new();
        for tradable in &represented {
            represent(&mut partial, tradable)?;
        }
        let coverage = partial.coverage(&registry);
        let covered: BTreeSet<Tradable> = coverage.covered.iter().cloned().collect();
        let gaps: BTreeSet<Tradable> = coverage.gaps.iter().cloned().collect();
        let expected_gaps: BTreeSet<Tradable> = registry
            .iter()
            .filter(|t| !represented.contains(*t))
            .cloned()
            .collect();
        assert_eq!(covered, represented);
        assert_eq!(
            gaps, expected_gaps,
            "a tradable conversion with no edge was not reported"
        );
        assert!(covered.is_disjoint(&gaps));
        assert_eq!(
            coverage.covered.len() + coverage.gaps.len(),
            registry.len(),
            "a tradable conversion was neither covered nor reported"
        );
        for tradable in &gaps {
            assert!(!has_directed_edge(&partial, tradable));
        }
        assert_eq!(
            coverage.gap_labels(),
            coverage
                .gaps
                .iter()
                .map(Tradable::label)
                .collect::<Vec<_>>()
        );
        gaps_seen += gaps.len();

        // Directed: where the other side of the same book has its edge and
        // this side does not, this side is still a gap.
        for tradable in &gaps {
            let other = match tradable {
                Tradable::Trade {
                    venue,
                    market,
                    side,
                } => Tradable::Trade {
                    venue: venue.clone(),
                    market: market.clone(),
                    side: match side {
                        BookSide::Bid => BookSide::Ask,
                        BookSide::Ask => BookSide::Bid,
                    },
                },
                Tradable::Transfer { object, from, to } => Tradable::Transfer {
                    object: object.clone(),
                    from: to.clone(),
                    to: from.clone(),
                },
            };
            if covered.contains(&other) {
                one_way_gaps += 1;
            }
        }
    }

    // Premise, checked last because it is about the run as a whole: the
    // generator really produced registries with trades and transfers, gaps
    // were really reported, and the directed case really arose. A generator
    // that quietly produced empty registries would pass everything above.
    assert_eq!(registries, 300);
    assert!(
        conversions > 1_000,
        "only {conversions} conversions generated"
    );
    assert!(transfers > 50, "only {transfers} transfers generated");
    assert!(gaps_seen > 300, "only {gaps_seen} gaps arose");
    assert!(
        one_way_gaps > 30,
        "the case of one direction represented and the other not arose {one_way_gaps} times"
    );
    Ok(())
}

#[test]
fn an_edge_for_one_side_of_a_book_does_not_cover_the_other_side_or_another_venue() -> Result<()> {
    let cx = VenueId::new("CX");
    let dx = VenueId::new("DX");
    let market = object("ETHUSDT");
    let registry = TradableRegistry::new()
        .with_market(cx.clone(), market.clone())
        .with_market(dx.clone(), market.clone())
        .with_transfer(object("ETH"), cx.clone(), dx.clone())?
        .with_transfer(object("ETH"), dx.clone(), cx.clone())?;
    assert_eq!(registry.len(), 6, "premise: two books, two sides, two ways");

    let mut graph = ArbitrageGraph::new();
    represent(
        &mut graph,
        &Tradable::Trade {
            venue: cx.clone(),
            market: market.clone(),
            side: BookSide::Ask,
        },
    )?;
    graph.add_transfer(
        object("ETH"),
        cx.clone(),
        dx.clone(),
        Decimal::ZERO,
        at(),
        1,
    )?;
    assert_eq!(graph.edge_count(), 2, "premise: exactly two edges");

    let coverage = graph.coverage(&registry);
    assert_eq!(
        coverage.covered,
        vec![
            Tradable::Trade {
                venue: cx.clone(),
                market: market.clone(),
                side: BookSide::Ask,
            },
            Tradable::Transfer {
                object: object("ETH"),
                from: cx.clone(),
                to: dx.clone(),
            },
        ]
    );
    assert_eq!(
        coverage.gap_labels(),
        vec![
            "ETHUSDT@CX/bid".to_string(),
            "ETHUSDT@DX/bid".to_string(),
            "ETHUSDT@DX/ask".to_string(),
            "ETH:DX>CX".to_string(),
        ]
    );
    assert!(!coverage.is_complete());

    // A transfer from a venue to itself is not a conversion and is refused
    // rather than registered as a gap nothing could ever close.
    assert!(
        TradableRegistry::new()
            .with_transfer(object("ETH"), cx.clone(), cx)
            .is_err()
    );
    Ok(())
}
