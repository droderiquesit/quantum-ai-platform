//! Blueprint §8.2's fifth query composed with its first: which entities does
//! the book depend on that it does not hold, and which instruments sit within
//! two hops of each of them.
//!
//! The fixture is the blueprint's own sentence, built as data. KESTREL is a
//! private supplier nobody can buy. It supplies NORTHWIND, which issues NWD,
//! and HOLLOWAY, which issues HWY. The book holds NWD and AAA, and the causal
//! graph says KESTREL drives both. So the desk depends on a name it cannot
//! trade, and the instrument through which that dependency could be taken or
//! hedged — HWY — is two hops away and not held. Turning one observation into
//! several positions is the claim §8.2 makes for the graph structure, and
//! these tests fail if either half of the walk stops short or runs long.

// Every test here returns `Result` so that a fixture the library refuses
// fails the test rather than being unwrapped past; the assertions inside are
// the point of the test and are not a panic in production code.
#![allow(clippy::panic_in_result_fn)]

use std::collections::BTreeSet;

use qip_core::{Duration, Result, Timestamp};
use qip_world_model::causal::{CausalEdge, CausalGraph, Mechanism};
use qip_world_model::exposure::{
    MAX_EXPOSURE_HOPS, MAX_SECOND_ORDER_DEPENDENCIES, second_order_exposure,
};
use qip_world_model::graph::{Fact, KnowledgeGraph, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

fn held(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

fn drives(causal: &mut CausalGraph, cause: &str, effect: &str, recorded: i64) -> Result<()> {
    causal.add(CausalEdge::new(
        cause,
        effect,
        Mechanism::TemporalPrecedence,
        0.5,
        Duration::from_days(1),
        at(recorded),
    )?);
    Ok(())
}

/// KESTREL supplies NORTHWIND (issues NWD) and HOLLOWAY (issues HWY).
/// KESTREL issues nothing listed, which is the whole reason the second hop
/// has to exist.
fn supply_chain() -> KnowledgeGraph {
    let mut graph = KnowledgeGraph::new();
    for entity in ["KESTREL", "NORTHWIND", "HOLLOWAY"] {
        graph.add_node(Node::new(entity, NodeKind::Entity, entity, at(0)));
    }
    for instrument in ["NWD", "HWY"] {
        graph.add_node(Node::new(
            instrument,
            NodeKind::FinancialObject,
            instrument,
            at(0),
        ));
    }
    for (from, to, kind) in [
        ("KESTREL", "NORTHWIND", RelationshipKind::Supplies),
        ("KESTREL", "HOLLOWAY", RelationshipKind::Supplies),
        ("NORTHWIND", "NWD", RelationshipKind::Issues),
        ("HOLLOWAY", "HWY", RelationshipKind::Issues),
    ] {
        graph.assert_fact(
            Fact::new(
                Relationship::new(from, to, kind, 1.0, "fixture"),
                at(0),
                at(0),
            )
            .with_confidence(0.9),
        );
    }
    graph
}

/// KESTREL drives both held positions, and is held by nobody.
fn kestrel_drives_the_book() -> Result<CausalGraph> {
    let mut causal = CausalGraph::new();
    drives(&mut causal, "KESTREL", "NWD", 1_000)?;
    drives(&mut causal, "KESTREL", "AAA", 1_000)?;
    Ok(causal)
}

#[test]
fn a_driver_the_book_depends_on_and_cannot_hold_is_named_with_the_instruments_two_hops_from_it()
-> Result<()> {
    let graph = supply_chain();
    let causal = kestrel_drives_the_book()?;
    let book = held(&["NWD", "AAA"]);

    // The premise, asserted rather than assumed. Two positions are held and
    // the driver is not one of them; without this the finding below would
    // pass against a book that held nothing at all.
    assert_eq!(book.len(), 2, "the premise: two positions are held");
    assert!(
        !book.contains("KESTREL"),
        "the premise: the driver is unheld"
    );

    let review = second_order_exposure(&graph, &causal, &book, at(2_000), at(2_000))?;

    assert!(
        review.was_answerable(),
        "the premise: there were positions and edges, so there was a question to answer"
    );
    assert_eq!(
        review.dependencies_found, 1,
        "one unheld driver reaches this book"
    );
    assert_eq!(review.dependencies.len(), 1);
    let dependency = &review.dependencies[0];
    assert_eq!(dependency.entity, "KESTREL");
    assert_eq!(
        dependency.drives,
        held(&["AAA", "NWD"]),
        "the evidence for the dependency is both held positions, not one"
    );
    assert_eq!(
        dependency.exposure.hops_searched, MAX_EXPOSURE_HOPS,
        "the exposure leg is asked at the hop count §8.2 names"
    );
    assert_eq!(
        dependency.exposure.instruments(),
        held(&["HWY", "NWD"]),
        "both listed securities sit two hops from KESTREL"
    );
    // The operative half: the instrument the desk does not hold. NWD is
    // already in the book, so naming it would bury the one finding a reader
    // can act on under one they already know.
    assert_eq!(
        dependency.unheld_instruments,
        held(&["HWY"]),
        "only the instrument the book does not hold is offered as a route to the dependency"
    );
    Ok(())
}

#[test]
fn an_instrument_the_book_already_holds_is_not_offered_as_a_route_to_a_dependency_it_already_carries()
-> Result<()> {
    let graph = supply_chain();
    let causal = kestrel_drives_the_book()?;

    // The premise: with HWY unheld, HWY is the finding.
    let narrow = second_order_exposure(&graph, &causal, &held(&["NWD", "AAA"]), at(2), at(2_000))?;
    assert_eq!(
        narrow.dependencies[0].unheld_instruments,
        held(&["HWY"]),
        "the premise: this graph and this book do produce HWY as an unheld route"
    );

    let wide = second_order_exposure(
        &graph,
        &causal,
        &held(&["NWD", "AAA", "HWY"]),
        at(2),
        at(2_000),
    )?;
    assert_eq!(
        wide.dependencies.len(),
        1,
        "the dependency itself is unchanged: KESTREL still drives the book"
    );
    assert!(
        wide.dependencies[0].unheld_instruments.is_empty(),
        "a desk that already owns every route to its dependency has nothing left to be told, \
         got {:?}",
        wide.dependencies[0].unheld_instruments
    );
    assert!(
        wide.dependencies[0].exposure.instruments().contains("HWY"),
        "and the exposure still names it, so the finding is narrowed by the book rather than \
         lost"
    );
    Ok(())
}

#[test]
fn a_review_over_a_book_holding_nothing_says_nothing_rather_than_reporting_it_is_clean()
-> Result<()> {
    let graph = supply_chain();
    let causal = kestrel_drives_the_book()?;

    // The premise: the same graph answers when there is a book to ask about.
    let answered =
        second_order_exposure(&graph, &causal, &held(&["NWD", "AAA"]), at(2), at(2_000))?;
    assert!(
        answered.was_answerable(),
        "the premise: this graph does answer a real book"
    );
    assert!(
        !answered.detail().is_empty(),
        "the premise: an answerable review says something"
    );

    let empty = second_order_exposure(&graph, &causal, &BTreeSet::new(), at(2), at(2_000))?;
    // The failure this prevents is the one `MaxExpectedShortfall` already
    // demonstrated once here: a control whose empty answer reads as
    // protection. "No unheld dependency" and "no position to have one" are
    // different facts and must never render alike.
    assert!(
        empty.is_clean(),
        "an empty book produces an empty finding list"
    );
    assert!(
        !empty.was_answerable(),
        "but the empty list is not evidence of anything"
    );
    assert_eq!(
        empty.detail(),
        "",
        "and the stage detail stays silent rather than claiming a clean book"
    );
    Ok(())
}

#[test]
fn a_book_examined_against_an_empty_causal_graph_is_not_reported_as_depending_on_nothing()
-> Result<()> {
    let graph = supply_chain();
    let review = second_order_exposure(
        &graph,
        &CausalGraph::new(),
        &held(&["NWD", "AAA"]),
        at(2),
        at(2_000),
    )?;
    assert_eq!(
        review.positions_examined, 2,
        "the positions were there to examine"
    );
    assert_eq!(
        review.edges_considered, 0,
        "and nothing was there to examine them against"
    );
    assert!(
        !review.was_answerable(),
        "a graph with nothing in it cannot clear a book"
    );
    assert_eq!(review.detail(), "");
    Ok(())
}

#[test]
fn a_dependency_is_invisible_before_the_instant_the_edge_establishing_it_became_knowable()
-> Result<()> {
    let graph = supply_chain();
    let mut causal = CausalGraph::new();
    drives(&mut causal, "KESTREL", "NWD", 5_000)?;
    drives(&mut causal, "KESTREL", "AAA", 5_000)?;
    let book = held(&["NWD", "AAA"]);

    // The premise: once the edges are knowable, the dependency is there.
    let later = second_order_exposure(&graph, &causal, &book, at(6_000), at(6_000))?;
    assert_eq!(
        later.dependencies_found, 1,
        "the premise: the dependency exists at 6,000"
    );

    // The defect this prevents: a decision taken at 4,000 reading a
    // dependency the platform did not learn about until 5,000 is look-ahead,
    // and look-ahead makes a backtest look better rather than anomalous, so
    // nothing downstream would flag it.
    let earlier = second_order_exposure(&graph, &causal, &book, at(4_000), at(4_000))?;
    assert_eq!(
        earlier.dependencies_found, 0,
        "an edge recorded at 5,000 must not be visible to a review run at 4,000"
    );
    assert_eq!(
        earlier.edges_considered, 0,
        "and the review says it examined nothing, so the empty answer is not read as clean"
    );
    assert!(!earlier.was_answerable());
    Ok(())
}

#[test]
fn a_review_follows_at_most_its_bound_and_reports_the_dependencies_it_left_behind() -> Result<()> {
    // One narrow driver per position, exactly as many as the bound will
    // follow, and then one wide driver on top: one more dependency than the
    // pass can take.
    let mut causal = CausalGraph::new();
    let mut positions: BTreeSet<String> = BTreeSet::new();
    for index in 0..MAX_SECOND_ORDER_DEPENDENCIES {
        let position = format!("P{index:02}");
        drives(&mut causal, &format!("d{index:02}"), &position, 1_000)?;
        positions.insert(position);
    }
    // The wide driver's name sorts after every narrow one, so a bound that
    // cut in id order rather than by reach would drop exactly the finding
    // worth having.
    drives(&mut causal, "zzz-wide", "P00", 1_000)?;
    drives(&mut causal, "zzz-wide", "P01", 1_000)?;

    let review = second_order_exposure(
        &KnowledgeGraph::new(),
        &causal,
        &positions,
        at(2),
        at(2_000),
    )?;

    assert_eq!(
        review.dependencies_found,
        MAX_SECOND_ORDER_DEPENDENCIES + 1,
        "the premise: more dependencies exist than the bound will follow"
    );
    assert_eq!(
        review.dependencies.len(),
        MAX_SECOND_ORDER_DEPENDENCIES,
        "the pass follows its bound and no more"
    );
    assert_eq!(
        review.dependencies_truncated, 1,
        "and it says how many it named and did not follow, so the list reads as a prefix"
    );
    assert_eq!(
        review.dependencies[0].entity, "zzz-wide",
        "the driver reaching most of the book is followed first, whatever its name sorts as"
    );
    assert!(
        review
            .detail()
            .contains("1 further dependency(ies) named and not followed this pass"),
        "an operator must be able to tell a complete review from a cut one: {}",
        review.detail()
    );
    Ok(())
}

#[test]
fn a_driver_the_book_holds_is_not_reported_as_a_dependency_the_book_does_not_hold() -> Result<()> {
    let graph = supply_chain();
    let causal = kestrel_drives_the_book()?;

    // The premise: with KESTREL unheld it is the dependency.
    let unheld = second_order_exposure(&graph, &causal, &held(&["NWD", "AAA"]), at(2), at(2_000))?;
    assert_eq!(
        unheld.dependencies_found, 1,
        "the premise: this graph does name KESTREL"
    );

    let owned = second_order_exposure(
        &graph,
        &causal,
        &held(&["NWD", "AAA", "KESTREL"]),
        at(2),
        at(2_000),
    )?;
    assert!(
        owned.is_clean(),
        "a driver the desk can see it owns is a counted exposure, not second-order risk"
    );
    assert!(
        owned.was_answerable(),
        "and the clean answer is evidence, because there were positions and edges"
    );
    Ok(())
}
