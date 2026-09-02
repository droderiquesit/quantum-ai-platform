//! What was decided about a source, and why.
//!
//! Every decision carries its reasoning, and [`RegistrationDecision`] has no
//! constructor that accepts an empty one. A source approved without a recorded
//! reason is a source nobody can audit: six months later the question is not
//! "is this feed allowed" but "who decided it was, on what evidence, and does
//! that evidence still hold" — and a decision with no trail cannot answer any
//! of the three.
//!
//! The reasoning is stage-by-stage rather than one sentence, so that a
//! reviewer can see which stage a rejection came from without re-running the
//! lifecycle.

use crate::legal::{LegalAssessment, SourcePolicy};
use crate::schema::SchemaDrift;
use crate::scoring::{Routing, RoutingClass, SourceScores};
use crate::source::{Source, SourceLineage};
use qip_contracts::governance::Entitlement;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_mesh::MeshPort;
use qip_mesh::catalog::{DatasetRegistration, QualityState};
use serde::{Deserialize, Serialize};

/// One stage of the source lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStage {
    Discover,
    Classify,
    Probe,
    AssessLegality,
    Score,
    Route,
    Register,
    Monitor,
    DetectDrift,
    FindReplacement,
}

impl LifecycleStage {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Classify => "classify",
            Self::Probe => "probe",
            Self::AssessLegality => "assess_legality",
            Self::Score => "score",
            Self::Route => "route",
            Self::Register => "register",
            Self::Monitor => "monitor",
            Self::DetectDrift => "detect_drift",
            Self::FindReplacement => "find_replacement",
        }
    }

    /// The stages in the order the lifecycle runs them.
    pub const ORDER: [Self; 10] = [
        Self::Discover,
        Self::Classify,
        Self::Probe,
        Self::AssessLegality,
        Self::Score,
        Self::Route,
        Self::Register,
        Self::Monitor,
        Self::DetectDrift,
        Self::FindReplacement,
    ];
}

/// One finding, at one stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasonStep {
    pub stage: LifecycleStage,
    pub finding: String,
}

impl ReasonStep {
    pub fn describe(&self) -> String {
        format!("{}: {}", self.stage.as_str(), self.finding)
    }
}

/// The ordered trail behind a decision.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reasoning {
    steps: Vec<ReasonStep>,
}

impl Reasoning {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, stage: LifecycleStage, finding: impl Into<String>) {
        self.steps.push(ReasonStep {
            stage,
            finding: finding.into(),
        });
    }

    pub fn steps(&self) -> &[ReasonStep] {
        &self.steps
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Whether a stage was reached at all.
    pub fn reached(&self, stage: LifecycleStage) -> bool {
        self.steps.iter().any(|step| step.stage == stage)
    }

    /// Findings recorded at one stage.
    pub fn at(&self, stage: LifecycleStage) -> Vec<&str> {
        self.steps
            .iter()
            .filter(|step| step.stage == stage)
            .map(|step| step.finding.as_str())
            .collect()
    }

    pub fn describe(&self) -> String {
        self.steps
            .iter()
            .map(ReasonStep::describe)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The terms a source is collected under.
///
/// Fields are private and there is no public constructor: the only code that
/// can build one is [`RegistrationDecision::registered`], which takes the
/// [`LegalAssessment`] and refuses unless it permits collection. Before this
/// type existed the `Registered` outcome had public fields, so any caller
/// could assemble one from a routing and a policy and ask for a
/// [`RegistrationDecision::catalogue_entry`] — a catalogue entry for a source
/// whose licence nobody had read, produced by a path that never touched the
/// gate this crate exists to hold. The mesh catalogue is what the rest of the
/// platform consults before it uses a dataset, so an entry that skipped
/// legality is a research-only feed that trades.
///
/// ```compile_fail
/// use qip_data_finder::decision::{DecisionOutcome, Registration};
/// use qip_data_finder::legal::SourcePolicy;
/// use qip_data_finder::scoring::Routing;
///
/// fn forge(routing: Routing, policy: SourcePolicy) -> DecisionOutcome {
///     DecisionOutcome::Registered(Registration {
///         routing,
///         policy: Box::new(policy),
///         entitlements: Vec::new(),
///     })
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Registration {
    routing: Routing,
    policy: Box<SourcePolicy>,
    entitlements: Vec<Entitlement>,
}

impl Registration {
    pub fn routing(&self) -> &Routing {
        &self.routing
    }

    pub fn policy(&self) -> &SourcePolicy {
        &self.policy
    }

    pub fn entitlements(&self) -> &[Entitlement] {
        &self.entitlements
    }
}

/// What the finder decided to do with a source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DecisionOutcome {
    /// Collect it, under the policy and entitlements in the [`Registration`].
    /// Only [`RegistrationDecision::registered`] can produce this arm.
    Registered(Registration),
    /// Do not collect it.
    Rejected { reason: String },
    /// Nothing was decided, because the source could not be reached. Distinct
    /// from rejection: a probe that failed is a fact about us, not about the
    /// source, and recording it as a rejection would bury a broken crawler
    /// under a pile of apparently bad sources.
    Deferred { reason: String },
    /// Registered previously and stopped, because its shape moved.
    Quarantined {
        drift: Box<SchemaDrift>,
        reason: String,
    },
}

impl DecisionOutcome {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Registered { .. } => "registered",
            Self::Rejected { .. } => "rejected",
            Self::Deferred { .. } => "deferred",
            Self::Quarantined { .. } => "quarantined",
        }
    }

    pub fn is_registered(&self) -> bool {
        matches!(self, Self::Registered { .. })
    }

    pub fn routing_class(&self) -> RoutingClass {
        match self {
            Self::Registered(registration) => registration.routing.class(),
            Self::Rejected { .. } | Self::Deferred { .. } | Self::Quarantined { .. } => {
                RoutingClass::Rejected
            }
        }
    }
}

/// One source, one decision, and the trail that produced it.
///
/// Fields are private and both constructors demand reasoning, so an
/// unauditable decision cannot be built. Of the two, only
/// [`Self::registered`] can produce the `Registered` outcome, and it takes the
/// legality assessment as an argument rather than trusting the caller to have
/// consulted one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistrationDecision {
    source_id: String,
    outcome: DecisionOutcome,
    legality: Option<LegalAssessment>,
    scores: Option<SourceScores>,
    lineage: Option<SourceLineage>,
    reasoning: Reasoning,
    decided_at: Timestamp,
}

impl RegistrationDecision {
    /// Record a decision not to collect: a rejection, a deferral or a
    /// quarantine.
    ///
    /// Refuses empty reasoning, and there is no setter that can empty it
    /// afterwards. A `Registered` outcome cannot reach this constructor from
    /// outside the crate, because [`Registration`] has no public constructor;
    /// registering goes through [`Self::registered`].
    pub fn new(
        source_id: impl Into<String>,
        outcome: DecisionOutcome,
        reasoning: Reasoning,
        decided_at: Timestamp,
    ) -> Result<Self> {
        let source_id = source_id.into();
        if reasoning.is_empty() {
            return Err(Error::invalid(format!(
                "the decision on source `{source_id}` records no reasoning; a source approved \
                 or refused without a recorded reason is one nobody can audit"
            )));
        }
        Ok(Self {
            source_id,
            outcome,
            legality: None,
            scores: None,
            lineage: None,
            reasoning,
            decided_at,
        })
    }

    /// Record a decision to collect.
    ///
    /// The legality assessment is an argument, not an afterthought: the
    /// `Registered` outcome is built here and nowhere else, so a source cannot
    /// be registered without the licensing, robots and host questions having
    /// been asked, and cannot be registered when any of them answered no. The
    /// routing carries the same verdict a second time — [`Routing::decide`]
    /// rejects on legality — and both are checked, because the two were
    /// computed separately and a caller that pairs a permitted assessment with
    /// a routing decided against a different one has made the mistake this
    /// constructor exists to catch.
    pub fn registered(
        source_id: impl Into<String>,
        legality: LegalAssessment,
        routing: Routing,
        policy: SourcePolicy,
        entitlements: Vec<Entitlement>,
        reasoning: Reasoning,
        decided_at: Timestamp,
    ) -> Result<Self> {
        let source_id = source_id.into();
        if !legality.overall().is_permitted() {
            return Err(Error::denied(format!(
                "source `{source_id}` cannot be registered: its legality for {} is {}; a source \
                 whose collection is not permitted has no catalogue entry, whatever its score",
                legality.usage().as_str(),
                legality.overall().describe()
            )));
        }
        if !routing.class().is_collected() {
            return Err(Error::denied(format!(
                "source `{source_id}` cannot be registered: it was routed to `{}` ({})",
                routing.class().as_str(),
                routing.basis()
            )));
        }
        let mut decision = Self::new(
            source_id,
            DecisionOutcome::Registered(Registration {
                routing,
                policy: Box::new(policy),
                entitlements,
            }),
            reasoning,
            decided_at,
        )?;
        decision.legality = Some(legality);
        Ok(decision)
    }

    pub fn with_legality(mut self, legality: LegalAssessment) -> Self {
        self.legality = Some(legality);
        self
    }

    pub fn with_scores(mut self, scores: SourceScores) -> Self {
        self.scores = Some(scores);
        self
    }

    pub fn with_lineage(mut self, lineage: SourceLineage) -> Self {
        self.lineage = Some(lineage);
        self
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn outcome(&self) -> &DecisionOutcome {
        &self.outcome
    }

    /// The legality questions and their answers, where the lifecycle got far
    /// enough to ask them.
    pub fn legality(&self) -> Option<&LegalAssessment> {
        self.legality.as_ref()
    }

    pub fn scores(&self) -> Option<&SourceScores> {
        self.scores.as_ref()
    }

    pub fn lineage(&self) -> Option<&SourceLineage> {
        self.lineage.as_ref()
    }

    pub fn reasoning(&self) -> &Reasoning {
        &self.reasoning
    }

    pub fn decided_at(&self) -> Timestamp {
        self.decided_at
    }

    pub fn is_registered(&self) -> bool {
        self.outcome.is_registered()
    }

    /// The policy an adapter must obey, where one was issued.
    pub fn policy(&self) -> Option<&SourcePolicy> {
        match &self.outcome {
            DecisionOutcome::Registered(registration) => Some(registration.policy()),
            _ => None,
        }
    }

    /// The terms the source is collected under, where it is collected.
    pub fn registration(&self) -> Option<&Registration> {
        match &self.outcome {
            DecisionOutcome::Registered(registration) => Some(registration),
            _ => None,
        }
    }

    /// The mesh catalogue entry for this source.
    ///
    /// The finder does not keep its own catalogue. It produces the mesh's
    /// registration type, entitlements included, so there is one answer to
    /// "what may this dataset be used for" rather than two that can disagree.
    ///
    /// Only a `Registered` outcome has an entry, and only
    /// [`Self::registered`] produces that outcome, so every entry this returns
    /// passed the legality assessment.
    pub fn catalogue_entry(&self, owner: &str) -> Result<DatasetRegistration> {
        let DecisionOutcome::Registered(Registration { entitlements, .. }) = &self.outcome else {
            return Err(Error::denied(format!(
                "source `{}` was {} and has no catalogue entry",
                self.source_id,
                self.outcome.as_str()
            )));
        };
        let dataset = format!("source.{}", self.source_id);
        let mut registration =
            DatasetRegistration::new(dataset, owner, MeshPort::Lakehouse, self.decided_at)?
                .with_quality(QualityState::Verified {
                    at: self.decided_at,
                    checks: self
                        .reasoning
                        .steps()
                        .iter()
                        .map(ReasonStep::describe)
                        .collect(),
                });
        if let Some(lineage) = &self.lineage {
            registration = registration.produced_from(lineage.produced_from());
        }
        for entitlement in entitlements {
            registration = registration.licensed(entitlement.clone());
        }
        Ok(registration)
    }
}

/// A source the finder is currently collecting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisteredSource {
    source: Source,
    routing: Routing,
    policy: SourcePolicy,
    lineage: SourceLineage,
    entitlements: Vec<Entitlement>,
    registered_at: Timestamp,
    quarantined: Option<String>,
}

impl RegisteredSource {
    pub(crate) fn new(
        source: Source,
        routing: Routing,
        policy: SourcePolicy,
        lineage: SourceLineage,
        entitlements: Vec<Entitlement>,
        registered_at: Timestamp,
    ) -> Self {
        Self {
            source,
            routing,
            policy,
            lineage,
            entitlements,
            registered_at,
            quarantined: None,
        }
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    pub fn id(&self) -> &str {
        self.source.id()
    }

    pub fn routing(&self) -> &Routing {
        &self.routing
    }

    pub fn policy(&self) -> &SourcePolicy {
        &self.policy
    }

    pub fn lineage(&self) -> &SourceLineage {
        &self.lineage
    }

    pub fn entitlements(&self) -> &[Entitlement] {
        &self.entitlements
    }

    pub fn registered_at(&self) -> Timestamp {
        self.registered_at
    }

    /// Why this source is not being consumed, if it is not.
    pub fn quarantine_reason(&self) -> Option<&str> {
        self.quarantined.as_deref()
    }

    pub fn is_quarantined(&self) -> bool {
        self.quarantined.is_some()
    }

    pub(crate) fn quarantine(&mut self, reason: impl Into<String>) {
        self.quarantined = Some(reason.into());
    }
}
