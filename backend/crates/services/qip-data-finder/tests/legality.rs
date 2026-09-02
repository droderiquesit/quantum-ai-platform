//! The legal gate, which is the reason this crate exists in the shape it does.
//!
//! Four properties, each of which has a plausible implementation that gets it
//! wrong: unknown legality read as permission, a high score compensating for a
//! legal refusal, an allowlist entry rescuing a denylisted host, and a
//! research licence being enough to trade on.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{
    AGENT, candidate, licensed_for, now, permissive_robots, probe_for, robots_absent, robots_served,
};
use qip_contracts::governance::{Entitlement, Usage};
use qip_core::error::{Error, Result};
use qip_data_finder::decision::{DecisionOutcome, LifecycleStage};
use qip_data_finder::finder::{DataFinder, FinderConfig};
use qip_data_finder::legal::{HostRules, Legality, LicensingPosture, SourceLicense};
use qip_data_finder::probe::{InMemoryProbe, RobotsFetch};
use qip_data_finder::scoring::{Routing, RoutingClass, SourceScores};

fn finder(usage: Usage) -> Result<DataFinder> {
    Ok(DataFinder::new(FinderConfig::new(
        AGENT,
        usage,
        "market-data",
        99,
    )?))
}

#[test]
fn a_forbidden_robots_rule_blocks_collection_however_good_the_source_is() -> Result<()> {
    let mut finder = finder(Usage::Trade)?;
    let candidate = candidate(
        "blocked",
        "https://example.com/data/prices.json",
        licensed_for(&[Usage::Research, Usage::Derive, Usage::Trade])?,
        &["EU0001"],
    )?;
    let mut probe = probe_for(
        "https://example.com/data/prices.json",
        "example.com",
        robots_served("User-agent: *\nAllow: /\nDisallow: /data/\n"),
    );

    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let decision = decisions
        .first()
        .ok_or_else(|| Error::not_found("a decision"))?;

    assert!(!decision.is_registered());
    let legality = decision
        .legality()
        .ok_or_else(|| Error::not_found("legality"))?;
    assert!(legality.robots().is_forbidden());
    assert!(
        legality.robots().reason().contains("/data/"),
        "the refusal must name the rule that produced it: {}",
        legality.robots().reason()
    );
    assert!(finder.registry().is_empty());
    Ok(())
}

#[test]
fn an_undetermined_legality_is_refused_rather_than_read_as_permission() -> Result<()> {
    // The realistic case: the host has no robots.txt at all. Nothing forbids
    // the crawl, and that is not the same as something permitting it.
    let mut finder = finder(Usage::Trade)?;
    let candidate = candidate(
        "no-robots",
        "https://silent.example/data/prices.json",
        licensed_for(&[Usage::Research, Usage::Derive, Usage::Trade])?,
        &["EU0001"],
    )?;
    let mut probe = probe_for(
        "https://silent.example/data/prices.json",
        "silent.example",
        robots_absent(),
    );

    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let decision = decisions
        .first()
        .ok_or_else(|| Error::not_found("a decision"))?;

    let legality = decision
        .legality()
        .ok_or_else(|| Error::not_found("legality"))?;
    assert!(legality.robots().is_unknown());
    assert!(!legality.overall().is_permitted());
    assert!(!decision.is_registered());
    assert!(
        legality.robots().reason().contains("not a permission"),
        "the record must say why absence is not consent: {}",
        legality.robots().reason()
    );
    Ok(())
}

#[test]
fn no_score_however_high_overrides_a_legal_refusal() -> Result<()> {
    let perfect = SourceScores::new(1.0, 1.0, 1.0, 1.0, 1.0)?;
    assert!(perfect.composite() > Routing::HOT_THRESHOLD);

    for refusal in [
        Legality::forbidden("robots.txt disallows /"),
        Legality::unknown("no licence could be located"),
    ] {
        let routing = Routing::decide(&refusal, &perfect);
        assert_eq!(routing.class(), RoutingClass::Rejected);
        assert!(
            routing.basis().contains("does not enter into it"),
            "the record must say the score was not what decided: {}",
            routing.basis()
        );
    }

    // And the permitted case reaches Hot on the same scores, so the test is
    // about legality rather than about the thresholds being unreachable.
    assert_eq!(
        Routing::decide(&Legality::permitted("licensed and allowed"), &perfect).class(),
        RoutingClass::Hot
    );
    Ok(())
}

#[test]
fn a_denylisted_host_is_refused_even_when_it_is_also_allowlisted() -> Result<()> {
    let rules = HostRules::new(["example.com".to_string()], ["example.com".to_string()]);
    let verdict = rules.verdict("example.com");
    assert!(verdict.is_forbidden());
    assert!(verdict.reason().contains("denylisted"));

    // And a subdomain of a denied host is denied with it.
    assert!(rules.verdict("api.example.com").is_forbidden());
    Ok(())
}

#[test]
fn a_denylisted_host_is_never_contacted_at_all() -> Result<()> {
    // Refusing after fetching would satisfy a verdict test and still have
    // sent the request the denylist exists to prevent.
    let config =
        FinderConfig::new(AGENT, Usage::Trade, "market-data", 99)?.with_host_rules(HostRules::new(
            ["denied.example".to_string()],
            ["denied.example".to_string()],
        ));
    let mut finder = DataFinder::new(config);
    let candidate = candidate(
        "denied",
        "https://denied.example/data/prices.json",
        licensed_for(&[Usage::Research, Usage::Derive, Usage::Trade])?,
        &["EU0001"],
    )?;
    let mut probe = probe_for(
        "https://denied.example/data/prices.json",
        "denied.example",
        permissive_robots(),
    );

    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let decision = decisions
        .first()
        .ok_or_else(|| Error::not_found("a decision"))?;

    assert!(matches!(
        decision.outcome(),
        DecisionOutcome::Rejected { .. }
    ));
    assert!(
        probe.calls().is_empty(),
        "a denylisted host must not be contacted, and these calls were made: {:?}",
        probe.calls()
    );
    assert!(
        decision
            .reasoning()
            .at(LifecycleStage::Probe)
            .iter()
            .any(|finding| finding.contains("not probed")),
        "the record must show that no request was made"
    );
    Ok(())
}

#[test]
fn a_research_only_licence_cannot_produce_a_source_usable_for_trading() -> Result<()> {
    let research_only = licensed_for(&[Usage::Research])?;

    // Asked about trading, the licence refuses outright rather than leaving
    // the question open — the terms were read, and they do not grant it.
    let verdict = research_only.legality_for(Usage::Trade, now());
    assert!(verdict.is_forbidden());
    assert!(verdict.reason().contains("research"));

    let mut trading_finder = finder(Usage::Trade)?;
    let mut probe = probe_for(
        "https://example.com/data/prices.json",
        "example.com",
        permissive_robots(),
    );
    let decisions = trading_finder.assess(
        vec![candidate(
            "research-feed",
            "https://example.com/data/prices.json",
            research_only.clone(),
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    assert!(!decisions[0].is_registered());
    assert!(trading_finder.registry().is_empty());

    // The same source is registrable by a finder that only wants research,
    // and even then its entitlements deny trading explicitly.
    let mut research_finder = finder(Usage::Research)?;
    let mut probe = probe_for(
        "https://example.com/data/prices.json",
        "example.com",
        permissive_robots(),
    );
    let decisions = research_finder.assess(
        vec![candidate(
            "research-feed",
            "https://example.com/data/prices.json",
            research_only,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    let decision = &decisions[0];
    assert!(decision.is_registered());

    let DecisionOutcome::Registered(registration) = decision.outcome() else {
        return Err(Error::invalid("expected a registration"));
    };
    let trade_entitlement = registration
        .entitlements()
        .iter()
        .find(|entitlement| matches!(entitlement, Entitlement::Denied { usage, .. } if *usage == Usage::Trade))
        .ok_or_else(|| Error::not_found("an explicit denial of trading"))?;
    assert!(!trade_entitlement.is_granted(now()));

    // And the mesh catalogue entry carries the same denial, so the two
    // cannot disagree about what the licence says.
    let entry = decision.catalogue_entry("market-data")?;
    assert!(entry.permits(Usage::Research, now()));
    assert!(!entry.permits(Usage::Trade, now()));
    Ok(())
}

/// The `Registered` outcome is built in exactly one place, and that place
/// takes the legality assessment as an argument.
///
/// Until `Registration` had private fields, `DecisionOutcome::Registered` was
/// a public struct variant: any caller with a routing and a policy in hand
/// could assemble one, pass it to `RegistrationDecision::new`, and ask for a
/// catalogue entry — a mesh registration for a source whose licence nobody
/// had read, produced by a path that never touched the gate. The compile-fail
/// doctest on `Registration` proves the struct literal no longer compiles
/// outside the crate; this test proves the one constructor that remains
/// refuses when the assessment it is handed did not permit collection, and
/// records the assessment on the decision when it did.
#[test]
fn the_registered_outcome_exists_only_on_the_far_side_of_the_legality_assessment() -> Result<()> {
    use qip_data_finder::decision::{LifecycleStage, Reasoning, RegistrationDecision};
    use qip_data_finder::legal::{LegalAssessment, SourcePolicy};

    let research_only = licensed_for(&[Usage::Research])?;
    let refused = LegalAssessment::combine(
        Legality::permitted("allowlisted"),
        Legality::permitted("robots.txt allows /"),
        research_only.legality_for(Usage::Trade, now()),
        Usage::Trade,
    );
    // Premise: asked about trading, the licence said no, and that refusal is
    // the overall verdict.
    assert!(refused.licensing().is_forbidden());
    assert!(refused.overall().is_forbidden());

    // A routing decided against somebody else's permissive verdict — the
    // mismatched pair a caller that skipped the gate would present.
    let perfect = SourceScores::new(1.0, 1.0, 1.0, 1.0, 1.0)?;
    let hot = Routing::decide(
        &Legality::permitted("a verdict about another source"),
        &perfect,
    );
    assert_eq!(hot.class(), RoutingClass::Hot);
    let policy = SourcePolicy::assemble("example.com", AGENT, None, None, Vec::new(), true);
    let mut reasoning = Reasoning::new();
    reasoning.record(LifecycleStage::AssessLegality, refused.overall().describe());

    let error = RegistrationDecision::registered(
        "research-feed",
        refused,
        hot.clone(),
        policy.clone(),
        Vec::new(),
        reasoning.clone(),
        now(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::Denied(_)), "{error}");
    assert!(
        error.message().contains("research"),
        "the refusal must name the licence that produced it: {}",
        error.message()
    );

    // The routing is checked on its own, not inferred from the assessment: a
    // permitted assessment paired with a routing that rejected is refused too.
    let permitted = LegalAssessment::combine(
        Legality::permitted("allowlisted"),
        Legality::permitted("robots.txt allows /"),
        licensed_for(&[Usage::Research, Usage::Trade])?.legality_for(Usage::Trade, now()),
        Usage::Trade,
    );
    assert!(permitted.overall().is_permitted());
    let rejected = Routing::decide(&Legality::forbidden("robots.txt disallows /"), &perfect);
    assert_eq!(rejected.class(), RoutingClass::Rejected);
    let error = RegistrationDecision::registered(
        "licensed-feed",
        permitted.clone(),
        rejected,
        policy.clone(),
        Vec::new(),
        reasoning.clone(),
        now(),
    )
    .unwrap_err();
    assert!(matches!(error, Error::Denied(_)), "{error}");
    assert!(
        error.message().contains("routed to `rejected`"),
        "{}",
        error.message()
    );

    // The same inputs with a permitting assessment register, and the decision
    // carries the assessment it was built from rather than `None`.
    let decision = RegistrationDecision::registered(
        "licensed-feed",
        permitted,
        hot,
        policy,
        Vec::new(),
        reasoning,
        now(),
    )?;
    assert!(decision.is_registered());
    assert!(
        decision
            .legality()
            .is_some_and(|legality| legality.overall().is_permitted()),
        "a registered decision must carry the assessment that permitted it"
    );
    assert!(decision.catalogue_entry("market-data").is_ok());
    Ok(())
}

#[test]
fn an_expired_licence_stops_granting_what_it_used_to() -> Result<()> {
    let expiry = now();
    let posture = LicensingPosture::declared(
        SourceLicense::new("expiring-terms", [Usage::Trade])?.expiring_at(expiry),
    );

    assert!(
        posture
            .legality_for(
                Usage::Trade,
                expiry.saturating_sub(qip_core::Duration::from_secs(1))
            )
            .is_permitted()
    );
    let after = posture.legality_for(Usage::Trade, expiry);
    assert!(after.is_forbidden());
    assert!(after.reason().contains("expired"));
    Ok(())
}

#[test]
fn ambiguous_terms_are_undetermined_rather_than_denied_or_granted() -> Result<()> {
    // The remedy differs: undetermined needs a fetch, ambiguous needs a
    // lawyer. Collapsing them loses which one to ask.
    let ambiguous = LicensingPosture::ambiguous("a terms page with no usage grant");
    let verdict = ambiguous.legality_for(Usage::Derive, now());
    assert!(verdict.is_unknown());
    assert!(verdict.reason().contains("human"));

    assert!(
        LicensingPosture::Undetermined
            .legality_for(Usage::Derive, now())
            .is_unknown()
    );
    Ok(())
}

#[test]
fn combining_verdicts_always_keeps_the_least_permissive_one() -> Result<()> {
    let permitted = Legality::permitted("allowlisted");
    let unknown = Legality::unknown("no robots.txt");
    let forbidden = Legality::forbidden("disallowed by rule");

    assert!(permitted.clone().and(unknown.clone()).is_unknown());
    assert!(unknown.clone().and(permitted.clone()).is_unknown());
    assert!(unknown.clone().and(forbidden.clone()).is_forbidden());
    assert!(forbidden.clone().and(permitted.clone()).is_forbidden());
    assert!(permitted.clone().and(permitted).is_permitted());

    // And the refusal is an error a caller cannot ignore.
    let refusal = unknown.require_permitted("source `x`").unwrap_err();
    assert!(matches!(refusal, Error::Denied(_)));
    assert!(refusal.message().contains("not a permission"));
    Ok(())
}

#[test]
fn a_probe_that_cannot_reach_a_source_defers_rather_than_rejecting_it() -> Result<()> {
    // A broken crawler must not look like a hundred bad sources.
    let mut finder = finder(Usage::Research)?;
    let mut probe = InMemoryProbe::new();
    let decisions = finder.assess(
        vec![candidate(
            "unreachable",
            "https://example.com/data/prices.json",
            licensed_for(&[Usage::Research])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;

    assert!(matches!(
        decisions[0].outcome(),
        DecisionOutcome::Deferred { .. }
    ));
    assert!(!decisions[0].reasoning().is_empty());
    Ok(())
}

#[test]
fn robots_does_not_govern_a_mechanism_it_was_never_about() -> Result<()> {
    // A licensed bulk drop's terms come from a contract. Treating a missing
    // robots.txt as an open question there would leave every contracted feed
    // permanently undetermined.
    use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, FileFormat, SourceEndpoint};
    let bulk = SourceEndpoint::parse(
        "https://vendor.example/drops/eod.csv",
        AccessMechanism::BulkFile {
            format: FileFormat::Csv,
            published_every: qip_core::Duration::from_days(1),
            auth: AuthRequirement::ApiKey {
                header: "X-Api-Key".to_string(),
            },
        },
    )?;
    assert!(!bulk.mechanism().is_governed_by_robots());

    let rest = SourceEndpoint::parse(
        "https://vendor.example/api/quotes",
        AccessMechanism::Rest {
            auth: AuthRequirement::None,
            incremental_parameter: None,
            page_size: 100,
        },
    )?;
    assert!(rest.mechanism().is_governed_by_robots());
    Ok(())
}

#[test]
fn an_unnamed_crawler_cannot_be_configured() -> Result<()> {
    // A publisher's only recourse is to block a user agent by name.
    let error = FinderConfig::new("   ", Usage::Research, "market-data", 1).unwrap_err();
    assert!(matches!(error, Error::Invalid(_)));
    assert!(error.message().contains("cannot refuse"));
    Ok(())
}

#[test]
fn an_absent_robots_file_is_still_recorded_with_the_status_that_produced_it() -> Result<()> {
    // A 404 and a 403 mean different things about a publisher's intent, and
    // the record has to keep which one happened.
    let forbidden_robots = RobotsFetch::Absent {
        status: 403,
        latency: qip_core::Duration::from_millis(5),
    };
    assert!(forbidden_robots.policy().is_none());
    assert!(forbidden_robots.describe().contains("403"));
    Ok(())
}
