//! Replacing a source that died, and refusing to pretend when nothing can.
//!
//! The failure mode under test is a silent downgrade: a feed stops, the search
//! returns the least-bad thing available, and the platform carries on with two
//! thirds of the instruments at a quarter of the frequency while every
//! downstream number stays plausible.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{AGENT, candidate, licensed_for, now, ok_head, permissive_robots, sample};
use qip_contracts::governance::Usage;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::finder::{DataFinder, FinderConfig, MonitorOutcome};
use qip_data_finder::health::HealthObservation;
use qip_data_finder::probe::{InMemoryProbe, ProbeEvidence};
use qip_data_finder::replacement::ReplacementOutcome;
use qip_data_finder::source::{Source, SourceCandidate};
use qip_financial::asset_class::AssetClass;

const PRIMARY_URL: &str = "https://primary.example/data/prices.json";

fn probe_for_hosts(urls: &[(&str, &str)]) -> InMemoryProbe {
    let mut probe = InMemoryProbe::new();
    for (host, url) in urls {
        probe = probe
            .with_robots(host, permissive_robots())
            .with_head(url, ok_head())
            .with_sample(url, sample(common::QUOTE_PAYLOAD));
    }
    probe
}

/// A probed source built outside the registry, for the replacement pool.
fn probed(candidate: SourceCandidate, probe: &mut InMemoryProbe) -> Result<Source> {
    let evidence = ProbeEvidence::gather(probe, candidate.endpoint(), now())?;
    Ok(Source::from_evidence(candidate, evidence))
}

fn covering(
    instruments: &[&str],
    regions: &[SourceRegion],
    frequency: UpdateFrequency,
) -> Result<SourceCoverage> {
    Ok(SourceCoverage::new(
        [AssetClass::Equity],
        regions.iter().copied(),
        instruments.iter().map(|i| (*i).to_string()),
        frequency,
    )?
    .with_history_from(now().saturating_sub(Duration::from_days(1_000))))
}

fn registered_primary(finder: &mut DataFinder, probe: &mut InMemoryProbe) -> Result<()> {
    let decisions = finder.assess(
        vec![candidate(
            "primary",
            PRIMARY_URL,
            licensed_for(&[Usage::Derive])?,
            &["EU0001", "EU0002", "EU0003"],
        )?],
        probe,
        now(),
    )?;
    if !decisions[0].is_registered() {
        return Err(Error::invalid(format!(
            "the fixture source was not registered: {}",
            decisions[0].reasoning().describe()
        )));
    }
    Ok(())
}

fn dead_observations(until: Timestamp) -> Vec<HealthObservation> {
    (1..=4)
        .map(|step| HealthObservation::timed_out(until.saturating_sub(Duration::from_mins(5 - step))))
        .collect()
}

#[test]
fn a_dead_source_is_reported_dead_and_triggers_a_replacement_search() -> Result<()> {
    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 5)?);
    let mut probe = probe_for_hosts(&[("primary.example", PRIMARY_URL)]);
    registered_primary(&mut finder, &mut probe)?;

    let later = now().saturating_add(Duration::from_mins(30));
    let outcome = finder.monitor(
        "primary",
        &dead_observations(later),
        None,
        later,
        Duration::from_hours(1),
    )?;

    let MonitorOutcome::Dead { health, reason } = &outcome else {
        return Err(Error::invalid(format!(
            "an all-timeout window must report death, not {}",
            outcome.as_str()
        )));
    };
    assert!(outcome.needs_replacement());
    assert!(health.availability() < 1e-9);
    assert!(reason.contains("served nothing"));
    Ok(())
}

#[test]
fn a_replacement_covering_the_same_ground_is_found_and_ranked() -> Result<()> {
    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 5)?);
    let mut probe = probe_for_hosts(&[
        ("primary.example", PRIMARY_URL),
        ("full.example", "https://full.example/data/prices.json"),
        ("wider.example", "https://wider.example/data/prices.json"),
        ("thin.example", "https://thin.example/data/prices.json"),
    ]);
    registered_primary(&mut finder, &mut probe)?;

    let full = probed(
        candidate(
            "full",
            "https://full.example/data/prices.json",
            licensed_for(&[Usage::Derive])?,
            &["EU0001", "EU0002", "EU0003"],
        )?,
        &mut probe,
    )?;
    let wider = probed(
        candidate(
            "wider",
            "https://wider.example/data/prices.json",
            licensed_for(&[Usage::Derive])?,
            &["EU0001", "EU0002", "EU0003", "EU0004"],
        )?,
        &mut probe,
    )?;
    let thin = probed(
        candidate(
            "thin",
            "https://thin.example/data/prices.json",
            licensed_for(&[Usage::Derive])?,
            &["EU0001"],
        )?,
        &mut probe,
    )?;

    let outcome = finder.find_replacement("primary", &[thin, full, wider], now())?;
    let ReplacementOutcome::Found { ranked } = &outcome else {
        return Err(Error::invalid(format!(
            "a full replacement exists: {}",
            outcome.describe()
        )));
    };
    assert_eq!(ranked.len(), 2, "only complete covers are replacements");
    let ids: Vec<&str> = ranked.iter().map(|entry| entry.source_id()).collect();
    assert!(ids.contains(&"full") && ids.contains(&"wider"));
    assert!(!ids.contains(&"thin"));
    Ok(())
}

#[test]
fn when_nothing_covers_the_source_the_search_says_so_and_names_the_gap() -> Result<()> {
    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 5)?);
    let mut probe = probe_for_hosts(&[
        ("primary.example", PRIMARY_URL),
        ("partial.example", "https://partial.example/data/prices.json"),
    ]);
    registered_primary(&mut finder, &mut probe)?;

    let partial = probed(
        candidate(
            "partial",
            "https://partial.example/data/prices.json",
            licensed_for(&[Usage::Derive])?,
            &["EU0001", "EU0002"],
        )?,
        &mut probe,
    )?;

    let outcome = finder.find_replacement("primary", &[partial], now())?;
    let ReplacementOutcome::NotFound {
        considered,
        uncovered,
        closest,
    } = &outcome
    else {
        return Err(Error::invalid(
            "a candidate missing an instrument is not a replacement",
        ));
    };

    assert_eq!(*considered, 1);
    assert!(uncovered.instruments.contains("EU0003"));
    assert!(!uncovered.instruments.contains("EU0001"));
    assert_eq!(closest.len(), 1);
    assert!(closest[0].describe().contains("EU0003"));

    // There is no accessor that hands a partial back as a replacement.
    assert!(outcome.best().is_none());
    assert!(outcome.describe().contains("none of the 1 candidate"));
    Ok(())
}

#[test]
fn an_empty_pool_reports_the_whole_coverage_as_uncovered() -> Result<()> {
    // Reporting an empty gap here would read as "nothing is missing", which
    // is the opposite of what an empty search means.
    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 5)?);
    let mut probe = probe_for_hosts(&[("primary.example", PRIMARY_URL)]);
    registered_primary(&mut finder, &mut probe)?;

    let outcome = finder.find_replacement("primary", &[], now())?;
    let ReplacementOutcome::NotFound {
        considered,
        uncovered,
        closest,
    } = &outcome
    else {
        return Err(Error::invalid("an empty pool cannot replace anything"));
    };
    assert_eq!(*considered, 0);
    assert!(closest.is_empty());
    assert_eq!(uncovered.instruments.len(), 3);
    assert!(uncovered.regions.contains(&SourceRegion::Europe));
    Ok(())
}

#[test]
fn a_candidate_publishing_too_slowly_is_not_a_replacement() -> Result<()> {
    // Right instruments, right region, a day late. The gap names the axis.
    let required = covering(
        &["EU0001"],
        &[SourceRegion::Europe],
        UpdateFrequency::Minutely,
    )?;
    let slow = covering(&["EU0001"], &[SourceRegion::Europe], UpdateFrequency::Daily)?;

    let against = slow.against(&required);
    assert!(!against.is_complete());
    assert!(!against.frequency_sufficient);
    assert_eq!(
        against.gap.frequency_shortfall,
        Some((UpdateFrequency::Daily, UpdateFrequency::Minutely))
    );
    assert!(against.gap.describe().contains("update frequency"));
    Ok(())
}

#[test]
fn a_source_on_the_wrong_continent_is_not_a_replacement_however_it_scores() -> Result<()> {
    let required = covering(
        &["EU0001"],
        &[SourceRegion::Europe],
        UpdateFrequency::Minutely,
    )?;
    let elsewhere = covering(
        &["EU0001"],
        &[SourceRegion::Apac],
        UpdateFrequency::Streaming,
    )?;
    assert!(!elsewhere.against(&required).is_complete());

    // A global source serves any region.
    let global = covering(
        &["EU0001"],
        &[SourceRegion::Global],
        UpdateFrequency::Streaming,
    )?;
    assert!(global.against(&required).is_complete());
    Ok(())
}

#[test]
fn replacing_a_source_that_was_never_registered_is_an_error() -> Result<()> {
    let finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 5)?);
    let error = finder
        .find_replacement("never-existed", &[], now())
        .unwrap_err();
    assert!(matches!(error, Error::NotFound(_)));
    Ok(())
}
