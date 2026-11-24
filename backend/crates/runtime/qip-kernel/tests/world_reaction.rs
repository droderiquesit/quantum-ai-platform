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
//! Since then: macro releases are measured too, against a materiality rule
//! on their series' own history and paired with every instrument; the
//! shortest horizon is read from ticks, which the default feed carries; and
//! each applied episode is journaled, so a restart rebuilds the graph from
//! the log instead of applying anything twice.
//!
//! These tests drive the platform on the six-instrument synthetic exchange
//! at its default configuration (`EnvironmentConfig::default()`: a 60 s
//! step and minute bars), a cycle every 30 minutes of feed time, the
//! committed universe catalogue and `PlatformConfig::default()`, and read
//! only what `run_cycle` left behind. That is **not** `qip-fastbrain`'s
//! cadence — its node steps the feed every 100 ms with second bars through
//! `admit_world_records` and the bus bridge — and the claim that it was
//! stood here until a reader checked. The node's own cadence is driven in
//! `qip-fastbrain/tests/world_reaction.rs`.

#![allow(clippy::panic_in_result_fn, clippy::expect_used)] // integration tests may expect: a panic is the failure report

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
    /// Every tick the feed published, as `(instrument, instant, price)`.
    ticks: Vec<(String, Timestamp, f64)>,
    /// Every bar the feed published, as `(instrument, open, close time, close)`.
    bars: Vec<(String, Timestamp, Timestamp, f64)>,
    /// The instant of the last cycle run.
    last_cycle: Timestamp,
}

/// Feed the default synthetic exchange into a default platform, a cycle every
/// [`CYCLE_EVERY`], until `done` holds of the platform or `days` have passed.
fn run(days: i64, done: impl Fn(&Platform) -> bool) -> Result<Run> {
    run_with(PlatformConfig::default(), days, done)
}

/// [`run`] on a given platform configuration.
fn run_with(config: PlatformConfig, days: i64, done: impl Fn(&Platform) -> bool) -> Result<Run> {
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
    let mut ticks = Vec::new();
    let mut bars = Vec::new();
    let mut last_cycle = start();
    while environment.cursor() < until {
        let records = environment.run_until(environment.cursor().saturating_add(CYCLE_EVERY));
        routine_stories += records
            .iter()
            .filter(
                |r| matches!(r, SensedRecord::News(item) if item.item_id.starts_with("routine-")),
            )
            .count();
        for record in &records {
            match record {
                SensedRecord::Tick(tick) => ticks.push((
                    tick.object_id.as_str().to_string(),
                    tick.at,
                    tick.price.to_f64(),
                )),
                SensedRecord::Bar(bar) => bars.push((
                    bar.object_id.as_str().to_string(),
                    bar.open_time,
                    bar.close_time(),
                    bar.close.to_f64(),
                )),
                _ => {}
            }
        }
        platform.observe(records);
        last_cycle = environment.cursor();
        platform.run_cycle(last_cycle);
        if done(&platform) {
            break;
        }
    }
    Ok(Run {
        platform,
        environment,
        routine_stories,
        ticks,
        bars,
        last_cycle,
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
/// `material_news_per_day` to zero (no event fires); read the first bar
/// after the event for every horizon in `bar_move` (the 5- and 20-bar moves
/// disagree with the published bars). Treating every story as
/// material survives here, because this run stops at the first release's
/// twentieth bar; the immaterial-release test below is the one that kills it.
#[test]
fn a_material_event_on_the_default_feed_yields_multi_asset_reaction_records_at_three_horizons()
-> Result<()> {
    let Run {
        platform,
        environment,
        routine_stories,
        bars,
        ..
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

    // Each bar horizon's measured move, against the published bars: the
    // close of the `h`-th bar opening at or after the event, against the
    // close of the bar before it. Recomputed here from what the feed
    // published, not read back through the platform, so a loop that read the
    // first bar for every horizon — and still labelled the episodes 5 and 20
    // — fails here (it passed every assertion above).
    let mut later_differs_from_first = false;
    for episode in episodes.iter().filter(|e| e.lag_bars > 1) {
        let h = episode.lag_bars;
        for (instrument, moved, _) in &episode.affected_instruments {
            let series: Vec<&(String, Timestamp, Timestamp, f64)> =
                bars.iter().filter(|(i, ..)| i == instrument).collect();
            let first = series
                .iter()
                .position(|(_, open, _, _)| *open >= at)
                .expect("a bar opened after the release");
            assert!(
                first > 0,
                "premise: {instrument} has no bar before the release"
            );
            let base = series[first - 1].3;
            let horizon_bar = series[first + h - 1];
            let expected = (horizon_bar.3 / base - 1.0) * 10_000.0;
            assert!(
                (moved - expected).abs() < 1e-9,
                "{instrument} moved {moved} bps in {}, its {h}th bar says {expected}",
                episode.episode_id
            );
            if instrument == &issuer {
                assert_eq!(
                    episode.reaction_time, horizon_bar.2,
                    "{} is not dated at the close of its {h}th bar",
                    episode.episode_id
                );
            }
            let at_first_bar = (series[first].3 / base - 1.0) * 10_000.0;
            if (expected - at_first_bar).abs() > 1e-9 {
                later_differs_from_first = true;
            }
        }
    }
    assert!(
        later_differs_from_first,
        "premise: the 5- and 20-bar moves equal the first bar's on every instrument, so this \
         test cannot tell the horizons apart"
    );

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
                SensedRecord::News(n)
                    if n.item_id == format!("earnings-NWSC-{}", at.as_nanos()) =>
                {
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
    assert_eq!(
        published.len(),
        2,
        "the immaterial release was not published whole"
    );
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

/// The synthetic macro series, as the feed names it.
const MACRO_SERIES: &str = qip_market_ingestion::synthetic::SYNTHETIC_MACRO_SERIES;

/// The materiality rule for a macro release, written from the decision
/// record rather than imported from the code under test:
/// `docs/blueprint/owner-decisions-2026-11-23.md`, section "Figures decided
/// on 2026-11-24", row R12-WORLD-6015 — a release is material when its
/// surprise is at least 1 sample standard deviation of that series' own past
/// surprises, with at least 2 past surprises. The two figures are literals
/// on purpose: an oracle that read `MACRO_MATERIALITY_SIGMAS` would loosen
/// with the constant it is meant to check. Returns, per release in order,
/// whether it is material.
fn material_by_the_stated_rule(surprises: &[f64]) -> Vec<bool> {
    // The decision record's figures.
    const SIGMAS: f64 = 1.0;
    const MIN_HISTORY: usize = 2;
    surprises
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let history = &surprises[..i];
            if history.len() < MIN_HISTORY {
                return false;
            }
            let n = history.len() as f64;
            let mean = history.iter().sum::<f64>() / n;
            let sd = (history.iter().map(|h| (h - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
            sd > 0.0 && s.abs() >= SIGMAS * sd
        })
        .collect()
}

/// Macro releases on the default feed are measured like company events: a
/// release whose surprise reaches one standard deviation of its series' own
/// past surprises yields reaction records at all three horizons, each on
/// every instrument the feed carries, and a release that does not yields
/// none.
///
/// Mutations (each fired): `with_material(material || true)` on the kernel's
/// macro arm (an immaterial release reacts); `MACRO_MATERIALITY_SIGMAS` set
/// to 0 (a release below one deviation reacts); drop the macro landing in
/// `react_to_world_events` (the material release at 2026-10-14T08:53 reacts
/// at no horizon).
#[test]
fn a_material_macro_release_on_the_default_feed_reacts_on_every_instrument_and_an_immaterial_one_does_not()
-> Result<()> {
    let Run {
        platform,
        environment,
        last_cycle,
        ..
    // Fourteen whole days, not "until the first macro reaction": a stop on
    // the first reaction stops wherever a broken rule first reacts, which
    // under a rule that admits everything is before any release the stated
    // rule calls material has settled — the test then failed on its premise
    // rather than on the immaterial release reacting. The default feed's
    // first release the stated rule calls material fires on day 13.
    } = run(14, |_| false)?;
    let releases: Vec<(Timestamp, f64)> = environment
        .fired_events()
        .iter()
        .filter_map(|e| match &e.kind {
            ScriptedEventKind::MacroSurprise {
                series_id,
                surprise,
                ..
            } if series_id == MACRO_SERIES => Some((e.at, *surprise)),
            _ => None,
        })
        .collect();
    let surprises: Vec<f64> = releases.iter().map(|(_, s)| *s).collect();
    let material = material_by_the_stated_rule(&surprises);
    // Premise: the feed published releases on both sides of the rule, old
    // enough that every horizon has closed.
    // Settled: the twentieth one-minute bar after the release has closed by
    // the last cycle (it opens nineteen minutes after the release's own bar).
    let settled = |at: Timestamp| at.saturating_add(Duration::from_mins(21)) <= last_cycle;
    let material_settled: Vec<Timestamp> = releases
        .iter()
        .zip(&material)
        .filter(|((at, _), m)| **m && settled(*at))
        .map(|((at, _), _)| *at)
        .collect();
    let immaterial_settled: Vec<Timestamp> = releases
        .iter()
        .zip(&material)
        .filter(|((at, _), m)| !**m && settled(*at))
        .map(|((at, _), _)| *at)
        .collect();
    assert!(
        !material_settled.is_empty(),
        "premise: no material macro release settled ({} releases)",
        releases.len()
    );
    assert!(
        !immaterial_settled.is_empty(),
        "premise: every macro release was material"
    );
    let carried: BTreeSet<String> = environment
        .instruments()
        .iter()
        .map(|i| i.object_id.as_str().to_string())
        .collect();
    let episodes_of = |at: Timestamp| -> Vec<&qip_world_model::reaction::ReactionEpisode> {
        let id = format!("{MACRO_SERIES}:{}", at.as_nanos());
        platform
            .reaction_episodes()
            .iter()
            .filter(|e| e.episode_id.rsplit_once('@').is_some_and(|(e, _)| e == id))
            .collect()
    };
    for at in &material_settled {
        let episodes = episodes_of(*at);
        assert_eq!(
            episodes.iter().map(|e| e.lag_bars).collect::<Vec<_>>(),
            vec![1, 5, 20],
            "the material release at {at} did not react at exactly three horizons"
        );
        for episode in episodes {
            assert_eq!(episode.event_name, format!("macro/{MACRO_SERIES}"));
            let measured: Vec<String> = episode
                .affected_instruments
                .iter()
                .map(|(id, _, _)| id.clone())
                .collect();
            let set: BTreeSet<String> = measured.iter().cloned().collect();
            assert_eq!(
                set, carried,
                "{} was not paired with every instrument",
                episode.episode_id
            );
            assert_eq!(set.len(), measured.len(), "an instrument measured twice");
        }
    }
    for at in &immaterial_settled {
        assert!(
            episodes_of(*at).is_empty(),
            "the immaterial macro release at {at} yielded a reaction record"
        );
    }
    Ok(())
}

/// The shortest horizon is read from ticks, the finest resolution the
/// default feed carries: each instrument's move in the 1-horizon episode is
/// its first tick after the event against its last tick before it, at the
/// first tick's instant — and that differs from what the bars would have
/// said, so the test can tell the two apart.
///
/// Mutation (fired): measure the shortest horizon from bars.
#[test]
fn the_shortest_reaction_horizon_is_measured_from_the_feeds_ticks() -> Result<()> {
    let Run {
        platform,
        ticks,
        bars,
        ..
    } = run(3, |p| {
        p.reaction_episodes()
            .iter()
            .any(|e| e.episode_id.starts_with("earnings-") && e.lag_bars == 1)
    })?;
    let episode = platform
        .reaction_episodes()
        .iter()
        .find(|e| e.episode_id.starts_with("earnings-") && e.lag_bars == 1)
        .expect("a material release reacted at its shortest horizon");
    let at = episode.event_time;
    assert!(
        episode.affected_instruments.len() > 1,
        "premise: one instrument measured"
    );
    let mut differs_from_bars = false;
    for (instrument, moved, _) in &episode.affected_instruments {
        let mine: Vec<(Timestamp, f64)> = ticks
            .iter()
            .filter(|(i, _, _)| i == instrument)
            .map(|(_, t, p)| (*t, *p))
            .collect();
        let base = mine
            .iter()
            .rev()
            .find(|(t, _)| *t < at)
            .map(|(_, p)| *p)
            .expect("a tick before the event");
        let (reacted_at, price) = *mine
            .iter()
            .find(|(t, _)| *t > at)
            .expect("a tick after the event");
        let expected = (price / base - 1.0) * 10_000.0;
        assert!(
            (moved - expected).abs() < 1e-9,
            "{instrument} moved {moved} bps in {}, its ticks say {expected}",
            episode.episode_id
        );
        if instrument == &episode.affected_instruments[0].0 {
            assert_eq!(
                episode.reaction_time, reacted_at,
                "the reaction is not dated at the first tick after the event"
            );
        }
        let series: Vec<&(String, Timestamp, Timestamp, f64)> =
            bars.iter().filter(|(i, ..)| i == instrument).collect();
        let first = series
            .iter()
            .position(|(_, open, _, _)| *open >= at)
            .expect("a bar after the event");
        let by_bars = (series[first].3 / series[first - 1].3 - 1.0) * 10_000.0;
        if (by_bars - expected).abs() > 1e-9 {
            differs_from_bars = true;
        }
    }
    assert!(
        differs_from_bars,
        "premise: the ticks and the bars agree on every instrument, so this test cannot tell \
         which the episode was read from"
    );
    assert!(
        episode.rationale.ends_with("over 1 tick"),
        "the episode does not say it was read from ticks: {}",
        episode.rationale
    );
    Ok(())
}

/// The episodes a WorldReactionRecorded record carries in `platform`'s log.
fn journaled_reactions(platform: &Platform) -> Vec<String> {
    use qip_events::EventBody;
    use qip_kernel::platform::WorldReactionRecorded;
    platform
        .event_log()
        .records()
        .iter()
        .filter(|r| r.event.topic == WorldReactionRecorded::TOPIC)
        .filter_map(|r| {
            qip_streaming::envelope::StreamEnvelope::from_frame(&r.event)
                .and_then(|e| e.decode::<WorldReactionRecorded>())
                .ok()
                .map(|d| d.body.episode.episode_id)
        })
        .collect()
}

/// The reaction-learnt edges of the causal graph, with their confidence.
fn reaction_edges(platform: &Platform) -> Vec<(String, String, f64, usize)> {
    platform
        .world()
        .causal()
        .edges()
        .iter()
        .filter(|e| e.evidence.iter().any(|id| id.starts_with("reaction:")))
        .map(|e| {
            (
                e.cause.clone(),
                e.effect.clone(),
                e.confidence,
                e.evidence.len(),
            )
        })
        .collect()
}

/// A restart rebuilds the reaction learning from the hash-chained log and
/// applies nothing twice: every episode the first process applied is
/// journaled with the confidence changes it caused; a second process on the
/// same log holds the same episodes, contradictions and edge confidences
/// before it has sensed anything; and when the feed re-delivers the same
/// events cycle by cycle, no episode is applied or journaled again.
///
/// Mutations (each fired): drop the `journal_record` in
/// `react_to_world_events` (the restart holds nothing); skip the
/// `apply_reaction` in `resume_world_reactions` (the edges are lost); skip
/// inserting into `reactions_resumed` (the replay applies every episode a
/// second time).
#[test]
fn a_restart_rebuilds_reaction_learning_from_the_log_and_applies_nothing_twice() -> Result<()> {
    let directory =
        std::env::temp_dir().join(format!("qip-world-reaction-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let path = directory.join("events.log");
    let config = PlatformConfig::default().with_event_log_file(&path);
    let first = run_with(config.clone(), 10, |p| {
        !p.reaction_contradictions().is_empty()
    })?;
    let episodes: Vec<String> = first
        .platform
        .reaction_episodes()
        .iter()
        .map(|e| e.episode_id.clone())
        .collect();
    let contradictions = first.platform.reaction_contradictions().to_vec();
    let edges = reaction_edges(&first.platform);
    let journaled = journaled_reactions(&first.platform);
    // Premise: the first process learnt something, contradicted an edge,
    // and journaled each episode it applied exactly once.
    assert!(!episodes.is_empty(), "premise: no reaction was applied");
    assert!(
        !contradictions.is_empty(),
        "premise: no edge was contradicted"
    );
    assert!(
        !edges.is_empty(),
        "premise: no edge was learnt from a reaction"
    );
    assert_eq!(
        journaled, episodes,
        "the log does not hold exactly the episodes the process applied"
    );
    let last = first.last_cycle;
    drop(first);

    // The second process, on the same log, before it senses anything.
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut second = Platform::new(
        config,
        context,
        Telemetry::silent(),
        catalogue(),
        LimitSet::new("world-reaction-test"),
    )?;
    let resumed: Vec<String> = second
        .reaction_episodes()
        .iter()
        .map(|e| e.episode_id.clone())
        .collect();
    assert_eq!(
        resumed, episodes,
        "the restart did not rebuild the episodes"
    );
    assert_eq!(
        second.reaction_contradictions(),
        contradictions.as_slice(),
        "the restart did not rebuild the contradictions"
    );
    assert_eq!(
        reaction_edges(&second),
        edges,
        "the restart did not rebuild the reaction-learnt edges"
    );

    // The same feed again, cycle by cycle, to the same instant.
    let mut environment = SyntheticEnvironment::demo(start(), EnvironmentConfig::default());
    while environment.cursor() < last {
        let records = environment.run_until(environment.cursor().saturating_add(CYCLE_EVERY));
        second.observe(records);
        second.run_cycle(environment.cursor());
    }
    assert_eq!(
        environment.cursor(),
        last,
        "premise: the replay stopped elsewhere"
    );
    let replayed: Vec<String> = second
        .reaction_episodes()
        .iter()
        .map(|e| e.episode_id.clone())
        .collect();
    assert_eq!(
        replayed, episodes,
        "the re-delivered events were applied again"
    );
    assert_eq!(
        journaled_reactions(&second),
        episodes,
        "the re-delivered events were journaled again"
    );
    assert_eq!(
        reaction_edges(&second),
        edges,
        "the re-delivered events moved an edge's confidence a second time"
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}
