//! R12-EXPAND-6058, R12-AMBIENT-3027, R12-AGENCY-3028: the Brain Registry
//! the node starts on.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_core::{Context, Timestamp};
use qip_deepbrain::brains::{
    AMBIENT_ATTENTION, ATTENTION_ROUTING, BrainRegistry, CAUSAL_AGENCY, INTERVENTION_PLANNING,
    ambient_specs, organisation_spec, standard,
};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) =
        Context::deterministic(Timestamp::from_secs(1_760_000_000), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap()
}

/// Every agent the platform convenes has an entry, read off the
/// organisation the platform built rather than off a list in the test, and
/// so do the two ambient brains. Mutation: skip the roster loop in
/// `brains::standard` — no `organisation/` entry exists and the per-agent
/// assertion fails on the first agent.
#[test]
fn every_agent_the_platform_convenes_and_both_ambient_brains_have_a_validated_entry() {
    let platform = platform();
    let roster = platform.organisation().roster();
    let ids: Vec<String> = roster.iter().map(|m| m.id.clone()).collect();
    assert!(
        ids.len() >= 20,
        "premise: the organisation convened its roster: {ids:?}"
    );

    let registry = standard(platform.organisation()).expect("the node's registry builds");
    for id in &ids {
        let spec = registry
            .get(&format!("organisation/{id}"))
            .unwrap_or_else(|| panic!("the convened agent `{id}` has no brain entry"));
        spec.validate().unwrap();
    }
    assert!(registry.get(AMBIENT_ATTENTION).is_some());
    assert!(registry.get(CAUSAL_AGENCY).is_some());
    assert!(
        registry.get("now-specialist/risk").is_some(),
        "the NOW specialists are registered too"
    );
}

/// An entry missing a declared field is refused at registration, and so is
/// a second entry under one id. Mutation: drop `spec.validate()?` from
/// `BrainRegistry::register` — the empty-domain spec is admitted and the
/// first assertion fails.
#[test]
fn an_entry_missing_a_declared_field_or_registered_twice_is_refused() {
    let platform = platform();
    let manifest = platform
        .organisation()
        .roster()
        .iter()
        .next()
        .unwrap()
        .clone();
    let mut registry = BrainRegistry::new();
    let mut hollow = organisation_spec(&manifest);
    hollow.competency_domains.clear();
    assert!(
        registry.register(hollow).is_err(),
        "a brain with no competency domain was registered"
    );
    assert!(registry.is_empty());
    registry.register(organisation_spec(&manifest)).unwrap();
    assert!(
        registry.register(organisation_spec(&manifest)).is_err(),
        "one id was registered twice"
    );
    assert_eq!(registry.len(), 1);
}

/// Removing an ambient brain drops its scope: nothing else on record covers
/// attention routing or intervention planning. Mutation: give a NOW
/// specialist the `attention-routing` domain in `now::specialist_registry`
/// — the scope survives the removal and the first `!contains` fails.
#[test]
fn removing_an_ambient_brain_drops_its_scope_from_the_registry() {
    let platform = platform();
    let registry = standard(platform.organisation()).unwrap();
    assert!(
        registry.scopes().contains(ATTENTION_ROUTING),
        "premise: covered"
    );
    assert!(
        registry.scopes().contains(INTERVENTION_PLANNING),
        "premise: covered"
    );
    assert_eq!(ambient_specs().len(), 2);

    let without_attention = registry.clone().without(AMBIENT_ATTENTION);
    assert!(!without_attention.scopes().contains(ATTENTION_ROUTING));
    assert!(without_attention.scopes().contains(INTERVENTION_PLANNING));
    let without_agency = registry.without(CAUSAL_AGENCY);
    assert!(!without_agency.scopes().contains(INTERVENTION_PLANNING));
    assert!(without_agency.scopes().contains(ATTENTION_ROUTING));
}

/// R12-WORLD-3025, ADR 0488: the World Model Arbitration Brain's registry
/// entry is the one derived from the production organisation's roster — the
/// platform's own organisation, not a hand-written spec — and removing it
/// drops its scope from coverage. The node then refuses to start, since
/// `standard` requires the scope. Nothing else on record covers world-model
/// arbitration, so the removal is observable.
///
/// Mutation: in `qip_investment_agents::manifests::world_model_arbitration_brain`,
/// replace the `"world-model-arbitration"` competency with another string --
/// `standard` then refuses to build, naming the missing scope, and the
/// `standard(..).unwrap()` fails on that refusal. (Dropping the manifest from
/// the roster instead fails earlier, at platform assembly, because
/// `standard_agents` still builds an agent the roster does not name.)
#[test]
fn removing_the_arbitration_brain_drops_world_model_arbitration_from_coverage() {
    use qip_deepbrain::brains::{
        REQUIRED_SCOPES, WORLD_MODEL_ARBITRATION, WORLD_MODEL_ARBITRATION_BRAIN,
    };
    let platform = platform();
    let registry = standard(platform.organisation()).unwrap();
    let spec = registry
        .get(WORLD_MODEL_ARBITRATION_BRAIN)
        .expect("the arbitration brain is in the production registry");
    assert!(spec.validate().is_ok());
    assert!(
        platform
            .organisation()
            .roster()
            .iter()
            .any(|manifest| format!("organisation/{}", manifest.id) == spec.id),
        "the arbitration entry is derived from a manifest on the running roster"
    );
    assert!(registry.scopes().contains(WORLD_MODEL_ARBITRATION));
    assert!(REQUIRED_SCOPES.contains(&WORLD_MODEL_ARBITRATION));
    assert!(registry.require_coverage().is_ok(), "premise: covered");

    let without = registry.without(WORLD_MODEL_ARBITRATION_BRAIN);
    assert!(
        !without.scopes().contains(WORLD_MODEL_ARBITRATION),
        "another brain still claims world-model arbitration"
    );
    let refused = without
        .require_coverage()
        .expect_err("a registry without the arbitration brain is refused");
    assert!(
        refused.message().contains(WORLD_MODEL_ARBITRATION),
        "{}",
        refused.message()
    );
}

/// R12-REASON-3026: the registry the node starts on covers
/// `symbolic-reasoning` only because the production organisation constructs
/// the Symbolic Reasoning Brain, and an organisation built from the same
/// production parts without `standard_late_agents` is refused at start-up.
///
/// The removal runs through the production builders, not
/// `BrainRegistry::without`, which no running path calls: the roster still
/// holds the reviewed manifest in the second organisation, so a registry read
/// off the roster alone would still list the entry and admit the node. The
/// earlier form of this test removed the entry by hand and could not see that.
///
/// Mutations: in `qip_investment_agents::chief::standard_late_agents`, return
/// an empty list -- the platform's own organisation constructs no symbolic
/// agent and `standard(platform.organisation())` refuses, so the first
/// `expect` fails; in `brains::standard`, drop the `constructs` filter -- the
/// organisation without late agents is admitted and the `expect_err` fails.
#[test]
fn removing_the_symbolic_brain_drops_symbolic_reasoning_from_coverage() {
    use qip_deepbrain::brains::{REQUIRED_SCOPES, SYMBOLIC_REASONING, SYMBOLIC_REASONING_BRAIN};
    use qip_investment_agents::chief::{
        Organisation, standard_agents, standard_arbiters, standard_late_agents,
    };
    use qip_investment_agents::{Desk, ids, roster};
    use std::sync::Arc;

    let platform = platform();
    // `standard` is what `assembly::assemble` runs on the platform's own
    // organisation, so its refusal is the node's refusal to start.
    let registry =
        standard(platform.organisation()).expect("the production organisation's registry builds");
    assert!(
        platform.organisation().constructs(ids::SYMBOLIC_REASONING),
        "the running organisation constructs the symbolic brain"
    );
    let spec = registry
        .get(SYMBOLIC_REASONING_BRAIN)
        .expect("the symbolic brain is in the production registry");
    assert_eq!(
        spec.competency_domains.first().map(String::as_str),
        Some(SYMBOLIC_REASONING)
    );
    assert!(REQUIRED_SCOPES.contains(&SYMBOLIC_REASONING));

    let t = Timestamp::from_secs(1_760_000_000);
    let desk = Arc::new(Desk::empty(t));
    let build = |late: bool| {
        let organisation = Organisation::new(
            roster(t),
            standard_agents(desk.clone(), t, Vec::new(), false),
            qip_agents::runtime::AgentHost::new(1),
            t,
        )
        .unwrap()
        .with_arbiters(standard_arbiters(desk.clone(), t))
        .unwrap();
        if late {
            organisation
                .with_late_agents(standard_late_agents(desk.clone(), t))
                .unwrap()
        } else {
            organisation
        }
    };

    let whole = build(true);
    assert!(whole.constructs(ids::SYMBOLIC_REASONING));
    assert!(
        standard(&whole).is_ok(),
        "premise: the production parts with the late agents are admitted"
    );

    let without = build(false);
    assert!(
        !without.constructs(ids::SYMBOLIC_REASONING),
        "premise: nothing else constructs the symbolic brain"
    );
    assert!(
        without.roster().get(ids::SYMBOLIC_REASONING).is_some(),
        "premise: the reviewed manifest is still on the roster"
    );
    let refused = standard(&without)
        .expect_err("a node whose organisation does not construct the symbolic brain is refused");
    assert!(
        refused.message().contains("[\"symbolic-reasoning\"]"),
        "{}",
        refused.message()
    );
}

/// R12-AGENCY-3028, ADR 0146 decision 3: the registry the node starts on
/// refuses to be used without the Causal Agency Brain. Removing it leaves
/// `intervention-planning` uncovered, and `require_coverage` -- which
/// `standard` ends on -- refuses naming that scope, so a node cannot start
/// running ambient agency plans that its own record does not mention.
///
/// Mutation: delete `INTERVENTION_PLANNING` from `REQUIRED_SCOPES` -- the
/// `expect_err` fails because the registry without the agency is admitted.
#[test]
fn a_registry_without_the_causal_agency_brain_is_refused_naming_its_scope() {
    use qip_deepbrain::brains::REQUIRED_SCOPES;
    let platform = platform();
    let registry = standard(platform.organisation()).unwrap();
    assert!(registry.get(CAUSAL_AGENCY).is_some(), "premise: registered");
    assert!(REQUIRED_SCOPES.contains(&INTERVENTION_PLANNING));
    assert!(registry.require_coverage().is_ok(), "premise: covered");

    let refused = registry
        .without(CAUSAL_AGENCY)
        .require_coverage()
        .expect_err("a registry without the causal agency brain is refused");
    assert!(
        refused.message().contains(INTERVENTION_PLANNING),
        "{}",
        refused.message()
    );
}
