//! What a later pass does to a source an earlier pass registered (DATA-016,
//! DATA-018, DATA-021).
//!
//! Every test here runs the finder at least twice over the same candidate,
//! because the defect these guard was invisible in one pass: a rejection of a
//! candidate wrote nothing, so a source whose publisher had since forbidden
//! the path stayed registered, and a source re-assessed while registered was
//! scored as redundant with itself.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{AGENT, QUOTE_PAYLOAD, licensed_for, now, ok_head, permissive_robots, robots_served};
use qip_contracts::governance::Usage;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Duration, Timestamp};
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::decision::LifecycleStage;
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
use qip_data_finder::finder::{DataFinder, FinderConfig};
use qip_data_finder::lifecycle::LifecycleAction;
use qip_data_finder::probe::{InMemoryProbe, PayloadSample};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::scoring::RoutingClass;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_data_finder::{DecisionOutcome, RegistrationDecision};
use qip_events::Topic;
use qip_financial::asset_class::AssetClass;

fn finder() -> Result<DataFinder> {
    Ok(DataFinder::new(FinderConfig::new(
        AGENT,
        Usage::Derive,
        "market-data",
        3,
    )?))
}

/// A candidate with no declared history, so the historical-value term is zero
/// and the composite is the sum a test can state: reliability 0.27 over TLS
/// (0.18 over plaintext), freshness 0.20, uniqueness 0.20, cost 0.15 when
/// free and 0 at the monthly ceiling.
fn source(id: &str, url: &str, instrument: &str, monthly: i64) -> Result<SourceCandidate> {
    source_with_history(id, url, instrument, monthly, 0)
}

/// As [`source`], declaring `history_days` of history: ten years saturates
/// the historical-value term at 0.15.
fn source_with_history(
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
            [SourceRegion::Europe],
            [instrument.to_string()],
            UpdateFrequency::Minutely,
        )?
        .with_history_from(now().saturating_sub(Duration::from_days(history_days))),
        licensed_for(&[Usage::Derive])?,
        SourceCost::new(
            Decimal::from_int(monthly),
            Decimal::ZERO,
            u64::MAX,
            Currency::EUR,
        )?,
        SourceRegion::Europe,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        now(),
    )
}

fn payload_at(at: Timestamp) -> PayloadSample {
    PayloadSample {
        body: QUOTE_PAYLOAD.to_string(),
        media_type: "application/json".to_string(),
        payload_at: Some(at),
        latency: Duration::from_millis(55),
    }
}

fn later() -> Timestamp {
    now().saturating_add(Duration::from_hours(1))
}

/// Ten minutes old against a one-minute cadence: freshness zero.
fn stale(at: Timestamp) -> PayloadSample {
    payload_at(at.saturating_sub(Duration::from_mins(10)))
}

fn decision_for<'a>(
    decisions: &'a [RegistrationDecision],
    id: &str,
) -> Result<&'a RegistrationDecision> {
    decisions
        .iter()
        .find(|decision| decision.source_id() == id)
        .ok_or_else(|| Error::not_found(format!("no decision for `{id}`")))
}

fn class_of(finder: &DataFinder, id: &str) -> Result<RoutingClass> {
    Ok(finder
        .registered(id)
        .ok_or_else(|| Error::not_found(format!("`{id}` is not registered")))?
        .routing()
        .class())
}

#[test]
fn a_registered_source_reassessed_on_unchanged_evidence_scores_the_same_and_moves_nothing()
-> Result<()> {
    // A source re-assessed while registered overlaps its own registration
    // completely. Counting that as redundancy took a fifth of the composite
    // off every standing source on its second pass, for being itself.
    const URL: &str = "https://steady.example/quotes";
    let mut finder = finder()?;
    let mut probe = InMemoryProbe::new()
        .with_robots("steady.example", permissive_robots())
        .with_head(URL, ok_head())
        .with_sample(URL, payload_at(now()));
    let candidates =
        || -> Result<Vec<SourceCandidate>> { Ok(vec![source("steady", URL, "EU0001", 0)?]) };

    let first = finder.assess(candidates()?, &mut probe, now())?;
    let before = first[0]
        .scores()
        .ok_or_else(|| Error::not_found("the first pass scored nothing"))?
        .composite();
    assert!(first[0].is_registered(), "the premise: it registers");
    assert!(first[0].transition().is_none());

    let second = finder.assess(candidates()?, &mut probe, now())?;
    let after = second[0]
        .scores()
        .ok_or_else(|| Error::not_found("the second pass scored nothing"))?
        .composite();
    assert!(
        (after - before).abs() < 1e-9,
        "unchanged evidence scored {before:.3} and then {after:.3}"
    );
    assert!(
        second[0].transition().is_none(),
        "nothing changed and the source moved: {:?}",
        second[0].transition()
    );
    Ok(())
}

#[test]
fn three_registered_sources_are_retired_quarantined_and_promoted_by_policy_alone() -> Result<()> {
    // DATA-018's own fixture. `rival` only exists so `fading` is redundant,
    // and declares ten years of history so that being overlapped in turn
    // (0.97 falling to 0.77) leaves it hot: two sources covering the same
    // thing are each redundant with the other, and this test is not about
    // that.
    const RIVAL: &str = "https://rival.example/quotes";
    const FADING: &str = "http://fading.example/quotes";
    const FORGER: &str = "https://forger.example/quotes";
    const RISING: &str = "https://rising.example/quotes";
    let mut finder = finder()?;
    let mut probe = InMemoryProbe::new();
    for (host, url) in [
        ("rival.example", RIVAL),
        ("fading.example", FADING),
        ("forger.example", FORGER),
        ("rising.example", RISING),
    ] {
        probe = probe
            .with_robots(host, permissive_robots())
            .with_head(url, ok_head());
    }
    let mut probe = probe
        .with_sample(RIVAL, payload_at(now()))
        .with_sample(RIVAL, payload_at(later()))
        // Plaintext, at the cost ceiling, redundant with `rival`: fresh it is
        // 0.18 + 0.20 = 0.38 (cold); stale it is 0.18, under the 0.25 floor.
        .with_sample(FADING, payload_at(now()))
        .with_sample(FADING, stale(later()))
        // Healthy, then dating its records a day after they are fetched.
        .with_sample(FORGER, payload_at(now()))
        .with_sample(
            FORGER,
            payload_at(later().saturating_add(Duration::from_days(1))),
        )
        // Stale it is 0.27 + 0.20 + 0.15 = 0.62 (warm); fresh it is 0.82.
        .with_sample(RISING, stale(now()))
        .with_sample(RISING, payload_at(later()));
    let candidates = || -> Result<Vec<SourceCandidate>> {
        Ok(vec![
            source_with_history("a-rival", RIVAL, "EU0001", 0, 3_650)?,
            source(
                "fading",
                FADING,
                "EU0001",
                DataFinder::COST_CEILING_PER_MONTH,
            )?,
            source("forger", FORGER, "EU0002", 0)?,
            source("rising", RISING, "EU0003", 0)?,
        ])
    };

    let first = finder.assess(candidates()?, &mut probe, now())?;
    // The premise: all three are registered, in the classes the arithmetic
    // above says, and no first assessment is a transition.
    assert_eq!(class_of(&finder, "fading")?, RoutingClass::Cold);
    assert_eq!(class_of(&finder, "forger")?, RoutingClass::Hot);
    assert_eq!(class_of(&finder, "rising")?, RoutingClass::Warm);
    assert!(first.iter().all(|decision| decision.transition().is_none()));

    let second = finder.assess(candidates()?, &mut probe, later())?;

    let retired = decision_for(&second, "fading")?
        .transition()
        .ok_or_else(|| Error::not_found("`fading` made no transition"))?;
    assert_eq!(retired.action, LifecycleAction::Retired);
    assert_eq!(retired.from, RoutingClass::Cold);
    assert!(
        finder.registered("fading").is_none(),
        "a retired source is still registered"
    );

    let quarantined = decision_for(&second, "forger")?
        .transition()
        .ok_or_else(|| Error::not_found("`forger` made no transition"))?;
    assert_eq!(quarantined.action, LifecycleAction::Quarantined);
    assert!(
        quarantined.reason.contains("manipulation risk flagged"),
        "quarantined for something other than manipulation: {}",
        quarantined.reason
    );
    assert!(
        finder
            .registered("forger")
            .is_some_and(|entry| entry.is_quarantined()),
        "a quarantined source keeps its record, marked"
    );

    let promoted = decision_for(&second, "rising")?
        .transition()
        .ok_or_else(|| Error::not_found("`rising` made no transition"))?;
    assert_eq!(promoted.action, LifecycleAction::Promoted);
    assert_eq!(
        (promoted.from, promoted.to),
        (RoutingClass::Warm, RoutingClass::Hot)
    );
    assert_eq!(class_of(&finder, "rising")?, RoutingClass::Hot);

    assert!(
        decision_for(&second, "a-rival")?.transition().is_none(),
        "the rival did not change and moved anyway"
    );
    Ok(())
}

#[test]
fn a_source_that_goes_stale_is_throttled_to_a_slower_class_and_stays_registered() -> Result<()> {
    const URL: &str = "https://slowing.example/quotes";
    let mut finder = finder()?;
    let mut probe = InMemoryProbe::new()
        .with_robots("slowing.example", permissive_robots())
        .with_head(URL, ok_head())
        .with_sample(URL, payload_at(now()))
        .with_sample(URL, stale(later()));
    let candidates =
        || -> Result<Vec<SourceCandidate>> { Ok(vec![source("slowing", URL, "EU0001", 0)?]) };

    finder.assess(candidates()?, &mut probe, now())?;
    assert_eq!(class_of(&finder, "slowing")?, RoutingClass::Hot);

    let second = finder.assess(candidates()?, &mut probe, later())?;
    let transition = second[0]
        .transition()
        .ok_or_else(|| Error::not_found("a stale source made no transition"))?;
    assert_eq!(transition.action, LifecycleAction::Throttled);
    assert_eq!(
        (transition.from, transition.to),
        (RoutingClass::Hot, RoutingClass::Warm)
    );
    assert_eq!(class_of(&finder, "slowing")?, RoutingClass::Warm);
    Ok(())
}

#[test]
fn a_source_whose_robots_txt_turns_against_the_path_is_quarantined_and_never_contacted_again()
-> Result<()> {
    // DATA-021. The publisher's terms were already re-read every pass; the
    // "no" was thrown away and the registration stood.
    const URL: &str = "https://withdrawn.example/quotes";
    let mut finder = finder()?;
    let mut probe = InMemoryProbe::new()
        .with_robots("withdrawn.example", permissive_robots())
        .with_robots(
            "withdrawn.example",
            robots_served("User-agent: *\nDisallow: /\n"),
        )
        .with_head(URL, ok_head())
        .with_sample(URL, payload_at(now()))
        .with_sample(URL, payload_at(later()));
    let candidates =
        || -> Result<Vec<SourceCandidate>> { Ok(vec![source("withdrawn", URL, "EU0001", 0)?]) };

    let first = finder.assess(candidates()?, &mut probe, now())?;
    assert!(
        first[0].is_registered(),
        "the premise: adopted while permitted"
    );

    let second = finder.assess(candidates()?, &mut probe, later())?;
    let transition = second[0]
        .transition()
        .ok_or_else(|| Error::not_found("forbidden terms moved nothing"))?;
    assert_eq!(transition.action, LifecycleAction::Quarantined);
    assert!(
        transition.reason.contains("robots.txt"),
        "the transition does not name the terms that changed: {}",
        transition.reason
    );
    let entry = finder
        .registered("withdrawn")
        .ok_or_else(|| Error::not_found("the quarantined registration was dropped"))?;
    assert!(entry.is_quarantined());

    // No further fetch: a third pass answers without touching the probe.
    let calls_before = probe.calls().len();
    assert!(calls_before > 0, "the premise: the probe was used at all");
    let third = finder.assess(
        candidates()?,
        &mut probe,
        later().saturating_add(Duration::from_hours(1)),
    )?;
    assert_eq!(
        probe.calls().len(),
        calls_before,
        "a quarantined source was contacted again: {:?}",
        &probe.calls()[calls_before..]
    );
    assert!(
        matches!(third[0].outcome(), DecisionOutcome::Rejected { reason } if reason.starts_with("quarantined:")),
        "the third pass did not answer from the quarantine: {:?}",
        third[0].outcome()
    );
    assert!(third[0].transition().is_none(), "quarantined twice");
    Ok(())
}

#[test]
fn a_source_that_starts_turning_the_probe_away_is_quarantined_naming_the_refusal() -> Result<()> {
    // Access, the other half of DATA-021: the publisher has not changed a
    // word of its terms, it has put the endpoint behind a login.
    const URL: &str = "https://gated.example/quotes";
    let mut finder = finder()?;
    let mut refused = ok_head();
    refused.status = 403;
    let mut probe = InMemoryProbe::new()
        .with_robots("gated.example", permissive_robots())
        .with_head(URL, ok_head())
        .with_head(URL, refused)
        .with_sample(URL, payload_at(now()))
        .with_sample(URL, payload_at(later()));
    let candidates =
        || -> Result<Vec<SourceCandidate>> { Ok(vec![source("gated", URL, "EU0001", 0)?]) };

    let first = finder.assess(candidates()?, &mut probe, now())?;
    assert!(first[0].is_registered(), "the premise: adopted while open");

    let second = finder.assess(candidates()?, &mut probe, later())?;
    let transition = second[0]
        .transition()
        .ok_or_else(|| Error::not_found("a revoked endpoint moved nothing"))?;
    assert_eq!(transition.action, LifecycleAction::Quarantined);
    assert!(
        transition.reason.contains("HTTP 403"),
        "the transition does not name how access was lost: {}",
        transition.reason
    );
    assert!(
        finder
            .registered("gated")
            .is_some_and(|entry| entry.is_quarantined())
    );
    Ok(())
}

#[test]
fn a_candidate_whose_records_are_dated_after_the_probe_is_refused_on_manipulation_risk()
-> Result<()> {
    const FORGED: &str = "https://forged.example/quotes";
    const PLAIN: &str = "http://plain.example/quotes";
    let mut finder = finder()?;
    let mut probe = InMemoryProbe::new()
        .with_robots("forged.example", permissive_robots())
        .with_head(FORGED, ok_head())
        .with_sample(
            FORGED,
            payload_at(now().saturating_add(Duration::from_days(1))),
        )
        .with_robots("plain.example", permissive_robots())
        .with_head(PLAIN, ok_head())
        // Inside the skew tolerance: a clock two minutes fast is a clock.
        .with_sample(
            PLAIN,
            payload_at(now().saturating_add(Duration::from_mins(2))),
        );

    let decisions = finder.assess(
        vec![
            source("forged", FORGED, "EU0001", 0)?,
            source("plain", PLAIN, "EU0002", 0)?,
        ],
        &mut probe,
        now(),
    )?;

    let forged = decision_for(&decisions, "forged")?;
    let DecisionOutcome::Rejected { reason } = forged.outcome() else {
        return Err(Error::invalid(format!(
            "a source dating its records tomorrow was {}",
            forged.outcome().as_str()
        )));
    };
    assert!(
        reason.contains("manipulation risk flagged") && reason.contains("newest record"),
        "the refusal does not name the attribute and the finding: {reason}"
    );
    assert!(finder.registered("forged").is_none());

    // Elevated is recorded and does not refuse: plaintext is already charged
    // to reliability, and refusing it here would reject the same fact twice.
    let plain = decision_for(&decisions, "plain")?;
    assert!(
        plain.is_registered(),
        "plaintext alone refused registration"
    );
    assert!(
        plain
            .reasoning()
            .steps()
            .iter()
            .any(|step| step.describe().contains("manipulation risk elevated")),
        "the inspection left no trace on the decision"
    );
    Ok(())
}

#[test]
fn a_registered_source_the_probe_cannot_reach_keeps_its_registration() -> Result<()> {
    // An outage on this platform's side is not a finding about the source. A
    // policy that retired what it could not reach would turn a crawler
    // outage into a deleted catalogue.
    const URL: &str = "https://steady.example/quotes";
    let mut finder = finder()?;
    let mut probe = InMemoryProbe::new()
        .with_robots("steady.example", permissive_robots())
        .with_head(URL, ok_head())
        .with_sample(URL, payload_at(now()));
    finder.assess(vec![source("steady", URL, "EU0001", 0)?], &mut probe, now())?;
    assert_eq!(class_of(&finder, "steady")?, RoutingClass::Hot);

    // A probe with no script answers nothing: every call is an error.
    let mut dark = InMemoryProbe::new();
    let second = finder.assess(
        vec![source("steady", URL, "EU0001", 0)?],
        &mut dark,
        later(),
    )?;
    assert!(
        matches!(second[0].outcome(), DecisionOutcome::Deferred { .. }),
        "the premise: an unreachable source is deferred, not {}",
        second[0].outcome().as_str()
    );
    assert!(second[0].transition().is_none());
    assert_eq!(class_of(&finder, "steady")?, RoutingClass::Hot);
    Ok(())
}

#[test]
fn a_registered_source_records_a_finding_for_every_inspection_attribute_in_its_trail() -> Result<()>
{
    // DATA-016's inspection record, on the path a clean source takes. Each
    // attribute already has a refusal test of its own (personal_data.rs,
    // legality.rs, the manipulation-risk test above). What none of them
    // shows is that a source which passes them all carries a finding for
    // every one in its trail, so that "why was this registered" is answered
    // attribute by attribute rather than by an absence. Every read is
    // through `at(stage)` and on a delimited prefix: a substring over the
    // joined trail matched tier.rs's "Probe it first" for the probe, and
    // could never match a stage written in CamelCase, because the trail
    // prints `assess_legality` and `score`.
    //
    // This does not test DATA-016's refusal half ("registration is refused,
    // naming the attribute, when any of the seven is missing"), and its
    // name does not claim to.
    const GOOD_URL: &str = "https://good.example/data";
    const PLAIN_URL: &str = "http://plain.example/data";
    let mut finder = finder()?;
    let mut probe = InMemoryProbe::new()
        .with_robots("good.example", permissive_robots())
        .with_head(GOOD_URL, ok_head())
        .with_sample(GOOD_URL, payload_at(now()))
        .with_robots("plain.example", permissive_robots())
        .with_head(PLAIN_URL, ok_head())
        .with_sample(PLAIN_URL, payload_at(now()));
    let decisions = finder.assess(
        vec![
            source("good_tls", GOOD_URL, "EU0001", 0)?,
            source("plain_http", PLAIN_URL, "EU0002", 0)?,
        ],
        &mut probe,
        now(),
    )?;

    // Premise: both candidates were assessed and both registered. An empty
    // result would pass every per-decision check below.
    assert_eq!(
        decisions.len(),
        2,
        "the premise failed: two candidates did not produce two decisions"
    );
    for (id, host, manipulation) in [
        ("good_tls", "good.example", "manipulation risk low"),
        ("plain_http", "plain.example", "manipulation risk elevated"),
    ] {
        let decision = decisions
            .iter()
            .find(|decision| decision.source_id() == id)
            .ok_or_else(|| Error::not_found(format!("no decision for {id}")))?;
        assert!(
            decision.is_registered(),
            "the premise failed: {id} was not registered: {}",
            decision.outcome().as_str()
        );
        let trail = decision.reasoning();
        let classify = trail.at(LifecycleStage::Classify);
        let probed = trail.at(LifecycleStage::Probe);
        let legality = trail.at(LifecycleStage::AssessLegality);
        let scored = trail.at(LifecycleStage::Score);

        // 1. Tier, settled on the probe's evidence, not the provisional
        // "not yet classifiable before the probe" that precedes it.
        assert!(
            classify.iter().any(|finding| finding.starts_with("tier ")
                && finding.ends_with(" on the probe's evidence")),
            "{id} has no tier settled on the probe's evidence: {classify:?}"
        );
        // 2. Host rules: the verdict of the finder's own rules, verbatim.
        let host_verdict = finder.config().host_rules().verdict(host).describe();
        assert!(
            legality.contains(&host_verdict.as_str()),
            "{id} does not record its host verdict {host_verdict:?}: {legality:?}"
        );
        // 3. Probe evidence: robots, HEAD and the sample, from the probe stage.
        assert!(
            probed
                .iter()
                .any(|finding| finding.starts_with("robots.txt served")
                    && finding.contains("; HEAD 200 in ")),
            "{id} has no probe-stage record of robots and HEAD: {probed:?}"
        );
        // 4. Personal-data screen, the Clear path's own record.
        assert!(
            legality
                .iter()
                .any(|finding| finding.starts_with("no personal identifier among the ")),
            "{id} has no personal-data finding: {legality:?}"
        );
        // 5. Manipulation risk, at the level its transport earns: TLS reads
        // low and plaintext elevated, so a constant finding fails one of them.
        assert!(
            probed
                .iter()
                .any(|finding| finding.starts_with(&format!("{manipulation}:"))),
            "{id} does not record `{manipulation}`: {probed:?}"
        );
        // 6. Legality: the robots verdict for this host and the licensing
        // verdict, each its own record.
        assert!(
            legality.iter().any(|finding| finding
                .starts_with(&format!("permitted: robots.txt for `{host}` "))),
            "{id} has no robots verdict for {host}: {legality:?}"
        );
        assert!(
            legality
                .iter()
                .any(|finding| finding.starts_with("permitted: licence ")),
            "{id} has no licensing verdict: {legality:?}"
        );
        // 7. Scores: exactly the five, each named.
        let names: Vec<&str> = scored
            .iter()
            .filter_map(|finding| finding.split_once(' ').map(|(name, _)| name))
            .collect();
        assert_eq!(
            names,
            [
                "reliability",
                "freshness",
                "uniqueness",
                "historical_value",
                "cost_efficiency"
            ],
            "{id}'s score stage is not the five named scores: {scored:?}"
        );
    }
    Ok(())
}
