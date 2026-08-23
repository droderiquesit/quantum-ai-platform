//! The lifecycle end to end: what a decision carries, and what it hands on.
//!
//! Every registration decision has to be auditable six months later, when the
//! question is not "is this feed allowed" but "who decided it was, on what
//! evidence, and does that evidence still hold". A decision with no trail
//! cannot answer any of the three, so the type refuses to exist without one.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{AGENT, candidate, licensed_for, now, ok_head, probe_for, robots_served, sample};
use qip_contracts::governance::Usage;
use qip_core::Duration;
use qip_core::error::{Error, Result};
use qip_data_finder::decision::{DecisionOutcome, LifecycleStage, Reasoning, RegistrationDecision};
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, Delivery};
use qip_data_finder::finder::{DataFinder, FinderConfig};
use qip_data_finder::legal::RateLimit;
use qip_data_finder::probe::{InMemoryProbe, ProbeEvidence};
use qip_data_finder::source::Source;

const URL: &str = "https://example.com/data/prices.json";

fn polite_robots() -> qip_data_finder::probe::RobotsFetch {
    robots_served("User-agent: *\nAllow: /data/\nDisallow: /admin/\nCrawl-delay: 4\n")
}

fn registering_finder(seed: u64) -> Result<(DataFinder, InMemoryProbe)> {
    let config = FinderConfig::new(AGENT, Usage::Derive, "market-data", seed)?
        .with_default_rate_limit(RateLimit::new(60, Duration::from_mins(1))?);
    Ok((
        DataFinder::new(config),
        probe_for(URL, "example.com", polite_robots()),
    ))
}

#[test]
fn every_registration_decision_carries_the_reasoning_that_produced_it() -> Result<()> {
    let (mut finder, mut probe) = registering_finder(11)?;
    let decisions = finder.assess(
        vec![candidate(
            "audited",
            URL,
            licensed_for(&[Usage::Research, Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    let decision = &decisions[0];
    assert!(decision.is_registered());

    // Every stage the lifecycle ran is in the record, in order.
    for stage in [
        LifecycleStage::Discover,
        LifecycleStage::Classify,
        LifecycleStage::Probe,
        LifecycleStage::AssessLegality,
        LifecycleStage::Score,
        LifecycleStage::Route,
        LifecycleStage::Register,
    ] {
        assert!(
            decision.reasoning().reached(stage),
            "the record does not show the {} stage: {}",
            stage.as_str(),
            decision.reasoning().describe()
        );
    }

    // The trail is specific, not decorative.
    assert!(
        decision
            .reasoning()
            .at(LifecycleStage::Discover)
            .iter()
            .any(|finding| finding.contains("curated directory"))
    );
    assert_eq!(decision.reasoning().at(LifecycleStage::Score).len(), 5);
    assert!(decision.legality().is_some());
    assert!(decision.scores().is_some());
    assert!(decision.lineage().is_some());
    Ok(())
}

#[test]
fn a_decision_with_no_recorded_reason_cannot_be_constructed() -> Result<()> {
    let error = RegistrationDecision::new(
        "unaudited",
        DecisionOutcome::Rejected {
            reason: "because".to_string(),
        },
        Reasoning::new(),
        now(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::Invalid(_)));
    assert!(error.message().contains("nobody can audit"));
    Ok(())
}

#[test]
fn the_emitted_policy_respects_the_crawl_delay_and_the_rate_limit_together() -> Result<()> {
    // robots.txt asks for four seconds; the configured budget allows one per
    // second. Obeying the looser of two ceilings breaches the tighter one.
    let (mut finder, mut probe) = registering_finder(11)?;
    let decisions = finder.assess(
        vec![candidate(
            "polite",
            URL,
            licensed_for(&[Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    let policy = decisions[0]
        .policy()
        .ok_or_else(|| Error::not_found("an emitted policy"))?;

    assert_eq!(policy.crawl_delay(), Some(Duration::from_secs(4)));
    assert_eq!(
        policy
            .declared_rate_limit()
            .map(|limit| limit.min_interval()),
        Some(Duration::from_secs(1))
    );
    assert_eq!(policy.min_request_interval(), Duration::from_secs(4));
    assert_eq!(policy.user_agent(), AGENT);
    assert_eq!(policy.disallowed_paths(), ["/admin/".to_string()]);
    assert_eq!(
        policy.earliest_next_request(now()),
        now().saturating_add(Duration::from_secs(4))
    );
    assert_eq!(policy.permitted_requests_over(Duration::from_mins(1)), 15);
    Ok(())
}

#[test]
fn a_publisher_that_states_nothing_still_gets_a_floor_between_requests() -> Result<()> {
    let config = FinderConfig::new(AGENT, Usage::Derive, "market-data", 11)?;
    let mut finder = DataFinder::new(config);
    let mut probe = probe_for(URL, "example.com", common::permissive_robots());

    let decisions = finder.assess(
        vec![candidate(
            "unstated",
            URL,
            licensed_for(&[Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    let policy = decisions[0]
        .policy()
        .ok_or_else(|| Error::not_found("an emitted policy"))?;
    assert!(policy.crawl_delay().is_none());
    assert_eq!(
        policy.min_request_interval(),
        qip_data_finder::legal::SourcePolicy::DEFAULT_MIN_INTERVAL
    );
    Ok(())
}

#[test]
fn a_registered_source_becomes_something_the_pull_adapter_contract_can_poll() -> Result<()> {
    let (mut finder, mut probe) = registering_finder(11)?;
    finder.assess(
        vec![candidate(
            "ingestible",
            URL,
            licensed_for(&[Usage::Research, Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    let registered = finder
        .registered("ingestible")
        .ok_or_else(|| Error::not_found("the registration"))?;

    let plan = qip_data_finder::plan_for(registered);
    assert_eq!(plan.delivery, Delivery::Pull);
    assert!(!plan.requires_buffering());
    assert!(plan.incremental, "the REST mechanism declares a cursor");
    // Polling faster than the policy would breach it, so the policy wins.
    assert!(plan.min_poll_interval >= Duration::from_secs(4));
    assert_eq!(
        plan.schema_fingerprint,
        registered.source().schema().fingerprint()
    );

    let descriptor = qip_data_finder::descriptor_for(registered);
    assert_eq!(descriptor.name, "ingestible");
    assert_eq!(descriptor.provider, "Example Data Ltd");
    assert!(descriptor.is_production_grade());
    assert_eq!(descriptor.topics, vec![qip_events::Topic::MarketQuote]);
    assert_eq!(descriptor.expected_latency, Duration::from_millis(55));
    Ok(())
}

#[test]
fn a_push_mechanism_is_marked_as_needing_a_buffer_behind_poll() -> Result<()> {
    // `poll(until)` leaves the clock with the caller. An adapter over a
    // websocket that did not buffer would block or drop, and neither shows up
    // until a replay disagrees with the live run.
    let socket = AccessMechanism::WebSocket {
        auth: AuthRequirement::ApiKey {
            header: "X-Api-Key".to_string(),
        },
        subscribe_frame: r#"{"op":"subscribe","channel":"quotes"}"#.to_string(),
        heartbeat_interval: Duration::from_secs(15),
    };
    let plan = socket.poll_plan();
    assert_eq!(plan.delivery, Delivery::PushBuffered);
    assert!(
        plan.credential_required
            .as_deref()
            .is_some_and(|need| need.contains("X-Api-Key"))
    );

    let multicast = AccessMechanism::StreamingMulticast {
        group: "233.54.12.1".to_string(),
        port: 26_477,
        wire_protocol: "itch".to_string(),
    };
    assert_eq!(multicast.poll_plan().delivery, Delivery::PushBuffered);
    Ok(())
}

#[test]
fn a_probed_source_and_an_unprobed_candidate_are_different_types() -> Result<()> {
    // A `Source` cannot be built without evidence, so a function that must
    // not run on hearsay takes a `&Source` and the compiler does the rest.
    let candidate = candidate("typed", URL, licensed_for(&[Usage::Derive])?, &["EU0001"])?;
    let mut probe = InMemoryProbe::new()
        .with_robots("example.com", common::permissive_robots())
        .with_head(URL, ok_head())
        .with_sample(URL, sample(common::QUOTE_PAYLOAD));

    let evidence = ProbeEvidence::gather(&mut probe, candidate.endpoint(), now())?;
    let source = Source::from_evidence(candidate.clone(), evidence);

    // The candidate's coverage is a claim; the source's schema is observed.
    assert_eq!(candidate.declared_coverage(), source.coverage());
    assert!(!source.schema().is_empty());
    assert_eq!(source.probed_at(), now());
    Ok(())
}

#[test]
fn a_registration_produces_the_mesh_catalogue_entry_rather_than_a_second_catalogue() -> Result<()> {
    let (mut finder, mut probe) = registering_finder(11)?;
    let decisions = finder.assess(
        vec![candidate(
            "catalogued",
            URL,
            licensed_for(&[Usage::Research, Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;

    let entry = decisions[0].catalogue_entry("market-data")?;
    assert_eq!(entry.dataset, "source.catalogued");
    assert_eq!(entry.owner, "market-data");
    assert!(entry.permits(Usage::Derive, now()));
    assert!(!entry.permits(Usage::Redistribute, now()));

    // It registers into the mesh's own catalogue type without modification.
    let mut catalog = qip_mesh::catalog::Catalog::new();
    catalog.register(entry)?;
    catalog.usable_for("source.catalogued", Usage::Derive, now())?;
    assert!(
        catalog
            .usable_for("source.catalogued", Usage::Trade, now())
            .is_err()
    );
    Ok(())
}

#[test]
fn a_rejected_source_has_no_catalogue_entry_to_offer() -> Result<()> {
    let (mut finder, mut probe) = registering_finder(11)?;
    let decisions = finder.assess(
        vec![candidate(
            "refused",
            URL,
            licensed_for(&[Usage::Research])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    assert!(!decisions[0].is_registered());

    let error = decisions[0].catalogue_entry("market-data").unwrap_err();
    assert!(matches!(error, Error::Denied(_)));
    assert!(error.message().contains("rejected"));
    Ok(())
}

#[test]
fn the_whole_lifecycle_is_reproducible_from_the_record_it_leaves() -> Result<()> {
    // Serialise a decision and read it back: the record is the artefact, and
    // a record that cannot round-trip is one that will not survive being
    // written to the evidence store.
    let (mut finder, mut probe) = registering_finder(11)?;
    let decisions = finder.assess(
        vec![candidate(
            "durable",
            URL,
            licensed_for(&[Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;

    let encoded = serde_json::to_string(&decisions[0])?;
    let decoded: RegistrationDecision = serde_json::from_str(&encoded)?;
    assert_eq!(decoded, decisions[0]);
    assert!(!decoded.reasoning().is_empty());
    Ok(())
}
