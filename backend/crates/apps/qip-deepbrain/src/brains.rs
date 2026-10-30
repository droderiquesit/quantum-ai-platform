//! The Brain Registry this node starts on (R12-EXPAND-6058, §23.2).
//!
//! `BrainSpec` existed as a validated contract and the NOW specialists were
//! its only entries: the twenty agents the platform convenes as its
//! organisation, and the two always-on ambient brains this node runs every
//! sweep, had no record of what each was competent at, which tools it held,
//! what it abstains on or what authority its output carries. A brain with no
//! entry is a brain nobody can audit, and a registry that covers some of the
//! brains reads as covering all of them.
//!
//! [`standard`] builds one entry per agent on the organisation's roster —
//! read from the organisation the platform actually convened, not from a
//! list here, so an agent added to the roster tomorrow is registered or the
//! node refuses to start — plus the NOW specialists and the two ambient
//! brains. Every entry passes [`BrainSpec::validate`] on the way in, and an
//! id registered twice is refused. `main.rs` builds it after the platform is
//! assembled and before anything is served.
//!
//! What is stated honestly: `calibration` is `0.0` for every entry because
//! no brain's output is yet scored against an outcome through this registry;
//! zero is the floor of the field, not a measurement.

use qip_agents::capability::Capability;
use qip_agents::governance::Roster;
use qip_agents::manifest::AgentManifest;
use qip_contracts::expansion::BrainSpec;
use qip_core::error::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

/// The always-on attention router `ResearchLoop::new` builds and every sweep
/// runs (R12-AMBIENT-3027).
pub const AMBIENT_ATTENTION: &str = "ambient/attention";

/// The causal agency `AmbientAgency` runs over every activated signal
/// (R12-AGENCY-3028).
pub const CAUSAL_AGENCY: &str = "ambient/causal-agency";

/// The scope the ambient attention brain covers, as a competency domain.
pub const ATTENTION_ROUTING: &str = "attention-routing";

/// The scope the causal agency brain covers, as a competency domain.
pub const INTERVENTION_PLANNING: &str = "intervention-planning";

/// The kernel's world-model federation stage, which `Platform::run_cycle`
/// runs every cycle: it scores competing members, keeps every view, records
/// contradictions and decides branch, merge, abstain or ask (R12-WORLD-3025).
pub const WORLD_MODEL_ARBITRATION_BRAIN: &str = "kernel/world-model-arbitration";

/// The scope the arbitration brain covers, as a competency domain.
pub const WORLD_MODEL_ARBITRATION: &str = "world-model-arbitration";

/// Scopes this node refuses to start without. Each names a brain that runs
/// on the default path, so a registry missing one is a node whose record
/// of what it can answer leaves out something it actually does.
pub const REQUIRED_SCOPES: [&str; 3] = [
    ATTENTION_ROUTING,
    INTERVENTION_PLANNING,
    WORLD_MODEL_ARBITRATION,
];

/// Validated brain specs keyed by id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrainRegistry {
    entries: BTreeMap<String, BrainSpec>,
}

impl BrainRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one brain, refusing a spec missing a declared field and an id
    /// already registered: a second spec under one id would make the record
    /// of what that brain may do depend on which one was read.
    pub fn register(&mut self, spec: BrainSpec) -> Result<()> {
        spec.validate()?;
        if self.entries.contains_key(&spec.id) {
            return Err(Error::invalid(format!(
                "brain `{}` is registered twice; give the successor a new version under the \
                 same lineage rather than a second entry",
                spec.id
            )));
        }
        self.entries.insert(spec.id.clone(), spec);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&BrainSpec> {
        self.entries.get(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Every competency domain some registered brain covers: what this
    /// node's brains, together, are on record as able to answer.
    pub fn scopes(&self) -> BTreeSet<&str> {
        self.entries
            .values()
            .flat_map(|spec| spec.competency_domains.iter().map(String::as_str))
            .collect()
    }

    /// Refuse a registry that leaves out a [`REQUIRED_SCOPES`] entry, naming
    /// every missing one.
    pub fn require_coverage(&self) -> Result<()> {
        let scopes = self.scopes();
        let missing: Vec<&str> = REQUIRED_SCOPES
            .iter()
            .copied()
            .filter(|scope| !scopes.contains(scope))
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(Error::invalid(format!(
                "no registered brain covers {missing:?}; a node whose brain registry leaves out \
                 a scope it runs is refused, so register the brain that covers it"
            )))
        }
    }

    /// Without the brain `id`, for the test that removing a brain drops its
    /// scope. Not called on a running path: the node never deregisters.
    pub fn without(mut self, id: &str) -> Self {
        self.entries.remove(id);
        self
    }
}

/// The registry entry for one organisation agent, read off its manifest.
pub fn organisation_spec(manifest: &AgentManifest) -> BrainSpec {
    let may_call_a_model = manifest
        .capabilities
        .contains(Capability::CallLanguageModel)
        || manifest.budget.language_model_calls > 0;
    BrainSpec {
        id: format!("organisation/{}", manifest.id),
        name: manifest.name.clone(),
        competency_domains: manifest.competencies.clone(),
        tools: manifest
            .capabilities
            .iter()
            .map(|capability| format!("capability:{capability}"))
            .collect(),
        source_permissions: vec!["desk-view:read".to_string()],
        memory_scope: "organisation: one panel convening".to_string(),
        model_stack: vec![if may_call_a_model {
            "language model through the platform's model chain".to_string()
        } else {
            "in-tree rules; no model consulted".to_string()
        }],
        evaluations: vec!["qip-investment-agents tests/organisation.rs".to_string()],
        abstention_rules: manifest.limitations.clone(),
        authority_envelope: format!(
            "{:?}, escalating to {}, owned by {}",
            manifest.escalation,
            manifest.escalates_to.as_deref().unwrap_or("nobody"),
            manifest.owner
        ),
        calibration: 0.0,
        version: 1,
        parent_version_id: None,
    }
}

/// The two ambient brains this node runs every sweep.
pub fn ambient_specs() -> Vec<BrainSpec> {
    vec![
        BrainSpec {
            id: AMBIENT_ATTENTION.to_string(),
            name: "Ambient Attention Brain".to_string(),
            competency_domains: vec![
                ATTENTION_ROUTING.to_string(),
                "surprise-opportunity-risk-assumption-fusion".to_string(),
            ],
            tools: vec!["qip-contracts::ambient::AttentionRouter".to_string()],
            source_permissions: vec!["ambient-signals:read".to_string()],
            memory_scope: "research-loop: pending activations, bounded by the routing budget"
                .to_string(),
            model_stack: vec!["in-tree routing policy; no model consulted".to_string()],
            evaluations: vec![
                "qip-contracts tests/ambient.rs: a_storm_never_exceeds_the_attention_budget_and_every_excess_signal_is_deferred_or_shed"
                    .to_string(),
            ],
            abstention_rules: vec![
                "a signal below materiality wakes nothing".to_string(),
                "a signal past its expiry wakes nothing".to_string(),
            ],
            authority_envelope: "advisory_only: wakes specialist, research and reflex pathways"
                .to_string(),
            calibration: 0.0,
            version: 1,
            parent_version_id: None,
        },
        BrainSpec {
            id: CAUSAL_AGENCY.to_string(),
            name: "Causal Agency Brain".to_string(),
            competency_domains: vec![
                INTERVENTION_PLANNING.to_string(),
                "goal-decomposition".to_string(),
                "effect-attribution".to_string(),
            ],
            tools: vec!["internal-outbox draft".to_string()],
            source_permissions: vec!["ambient-signals:read".to_string()],
            memory_scope: "action outcome memory, bounded".to_string(),
            model_stack: vec!["qip-agency in-tree planner; no model consulted".to_string()],
            evaluations: vec!["qip-deepbrain tests/ambient_agency.rs".to_string()],
            abstention_rules: vec![
                "a stage that refuses withholds the draft and names the stage".to_string(),
                "nothing is sent outside the internal outbox (ADR 0114)".to_string(),
            ],
            authority_envelope: "shadow and internal drafts only; reaches no venue".to_string(),
            calibration: 0.0,
            version: 1,
            parent_version_id: None,
        },
    ]
}

/// The kernel's world-model arbitration brain (R12-WORLD-3025, §9.1).
pub fn arbitration_spec() -> BrainSpec {
    BrainSpec {
        id: WORLD_MODEL_ARBITRATION_BRAIN.to_string(),
        name: "World Model Arbitration Brain".to_string(),
        competency_domains: vec![
            WORLD_MODEL_ARBITRATION.to_string(),
            "disagreement-preservation".to_string(),
            "calibration-tracking".to_string(),
        ],
        tools: vec!["qip-kernel::federation_stage::FederationStage".to_string()],
        source_permissions: vec!["world-model-federation:read".to_string()],
        memory_scope: "federation: live members, bounded branches, pending forecasts".to_string(),
        model_stack: vec!["in-tree arbitration rules; no model consulted".to_string()],
        evaluations: vec![
            "qip-kernel tests/symbolic_federation.rs: every_cycle_journals_one_federation_review_and_each_replays_and_reproduces_its_arbitrations"
                .to_string(),
        ],
        abstention_rules: vec![
            "abstains when the views' mean uncertainty is too high to say anything".to_string(),
            "asks for more information while a member has no resolved forecast".to_string(),
        ],
        authority_envelope:
            "advisory_only: records arbitrations and research requests; sizes and sends nothing"
                .to_string(),
        calibration: 0.0,
        version: 1,
        parent_version_id: None,
    }
}

/// The registry this node starts on: every agent on the organisation's
/// roster, the NOW specialists, the ambient brains and the world-model
/// arbitration brain. Refuses to build if any entry is missing a declared
/// field, two share an id, or a [`REQUIRED_SCOPES`] entry is uncovered.
pub fn standard(organisation: &Roster) -> Result<BrainRegistry> {
    let mut registry = BrainRegistry::new();
    for manifest in organisation.iter() {
        registry
            .register(organisation_spec(manifest))
            .map_err(|error| {
                Error::invalid(format!(
                    "the organisation's agent `{}` has no valid brain entry: {}",
                    manifest.id,
                    error.message()
                ))
            })?;
    }
    for (_, spec) in crate::now::specialist_registry()? {
        registry.register(spec)?;
    }
    for spec in ambient_specs() {
        registry.register(spec)?;
    }
    registry.register(arbitration_spec())?;
    registry.require_coverage()?;
    Ok(registry)
}
