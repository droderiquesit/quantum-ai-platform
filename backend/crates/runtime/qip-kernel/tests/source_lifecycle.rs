//! Policy moves a registered source, and the move is on the log (DATA-018).
//!
//! `qip-data-finder`'s own suite proves the policy. This proves the half it
//! cannot see: that a transition decided inside `Platform::assess_sources`
//! reaches the hash-chained journal, because the registry that was moved is
//! in memory and a retired source is otherwise a source that never existed.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::governance::Usage;
use qip_core::Context;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal};
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
use qip_data_finder::finder::DataFinder;
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_data_finder::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_data_finder::{LifecycleAction, RoutingClass};
use qip_events::{EventFilter, Topic};
use qip_financial::asset_class::AssetClass;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_kernel::source_lifecycle::SourceLifecycleChanged;
use qip_observability::Telemetry;
use qip_prediction::resolution::{Observation, Observations};
use qip_risk::limits::LimitSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn later() -> Timestamp {
    start().saturating_add(Duration::from_hours(1))
}

fn platform() -> Result<Platform> {
    // A named desk other than the shipped default, so an entry's owner is
    // visibly the configured one rather than whatever the default happens to
    // be (see `a_catalogue_entry_names_the_configured_operator_and_the_process_name_is_refused`).
    platform_owned_by("market-data")
}

fn platform_owned_by(owner: &str) -> Result<Platform> {
    let config = PlatformConfig::default().with_owner(owner);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

/// No declared history unless `history_days` says so, so the composite is
/// the sum the comments below state (see `qip-data-finder`'s
/// `tests/lifecycle_policy.rs`, whose fixture this mirrors).
fn source(
    id: &str,
    url: &str,
    instrument: &str,
    monthly: i64,
    history_days: i64,
) -> Result<SourceCandidate> {
    SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd")?,
        SourceEndpoint::parse(
            url,
            AccessMechanism::Rest {
                auth: AuthRequirement::None,
                incremental_parameter: None,
                page_size: 100,
            },
        )?,
        SourceCoverage::new(
            [AssetClass::Equity],
            [SourceRegion::UsEast],
            [instrument.to_string()],
            UpdateFrequency::Minutely,
        )?
        .with_history_from(start().saturating_sub(Duration::from_days(history_days))),
        LicensingPosture::declared(SourceLicense::new(
            "vendor-terms-2026",
            [Usage::Research, Usage::Derive, Usage::Trade],
        )?),
        SourceCost::new(
            Decimal::from_int(monthly),
            Decimal::ZERO,
            u64::MAX,
            Currency::USD,
        )?,
        SourceRegion::UsEast,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        start(),
    )
}

fn payload_at(at: Timestamp) -> PayloadSample {
    PayloadSample {
        body: qip_core::untrusted::Untrusted::new(
            "a scripted sample",
            r#"{"symbol":"US0001","bid":10.25,"ask":10.27}"#.to_string(),
        ),
        media_type: "application/json".to_string(),
        payload_at: Some(at),
        latency: Duration::from_millis(55),
    }
}

fn stale(at: Timestamp) -> PayloadSample {
    payload_at(at.saturating_sub(Duration::from_mins(10)))
}

const RIVAL: &str = "https://rival.example/quotes";
const FADING: &str = "http://fading.example/quotes";
const FORGER: &str = "https://forger.example/quotes";
const RISING: &str = "https://rising.example/quotes";

fn probe() -> InMemoryProbe {
    let mut probe = InMemoryProbe::new();
    for (host, url) in [
        ("rival.example", RIVAL),
        ("fading.example", FADING),
        ("forger.example", FORGER),
        ("rising.example", RISING),
    ] {
        probe = probe
            .with_robots(
                host,
                RobotsFetch::Served {
                    body: "User-agent: *\nAllow: /\n".to_string(),
                    latency: Duration::from_millis(12),
                },
            )
            .with_head(
                url,
                HeadResponse {
                    status: 200,
                    content_type: Some("application/json".to_string()),
                    content_length: Some(512),
                    last_modified: Some(start()),
                    latency: Duration::from_millis(40),
                },
            );
    }
    probe
        .with_sample(RIVAL, payload_at(start()))
        .with_sample(RIVAL, payload_at(later()))
        // Plaintext, at the cost ceiling, redundant with `rival`: 0.38 fresh
        // (cold), 0.18 stale — under the floor.
        .with_sample(FADING, payload_at(start()))
        .with_sample(FADING, stale(later()))
        // Healthy, then dating its records a day after they are fetched.
        .with_sample(FORGER, payload_at(start()))
        .with_sample(
            FORGER,
            payload_at(later().saturating_add(Duration::from_days(1))),
        )
        // 0.62 stale (warm), 0.82 fresh (hot).
        .with_sample(RISING, stale(start()))
        .with_sample(RISING, payload_at(later()))
}

fn candidates() -> Result<Vec<SourceCandidate>> {
    Ok(vec![
        source("a-rival", RIVAL, "US0001", 0, 3_650)?,
        source(
            "fading",
            FADING,
            "US0001",
            DataFinder::COST_CEILING_PER_MONTH,
            0,
        )?,
        source("forger", FORGER, "US0002", 0, 0)?,
        source("rising", RISING, "US0003", 0, 0)?,
    ])
}

fn journaled(platform: &Platform) -> Result<Vec<SourceLifecycleChanged>> {
    platform
        .replay_journal(&EventFilter::new().topic(Topic::SourceLifecycleChanged))?
        .iter()
        .map(|envelope| Ok(envelope.decode::<SourceLifecycleChanged>()?.body))
        .collect()
}

#[test]
fn each_transition_policy_makes_to_a_registered_source_appears_in_the_event_log() -> Result<()> {
    let mut platform = platform()?;
    let mut probe = probe();

    // The premise: all four register, none of that is a transition, and the
    // log holds no lifecycle record yet — so whatever it holds afterwards was
    // written by the second pass and by nothing else.
    let first = platform.assess_sources(candidates()?, &mut probe, start())?;
    assert_eq!(first.registered(), 4, "{:?}", first.decisions);
    assert!(journaled(&platform)?.is_empty());
    let class = |platform: &Platform, id: &str| {
        platform
            .registered_sources()
            .get(id)
            .map(|entry| entry.routing().class())
    };
    assert_eq!(class(&platform, "fading"), Some(RoutingClass::Cold));
    assert_eq!(class(&platform, "rising"), Some(RoutingClass::Warm));

    // No operator action between the passes: the probe's answers change and
    // nothing else does.
    platform.assess_sources(candidates()?, &mut probe, later())?;

    let records = journaled(&platform)?;
    let moved: Vec<(&str, LifecycleAction)> = records
        .iter()
        .map(|record| {
            (
                record.transition.source_id.as_str(),
                record.transition.action,
            )
        })
        .collect();
    assert_eq!(
        moved,
        vec![
            ("fading", LifecycleAction::Retired),
            ("forger", LifecycleAction::Quarantined),
            ("rising", LifecycleAction::Promoted),
        ],
        "the log does not hold exactly the three transitions policy made"
    );
    assert!(
        records
            .iter()
            .all(|record| record.transition.at == later() && !record.transition.reason.is_empty()),
        "a transition was journaled without its instant or its reason: {records:?}"
    );
    // And the registry agrees with the record, so the log describes what
    // happened rather than what was intended.
    assert!(!platform.registered_sources().contains_key("fading"));
    assert!(
        platform
            .registered_sources()
            .get("forger")
            .is_some_and(|entry| entry.is_quarantined())
    );
    assert_eq!(class(&platform, "rising"), Some(RoutingClass::Hot));
    // A third pass on the same evidence moves nothing and writes nothing.
    platform.assess_sources(candidates()?, &mut probe, later())?;
    assert_eq!(
        journaled(&platform)?.len(),
        3,
        "an unchanged pass journaled"
    );
    Ok(())
}

#[test]
fn a_source_policy_quarantines_or_retires_is_no_longer_offered_by_the_mesh_catalogue() -> Result<()>
{
    // The catalogue is the one answer to "may this dataset be read". A
    // quarantine that reached the finder's registry and not the catalogue
    // left that answer at "yes" for a feed whose publisher had said no.
    let mut platform = platform()?;
    let mut probe = probe();
    let readable = |platform: &Platform, id: &str, at: Timestamp| {
        platform
            .catalog()
            .usable_for(&format!("source.{id}"), Usage::Trade, at)
            .is_ok()
    };

    platform.assess_sources(candidates()?, &mut probe, start())?;
    for id in ["fading", "forger", "rising"] {
        assert!(
            readable(&platform, id, start()),
            "the premise: {id} is offered by the mesh while it is registered"
        );
    }

    platform.assess_sources(candidates()?, &mut probe, later())?;
    assert!(
        !readable(&platform, "forger", later()),
        "a quarantined source is still offered by the mesh catalogue"
    );
    assert!(
        !readable(&platform, "fading", later()),
        "a retired source is still offered by the mesh catalogue"
    );
    assert!(
        readable(&platform, "rising", later()),
        "a promoted source stopped being offered"
    );
    Ok(())
}

#[test]
fn a_catalogue_entry_names_the_configured_operator_and_the_process_name_is_refused() -> Result<()> {
    // R12-DATA-2020. The catalogue entry a registration produces must be
    // owned by somebody who can be asked about the source later. The shipped
    // default owner was `qip-platform` — the process's own name — and until
    // this seam went through `NamedOperator` every production deployment
    // catalogued under it, so the owner field recorded nobody. A deployment
    // that configures that name, or any placeholder, is still refused.
    let mut named_probe = probe();
    let mut named = platform_owned_by("market-data")?;
    let assessment = named.assess_sources(candidates()?, &mut named_probe, start())?;
    assert!(
        assessment.registered() > 0,
        "the premise: the fixture registers at least one source"
    );
    assert!(
        assessment.catalogue_problems.is_empty(),
        "{:?}",
        assessment.catalogue_problems
    );
    assert!(!assessment.catalogued.is_empty());
    for dataset in &assessment.catalogued {
        assert_eq!(named.catalog().require(dataset)?.owner, "market-data");
    }

    // The same candidates under the old default: the registrations are
    // kept (the finder's registry is not the catalogue), and no entry reaches
    // the mesh. Each refusal names the source and says what to configure.
    let mut default_probe = probe();
    let mut defaulted = platform_owned_by("qip-platform")?;
    let assessment = defaulted.assess_sources(candidates()?, &mut default_probe, start())?;
    assert_eq!(
        assessment.registered(),
        defaulted.registered_sources().len(),
        "the registrations themselves are not lost"
    );
    assert!(
        assessment.catalogued.is_empty(),
        "{:?}",
        assessment.catalogued
    );
    assert!(defaulted.catalog().is_empty());
    assert_eq!(
        assessment.catalogue_problems.len(),
        assessment.registered(),
        "one recorded refusal per registration: {:?}",
        assessment.catalogue_problems
    );
    for problem in &assessment.catalogue_problems {
        assert!(
            problem.contains("`qip-platform` names a process") && problem.contains("configure"),
            "a refusal that does not say what to fix: {problem}"
        );
    }
    Ok(())
}

#[test]
fn the_shipped_default_owner_is_one_the_catalogue_accepts() -> Result<()> {
    // `46ef4c11` began refusing a process name as a catalogue owner while the
    // shipped default was still `qip-platform`, so every default composition
    // refused its own registrations and the end-to-end walk in
    // `qip-acceptance`'s `e2e` failed on it, blocking the dev deploy. The
    // refusal is right; a default that trips it is not. This pins the default
    // through the same seam rather than through `NamedOperator` alone, because
    // the failure was the composition, not the type.
    let mut default_probe = probe();
    let config = PlatformConfig::default();
    let owner = config.owner.clone();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )?;
    let assessment = platform.assess_sources(candidates()?, &mut default_probe, start())?;
    assert!(
        assessment.registered() > 0,
        "the premise: the fixture registers at least one source"
    );
    assert!(
        assessment.catalogue_problems.is_empty(),
        "the default composition refused its own registrations: {:?}",
        assessment.catalogue_problems
    );
    assert_eq!(assessment.catalogued.len(), assessment.registered());
    for dataset in &assessment.catalogued {
        assert_eq!(platform.catalog().require(dataset)?.owner, owner);
    }
    Ok(())
}

/// A story from `source`, mentioning one company, so the world model holds a
/// `ConcernsEntity` fact whose provenance is that source.
fn story_from(source: &str, item_id: &str) -> Result<qip_financial::intelligence::NewsItem> {
    use qip_financial::intelligence::{EntityMention, NewsItem, NewsSource, Sentiment};
    use qip_financial::manifest::SourceManifest;
    use qip_financial::quality::{DataQuality, Provenance};
    let published_at = start().saturating_sub(Duration::from_hours(2));
    Ok(NewsItem {
        item_id: item_id.to_string(),
        headline: format!("Acme Corporation guides revenue higher ({item_id})"),
        manifest: SourceManifest::of(
            source,
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
            kind: Default::default(),
            identifiers: Default::default(),
        }],
        sentiment: Sentiment {
            polarity: 0.8,
            confidence: 0.9,
            novelty: 0.7,
        },
        topics: vec!["guidance".to_string()],
        provenance: Provenance::new(source, published_at, start()),
        quality: DataQuality::clean(),
        evidence_unretrievable: false,
    })
}

#[test]
fn quarantining_a_source_stops_the_world_model_believing_what_it_said() -> Result<()> {
    // R12-RES-7097: a poisoned source has its evidence quarantined and the
    // beliefs citing it invalidated. Before this the quarantine reached the
    // finder's registry and the mesh catalogue, and every fact the source had
    // already written into the graph went on being believed and reasoned
    // over — the quarantine stopped the next poisoned fact and none of the old.
    use qip_market_ingestion::adapter::SensedRecord;
    let mut platform = platform()?;
    let mut probe = probe();
    platform.assess_sources(candidates()?, &mut probe, start())?;
    let absorbed = platform.observe(vec![
        SensedRecord::News(Box::new(story_from("forger", "news-forged")?)),
        SensedRecord::News(Box::new(story_from("rising", "news-clean")?)),
    ]);
    assert_eq!(absorbed, 2, "premise: both stories are absorbed");
    let believed = |platform: &Platform, source: &str, known_at: Timestamp| {
        platform
            .world()
            .graph()
            .facts_at(known_at, known_at)
            .iter()
            .filter(|fact| fact.relationship.source == source)
            .count()
    };
    assert!(
        believed(&platform, "forger", later()) > 0 && believed(&platform, "rising", later()) > 0,
        "premise: the graph holds facts from both sources before the quarantine"
    );

    platform.assess_sources(candidates()?, &mut probe, later())?;
    assert!(
        platform
            .registered_sources()
            .get("forger")
            .is_some_and(|entry| entry.is_quarantined()),
        "premise: policy quarantined forger on the second pass"
    );

    assert_eq!(
        believed(&platform, "forger", later()),
        0,
        "a quarantined source's facts are still believed"
    );
    assert!(
        believed(&platform, "rising", later()) > 0,
        "a clean source's facts were retracted with the quarantined one's"
    );
    // Kept, not deleted: what the platform knew before the quarantine is
    // still answerable, so a decision taken then stays explicable.
    assert!(
        believed(&platform, "forger", start()) > 0,
        "the retraction rewrote history instead of ending belief from the quarantine"
    );
    assert!(
        platform.world().changes().iter().any(|change| {
            change.kind == qip_world_model::state::ChangeKind::RelationshipRetracted
                && change.description.contains("`forger` quarantined")
        }),
        "the retraction is not on the world model's change journal"
    );
    Ok(())
}

#[test]
fn a_belief_citing_a_source_retired_as_stale_is_downweighted_not_retracted() -> Result<()> {
    // R12-RES-7097, the stale half: a source retired because its data went
    // stale has its beliefs downweighted. Before this a retirement stopped
    // the feed and left everything it had told the graph at the confidence a
    // fresh source earned, so the platform reasoned over stale figures as
    // firmly as current ones.
    use qip_market_ingestion::adapter::SensedRecord;
    let mut platform = platform()?;
    let mut probe = probe();
    platform.assess_sources(candidates()?, &mut probe, start())?;
    let absorbed = platform.observe(vec![
        SensedRecord::News(Box::new(story_from("fading", "news-fading")?)),
        SensedRecord::News(Box::new(story_from("rising", "news-clean")?)),
    ]);
    assert_eq!(absorbed, 2, "premise: both stories are absorbed");
    let confidences = |platform: &Platform, source: &str, known_at: Timestamp| -> Vec<f64> {
        let mut held: Vec<f64> = platform
            .world()
            .graph()
            .facts_at(known_at, known_at)
            .iter()
            .filter(|fact| fact.relationship.source == source)
            .map(|fact| fact.confidence)
            .collect();
        held.sort_by(f64::total_cmp);
        held
    };
    let before = confidences(&platform, "fading", later());
    assert!(
        !before.is_empty() && before.iter().all(|c| *c > 0.0),
        "premise: the graph believes facts from fading before it retires: {before:?}"
    );
    let rising_before = confidences(&platform, "rising", later());

    platform.assess_sources(candidates()?, &mut probe, later())?;
    assert!(
        !platform.registered_sources().contains_key("fading"),
        "premise: policy retired fading on the second pass"
    );

    let after = confidences(&platform, "fading", later());
    let expected: Vec<f64> = before
        .iter()
        .map(|c| c * qip_world_model::world::STALE_SOURCE_CONFIDENCE)
        .collect();
    assert_eq!(
        after, expected,
        "a stale source's beliefs were not held at the downweighted confidence"
    );
    assert_eq!(
        confidences(&platform, "rising", later()),
        rising_before,
        "a fresh source's beliefs moved with the stale one's"
    );
    // As of before the retirement, the confidence a decision was formed on.
    assert_eq!(
        confidences(&platform, "fading", start()),
        before,
        "the downweight rewrote what the platform believed before it"
    );
    assert!(
        platform.world().changes().iter().any(|change| {
            change.kind == qip_world_model::state::ChangeKind::BeliefRevised
                && change.description.contains("`fading` retired as stale")
        }),
        "the downweight is not on the world model's change journal"
    );
    Ok(())
}

/// R12-EXPAND-6055: the Source Registry entry is consulted where a source
/// registers, and travels with the assessment: one view per registration,
/// naming where the source was found, what it costs, the licence it was
/// decided on, how often it says it updates and the class it was routed to.
/// Until this change `RegisteredSource::entry` was read by tests alone.
/// Mutation: delete the `entries.push(..)` in `assess_sources` — the count
/// assertion fails.
#[test]
fn every_registration_carries_its_source_registry_entry_on_the_assessment() -> Result<()> {
    let mut platform = platform()?;
    let mut probe = probe();
    let first = platform.assess_sources(candidates()?, &mut probe, start())?;
    assert_eq!(first.registered(), 4, "premise: {:?}", first.decisions);
    assert!(
        first.catalogue_problems.is_empty(),
        "{:?}",
        first.catalogue_problems
    );

    assert_eq!(
        first.entries.len(),
        4,
        "one entry per registration: {:?}",
        first.entries
    );
    for entry in &first.entries {
        assert_eq!(entry.licence, "vendor-terms-2026", "{entry:?}");
        assert_eq!(
            entry.discovered_from,
            "a curated directory of exchange data vendors"
        );
        assert_eq!(entry.freshness, UpdateFrequency::Minutely);
        assert!(entry.composite.is_finite() && (0.0..=1.0).contains(&entry.composite));
    }
    let fading = first
        .entries
        .iter()
        .find(|e| e.source_id == "fading")
        .ok_or_else(|| qip_core::error::Error::not_found("fading"))?;
    assert_eq!(
        fading.monthly_fee,
        Decimal::from_int(DataFinder::COST_CEILING_PER_MONTH),
        "the cost is the registered source's own"
    );
    assert!(
        first.entries.iter().any(|e| e.monthly_fee == Decimal::ZERO),
        "the free sources read as free"
    );
    Ok(())
}

/// The fixture a thesis-queuing cycle needs: a priced, liquid instrument the
/// detectors can find a jump in. `LiquidityProfile` has no `Default` on
/// purpose — the controls that veto trading read exactly these two figures,
/// so a fixture states them rather than inheriting a number nobody wrote.
fn tradeable_universe(symbol: &str) -> Result<Universe> {
    use qip_financial::asset_class::{InstrumentType, Sector};
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            traded(symbol),
            symbol,
            InstrumentType::CommonStock,
            qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue("XNYS")
        .issuer("Acme Corporation")
        .sector(Sector::InformationTechnology)
        .price(Decimal::from_int(100))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
    Ok(universe)
}

fn traded(symbol: &str) -> qip_core::ids::ObjectId {
    qip_core::ids::ObjectId::from_string(format!("obj-{symbol}"))
}

/// A platform whose cycle reaches REASON's claim: a stated book, a
/// leverage-only limit set and the same review floor `valuation_seam`'s
/// suite states, so the fixture's own hypothesis is reviewed rather than
/// rejected for a thinness that is the fixture's and not the platform's.
/// The floor is a configured input, not a weakened control: the shipped
/// default is untouched and every other limit here is the conservative one.
fn platform_trading(symbol: &str) -> Result<Platform> {
    use qip_risk::limits::{Limit, LimitKind};
    let config = PlatformConfig {
        initial_equity: Decimal::from_int(1_000_000),
        review: qip_reasoning_engine::redteam::ReviewPolicy {
            minimum_surviving_confidence: 0.10,
            ..qip_reasoning_engine::redteam::ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    }
    .with_owner("market-data");
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        tradeable_universe(symbol)?,
        LimitSet::new("source-lifecycle-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        ),
    )
}

/// A price series with a jump two thirds of the way through, so the detectors
/// have a real displacement to find — the shape the kernel's own suites feed.
fn bars(symbol: &str, count: usize) -> Vec<qip_market_ingestion::adapter::SensedRecord> {
    use qip_financial::quality::DataQuality;
    use qip_market::bar::{Bar, Interval};
    use qip_market_ingestion::adapter::SensedRecord;
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            // The displacement is the last bar, so the thesis is about a
            // move the tape has not yet reverted.
            let jump = if i + 1 == count { -0.09 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            SensedRecord::Bar(Box::new(Bar {
                object_id: traded(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).expect("a price"),
                high: Decimal::from_f64(open.max(price) * 1.002).expect("a price"),
                low: Decimal::from_f64(open.min(price) * 0.998).expect("a price"),
                close: Decimal::from_f64(price).expect("a price"),
                volume: Decimal::from_int(1_000_000),
                trade_count: 5_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

#[test]
fn quarantining_a_source_withdraws_the_open_claims_that_stood_on_what_it_said() -> Result<()> {
    // R12-RES-7097, the half the world model cannot reach. Retracting a
    // poisoned source's facts narrows what is *read afterwards*. A claim the
    // platform had already made on the poisoned source's word stayed open,
    // was graded when its horizon passed, and the belief calibration learnt
    // the platform's confidence from evidence the platform had stopped
    // believing. A quarantine now withdraws it, and a withdrawn claim is
    // never scored.
    //
    // Mutation: delete the `prediction.withdrawn = Some(..)` loop in
    // `assess_sources` — the withdrawal assertion fails, naming the claim
    // still open on the quarantined source.
    let mut platform = platform_trading("AAA")?;
    let mut probe = probe();
    platform.assess_sources(candidates()?, &mut probe, start())?;
    // What an ingestion path does: a fact about the instrument, stated by
    // the source, absorbed through the kernel's own knowledge intake. The
    // graph must hold it *before* the claim is made, because attribution is
    // recorded where the claim is — by quarantine time the source's facts
    // are already retracted, and asking the graph then would name nobody.
    {
        use qip_world_model::relationship::{Relationship, RelationshipKind};
        let node = qip_world_model::graph::Node::entity(
            "company:Acme Corporation",
            qip_world_model::graph::EntityKind::Company,
            "Acme Corporation",
            start(),
        );
        let written = platform.absorb_knowledge(
            vec![node],
            vec![qip_kernel::evidence_intake::KnowledgeRelation {
                relationship: Relationship::new(
                    "company:Acme Corporation",
                    traded("AAA").as_str(),
                    RelationshipKind::Issues,
                    1.0,
                    "forger",
                ),
                valid_from: start(),
                confidence: 0.9,
            }],
            start(),
        )?;
        assert_eq!(
            written.relations, 1,
            "premise: the source's fact about the instrument was not written"
        );
    }
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());

    let open: Vec<_> = platform
        .predictions()
        .iter()
        .filter(|claim| claim.is_open())
        .cloned()
        .collect();
    assert!(
        !open.is_empty(),
        "premise: the cycle made no claim, so a withdrawal below would prove nothing:\n{}",
        report.summarise()
    );
    assert!(
        open.iter().any(|claim| claim.cites("forger")),
        "premise: no open claim names the source about to be quarantined: {:?}",
        open.iter().map(|c| c.sources.clone()).collect::<Vec<_>>()
    );
    // A clean source's claim must be left alone, or the assertion below
    // would pass for a pass that withdrew everything.
    assert!(
        open.iter().all(|claim| !claim.cites("rising")),
        "premise: the fixture's claims already name the clean source"
    );

    let assessment = platform.assess_sources(candidates()?, &mut probe, later())?;
    assert!(
        platform
            .registered_sources()
            .get("forger")
            .is_some_and(|entry| entry.is_quarantined()),
        "premise: policy quarantined forger on the second pass"
    );
    assert!(
        !assessment.claims_invalidated.is_empty(),
        "the assessment reports no withdrawn claim"
    );
    for claim in platform.predictions() {
        if claim.cites("forger") {
            assert!(
                claim.withdrawn.is_some(),
                "a claim standing on the quarantined source is still open: {claim:?}"
            );
        }
    }
    // Withdrawn, not deleted: what was claimed and why it was dropped is the
    // record a reader needs.
    assert!(
        platform.predictions().iter().any(|claim| claim
            .withdrawn
            .as_deref()
            .is_some_and(|why| { why.contains("`forger` quarantined") })),
        "the withdrawal names neither the source nor the reason"
    );
    Ok(())
}

#[test]
fn a_withdrawn_claim_is_never_scored_so_calibration_reads_no_poisoned_evidence() -> Result<()> {
    // The second half of the same rule: withdrawing a claim is only worth
    // something if the scoring pass honours it. Before this a withdrawn
    // claim would have been settled at its horizon like any other, and the
    // belief calibration — which is what sizes later theses — would have
    // learnt from the poisoned source after all.
    //
    // Mutation: drop the `prediction.withdrawn.is_some()` arm from
    // `score_predictions` — the withdrawn claim is scored and the count
    // assertion fails.
    use qip_kernel::platform::RecordedPrediction;
    let mut platform = platform_trading("AAA")?;
    let mut probe = probe();
    platform.assess_sources(candidates()?, &mut probe, start())?;
    {
        use qip_world_model::relationship::{Relationship, RelationshipKind};
        platform.absorb_knowledge(
            vec![qip_world_model::graph::Node::entity(
                "company:Acme Corporation",
                qip_world_model::graph::EntityKind::Company,
                "Acme Corporation",
                start(),
            )],
            vec![qip_kernel::evidence_intake::KnowledgeRelation {
                relationship: Relationship::new(
                    "company:Acme Corporation",
                    traded("AAA").as_str(),
                    RelationshipKind::Issues,
                    1.0,
                    "forger",
                ),
                valid_from: start(),
                confidence: 0.9,
            }],
            start(),
        )?;
    }
    platform.observe(bars("AAA", 120));
    platform.run_cycle(start());
    let before: Vec<RecordedPrediction> = platform
        .predictions()
        .iter()
        .filter(|claim| claim.is_open() && claim.cites("forger"))
        .cloned()
        .collect();
    assert!(
        !before.is_empty(),
        "premise: no open claim on the source, so nothing below is tested"
    );
    platform.assess_sources(candidates()?, &mut probe, later())?;

    // The premise the second half needs: a claim that did not stand on the
    // quarantined source, settled by the same observations, IS scored — so
    // the assertion below is about the withdrawal and not about a scoring
    // pass that settles nothing. A cycle after the quarantine makes one: the
    // source's facts are retracted by then, so nothing it said is cited.
    platform.run_cycle(later());
    let control = platform
        .predictions()
        .iter()
        .find(|claim| claim.is_open() && claim.withdrawn.is_none() && !claim.cites("forger"))
        .cloned();
    assert!(
        control.is_some(),
        "premise: the fixture has no clean claim to compare the withdrawn one against"
    );

    // Long past every horizon, with an observation for every metric the open
    // propositions name, so each one can actually settle.
    let resolved = later().saturating_add(Duration::from_days(400));
    let mut observations = Observations::at(resolved);
    for claim in platform.predictions() {
        for metric in claim.proposition.criteria.metrics() {
            observations = observations.with(metric, Observation::Numeric(Decimal::ZERO));
        }
    }
    let scored = platform.score_predictions(&observations, resolved);
    assert!(
        !scored.is_empty(),
        "premise: the scoring pass settled nothing, so a withdrawn claim being \
         unscored says nothing about the withdrawal"
    );
    if let Some(control) = control {
        assert!(
            scored
                .iter()
                .any(|(hypothesis, _)| hypothesis == &control.hypothesis),
            "premise: a claim that was not withdrawn was not scored either"
        );
    }
    for claim in &before {
        assert!(
            !scored
                .iter()
                .any(|(hypothesis, _)| hypothesis == &claim.hypothesis),
            "a withdrawn claim was scored: {}",
            claim.hypothesis
        );
    }
    Ok(())
}

/// Absorb one fact about `symbol` stated by `source`, the way an ingestion
/// path does, through the kernel's own knowledge intake. The graph must hold
/// it before a claim is made: attribution is recorded at the claim, because
/// a quarantine retracts the source's facts before anything can ask what
/// stood on them.
fn fact_about(platform: &mut Platform, source: &str, symbol: &str) -> Result<()> {
    use qip_world_model::relationship::{Relationship, RelationshipKind};
    let issuer = format!("company:{source}-issuer");
    let written = platform.absorb_knowledge(
        vec![qip_world_model::graph::Node::entity(
            &issuer,
            qip_world_model::graph::EntityKind::Company,
            &issuer,
            start(),
        )],
        vec![qip_kernel::evidence_intake::KnowledgeRelation {
            relationship: Relationship::new(
                &issuer,
                traded(symbol).as_str(),
                RelationshipKind::Issues,
                1.0,
                source,
            ),
            valid_from: start(),
            confidence: 0.9,
        }],
        start(),
    )?;
    assert_eq!(
        written.relations, 1,
        "premise: {source}'s fact about {symbol} was not written"
    );
    Ok(())
}

#[test]
fn a_claim_standing_on_a_source_retired_as_stale_is_held_at_the_stale_confidence() -> Result<()> {
    // R12-RES-7097, the stale half at the claim. A retirement is not a
    // finding that the evidence was false, so the claim stands — but a claim
    // the platform made on a feed that has since gone stale must not be
    // graded, and must not size anything, at the confidence a fresh feed
    // earned. Before this a retirement moved the source's facts in the graph
    // and left every claim already made at its original confidence.
    //
    // Mutation: delete the `claim.confidence *= ..` line in
    // `assess_sources` — the downweight assertion fails with the original
    // confidence.
    let mut platform = platform_trading("AAA")?;
    let mut probe = probe();
    platform.assess_sources(candidates()?, &mut probe, start())?;
    fact_about(&mut platform, "fading", "AAA")?;
    platform.observe(bars("AAA", 120));
    let report = platform.run_cycle(start());

    let before: Vec<(String, f64)> = platform
        .predictions()
        .iter()
        .filter(|claim| claim.is_open() && claim.cites("fading"))
        .filter_map(|claim| {
            claim
                .claim
                .as_ref()
                .map(|stated| (claim.hypothesis.clone(), stated.confidence))
        })
        .collect();
    assert!(
        !before.is_empty(),
        "premise: no open claim names the source about to be retired:\n{}",
        report.summarise()
    );
    assert!(
        before.iter().all(|(_, confidence)| *confidence > 0.0),
        "premise: the claims were stated at zero confidence, so a downweight \
         below would be unobservable: {before:?}"
    );

    let assessment = platform.assess_sources(candidates()?, &mut probe, later())?;
    assert!(
        !platform.registered_sources().contains_key("fading"),
        "premise: policy retired fading on the second pass"
    );
    assert_eq!(
        assessment.claims_downweighted,
        before
            .iter()
            .map(|(hypothesis, _)| hypothesis.clone())
            .collect::<Vec<_>>(),
        "the assessment does not report the claims it held less firmly"
    );
    for (hypothesis, was) in &before {
        let now_at = platform
            .predictions()
            .iter()
            .find(|claim| &claim.hypothesis == hypothesis)
            .and_then(|claim| claim.claim.as_ref())
            .map(|stated| stated.confidence)
            .ok_or_else(|| qip_core::error::Error::not_found(hypothesis.clone()))?;
        let expected = was * qip_world_model::world::STALE_SOURCE_CONFIDENCE;
        // The factor is a committed constant and both sides are the same
        // arithmetic on the same f64, so the comparison is exact by
        // construction rather than by luck; a tolerance here would admit a
        // downweight by some other factor.
        assert!(
            (now_at - expected).abs() < f64::EPSILON,
            "{hypothesis} is held at {now_at}, not at the stale fraction {expected} of its \
             confidence"
        );
        // Stale, not false: the claim is still open and still gradeable.
        assert!(
            platform
                .predictions()
                .iter()
                .any(|claim| &claim.hypothesis == hypothesis
                    && claim.is_open()
                    && claim.withdrawn.is_none()),
            "{hypothesis} was withdrawn by a retirement, which is not a finding \
             against the evidence"
        );
    }
    Ok(())
}
