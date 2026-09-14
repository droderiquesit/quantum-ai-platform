//! Blueprint §8.2's two-hop exposure query, over the bitemporal
//! relationship graph.
//!
//! The chain §8.2 argues for, built as a fixture: a supplier that supplies a
//! manufacturer that issues a listed security. One hop reaches nothing
//! tradeable; two hops reach the instrument. That is the whole claim the
//! graph structure makes for itself, and it is worth a test that would fail
//! if the traversal stopped short or ran long.

use qip_core::{Result, Timestamp};
use qip_world_model::exposure::{MAX_EXPOSURE_HOPS, instruments_exposed_to};
use qip_world_model::graph::{Fact, KnowledgeGraph, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

/// KESTREL supplies NORTHWIND, which issues NWD. KESTREL itself issues
/// nothing listed.
///
/// `known_from` is when each fact became knowable, so the point-in-time test
/// below can ask the graph a question from before it knew the second hop.
fn supply_chain(known_from: i64) -> Result<KnowledgeGraph> {
    let mut graph = KnowledgeGraph::new();
    graph.add_node(Node::new("KESTREL", NodeKind::Entity, "Kestrel", at(0)));
    graph.add_node(Node::new("NORTHWIND", NodeKind::Entity, "Northwind", at(0)));
    graph.add_node(Node::new("NWD", NodeKind::FinancialObject, "NWD", at(0)));

    graph.assert_fact(
        Fact::new(
            Relationship::new(
                "KESTREL",
                "NORTHWIND",
                RelationshipKind::Supplies,
                1.0,
                "fixture",
            ),
            at(0),
            at(0),
        )
        .with_confidence(0.9),
    );
    graph.assert_fact(
        Fact::new(
            Relationship::new("NORTHWIND", "NWD", RelationshipKind::Issues, 1.0, "fixture"),
            at(0),
            at(known_from),
        )
        .with_confidence(0.8),
    );
    Ok(graph)
}

#[test]
fn an_instrument_two_hops_from_an_entity_is_found_and_one_hop_does_not_reach_it() {
    let graph = supply_chain(0).unwrap();

    // The premise: one hop from KESTREL reaches NORTHWIND, which is an
    // entity and not an instrument. Without this, the two-hop result below
    // could be a direct link the fixture accidentally created.
    let one = instruments_exposed_to(&graph, "KESTREL", 1, at(10), at(10)).unwrap();
    assert!(
        one.is_empty(),
        "one hop from the supplier reaches a company, not a tradeable instrument; got {:?}",
        one.exposures
    );

    let two = instruments_exposed_to(&graph, "KESTREL", 2, at(10), at(10)).unwrap();
    assert_eq!(
        two.exposures.len(),
        1,
        "two hops reach exactly one instrument"
    );
    let found = &two.exposures[0];
    assert_eq!(found.instrument, "NWD");
    assert_eq!(found.hops, 2);
    assert_eq!(
        found.path,
        vec![
            "KESTREL".to_string(),
            "NORTHWIND".to_string(),
            "NWD".to_string()
        ],
        "the path is carried so the exposure can be explained rather than only asserted"
    );
    // 0.9 * 0.8. Multiplied, not averaged: two uncertain links in series are
    // less trustworthy than either alone, and an average would let the
    // certain first hop launder the second.
    assert!(
        (found.confidence - 0.72).abs() < 1e-9,
        "confidence multiplies along the path; got {}",
        found.confidence
    );
    assert_eq!(two.truncated, 0);
}

#[test]
fn an_exposure_is_invisible_before_the_instant_the_link_became_knowable() {
    // The second hop is not knowable until 500.
    let graph = supply_chain(500).unwrap();

    // The premise: after 500 the exposure is there to be found.
    let later = instruments_exposed_to(&graph, "KESTREL", 2, at(1_000), at(1_000)).unwrap();
    assert_eq!(
        later.exposures.len(),
        1,
        "the premise: the exposure exists once the link is knowable"
    );

    // The defect this prevents: a position sized at 100 on an exposure the
    // platform did not learn about until 500 is look-ahead, and a backtest
    // containing it looks *better* rather than anomalous, which is why no
    // result will reveal it.
    let earlier = instruments_exposed_to(&graph, "KESTREL", 2, at(100), at(100)).unwrap();
    assert!(
        earlier.is_empty(),
        "a fact learned at 500 must not be readable at 100; got {:?}",
        earlier.exposures
    );
}

#[test]
fn a_cycle_in_the_relationship_graph_terminates_rather_than_walking_for_ever() {
    let mut graph = KnowledgeGraph::new();
    for id in ["A", "B"] {
        graph.add_node(Node::new(id, NodeKind::Entity, id, at(0)));
    }
    graph.add_node(Node::new("SEC", NodeKind::FinancialObject, "SEC", at(0)));
    // A -> B -> A, and B -> SEC.
    graph.assert_fact(Fact::new(
        Relationship::new("A", "B", RelationshipKind::Supplies, 1.0, "fixture"),
        at(0),
        at(0),
    ));
    graph.assert_fact(Fact::new(
        Relationship::new("B", "A", RelationshipKind::Customer, 1.0, "fixture"),
        at(0),
        at(0),
    ));
    graph.assert_fact(Fact::new(
        Relationship::new("B", "SEC", RelationshipKind::Issues, 1.0, "fixture"),
        at(0),
        at(0),
    ));

    let found = instruments_exposed_to(&graph, "A", MAX_EXPOSURE_HOPS, at(10), at(10)).unwrap();
    // The premise and the property in one: the walk terminated, and it found
    // the instrument on the way.
    assert_eq!(
        found.exposures.len(),
        1,
        "the cycle did not hide the instrument"
    );
    assert_eq!(found.exposures[0].instrument, "SEC");
    assert_eq!(found.exposures[0].hops, 2);
}

#[test]
fn an_entity_with_no_outgoing_facts_answers_empty_rather_than_failing() {
    let mut graph = KnowledgeGraph::new();
    graph.add_node(Node::new("LONELY", NodeKind::Entity, "Lonely", at(0)));
    let found = instruments_exposed_to(&graph, "LONELY", 2, at(10), at(10)).unwrap();
    assert!(
        found.is_empty(),
        "an entity nothing connects to is a legitimate empty answer, not an error"
    );
    assert_eq!(found.entity, "LONELY");
    assert_eq!(found.hops_searched, 2);
}

#[test]
fn a_retracted_relationship_stops_carrying_an_exposure_from_the_instant_it_was_retracted() {
    let mut graph = supply_chain(0).unwrap();
    // The premise: the exposure exists before the retraction.
    let before = instruments_exposed_to(&graph, "KESTREL", 2, at(100), at(100)).unwrap();
    assert_eq!(
        before.exposures.len(),
        1,
        "the premise: the exposure exists at 100"
    );

    let key = Relationship::new("NORTHWIND", "NWD", RelationshipKind::Issues, 1.0, "fixture").key();
    assert!(
        graph.retract(&key, at(200)),
        "the premise: the retraction landed"
    );

    let after = instruments_exposed_to(&graph, "KESTREL", 2, at(300), at(300)).unwrap();
    assert!(
        after.is_empty(),
        "a relationship retracted at 200 carries no exposure at 300; got {:?}",
        after.exposures
    );
    // And the bitemporal guarantee, stated in the dimension `retract`
    // actually works in. `Fact::holds` compares `retracted_at` against
    // `known_at`, not against `valid_at`: a retraction here is the platform
    // saying "we no longer believe this", which is a fact about *knowledge*,
    // distinct from `valid_to`, which says "it stopped being true". So
    // replaying the decision made at 100 means asking what was known at 100,
    // and that still answers — which is what keeps a decision taken while
    // the fact was believed explicable rather than looking arbitrary.
    //
    // This test asserted `known_at = 300` here at first and failed, which is
    // worth recording: the two dimensions are easy to reach for the wrong
    // way round, and a traversal that conflated them would have made the
    // failure invisible instead of loud.
    let replayed = instruments_exposed_to(&graph, "KESTREL", 2, at(100), at(100)).unwrap();
    assert_eq!(
        replayed.exposures.len(),
        1,
        "a replay of the instant the decision was made must see what the platform then believed"
    );
}
