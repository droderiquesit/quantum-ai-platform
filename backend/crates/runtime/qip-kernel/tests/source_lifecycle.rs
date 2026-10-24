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
        body: r#"{"symbol":"US0001","bid":10.25,"ask":10.27}"#.to_string(),
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
