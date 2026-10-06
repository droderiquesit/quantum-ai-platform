//! WORLD-009: when two sources contradict each other about one fact, the
//! contradiction is recorded as an object linking both, and neither belief is
//! overwritten.
//!
//! The failure these tests prevent: the graph kept both statements as two
//! versions under one key and kept nothing saying they disagree. A reader saw
//! two edges and used whichever came first, so "the sources conflict about
//! this" was something the platform held and could not state.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::testing::approx_eq;
use qip_core::{Context, Duration, Timestamp};
use qip_financial::intelligence::{EntityMention, NewsItem, NewsSource, Sentiment};
use qip_financial::manifest::SourceManifest;
use qip_financial::quality::{DataQuality, Provenance};
use qip_world_model::graph::{CONTRADICTION_GAP, Fact, KnowledgeGraph};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use qip_world_model::world::{WorldModel, seed_demo_world};

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn days_ago(days: i64) -> Timestamp {
    now().saturating_sub(Duration::from_days(days))
}

/// `source` says `from` issues `to` at `weight`, learned `learned` days ago.
/// `Issues` has no inverse, so one statement is one fact.
fn says(from: &str, to: &str, weight: f64, source: &str, learned: i64) -> Fact {
    Fact::new(
        Relationship::new(from, to, RelationshipKind::Issues, weight, source),
        days_ago(100),
        days_ago(learned),
        0.8,
    )
    .unwrap()
}

#[test]
fn two_sources_stating_one_relationship_materially_differently_are_linked_by_a_contradiction_record_and_both_stay_readable()
 {
    let mut graph = KnowledgeGraph::new();
    graph.assert_fact(says("a", "b", 0.30, "filings", 10));
    // Premise: one statement is one fact and no contradiction.
    assert_eq!(graph.fact_count(), 1);
    assert!(graph.contradictions_at(now()).is_empty());

    graph.assert_fact(says("a", "b", 0.80, "newswire", 5));

    // Exactly one record, and it links the two beliefs by what identifies
    // each: its source, its figure and when it was learned.
    let recorded = graph.contradictions_at(now());
    assert_eq!(recorded.len(), 1, "the conflict was not recorded");
    let contradiction = recorded[0];
    assert_eq!(contradiction.key, "a|issues|b");
    assert_eq!(contradiction.held.source, "filings");
    assert_eq!(contradiction.arrived.source, "newswire");
    assert!(approx_eq(contradiction.held.weight, 0.30, 1e-12));
    assert!(approx_eq(contradiction.arrived.weight, 0.80, 1e-12));
    assert!(approx_eq(contradiction.gap, 0.50, 1e-12));
    assert_eq!(contradiction.held.recorded_at, days_ago(10));
    assert_eq!(contradiction.arrived.recorded_at, days_ago(5));

    // Neither side was overwritten or retracted: both beliefs are still
    // readable, each with its own source.
    let mut held: Vec<(String, f64)> = graph
        .facts_at(now(), now())
        .into_iter()
        .map(|fact| (fact.relationship.source.clone(), fact.relationship.weight))
        .collect();
    held.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(held.len(), 2, "a belief was dropped: {held:?}");
    assert_eq!(held[0].0, "filings");
    assert!(approx_eq(held[0].1, 0.30, 1e-12));
    assert_eq!(held[1].0, "newswire");
    assert!(approx_eq(held[1].1, 0.80, 1e-12));

    // Point in time: the conflict became knowable when the second statement
    // was learned, and not a day before.
    assert_eq!(contradiction.detected_at, days_ago(5));
    assert!(
        graph.contradictions_at(days_ago(6)).is_empty(),
        "a replay of six days ago sees a conflict created five days ago"
    );
}

#[test]
fn a_small_difference_a_sources_own_revision_and_a_retracted_belief_are_not_contradictions() {
    let mut graph = KnowledgeGraph::new();

    // Premise: the detector does fire in this graph, and at exactly the
    // declared gap, so the silences below are judgements and not a detector
    // that is switched off. Binary fractions throughout, so "exactly the
    // gap" is exact in `f64` and the boundary is the thing under test.
    assert!(approx_eq(CONTRADICTION_GAP, 0.25, 0.0), "premise: the gap");
    graph.assert_fact(says("p", "q", 0.25, "filings", 10));
    graph.assert_fact(says("p", "q", 0.5, "newswire", 9));
    assert_eq!(graph.contradictions_at(now()).len(), 1, "premise");

    // Two sources just under the gap apart: noise, not a conflict.
    graph.assert_fact(says("c", "d", 0.5, "filings", 10));
    graph.assert_fact(says("c", "d", 0.734_375, "newswire", 9));

    // One source restating its own figure: a revision. Nobody disagrees.
    graph.assert_fact(says("e", "f", 0.20, "filings", 10));
    graph.assert_fact(says("e", "f", 0.90, "filings", 9));

    // A belief retracted before the other statement was learned is no longer
    // held, so there is nothing for the new one to contradict.
    let first = says("g", "h", 0.20, "filings", 10);
    let key = first.relationship.key();
    graph.assert_fact(first);
    assert!(graph.retract(&key, days_ago(8)));
    graph.assert_fact(says("g", "h", 0.90, "newswire", 7));

    assert_eq!(
        graph.contradictions_at(now()).len(),
        1,
        "something that is not a contradiction was recorded as one: {:?}",
        graph.contradictions_at(now())
    );
}

/// One story about Northwind as `feed` carried it, concerning the company at
/// `confidence`.
fn story(feed: &str, confidence: f64, ingested: Timestamp) -> NewsItem {
    NewsItem {
        item_id: "story-1".into(),
        headline: "Northwind Semiconductor Corporation cut full year revenue guidance".into(),
        manifest: SourceManifest::generated(feed, "story-1", ingested, "The outlook fell."),
        source: NewsSource::Newswire,
        published_at: days_ago(2),
        entities: vec![EntityMention {
            text: "Northwind Semiconductor Corporation".into(),
            entity_id: None,
            confidence,
            is_primary: true,
            sentiment: None,
            kind: Default::default(),
            identifiers: Default::default(),
        }],
        sentiment: Sentiment::neutral(),
        topics: Vec::new(),
        provenance: Provenance::new(feed, days_ago(2), ingested),
        quality: DataQuality::clean(),
        evidence_unretrievable: false,
    }
}

#[test]
fn two_contradicting_evidence_items_about_one_fact_are_linked_through_ingestion_and_counted_in_the_world_state()
 {
    let (context, clock) = Context::deterministic(days_ago(1), 7);
    let mut model = WorldModel::new();
    seed_demo_world(&mut model, &context).unwrap();
    // Premise: the seeded world holds no conflict, so the one below is the
    // one this test fed.
    assert_eq!(model.state_at(now(), now()).contradiction_count, 0);

    // Two feeds carry the same story. One says it is squarely about
    // Northwind; the other says it barely concerns the company.
    let resolved = model
        .absorb_news(&story("wire-a", 0.95, days_ago(1)), &context)
        .unwrap();
    assert_eq!(resolved, vec!["ent-northwind".to_string()], "premise");
    assert_eq!(model.state_at(now(), now()).contradiction_count, 0);

    clock.advance(Duration::from_hours(12));
    model
        .absorb_news(&story("wire-b", 0.40, context.now()), &context)
        .unwrap();

    let recorded = model.graph().contradictions_at(now());
    assert_eq!(recorded.len(), 1, "ingestion did not record the conflict");
    assert_eq!(recorded[0].held.source, "wire-a");
    assert_eq!(recorded[0].arrived.source, "wire-b");
    assert!(recorded[0].key.contains("ent-northwind"));

    // Both beliefs remain readable with their sources.
    let mut sources: Vec<String> = model
        .graph()
        .facts_at(now(), now())
        .into_iter()
        .filter(|fact| fact.relationship.kind == RelationshipKind::ConcernsEntity)
        .map(|fact| fact.relationship.source.clone())
        .collect();
    sources.sort();
    assert_eq!(sources, vec!["wire-a".to_string(), "wire-b".to_string()]);

    // And the state the UNDERSTAND stage reads counts it, as of when it was
    // knowable and not before.
    assert_eq!(model.state_at(now(), now()).contradiction_count, 1);
    assert_eq!(
        model.state_at(now(), days_ago(1)).contradiction_count,
        0,
        "the state as known before the second feed arrived already shows the conflict"
    );
}
