//! The NOW coordinator's three §11.2 contracts, produced by the cycle a
//! shipped binary runs (R12-WORLD-9016).
//!
//! `GlobalState` and `RegionalState` had constructors on the NOW Brain and no
//! test that read them off a platform that had run a cycle, so a brain whose
//! state was never promoted, or whose filters returned every variable, read
//! the same as a working one. The fixtures are `now_lattice.rs`'s, copied
//! rather than shared so this file does not move with that suite's edits.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_contracts::world_model::WorldModelState;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_events::{EventBody, Topic};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::intelligence::{
    EntityMention, FiscalPeriod, FundamentalUpdate, MacroObservation, NewsItem, NewsSource,
    Sentiment,
};
use qip_financial::manifest::SourceManifest;
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::now_stage::NowReview;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_streaming::envelope::StreamEnvelope;
use qip_world_model::graph::NodeKind;
use qip_world_model::now::{
    BeliefDelta, Domain, NowVariable, PartitionKey, Scope, ScopeKind, VariableKey,
};

const SYMBOLS: [&str; 2] = ["AAA", "BBB"];

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn day(n: i64) -> Timestamp {
    start().saturating_add(Duration::from_days(n))
}

fn object(symbol: &str) -> qip_core::ObjectId {
    qip_core::ObjectId::from_string(format!("obj-{symbol}"))
}

fn platform() -> Platform {
    let mut universe = Universe::new();
    for symbol in SYMBOLS {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    InstrumentType::CommonStock,
                    qip_financial::costs::LiquidityProfile::listed(
                        Decimal::from_int(5_000_000),
                        3.0,
                    ),
                )
                .venue("XNYS")
                .geography("US")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .unwrap(),
            )
            .unwrap();
    }
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::new("now-lattice-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        ),
    )
    .unwrap()
}

/// A daily bar for `symbol` closing at `closes_at`.
fn bar(symbol: &str, closes_at: Timestamp, close: f64) -> SensedRecord {
    let price = |v: f64| Decimal::from_f64(v).unwrap();
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: closes_at.saturating_sub(Duration::from_days(1)),
        open: price(close),
        high: price(close * 1.002),
        low: price(close * 0.998),
        close: price(close),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: None,
        quality: DataQuality::default(),
    }))
}

/// Thirty calm days for each symbol, the last closing at `start()`.
fn calm_history() -> Vec<SensedRecord> {
    let mut out = Vec::new();
    for symbol in SYMBOLS {
        for i in 0..30_i64 {
            let close = if i % 2 == 0 { 100.2 } else { 99.8 };
            out.push(bar(symbol, day(i - 29), close));
        }
    }
    out
}

fn reviews<T: EventBody>(platform: &Platform, prefix: &str) -> Vec<T> {
    platform
        .event_log()
        .by_topic(Topic::WorldModelUpdated)
        .into_iter()
        .filter(|e| {
            e.idempotency_key
                .as_deref()
                .is_some_and(|k| k.starts_with(prefix))
        })
        // `Platform::journal_record` appends a sealed `StreamEnvelope`'s
        // frame, so the log record's payload is the envelope wire and the
        // review is inside it. Decoding the frame as the review directly
        // read the wire's fields as the review's and failed on `cycle`,
        // which is how all seven tests here failed at their first read.
        .map(|e| {
            StreamEnvelope::from_frame(e)
                .and_then(|envelope| envelope.decode::<T>())
                .unwrap()
                .body
        })
        .collect()
}

fn last<T: EventBody>(platform: &Platform, prefix: &str) -> T {
    reviews::<T>(platform, prefix)
        .pop()
        .unwrap_or_else(|| panic!("no `{prefix}` review in the log"))
}

/// Fed a calm history then a 30% break in one instrument, the coordinator
/// produces a `GlobalState` holding only global-scope variables, a
/// `RegionalState` for the instruments' region holding only that region's,
/// and a `SurpriseEvent` for the broken instrument — whose residual is past
/// the configured bound — and none for the quiet one.
///
/// Mutations: flip the `ScopeKind::Global` filter in `NowBrain::global_state`
/// (the scope assertion fires); flip the region filter in `regional_state`
/// (the regional premise fires); raise the surprise test in the filter to
/// `>= surprise_sigmas * 1e6` (no surprise is found).
#[test]
fn the_cycle_produces_global_and_regional_state_and_a_surprise_only_where_a_residual_breaks_its_bound()
 {
    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    platform.observe(vec![bar("AAA", day(1), 130.0), bar("BBB", day(1), 100.1)]);
    platform.run_cycle(day(1));
    let brain = platform.now_brain().brain();

    let global = brain.global_state(day(1));
    assert!(!global.variables.is_empty(), "no global state was promoted");
    assert!(
        global
            .variables
            .iter()
            .all(|v| v.key.scope.kind == ScopeKind::Global),
        "GlobalState carried a non-global variable: {:?}",
        global.variables.iter().map(|v| &v.key).collect::<Vec<_>>()
    );

    let regional = brain.regional_state("US", day(1));
    assert!(
        regional
            .variables
            .iter()
            .any(|v| v.key.scope.kind == ScopeKind::Instrument && v.key.scope.id == "obj-AAA"),
        "the US instrument is missing from the US RegionalState"
    );
    assert!(
        regional
            .variables
            .iter()
            .all(|v| v.partition.region == "US"),
        "RegionalState carried another region's variable"
    );
    assert!(
        brain.regional_state("nowhere", day(1)).variables.is_empty(),
        "a region nothing is observed in has state"
    );

    let review: NowReview = last(&platform, "now:");
    let instrument = |id: &str| {
        review
            .surprises
            .iter()
            .filter(|s| s.key.scope.kind == ScopeKind::Instrument && s.key.scope.id == id)
            .count()
    };
    assert!(
        instrument("obj-AAA") > 0,
        "the 30% break raised no surprise"
    );
    assert_eq!(
        instrument("obj-BBB"),
        0,
        "the quiet instrument was a surprise"
    );
    for surprise in &review.surprises {
        assert!(
            surprise.sigmas.abs() >= qip_world_model::now::DEFAULT_SURPRISE_SIGMAS,
            "a surprise inside its bound: {surprise:?}"
        );
    }
}

/// RM-P2-15 at the cycle seam: a cycle that absorbs no new macro release
/// does not tighten the NOW Brain's belief about the surprise.
///
/// This failed until the fix that landed with it. The brain itself never
/// shrinks a variable's variance on a step that brings it nothing
/// (`qip-world-model`'s
/// `the_state_covariance_never_shrinks_when_an_observation_is_withheld`). But
/// `now_stage::own_observations` stamped every macro surprise `valid_at: now`
/// on every pass, so one release, readable for the feature's 120 days,
/// arrived as a new reading each cycle and the brain's idempotency filter
/// could not tell. Measured on this fixture before the fix, the variance fell
/// from 0.209 to 0.104, 0.069, 0.050, 0.040 and 0.033 over the five cycles
/// and the confidence rose from 8,269 bp to 9,679 bp: confidence
/// manufactured from one data point read again and again.
///
/// Mutation: in `now_stage::FirstReads::stamp`, stamp every world reading
/// with the pass rather than its first read (fires at +1h).
#[test]
fn a_cycle_that_absorbs_no_new_macro_release_does_not_tighten_the_now_belief_about_it() {
    let mut platform = platform();
    let reference = start().saturating_sub(Duration::from_days(20));
    let release = SensedRecord::Macro(Box::new(MacroObservation {
        series_id: "US.CPI.YOY".to_string(),
        region: "US".to_string(),
        value: 3.4,
        unit: "percent".to_string(),
        reference_date: reference,
        // A consensus, so the world model records a surprise.
        consensus: Some(3.1),
        previous: None,
        is_revision: false,
        provenance: Provenance::new(
            "wire",
            reference,
            start().saturating_sub(Duration::from_days(1)),
        ),
        quality: DataQuality::clean(),
        evidence_unretrievable: false,
    }));
    assert_eq!(
        platform.observe(vec![release]),
        1,
        "premise: the release was absorbed"
    );
    let key = VariableKey::new(Scope::global(), "macro_surprise").unwrap();
    let partition = PartitionKey::new(Domain::Economy, "global").unwrap();
    platform.run_cycle(start());
    let first = platform
        .now_brain()
        .brain()
        .store()
        .get(&partition, &key)
        .cloned()
        .expect("premise: the first cycle promoted the macro surprise");

    for hour in 1..=5 {
        platform.run_cycle(start().saturating_add(Duration::from_hours(hour)));
        let later = platform
            .now_brain()
            .brain()
            .store()
            .get(&partition, &key)
            .cloned()
            .expect("premise: the macro surprise is still promoted");
        // Premise: nothing new arrived. The belief rests on exactly the
        // manifest it rested on after the release.
        assert_eq!(
            later.manifests, first.manifests,
            "premise: a manifest other than the one release reached the belief"
        );
        assert!(
            later.uncertainty >= first.uncertainty,
            "the cycle at +{hour}h tightened the macro_surprise belief from {} to {} (confidence \
             {} bp to {} bp) on no new release, still resting on {:?}",
            first.uncertainty,
            later.uncertainty,
            first.confidence_bp,
            later.confidence_bp,
            later.manifests
        );
    }
}

fn hours(n: i64) -> Timestamp {
    start().saturating_add(Duration::from_hours(n))
}

/// A release of `series` describing `period`, ingested at `ingested`, with a
/// consensus so the world model records a surprise.
fn release(
    series: &str,
    period: Timestamp,
    ingested: Timestamp,
    value: f64,
    consensus: f64,
) -> MacroObservation {
    MacroObservation {
        series_id: series.to_string(),
        region: "US".to_string(),
        value,
        unit: "percent".to_string(),
        reference_date: period,
        consensus: Some(consensus),
        previous: None,
        is_revision: false,
        provenance: Provenance::new("wire", period, ingested),
        quality: DataQuality::clean(),
        evidence_unretrievable: false,
    }
}

fn observe_release(platform: &mut Platform, observation: MacroObservation) {
    let series = observation.series_id.clone();
    assert_eq!(
        platform.observe(vec![SensedRecord::Macro(Box::new(observation))]),
        1,
        "premise: the world model absorbed the {series} release"
    );
}

fn macro_key() -> VariableKey {
    VariableKey::new(Scope::global(), "macro_surprise").unwrap()
}

/// The global macro surprise as the NOW Brain holds it.
fn macro_belief(platform: &Platform) -> Option<NowVariable> {
    platform
        .now_brain()
        .brain()
        .store()
        .get(
            &PartitionKey::new(Domain::Economy, "global").unwrap(),
            &macro_key(),
        )
        .cloned()
}

/// Every belief delta the journaled NOW reviews hold for the global macro
/// surprise, with the instant of the pass that wrote it.
fn macro_deltas(platform: &Platform) -> Vec<(Timestamp, BeliefDelta)> {
    let key = macro_key();
    reviews::<NowReview>(platform, "now:")
        .into_iter()
        .flat_map(|review| {
            let at = review.at;
            review
                .deltas
                .into_iter()
                .filter(|delta| delta.key == key)
                .map(|delta| (at, delta))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// How many passes absorbed a release of `series`: macro-surprise deltas
/// whose manifests name it as their source.
fn absorptions(platform: &Platform, series: &str) -> usize {
    macro_deltas(platform)
        .iter()
        .filter(|(_, delta)| delta.manifests.iter().any(|m| m.source == series))
        .count()
}

/// The NOW review the cycle numbered `cycle` journaled.
fn review_of(platform: &Platform, cycle: u64) -> NowReview {
    reviews::<NowReview>(platform, "now:")
        .into_iter()
        .find(|review| review.cycle == cycle)
        .unwrap_or_else(|| panic!("the cycle asked for journaled no NOW review"))
}

/// Every macro series feeds the one global `macro_surprise` belief, so a
/// release of one series must never be filtered out behind another's.
/// Release A, CPI for twenty days back, is absorbed first; release B, GDP for
/// ninety days back, an earlier period than A's, reaches the platform after
/// A was absorbed. B is new evidence and is absorbed exactly once, and
/// neither release is absorbed again on the cycles after.
///
/// The obvious fix for the re-read is to stamp each reading with the period
/// it describes, and this is where it breaks: B's period falls behind the
/// mark A set, and B is filtered as already absorbed — a new release lost,
/// with nothing to say so.
///
/// Mutation: in `now_stage::FirstReads::stamp`, stamp a newly read value
/// with its `valid_at` rather than the pass (B is never absorbed).
#[test]
fn a_release_of_another_series_naming_an_earlier_period_is_absorbed_once_after_one_already_absorbed()
 {
    let mut platform = platform();
    let a_period = start().saturating_sub(Duration::from_days(20));
    let b_period = start().saturating_sub(Duration::from_days(90));
    assert!(
        b_period < a_period,
        "premise: B describes an earlier period than A"
    );
    observe_release(
        &mut platform,
        release(
            "US.CPI.YOY",
            a_period,
            start().saturating_sub(Duration::from_days(1)),
            3.4,
            3.1,
        ),
    );
    platform.run_cycle(start());
    assert_eq!(
        absorptions(&platform, "US.CPI.YOY"),
        1,
        "premise: the first cycle absorbed A"
    );

    observe_release(
        &mut platform,
        release(
            "US.GDP.QOQ",
            b_period,
            start().saturating_add(Duration::from_mins(30)),
            2.0,
            1.2,
        ),
    );
    platform.run_cycle(hours(1));
    let belief = macro_belief(&platform).expect("the macro surprise is promoted");
    assert!(
        belief.manifests.iter().any(|m| m.source == "US.GDP.QOQ"),
        "B, another series' release for an earlier period, was not absorbed: the belief still \
         rests on {:?}",
        belief.manifests
    );
    for hour in 2..=4 {
        platform.run_cycle(hours(hour));
    }
    assert_eq!(
        absorptions(&platform, "US.GDP.QOQ"),
        1,
        "B was absorbed other than once: {:?}",
        macro_deltas(&platform)
    );
    assert_eq!(
        absorptions(&platform, "US.CPI.YOY"),
        1,
        "A was absorbed again after B arrived: {:?}",
        macro_deltas(&platform)
    );
}

/// A release ingested before one already absorbed, but delivered after it,
/// is still absorbed exactly once.
///
/// Connectors stamp a record's ingestion instant from the caller's horizon
/// and the streaming path delivers out of order, so this is where the second
/// obvious fix for the re-read breaks: stamp each reading with when the
/// platform ingested it. B here was ingested two days back, a day before A,
/// and reaches the platform only after A has been absorbed; stamped with its
/// ingestion instant it falls behind A's and is lost. B's period is later
/// than A's, so only its ingestion instant is out of order.
///
/// Mutation: in `now_stage::FirstReads::stamp`, stamp a newly read value
/// with its `available_at` rather than the pass (B is never absorbed).
#[test]
fn a_release_ingested_before_one_already_absorbed_but_delivered_after_it_is_absorbed_once() {
    let mut platform = platform();
    let a_ingested = start().saturating_sub(Duration::from_days(1));
    let b_ingested = start().saturating_sub(Duration::from_days(2));
    assert!(b_ingested < a_ingested, "premise: B was ingested before A");
    observe_release(
        &mut platform,
        release(
            "US.CPI.YOY",
            start().saturating_sub(Duration::from_days(20)),
            a_ingested,
            3.4,
            3.1,
        ),
    );
    platform.run_cycle(start());
    assert_eq!(
        absorptions(&platform, "US.CPI.YOY"),
        1,
        "premise: the first cycle absorbed A"
    );

    observe_release(
        &mut platform,
        release(
            "US.PPI.YOY",
            start().saturating_sub(Duration::from_days(15)),
            b_ingested,
            2.9,
            2.5,
        ),
    );
    for hour in 1..=4 {
        platform.run_cycle(hours(hour));
    }
    assert_eq!(
        absorptions(&platform, "US.PPI.YOY"),
        1,
        "B, ingested before A and delivered after it, was absorbed other than once: {:?}",
        macro_deltas(&platform)
    );
    assert_eq!(
        absorptions(&platform, "US.CPI.YOY"),
        1,
        "A was absorbed again after B arrived: {:?}",
        macro_deltas(&platform)
    );
}

/// A release that arrives between two cycles run at the same instant is held
/// to the next later cycle and absorbed there exactly once, rather than
/// stamped with the instant the belief already rests on and lost.
///
/// `Platform::run_cycle` admits a cycle at the instant of the last one. A
/// release first read at that second cycle and stamped with its instant ties
/// the mark A set at the first, is filtered as already absorbed, and carries
/// that stamp into every cycle after: never absorbed, and nothing says so.
/// The hold-back is named in the review instead.
///
/// Mutation: in `now_stage::FirstReads::stamp`, drop the hold-back and stamp
/// a newly read value at a pass no later than the last stamp (B is never
/// absorbed).
#[test]
fn a_release_first_read_at_the_instant_of_the_last_cycle_is_held_to_the_next_cycle_and_absorbed_once()
 {
    let mut platform = platform();
    observe_release(
        &mut platform,
        release(
            "US.CPI.YOY",
            start().saturating_sub(Duration::from_days(20)),
            start().saturating_sub(Duration::from_days(1)),
            3.4,
            3.1,
        ),
    );
    platform.run_cycle(start());
    assert_eq!(
        absorptions(&platform, "US.CPI.YOY"),
        1,
        "premise: the first cycle absorbed A"
    );

    observe_release(
        &mut platform,
        release(
            "US.PPI.YOY",
            start().saturating_sub(Duration::from_days(15)),
            start().saturating_sub(Duration::from_hours(1)),
            2.9,
            2.5,
        ),
    );
    let tied = platform.run_cycle(start());
    assert!(
        tied.stage(Stage::Understand).is_some_and(|stage| stage.ran),
        "premise: the platform ran a second cycle at the instant of the first"
    );
    let held = review_of(&platform, tied.cycle);
    assert!(
        held.refused.iter().any(|(label, why)| {
            label == "macro_surprise/US.PPI.YOY" && why.starts_with("first read at")
        }),
        "the cycle at the instant of the last did not name B as held back: {:?}",
        held.refused
    );
    assert_eq!(
        absorptions(&platform, "US.PPI.YOY"),
        0,
        "B was absorbed at the instant the belief already rested on"
    );

    platform.run_cycle(hours(1));
    platform.run_cycle(hours(2));
    assert_eq!(
        absorptions(&platform, "US.PPI.YOY"),
        1,
        "B, held back at the tie, was absorbed other than once after it: {:?}",
        macro_deltas(&platform)
    );
}

/// A release delivered again is the same release; a revision of it is not.
///
/// A value is known by the period it describes and by the number itself,
/// not by the instant it was ingested. A re-poll or an at-least-once
/// redelivery re-stamps the stored value's `available_at` without bringing
/// anything new, and absorbing it would be the re-read through another
/// door; a revision changes the number for the same period and is new
/// evidence, absorbed exactly once.
///
/// Mutations in `now_stage::FirstReads::stamp`: compare `available_at` as
/// well (the redelivery tightens the belief); compare the period alone, not
/// the number (the revision is never absorbed).
#[test]
fn a_release_delivered_again_is_not_absorbed_again_and_a_revision_of_it_is_absorbed_once() {
    let mut platform = platform();
    let period = start().saturating_sub(Duration::from_days(20));
    let original = release(
        "US.CPI.YOY",
        period,
        start().saturating_sub(Duration::from_days(1)),
        3.4,
        3.1,
    );
    observe_release(&mut platform, original.clone());
    platform.run_cycle(start());
    let first = macro_belief(&platform).expect("premise: the first cycle promoted the surprise");

    let redelivered_at = start().saturating_add(Duration::from_mins(10));
    let mut again = original.clone();
    again.provenance = Provenance::new("wire", period, redelivered_at);
    observe_release(&mut platform, again);
    let stored = platform
        .world()
        .features()
        .current("macro_surprise", "US.CPI.YOY", hours(1))
        .map(|value| value.available_at);
    assert_eq!(
        stored,
        Some(redelivered_at),
        "premise: the redelivery re-stamped the value the world model holds"
    );
    platform.run_cycle(hours(1));
    let after = macro_belief(&platform).expect("the macro surprise is still promoted");
    assert!(
        after.uncertainty >= first.uncertainty,
        "the redelivered release tightened the belief from {} to {} (confidence {} bp to {} bp)",
        first.uncertainty,
        after.uncertainty,
        first.confidence_bp,
        after.confidence_bp
    );

    let mut revision = original;
    revision.value = 3.6;
    revision.is_revision = true;
    revision.provenance = Provenance::new(
        "wire",
        period,
        hours(1).saturating_add(Duration::from_mins(30)),
    );
    observe_release(&mut platform, revision);
    platform.run_cycle(hours(2));
    platform.run_cycle(hours(3));
    let deltas = macro_deltas(&platform);
    assert_eq!(
        deltas.iter().filter(|(at, _)| *at == hours(2)).count(),
        1,
        "the revision was not absorbed at the cycle after it arrived: {deltas:?}"
    );
    assert_eq!(
        deltas.len(),
        2,
        "the release and its revision were not each absorbed exactly once: {deltas:?}"
    );
}

/// A cycle that absorbs no new fundamental does not tighten the NOW Brain's
/// belief about the company either. Company features went through the same
/// stamping as macro surprises, so a quarterly filing, readable for its
/// feature's 200 days, and the news sentiment beside it were new readings on
/// every cycle.
///
/// Mutation: as for the macro surprise, stamp every world reading with the
/// pass in `now_stage::FirstReads::stamp` (fires at +1h).
#[test]
fn a_cycle_that_absorbs_no_new_fundamental_does_not_tighten_the_now_belief_about_the_company() {
    let mut platform = platform();
    // A company is an entity in the knowledge graph, and news is how one
    // gets there.
    let published_at = start().saturating_sub(Duration::from_hours(2));
    let news = NewsItem {
        item_id: "news-acme".to_string(),
        headline: "Acme posts record earnings".to_string(),
        manifest: SourceManifest::of(
            "test-newswire",
            "http://vendor.test/docs#acme",
            start(),
            b"Acme Corporation reported record earnings.",
        )
        .unwrap(),
        source: NewsSource::Newswire,
        published_at,
        entities: vec![EntityMention {
            text: "Acme Corporation".to_string(),
            entity_id: None,
            confidence: 0.95,
            is_primary: true,
            sentiment: None,
            kind: Default::default(),
            identifiers: Default::default(),
        }],
        sentiment: Sentiment {
            polarity: 0.6,
            confidence: 0.9,
            novelty: 0.8,
        },
        topics: vec!["earnings".to_string()],
        provenance: Provenance::new("test-newswire", published_at, published_at),
        quality: DataQuality::clean(),
        evidence_unretrievable: false,
    };
    assert_eq!(
        platform.observe(vec![SensedRecord::News(Box::new(news))]),
        1,
        "premise: the news item was absorbed"
    );
    let entity = {
        let world = platform.world();
        let entities = world.graph().nodes_of_kind(NodeKind::Entity);
        assert_eq!(entities.len(), 1, "premise: the news item named one entity");
        entities[0].id.clone()
    };
    let period_end = start().saturating_sub(Duration::from_days(35));
    let filing = FundamentalUpdate {
        entity_id: entity.clone(),
        metric: "eps".to_string(),
        value: dec!("2.40"),
        unit: "USD".to_string(),
        period_end,
        period: FiscalPeriod::Quarter,
        consensus: Some(dec!("2.00")),
        prior_value: None,
        is_restatement: false,
        provenance: Provenance::new(
            "test-filings",
            period_end,
            start().saturating_sub(Duration::from_days(3)),
        ),
        quality: DataQuality::clean(),
        evidence_unretrievable: false,
    };
    assert_eq!(
        platform.observe(vec![SensedRecord::Fundamental(Box::new(filing))]),
        1,
        "premise: the filing was absorbed"
    );

    let company = |platform: &Platform| {
        platform
            .now_brain()
            .brain()
            .store()
            .partition(&PartitionKey::new(Domain::Companies, "global").unwrap())
            .cloned()
            .unwrap_or_default()
    };
    platform.run_cycle(start());
    let first = company(&platform);
    let eps = VariableKey::new(
        Scope::new(ScopeKind::Company, entity.as_str()).unwrap(),
        "fundamental.eps",
    )
    .unwrap();
    assert!(
        first.contains_key(&eps),
        "premise: the first cycle promoted the filing for the company; it promoted {:?}",
        first.keys().collect::<Vec<_>>()
    );

    for hour in 1..=5 {
        platform.run_cycle(hours(hour));
        let later = company(&platform);
        assert_eq!(
            later.keys().collect::<Vec<_>>(),
            first.keys().collect::<Vec<_>>(),
            "premise: the company's variables are the ones the first cycle promoted"
        );
        for (key, before) in &first {
            let after = &later[key];
            assert_eq!(
                after.manifests, before.manifests,
                "premise: a manifest other than the filing's or the news item's reached {key}"
            );
            assert!(
                after.uncertainty >= before.uncertainty,
                "the cycle at +{hour}h tightened {key} from {} to {} (confidence {} bp to {} bp) \
                 on no new filing",
                before.uncertainty,
                after.uncertainty,
                before.confidence_bp,
                after.confidence_bp
            );
        }
    }
}

/// R12-CONTRACT-7076: a cycle whose estimators disagree about a variable
/// beyond the configured bound journals that dispute as a `ModelDisagreement`
/// in the cycle's `NowReview`, open for arbitration. The contract had
/// round-trip tests and the estimator produced it, but nothing asserted it
/// survived into the record a second brain reads off the log, so a stage that
/// dropped it between `NowBrain::step` and the journal read the same as one
/// that kept it.
///
/// The 30% break in AAA sits far from the calm regional and global beliefs
/// the graph estimator is pulled toward, while the Kalman filter follows the
/// reading, so their spread on AAA's return passes half the variable's scale.
///
/// Mutation: in `NowStage::run`, journal `disagreements: Vec::new()` instead
/// of `step.disagreements` — the dispute assertion fires.
#[test]
fn estimators_that_disagree_past_their_bound_are_journaled_as_an_open_model_disagreement() {
    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    platform.observe(vec![bar("AAA", day(1), 130.0), bar("BBB", day(1), 100.1)]);
    platform.run_cycle(day(1));

    let review: NowReview = last(&platform, "now:");
    assert!(
        review.ensemble.len() > 1,
        "premise: more than one estimator ran, {:?}",
        review.ensemble
    );
    let disputed = format!(
        "the latent value of {}",
        VariableKey::new(
            Scope::new(ScopeKind::Instrument, "obj-AAA").unwrap(),
            "return"
        )
        .unwrap()
    );
    let dispute = review
        .disagreements
        .iter()
        .find(|d| d.disputed_propositions.contains(&disputed))
        .unwrap_or_else(|| {
            panic!(
                "no journaled disagreement on {disputed:?}; journaled: {:?}",
                review
                    .disagreements
                    .iter()
                    .map(|d| &d.disputed_propositions)
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        dispute.status,
        qip_contracts::world_model::ArbitrationStatus::Open
    );
    assert!(
        dispute.views.len() >= 2,
        "a dispute names fewer than two views: {:?}",
        dispute.views
    );
    for view in &dispute.views {
        assert!(
            review.ensemble.contains(&view.id),
            "a view {:?} names no estimator that ran",
            view.id
        );
    }
}

/// The NOW stage had `NowBrain::world_model_state` and no caller: no pass
/// ever produced the §27 `WorldModelState` a second brain is meant to read,
/// so the contract existed and nothing crossed it. A pass that moves beliefs
/// now journals the state of each partition it moved, re-admissible through
/// the contract, stamped with the pass's own clock and naming the partition
/// it covers; a pass that moves nothing journals none.
///
/// Mutation: skip the `world_model_state` call in `NowStage::run` — the
/// moved partition has no state in the review and the first assertion fails.
#[test]
fn a_pass_that_moves_a_belief_journals_the_world_model_state_of_the_partition_it_moved() {
    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    platform.observe(vec![bar("AAA", day(1), 130.0), bar("BBB", day(1), 100.1)]);
    platform.run_cycle(day(1));

    let review: NowReview = last(&platform, "now:");
    assert!(
        !review.deltas.is_empty(),
        "premise: the pass moved at least one belief"
    );
    let moved: std::collections::BTreeSet<String> = review
        .deltas
        .iter()
        .map(|d| d.fields().partition.to_string())
        .collect();
    assert!(
        !review.world_states.is_empty(),
        "a moved partition left no WorldModelState in the review: {:?}",
        review.refused
    );
    let covered: std::collections::BTreeSet<String> = review
        .world_states
        .iter()
        .map(|s| s.scope.clone())
        .collect();
    assert_eq!(covered, moved, "one state per moved partition, no more");
    for fields in &review.world_states {
        let state = WorldModelState::new(fields.clone()).unwrap();
        assert_eq!(
            state.fields().updated_at,
            day(1),
            "a state not stamped with the pass that moved it"
        );
    }
}

/// Policy had a domain and no feed, and a central bank's published rate was
/// read into the economy's one `macro_surprise` belief beside inflation and
/// growth prints, so a rate decision and a CPI print tightened the same
/// number. A policy-rate series' surprise now moves `policy_rate_surprise` in
/// the policy partition, and the economy partition holds none of it.
///
/// Mutation: drop the `POLICY_RATE_SERIES_PREFIX` test in `world_observations`
/// — the surprise lands in the economy partition and the first assertion
/// fails.
#[test]
fn a_policy_rate_surprise_moves_the_policy_belief_and_not_the_economys() {
    let mut platform = platform();
    let reference = start().saturating_sub(Duration::from_days(20));
    let ingested = start().saturating_sub(Duration::from_days(1));
    observe_release(
        &mut platform,
        release("US.CPI.YOY", reference, ingested, 3.4, 3.1),
    );
    observe_release(
        &mut platform,
        release("POLICY_RATE.US.EFFR", reference, ingested, 5.5, 5.25),
    );
    platform.run_cycle(start());

    let store = platform.now_brain().brain().store();
    let policy_key = VariableKey::new(Scope::global(), "policy_rate_surprise").unwrap();
    let policy = store
        .get(
            &PartitionKey::new(Domain::Policy, "global").unwrap(),
            &policy_key,
        )
        .cloned()
        .expect("the policy-rate surprise reached no policy belief");
    assert!(
        policy
            .manifests
            .iter()
            .all(|m| m.source == "POLICY_RATE.US.EFFR"),
        "the policy belief rests on another series: {:?}",
        policy.manifests
    );
    let economy = macro_belief(&platform).expect("premise: the CPI surprise reached the economy");
    assert!(
        economy.manifests.iter().all(|m| m.source == "US.CPI.YOY"),
        "the policy rate leaked into the economy belief: {:?}",
        economy.manifests
    );
    assert!(
        store
            .get(
                &PartitionKey::new(Domain::Economy, "global").unwrap(),
                &policy_key
            )
            .is_none(),
        "a policy variable was written to the economy partition"
    );
}
