//! A scout pass records what it found about every candidate (R12-DATA-1102,
//! R12-DATA-1103; ADR 0626).

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::governance::Usage;
use qip_core::Context;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal};
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
use qip_data_finder::legal::Legality;
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_data_finder::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_events::{EventFilter, Topic};
use qip_financial::asset_class::AssetClass;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_kernel::scout_record::ScoutChecks;
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
        body: qip_core::untrusted::Untrusted::new(
            "a scripted sample",
            r#"{"symbol":"US0001","bid":10.25,"ask":10.27}"#.to_string(),
        ),
        media_type: "application/json".to_string(),
        payload_at: Some(at),
        latency: Duration::from_millis(55),
    }
}

const GOOD: &str = "https://good.example/quotes";
const BARRED: &str = "https://barred.example/quotes";

fn probe() -> InMemoryProbe {
    let mut probe = InMemoryProbe::new();
    for (host, url, robots) in [
        ("good.example", GOOD, "User-agent: *\nAllow: /\n"),
        ("barred.example", BARRED, "User-agent: *\nDisallow: /\n"),
    ] {
        probe = probe
            .with_robots(
                host,
                RobotsFetch::Served {
                    body: robots.to_string(),
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
    for _ in 0..4 {
        probe = probe
            .with_sample(GOOD, payload_at(start()))
            .with_sample(BARRED, payload_at(start()));
    }
    probe
}

fn candidates() -> Result<Vec<SourceCandidate>> {
    Ok(vec![
        source("good", GOOD, "US0001", 0, 3_650)?,
        source("barred", BARRED, "US0002", 0, 0)?,
    ])
}

/// The failure prevented: a first-seen source refused for its publisher's
/// robots rules moved no registration, so `SourceLifecycleChanged` — the only
/// record `assess_sources` wrote — held nothing, and nothing durable said it
/// had been looked at or why it was turned away.
///
/// The premises first: the barred source never registers and the lifecycle
/// topic is empty, so whatever the scout topic holds is the new record and
/// not the old one. The permitted source in the same pass keeps the
/// assertions from passing on a recorder that marks everything refused.
///
/// Mutated by deleting the `journal_scout_verdicts` call in
/// `assess_sources_scouted`: `scout_passes()` is empty and the first
/// assertion on it fails; restored.
#[test]
fn a_first_pass_robots_refusal_and_a_novelty_score_are_in_the_event_log() -> Result<()> {
    let mut platform = platform()?;
    let mut probe = probe();
    platform.assess_sources(candidates()?, &mut probe, start())?;

    assert!(
        platform.registered_sources().get("barred").is_none(),
        "premise: the barred source was never registered"
    );
    assert!(
        platform.registered_sources().get("good").is_some(),
        "premise: the permitted source registered"
    );
    assert!(
        platform
            .replay_journal(&EventFilter::new().topic(Topic::SourceLifecycleChanged))?
            .is_empty(),
        "premise: no lifecycle record exists, so the old record says nothing"
    );

    let passes = platform.scout_passes()?;
    assert_eq!(passes.len(), 1, "the pass is not on the log: {passes:?}");
    let by_id = |id: &str| {
        passes[0]
            .verdicts
            .iter()
            .find(|verdict| verdict.source_id == id)
            .cloned()
    };
    let barred = by_id("barred").expect("the refused candidate has a verdict");
    assert!(barred.first_pass);
    let access = barred.access.expect("the access verdict was recorded");
    assert!(!access.permitted);
    assert!(
        matches!(access.robots, Legality::Forbidden { .. }),
        "the robots refusal is not the recorded reason: {access:?}"
    );
    let good = by_id("good").expect("the permitted candidate has a verdict");
    assert!(good.access.expect("access").permitted);
    let novelty = good.novelty.expect("novelty was scored and recorded");
    assert!((0.0..=1.0).contains(&novelty), "{novelty}");

    // A second look at a source the registry now holds is not a first pass.
    platform.assess_sources(
        vec![source("good", GOOD, "US0001", 0, 3_650)?],
        &mut probe,
        later(),
    )?;
    let passes = platform.scout_passes()?;
    assert_eq!(passes.len(), 2);
    assert!(!passes[1].verdicts[0].first_pass);
    Ok(())
}

/// Disabling a check changes what is recorded, which is the row's own
/// requirement: the same pass with the access check off records no verdict
/// at all, not a default "permitted", and with novelty off records no score.
///
/// Mutated by ignoring `checks.access_rights` in `journal_scout_verdicts`
/// (always recording): the disabled pass's `access` is `Some` and the
/// assertion fails; restored.
#[test]
fn disabling_a_scout_check_removes_its_finding_from_the_record() -> Result<()> {
    let mut platform = platform()?;
    let mut probe = probe();
    platform.assess_sources_scouted(
        candidates()?,
        &mut probe,
        start(),
        ScoutChecks {
            novelty: false,
            access_rights: false,
        },
    )?;
    let passes = platform.scout_passes()?;
    assert_eq!(passes.len(), 1);
    assert!(!passes[0].checks_access_rights && !passes[0].checks_novelty);
    assert_eq!(
        passes[0].verdicts.len(),
        2,
        "premise: both candidates recorded"
    );
    for verdict in &passes[0].verdicts {
        assert!(verdict.access.is_none(), "{verdict:?}");
        assert!(verdict.novelty.is_none(), "{verdict:?}");
    }
    Ok(())
}
