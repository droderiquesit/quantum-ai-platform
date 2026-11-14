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

/// The registry id of the organisation's World Model Arbitration Brain
/// (R12-WORLD-3025, ADR 0488): the entry [`organisation_spec`] derives from
/// the roster manifest `qip_investment_agents::manifests::
/// world_model_arbitration_brain`, whose first competency is
/// [`WORLD_MODEL_ARBITRATION`]. Until ADR 0488 this was a hand-written entry,
/// `kernel/world-model-arbitration`, describing the kernel's federation stage
/// with no specialist in the organisation behind it; removing it stopped
/// nothing but start-up. The scope is now covered only because the
/// production organisation constructs that specialist.
pub const WORLD_MODEL_ARBITRATION_BRAIN: &str = "organisation/world-model-arbitration-brain";

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

/// The id a successor names its parent by: `<brain id>-v<version>`, the form
/// `BrainSpec::parent_version_id` already carries in the contract's own
/// tests.
pub fn version_id(id: &str, version: u32) -> String {
    format!("{id}-v{version}")
}

/// Validated brain specs keyed by id, each at the version in force.
///
/// A superseded version is not retained here (ADR 0410 decision 2): its
/// successor's `parent_version_id` names it, and no figure has been given
/// that would bound a history kept for the life of the node.
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
                "brain `{}` is registered twice; admit the successor through `succeed`, as \
                 the next version naming this one as its parent, rather than a second entry",
                spec.id
            )));
        }
        self.entries.insert(spec.id.clone(), spec);
        Ok(())
    }

    /// Admit `spec` as the successor of the version held under its id,
    /// returning the version now in force (R12-COVERAGE-7170).
    ///
    /// The failure it prevents: `register` refused a second spec and told the
    /// caller to give the successor a new version, and no door admitted one,
    /// so `version` and `parent_version_id` were fields nothing could move
    /// and a brain whose tools or authority changed could only be recorded
    /// by rebuilding the registry without its past.
    ///
    /// Refused: a spec that fails its own validation; an id not yet
    /// registered (a first version is `register`'s); a version that is not
    /// exactly the held one plus one, which covers a stale version, the held
    /// version offered again and a skipped one; and a `parent_version_id`
    /// that does not name the held version, because a successor whose stated
    /// parent is not what it replaces has a lineage nobody can follow. The
    /// superseded spec is named by the successor's parent and not kept.
    pub fn succeed(&mut self, spec: BrainSpec) -> Result<u32> {
        spec.validate()?;
        let Some(held) = self.entries.get(&spec.id) else {
            return Err(Error::not_found(format!(
                "brain `{}` is not registered; register its first version before a successor",
                spec.id
            )));
        };
        let Some(next) = held.version.checked_add(1) else {
            return Err(Error::numeric(format!(
                "brain `{}` has no version left to take; register its successor under a new id",
                spec.id
            )));
        };
        if spec.version != next {
            return Err(Error::invalid(format!(
                "brain `{}` is held at version {} and the successor offered is version {}; a \
                 successor is version {next}, so renumber it",
                spec.id, held.version, spec.version
            )));
        }
        let parent = version_id(&held.id, held.version);
        if spec.parent_version_id.as_deref() != Some(parent.as_str()) {
            return Err(Error::invalid(format!(
                "brain `{}` version {next} names parent {:?} and the version it succeeds is \
                 `{parent}`; set `parent_version_id` to that",
                spec.id, spec.parent_version_id
            )));
        }
        self.entries.insert(spec.id.clone(), spec);
        Ok(next)
    }

    pub fn get(&self, id: &str) -> Option<&BrainSpec> {
        self.entries.get(id)
    }

    /// The version of brain `id` in force.
    pub fn latest_version(&self, id: &str) -> Option<u32> {
        self.entries.get(id).map(|spec| spec.version)
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

/// The registry this node starts on: every agent on the organisation's
/// roster — the World Model Arbitration Brain among them — the NOW
/// specialists and the ambient brains. Refuses to build if any entry is missing a declared
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
    registry.require_coverage()?;
    Ok(registry)
}
