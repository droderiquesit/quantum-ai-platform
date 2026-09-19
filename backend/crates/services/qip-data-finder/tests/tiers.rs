//! The deep-web source tier: classification from evidence, the six access
//! modes and what each needs, the discovery enclave, and the dark web's
//! standing refusal.
//!
//! Every property here has a plausible implementation that gets it wrong:
//! a tier defaulted to "surface" for a candidate nobody probed, a credential
//! value pasted where a name was wanted, a rendered page fetched outside the
//! enclave because no enclave was configured, and a dark-web host that was
//! "only" HEADed. Each test names the one it prevents.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{
    AGENT, QUOTE_PAYLOAD, coverage, licensed_for, now, ok_head, permissive_robots, probe_for,
    sample,
};
use qip_contracts::governance::Usage;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Duration};
use qip_data_finder::category::{CategoryApproval, ContentSignal, SourceCategory};
use qip_data_finder::coverage::{SourceRegion, UpdateFrequency};
use qip_data_finder::decision::{DecisionOutcome, LifecycleStage, RegistrationDecision};
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, FileFormat, SourceEndpoint};
use qip_data_finder::finder::{DataFinder, FinderConfig};
use qip_data_finder::legal::{LicensingPosture, RateLimit, SourceLicense};
use qip_data_finder::probe::{HeadResponse, InMemoryProbe};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::registration::{RegistrationRecord, RegistrationRequirement};
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_data_finder::tier::{
    AccessMode, BulkCadence, CredentialReference, DeepWebAdapter, DefensiveMonitoring,
    DiscoveryEnclave, Promotion, RenderingBudget, RobotsPosture, SourceTier, TierEvidence,
};
use qip_events::Topic;
use qip_market_ingestion::connector::manifest::SecretRef;

/// A candidate that declares what it is — a filing search — so the finder
/// can place it in §7.6.1's regulatory-and-legal category. Every fixture
/// here declares one, and [`config`] approves that category, because
/// §7.6.3 promotes a deep-web adapter only within an approved category and
/// the refusals these tests prove are about something else; the approval
/// itself is proven by the `§7.6.3` tests below on a config without one.
fn candidate_with(
    id: &str,
    url: &str,
    mechanism: AccessMechanism,
    licensing: LicensingPosture,
    cost: SourceCost,
) -> Result<SourceCandidate> {
    Ok(
        uncategorised_candidate(id, url, mechanism, licensing, cost)?
            .with_content_signal(ContentSignal::RegulatoryFiling),
    )
}

/// The same candidate with nothing declared about what it is.
fn uncategorised_candidate(
    id: &str,
    url: &str,
    mechanism: AccessMechanism,
    licensing: LicensingPosture,
    cost: SourceCost,
) -> Result<SourceCandidate> {
    SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd")?,
        SourceEndpoint::parse(url, mechanism)?,
        coverage(&["EU0001"], UpdateFrequency::Minutely)?,
        licensing,
        cost,
        SourceRegion::Europe,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        now(),
    )
}

/// A person's one-time approval of the category every fixture declares.
fn filings_approved_by_desk_owner() -> Result<CategoryApproval> {
    CategoryApproval::new(
        SourceCategory::RegulatoryAndLegal,
        "desk-owner",
        now(),
        "https://desk.example/reviews/regulatory-sources-2026-09",
    )
}

fn open_rest() -> AccessMechanism {
    AccessMechanism::Rest {
        auth: AuthRequirement::None,
        incremental_parameter: Some("since".to_string()),
        page_size: 500,
    }
}

fn keyed_rest() -> AccessMechanism {
    AccessMechanism::Rest {
        auth: AuthRequirement::ApiKey {
            header: "X-Api-Key".to_string(),
        },
        incremental_parameter: Some("since".to_string()),
        page_size: 500,
    }
}

fn html_page() -> AccessMechanism {
    AccessMechanism::HtmlPage {
        selector: "table.prices".to_string(),
    }
}

fn finder(config: FinderConfig) -> DataFinder {
    DataFinder::new(config)
}

fn config() -> Result<FinderConfig> {
    unapproved_config()?.with_category_approval(filings_approved_by_desk_owner()?)
}

/// A finder with no category approved: the closed position every
/// deployment starts in.
fn unapproved_config() -> Result<FinderConfig> {
    FinderConfig::new(AGENT, Usage::Derive, "market-data", 7)
}

/// A registration the owner made for `source_id`, so a keyed fixture has a
/// person's name behind its credential. The finder refuses a credentialed
/// source without one, and that refusal is proven in `registration.rs`;
/// here it is the premise of tests about something else.
fn registered_by_owner(config: FinderConfig, source_id: &str) -> Result<FinderConfig> {
    config
        .with_registration_requirement(source_id, RegistrationRequirement::SelfServiceApiKey)
        .with_registration(RegistrationRecord::new(
            source_id,
            "desk-owner",
            now(),
            "https://portal.example/api-terms",
            SecretRef::new("QIP_EXAMPLE_PORTAL_KEY")?,
        )?)
}

fn enclave_reaching(host: &str) -> Result<DiscoveryEnclave> {
    Ok(DiscoveryEnclave::new("discovery-eu", Duration::from_mins(5))?.admitting_egress_to(host))
}

fn first(decisions: &[RegistrationDecision]) -> Result<&RegistrationDecision> {
    decisions
        .first()
        .ok_or_else(|| Error::not_found("a decision"))
}

fn rejection_reason(decision: &RegistrationDecision) -> Result<&str> {
    match decision.outcome() {
        DecisionOutcome::Rejected { reason } => Ok(reason),
        other => Err(Error::invalid(format!(
            "expected a rejection and got {}",
            other.as_str()
        ))),
    }
}

// --- classification ---------------------------------------------------------

#[test]
fn an_unprobed_open_api_candidate_is_refused_a_tier_rather_than_read_as_surface_web() -> Result<()>
{
    // The failure: a candidate nobody has probed is routed as an open API
    // because "surface" was the cheapest answer, and turns out to be a login
    // wall. The premise first — this is the candidate that would have been
    // defaulted, one that declares no credential and reads as data.
    let candidate = candidate_with(
        "unprobed",
        "https://open.example/api/quotes",
        open_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    let evidence = TierEvidence::from_candidate(&candidate);
    assert!(evidence.credential_required().is_none());
    assert_eq!(evidence.robots(), RobotsPosture::NotFetched);
    assert!(evidence.unauthenticated_reachability().is_none());

    let refused = SourceTier::classify(&evidence)
        .expect_err("an unprobed candidate was placed in a tier on no evidence");
    assert!(matches!(refused, Error::Invalid(_)));
    assert!(
        refused.message().contains("Probe it first"),
        "the refusal must say what would settle it: {}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_hidden_service_host_is_classified_dark_web_and_never_probed() -> Result<()> {
    // The failure: a `.onion` candidate is HEADed "just to see", which is
    // the one request the dark-web rule exists to prevent. The premise: the
    // suffix is matched as a label, so a clearnet host that merely contains
    // the word is not swept up.
    assert!(SourceTier::is_dark_host("market.onion"));
    assert!(SourceTier::is_dark_host("Exchange.I2P"));
    assert!(!SourceTier::is_dark_host("onion.example"));

    let candidate = candidate_with(
        "hidden",
        "https://market.onion/prices.json",
        open_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    assert_eq!(
        SourceTier::classify(&TierEvidence::from_candidate(&candidate))?,
        SourceTier::DarkWeb
    );

    // A probe scripted to answer, so a finder that contacted the host would
    // find something rather than an error it could hide behind.
    let mut probe = probe_for(
        "https://market.onion/prices.json",
        "market.onion",
        permissive_robots(),
    );
    let mut finder = finder(config()?);
    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let decision = first(&decisions)?;

    assert!(
        probe.calls().is_empty(),
        "the hidden service was contacted: {:?}",
        probe.calls()
    );
    let reason = rejection_reason(decision)?;
    assert!(
        reason.contains("monitoring-only"),
        "the refusal must name the dark-web rule: {reason}"
    );
    assert!(
        decision
            .reasoning()
            .at(LifecycleStage::Classify)
            .iter()
            .any(|finding| finding.starts_with("tier dark_web")),
        "the classification was not recorded: {}",
        decision.reasoning().describe()
    );
    assert!(finder.registry().is_empty());
    Ok(())
}

#[test]
fn a_credentialed_endpoint_is_deep_web_by_its_own_description() -> Result<()> {
    // Login-gated by declaration needs no probe to be placed. The failure:
    // a keyed API waiting on a probe that a deployment without the key can
    // never make, and so never being classified at all.
    let candidate = candidate_with(
        "keyed",
        "https://portal.example/api/filings",
        keyed_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    let evidence = TierEvidence::from_candidate(&candidate);
    assert!(evidence.credential_required().is_some());
    assert_eq!(SourceTier::classify(&evidence)?, SourceTier::DeepWeb);
    Ok(())
}

#[test]
fn a_probe_that_answers_neither_open_nor_gated_leaves_the_tier_undecided_and_the_source_deferred()
-> Result<()> {
    // A 503 says nothing about whether the source wants a credential. The
    // failure: reading it as "not gated, so surface" and routing a source
    // whose access mode nobody has seen.
    let url = "https://flaky.example/api/quotes";
    let candidate = candidate_with(
        "flaky",
        url,
        open_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    let mut probe = InMemoryProbe::new()
        .with_robots("flaky.example", permissive_robots())
        .with_head(
            url,
            HeadResponse {
                status: 503,
                ..ok_head()
            },
        )
        .with_sample(url, sample(QUOTE_PAYLOAD));
    let mut finder = finder(config()?);
    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let decision = first(&decisions)?;
    let DecisionOutcome::Deferred { reason } = decision.outcome() else {
        return Err(Error::invalid(format!(
            "expected a deferral and got {}",
            decision.outcome().as_str()
        )));
    };
    assert!(
        reason.contains("no probe has shown"),
        "the deferral must name the missing evidence: {reason}"
    );
    assert!(finder.registry().is_empty());
    Ok(())
}

// --- the existing sources ---------------------------------------------------

#[test]
fn the_open_api_sources_the_finder_already_registered_still_classify_surface_web_and_route_as_before()
-> Result<()> {
    // Every source the finder registered before the tier existed is an
    // unauthenticated REST endpoint that answered 200 with a served
    // robots.txt. The failure: the tier gate refusing them, or placing them
    // anywhere but the surface web with the `api` mode.
    let url = "https://a.example/data/prices.json";
    let candidate = candidate_with(
        "alpha",
        url,
        open_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    let mut probe = probe_for(url, "a.example", permissive_robots());
    let mut finder = finder(config()?);
    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let decision = first(&decisions)?;

    assert!(
        decision.is_registered(),
        "the surface-web API was not registered: {}",
        decision.reasoning().describe()
    );
    let registered = finder
        .registered("alpha")
        .ok_or_else(|| Error::not_found("the registered source"))?;
    assert_eq!(registered.source().tier(), Some(SourceTier::SurfaceWeb));
    let route = decision.reasoning().at(LifecycleStage::Route);
    assert!(
        route
            .iter()
            .any(|finding| finding.contains("tier surface_web via access mode api")),
        "the routing decision does not record the tier and mode: {route:?}"
    );
    Ok(())
}

#[test]
fn an_endpoint_that_declares_no_credential_but_is_turned_away_is_deep_web_and_refused_not_guessed()
-> Result<()> {
    // The candidate's claim of open access was wrong. The failure: the
    // finder inventing a `registered` mode to make the source fit, or
    // scoring it as an API it cannot reach.
    let url = "https://gated.example/api/quotes";
    let candidate = candidate_with(
        "gated",
        url,
        open_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    let mut probe = InMemoryProbe::new()
        .with_robots("gated.example", permissive_robots())
        .with_head(
            url,
            HeadResponse {
                status: 401,
                ..ok_head()
            },
        )
        .with_sample(url, sample(QUOTE_PAYLOAD));
    let mut finder = finder(config()?);
    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let decision = first(&decisions)?;

    // Premise: the score alone would have collected it, so the rejection
    // below can only be the tier's.
    let scores = decision
        .scores()
        .ok_or_else(|| Error::not_found("scores"))?;
    assert!(
        scores.composite() >= qip_data_finder::scoring::Routing::COLD_THRESHOLD,
        "the composite {} would have rejected it before the tier was asked",
        scores.composite()
    );
    assert!(
        decision
            .reasoning()
            .at(LifecycleStage::Classify)
            .iter()
            .any(|finding| finding.starts_with("tier deep_web")),
        "{}",
        decision.reasoning().describe()
    );
    let reason = rejection_reason(decision)?;
    assert!(
        reason.contains("gated in fact"),
        "the refusal must say the claim was wrong: {reason}"
    );
    assert!(finder.registry().is_empty());
    Ok(())
}

// --- access modes -----------------------------------------------------------

#[test]
fn a_credential_reference_is_a_name_and_refuses_anything_shaped_like_a_value() -> Result<()> {
    // The failure: a key pasted where a name was wanted, serialised into
    // every decision record. The premise: a name is accepted.
    assert_eq!(
        CredentialReference::new("vendor-account")?.name(),
        "vendor-account"
    );
    for value in [
        "",
        "sk_live_ABC123",
        "AKIAIOSFODNN7EXAMPLE",
        "-----BEGIN PRIVATE KEY-----",
        "token=abc",
        "a".repeat(CredentialReference::MAX_LENGTH + 1).as_str(),
    ] {
        let refused = CredentialReference::new(value)
            .expect_err("a credential value was accepted as a reference");
        assert!(matches!(refused, Error::Invalid(_)));
        assert!(
            refused.message().contains("never carries the value")
                || refused.message().contains("cannot be provided"),
            "the refusal must say a reference is a name: {}",
            refused.message()
        );
    }
    Ok(())
}

#[test]
fn a_registered_mode_needs_a_named_credential_the_deployment_projects_as_a_file() -> Result<()> {
    // The failure: a free-registration source registered with no credential
    // named, so the first poll fails in production with nothing in the
    // decision record saying what was missing.
    let url = "https://portal.example/api/filings";
    let candidate = candidate_with(
        "registered-portal",
        url,
        keyed_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;

    let mut probe = probe_for(url, "portal.example", permissive_robots());
    let mut unnamed = finder(config()?);
    let decisions = unnamed.assess(vec![candidate.clone()], &mut probe, now())?;
    let reason = rejection_reason(first(&decisions)?)?;
    assert!(
        reason.contains("no credential reference is named for `portal.example`"),
        "the refusal must name the host and the remedy: {reason}"
    );
    assert!(unnamed.registry().is_empty());

    let mut named = finder(registered_by_owner(
        config()?
            .with_credential_reference("portal.example", CredentialReference::new("portal-key")?),
        "registered-portal",
    )?);
    let decisions = named.assess(vec![candidate], &mut probe, now())?;
    let decision = first(&decisions)?;
    assert!(
        decision.is_registered(),
        "{}",
        decision.reasoning().describe()
    );
    let route = decision.reasoning().at(LifecycleStage::Route);
    assert!(
        route.iter().any(|finding| {
            finding.contains("access mode registered")
                && finding.contains("credential `portal-key` projected as a file")
        }),
        "the routing decision does not record the mode and the credential name: {route:?}"
    );
    assert_eq!(
        named
            .registered("registered-portal")
            .and_then(|entry| entry.source().tier()),
        Some(SourceTier::DeepWeb)
    );
    Ok(())
}

#[test]
fn a_licensed_mode_is_admissible_only_under_a_declared_posture_naming_the_same_licence()
-> Result<()> {
    // The failure: a paid subscription reached under terms nobody read, or
    // under a licence identifier that disagrees with the one the posture
    // declares — two claims about one agreement.
    let adapter = DeepWebAdapter::new(
        "paid-terminal",
        "terminal.example",
        SourceTier::DeepWeb,
        AccessMode::Licensed {
            credential: CredentialReference::new("terminal-account")?,
            licence: "terminal-subscription-2026".to_string(),
        },
    )?;
    // Premise: under the declared licence it names, the mode is admissible.
    adapter.admissible(
        &LicensingPosture::declared(SourceLicense::new(
            "terminal-subscription-2026",
            [Usage::Derive],
        )?),
        None,
    )?;

    let undeclared = adapter
        .admissible(&LicensingPosture::Undetermined, None)
        .expect_err("a licensed subscription was admitted with no licence read");
    assert!(
        undeclared.message().contains("not declared"),
        "{}",
        undeclared.message()
    );
    let ambiguous = adapter
        .admissible(&LicensingPosture::ambiguous("a terms page"), None)
        .expect_err("a licensed subscription was admitted on ambiguous terms");
    assert!(ambiguous.message().contains("not declared"));

    let other = adapter
        .admissible(
            &LicensingPosture::declared(SourceLicense::new("some-other-terms", [Usage::Derive])?),
            None,
        )
        .expect_err("a licence disagreement was admitted");
    assert!(other.message().contains("disagree"), "{}", other.message());
    Ok(())
}

#[test]
fn a_paid_credentialed_source_routes_as_licensed_under_its_declared_licence() -> Result<()> {
    // The finder's own mapping: a credential plus a fee is the licensed
    // mode, and the licence it names is the posture's. The failure: a paid
    // source routed as a free registration, with no licence recorded on it.
    let url = "https://terminal.example/api/quotes";
    let candidate = candidate_with(
        "paid",
        url,
        keyed_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::new(
            Decimal::from_int(500),
            Decimal::ZERO,
            u64::MAX,
            Currency::EUR,
        )?,
    )?;
    let mut probe = probe_for(url, "terminal.example", permissive_robots());
    let mut finder = finder(registered_by_owner(
        config()?.with_credential_reference(
            "terminal.example",
            CredentialReference::new("terminal-account")?,
        ),
        "paid",
    )?);
    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let decision = first(&decisions)?;
    assert!(
        decision.is_registered(),
        "{}",
        decision.reasoning().describe()
    );
    let route = decision.reasoning().at(LifecycleStage::Route);
    assert!(
        route
            .iter()
            .any(|finding| finding.contains("tier deep_web via access mode licensed")),
        "{route:?}"
    );
    Ok(())
}

#[test]
fn a_rendered_page_is_refused_without_an_enclave_and_admitted_only_inside_one_that_reaches_it()
-> Result<()> {
    // The failure: a page's client-side code executed on the box that holds
    // the trading credentials, because no enclave was configured and the
    // finder went ahead anyway.
    let url = "https://dashboard.example/prices";
    let candidate = candidate_with(
        "rendered",
        url,
        html_page(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    let mut probe = probe_for(url, "dashboard.example", permissive_robots());

    let mut bare = finder(config()?);
    let decisions = bare.assess(vec![candidate.clone()], &mut probe, now())?;
    let decision = first(&decisions)?;
    // Premise: the score would have collected it.
    let scores = decision
        .scores()
        .ok_or_else(|| Error::not_found("scores"))?;
    assert!(scores.composite() >= qip_data_finder::scoring::Routing::COLD_THRESHOLD);
    let reason = rejection_reason(decision)?;
    assert!(
        reason.contains("`rendered` access mode") && reason.contains("no enclave is configured"),
        "the refusal must name the mode and the missing enclave: {reason}"
    );
    assert!(bare.registry().is_empty());

    // An enclave that cannot reach the source is not the enclave the fetch
    // would run in.
    let mut elsewhere =
        finder(config()?.with_discovery_enclave(enclave_reaching("other.example")?));
    let decisions = elsewhere.assess(vec![candidate.clone()], &mut probe, now())?;
    let reason = rejection_reason(first(&decisions)?)?;
    assert!(
        reason.contains("does not admit `dashboard.example`"),
        "{reason}"
    );

    let mut enclosed =
        finder(config()?.with_discovery_enclave(enclave_reaching("dashboard.example")?));
    let decisions = enclosed.assess(vec![candidate], &mut probe, now())?;
    let decision = first(&decisions)?;
    assert!(
        decision.is_registered(),
        "{}",
        decision.reasoning().describe()
    );
    let route = decision.reasoning().at(LifecycleStage::Route);
    assert!(
        route.iter().any(|finding| {
            finding.contains("access mode rendered")
                && finding.contains("inside enclave `discovery-eu`")
        }),
        "the routing decision does not name the enclave: {route:?}"
    );
    Ok(())
}

#[test]
fn an_open_query_interface_is_told_apart_from_a_rendered_page_by_its_query_string_and_needs_no_enclave()
-> Result<()> {
    // The failure: every extracted page treated as rendered, so a plain
    // search form — the blueprint's open-query mode, which needs a rate
    // limit and robots.txt and nothing else — is refused for want of an
    // enclave it does not need.
    let url = "https://registry.example/search?q=EU0001";
    let candidate = candidate_with(
        "open-query",
        url,
        html_page(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    let mut probe = probe_for(url, "registry.example", permissive_robots());
    let mut finder =
        finder(config()?.with_default_rate_limit(RateLimit::new(10, Duration::from_mins(1))?));
    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let decision = first(&decisions)?;
    assert!(
        decision.is_registered(),
        "{}",
        decision.reasoning().describe()
    );
    let route = decision.reasoning().at(LifecycleStage::Route);
    assert!(
        route
            .iter()
            .any(|finding| finding.contains("tier deep_web via access mode open_query")),
        "{route:?}"
    );
    Ok(())
}

#[test]
fn a_bulk_extract_needs_an_enclave_and_a_retention_bound() -> Result<()> {
    // The failure: a whole extract fetched onto the trading host and kept
    // forever — the accumulation pass-through forbids, on the box it
    // forbids it on.
    let unbounded = BulkCadence::new(Duration::from_days(1), Duration::ZERO)
        .expect_err("a bulk cadence with no retention bound was accepted");
    assert!(unbounded.message().contains("retention bound"));

    let adapter = DeepWebAdapter::new(
        "eod-drop",
        "vendor.example",
        SourceTier::DeepWeb,
        AccessMode::Bulk {
            cadence: BulkCadence::new(Duration::from_days(1), Duration::from_days(7))?,
        },
    )?;
    let posture = licensed_for(&[Usage::Derive])?;
    let refused = adapter
        .admissible(&posture, None)
        .expect_err("a bulk fetch was admitted with no enclave");
    assert!(
        refused.message().contains("`bulk` access mode")
            && refused.message().contains("no enclave is configured"),
        "{}",
        refused.message()
    );
    adapter.admissible(&posture, Some(&enclave_reaching("vendor.example")?))?;

    // And through the finder, the unauthenticated bulk file maps to the
    // bulk mode rather than to the API the surface web would get.
    let url = "https://vendor.example/drops/eod.csv";
    let candidate = candidate_with(
        "eod-drop",
        url,
        AccessMechanism::BulkFile {
            format: FileFormat::Csv,
            published_every: Duration::from_days(1),
            auth: AuthRequirement::None,
        },
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    let mut probe = InMemoryProbe::new()
        .with_head(url, ok_head())
        .with_sample(url, sample(QUOTE_PAYLOAD));
    let mut finder = finder(config()?);
    let decisions = finder.assess(vec![candidate], &mut probe, now())?;
    let reason = rejection_reason(first(&decisions)?)?;
    assert!(reason.contains("`bulk` access mode"), "{reason}");
    Ok(())
}

// --- the dark web -----------------------------------------------------------

#[test]
fn the_dark_web_refuses_every_access_mode_by_name() -> Result<()> {
    // The failure: a seventh mode, or a refactor of the six, that reaches a
    // hidden service because the refusal matched on a subset. Each mode is
    // driven through with everything else it could need supplied, so the
    // only rule that can fire is the tier's.
    let posture = LicensingPosture::declared(SourceLicense::new("terms", [Usage::Derive])?);
    let enclave = enclave_reaching("market.onion")?;
    let modes = [
        AccessMode::OpenQuery {
            rate_limit: RateLimit::new(1, Duration::from_mins(1))?,
        },
        AccessMode::Api,
        AccessMode::Registered {
            credential: CredentialReference::new("account")?,
        },
        AccessMode::Licensed {
            credential: CredentialReference::new("account")?,
            licence: "terms".to_string(),
        },
        AccessMode::Rendered {
            budget: RenderingBudget::new(1, Duration::from_secs(1))?,
        },
        AccessMode::Bulk {
            cadence: BulkCadence::new(Duration::from_days(1), Duration::from_days(1))?,
        },
    ];
    assert_eq!(modes.len(), 6, "the blueprint names six access modes");

    for mode in modes {
        // Premise: on the deep web, with the same provisions, this mode is
        // admissible — so the refusal below is the tier's and nothing else's.
        DeepWebAdapter::new("hidden", "market.onion", SourceTier::DeepWeb, mode.clone())?
            .admissible(&posture, Some(&enclave))?;

        let name = mode.as_str();
        let refused =
            match DeepWebAdapter::new("hidden", "market.onion", SourceTier::DarkWeb, mode)?
                .admissible(&posture, Some(&enclave))
            {
                Ok(()) => {
                    return Err(Error::invalid(format!(
                        "the `{name}` mode was admitted on the dark web"
                    )));
                }
                Err(error) => error,
            };
        assert!(matches!(refused, Error::Denied(_)));
        assert!(
            refused
                .message()
                .contains(&format!("`{name}` access mode is refused")),
            "the refusal must name the mode it refuses: {}",
            refused.message()
        );
    }
    assert!(!SourceTier::DarkWeb.feeds_training());
    assert!(SourceTier::DeepWeb.feeds_training());
    Ok(())
}

#[test]
fn defensive_monitoring_names_what_is_watched_and_watches_for_something() -> Result<()> {
    // The failure: a monitoring record that watches for nothing, reading as
    // monitoring. And the shape: identifiers and credential *shapes*, so the
    // watcher holds nothing that could itself leak.
    let empty = DefensiveMonitoring::new([], [], [])
        .expect_err("a defensive monitoring record watching for nothing was accepted");
    assert!(empty.message().contains("monitoring in name only"));

    let watch = DefensiveMonitoring::new(
        ["qip-api".to_string(), "venue-account-label".to_string()],
        ["prefix `qip_` then 40 hex characters".to_string()],
        ["PEOS Quantum".to_string()],
    )?;
    assert_eq!(watch.tier(), SourceTier::DarkWeb);
    assert_eq!(watch.own_identifiers().len(), 2);
    assert!(watch.describe().contains("never content"));
    Ok(())
}

#[test]
fn an_enclave_holds_no_trading_zone_credential_and_admits_hosts_exactly() -> Result<()> {
    // The failure: an enclave that admitted `example.com` and thereby every
    // subdomain nobody named, or one with nowhere to record that it holds
    // no capital-moving credential.
    let unnamed = DiscoveryEnclave::new("  ", Duration::from_mins(1))
        .expect_err("an unnamed enclave was accepted");
    assert!(unnamed.message().contains("named"));
    let unbounded = DiscoveryEnclave::new("discovery", Duration::ZERO)
        .expect_err("an enclave with no runtime bound was accepted");
    assert!(unbounded.message().contains("bound"));

    let enclave = enclave_reaching("Dashboard.Example")?;
    assert!(enclave.permits_egress_to("dashboard.example"));
    assert!(!enclave.permits_egress_to("api.dashboard.example"));
    assert!(!enclave.permits_egress_to("example"));
    assert!(!enclave.holds_trading_zone_credential());
    assert_eq!(enclave.max_runtime(), Duration::from_mins(5));
    Ok(())
}

// --- §7.6.3: a human approves a category once ---------------------------------

#[test]
fn a_deep_web_adapter_in_no_category_is_held_rather_than_promoted_under_the_nearest_one()
-> Result<()> {
    // The failure: §7.6.3's "adapters within an approved category are
    // promoted automatically" applied to an adapter nobody categorised, by
    // promoting it under whichever category was nearest — a category
    // assigned because something had to be, read downstream as a finding.
    let url = "https://portal.example/api/filings";
    let candidate = uncategorised_candidate(
        "uncategorised-portal",
        url,
        keyed_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    // Premise: the same source, categorised, registers on this config — so
    // the deferral below is the missing category and nothing upstream.
    let mut probe = probe_for(url, "portal.example", permissive_robots());
    let mut desk = finder(registered_by_owner(
        config()?
            .with_credential_reference("portal.example", CredentialReference::new("portal-key")?),
        "uncategorised-portal",
    )?);
    let decisions = desk.assess(
        vec![
            candidate
                .clone()
                .with_content_signal(ContentSignal::RegulatoryFiling),
        ],
        &mut probe,
        now(),
    )?;
    assert!(
        first(&decisions)?.is_registered(),
        "{}",
        first(&decisions)?.reasoning().describe()
    );

    let mut probe = probe_for(url, "portal.example", permissive_robots());
    let mut desk = finder(registered_by_owner(
        config()?
            .with_credential_reference("portal.example", CredentialReference::new("portal-key")?),
        "uncategorised-portal",
    )?);
    let decisions = desk.assess(vec![candidate], &mut probe, now())?;
    let decision = first(&decisions)?;
    let DecisionOutcome::Deferred { reason } = decision.outcome() else {
        return Err(Error::invalid(format!(
            "an uncategorised deep-web source was not deferred: {}",
            decision.reasoning().describe()
        )));
    };
    assert!(
        reason.contains("in no category") && reason.contains("with_content_signal"),
        "the deferral must name what to declare: {reason}"
    );
    assert!(desk.registry().is_empty());

    // The clause governs deep-web adapters only; a surface-web bulk drop
    // builds an adapter for the enclave rules and is not held on an approval
    // the blueprint never asked for.
    let surface = DeepWebAdapter::new(
        "eod-drop",
        "vendor.example",
        SourceTier::SurfaceWeb,
        AccessMode::Bulk {
            cadence: BulkCadence::new(Duration::from_days(1), Duration::from_days(7))?,
        },
    )?;
    assert_eq!(
        surface.promotion(desk.approved_categories())?,
        Promotion::NotGoverned
    );
    Ok(())
}

#[test]
fn a_human_approves_a_category_once_and_adapters_within_it_are_promoted_automatically() -> Result<()>
{
    // The failure: the governance clause as a sentence in a table. Nothing
    // linked an adapter to its category, so there was nothing a human could
    // approve once and nothing that could be promoted on that approval — a
    // deep-web source either registered on its score or did not, and the
    // person the blueprint puts in the loop was never asked.
    let url = "https://portal.example/api/filings";
    let candidate = candidate_with(
        "first-portal",
        url,
        keyed_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    let mut probe = probe_for(url, "portal.example", permissive_robots());
    let mut desk = finder(registered_by_owner(
        unapproved_config()?
            .with_credential_reference("portal.example", CredentialReference::new("portal-key")?),
        "first-portal",
    )?);
    assert!(desk.approved_categories().is_empty());

    let decisions = desk.assess(vec![candidate.clone()], &mut probe, now())?;
    let decision = first(&decisions)?;
    // Premise: legality permitted collection and the score would have
    // collected it — the deferral is the approval and nothing else.
    assert!(
        decision
            .legality()
            .is_some_and(|legality| legality.overall().is_permitted()),
        "{}",
        decision.reasoning().describe()
    );
    assert!(
        decision.scores().is_some_and(
            |scores| scores.composite() >= qip_data_finder::scoring::Routing::COLD_THRESHOLD
        ),
        "{}",
        decision.reasoning().describe()
    );
    let DecisionOutcome::Deferred { reason } = decision.outcome() else {
        return Err(Error::invalid(format!(
            "a deep-web source in an unapproved category was not deferred: {}",
            decision.reasoning().describe()
        )));
    };
    assert!(
        reason.contains("`regulatory_and_legal` category, which no human has approved")
            && reason.contains("CategoryApproval"),
        "the deferral must name the category and the approval it needs: {reason}"
    );
    assert!(desk.registry().is_empty());

    // A person approves the category once, on the running finder — the
    // seam `Platform::approve_source_category` reaches.
    desk.approve_category(filings_approved_by_desk_owner()?)?;
    let mut probe = probe_for(url, "portal.example", permissive_robots());
    let decisions = desk.assess(vec![candidate], &mut probe, now())?;
    let decision = first(&decisions)?;
    assert!(
        decision.is_registered(),
        "{}",
        decision.reasoning().describe()
    );
    let route = decision.reasoning().at(LifecycleStage::Route);
    assert!(
        route.iter().any(|finding| {
            finding.contains(
                "promoted automatically: category `regulatory_and_legal` approved \
                              by desk-owner",
            )
        }),
        "the promotion must record whose approval it ran on: {route:?}"
    );

    // A second adapter in the same category is promoted with no further
    // review — "once" is the whole point of approving the category rather
    // than the source.
    let second_url = "https://registry.example/api/filings";
    let second = candidate_with(
        "second-portal",
        second_url,
        keyed_rest(),
        licensed_for(&[Usage::Derive])?,
        SourceCost::free(Currency::EUR),
    )?;
    let mut probe = probe_for(second_url, "registry.example", permissive_robots());
    let mut desk = finder(registered_by_owner(
        unapproved_config()?
            .with_credential_reference(
                "registry.example",
                CredentialReference::new("registry-key")?,
            )
            .with_category_approval(filings_approved_by_desk_owner()?)?,
        "second-portal",
    )?);
    let decisions = desk.assess(vec![second], &mut probe, now())?;
    assert!(
        first(&decisions)?.is_registered(),
        "{}",
        first(&decisions)?.reasoning().describe()
    );

    // And the approval is given once: a second for the same category is
    // refused naming the first, rather than overwriting whose name is on it.
    let refused = desk
        .approve_category(CategoryApproval::new(
            SourceCategory::RegulatoryAndLegal,
            "someone-else",
            now(),
            "a second review",
        )?)
        .expect_err("a second approval of an approved category was accepted");
    assert!(
        refused.message().contains("already approved")
            && refused.message().contains("approved by desk-owner"),
        "{}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_category_approval_has_a_persons_name_and_a_basis_on_it() -> Result<()> {
    // The failure: an approval nobody signed, which is an approval nobody
    // can be asked about when the category turns out to have been the wrong
    // one to admit.
    let unsigned = CategoryApproval::new(SourceCategory::Marketplace, "  ", now(), "a review")
        .expect_err("an approval with no approver was accepted");
    assert!(unsigned.message().contains("must name the person"));
    let baseless = CategoryApproval::new(SourceCategory::Marketplace, "desk-owner", now(), "")
        .expect_err("an approval citing nothing was accepted");
    assert!(baseless.message().contains("must cite what was reviewed"));
    let approval =
        CategoryApproval::new(SourceCategory::Marketplace, "desk-owner", now(), "a review")?;
    assert_eq!(approval.category(), SourceCategory::Marketplace);
    assert_eq!(approval.approved_by(), "desk-owner");
    Ok(())
}
