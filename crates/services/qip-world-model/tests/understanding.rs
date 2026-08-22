//! The world model: bitemporality, causal propagation, point-in-time features.

use qip_core::testing::approx_eq;
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp};
use qip_financial::intelligence::{
    EntityMention, FiscalPeriod, FundamentalUpdate, MacroObservation, NewsItem, NewsSource,
    Sentiment,
};
use qip_financial::quality::{DataQuality, Provenance};
use qip_market::bar::{Bar, Interval};
use qip_world_model::causal::{CausalEdge, CausalGraph, Mechanism};
use qip_world_model::features::{Feature, FeatureStore, FeatureValue};
use qip_world_model::graph::{Fact, KnowledgeGraph, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use qip_world_model::state::ChangeKind;
use qip_world_model::world::{WorldModel, seed_demo_world};

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn context() -> Context {
    let (context, _clock) = Context::deterministic(now(), 7);
    context
}

fn days_ago(days: i64) -> Timestamp {
    now().saturating_sub(Duration::from_days(days))
}

// --- bitemporality ----------------------------------------------------------

#[test]
fn a_fact_is_invisible_before_it_was_known_even_if_it_was_already_true() {
    // The whole point of two timestamps. The supply relationship began in
    // January but the platform only learned of it in June; a decision made in
    // March must not see it.
    let began = Timestamp::from_civil(2026, 1, 15);
    let learned = Timestamp::from_civil(2026, 6, 1);
    let march = Timestamp::from_civil(2026, 3, 1);

    let fact = Fact::new(
        Relationship::new("a", "b", RelationshipKind::Supplies, 0.5, "filings"),
        began,
        learned,
    );

    assert!(!fact.holds(march, march), "not knowable in March");
    assert!(
        fact.holds(march, now()),
        "knowable now, and it was true in March"
    );
    assert!(fact.holds_now(now()));
}

#[test]
fn a_fact_outside_its_validity_window_does_not_hold() {
    let fact = Fact::new(
        Relationship::new("a", "b", RelationshipKind::Supplies, 0.5, "filings"),
        days_ago(100),
        days_ago(100),
    )
    .valid_until(days_ago(30));

    assert!(fact.holds(days_ago(50), now()), "inside the window");
    assert!(!fact.holds(days_ago(10), now()), "after the window closed");
    assert!(!fact.holds(days_ago(200), now()), "before it began");
}

#[test]
fn a_retracted_fact_is_hidden_going_forward_but_not_rewritten_backwards() {
    // A decision made while the fact was believed must remain explicable.
    let mut graph = KnowledgeGraph::new();
    graph.add_node(Node::new("a", NodeKind::Entity, "A", days_ago(100)));
    graph.add_node(Node::new("b", NodeKind::Entity, "B", days_ago(100)));
    let relationship = Relationship::new("a", "b", RelationshipKind::Supplies, 0.5, "filings");
    let key = relationship.key();
    graph.assert_fact(Fact::new(relationship, days_ago(100), days_ago(100)));

    assert_eq!(
        graph
            .neighbours("a", None, days_ago(50), days_ago(50))
            .len(),
        1
    );
    assert!(graph.retract(&key, days_ago(20)));

    assert_eq!(
        graph
            .neighbours("a", None, days_ago(50), days_ago(50))
            .len(),
        1,
        "what was believed at the time is unchanged"
    );
    assert_eq!(
        graph.neighbours("a", None, now(), now()).len(),
        0,
        "it is no longer believed"
    );
    assert!(!graph.retract("nonexistent", now()));
}

#[test]
fn an_inverse_edge_is_asserted_automatically() {
    let mut graph = KnowledgeGraph::new();
    graph.assert_fact(Fact::new(
        Relationship::new(
            "supplier",
            "buyer",
            RelationshipKind::Supplies,
            0.6,
            "filings",
        ),
        days_ago(10),
        days_ago(10),
    ));

    let forward = graph.neighbours("supplier", Some(RelationshipKind::Supplies), now(), now());
    assert_eq!(forward.len(), 1);
    let backward = graph.neighbours("buyer", Some(RelationshipKind::Customer), now(), now());
    assert_eq!(backward.len(), 1, "the customer edge is implied");

    assert_eq!(
        RelationshipKind::Supplies.inverse(),
        Some(RelationshipKind::Customer)
    );
    assert!(RelationshipKind::Competitor.is_symmetric());
}

// --- graph traversal --------------------------------------------------------

fn chain_graph() -> KnowledgeGraph {
    let mut graph = KnowledgeGraph::new();
    for id in ["kestrel", "northwind", "vantage", "meridian"] {
        graph.add_node(Node::new(id, NodeKind::Entity, id, days_ago(400)));
    }
    graph.assert_fact(Fact::new(
        Relationship::new(
            "kestrel",
            "northwind",
            RelationshipKind::Supplies,
            0.4,
            "filings",
        ),
        days_ago(400),
        days_ago(400),
    ));
    graph.assert_fact(Fact::new(
        Relationship::new(
            "northwind",
            "vantage",
            RelationshipKind::Supplies,
            0.6,
            "filings",
        ),
        days_ago(400),
        days_ago(400),
    ));
    graph.assert_fact(Fact::new(
        Relationship::new(
            "northwind",
            "meridian",
            RelationshipKind::Competitor,
            0.2,
            "research",
        ),
        days_ago(400),
        days_ago(400),
    ));
    graph
}

#[test]
fn paths_are_found_shortest_first_and_carry_their_strength() {
    let graph = chain_graph();
    let paths = graph.paths_between("kestrel", "vantage", 4, now(), now());
    assert!(!paths.is_empty());
    assert_eq!(paths[0].nodes, vec!["kestrel", "northwind", "vantage"]);
    assert_eq!(paths[0].length(), 2);
    assert!(approx_eq(paths[0].strength, 0.4 * 0.6, 1e-9));
}

#[test]
fn path_search_is_bounded_and_avoids_cycles() {
    let graph = chain_graph();
    assert!(
        graph
            .paths_between("kestrel", "vantage", 1, now(), now())
            .is_empty(),
        "two hops cannot be found within one"
    );
    assert!(
        graph
            .paths_between("kestrel", "kestrel", 3, now(), now())
            .is_empty()
    );
    // Competitor edges are symmetric, so a naive search would loop forever.
    let paths = graph.paths_between("meridian", "vantage", 4, now(), now());
    for path in &paths {
        let mut sorted = path.nodes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), path.nodes.len(), "a path revisited a node");
    }
}

#[test]
fn reachability_reports_hop_counts() {
    let graph = chain_graph();
    let reachable = graph.reachable("kestrel", 3, now(), now());
    assert_eq!(reachable.get("northwind"), Some(&1));
    assert_eq!(reachable.get("vantage"), Some(&2));
    assert!(
        !reachable.contains_key("kestrel"),
        "the origin is not its own neighbour"
    );
}

#[test]
fn hubs_are_the_most_connected_nodes() {
    let graph = chain_graph();
    let hubs = graph.most_connected(2, now(), now());
    assert_eq!(hubs[0].0.id, "northwind", "the hub of the chain");
    assert!(hubs[0].1 >= 3);
}

// --- causal propagation -----------------------------------------------------

fn causal_chain() -> CausalGraph {
    let mut causal = CausalGraph::new();
    causal.add(
        CausalEdge::new(
            "kestrel",
            "northwind",
            Mechanism::InputCost,
            0.5,
            Duration::from_days(7),
            days_ago(300),
        )
        .with_confidence(0.8)
        .with_evidence(vec!["filing:input-costs".into()]),
    );
    causal.add(
        CausalEdge::new(
            "northwind",
            "vantage",
            Mechanism::SupplyChain,
            0.6,
            Duration::from_days(3),
            days_ago(300),
        )
        .with_confidence(0.75)
        .with_evidence(vec!["filing:supplier-concentration".into()]),
    );
    causal.add(
        CausalEdge::new(
            "northwind",
            "meridian",
            Mechanism::CompetitiveSubstitution,
            0.3,
            Duration::from_days(5),
            days_ago(300),
        )
        .with_confidence(0.5)
        .with_evidence(vec!["research:substitution".into()]),
    );
    causal
}

#[test]
fn a_shock_propagates_and_attenuates_with_each_hop() {
    let causal = causal_chain();
    let result = causal.propagate("kestrel", -0.10, 3, 0.001, now(), now());

    let first = result.at_order(1);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].target, "northwind");
    // -10% through 0.5 strength at 0.8 confidence.
    assert!(approx_eq(first[0].magnitude, -0.10 * 0.5 * 0.8, 1e-9));

    let second = result.at_order(2);
    assert_eq!(second.len(), 2, "vantage and meridian");
    let vantage = second.iter().find(|e| e.target == "vantage").unwrap();
    assert!(
        vantage.magnitude.abs() < first[0].magnitude.abs(),
        "the effect must attenuate"
    );
    assert_eq!(vantage.order, 2);
    assert_eq!(vantage.chain.len(), 2);
}

#[test]
fn competitive_substitution_flips_the_sign() {
    // A rival's loss is a gain. Treating it as same-signed produces theses that
    // are exactly backwards.
    let causal = causal_chain();
    let result = causal.propagate("northwind", -0.20, 2, 0.001, now(), now());

    let vantage = result
        .effects
        .iter()
        .find(|e| e.target == "vantage")
        .unwrap();
    assert!(
        vantage.magnitude < 0.0,
        "a supplier shock hurts the customer"
    );

    let meridian = result
        .effects
        .iter()
        .find(|e| e.target == "meridian")
        .unwrap();
    assert!(
        meridian.magnitude > 0.0,
        "a competitor's loss is a gain: {}",
        meridian.magnitude
    );
    assert!(!Mechanism::CompetitiveSubstitution.preserves_sign());
}

#[test]
fn effects_are_timed_by_the_accumulated_lag() {
    let causal = causal_chain();
    let result = causal.propagate("kestrel", -0.10, 3, 0.001, now(), now());
    let vantage = result
        .effects
        .iter()
        .find(|e| e.target == "vantage")
        .unwrap();
    // Seven days to reach Northwind, three more to reach Vantage.
    assert_eq!(
        vantage.expected_at,
        now().saturating_add(Duration::from_days(10))
    );
}

#[test]
fn a_negligible_effect_is_dropped_and_counted() {
    let causal = causal_chain();
    // A floor above the second-order magnitude cuts the chain.
    let result = causal.propagate("kestrel", -0.10, 3, 0.03, now(), now());
    assert!(
        result.at_order(2).is_empty(),
        "second order falls below the floor"
    );
    assert!(
        result.truncated > 0,
        "the caller must know the chain was cut"
    );
}

#[test]
fn propagation_terminates_on_a_cycle() {
    let mut causal = CausalGraph::new();
    causal.add(CausalEdge::new(
        "a",
        "b",
        Mechanism::Sentiment,
        0.9,
        Duration::ZERO,
        days_ago(10),
    ));
    causal.add(CausalEdge::new(
        "b",
        "a",
        Mechanism::Sentiment,
        0.9,
        Duration::ZERO,
        days_ago(10),
    ));
    let result = causal.propagate("a", 1.0, 10, 1e-6, now(), now());
    assert!(
        result.effects.len() <= 2,
        "a cycle must not amplify without limit"
    );
}

#[test]
fn a_causal_claim_recorded_later_is_invisible_earlier() {
    let mut causal = CausalGraph::new();
    causal.add(CausalEdge::new(
        "a",
        "b",
        Mechanism::SupplyChain,
        0.5,
        Duration::ZERO,
        now(),
    ));
    assert!(
        causal
            .propagate("a", 1.0, 2, 1e-6, days_ago(10), days_ago(10))
            .is_empty()
    );
    assert!(!causal.propagate("a", 1.0, 2, 1e-6, now(), now()).is_empty());
}

#[test]
fn explanations_rank_the_most_likely_cause_first() {
    let causal = causal_chain();
    let explanations = causal.explanations("northwind", now());
    assert_eq!(explanations.len(), 1);
    assert_eq!(explanations[0].cause, "kestrel");
    assert!(explanations[0].is_evidenced());
}

#[test]
fn unevidenced_claims_are_surfaced() {
    let mut causal = causal_chain();
    causal.add(CausalEdge::new(
        "x",
        "y",
        Mechanism::Sentiment,
        0.9,
        Duration::ZERO,
        days_ago(1),
    ));
    let unevidenced = causal.unevidenced();
    assert_eq!(unevidenced.len(), 1);
    assert_eq!(unevidenced[0].cause, "x");
}

#[test]
fn an_effect_explains_itself_in_words() {
    let causal = causal_chain();
    let result = causal.propagate("kestrel", -0.10, 3, 0.001, now(), now());
    let vantage = result
        .effects
        .iter()
        .find(|e| e.target == "vantage")
        .unwrap();
    let explanation = vantage.explain();
    assert!(explanation.contains("order 2"));
    assert!(explanation.contains("kestrel"));
    assert!(explanation.contains("northwind"));
    assert!(explanation.contains("input") || explanation.contains("supply"));
}

// --- point-in-time features -------------------------------------------------

#[test]
fn a_feature_is_invisible_before_it_became_available() {
    // A quarterly figure describes March but only arrives in May. A backtest
    // reading it in April is reading data that did not exist.
    let mut store = FeatureStore::new();
    store.define(
        Feature::new("revenue", "quarterly revenue", "fundamentals")
            .with_staleness(Duration::from_days(200)),
    );
    let period_end = Timestamp::from_civil(2026, 3, 31);
    let published = Timestamp::from_civil(2026, 5, 12);
    store.record(
        "revenue",
        "ent-northwind",
        FeatureValue::new(4200.0, period_end, published),
    );

    let april = Timestamp::from_civil(2026, 4, 15);
    assert!(
        store
            .value_as_of("revenue", "ent-northwind", april, april)
            .is_none(),
        "not yet published"
    );
    assert!(
        store
            .value_as_of("revenue", "ent-northwind", period_end, published)
            .is_some(),
        "available once published"
    );
    // And retrospectively, we can ask what was true in March knowing what we
    // know now.
    assert!(
        store
            .value_as_of("revenue", "ent-northwind", period_end, now())
            .is_some()
    );
}

#[test]
fn a_stale_feature_is_not_returned() {
    let mut store = FeatureStore::new();
    store.define(
        Feature::new("close", "last price", "market").with_staleness(Duration::from_days(5)),
    );
    store.record(
        "close",
        "obj-a",
        FeatureValue::immediate(100.0, days_ago(30)),
    );
    assert!(
        store.current("close", "obj-a", now()).is_none(),
        "a month-old price is not a price"
    );
    assert!(
        store
            .value_as_of("close", "obj-a", days_ago(28), now())
            .is_some()
    );
}

#[test]
fn a_restatement_replaces_the_value_for_the_same_instant() {
    let mut store = FeatureStore::new();
    store.define(
        Feature::new("revenue", "revenue", "fundamentals").with_staleness(Duration::from_days(400)),
    );
    let period = days_ago(90);
    store.record(
        "revenue",
        "e",
        FeatureValue::new(100.0, period, days_ago(60)),
    );
    store.record(
        "revenue",
        "e",
        FeatureValue::new(95.0, period, days_ago(10)),
    );
    assert_eq!(
        store.history("revenue", "e", now()).len(),
        1,
        "one instant, one value"
    );
    assert!(approx_eq(
        store.current("revenue", "e", now()).unwrap().value,
        95.0,
        1e-9
    ));
}

#[test]
fn a_missing_feature_is_reported_rather_than_defaulted_to_zero() {
    // Substituting zero for unknown is how a model ends up trained on a fact
    // that was never true.
    let mut store = FeatureStore::new();
    store.define(Feature::new("close", "price", "market").with_staleness(Duration::from_days(5)));
    store.define(
        Feature::new("revenue", "revenue", "fundamentals").with_staleness(Duration::from_days(400)),
    );
    store.record("close", "obj-a", FeatureValue::immediate(100.0, now()));

    let (values, missing) = store.vector_as_of(
        &["close".to_string(), "revenue".to_string()],
        "obj-a",
        now(),
        now(),
    );
    assert!(approx_eq(values[0], 100.0, 1e-9));
    assert!(values[1].is_nan(), "unknown is not zero");
    assert_eq!(missing, vec!["revenue".to_string()]);
}

#[test]
fn a_cross_section_ranks_subjects_by_feature() {
    let mut store = FeatureStore::new();
    store.define(Feature::new("close", "price", "market").with_staleness(Duration::from_days(5)));
    for (subject, value) in [("a", 10.0), ("b", 30.0), ("c", 20.0)] {
        store.record("close", subject, FeatureValue::immediate(value, now()));
    }
    let ranked = store.cross_section("close", now(), now());
    assert_eq!(ranked[0].0, "b");
    assert_eq!(ranked[2].0, "a");
    assert_eq!(store.subjects_with("close", now(), now()).len(), 3);
}

// --- the world model --------------------------------------------------------

fn seeded_model() -> (WorldModel, Context) {
    let context = context();
    let mut model = WorldModel::new();
    seed_demo_world(&mut model, &context).unwrap();
    (model, context)
}

#[test]
fn the_demo_world_seeds_a_connected_graph() {
    let (model, _) = seeded_model();
    let statistics = model.statistics();
    assert!(statistics["nodes"] >= 7, "{statistics:?}");
    assert!(statistics["causal_claims"] >= 4);

    // Kestrel reaches Vantage through Northwind.
    let paths = model
        .graph()
        .paths_between("ent-kestrel", "ent-vantage", 3, now(), now());
    assert!(!paths.is_empty(), "the supply chain must connect");
}

#[test]
fn a_shock_at_the_supplier_reaches_the_customers_customer() {
    // The second- and third-order effects the charter asks for, produced by a
    // mechanism rather than by correlation.
    let (model, _) = seeded_model();
    let result = model.propagate("ent-kestrel", -0.15, 3, now(), now());

    assert!(!result.is_empty());
    let targets: Vec<&str> = result.effects.iter().map(|e| e.target.as_str()).collect();
    assert!(targets.contains(&"ent-northwind"), "{targets:?}");
    assert!(targets.contains(&"ent-vantage"), "{targets:?}");

    let strongest = result.strongest(1)[0];
    assert_eq!(
        strongest.target, "ent-northwind",
        "the direct effect is the largest"
    );
}

#[test]
fn absorbing_news_resolves_entities_and_indexes_the_document() {
    let (mut model, context) = seeded_model();
    let item = NewsItem {
        item_id: "news-1".into(),
        headline: "Northwind Semiconductor Corporation cut full year revenue guidance".into(),
        body: "The company reduced its outlook, citing weak demand for its components.".into(),
        source: NewsSource::CompanyAnnouncement,
        published_at: now(),
        entities: vec![EntityMention {
            text: "Northwind Semiconductor Corporation".into(),
            entity_id: None,
            confidence: 0.94,
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
    };

    let resolved = model.absorb_news(&item, &context);
    assert_eq!(
        resolved,
        vec!["ent-northwind".to_string()],
        "resolved onto the seeded entity"
    );

    // The document is retrievable as evidence.
    let hits = model.retrieve("revenue guidance cut", 3, now());
    assert!(!hits.is_empty());
    assert_eq!(hits[0].document.id, "news:news-1");

    // And sentiment became a feature of the entity.
    let sentiment = model
        .features()
        .current("sentiment", "ent-northwind", now())
        .expect("sentiment recorded");
    assert!(sentiment.value < 0.0);
}

#[test]
fn absorbing_a_fundamental_records_the_publication_lag() {
    let (mut model, _) = seeded_model();
    let period_end = days_ago(45);
    let published = days_ago(5);
    let mut provenance = Provenance::synthetic("fundamentals", period_end);
    provenance.ingestion_time = published;

    let update = FundamentalUpdate {
        entity_id: "ent-northwind".into(),
        metric: "revenue".into(),
        value: Decimal::from_int(4600),
        unit: "USD_millions".into(),
        period_end,
        period: FiscalPeriod::Quarter,
        consensus: Some(Decimal::from_int(4200)),
        prior_value: Some(Decimal::from_int(4000)),
        is_restatement: false,
        provenance,
        quality: DataQuality::clean(),
    };
    model.absorb_fundamental(&update);

    // Invisible before publication, visible after.
    assert!(
        model
            .features()
            .value_as_of("revenue", "ent-northwind", period_end, days_ago(20))
            .is_none()
    );
    let value = model
        .features()
        .value_as_of("revenue", "ent-northwind", period_end, now())
        .expect("visible once published");
    assert!(approx_eq(value.value, 4600.0, 1e-9));
    assert_eq!(value.availability_lag(), Duration::from_days(40));

    let surprise = model
        .features()
        .value_as_of("revenue_surprise", "ent-northwind", period_end, now())
        .unwrap();
    assert!(approx_eq(surprise.value, 400.0 / 4200.0, 1e-9));
}

#[test]
fn absorbing_bars_builds_a_price_history_and_a_volatility_feature() {
    let (mut model, _) = seeded_model();
    let object = ObjectId::from_string("OBJ00000000000000000NWSC");
    let mut price = 100.0;
    for day in 0..40 {
        price *= if day % 2 == 0 { 1.015 } else { 0.99 };
        let bar = Bar {
            object_id: object.clone(),
            venue: "XNYS".into(),
            interval: Interval::Day,
            open_time: days_ago(40 - day),
            open: Decimal::from_f64(price).unwrap(),
            high: Decimal::from_f64(price * 1.01).unwrap(),
            low: Decimal::from_f64(price * 0.99).unwrap(),
            close: Decimal::from_f64(price).unwrap(),
            volume: Decimal::from_int(1_000_000),
            vwap: None,
            trade_count: 5_000,
            quality: DataQuality::clean(),
        };
        // A daily bar this test builds itself is knowable the moment its
        // period closes; a bar off a wire would pass its arrival time.
        let known_at = bar.close_time();
        model.absorb_bar(&bar, known_at);
    }
    model.recompute_volatility(object.as_str(), 20, now());

    let volatility = model
        .features()
        .current("realised_volatility_20d", object.as_str(), now())
        .expect("volatility computed");
    assert!(
        volatility.value > 0.0 && volatility.value < 3.0,
        "vol {}",
        volatility.value
    );
}

#[test]
fn the_world_state_can_be_reconstructed_at_a_past_instant() {
    let (mut model, context) = seeded_model();
    let macro_update = MacroObservation {
        series_id: "US.CPI.YOY".into(),
        region: "US".into(),
        value: 3.4,
        unit: "percent".into(),
        reference_date: days_ago(10),
        consensus: Some(3.0),
        previous: Some(2.9),
        is_revision: false,
        provenance: Provenance::synthetic("macro", days_ago(2)),
        quality: DataQuality::clean(),
    };
    model.absorb_macro(&macro_update);
    let _ = &context;

    let current = model.state_at(now(), now());
    assert!(current.entity_count >= 7);
    assert!(current.relationship_count > 0);
    assert!(current.summary().contains("entities"));

    // A month ago the macro reading had not been published.
    let past = model.state_at(days_ago(30), days_ago(30));
    assert!(
        !past.features.keys().any(|k| k.starts_with("macro_level/")),
        "the macro print was not known a month ago"
    );
    assert!(
        current
            .features
            .keys()
            .any(|k| k.starts_with("macro_level/"))
    );
}

#[test]
fn the_diff_reports_what_changed_between_two_instants() {
    let (mut model, context) = seeded_model();
    let before = now();

    let item = NewsItem {
        item_id: "news-2".into(),
        headline: "Northwind Semiconductor Corporation warns on production".into(),
        body: "A disruption is expected to affect output for several weeks.".into(),
        source: NewsSource::RegulatoryFiling,
        published_at: before.saturating_add(Duration::from_hours(1)),
        entities: vec![EntityMention {
            text: "Northwind Semiconductor Corporation".into(),
            entity_id: None,
            confidence: 0.95,
            is_primary: true,
            sentiment: None,
        }],
        sentiment: Sentiment {
            polarity: -0.8,
            confidence: 0.9,
            novelty: 0.9,
        },
        topics: vec!["supply_chain".into()],
        provenance: Provenance::synthetic("news", before.saturating_add(Duration::from_hours(1))),
        quality: DataQuality::clean(),
    };
    model.absorb_news(&item, &context);

    let diff = model.diff(before, before.saturating_add(Duration::from_hours(2)));
    assert!(!diff.is_empty(), "the news should register as a change");
    assert!(!diff.material(0.5).is_empty());
    assert!(diff.summary().contains("material"));
    assert!(!diff.of_kind(ChangeKind::EntityUpdated).is_empty());

    // Nothing happened in a window before the news.
    let quiet = model.diff(days_ago(2), days_ago(1));
    assert!(quiet.is_empty(), "{}", quiet.summary());
    assert!(quiet.summary().contains("nothing changed"));
}

#[test]
fn retrieval_from_the_world_model_respects_the_point_in_time_cutoff() {
    let (mut model, context) = seeded_model();
    for (id, when) in [("old", days_ago(10)), ("recent", days_ago(1))] {
        let item = NewsItem {
            item_id: id.into(),
            headline: format!("Northwind Semiconductor guidance update {id}"),
            body: "Guidance was revised.".into(),
            source: NewsSource::Newswire,
            published_at: when,
            entities: vec![EntityMention {
                text: "Northwind Semiconductor Corporation".into(),
                entity_id: None,
                confidence: 0.9,
                is_primary: true,
                sentiment: None,
            }],
            sentiment: Sentiment::neutral(),
            topics: Vec::new(),
            provenance: Provenance::synthetic("news", when),
            quality: DataQuality::clean(),
        };
        model.absorb_news(&item, &context);
    }

    let all = model.retrieve("guidance update", 5, now());
    assert_eq!(all.len(), 2);
    let historical = model.retrieve("guidance update", 5, days_ago(5));
    assert_eq!(
        historical.len(),
        1,
        "only the older document existed five days ago"
    );
    assert_eq!(historical[0].document.id, "news:old");
}

#[test]
fn the_model_is_deterministic() {
    let build = || {
        let context = context();
        let mut model = WorldModel::new();
        seed_demo_world(&mut model, &context).unwrap();
        let result = model.propagate("ent-kestrel", -0.15, 3, now(), now());
        result
            .effects
            .iter()
            .map(|e| format!("{}:{:.9}", e.target, e.magnitude))
            .collect::<Vec<_>>()
    };
    assert_eq!(build(), build());
}

#[test]
fn a_bar_off_a_wire_is_not_knowable_when_it_closed() {
    // The look-ahead this argument exists to prevent. A bar closes at the
    // venue and arrives here later; stamping availability at the close makes
    // every feature derived from it readable before it existed — and a
    // point-in-time read cannot catch that, because the record itself claims
    // to have been available.
    let (mut model, _) = seeded_model();
    let object = ObjectId::from_string("OBJ00000000000000000NWSC");
    let bar = Bar {
        object_id: object.clone(),
        venue: "XNYS".into(),
        interval: Interval::Day,
        open_time: days_ago(1),
        open: Decimal::from_int(100),
        high: Decimal::from_int(101),
        low: Decimal::from_int(99),
        close: Decimal::from_int(100),
        volume: Decimal::from_int(1_000_000),
        vwap: None,
        trade_count: 5_000,
        quality: DataQuality::clean(),
    };
    let closed = bar.close_time();
    let arrived = closed.saturating_add(qip_core::Duration::from_millis(250));
    model.absorb_bar(&bar, arrived);

    assert!(
        model
            .features()
            .current("close", object.as_str(), closed)
            .is_none(),
        "the close was readable a quarter of a second before the platform had it"
    );
    let known = model
        .features()
        .current("close", object.as_str(), arrived)
        .expect("readable once it had arrived");
    assert_eq!(
        known.availability_lag(),
        qip_core::Duration::from_millis(250),
        "the feed's latency was lost rather than recorded"
    );
    assert_eq!(known.valid_at, closed, "valid time is still the bar's own");
}

#[test]
fn a_bar_cannot_be_known_before_the_period_it_summarises_ended() {
    // The combination has no physical meaning and always means a clock or a
    // parser rather than a very fast feed, so it is clamped rather than
    // trusted.
    let (mut model, _) = seeded_model();
    let object = ObjectId::from_string("OBJ00000000000000000NWSC");
    let bar = Bar {
        object_id: object.clone(),
        venue: "XNYS".into(),
        interval: Interval::Day,
        open_time: days_ago(1),
        open: Decimal::from_int(100),
        high: Decimal::from_int(101),
        low: Decimal::from_int(99),
        close: Decimal::from_int(100),
        volume: Decimal::from_int(1_000_000),
        vwap: None,
        trade_count: 5_000,
        quality: DataQuality::clean(),
    };
    let closed = bar.close_time();
    model.absorb_bar(
        &bar,
        closed.saturating_sub(qip_core::Duration::from_hours(1)),
    );

    let value = model
        .features()
        .current("close", object.as_str(), closed)
        .expect("clamped forward to the close rather than dropped");
    assert_eq!(value.available_at, closed);
}
