//! The eight registries are versioned: the composite of R12-COVERAGE-7170.
//!
//! "Source, ontology, memory, brain, tool, model, research and capability
//! registries are versioned and extensible rather than hard-coded." Each
//! registry had, at best, a test of its own in its own crate, and four had
//! none: a tool's scope was widened in place, a brain's `version` field had
//! no door that could move it, a memory entry re-admitted under a held id
//! sat beside the first with nothing to say which was current, and a model
//! card re-registered under a promoted version swapped the artifact the
//! version named. No single place said what "versioned" means, so each
//! registry could drift from the rule and leave every other test green.
//!
//! The rule, driven here through each registry's public door:
//!
//! * a first registration is admitted (asserted as the premise, so the
//!   refusals below are the rule and not a registry that refuses everything);
//! * a successor is admitted as a new version, and the version it follows is
//!   still readable afterwards where the registry keeps history, or named by
//!   the successor where it does not (the source and brain registries, ADR
//!   0410 decision 2);
//! * a stale, equal or duplicate version is refused, and the refusal moves
//!   nothing.
//!
//! Two registries meet the last clause by construction rather than by
//! comparison, and the tests say so where it happens. The tool registry
//! takes no version from a caller: it numbers a promotion that widens the
//! scope and mints nothing for one that does not. The model registry's
//! version is a string and is not ordered: `name@version` names one
//! artifact, a successor is a new reference beside the old one, and a card
//! that would rebind a version to other bytes is refused.
//!
//! No figure bounds a history here: a registry whose history could grow with
//! uptime keeps the version in force and the lineage, not every predecessor.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_agents::memory::MemoryRegistry;
use qip_agents::registries::{CapabilityRegistry, CitedEvaluation, OntologyRegistry};
use qip_agents::research::{ResearchRegistry, Verdict};
use qip_agents::tools::{ToolKind, ToolName, ToolPermission, ToolRegistry};
use qip_ai::registry::{ModelCard, ModelRegistry};
use qip_contracts::expansion::{
    BrainSpec, CapabilityEntry, CapabilityVerb, MemoryEntry, MemoryKind, OntologyTypeFamily,
    OntologyTypeSpec, RetentionPolicy, ValuePolicy,
};
use qip_contracts::governance::Usage;
use qip_core::{Currency, Duration, ModelId, Timestamp};
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_data_finder::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_data_finder::{DataFinder, FinderConfig};
use qip_deepbrain::brains::{BrainRegistry, version_id};
use qip_events::Topic;
use qip_financial::asset_class::AssetClass;
use std::collections::BTreeSet;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

// --- source -----------------------------------------------------------------

const SOURCE_URL: &str = "https://example.com/v1/quotes";

fn source_candidate(id: &str) -> SourceCandidate {
    let endpoint = SourceEndpoint::parse(
        SOURCE_URL,
        AccessMechanism::Rest {
            auth: AuthRequirement::None,
            incremental_parameter: Some("since".to_string()),
            page_size: 500,
        },
    )
    .unwrap();
    let coverage = SourceCoverage::new(
        [AssetClass::Equity],
        [SourceRegion::Europe],
        ["EU0001".to_string()],
        UpdateFrequency::Minutely,
    )
    .unwrap()
    .with_history_from(now().saturating_sub(Duration::from_days(3_650)));
    SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd").unwrap(),
        endpoint,
        coverage,
        LicensingPosture::declared(
            SourceLicense::new("vendor-terms-2026", [Usage::Derive]).unwrap(),
        ),
        SourceCost::free(Currency::EUR),
        SourceRegion::Europe,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        now(),
    )
    .unwrap()
}

fn source_probe() -> InMemoryProbe {
    InMemoryProbe::new()
        .with_robots(
            "example.com",
            RobotsFetch::Served {
                body: "User-agent: *\nAllow: /\n".to_string(),
                latency: Duration::from_millis(12),
            },
        )
        .with_head(
            SOURCE_URL,
            HeadResponse {
                status: 200,
                content_type: Some("application/json".to_string()),
                content_length: Some(512),
                last_modified: Some(now()),
                latency: Duration::from_millis(40),
            },
        )
        .with_sample(
            SOURCE_URL,
            PayloadSample {
                body: qip_core::untrusted::Untrusted::new(
                    "a scripted sample",
                    r#"{"symbol":"EU0001","bid":10.25,"ask":10.27,"volume":41000}"#.to_string(),
                ),
                media_type: "application/json".to_string(),
                payload_at: Some(now()),
                latency: Duration::from_millis(55),
            },
        )
}

fn finder() -> DataFinder {
    DataFinder::new(
        FinderConfig::new("qip-data-finder/1.0", Usage::Derive, "market-data", 7).unwrap(),
    )
}

/// A re-assessment older than the held registration once replaced it, so a
/// late-arriving stale probe could hand the router a source's past routing
/// and entitlements as its present ones.
#[test]
fn a_source_re_registered_later_is_the_next_version_and_an_older_one_is_refused() {
    let mut finder = finder();
    let mut probe = source_probe();

    let first = finder
        .assess(vec![source_candidate("versioned")], &mut probe, now())
        .unwrap();
    // Premise: the source registers, as its first version.
    assert!(first[0].is_registered(), "{:?}", first[0].outcome());
    assert_eq!(finder.registered("versioned").map(|r| r.version()), Some(1));

    let later = now().saturating_add(Duration::from_hours(1));
    let second = finder
        .assess(vec![source_candidate("versioned")], &mut probe, later)
        .unwrap();
    assert!(second[0].is_registered(), "{:?}", second[0].outcome());
    assert_eq!(finder.registered("versioned").map(|r| r.version()), Some(2));
    assert_eq!(
        finder.registered("versioned").map(|r| r.registered_at()),
        Some(later)
    );

    // Stale: an assessment older than the held registration.
    assert!(
        finder
            .assess(vec![source_candidate("versioned")], &mut probe, now())
            .is_err(),
        "a registration older than the held one replaced it"
    );
    assert_eq!(finder.registered("versioned").map(|r| r.version()), Some(2));
    assert_eq!(
        finder.registered("versioned").map(|r| r.registered_at()),
        Some(later),
        "the refused registration replaced the one in force"
    );
}

// --- ontology ---------------------------------------------------------------

fn ontology_type(version: u32) -> OntologyTypeSpec {
    OntologyTypeSpec {
        family: OntologyTypeFamily::Event,
        name: "supply-shock".to_string(),
        version,
        schema: "{\"fields\":[\"region\"]}".to_string(),
        description: "a disruption to supply".to_string(),
    }
}

/// A late-arriving old definition once moved `get_latest` back, replacing the
/// schema every detector reads with one already superseded.
#[test]
fn an_ontology_type_extended_is_the_next_version_with_its_predecessor_readable_and_a_version_not_above_the_latest_is_refused()
 {
    let mut registry = OntologyRegistry::new();
    registry.register(ontology_type(1)).unwrap();
    // Premise: the type is held at version 1.
    assert_eq!(
        registry
            .get_latest(OntologyTypeFamily::Event, "supply-shock")
            .map(|t| t.version),
        Some(1)
    );

    let next = registry
        .extend(
            OntologyTypeFamily::Event,
            "supply-shock",
            "{\"fields\":[\"region\",\"commodity\"]}",
            "a disruption to supply, by commodity",
            &CitedEvaluation {
                id: "eval-ontology-1".to_string(),
                passed: true,
            },
        )
        .unwrap();
    assert_eq!(next, 2);
    assert_eq!(
        registry.get_type(OntologyTypeFamily::Event, "supply-shock", 1),
        Some(ontology_type(1)),
        "version 1 is no longer readable once version 2 was admitted"
    );

    for stale in [1, 2] {
        assert!(
            registry.register(ontology_type(stale)).is_err(),
            "version {stale} was accepted over latest 2"
        );
    }
    let latest = registry
        .get_latest(OntologyTypeFamily::Event, "supply-shock")
        .unwrap();
    assert_eq!(latest.version, 2);
    assert!(latest.schema.contains("commodity"), "{latest:?}");
}

// --- memory -----------------------------------------------------------------

fn memory_entry(content: &str) -> MemoryEntry {
    MemoryEntry {
        id: "abstraction/credit-leads-equity".to_string(),
        kind: MemoryKind::CompressedAbstraction,
        content: content.to_string(),
        retention_policy: RetentionPolicy {
            max_age_ms: 86_400_000,
            description: "one day".to_string(),
        },
        value_policy: ValuePolicy {
            importance_threshold: 0.3,
            rationale: "calibration surprise".to_string(),
        },
        created_at_ms: 1_760_000_000_000,
        importance: 0.9,
        tags: vec!["credit".to_string()],
    }
}

/// Two entries under one id were both held with nothing to say which was
/// current, so a reader taking the first match and one taking the last read
/// different memories under the same name.
#[test]
fn a_memory_entry_re_admitted_is_the_next_version_with_its_predecessor_readable_and_a_version_not_above_the_latest_is_refused()
 {
    const ID: &str = "abstraction/credit-leads-equity";
    let mut registry = MemoryRegistry::standard();
    registry
        .admit(memory_entry("spreads lead by a day"))
        .unwrap();
    // Premise: the entry is held, as version 1.
    assert_eq!(registry.latest_version(ID), Some(1));

    registry
        .admit(memory_entry("spreads lead by two days in stress"))
        .unwrap();
    assert_eq!(registry.latest_version(ID), Some(2));
    assert_eq!(
        registry.entry_at(ID, 1).map(|e| e.content.as_str()),
        Some("spreads lead by a day"),
        "version 1 is no longer readable once version 2 was admitted"
    );
    assert_eq!(
        registry.entry_at(ID, 2).map(|e| e.content.as_str()),
        Some("spreads lead by two days in stress")
    );
    assert_eq!(registry.refused(), 0, "premise: nothing refused yet");

    for (n, stale) in [0, 1, 2].into_iter().enumerate() {
        assert!(
            registry
                .admit_version(memory_entry("a replayed old revision"), stale)
                .is_err(),
            "version {stale} was accepted over latest 2"
        );
        assert_eq!(registry.refused(), n + 1, "the refusal was not counted");
    }
    assert_eq!(registry.latest_version(ID), Some(2));
    assert_eq!(registry.entries().count(), 2, "a refused entry was held");

    // The control: the next version through the same door is admitted.
    registry
        .admit_version(memory_entry("spreads lead by three days"), 3)
        .unwrap();
    assert_eq!(registry.latest_version(ID), Some(3));
}

// --- brain ------------------------------------------------------------------

fn brain(version: u32, parent: Option<String>, tool: &str) -> BrainSpec {
    BrainSpec {
        id: "analyst/credit".to_string(),
        name: "Credit Analyst".to_string(),
        competency_domains: vec!["credit".to_string()],
        tools: vec![tool.to_string()],
        source_permissions: vec!["desk-view:read".to_string()],
        memory_scope: "one panel convening".to_string(),
        model_stack: vec!["in-tree rules; no model consulted".to_string()],
        evaluations: vec!["registry_versioning".to_string()],
        abstention_rules: vec!["abstains on an unrated issuer".to_string()],
        authority_envelope: "advisory_only".to_string(),
        calibration: 0.0,
        version,
        parent_version_id: parent,
    }
}

/// `BrainRegistry::register` refused a second spec and told the caller to
/// give the successor a new version, and no door admitted one: a brain whose
/// tools changed could only be recorded by rebuilding the registry without
/// its past.
#[test]
fn a_brain_successor_naming_the_held_version_as_its_parent_is_admitted_and_any_other_is_refused() {
    const ID: &str = "analyst/credit";
    let mut registry = BrainRegistry::new();
    registry.register(brain(1, None, "screen")).unwrap();
    // Premise: the brain is held at version 1.
    assert_eq!(registry.latest_version(ID), Some(1));

    let admitted = registry
        .succeed(brain(2, Some(version_id(ID, 1)), "screen-and-model"))
        .unwrap();
    assert_eq!(admitted, 2);
    assert_eq!(registry.get(ID).map(|b| b.version), Some(2));
    assert_eq!(
        registry.get(ID).and_then(|b| b.parent_version_id.clone()),
        Some(version_id(ID, 1)),
        "the version in force does not name the version it succeeded"
    );
    assert_eq!(registry.len(), 1, "a successor is not a second brain");

    let parent = Some(version_id(ID, 2));
    for (what, refused) in [
        ("a stale version", brain(1, None, "x")),
        (
            "the held version again",
            brain(2, Some(version_id(ID, 1)), "x"),
        ),
        ("a skipped version", brain(4, parent.clone(), "x")),
        ("a successor naming no parent", brain(3, None, "x")),
        (
            "a successor naming a parent it does not succeed",
            brain(3, Some(version_id(ID, 1)), "x"),
        ),
    ] {
        assert!(registry.succeed(refused).is_err(), "{what} was admitted");
        assert_eq!(
            registry.latest_version(ID),
            Some(2),
            "{what} moved the brain"
        );
    }
    assert!(
        registry.register(brain(3, parent.clone(), "x")).is_err(),
        "a successor entered through `register`, around the lineage check"
    );
    assert_eq!(
        registry.get(ID).map(|b| b.tools.clone()),
        Some(vec!["screen-and-model".to_string()])
    );

    // The control: the well-formed third version is admitted.
    assert_eq!(registry.succeed(brain(3, parent, "x")).unwrap(), 3);
}

// --- tool -------------------------------------------------------------------

/// `promote` widened a tool's scope in place: the registry could say what a
/// tool may do now and nothing about what it could do when a decision used
/// it, or which evaluation widened it.
#[test]
fn a_tool_promotion_is_the_next_version_with_its_predecessor_readable_and_a_duplicate_registration_or_grant_mints_none()
 {
    let name = ToolName::parse("parser/filings").unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(name.clone(), ToolKind::Parser).unwrap();
    // Premise: the tool is held, read-only, at version 1.
    assert_eq!(registry.version(&name), Some(1));
    assert_eq!(
        registry.get(&name).map(|t| t.scope().clone()),
        Some(BTreeSet::from([ToolPermission::Read]))
    );

    registry
        .promote(&name, ToolPermission::Write, "eval-2026-11-11")
        .unwrap();
    assert_eq!(registry.version(&name), Some(2));
    let predecessor = registry
        .at_version(&name, 1)
        .expect("version 1 is no longer readable once the promotion was recorded");
    assert_eq!(
        predecessor.spec().scope(),
        &BTreeSet::from([ToolPermission::Read]),
        "the promotion rewrote the version it superseded"
    );
    assert_eq!(predecessor.evaluation(), None);
    let current = registry.at_version(&name, 2).unwrap();
    assert_eq!(
        current.spec().scope(),
        &BTreeSet::from([ToolPermission::Read, ToolPermission::Write])
    );
    assert_eq!(current.evaluation(), Some("eval-2026-11-11"));

    // Duplicate registration: refused, and nothing moves.
    assert!(
        registry.register(name.clone(), ToolKind::Parser).is_err(),
        "a registered tool was registered again"
    );
    // The registry numbers versions itself, so a stale or equal version
    // cannot be offered; what can be offered is a grant already held, and
    // that mints no version.
    registry
        .promote(&name, ToolPermission::Write, "eval-2026-11-12")
        .unwrap();
    assert_eq!(registry.version(&name), Some(2));
    assert_eq!(registry.history(&name).len(), 2);
    assert_eq!(
        registry.at_version(&name, 2).and_then(|v| v.evaluation()),
        Some("eval-2026-11-11"),
        "a grant that widened nothing replaced the evaluation on record"
    );
    assert!(registry.authorise(&name, ToolPermission::Write).is_ok());
    assert!(
        registry
            .authorise(&name, ToolPermission::LeaveSandbox)
            .is_err()
    );
}

// --- model ------------------------------------------------------------------

fn model_card(version: &str, digest: &str) -> ModelCard {
    let mut card = ModelCard::new(
        ModelId::from_string(format!("m-{version}")),
        "credit-signal",
        version,
        "research",
        now(),
    );
    card.artifact_digest = Some(digest.to_string());
    card
}

/// A model version is a string and is not ordered, so the rule is met another
/// way: `name@version` names one artifact. `register` did not hold that
/// line, so re-registering a promoted version swapped the bytes every
/// earlier decision cited, with no refusal.
#[test]
fn a_model_version_is_its_own_card_beside_its_predecessor_and_a_card_rebinding_a_version_to_another_artifact_is_refused()
 {
    let first = "a".repeat(64);
    let second = "b".repeat(64);
    let mut registry = ModelRegistry::new();
    registry.register(model_card("1", &first)).unwrap();
    // Premise: the first version is held, bound to its artifact.
    assert_eq!(
        registry
            .get("credit-signal@1")
            .and_then(|c| c.artifact_digest.clone()),
        Some(first.clone())
    );

    registry.register(model_card("2", &second)).unwrap();
    assert_eq!(registry.len(), 2, "a successor is a card of its own");
    assert_eq!(
        registry
            .get("credit-signal@1")
            .and_then(|c| c.artifact_digest.clone()),
        Some(first.clone()),
        "version 1 is no longer readable as registered once version 2 was admitted"
    );

    // Duplicate version, other bytes: refused, with the digest it would have
    // replaced named, and the held card unmoved.
    let refusal = registry
        .register(model_card("1", &second))
        .expect_err("a held version was rebound to another artifact");
    assert!(
        refusal.message().contains("a version names one artifact"),
        "the refusal does not say why: {}",
        refusal.message()
    );
    let mut unbound = model_card("1", &first);
    unbound.artifact_digest = None;
    assert!(
        registry.register(unbound).is_err(),
        "a held version was replaced by a card naming no artifact"
    );
    assert_eq!(
        registry
            .get("credit-signal@1")
            .and_then(|c| c.artifact_digest.clone()),
        Some(first.clone())
    );

    // The control: a card revising what is recorded about the same artifact
    // is still admitted, so the refusal is about the bytes.
    let mut revised = model_card("1", &first);
    revised.drift_score = 0.5;
    registry.register(revised).unwrap();
    assert_eq!(registry.len(), 2);
}

// --- research ---------------------------------------------------------------

/// Recording once overwrote the entry, so a hypothesis found null and later
/// re-opened lost the null result it was screened against.
#[test]
fn a_research_verdict_recorded_again_is_the_next_version_with_its_predecessor_readable_and_a_version_not_above_the_latest_is_refused()
 {
    const HYPOTHESIS: &str = "credit leads equity";
    let null = Verdict::Null {
        evidence: "exp-1".to_string(),
    };
    let mut registry = ResearchRegistry::new();
    assert_eq!(registry.record(HYPOTHESIS, null.clone()).unwrap(), 1);
    // Premise: the hypothesis is held at version 1.
    assert_eq!(registry.latest_version(HYPOTHESIS), Some(1));

    assert_eq!(registry.record(HYPOTHESIS, Verdict::Open).unwrap(), 2);
    let history = registry.history(HYPOTHESIS);
    assert_eq!(history.len(), 2);
    assert_eq!(
        (history[0].version, &history[0].verdict),
        (1, &null),
        "version 1 is no longer readable once version 2 was recorded"
    );

    for stale in [0, 1, 2] {
        assert!(
            registry
                .record_version(
                    HYPOTHESIS,
                    stale,
                    Verdict::Supported {
                        evidence: "exp-2".to_string(),
                    },
                )
                .is_err(),
            "version {stale} was accepted over latest 2"
        );
    }
    assert_eq!(registry.latest_version(HYPOTHESIS), Some(2));
    assert_eq!(registry.history(HYPOTHESIS).len(), 2);
}

// --- capability -------------------------------------------------------------

fn capability(implementation: &str) -> CapabilityEntry {
    CapabilityEntry {
        verb: CapabilityVerb::Simulate,
        implementations: vec![implementation.to_string()],
        confidence_level: 0.8,
        eligibility: "paper only".to_string(),
    }
}

/// The capability registry is versioned as a whole: a promotion that changed
/// the set in place would leave no record of what the platform could do when
/// an earlier decision relied on it.
#[test]
fn a_capability_promotion_is_the_next_registry_version_with_its_predecessor_readable_and_a_duplicate_is_refused()
 {
    let mut registry = CapabilityRegistry::new();
    let empty = registry.current_version();
    registry.promote(capability("twin")).unwrap();
    // Premise: the capability is held, and admitting it raised the version.
    let first = registry.current_version();
    assert_eq!(first, empty + 1);
    assert_eq!(
        registry
            .get_capability(CapabilityVerb::Simulate)
            .map(|c| c.implementations),
        Some(vec!["twin".to_string()])
    );

    registry.promote(capability("monte-carlo")).unwrap();
    assert_eq!(registry.current_version(), first + 1);
    let predecessor = registry
        .capabilities_at_version(first)
        .expect("the version before the promotion is no longer readable");
    assert_eq!(predecessor.len(), 1);
    assert_eq!(
        predecessor[0].implementations,
        vec!["twin".to_string()],
        "the promotion rewrote the version it superseded"
    );

    assert!(
        registry.promote(capability("monte-carlo")).is_err(),
        "a capability already recorded was recorded again"
    );
    assert_eq!(registry.current_version(), first + 1);
    assert_eq!(
        registry
            .get_capability(CapabilityVerb::Simulate)
            .map(|c| c.implementations),
        Some(vec!["monte-carlo".to_string(), "twin".to_string()])
    );
}
