//! The legal gate, which is the reason this crate exists in the shape it does.
//!
//! Four properties, each of which has a plausible implementation that gets it
//! wrong: unknown legality read as permission, a high score compensating for a
//! legal refusal, an allowlist entry rescuing a denylisted host, and a
//! research licence being enough to trade on.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{
    AGENT, QUOTE_PAYLOAD, candidate, endpoint, licensed_for, now, ok_head, permissive_robots,
    probe_for, robots_absent, robots_served, sample,
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
    // An undetermined verdict must not leave a trace in the finder's own
    // registry either: `registry()` backs the uniqueness score of every
    // later candidate and is what `Platform::data_finder` counts as
    // "currently collected" sources, so a source sitting there with no
    // registered decision behind it would be consulted as though it had
    // passed the gate this crate exists to hold.
    assert!(finder.registry().is_empty());
    assert!(finder.registered("no-robots").is_none());
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
    let rules = HostRules::new(["example.com".to_string()], ["example.com".to_string()])?;
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
        )?);
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
    assert!(finder.registry().is_empty());
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
    // A deferral is a fact about the crawler, not the source, but it must
    // still leave nothing in the registry: legality was never assessed for
    // a source the probe never reached.
    assert!(finder.registry().is_empty());
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

#[test]
fn ambiguous_licensing_terms_leave_the_registry_empty_too() -> Result<()> {
    // "Ambiguous" and "undetermined" reach `require_permitted` by different
    // roads (a lawyer versus a fetch) but must land in the same place here:
    // neither may leave a trace in what the finder considers collected.
    let mut finder = finder(Usage::Trade)?;
    let candidate = candidate(
        "ambiguous-terms",
        "https://example.com/data/prices.json",
        LicensingPosture::ambiguous("a terms page that names no usage"),
        &["EU0001"],
    )?;
    let mut probe = probe_for(
        "https://example.com/data/prices.json",
        "example.com",
        permissive_robots(),
    );

    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    assert!(!decisions[0].is_registered());
    assert!(finder.registry().is_empty());
    assert!(finder.registered("ambiguous-terms").is_none());
    Ok(())
}

/// Enumerates every way `assess_one` can decline to register a source and
/// proves each one leaves the finder's registry untouched, in a single
/// finder instance so a leak on one candidate cannot be masked by an
/// unrelated successful registration clearing state in between.
///
/// This is the adversarial sweep: rather than trusting that the code path
/// which inserts into the registry is only reachable after
/// `RegistrationDecision::registered` has already agreed, it drives every
/// declining branch (denylisted host, forbidden robots, undetermined robots,
/// a licence that does not cover the required usage, ambiguous terms, and an
/// unreachable probe) through the
/// same registry and checks none of them left anything behind — and that a
/// source which *does* pass every gate is the only one that appears.
#[test]
fn no_declining_branch_of_the_lifecycle_leaves_a_source_in_the_finders_registry() -> Result<()> {
    let config = FinderConfig::new(AGENT, Usage::Trade, "market-data", 7)?
        .with_host_rules(HostRules::new(Vec::new(), ["denied.example".to_string()])?);
    let mut finder = DataFinder::new(config);

    // 1. Denylisted host: refused before any request is made.
    let denylisted = candidate(
        "denylisted-host",
        "https://denied.example/data/prices.json",
        licensed_for(&[Usage::Trade])?,
        &["EU0001"],
    )?;
    // 2. robots.txt names the exact path forbidden.
    let robots_forbidden = candidate(
        "robots-forbidden",
        "https://a.example/data/prices.json",
        licensed_for(&[Usage::Trade])?,
        &["EU0002"],
    )?;
    // 3. No robots.txt at all: undetermined, not permission.
    let robots_undetermined = candidate(
        "robots-undetermined",
        "https://b.example/data/prices.json",
        licensed_for(&[Usage::Trade])?,
        &["EU0003"],
    )?;
    // 4. A licence that covers Research but was asked about Trade.
    let wrong_usage = candidate(
        "wrong-usage",
        "https://c.example/data/prices.json",
        licensed_for(&[Usage::Research])?,
        &["EU0004"],
    )?;
    // 5. Terms exist but were never mapped to a usage.
    let ambiguous = candidate(
        "ambiguous-usage",
        "https://d.example/data/prices.json",
        LicensingPosture::ambiguous("no usage grant stated"),
        &["EU0005"],
    )?;
    // 6. Permitted on every legal question, but the source passes.
    let admitted = candidate(
        "admitted",
        "https://e.example/data/prices.json",
        licensed_for(&[Usage::Trade])?,
        &["EU0006"],
    )?;

    let mut probe = InMemoryProbe::new()
        .with_head("https://a.example/data/prices.json", ok_head())
        .with_sample("https://a.example/data/prices.json", sample(QUOTE_PAYLOAD))
        .with_robots(
            "a.example",
            robots_served("User-agent: *\nAllow: /\nDisallow: /data/\n"),
        )
        .with_head("https://b.example/data/prices.json", ok_head())
        .with_sample("https://b.example/data/prices.json", sample(QUOTE_PAYLOAD))
        .with_robots("b.example", robots_absent())
        .with_head("https://c.example/data/prices.json", ok_head())
        .with_sample("https://c.example/data/prices.json", sample(QUOTE_PAYLOAD))
        .with_robots("c.example", permissive_robots())
        .with_head("https://d.example/data/prices.json", ok_head())
        .with_sample("https://d.example/data/prices.json", sample(QUOTE_PAYLOAD))
        .with_robots("d.example", permissive_robots())
        .with_head("https://e.example/data/prices.json", ok_head())
        .with_sample("https://e.example/data/prices.json", sample(QUOTE_PAYLOAD))
        .with_robots("e.example", permissive_robots());
    // "unreachable" is deliberately given no scripted response: the probe
    // must refuse rather than invent one, which is what turns the outcome
    // into a deferral.
    let unreachable = candidate(
        "unreachable",
        "https://f.example/data/prices.json",
        licensed_for(&[Usage::Trade])?,
        &["EU0007"],
    )?;

    let decisions = finder.assess(
        vec![
            denylisted,
            robots_forbidden,
            robots_undetermined,
            wrong_usage,
            ambiguous,
            admitted,
            unreachable,
        ],
        &mut probe,
        now(),
    )?;

    let by_id = |id: &str| decisions.iter().find(|decision| decision.source_id() == id);

    assert!(!by_id("denylisted-host").unwrap().is_registered());
    assert!(!by_id("robots-forbidden").unwrap().is_registered());
    assert!(!by_id("robots-undetermined").unwrap().is_registered());
    assert!(!by_id("wrong-usage").unwrap().is_registered());
    assert!(!by_id("ambiguous-usage").unwrap().is_registered());
    assert!(!by_id("unreachable").unwrap().is_registered());
    assert!(by_id("admitted").unwrap().is_registered());

    // The only entry the registry may hold is the one source that actually
    // cleared every gate.
    assert_eq!(
        finder.registry().keys().collect::<Vec<_>>(),
        vec!["admitted"],
        "a declining branch left a source in the registry: {:?}",
        finder.registry().keys().collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn a_licence_that_has_not_taken_effect_yet_does_not_retroactively_permit_an_earlier_instant()
-> Result<()> {
    // A licence agreed today has a beginning. Asked about a usage at an
    // instant before that beginning, the honest answer is that the terms did
    // not yet apply — not that today's terms silently cover the past, which
    // is exactly the kind of point-in-time leakage a bitemporal feature store
    // exists to refuse.
    let signed_at = now();
    let before_signing = signed_at.saturating_sub(qip_core::Duration::from_days(30));

    let license =
        SourceLicense::new("vendor-terms-2026", [Usage::Trade])?.effective_from(signed_at);

    assert!(license.permits_at(Usage::Trade, signed_at));
    assert!(!license.permits_at(Usage::Trade, before_signing));

    let posture = LicensingPosture::declared(license);
    let too_early = posture.legality_for(Usage::Trade, before_signing);
    assert!(too_early.is_forbidden());
    assert!(
        too_early.reason().contains("does not take effect"),
        "the refusal must name why: {}",
        too_early.reason()
    );

    let after_signing = posture.legality_for(Usage::Trade, signed_at);
    assert!(after_signing.is_permitted());

    // A licence with no stated effective date keeps behaving exactly as
    // before: this is additive, not a change of default for every existing
    // caller that never states one.
    let undated = SourceLicense::new("undated-terms", [Usage::Trade])?;
    assert!(undated.permits_at(Usage::Trade, before_signing));
    Ok(())
}

#[test]
fn a_denylisted_host_cannot_be_smuggled_past_the_rules_as_userinfo_in_the_url() -> Result<()> {
    // Premise: the rule fires on the host the parser produces for the plain
    // URL, so this test is about the smuggled form and not about a denylist
    // that never worked.
    let rules = HostRules::new(Vec::new(), ["denied.example".to_string()])?;
    assert!(rules.verdict("denied.example").is_forbidden());
    assert_eq!(
        endpoint("https://denied.example/x")?.host(),
        "denied.example"
    );

    // `covers` compares the character before the entry against `.`. In
    // `collector.example@denied.example` that character is `@`, so the
    // denylist entry did not fire and the request the denylist exists to
    // prevent was made. The parser refuses the host rather than guessing
    // which half of it was meant.
    let error = endpoint("https://collector.example@denied.example/data/prices.json").unwrap_err();
    assert!(matches!(error, Error::Invalid(_)), "{error}");
    assert!(
        error.message().contains("user information"),
        "the refusal must name what is wrong with the host: {}",
        error.message()
    );

    // The DNS root dot is the same hole by another road: `denied.example.`
    // matches no rule written as `denied.example`.
    assert!(rules.verdict("denied.example.").is_permitted());
    let rooted = endpoint("https://denied.example./data/prices.json").unwrap_err();
    assert!(matches!(rooted, Error::Invalid(_)), "{rooted}");
    assert!(
        rooted.message().contains("empty label"),
        "the refusal must name what is wrong with the host: {}",
        rooted.message()
    );
    Ok(())
}

#[test]
fn a_host_rule_that_no_parsed_host_could_ever_match_is_refused_when_the_rules_are_built()
-> Result<()> {
    // Premise: a well-formed entry is accepted and does fire, on the host
    // itself and on a subdomain of it.
    let rules = HostRules::new(Vec::new(), ["example.com".to_string()])?;
    assert!(rules.verdict("example.com").is_forbidden());
    assert!(rules.verdict("api.example.com").is_forbidden());

    // Each of these was storable, and stored it was a control that reads as
    // protection and can never fire — the `MaxExpectedShortfall` class the
    // risk rules name by name.
    for unmatchable in [
        ".example.com",
        "https://example.com",
        "example.com:443",
        "  ",
    ] {
        let error = HostRules::new(Vec::new(), [unmatchable.to_string()]).unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{unmatchable}: {error}");
        assert!(
            error.message().contains("cannot be a host denylist entry"),
            "`{unmatchable}` must be refused as a denylist entry: {}",
            error.message()
        );
    }

    // The allowlist is validated by the same rule and says which list it was.
    let allow = HostRules::new([".example.com".to_string()], Vec::new()).unwrap_err();
    assert!(
        allow.message().contains("cannot be a host allowlist entry"),
        "{}",
        allow.message()
    );
    Ok(())
}

#[test]
fn a_host_absent_from_a_configured_allowlist_is_refused_even_though_no_denylist_names_it()
-> Result<()> {
    // Premise: the allowlist is real — it admits its own entry and a
    // subdomain of it — and the denylist is empty, so nothing else could be
    // producing the refusal below.
    let rules = HostRules::new(["allowed.example".to_string()], Vec::new())?;
    assert!(rules.verdict("allowed.example").is_permitted());
    assert!(rules.verdict("api.allowed.example").is_permitted());
    assert!(rules.denylist().is_empty());

    // This arm had never fired in any test in the workspace: an allowlist
    // that admitted everything not denied would have passed every one of
    // them, which is the whole of an allowlist's purpose gone.
    let verdict = rules.verdict("other.example");
    assert!(verdict.is_forbidden());
    assert!(
        verdict.reason().contains("allowlist"),
        "the refusal must name the allowlist that produced it: {}",
        verdict.reason()
    );
    Ok(())
}

#[test]
fn an_expired_licence_denies_its_entitlements_as_expired_rather_than_out_of_scope() -> Result<()> {
    let expiry = now();
    let license = SourceLicense::new("expiring-terms", [Usage::Trade])?.expiring_at(expiry);

    // Premise: a second before expiry the licence really did grant trading,
    // so the denial below is about the lapse and not about the scope.
    let before = expiry.saturating_sub(qip_core::Duration::from_secs(1));
    let granted = license.entitlements("source.x", before);
    assert!(
        granted.iter().any(|entitlement| matches!(
            entitlement,
            Entitlement::Granted { usage, .. } if *usage == Usage::Trade
        )),
        "the premise failed: {granted:?}"
    );

    let lapsed = license.entitlements("source.x", expiry);
    let Some(Entitlement::Denied { reason, .. }) = lapsed.iter().find(|entitlement| {
        matches!(entitlement, Entitlement::Denied { usage, .. } if *usage == Usage::Trade)
    }) else {
        return Err(Error::not_found("an explicit denial of trading"));
    };
    // The entitlement is what travels to the mesh catalogue. Recorded as
    // out of scope, it sends the reader to renegotiate when the remedy is to
    // renew.
    assert!(
        reason.contains("expired"),
        "an expired licence must deny as expired: {reason}"
    );
    assert!(
        !reason.contains("and not trade"),
        "an expired licence must not deny as out of scope: {reason}"
    );
    Ok(())
}

#[test]
fn a_licence_whose_term_has_not_begun_denies_its_entitlements_as_not_yet_in_effect() -> Result<()> {
    let signed_at = now();
    let before_signing = signed_at.saturating_sub(qip_core::Duration::from_days(30));
    let license =
        SourceLicense::new("vendor-terms-2026", [Usage::Trade])?.effective_from(signed_at);

    // Premise: at signing the licence grants trading, so the denial below is
    // about the beginning of the term and nothing else.
    let granted = license.entitlements("source.x", signed_at);
    assert!(
        granted.iter().any(|entitlement| matches!(
            entitlement,
            Entitlement::Granted { usage, .. } if *usage == Usage::Trade
        )),
        "the premise failed: {granted:?}"
    );

    let early = license.entitlements("source.x", before_signing);
    let Some(Entitlement::Denied { reason, .. }) = early.iter().find(|entitlement| {
        matches!(entitlement, Entitlement::Denied { usage, .. } if *usage == Usage::Trade)
    }) else {
        return Err(Error::not_found("an explicit denial of trading"));
    };
    assert!(
        reason.contains("does not take effect"),
        "a licence that has not begun must say so: {reason}"
    );
    assert!(
        !reason.contains("and not trade"),
        "a licence that has not begun must not deny as out of scope: {reason}"
    );
    Ok(())
}

#[test]
fn an_unnamed_licence_cannot_be_constructed() -> Result<()> {
    // Premise: a named licence is built and keeps the usages it was given, so
    // the refusal below is about the name.
    let named = SourceLicense::new("vendor-terms-2026", [Usage::Trade])?;
    assert!(named.permits().contains(&Usage::Trade));

    // "We believe this is fine" is the claim this type exists to make
    // unrepresentable, and a whitespace identifier is that claim wearing a
    // name.
    let error = SourceLicense::new("   ", [Usage::Trade]).unwrap_err();
    assert!(matches!(error, Error::Invalid(_)), "{error}");
    assert!(
        error.message().contains("unnamed licence"),
        "the refusal must say what is missing: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn the_perpetual_expiry_survives_the_json_round_trip_a_nanosecond_sentinel_would_not() -> Result<()>
{
    // Premise: the property is real and specific. A timestamp is serialised
    // at millisecond precision, so `Timestamp::MAX` — a nanosecond value —
    // comes back a fraction earlier and no longer equals what was written.
    let max_json = serde_json::to_string(&qip_core::Timestamp::MAX)
        .map_err(|error| Error::invalid(error.to_string()))?;
    let max_back: qip_core::Timestamp =
        serde_json::from_str(&max_json).map_err(|error| Error::invalid(error.to_string()))?;
    assert_ne!(
        max_back,
        qip_core::Timestamp::MAX,
        "the premise failed: the round trip is lossless, so this test proves nothing"
    );

    // The entitlement for a licence that never lapses is the one the evidence
    // store has to read back equal to what it wrote; a decision that no longer
    // equals its own record makes the evidence store's whole point false for
    // exactly the entitlements nobody renews.
    let license = SourceLicense::new("perpetual-terms", [Usage::Trade])?;
    let entitlements = license.entitlements("source.x", now());
    let granted = entitlements
        .iter()
        .find(|entitlement| {
            matches!(entitlement, Entitlement::Granted { usage, .. } if *usage == Usage::Trade)
        })
        .ok_or_else(|| Error::not_found("a grant of trading"))?;
    assert!(
        matches!(granted, Entitlement::Granted { expires_at, .. } if *expires_at == SourceLicense::PERPETUAL)
    );

    let json = serde_json::to_string(granted).map_err(|error| Error::invalid(error.to_string()))?;
    let restored: Entitlement =
        serde_json::from_str(&json).map_err(|error| Error::invalid(error.to_string()))?;
    assert_eq!(&restored, granted);
    Ok(())
}
