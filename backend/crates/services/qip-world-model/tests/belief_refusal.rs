//! WORLD-008 and WORLD-056: a belief is stated with a confidence and a source,
//! or it is refused. Never defaulted, never clamped.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::testing::approx_eq;
use qip_core::{Context, Duration, Timestamp};
use qip_financial::intelligence::{EntityMention, NewsItem, NewsSource, Sentiment};
use qip_financial::manifest::SourceManifest;
use qip_financial::quality::{DataQuality, Provenance};
use qip_world_model::graph::{Fact, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use qip_world_model::world::{WorldModel, seed_demo_world};

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn days_ago(days: i64) -> Timestamp {
    now().saturating_sub(Duration::from_days(days))
}

fn claim(weight: f64, source: &str) -> Relationship {
    Relationship::new("a", "b", RelationshipKind::Supplies, weight, source)
}

#[test]
fn a_fact_whose_stated_confidence_is_outside_zero_to_one_or_not_a_number_is_refused_not_clamped() {
    // Premise: the bounds are admitted and come back unchanged, so the
    // refusals below are about the value and not about the constructor.
    for admitted in [0.0, 0.37, 1.0] {
        let fact = Fact::new(claim(0.5, "filings"), days_ago(5), days_ago(5), admitted).unwrap();
        assert!(
            approx_eq(fact.confidence, admitted, 1e-12),
            "a valid confidence is stored as stated"
        );
    }
    // 2.0 once became certainty and NaN passed straight through a clamp.
    for refused in [
        1.0000001,
        2.0,
        -0.5,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        let result = Fact::new(claim(0.5, "filings"), days_ago(5), days_ago(5), refused);
        assert!(result.is_err(), "confidence {refused} must be refused");
    }
}

#[test]
fn a_fact_with_an_out_of_range_relationship_weight_or_no_source_is_refused() {
    assert!(Fact::new(claim(0.5, "filings"), days_ago(5), days_ago(5), 0.5).is_ok());
    for weight in [1.5, -0.1, f64::NAN] {
        assert!(
            Fact::new(claim(weight, "filings"), days_ago(5), days_ago(5), 0.5).is_err(),
            "weight {weight} must be refused, not clamped"
        );
    }
    for source in ["", "   "] {
        assert!(
            Fact::new(claim(0.5, source), days_ago(5), days_ago(5), 0.5).is_err(),
            "a claim naming no source ({source:?}) must be refused"
        );
    }
}

fn one_way(weight: f64) -> Relationship {
    // `Issues` has no inverse, so one write is one fact.
    Relationship::new("a", "b", RelationshipKind::Issues, weight, "filings")
}

#[test]
fn a_refused_relationship_leaves_neither_a_fact_nor_a_journal_entry() {
    let mut world = WorldModel::new();
    assert_eq!(world.graph().fact_count(), 0, "premise: an empty graph");
    let journal_before = world.changes().len();

    for confidence in [f64::NAN, 1.5] {
        assert!(
            world
                .relate(one_way(0.5), days_ago(5), days_ago(5), confidence)
                .is_err()
        );
    }
    assert_eq!(world.graph().fact_count(), 0);
    assert_eq!(world.changes().len(), journal_before);

    world
        .relate(one_way(0.5), days_ago(5), days_ago(5), 0.6)
        .unwrap();
    assert_eq!(world.graph().fact_count(), 1, "a valid one is written");
    assert_eq!(world.changes().len(), journal_before + 1);
}

fn news_mentioning_northwind(confidence: f64) -> NewsItem {
    NewsItem {
        item_id: "news-conf".into(),
        headline: "Northwind Semiconductor Corporation cut full year revenue guidance".into(),
        manifest: SourceManifest::generated("test", "news-conf", now(), "The outlook fell."),
        source: NewsSource::CompanyAnnouncement,
        published_at: now(),
        entities: vec![EntityMention {
            text: "Northwind Semiconductor Corporation".into(),
            entity_id: None,
            confidence,
            is_primary: true,
            sentiment: None,
            kind: Default::default(),
            identifiers: Default::default(),
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
fn a_news_mention_is_stored_at_its_own_confidence_and_one_with_no_valid_confidence_is_dropped() {
    let (context, _clock) = Context::deterministic(now(), 7);
    let mut model = WorldModel::new();
    seed_demo_world(&mut model, &context).unwrap();
    let concerns = |m: &WorldModel| {
        m.graph()
            .facts_at(now(), now())
            .into_iter()
            .filter(|f| f.relationship.kind == RelationshipKind::ConcernsEntity)
            .map(|f| f.confidence)
            .collect::<Vec<f64>>()
    };
    assert!(concerns(&model).is_empty(), "premise: no news fact yet");

    // A mention with no usable confidence used to be stored at 1.0.
    assert!(
        model
            .absorb_news(&news_mentioning_northwind(f64::NAN), &context)
            .unwrap()
            .is_empty(),
        "a refused mention resolves nothing"
    );
    assert!(
        model
            .absorb_news(&news_mentioning_northwind(7.0), &context)
            .unwrap()
            .is_empty()
    );
    assert!(concerns(&model).is_empty(), "and writes no fact");

    let kept = model
        .absorb_news(&news_mentioning_northwind(0.94), &context)
        .unwrap();
    assert_eq!(
        kept.len(),
        1,
        "premise: the same item with a valid confidence resolves"
    );
    let stored = concerns(&model);
    assert_eq!(stored.len(), 1);
    assert!(
        approx_eq(stored[0], 0.94, 1e-12),
        "stored at the mention's confidence"
    );
}

#[test]
fn the_graph_schema_names_a_kind_for_each_of_the_eight_knowledge_categories() {
    // entities, beliefs (a thesis), source references (a resolution source),
    // and evidence are node kinds.
    let kinds = [
        NodeKind::Entity,
        NodeKind::Thesis,
        NodeKind::ResolutionSource,
        NodeKind::Evidence,
    ];
    assert_eq!(
        kinds
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    // relationships and contradictions are edge kinds.
    assert_eq!(RelationshipKind::Supplies.as_str(), "supplies");
    assert_eq!(
        RelationshipKind::ContradictsThesis.as_str(),
        "contradicts_thesis"
    );
    // temporal facts carry both time ranges, a source and a confidence.
    let fact = Fact::new(claim(0.5, "filings"), days_ago(9), days_ago(8), 0.5).unwrap();
    assert_eq!(fact.relationship.source, "filings");
    assert!(fact.recorded_at > fact.valid_from);
    assert!(fact.valid_to.is_none() && fact.retracted_at.is_none());
}
