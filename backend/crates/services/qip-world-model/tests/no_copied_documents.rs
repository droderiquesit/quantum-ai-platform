//! WORLD-057: the knowledge graph holds distilled knowledge and the reference
//! a source can be fetched from again. It never holds the source.
//!
//! The failure these tests prevent: until the excerpt limit existed, "the
//! graph stores no documents" was true only because the upstream types had no
//! body field and headlines happened to be short. `add_node` took any string
//! as a label or an attribute, so one caller putting an article where a
//! headline goes would have made the graph a copy of a document.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::{Context, Timestamp};
use qip_financial::intelligence::{EntityMention, NewsItem, NewsSource, Sentiment};
use qip_financial::manifest::SourceManifest;
use qip_financial::quality::{DataQuality, Provenance};
use qip_world_model::graph::{EXCERPT_LIMIT, Fact, KnowledgeGraph, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use qip_world_model::world::{WorldModel, seed_demo_world};

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

/// A payload of exactly `length` characters in which no window repeats: each
/// word carries its own position. A span a graph record shares with it can
/// therefore only have been copied from it, and its length is unambiguous.
fn payload(seed: u64, length: usize) -> String {
    let mut text = String::new();
    let mut state = seed;
    let mut word = 0usize;
    while text.chars().count() < length {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        text.push_str(&format!("w{word}x{:05} ", (state >> 33) % 100_000));
        word += 1;
    }
    text.chars().take(length).collect()
}

/// The longest run of characters `record` shares verbatim with `source`.
fn longest_shared_span(record: &str, source: &str) -> usize {
    let record: Vec<char> = record.chars().collect();
    let source: Vec<char> = source.chars().collect();
    let mut longest = 0;
    let mut previous = vec![0usize; source.len() + 1];
    for r in &record {
        let mut current = vec![0usize; source.len() + 1];
        for (j, s) in source.iter().enumerate() {
            if r == s {
                current[j + 1] = previous[j] + 1;
                longest = longest.max(current[j + 1]);
            }
        }
        previous = current;
    }
    longest
}

/// Every string the graph holds: each node's id, label and attributes, and
/// each fact's two ends and its source.
fn graph_strings(graph: &KnowledgeGraph) -> Vec<String> {
    let mut held = Vec::new();
    for node in graph.nodes() {
        held.push(node.id.clone());
        held.push(node.label.clone());
        for (key, value) in &node.attributes {
            held.push(key.clone());
            held.push(value.clone());
        }
    }
    for fact in graph.facts_at(now(), now()) {
        held.push(fact.relationship.from.clone());
        held.push(fact.relationship.to.clone());
        held.push(fact.relationship.source.clone());
    }
    held
}

/// A news item about a company the seeded world has never heard of, so a
/// half-absorbed item leaves a new entity node behind and cannot hide.
fn news(item_id: &str, company: &str, headline: String, article: &str) -> NewsItem {
    NewsItem {
        item_id: item_id.into(),
        headline,
        manifest: SourceManifest::generated("test", item_id, now(), article),
        source: NewsSource::Newswire,
        published_at: now(),
        entities: vec![EntityMention {
            text: company.into(),
            entity_id: None,
            confidence: 0.9,
            is_primary: true,
            sentiment: None,
        }],
        sentiment: Sentiment {
            polarity: -0.7,
            confidence: 0.9,
            novelty: 0.8,
        },
        topics: vec!["guidance".into()],
        provenance: Provenance::synthetic("synthetic-news", now()),
        quality: DataQuality::clean(),
    }
}

#[test]
fn no_graph_record_holds_a_verbatim_span_of_an_ingested_payload_longer_than_the_excerpt_limit() {
    let (context, _clock) = Context::deterministic(now(), 7);
    let mut model = WorldModel::new();
    seed_demo_world(&mut model, &context).unwrap();

    // Premise: ingestion really does write source text into the graph, and
    // the scan really can see it. A headline at the limit is absorbed and the
    // graph then shares the whole of it with the payload; without this, "no
    // span over the limit" would hold of a scan that could see nothing.
    let at_limit = payload(1, EXCERPT_LIMIT);
    assert_eq!(at_limit.chars().count(), EXCERPT_LIMIT);
    let resolved = model
        .absorb_news(
            &news(
                "at-limit",
                "Farrowdale Tidal Works",
                at_limit.clone(),
                &at_limit,
            ),
            &context,
        )
        .expect("a headline at the limit is absorbed");
    assert_eq!(resolved.len(), 1, "premise: the mention resolved");
    let seen = graph_strings(model.graph())
        .iter()
        .map(|held| longest_shared_span(held, &at_limit))
        .max()
        .unwrap();
    assert_eq!(
        seen, EXCERPT_LIMIT,
        "premise: the absorbed headline is in the graph and the scan finds it"
    );

    // The property, over generated payloads on both sides of the limit: the
    // body arrives where a headline goes, and no graph record ends up holding
    // more than the limit of it.
    // A different unheard-of company each time: a half-absorbed item then
    // leaves its new entity node behind, where reusing one company would let
    // a partial write hide behind the node the last item already wrote.
    for (seed, company, length) in [
        (2, "Quillhaven Ore Partners", EXCERPT_LIMIT - 1),
        (3, "Marrowgate Shipping Lines", EXCERPT_LIMIT),
        (4, "Dunmere Glassworks", EXCERPT_LIMIT + 1),
        (5, "Oswin Saltmarsh Brewery", EXCERPT_LIMIT * 2),
        (6, "Tavistock Peat Collective", EXCERPT_LIMIT * 20),
    ] {
        let body = payload(seed, length);
        let nodes = model.graph().node_count();
        let facts = model.graph().fact_count();
        let journal = model.changes().len();
        let indexed = model.index().len();

        let item = news(&format!("item-{seed}"), company, body.clone(), &body);
        let outcome = model.absorb_news(&item, &context);

        if length > EXCERPT_LIMIT {
            let refusal = outcome.expect_err("a body where a headline goes is refused");
            assert!(
                refusal.message().contains("excerpt limit"),
                "the refusal does not say why: {}",
                refusal.message()
            );
            // Refused whole: nothing half-written anywhere in the model.
            assert_eq!(
                model.graph().node_count(),
                nodes,
                "{length}: a node was written"
            );
            assert_eq!(
                model.graph().fact_count(),
                facts,
                "{length}: a fact was written"
            );
            assert_eq!(
                model.changes().len(),
                journal,
                "{length}: the journal moved"
            );
            assert_eq!(
                model.index().len(),
                indexed,
                "{length}: the index took the body"
            );
        } else {
            outcome.expect("a headline within the limit is absorbed");
            assert!(
                model.graph().node_count() > nodes,
                "{length}: nothing was written"
            );
        }

        for held in graph_strings(model.graph()) {
            let shared = longest_shared_span(&held, &body);
            assert!(
                shared <= EXCERPT_LIMIT,
                "a graph record holds {shared} verbatim characters of a {length}-character \
                 payload, over the {EXCERPT_LIMIT}-character excerpt limit"
            );
        }
    }
}

#[test]
fn a_body_in_a_news_items_id_or_source_refuses_the_item_whole_rather_than_keeping_it_without_its_facts()
 {
    // The headline is not the only place a body can ride in. The item id
    // becomes the event node's id and the provenance source every fact's
    // source, and the graph refuses a body in either. Refused at the fact
    // alone, the item was half-kept: the mention dropped in silence, the
    // index entry and the journal line written anyway, and the call
    // reporting success.
    let (context, _clock) = Context::deterministic(now(), 7);
    let mut model = WorldModel::new();
    seed_demo_world(&mut model, &context).unwrap();

    // Premise: the fixture is absorbed when nothing in it is a body.
    let headline = || "Hesketh Lanolin Works cut full year guidance".to_string();
    let well_formed = news(
        "ok",
        "Hesketh Lanolin Works",
        headline(),
        "The outlook fell.",
    );
    assert_eq!(
        model.absorb_news(&well_formed, &context).unwrap().len(),
        1,
        "premise: a well-formed item resolves its mention"
    );

    // An id short enough to pass on its own and too long as the graph holds
    // it ("news:" and then the id), a body as the id, and a body as the
    // provenance source.
    let id_at_the_edge = payload(12, EXCERPT_LIMIT - 2);
    let body = payload(13, EXCERPT_LIMIT + 1);
    let mut body_as_source = news(
        "src",
        "Ardmore Kelp Traders",
        headline(),
        "The outlook fell.",
    );
    body_as_source.provenance = Provenance::synthetic(body.clone(), now());
    let refused = [
        news(
            &id_at_the_edge,
            "Corran Slate Quarries",
            headline(),
            "The outlook fell.",
        ),
        news(
            &body,
            "Penhallow Tin Streamers",
            headline(),
            "The outlook fell.",
        ),
        body_as_source,
    ];
    for item in refused {
        let before = (
            model.graph().node_count(),
            model.graph().fact_count(),
            model.changes().len(),
            model.index().len(),
        );
        let refusal = model
            .absorb_news(&item, &context)
            .expect_err("an item carrying a body in its id or source reported success");
        assert!(
            refusal.message().contains("excerpt limit"),
            "{}",
            refusal.message()
        );
        let after = (
            model.graph().node_count(),
            model.graph().fact_count(),
            model.changes().len(),
            model.index().len(),
        );
        assert_eq!(
            after, before,
            "a refused item was half-kept (nodes, facts, journal, index)"
        );
    }
}

#[test]
fn a_graph_write_carrying_a_payload_body_is_refused_and_leaves_the_graph_as_it_was() {
    let body = payload(9, EXCERPT_LIMIT + 1);
    let within = payload(9, EXCERPT_LIMIT);
    let mut graph = KnowledgeGraph::new();

    // Premise: the same writes at the limit are admitted, so each refusal
    // below is about the length and not about the write.
    graph
        .add_node(
            Node::new("ok", NodeKind::Evidence, within.clone(), now())
                .with_attribute("note", within.clone()),
        )
        .expect("a label and an attribute at the limit are admitted");
    assert!(
        Fact::new(
            Relationship::new("ok", "b", RelationshipKind::Issues, 0.5, within.clone()),
            now(),
            now(),
            0.5,
        )
        .is_ok(),
        "a source at the limit is admitted"
    );
    assert_eq!(graph.node_count(), 1);

    // A body as the label, as an attribute, and as the id.
    for refused in [
        Node::new("label", NodeKind::Evidence, body.clone(), now()),
        Node::new("attribute", NodeKind::Evidence, "short", now())
            .with_attribute("text", body.clone()),
        Node::new(body.clone(), NodeKind::Evidence, "short", now()),
    ] {
        let id = refused.id.clone();
        assert!(
            graph.add_node(refused).is_err(),
            "a node carrying a payload body was written"
        );
        assert!(
            graph.node(&id).is_none(),
            "the refused node is in the graph"
        );
    }
    assert_eq!(graph.node_count(), 1, "a refused write changed the graph");

    // A body as a fact's source: a reference is not a place to keep the text.
    assert!(
        Fact::new(
            Relationship::new("ok", "b", RelationshipKind::Issues, 0.5, body),
            now(),
            now(),
            0.5,
        )
        .is_err(),
        "a fact whose source is a payload body was admitted"
    );
}
