//! The world-market reaction loop on the shipped default path (R12-WORLD-6015).
//!
//! Blueprint §22: each material world event is paired with the observed
//! multi-asset response over several horizons, and those reaction episodes
//! update the causal propagation model. The loop (`react_to_world_events`,
//! called from `run_cycle`) existed and was proven on a hand-built fixture,
//! while on the feed every shipped binary runs it matched nothing: synthetic
//! stories named no entity, so no story became a world event; a story named
//! a company and the bars an instrument, with nothing joining the two; and
//! every routine headline would have counted as evidence had it matched.
//!
//! These tests drive the platform exactly as `qip-fastbrain` does — the
//! six-instrument synthetic exchange on its default configuration
//! (`EnvironmentConfig::default()`, which `Feed::synthetic` spreads), the
//! committed universe catalogue, `PlatformConfig::default()` — and read only
//! what `run_cycle` left behind.

#![allow(clippy::panic_in_result_fn)]

use std::collections::BTreeSet;

use qip_core::Context;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::synthetic::{EnvironmentConfig, ScriptedEventKind, SyntheticEnvironment};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

/// The catalogue every central root reads, relative to this crate so the test
/// reads the committed artefact and not a copy of it.
const COMMITTED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/universe.json"
);

/// How far the feed advances between cycles.
const CYCLE_EVERY: Duration = Duration::from_mins(30);

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 10, 1)
}

fn catalogue() -> Universe {
    let text = std::fs::read_to_string(COMMITTED).expect("the committed catalogue is readable");
    qip_financial::catalogue::load(&text, start())
        .expect("the committed catalogue loads")
        .universe
}

/// What one run of the default feed through the default platform left.
struct Run {
    platform: Platform,
    environment: SyntheticEnvironment,
    routine_stories: usize,
}

/// Feed the default synthetic exchange into a default platform, a cycle every
/// [`CYCLE_EVERY`], until `done` holds of the platform or `days` have passed.
fn run(days: i64, done: impl Fn(&Platform) -> bool) -> Result<Run> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        catalogue(),
        LimitSet::new("world-reaction-test"),
    )?;
    let mut environment = SyntheticEnvironment::demo(start(), EnvironmentConfig::default());
    let until = start().saturating_add(Duration::from_days(days));
    let mut routine_stories = 0;
    while environment.cursor() < until {
        let records = environment.run_until(environment.cursor().saturating_add(CYCLE_EVERY));
        routine_stories += records
            .iter()
            .filter(
                |r| matches!(r, SensedRecord::News(item) if item.item_id.starts_with("routine-")),
            )
            .count();
        platform.observe(records);
        platform.run_cycle(environment.cursor());
        if done(&platform) {
            break;
        }
    }
    Ok(Run {
        platform,
        environment,
        routine_stories,
    })
}

/// The object id of the instrument the synthetic world lists for `entity`.
fn instrument_of(environment: &SyntheticEnvironment, entity: &str) -> String {
    environment
        .instruments()
        .iter()
        .find(|i| i.contract.is_none() && i.entity_id.as_deref() == Some(entity))
        .map(|i| i.object_id.as_str().to_string())
        .expect("the entity lists an instrument")
}

/// A material earnings release on the default feed yields a reaction record
/// at each of the three horizons, each measuring the response on **every**
/// instrument the feed carries — the issuer's own first — and no routine
/// story yields one.
///
/// Mutations (each fired): skip the `entity_instruments` lookup in
/// `react_to_world_events` (nothing lands); measure only the landing
/// instrument (one response per episode); set the default
/// `material_news_per_day` to zero (no event fires). Treating every story as
/// material survives here, because this run stops at the first release's
/// twentieth bar; the immaterial-release test below is the one that kills it.
#[test]
fn a_material_event_on_the_default_feed_yields_multi_asset_reaction_records_at_three_horizons()
-> Result<()> {
    let Run {
        platform,
        environment,
        routine_stories,
    } = run(3, |p| {
        p.reaction_episodes()
            .iter()
            .any(|e| e.episode_id.starts_with("earnings-") && e.lag_bars == 20)
    })?;

    // Premise: the feed published a material release and routine noise.
    let (entity, at) = environment
        .fired_events()
        .iter()
        .find_map(|e| match &e.kind {
            ScriptedEventKind::EarningsSurprise { entity_id, .. } => {
                Some((entity_id.clone(), e.at))
            }
            _ => None,
        })
        .expect("the default feed published a material release within three days");
    assert!(
        routine_stories > 0,
        "premise: no routine story was published"
    );

    let issuer = instrument_of(&environment, &entity);
    let carried: BTreeSet<String> = environment
        .instruments()
        .iter()
        .map(|i| i.object_id.as_str().to_string())
        .collect();
    assert!(
        carried.len() > 1,
        "premise: the feed carries one instrument"
    );

    // The story about that release, at its own instant.
    let ticker = environment
        .companies()
        .iter()
        .find(|c| c.entity_id == entity)
        .map(|c| c.ticker.clone())
        .expect("the entity is a company");
    let event_id = format!("earnings-{ticker}-{}:{entity}", at.as_nanos());
    let episodes: Vec<_> = platform
        .reaction_episodes()
        .iter()
        .filter(|e| {
            e.episode_id
                .rsplit_once('@')
                .is_some_and(|(id, _)| id == event_id)
        })
        .collect();
    let horizons: Vec<usize> = episodes.iter().map(|e| e.lag_bars).collect();
    assert_eq!(
        horizons,
        vec![1, 5, 20],
        "the release {event_id} did not react at exactly the three horizons"
    );
    for episode in &episodes {
        assert_eq!(episode.event_name, "earnings", "{}", episode.episode_id);
        assert_eq!(episode.event_time, at, "{}", episode.episode_id);
        let measured: Vec<&str> = episode
            .affected_instruments
            .iter()
            .map(|(id, _, _)| id.as_str())
            .collect();
        assert_eq!(
            measured.first().copied(),
            Some(issuer.as_str()),
            "{} did not land on the issuer's instrument first",
            episode.episode_id
        );
        let set: BTreeSet<String> = measured.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            set, carried,
            "{} did not measure every instrument the feed carries",
            episode.episode_id
        );
        assert_eq!(
            set.len(),
            measured.len(),
            "an instrument was measured twice"
        );
    }

    // No routine headline reached the causal graph as evidence.
    assert!(
        platform
            .reaction_episodes()
            .iter()
            .all(|e| !e.episode_id.starts_with("routine-")),
        "a routine story was treated as a material event"
    );
    Ok(())
}

/// On the same default feed, a later material event whose market moved
/// against an edge an earlier one established lowers that edge's
/// confidence in the causal graph, and the graph holds the lowered figure.
///
/// Mutations: skip `weaken_on_contradiction` in `granger::apply_reaction`;
/// drop the `reaction_contradictions.extend` in `react_to_world_events`.
#[test]
fn a_contradicting_reaction_on_the_default_feed_lowers_a_causal_edges_confidence() -> Result<()> {
    let Run { platform, .. } = run(10, |p| !p.reaction_contradictions().is_empty())?;
    let contradiction = platform
        .reaction_contradictions()
        .first()
        .expect("no reaction on ten days of the default feed contradicted a held edge");
    // Premise: the contradiction is about a measured reaction to a material
    // event, not a fixture.
    let (event_id, _) = contradiction
        .episode_id
        .rsplit_once('@')
        .expect("an episode id names its horizon");
    assert!(
        platform
            .reaction_episodes()
            .iter()
            .any(|e| e.episode_id == contradiction.episode_id),
        "the contradicting episode {} is not among the recorded reactions",
        contradiction.episode_id
    );
    assert!(
        !event_id.starts_with("routine-"),
        "a routine story contradicted an edge"
    );
    assert!(
        contradiction.expected_sign * contradiction.observed_sign < 0.0,
        "a contradiction whose observed move agreed with the edge"
    );
    assert!(
        contradiction.confidence_after < contradiction.confidence_before,
        "the contradicting reaction did not lower the edge's confidence: {} -> {}",
        contradiction.confidence_before,
        contradiction.confidence_after
    );
    let world = platform.world();
    let edge = world
        .causal()
        .edges()
        .iter()
        .find(|e| e.cause == contradiction.cause && e.effect == contradiction.effect)
        .expect("the contradicted edge is kept in the graph, not deleted");
    assert!(
        edge.confidence <= contradiction.confidence_after,
        "the graph holds {} for {} -> {}, above the lowered {}",
        edge.confidence,
        edge.cause,
        edge.effect,
        contradiction.confidence_after
    );
    assert!(
        edge.evidence
            .iter()
            .any(|id| id.starts_with(&format!("reaction:{}:", contradiction.episode_id))),
        "the edge does not cite the reaction that contradicted it"
    );
    Ok(())
}

/// An immaterial release — a two-per-cent surprise, below the world model's
/// five-per-cent threshold for the fundamental and below
/// `Sentiment::is_material` for its story — reaches the platform on the
/// default feed and yields no reaction record, while the material releases
/// around it do.
///
/// Mutations (each fired): `with_material(true)` in the kernel's fundamental
/// arm (the fundamental reacts); `with_material(true)` in
/// `MarketEvent::from_news` (the story reacts); drop `!event.material` from
/// `react_to_world_events` (both react).
#[test]
fn an_immaterial_release_on_the_default_feed_yields_no_reaction_record() -> Result<()> {
    use qip_market_ingestion::synthetic::ScriptedEvent;
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        catalogue(),
        LimitSet::new("world-reaction-test"),
    )?;
    let mut environment = SyntheticEnvironment::demo(start(), EnvironmentConfig::default());
    // On a step boundary, so the release is stamped at exactly this instant.
    let at = start().saturating_add(Duration::from_hours(6));
    environment.schedule(ScriptedEvent::new(
        at,
        ScriptedEventKind::EarningsSurprise {
            entity_id: "ent-northwind".into(),
            metric: "revenue".into(),
            surprise: 0.02,
        },
    ));
    let until = start().saturating_add(Duration::from_days(2));
    let mut published = Vec::new();
    while environment.cursor() < until {
        let records = environment.run_until(environment.cursor().saturating_add(CYCLE_EVERY));
        for record in &records {
            match record {
                SensedRecord::Fundamental(f) if f.period_end == at => {
                    published.push(format!("{}:{}:{}", f.entity_id, f.metric, at.as_nanos()));
                }
                SensedRecord::News(n) if n.item_id == format!("earnings-NWSC-{}", at.as_nanos()) => {
                    published.push(format!("{}:ent-northwind", n.item_id));
                }
                _ => {}
            }
        }
        platform.observe(records);
        platform.run_cycle(environment.cursor());
    }
    // Premise: both records of the immaterial release were published, and
    // the loop did react to material events on the same run.
    assert_eq!(published.len(), 2, "the immaterial release was not published whole");
    assert!(
        platform
            .reaction_episodes()
            .iter()
            .any(|e| e.episode_id.starts_with("earnings-") && e.event_time != at),
        "premise: no material release reacted on the same run"
    );
    for event_id in &published {
        assert!(
            platform.reaction_episodes().iter().all(|e| e
                .episode_id
                .rsplit_once('@')
                .is_none_or(|(id, _)| id != event_id)),
            "the immaterial event {event_id} yielded a reaction record"
        );
    }
    Ok(())
}
