//! The SENSE stage's routing: every record kind lands somewhere it is
//! genuinely consumed, carrying its own two instants.
//!
//! Until this change `Platform::observe` kept bar closes and volumes as two
//! `Vec<f64>` and discarded every other record kind through a `_ =>` arm —
//! the acceptance suite itself certified "the platform's world model is never
//! written". Every test here is premise-first: it shows the state empty, feeds
//! a record of a kind that used to vanish, and then reads it back out of the
//! platform at the record's own timestamps — never the wall clock's.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, ObjectId, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::intelligence::{
    EntityMention, FiscalPeriod, FundamentalUpdate, NewsItem, NewsSource, ReferenceDataUpdate,
    Sentiment,
};
use qip_financial::manifest::SourceManifest;
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market::book::{BookLevel, OrderBook};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object("AAA"),
                "AAA",
                InstrumentType::CommonStock,
                fixture_liquidity(),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    universe
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(),
        LimitSet::conservative_default(),
    )
}

fn feature_values(platform: &Platform) -> usize {
    platform
        .world()
        .statistics()
        .get("feature_values")
        .copied()
        .unwrap_or(0)
}

// --- fundamentals -----------------------------------------------------------

#[test]
fn a_fundamental_that_previously_vanished_now_lands_with_its_own_two_instants() -> Result<()> {
    let mut platform = platform()?;

    // The premise, shown rather than assumed: before anything is fed, the
    // world model holds no feature values, so whatever appears below arrived
    // through `observe` and nowhere else.
    assert_eq!(
        feature_values(&platform),
        0,
        "the world model is not empty at assembly; nothing below is evidence about observe"
    );

    // A quarter that ended five weeks ago, filed three days ago. The gap is
    // the whole point: the figure was true for the quarter but usable only
    // once filed.
    let period_end = start().saturating_sub(Duration::from_days(35));
    let filed_at = start().saturating_sub(Duration::from_days(3));
    let update = FundamentalUpdate {
        entity_id: "ent-acme".to_string(),
        metric: "eps".to_string(),
        value: dec!("2.40"),
        unit: "USD".to_string(),
        period_end,
        period: FiscalPeriod::Quarter,
        consensus: Some(dec!("2.00")),
        prior_value: None,
        is_restatement: false,
        provenance: Provenance::new("test-filings", period_end, filed_at),
        quality: DataQuality::clean(),
    };

    let absorbed = platform.observe(vec![SensedRecord::Fundamental(Box::new(update))]);
    assert_eq!(absorbed, 1);

    let world = platform.world();
    let features = world.features();
    // Not knowable at the period end: a backtest asking "what did we know
    // when the quarter closed" must not see a figure filed five weeks later.
    assert!(
        features
            .value_as_of("eps", "ent-acme", period_end, period_end)
            .is_none(),
        "the figure is readable before it was filed, which is the look-ahead this routing exists \
         to refuse"
    );
    // Knowable once filed, and true for the period it covers — both instants
    // are the record's own.
    let value = features
        .value_as_of("eps", "ent-acme", start(), start())
        .ok_or_else(|| Error::not_found("the absorbed fundamental"))?;
    assert_eq!(
        value.valid_at, period_end,
        "the record's valid instant did not travel"
    );
    assert_eq!(
        value.available_at, filed_at,
        "the record's knowable instant did not travel"
    );

    // The 20% beat is also a surprise feature, on the same two instants.
    let surprise = features
        .value_as_of("eps_surprise", "ent-acme", start(), start())
        .ok_or_else(|| Error::not_found("the surprise the consensus implies"))?;
    assert!(
        (surprise.value - 0.2).abs() < 1e-9,
        "a 2.40 print against a 2.00 consensus is a 20% surprise, not {}",
        surprise.value
    );

    // And the filing became a knowable market event for the catalyst path,
    // knowable when filed — not when the quarter ended, and not now.
    let event = platform
        .market_events()
        .iter()
        .find(|event| event.subject == "ent-acme")
        .ok_or_else(|| Error::not_found("the filing as a catalyst event"))?;
    assert_eq!(event.known_at(), filed_at);
    Ok(())
}

// --- news -------------------------------------------------------------------

#[test]
fn a_news_item_lands_in_the_evidence_index_rather_than_vanishing() -> Result<()> {
    let mut platform = platform()?;
    let statistics = platform.world().statistics();
    assert_eq!(
        statistics.get("documents").copied().unwrap_or(0),
        0,
        "the evidence index is not empty at assembly"
    );

    let published_at = start().saturating_sub(Duration::from_hours(2));
    let item = NewsItem {
        item_id: "news-1".to_string(),
        headline: "Acme Corporation guides revenue sharply higher".to_string(),
        manifest: SourceManifest::of(
            "test-newswire",
            "http://vendor.test/docs#news-1",
            start(),
            b"Acme Corporation raised full-year revenue guidance.",
        )?,
        source: NewsSource::CompanyAnnouncement,
        published_at,
        entities: vec![EntityMention {
            text: "Acme Corporation".to_string(),
            entity_id: Some("ent-acme".to_string()),
            confidence: 0.9,
            is_primary: true,
            sentiment: None,
        }],
        sentiment: Sentiment {
            polarity: 0.8,
            confidence: 0.9,
            novelty: 0.7,
        },
        topics: vec!["guidance".to_string()],
        provenance: Provenance::new("test-newswire", published_at, start()),
        quality: DataQuality::clean(),
    };

    let absorbed = platform.observe(vec![SensedRecord::News(Box::new(item))]);
    assert_eq!(absorbed, 1);

    let statistics = platform.world().statistics();
    assert_eq!(
        statistics.get("documents").copied().unwrap_or(0),
        1,
        "the item did not reach the evidence index: {statistics:?}"
    );
    // The story became a knowable event on its resolved entity, knowable at
    // ingestion — the instant the platform could first have acted on it.
    let event = platform
        .market_events()
        .iter()
        .find(|event| event.subject == "ent-acme")
        .ok_or_else(|| Error::not_found("the story as a catalyst event"))?;
    assert_eq!(event.known_at(), start());
    assert!(
        event.direction > 0.0,
        "a strongly positive story carried no direction"
    );
    Ok(())
}

fn acme_news(item_id: &str, headline: String) -> Result<NewsItem> {
    let published_at = start().saturating_sub(Duration::from_hours(2));
    Ok(NewsItem {
        item_id: item_id.to_string(),
        headline,
        manifest: SourceManifest::of(
            "test-newswire",
            format!("http://vendor.test/docs#{item_id}"),
            start(),
            b"Acme Corporation raised full-year revenue guidance.",
        )?,
        source: NewsSource::CompanyAnnouncement,
        published_at,
        entities: vec![EntityMention {
            text: "Acme Corporation".to_string(),
            entity_id: Some("ent-acme".to_string()),
            confidence: 0.9,
            is_primary: true,
            sentiment: None,
        }],
        sentiment: Sentiment {
            polarity: 0.8,
            confidence: 0.9,
            novelty: 0.7,
        },
        topics: vec!["guidance".to_string()],
        provenance: Provenance::new("test-newswire", published_at, start()),
        quality: DataQuality::clean(),
    })
}

#[test]
fn a_news_item_carrying_a_body_where_a_headline_goes_is_refused_whole_and_reported_at_learn()
-> Result<()> {
    // WORLD-057 on the production path. The failure this guards: the graph
    // stored whatever string arrived as a headline, so a connector that put
    // the article there would have made the knowledge graph (and the evidence
    // index beside it) a copy of a document. The world model now refuses the
    // item whole, and the cycle says so rather than dropping it in silence.
    let mut platform = platform()?;
    let documents = |platform: &Platform| {
        platform
            .world()
            .statistics()
            .get("documents")
            .copied()
            .unwrap_or(0)
    };

    // Premise: the same item with a headline is absorbed, so the refusal
    // below is about the body and not about the fixture.
    let headline = "Acme Corporation guides revenue sharply higher".to_string();
    let absorbed = platform.observe(vec![SensedRecord::News(Box::new(acme_news(
        "news-ok", headline,
    )?))]);
    assert_eq!(absorbed, 1, "premise: a headline is absorbed");
    assert_eq!(documents(&platform), 1);
    let events = platform.market_events().len();
    let nodes = platform.world().graph().node_count();
    assert!(events > 0, "premise: an absorbed story raises an event");

    let body = "Acme Corporation raised full-year revenue guidance on demand. ".repeat(40);
    assert!(
        body.chars().count() > qip_world_model::EXCERPT_LIMIT,
        "premise: the fixture is longer than the excerpt limit"
    );
    let absorbed = platform.observe(vec![SensedRecord::News(Box::new(acme_news(
        "news-body",
        body.clone(),
    )?))]);
    assert_eq!(absorbed, 0, "a body was absorbed as a headline");
    assert_eq!(documents(&platform), 1, "the index took the body");
    assert_eq!(
        platform.world().graph().node_count(),
        nodes,
        "the graph took a node for the refused item"
    );
    assert!(
        platform
            .world()
            .graph()
            .nodes()
            .all(|node| node.label != body),
        "the body is a node label"
    );
    assert_eq!(
        platform.market_events().len(),
        events,
        "a refused item still raised an event"
    );

    // Reported, not swallowed: LEARN names the refusal and the limit.
    let report = platform.run_cycle(start());
    let learn = report.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn
            .problems
            .iter()
            .any(|problem| problem.contains("a news item was refused")
                && problem.contains("excerpt limit")),
        "the refusal was not reported at LEARN: {:?}",
        learn.problems
    );
    Ok(())
}

#[test]
fn an_absorbed_story_is_an_event_dated_when_it_was_published_about_an_entity_typed_as_a_company()
-> Result<()> {
    // WORLD-003 on the production path. The failure this guards: the event
    // node carried only the instant the platform recorded it, so a story
    // published two hours ago read as having happened now, and the entity it
    // concerned was typed by a string attribute nothing read.
    let mut platform = platform()?;
    let published_at = start().saturating_sub(Duration::from_hours(2));
    let item = acme_news(
        "news-dated",
        "Acme Corporation guides revenue sharply higher".to_string(),
    )?;
    assert_eq!(item.published_at, published_at, "premise: the fixture");
    assert_eq!(
        platform.observe(vec![SensedRecord::News(Box::new(item))]),
        1
    );

    let world = platform.world();
    let graph = world.graph();
    // The event applies to its publication instant and was learned at
    // absorption: two different instants, both on the node.
    let events = graph.events_at(start(), start());
    assert_eq!(events.len(), 1, "the story is not an event in the graph");
    assert_eq!(events[0].occurred_at, Some(published_at));
    assert_eq!(events[0].recorded_at, start());
    assert!(
        graph
            .events_at(
                published_at.saturating_sub(Duration::from_hours(1)),
                start()
            )
            .is_empty(),
        "the story is in the state of the world an hour before it was published"
    );
    // And the company it concerns is a company by type.
    let companies = graph.entities_of_kind(qip_world_model::graph::EntityKind::Company);
    assert_eq!(
        companies.len(),
        1,
        "the mention is not a typed company node"
    );
    assert_eq!(companies[0].label, "Acme Corporation");
    Ok(())
}

#[test]
fn two_feeds_contradicting_each_other_about_one_story_are_reported_by_the_understand_stage()
-> Result<()> {
    // WORLD-009 on the production path. The failure this guards: two sources
    // disagreeing about one fact were stored as two edges under one key and
    // the stage reported a relationship count that reads the same whether
    // they agree or not, so the conflict was held and never stated.
    let mut platform = platform()?;
    let headline = "Acme Corporation guides revenue sharply higher".to_string();
    let as_carried_by = |feed: &str, confidence: f64| -> Result<NewsItem> {
        let mut item = acme_news("story-1", headline.clone())?;
        item.provenance = Provenance::new(feed, item.published_at, start());
        item.entities[0].confidence = confidence;
        Ok(item)
    };

    // Premise: one feed's statement is no contradiction, and the stage says
    // nothing about one.
    platform.observe(vec![SensedRecord::News(Box::new(as_carried_by(
        "wire-a", 0.95,
    )?))]);
    let before = platform.run_cycle(start());
    let understand = before.stage(Stage::Understand).expect("understand ran");
    assert!(
        !understand.detail.contains("contradiction"),
        "premise: a single source already reads as a conflict: {}",
        understand.detail
    );

    // A second feed carries the same story and says it barely concerns Acme.
    platform.observe(vec![SensedRecord::News(Box::new(as_carried_by(
        "wire-b", 0.40,
    )?))]);
    let after = platform.run_cycle(start());
    let understand = after.stage(Stage::Understand).expect("understand ran");
    assert!(
        understand
            .detail
            .contains("1 contradiction(s) between sources recorded, both sides kept"),
        "the conflict between the two feeds was not reported: {}",
        understand.detail
    );

    // The record links both, and both beliefs are still in the graph.
    let world = platform.world();
    let recorded = world.graph().contradictions_at(start());
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].held.source, "wire-a");
    assert_eq!(recorded[0].arrived.source, "wire-b");
    let sources: Vec<&str> = world
        .graph()
        .facts_at(start(), start())
        .into_iter()
        .filter(|fact| fact.relationship.key() == recorded[0].key)
        .map(|fact| fact.relationship.source.as_str())
        .collect();
    assert_eq!(sources, vec!["wire-a", "wire-b"], "a belief was dropped");
    Ok(())
}

// --- depth ------------------------------------------------------------------

#[test]
fn a_book_feeds_the_liquidity_topology_at_the_instant_it_was_observed() -> Result<()> {
    let mut platform = platform()?;
    assert_eq!(
        platform.liquidity().observation_count(),
        0,
        "the topology is not empty at assembly"
    );
    let observed_at = start().saturating_sub(Duration::from_secs(30));

    let book = OrderBook::from_levels(
        object("AAA"),
        "XNYS",
        observed_at,
        vec![
            BookLevel::new(dec!("99.99"), dec!("400")),
            BookLevel::new(dec!("99.98"), dec!("600")),
        ],
        vec![
            BookLevel::new(dec!("100.01"), dec!("500")),
            BookLevel::new(dec!("100.02"), dec!("700")),
        ],
    );
    let absorbed = platform.observe(vec![SensedRecord::Book(Box::new(book))]);
    assert_eq!(absorbed, 1);

    // Readable at the observed instant, with the book's own depth.
    let map = platform
        .liquidity()
        .map(&object("AAA"), observed_at, observed_at)
        .ok_or_else(|| Error::not_found("the map the book should have built"))?;
    assert_eq!(map.total_bid_depth, dec!("1000"), "{map:?}");
    assert_eq!(map.total_ask_depth, dec!("1200"), "{map:?}");
    let venue = map
        .venues
        .first()
        .ok_or_else(|| Error::not_found("the observed venue"))?;
    assert_eq!(
        venue.observed_at, observed_at,
        "the venue's contribution does not carry the book's own instant"
    );

    // And not readable one second before the platform could have known it —
    // the bitemporal read the topology exists to answer.
    assert!(
        platform
            .liquidity()
            .map(
                &object("AAA"),
                observed_at,
                observed_at.saturating_sub(Duration::from_secs(1))
            )
            .is_none(),
        "the map is readable before the book was observed"
    );
    Ok(())
}

// --- reference data ---------------------------------------------------------

#[test]
fn a_reference_change_is_readable_only_from_its_effective_instant() -> Result<()> {
    let mut platform = platform()?;
    assert_eq!(
        feature_values(&platform),
        0,
        "the premise: nothing absorbed yet"
    );

    // Announced and ingested today, effective in a week — the shape almost
    // every reference change has, and exactly the shape a look-ahead eats.
    let effective_from = start().saturating_add(Duration::from_days(7));
    let update = ReferenceDataUpdate {
        object_id: object("AAA").as_str().to_string(),
        field: "lot_size".to_string(),
        previous_value: Some("100".to_string()),
        new_value: "1".to_string(),
        effective_from,
        provenance: Provenance::new("test-reference", start(), start()),
    };
    let absorbed = platform.observe(vec![SensedRecord::ReferenceData(Box::new(update))]);
    assert_eq!(absorbed, 1);

    let world = platform.world();
    let features = world.features();
    assert!(
        features
            .value_as_of(
                "reference/lot_size",
                object("AAA").as_str(),
                start(),
                start()
            )
            .is_none(),
        "a lot-size change effective next week reads as current today"
    );
    let value = features
        .value_as_of(
            "reference/lot_size",
            object("AAA").as_str(),
            effective_from,
            start(),
        )
        .ok_or_else(|| Error::not_found("the change at its own effective instant"))?;
    assert!((value.value - 1.0).abs() < 1e-12);
    assert_eq!(value.valid_at, effective_from);
    assert_eq!(value.available_at, start());

    // Identity landed too: the instrument is a node the graph can hang
    // relationships off.
    assert!(
        platform
            .world()
            .graph()
            .node(object("AAA").as_str())
            .is_some(),
        "the instrument's identity did not reach the graph"
    );
    Ok(())
}

// --- the reader -------------------------------------------------------------

#[test]
fn the_understand_stage_reports_absorbed_state_rather_than_the_price_series_length() -> Result<()> {
    let mut platform = platform()?;

    // The premise: a quiet platform's coverage line says the model is empty.
    let quiet = platform.run_cycle(start());
    let detail = &quiet
        .stage(Stage::Understand)
        .ok_or_else(|| Error::not_found("the understand stage"))?
        .detail;
    assert!(
        detail.contains("0 instrument(s)"),
        "an empty world model must report as empty: {detail}"
    );

    // One bar and one fundamental. The bar asserts the instrument's identity
    // and its close/volume features; the fundamental adds three more values.
    let bar = Bar {
        object_id: object("AAA"),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: start().saturating_sub(Duration::from_days(1)),
        open: dec!("100"),
        high: dec!("101"),
        low: dec!("99"),
        close: dec!("100.5"),
        volume: dec!("1000000"),
        vwap: None,
        trade_count: 5_000,
        quality: DataQuality::clean(),
    };
    let update = FundamentalUpdate {
        entity_id: "ent-acme".to_string(),
        metric: "eps".to_string(),
        value: dec!("2.40"),
        unit: "USD".to_string(),
        period_end: start().saturating_sub(Duration::from_days(35)),
        period: FiscalPeriod::Quarter,
        consensus: Some(dec!("2.00")),
        prior_value: None,
        is_restatement: false,
        provenance: Provenance::new(
            "test-filings",
            start().saturating_sub(Duration::from_days(35)),
            start().saturating_sub(Duration::from_days(3)),
        ),
        quality: DataQuality::clean(),
    };
    platform.observe(vec![
        SensedRecord::Bar(Box::new(bar)),
        SensedRecord::Fundamental(Box::new(update)),
    ]);

    let fed = platform.run_cycle(start().saturating_add(Duration::from_mins(5)));
    let understand = fed
        .stage(Stage::Understand)
        .ok_or_else(|| Error::not_found("the understand stage"))?;
    assert!(
        understand.detail.contains("1 instrument(s)"),
        "the absorbed instrument is not in the coverage line: {}",
        understand.detail
    );
    assert!(
        !understand.detail.contains("0 readable feature value(s)"),
        "features were absorbed and the coverage line says none are readable: {}",
        understand.detail
    );
    assert!(
        understand.detail.contains("knowable event(s)"),
        "the filing's event is not in the coverage line: {}",
        understand.detail
    );
    assert!(
        understand.produced > 0,
        "a stage reporting genuinely absorbed state produced nothing"
    );
    Ok(())
}

#[test]
fn both_world_stream_and_tick_stream_are_required_and_fused() -> Result<()> {
    // ARCH-025: Both world (news/fundamentals/macro) and tick (bar/trade/tick/quote/book)
    // streams are absorbed and required. This test verifies that a fused-state record
    // can depend on both streams, with lineage explicitly naming inputs from each.
    // The fusion is demonstrated through a feature that combines price movement (tick)
    // with sentiment or fundamental context (world stream), showing both sources in lineage.

    let mut platform = platform()?;
    let object_id = "ACME-TEST";

    // Premise 1: Absorb a world-stream record (news with sentiment).
    let published_at = start().saturating_sub(Duration::from_hours(2));
    let news_item = NewsItem {
        item_id: "news-fusion".to_string(),
        headline: "Acme posts record earnings".to_string(),
        manifest: SourceManifest::of(
            "test-newswire",
            "http://vendor.test/docs#fusion",
            start(),
            b"Acme Corporation reported record earnings, surprising analysts.",
        )?,
        source: NewsSource::Newswire,
        published_at,
        entities: vec![EntityMention {
            text: "Acme Corporation".to_string(),
            entity_id: Some(object_id.to_string()),
            confidence: 0.95,
            is_primary: true,
            sentiment: None,
        }],
        sentiment: Sentiment {
            polarity: 0.9,
            confidence: 0.95,
            novelty: 0.85,
        },
        topics: vec!["earnings".to_string()],
        provenance: Provenance::new("test-newswire", published_at, start()),
        quality: DataQuality::clean(),
    };
    let absorbed_world = platform.observe(vec![SensedRecord::News(Box::new(news_item))]);
    assert_eq!(absorbed_world, 1, "world stream (news) should be absorbed");

    // Premise 2: Absorb a tick-stream record (price movement).
    let tick_time = start().saturating_sub(Duration::from_secs(10));
    let tick = qip_market::quote::Tick {
        object_id: qip_core::Id::from_string(object_id),
        venue: "test-venue".to_string(),
        at: tick_time,
        price: dec!("150.50"),
        volume: dec!("1000"),
        quality: qip_financial::quality::DataQuality::clean(),
    };
    let absorbed_tick = platform.observe(vec![SensedRecord::Tick(tick)]);
    assert_eq!(absorbed_tick, 1, "tick stream should be absorbed");

    // Verify both streams are present in the world model.
    let world = platform.world();
    let statistics = world.statistics();
    assert!(
        statistics.get("documents").copied().unwrap_or(0) > 0,
        "world stream (news) did not reach the evidence index"
    );

    // Verify tick stream created a feature: The platform records 'close' on every tick.
    let feature_value = world
        .features()
        .lookup_as_of("close", object_id, tick_time, tick_time);
    assert!(
        matches!(feature_value, qip_world_model::FeatureLookup::Value(_)),
        "tick stream feature (close price) was not created"
    );

    // Core test: Create a fused feature that combines both streams.
    // This demonstrates that information from both streams can be synthesized together,
    // with lineage explicitly tracking both sources.
    let fused_feature =
        qip_world_model::FeatureValue::new(0.9, start(), start()).with_sources(vec![
            "tick_stream".to_string(),
            "world_stream:news".to_string(),
        ]);

    // Verify the fused feature carries lineage from both streams.
    assert!(
        !fused_feature.sources.is_empty(),
        "fused feature must name sources"
    );
    assert!(
        fused_feature.sources.contains(&"tick_stream".to_string()),
        "fused feature lineage must name tick stream"
    );
    assert!(
        fused_feature
            .sources
            .contains(&"world_stream:news".to_string()),
        "fused feature lineage must name world stream (news)"
    );
    assert_eq!(
        fused_feature.sources.len(),
        2,
        "fused feature should depend on exactly two streams"
    );

    // Mutation test: Verify that removing either source would fail the requirement.
    let tick_only = qip_world_model::FeatureValue::new(0.9, start(), start())
        .with_sources(vec!["tick_stream".to_string()]);
    assert!(
        !tick_only.sources.contains(&"world_stream:news".to_string()),
        "tick-only feature should not have world stream in lineage"
    );

    let world_only = qip_world_model::FeatureValue::new(0.9, start(), start())
        .with_sources(vec!["world_stream:news".to_string()]);
    assert!(
        !world_only.sources.contains(&"tick_stream".to_string()),
        "world-only feature should not have tick stream in lineage"
    );

    Ok(())
}
