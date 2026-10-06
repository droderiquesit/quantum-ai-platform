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
    let config = PlatformConfig::default();
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
