//! The symbolic reasoners, run beside every hypothesis REASON forms
//! (blueprint §9.4; REASON-3053 to -3060).
//!
//! `qip-reasoning-engine` carried a rule engine, a temporal order, a Bayesian
//! network, a constraint solver and a lifecycle judge, and `qip-world-model`
//! carried `abduce` and `intervene`; every one was tested and none was called
//! by the cycle, so the platform's reasoning was the panel and a log-odds
//! update and nothing a person could re-derive step by step. This module
//! composes them into one [`SymbolicDerivation`] per hypothesis, each part a
//! trace the log keeps: the rules that fired, the links the timeline admitted
//! or refused, the posterior and the structure it came from, the minimal
//! conflict that blocks acting, the first lifecycle break, the ranked
//! explanations of the surprise and what an intervention on the subject
//! would reach.
//!
//! # A veto, and only a veto (ADR 0125)
//!
//! [`derive`] takes its inputs by value and the causal graph by shared
//! reference and returns a record. It holds nothing that reaches a thesis, a
//! size or an order. The kernel runs it beside REASON's verdict and reads one
//! thing off the record, [`SymbolicDerivation::veto`]: when review approved a
//! hypothesis and the rule pack, having evaluated, does not find it eligible,
//! the hypothesis forms no proposal. That is the whole of the power ADR 0125
//! decision 2 gives it: it subtracts a proposal and never creates, enlarges
//! or re-signs one. A reasoner that refused its input is not a veto (a
//! malformed timeline is a finding about the evidence, and vetoing on it
//! would let a broken input silence every hypothesis). Pre-trade checks stay
//! deterministic and stay where `Determinism::Required` puts them.
//!
//! # Why every part refuses rather than guesses
//!
//! Each reasoner already refuses malformed input — an effect before its
//! cause, a posterior on impossible evidence, a surprise that is not one. A
//! refusal is a finding about this hypothesis and lands in the record as
//! [`Step::Refused`] with the reasoner's own sentence; the other parts still
//! run. A derivation that silently dropped the part that refused would read
//! as one where that reasoner agreed.
//!
//! # A contradiction is not a refusal, and it is not dissent (ADR 0212)
//!
//! Three things look alike on a dashboard and are kept apart here. *Dissent*
//! is the panel disagreeing: a contested thesis is held small and scored
//! (ADR 0150 decision 14) and nothing in this module stops it. A *refused
//! part* is a reasoner rejecting a malformed input: a finding about the
//! evidence, recorded in `violated_assumptions` and left to ADR 0150
//! decision 13. A *symbolic contradiction* is the derivation's own parts
//! being unable to hold together — the rule pack deriving a fact against one
//! it already established or reaching a state its own invariant forbids, a
//! verdict that does not reproduce, or the pack finding acting eligible while
//! the acting constraints are unsatisfiable. [`Feasibility`] marks that on
//! the record with what contradicted what and the revision tasks the pack's
//! own contradiction search offers as alternates; the kernel reads
//! [`SymbolicDerivation::infeasibility`] and forms no proposal on it.

use std::collections::{BTreeMap, BTreeSet};

use qip_agents::registries::OntologyRegistry;
use qip_contracts::expansion::{OntologyTypeFamily, OntologyTypeSpec};
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_reasoning_engine::constraint::{Answer, Constraint, Form, Problem, Variable};
use qip_reasoning_engine::lifecycle::{Admissibility, Lifecycle};
use qip_reasoning_engine::network::{Network, Node};
use qip_reasoning_engine::rules::{
    Cond, Invariant, Op, RevisionTask, Rule, RulePack, Value, Verdict,
};
use qip_reasoning_engine::temporal::{self, Order, Timeline};
use qip_world_model::causal::CausalGraph;
use qip_world_model::inference::{Abduction, CausalPath, Surprise};
use serde::{Deserialize, Serialize};

/// The rule pack's name and version, cited by every verdict.
pub const RULE_PACK: &str = "reason/advisory-eligibility";
pub const RULE_PACK_VERSION: u32 = 3;

/// The hypothesis lifecycle's name and version.
pub const HYPOTHESIS_LIFECYCLE: &str = "reason/hypothesis";
pub const HYPOTHESIS_LIFECYCLE_VERSION: u32 = 1;

/// Paths of an intervention kept in the record; the count is kept whole.
pub const INTERVENTION_PATHS_KEPT: usize = 16;

/// How many abduced causes a derivation explains counterfactually, best
/// first. Bounded so the record stays bounded; the abduction beneath it is
/// already bounded by its own candidate cap.
pub const COUNTERFACTUALS_KEPT: usize = 4;

/// What every derivation's idempotency key starts with.
pub const IDEMPOTENCY_PREFIX: &str = "symbolic:";

/// The anomaly a hypothesis came from, as the reasoners read it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnomalyFacts {
    pub detector: String,
    pub detected_at: Timestamp,
    pub observed: f64,
    pub expected: f64,
    /// The detector's confidence the anomaly is real, in `[0, 1]`.
    pub confidence: f64,
}

/// Everything one derivation reads.
#[derive(Clone, Debug, PartialEq)]
pub struct SymbolicInput {
    pub cycle: u64,
    pub hypothesis_id: String,
    pub subject: String,
    /// The review's verdict, as `HypothesisStatus::as_str` spells it.
    pub status: String,
    /// The hypothesis's effective confidence, in `[0, 1]`.
    pub confidence: f64,
    pub contested: bool,
    pub dissenters: usize,
    pub evidence_items: usize,
    pub kill_switch: bool,
    /// Whether the autonomy level in force is a live one. Read so the rule
    /// pack's invariant can hold the paper boundary as a theorem of the pack.
    pub live_authority: bool,
    pub anomaly: AnomalyFacts,
    /// A catalyst the anomaly was linked to: its id and when it was knowable.
    pub catalyst: Option<(String, Timestamp)>,
    pub formed_at: Timestamp,
    pub horizon: Duration,
    /// The latest observed return of each instrument, for abduction.
    pub moves: BTreeMap<String, f64>,
    /// The subject's settlement convention and its lag (REASON-3055), from
    /// the asset-class record the platform registered it under; `None` for
    /// a class with no delivery cycle or a subject with no record.
    pub settlement: Option<(String, Duration)>,
    /// The asset class the platform registered the subject under, as
    /// `AssetClass::as_str` spells it; `None` for a subject with no record.
    /// What the subject is typed against in [`platform_ontology`].
    pub subject_class: Option<String>,
    /// Whole currency units left in the position budget after buying one lot
    /// of the subject at its last price (R12-REASON-3057, market structure):
    /// negative means the smallest unit the venue sells is larger than the
    /// most the book may hold in one name, so no order could ever be both
    /// expressible and inside the limit. `None` where the platform could not
    /// read a lot, a price or a position cap — an unreadable fact is left
    /// out of the pack rather than invented, and the rules that need it do
    /// not fire.
    pub lot_headroom: Option<i64>,
    /// Whole currency units of cash left after paying for one lot
    /// (R12-REASON-3057, accounting): negative means the cheapest trade the
    /// instrument allows would drive the book's cash negative.
    pub cash_headroom: Option<i64>,
    /// Whole days between the hypothesis's deadline (`formed_at + horizon`)
    /// and the expiry of the contract the subject is (R12-REASON-3057,
    /// contracts): read from the derivative extension's `expiry` for a
    /// future or an option, floored, so an expiry an hour before the
    /// deadline reads as minus one. Negative means the contract stops
    /// existing before the claim can resolve, so no position in it could be
    /// held to the horizon. `None` for a subject that is not a dated
    /// contract — an equity has no expiry to fall short of — and the rule
    /// that needs it does not fire.
    pub contract_expiry_margin_days: Option<i64>,
}

/// The ontology version this build seeds. A recorded typing names it, so a
/// replay under a build whose ontology moved is refused rather than retyped.
pub const ONTOLOGY_VERSION: u32 = 1;

/// The ontology the symbolic stage types a hypothesis's subject against
/// (R12-REASON-1131): every asset class as an `Asset` type, the anomaly a
/// hypothesis starts from as an `Event` type, and the hypothesis's own
/// lifecycle as a `Lifecycle` type.
///
/// Seeded here, from the platform's own vocabulary, rather than loaded:
/// `OntologyRegistry` existed and was constructed only in its own tests, so
/// no reasoning step ever asked what kind of thing its subject was.
pub fn platform_ontology() -> Result<OntologyRegistry> {
    let mut registry = OntologyRegistry::new();
    let spec = |family, name: &str, description: String| OntologyTypeSpec {
        family,
        name: name.to_string(),
        version: ONTOLOGY_VERSION,
        schema: format!("qip-kernel/symbolic/{name}"),
        description,
    };
    for class in qip_financial::asset_class::AssetClass::ALL {
        registry.register(spec(
            OntologyTypeFamily::Asset,
            class.as_str(),
            format!(
                "an instrument the platform registered under the {} class",
                class.as_str()
            ),
        ))?;
    }
    registry.register(spec(
        OntologyTypeFamily::Event,
        "anomaly",
        "a detector's observation departing from its expectation".to_string(),
    ))?;
    registry.register(spec(
        OntologyTypeFamily::Lifecycle,
        "hypothesis",
        "a hypothesis from draft through review".to_string(),
    ))?;
    Ok(registry)
}

/// The subject's type as the ontology holds it, or the refusal.
fn type_subject(class: Option<&str>) -> Result<String> {
    let ontology = platform_ontology()?;
    let Some(class) = class else {
        return Err(Error::invalid(
            "the subject has no registered asset class, so the ontology cannot type it; register \
             the instrument under an asset class before reasoning about it",
        ));
    };
    let spec = ontology
        .get_latest(OntologyTypeFamily::Asset, class)
        .ok_or_else(|| {
            Error::invalid(format!(
                "the subject's asset class `{class}` is not a type the ontology holds; add it to \
                 platform_ontology before reasoning about it"
            ))
        })?;
    Ok(format!("asset/{} v{}", spec.name, spec.version))
}

/// The subject typed against the ontology, with the class it was typed from
/// so a replay re-types it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubjectTyping {
    pub class: Option<String>,
    pub typed: Step<String>,
}

/// A part that ran, or the reasoner's refusal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Step<T> {
    Derived(T),
    Refused(String),
}

impl<T> Step<T> {
    fn of(result: Result<T>) -> Self {
        match result {
            Ok(value) => Self::Derived(value),
            Err(error) => Self::Refused(error.message().to_string()),
        }
    }

    pub fn derived(&self) -> Option<&T> {
        match self {
            Self::Derived(value) => Some(value),
            Self::Refused(_) => None,
        }
    }
}

/// The rule pack's part: inputs, verdict, and whether re-evaluation agreed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleDerivation {
    /// The facts exactly as evaluated, so a replay re-runs the pack on them.
    pub inputs: Vec<(String, Value)>,
    pub verdict: Step<Verdict>,
    /// `RulePack::verify` on the verdict just produced.
    pub verified: bool,
    /// Whether the pack's eligibility matches the review's approval.
    pub agrees_with_review: bool,
}

/// One causal link the timeline was asked to hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkClaim {
    pub cause: String,
    pub effect: String,
    pub admitted: Step<()>,
}

/// The instants the timeline was built from, kept on the record so a replay
/// rebuilds the timeline rather than trusting the order it reports
/// (R12-REASON-1133).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TemporalInputs {
    pub detector: String,
    pub detected_at: Timestamp,
    pub formed_at: Timestamp,
    pub horizon: Duration,
    /// The linked catalyst's id and the instant it became knowable.
    pub catalyst: Option<(String, Timestamp)>,
    /// The subject's settlement convention and lag (REASON-3055). A trade
    /// entered when the hypothesis formed settles that long after it, and
    /// the timeline is asked to hold settlement before the deadline: a
    /// claim that resolves before its trade could settle is refused as a
    /// link. The lag counts calendar days, so the settlement instant is the
    /// earliest the convention allows — a weekend only makes it later.
    /// Absent on a record written before the field existed.
    #[serde(default)]
    pub settlement: Option<(String, Duration)>,
}

/// The temporal part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TemporalDerivation {
    pub links: Vec<LinkClaim>,
    /// How the anomaly and the hypothesis are ordered as of now.
    pub anomaly_to_hypothesis: Order,
    /// When the claim should have resolved.
    pub deadline: Timestamp,
    pub deadline_passed: bool,
    /// What the timeline was built from. Absent only on a record written
    /// before the field existed. Such a record still decodes, but
    /// [`verify`] refuses it by name: its timeline cannot be rebuilt, and a
    /// derivation whose ordering nobody can re-check is not a proof. Inventing
    /// the instants would make the check pass vacuously, and so did skipping
    /// it, which reported the record as identical.
    #[serde(default)]
    pub inputs: Option<TemporalInputs>,
}

/// What an intervention on the subject reaches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InterventionSummary {
    pub affected: BTreeSet<String>,
    pub paths: usize,
    /// The first [`INTERVENTION_PATHS_KEPT`] paths, with their evidence.
    pub kept: Vec<CausalPath>,
    /// The counterfactual explanations of the surprise (REASON-3056): for
    /// each abduced cause that was seen to move, best first and at most
    /// [`COUNTERFACTUALS_KEPT`], what the subject would have been had that
    /// cause not moved, by [`CausalGraph::counterfactual`] over every path
    /// the graph holds from the cause to the subject. Absent on a record
    /// written before the field existed, which decodes as none rather than
    /// refusing the whole derivation.
    #[serde(default)]
    pub counterfactuals: Vec<CounterfactualExplanation>,
}

/// "Had `cause` not moved, the subject would have been
/// `counterfactual_subject` rather than what was observed": one abduced
/// cause's counterfactual, as the derivation keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CounterfactualExplanation {
    pub cause: String,
    /// What the cause was seen to do.
    pub observed_move: f64,
    /// The subject's observed move, as the anomaly reported it.
    pub observed_subject: f64,
    /// The subject's move the graph implies had the cause stayed flat.
    pub counterfactual_subject: f64,
    /// Signed transmission from the cause to the subject over every path.
    pub total_effect: f64,
    /// How many paths that transmission was summed over.
    pub paths: usize,
}

/// Whether a derivation's parts can all be true together (ADR 0212,
/// R12-RES-7106).
///
/// `Infeasible` is a property of the record, not of the market: the platform
/// reasoned to a conclusion its own steps cannot support, so nothing that
/// moves money may rest on it. The trace that shows why is the rest of the
/// record, kept whole; this enum only says which parts collided.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Feasibility {
    /// No part contradicts another.
    Feasible,
    /// The derivation contradicts itself.
    Infeasible {
        /// What contradicted what, in the fixed order [`contradictions`]
        /// produces. Never empty.
        contradictions: Vec<String>,
        /// The alternates the reasoning engine offers: the revision tasks
        /// [`RulePack::contradictions`] raises on the facts the pack was
        /// given, each naming the rule or invariant on either side. Advisory
        /// (ADR 0125): a task changes no rule and nothing reads it to decide;
        /// it is what a reviewer revising the pack starts from. A search the
        /// engine refused is kept as its refusal, not as an empty list.
        alternates: Step<Vec<RevisionTask>>,
    },
}

/// One hypothesis's symbolic derivation, as the log keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SymbolicDerivation {
    pub cycle: u64,
    pub hypothesis_id: String,
    pub subject: String,
    pub at: Timestamp,
    pub rules: RuleDerivation,
    pub temporal: TemporalDerivation,
    /// `P(claim_holds | panel_supports = !contested)`, over the network the
    /// record names by its nodes.
    pub network_nodes: Vec<Node>,
    pub posterior: Step<BTreeMap<String, f64>>,
    /// "Could the platform act on this?" as a constraint problem: a
    /// satisfying assignment, or the minimal set of constraints that block it.
    pub acting: Step<Answer>,
    pub lifecycle: Step<Admissibility>,
    pub abduction: Step<Abduction>,
    pub intervention: Step<InterventionSummary>,
    /// What the derivation found does not hold (R12-REASON-3062): the rule
    /// pack's refusal, each timeline link refused, and each constraint in the
    /// conflict that blocks acting, as `rules: …`, `link a -> b: …` and
    /// `constraint: <id>`. Empty when every part held. Computed from the
    /// parts above by [`violated_assumptions`], so a replay recomputes it
    /// rather than trusting it.
    #[serde(default)]
    pub violated_assumptions: Vec<String>,
    /// How undecided the posterior leaves the claim: the Shannon entropy of
    /// `P(claim_holds)` divided by its maximum, so 0 is certain and 1 is a
    /// coin flip. `None` when the posterior was refused — no spread was
    /// computed, and a number here would be one nobody computed.
    #[serde(default)]
    pub uncertainty: Option<f64>,
    /// The subject typed against [`platform_ontology`] (R12-REASON-1131).
    /// `None` only on a record written before typing existed.
    #[serde(default)]
    pub subject_type: Option<SubjectTyping>,
    /// Whether the parts above hold together (ADR 0212), computed from them
    /// by [`feasibility_of`] so a replay recomputes it rather than trusting
    /// it. `None` only on a record written before the mark existed, which
    /// [`SymbolicDerivation::infeasibility`] reads as not established and so
    /// not acted on.
    #[serde(default)]
    pub feasibility: Option<Feasibility>,
}

impl EventBody for SymbolicDerivation {
    const TOPIC: Topic = Topic::EvidenceAttached;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{IDEMPOTENCY_PREFIX}{}", self.hypothesis_id))
    }
}

impl SymbolicDerivation {
    /// ADR 0125 decision 2: why the rule pack refuses a hypothesis that
    /// review approved, or `None` when it does not.
    ///
    /// Only the pack's hard stops veto: a live
    /// ceiling, one lot that exceeds the position budget, cash short of one
    /// lot. A tripped kill switch is deliberately not one: ACT's own
    /// `kill-switch` refusal gate is the recorded halt, and vetoing earlier
    /// would remove the refusal the gate journals. The other two conditions of `r10-eligible`, confidence under
    /// 5000 bp and contested evidence, are sizing inputs, not stops: a
    /// contested thesis is held small and scored because
    /// `Hypothesis::confidence_for_sizing` can only narrow (ADR 0150
    /// decision 14), and its effective confidence has already been narrowed
    /// by the conflict. Vetoing on them would reverse that decision and
    /// refuse every contested thesis the review approved at a size the
    /// platform chose to allow.
    ///
    /// `None` too for a hypothesis review did not approve (nothing to
    /// subtract from), for one the pack finds eligible, and for a pack that
    /// refused its input: `Step::Refused` is a finding about the evidence
    /// and not a rule firing.
    pub fn veto(&self) -> Option<String> {
        let Step::Derived(verdict) = &self.rules.verdict else {
            return None;
        };
        let fact = |name: &str| {
            self.rules
                .inputs
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v)
        };
        if fact("status") != Some(&Value::Text("approved".into())) {
            return None;
        }
        if verdict.facts.get("eligible") == Some(&Value::Bool(true)) {
            return None;
        }
        let mut stops = Vec::new();
        if fact("live_authority") == Some(&Value::Bool(true)) {
            stops.push("the ceiling is live");
        }
        if verdict.facts.get("lot_exceeds_budget") == Some(&Value::Bool(true)) {
            stops.push("one lot exceeds the position budget");
        }
        if verdict.facts.get("cash_short") == Some(&Value::Bool(true)) {
            stops.push("cash is short of one lot");
        }
        (!stops.is_empty()).then(|| {
            format!(
                "{RULE_PACK} v{RULE_PACK_VERSION} does not find it eligible ({})",
                stops.join("; ")
            )
        })
    }

    /// ADR 0212: why no money may move on this conclusion, or `None` when
    /// its parts hold together.
    ///
    /// Unlike [`Self::veto`] this does not ask whether review approved: a
    /// conclusion that contradicts itself is infeasible whatever any reviewer
    /// said of it, and the kernel forms no proposal on it either way. Fails
    /// closed on a record with no mark at all — a derivation whose
    /// feasibility was never established is not one money may rest on.
    /// Dissent never reaches here: a contested thesis is eligible or not by
    /// the pack's ordinary rules, and only a collision between parts is a
    /// contradiction (ADR 0150 decision 14 stands).
    pub fn infeasibility(&self) -> Option<String> {
        match &self.feasibility {
            Some(Feasibility::Feasible) => None,
            Some(Feasibility::Infeasible { contradictions, .. }) => Some(format!(
                "the symbolic derivation contradicts itself ({})",
                contradictions.join("; ")
            )),
            None => Some(
                "the symbolic derivation carries no feasibility mark, so whether its parts hold \
                 together was never established"
                    .to_string(),
            ),
        }
    }

    /// The derivation as one clause for the REASON stage's detail line.
    pub fn detail(&self) -> String {
        let eligibility = match &self.rules.verdict {
            Step::Derived(v) => {
                if v.facts.get("eligible") == Some(&Value::Bool(true)) {
                    "eligible"
                } else {
                    "not eligible"
                }
            }
            Step::Refused(_) => "refused",
        };
        let review = if self.rules.agrees_with_review {
            ""
        } else {
            " (disagrees with review)"
        };
        let lifecycle = match &self.lifecycle {
            Step::Derived(a) if a.is_admissible() => "admissible",
            Step::Derived(_) => "broken",
            Step::Refused(_) => "refused",
        };
        let temporal = if self
            .temporal
            .links
            .iter()
            .all(|l| matches!(l.admitted, Step::Derived(())))
        {
            "ordered"
        } else {
            "link refused"
        };
        let posterior = match &self.posterior {
            Step::Derived(p) => p
                .get("true")
                .map_or_else(|| "n/a".to_string(), |v| format!("{v:.2}")),
            Step::Refused(_) => "refused".to_string(),
        };
        let acting = match &self.acting {
            Step::Derived(Answer::Satisfied(_)) => "satisfiable".to_string(),
            Step::Derived(Answer::Unsatisfiable { conflict }) => {
                format!("blocked by {}", conflict.join("+"))
            }
            Step::Refused(_) => "refused".to_string(),
        };
        let abduction = match &self.abduction {
            Step::Derived(a) => format!("{} explanation(s)", a.candidates.len()),
            Step::Refused(_) => "no surprise to explain".to_string(),
        };
        let typed = match self.subject_type.as_ref().map(|t| &t.typed) {
            Some(Step::Derived(name)) => format!(", subject typed {name}"),
            Some(Step::Refused(_)) => ", subject untyped".to_string(),
            None => String::new(),
        };
        let feasibility = match &self.feasibility {
            Some(Feasibility::Infeasible { contradictions, .. }) => {
                format!(", infeasible ({} contradiction(s))", contradictions.len())
            }
            Some(Feasibility::Feasible) | None => String::new(),
        };
        format!(
            "; symbolic: rules {eligibility}{review}, lifecycle {lifecycle}, timeline {temporal}, \
             P(claim) {posterior}, acting {acting}, abduction {abduction}{typed}{feasibility}"
        )
    }
}

fn cmp(fact: &str, op: Op, value: Value) -> Cond {
    Cond::Cmp {
        fact: fact.to_string(),
        op,
        value,
    }
}

/// The advisory eligibility pack (REASON-3057), versioned and plain data.
///
/// Its two invariants are the paper boundary and the kill switch stated as
/// theorems: a pack revision that let `eligible` hold under live authority or
/// a tripped switch is refused at the step that would have reached it.
pub fn rule_pack() -> RulePack {
    let rule = |id: &str, when: Cond, fact: &str, value: Value| Rule {
        id: id.to_string(),
        when,
        fact: fact.to_string(),
        value,
    };
    RulePack {
        name: RULE_PACK.to_string(),
        version: RULE_PACK_VERSION,
        rules: vec![
            rule(
                "r10-eligible",
                Cond::And(vec![
                    cmp("status", Op::Eq, Value::Text("approved".into())),
                    cmp("confidence_bp", Op::Ge, Value::Int(5_000)),
                    cmp("contested", Op::Eq, Value::Bool(false)),
                    cmp("kill_switch", Op::Eq, Value::Bool(false)),
                    cmp("live_authority", Op::Eq, Value::Bool(false)),
                    // Market structure and accounting (REASON-3057). Stated
                    // as "not below zero" so an unreadable headroom, which is
                    // left out of the facts, withholds nothing it cannot
                    // support; a readable negative one withholds eligibility.
                    Cond::Not(Box::new(cmp("lot_headroom", Op::Lt, Value::Int(0)))),
                    Cond::Not(Box::new(cmp("cash_headroom", Op::Lt, Value::Int(0)))),
                    // Contracts (REASON-3057, v3): a dated contract that
                    // expires before the claim's horizon cannot carry the
                    // position to the horizon. Same "not below zero" shape,
                    // so an undated subject withholds nothing.
                    Cond::Not(Box::new(cmp("expiry_margin_days", Op::Lt, Value::Int(0)))),
                ]),
                "eligible",
                Value::Bool(true),
            ),
            rule(
                "r20-halted",
                cmp("kill_switch", Op::Eq, Value::Bool(true)),
                "halted",
                Value::Bool(true),
            ),
            rule(
                "r30-second-look",
                cmp("dissenters", Op::Ge, Value::Int(2)),
                "second_look",
                Value::Bool(true),
            ),
            rule(
                "r40-thin-evidence",
                cmp("evidence_items", Op::Lt, Value::Int(2)),
                "thin_evidence",
                Value::Bool(true),
            ),
            rule(
                "r60-lot-exceeds-position-budget",
                cmp("lot_headroom", Op::Lt, Value::Int(0)),
                "lot_exceeds_budget",
                Value::Bool(true),
            ),
            rule(
                "r61-cash-short-of-one-lot",
                cmp("cash_headroom", Op::Lt, Value::Int(0)),
                "cash_short",
                Value::Bool(true),
            ),
            rule(
                "r70-contract-expires-before-horizon",
                cmp("expiry_margin_days", Op::Lt, Value::Int(0)),
                "contract_expires_before_horizon",
                Value::Bool(true),
            ),
            rule(
                "r50-needs-corroboration",
                Cond::And(vec![
                    cmp("thin_evidence", Op::Eq, Value::Bool(true)),
                    cmp("eligible", Op::Eq, Value::Bool(true)),
                ]),
                "needs_corroboration",
                Value::Bool(true),
            ),
        ],
        invariants: vec![
            Invariant {
                id: "i10-no-eligibility-under-live-authority".into(),
                forbidden: Cond::And(vec![
                    cmp("eligible", Op::Eq, Value::Bool(true)),
                    cmp("live_authority", Op::Eq, Value::Bool(true)),
                ]),
            },
            Invariant {
                id: "i20-no-eligibility-while-halted".into(),
                forbidden: Cond::And(vec![
                    cmp("eligible", Op::Eq, Value::Bool(true)),
                    cmp("kill_switch", Op::Eq, Value::Bool(true)),
                ]),
            },
            Invariant {
                id: "i30-no-eligibility-on-negative-cash".into(),
                forbidden: Cond::And(vec![
                    cmp("eligible", Op::Eq, Value::Bool(true)),
                    cmp("cash_headroom", Op::Lt, Value::Int(0)),
                ]),
            },
            Invariant {
                id: "i40-no-eligibility-past-contract-expiry".into(),
                forbidden: Cond::And(vec![
                    cmp("eligible", Op::Eq, Value::Bool(true)),
                    cmp("expiry_margin_days", Op::Lt, Value::Int(0)),
                ]),
            },
        ],
    }
}

/// The hypothesis lifecycle as data (REASON-3054): the moves
/// `HypothesisStatus` may make, declared once and judged against.
pub fn hypothesis_lifecycle() -> Lifecycle {
    let moves = [
        ("draft", "approved"),
        ("draft", "rejected"),
        ("approved", "active"),
        ("approved", "resolved"),
        ("approved", "superseded"),
        ("active", "resolved"),
        ("active", "superseded"),
    ];
    Lifecycle {
        name: HYPOTHESIS_LIFECYCLE.to_string(),
        version: HYPOTHESIS_LIFECYCLE_VERSION,
        initial: BTreeSet::from(["draft".to_string()]),
        transitions: moves
            .iter()
            .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
            .collect(),
    }
}

/// The rule pack's facts for `input`.
///
/// Confidence crosses from a statistic to an integer here, in basis points,
/// because the pack is integer-only so that no float rounding can make two
/// evaluations of one record disagree.
fn rule_inputs(input: &SymbolicInput) -> Result<Vec<(String, Value)>> {
    if !(input.confidence.is_finite() && (0.0..=1.0).contains(&input.confidence)) {
        return Err(Error::invalid(format!(
            "hypothesis {} has confidence {} outside [0, 1]; refused rather than clamped",
            input.hypothesis_id, input.confidence
        )));
    }
    let count = |n: usize| Value::Int(i64::try_from(n).unwrap_or(i64::MAX));
    let mut inputs = vec![
        ("status".into(), Value::Text(input.status.clone())),
        (
            "confidence_bp".into(),
            Value::Int((input.confidence * 10_000.0).round() as i64),
        ),
        ("contested".into(), Value::Bool(input.contested)),
        ("dissenters".into(), count(input.dissenters)),
        ("evidence_items".into(), count(input.evidence_items)),
        ("kill_switch".into(), Value::Bool(input.kill_switch)),
        ("live_authority".into(), Value::Bool(input.live_authority)),
    ];
    for (name, headroom) in [
        ("lot_headroom", input.lot_headroom),
        ("cash_headroom", input.cash_headroom),
        ("expiry_margin_days", input.contract_expiry_margin_days),
    ] {
        if let Some(units) = headroom {
            inputs.push((name.into(), Value::Int(units)));
        }
    }
    Ok(inputs)
}

fn rules(pack: &RulePack, input: &SymbolicInput) -> RuleDerivation {
    let inputs = match rule_inputs(input) {
        Ok(inputs) => inputs,
        Err(error) => {
            return RuleDerivation {
                inputs: Vec::new(),
                verdict: Step::Refused(error.message().to_string()),
                verified: false,
                agrees_with_review: false,
            };
        }
    };
    let verdict = Step::of(pack.evaluate(&inputs));
    let verified = verdict
        .derived()
        .is_some_and(|v| pack.verify(&inputs, v).is_ok());
    let eligible = verdict
        .derived()
        .is_some_and(|v| v.facts.get("eligible") == Some(&Value::Bool(true)));
    RuleDerivation {
        agrees_with_review: verdict.derived().is_some() && eligible == (input.status == "approved"),
        inputs,
        verdict,
        verified,
    }
}

fn temporal(input: &SymbolicInput, now: Timestamp) -> TemporalDerivation {
    timeline_of(
        &input.hypothesis_id,
        TemporalInputs {
            detector: input.anomaly.detector.clone(),
            detected_at: input.anomaly.detected_at,
            formed_at: input.formed_at,
            horizon: input.horizon,
            catalyst: input.catalyst.clone(),
            settlement: input.settlement.clone(),
        },
        now,
    )
}

/// The timeline over recorded instants alone, so [`derive`] and [`verify`]
/// run the same construction and a replay cannot agree with a record by
/// building something else.
fn timeline_of(hypothesis_id: &str, input: TemporalInputs, now: Timestamp) -> TemporalDerivation {
    let anomaly = format!("anomaly:{}", input.detector);
    let hypothesis = format!("hypothesis:{hypothesis_id}");
    let deadline_id = format!("deadline:{hypothesis_id}");
    let deadline = input.formed_at.saturating_add(input.horizon);
    let mut timeline = Timeline::new();
    let mut links = Vec::new();
    let record = |timeline: &mut Timeline, id: &str, occurred_at, knowable_at| {
        timeline.record(temporal::Event {
            id: id.to_string(),
            occurred_at,
            knowable_at,
        })
    };
    let mut recorded = vec![
        record(
            &mut timeline,
            &anomaly,
            input.detected_at,
            input.detected_at,
        ),
        record(&mut timeline, &hypothesis, input.formed_at, input.formed_at),
        // A deadline is known the instant the claim is made.
        record(&mut timeline, &deadline_id, deadline, input.formed_at),
    ];
    let mut chain = vec![
        (anomaly.clone(), hypothesis.clone()),
        (hypothesis.clone(), deadline_id.clone()),
    ];
    if let Some((id, known_at)) = &input.catalyst {
        let catalyst = format!("catalyst:{id}");
        recorded.push(record(&mut timeline, &catalyst, *known_at, *known_at));
        chain.insert(0, (catalyst, anomaly.clone()));
    }
    if let Some((convention, lag)) = &input.settlement {
        let settlement = format!("settlement:{convention}:{hypothesis_id}");
        // Known the instant the claim is made, like the deadline: the
        // convention is reference data, not something learned later.
        recorded.push(record(
            &mut timeline,
            &settlement,
            input.formed_at.saturating_add(*lag),
            input.formed_at,
        ));
        chain.push((settlement, deadline_id.clone()));
    }
    for result in recorded {
        if let Err(error) = result {
            links.push(LinkClaim {
                cause: String::new(),
                effect: String::new(),
                admitted: Step::Refused(error.message().to_string()),
            });
        }
    }
    for (cause, effect) in chain {
        let admitted = Step::of(timeline.link(&cause, &effect, input.formed_at));
        links.push(LinkClaim {
            cause,
            effect,
            admitted,
        });
    }
    TemporalDerivation {
        links,
        anomaly_to_hypothesis: timeline.order(&anomaly, &hypothesis, now),
        deadline,
        deadline_passed: now >= deadline,
        inputs: Some(input),
    }
}

/// The network (REASON-3059): whether the anomaly is real, and two uncertain
/// rules hanging off it. The tables are declared, not fitted — this version
/// says how strongly a supportive panel and a holding claim each follow from
/// a real anomaly, and a revision changes them under a new record.
fn network_nodes(anomaly_confidence: f64) -> Vec<Node> {
    vec![
        Node {
            name: "anomaly_real".into(),
            states: vec!["true".into(), "false".into()],
            parents: Vec::new(),
            table: vec![vec![anomaly_confidence, 1.0 - anomaly_confidence]],
        },
        Node::uncertain_rule("panel_supports", "anomaly_real", 0.8, 0.3),
        Node::uncertain_rule("claim_holds", "anomaly_real", 0.65, 0.2),
    ]
}

/// The refusals the derivation's parts recorded, in a fixed order: the rule
/// pack, then the timeline's links, then the acting conflict.
pub fn violated_assumptions(
    rules: &RuleDerivation,
    temporal: &TemporalDerivation,
    acting: &Step<Answer>,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Step::Refused(why) = &rules.verdict {
        out.push(format!("rules: {why}"));
    }
    for link in &temporal.links {
        if let Step::Refused(why) = &link.admitted {
            out.push(format!("link {} -> {}: {why}", link.cause, link.effect));
        }
    }
    match acting {
        Step::Derived(Answer::Unsatisfiable { conflict }) => {
            out.extend(conflict.iter().map(|id| format!("constraint: {id}")));
        }
        Step::Refused(why) => out.push(format!("acting: {why}")),
        Step::Derived(Answer::Satisfied(_)) => {}
    }
    out
}

/// The symbolic contradictions among a derivation's parts (ADR 0212), in a
/// fixed order: the rule pack's, then the acting problem's.
///
/// Read off the record alone, so [`verify`] recomputes the same list. What
/// counts, and what deliberately does not:
///
/// - The pack refused facts it was actually given. [`rule_inputs`] refuses
///   only a malformed confidence and then records no facts, so a refused
///   verdict *with* facts is the pack's own evaluation colliding — a rule
///   deriving a fact against an established one, or a step reaching a state
///   an invariant forbids. A refused verdict without facts is a malformed
///   input, ADR 0150 decision 13's refused part, and is not listed here.
/// - A verdict the pack derived and could not re-derive.
/// - The pack finding acting eligible while the acting constraints are
///   unsatisfiable: the two parts answer "may the platform act" both ways.
///   An unsatisfiable acting problem on an *ineligible* verdict is the two
///   parts agreeing and is not a contradiction — that is how a contested or
///   low-confidence thesis reads, and listing it would block dissent.
///
/// A refused timeline link is not listed either: it is a finding about the
/// evidence, which ADR 0150 decision 13 narrows rather than blocks.
pub fn contradictions(rules: &RuleDerivation, acting: &Step<Answer>) -> Vec<String> {
    let mut out = Vec::new();
    match &rules.verdict {
        Step::Refused(why) if !rules.inputs.is_empty() => {
            out.push(format!(
                "rules: the pack refused the facts it was given: {why}"
            ));
        }
        Step::Derived(_) if !rules.verified => {
            out.push("rules: the verdict does not reproduce on re-evaluation".to_string());
        }
        _ => {}
    }
    let eligible = rules
        .verdict
        .derived()
        .is_some_and(|v| v.facts.get("eligible") == Some(&Value::Bool(true)));
    if eligible && let Step::Derived(Answer::Unsatisfiable { conflict }) = acting {
        out.push(format!(
            "acting: the pack finds acting eligible and the constraints {} make it unsatisfiable",
            conflict.join("+")
        ));
    }
    out
}

/// The feasibility mark for a derivation's parts, under `pack`: feasible
/// when [`contradictions`] finds nothing, and otherwise infeasible with the
/// pack's own revision search over the facts it was given as the alternates.
///
/// The search cites the derivation's own idempotency key as its evidence,
/// because the record is what a reviewer reads the task against.
pub fn feasibility_of(
    pack: &RulePack,
    hypothesis_id: &str,
    rules: &RuleDerivation,
    acting: &Step<Answer>,
) -> Feasibility {
    let contradictions = contradictions(rules, acting);
    if contradictions.is_empty() {
        return Feasibility::Feasible;
    }
    let evidence = [format!("{IDEMPOTENCY_PREFIX}{hypothesis_id}")];
    Feasibility::Infeasible {
        contradictions,
        alternates: Step::of(pack.contradictions(&rules.inputs, &evidence)),
    }
}

/// Normalised entropy of a posterior distribution: 0 when one state holds
/// all the mass, 1 when it is spread evenly. `None` for a refused posterior,
/// or one with fewer than two states, over which spread is undefined.
pub fn uncertainty_of(posterior: &Step<BTreeMap<String, f64>>) -> Option<f64> {
    let distribution = posterior.derived()?;
    if distribution.len() < 2 {
        return None;
    }
    let entropy: f64 = distribution
        .values()
        .filter(|p| **p > 0.0)
        .map(|p| -p * p.log2())
        .sum();
    Some(entropy / (distribution.len() as f64).log2())
}

/// `P(claim_holds | panel_supports = !contested)` over `nodes`, the one
/// computation [`derive`] records and [`verify`] repeats.
fn posterior_of(nodes: &[Node], contested: bool) -> Step<BTreeMap<String, f64>> {
    Step::of(Network::new(nodes.to_vec()).and_then(|network| {
        let evidence = BTreeMap::from([(
            "panel_supports".to_string(),
            if contested { "false" } else { "true" }.to_string(),
        )]);
        network.posterior("claim_holds", &evidence)
    }))
}

fn acting(eligible: bool, halted: bool) -> Result<Answer> {
    let holds = |id: &str, cond: Cond| Constraint {
        id: id.to_string(),
        form: Form::Holds(cond),
    };
    Problem {
        variables: vec![
            Variable {
                name: "act".into(),
                domain: vec![Value::Int(0), Value::Int(1)],
            },
            Variable {
                name: "eligible".into(),
                domain: vec![Value::Bool(eligible)],
            },
            Variable {
                name: "halted".into(),
                domain: vec![Value::Bool(halted)],
            },
        ],
        constraints: vec![
            holds(
                "acting-requires-eligibility",
                Cond::Or(vec![
                    cmp("act", Op::Eq, Value::Int(0)),
                    cmp("eligible", Op::Eq, Value::Bool(true)),
                ]),
            ),
            holds(
                "a-halt-forbids-acting",
                Cond::Or(vec![
                    cmp("act", Op::Eq, Value::Int(0)),
                    cmp("halted", Op::Eq, Value::Bool(false)),
                ]),
            ),
            Constraint {
                id: "posed-acting".into(),
                form: Form::Linear {
                    terms: vec![(1, "act".into())],
                    op: Op::Eq,
                    bound: 1,
                },
            },
        ],
    }
    .solve()
}

/// Run every reasoner over one hypothesis and return the record.
pub fn derive(input: SymbolicInput, causal: &CausalGraph, now: Timestamp) -> SymbolicDerivation {
    derive_under(&rule_pack(), input, causal, now)
}

/// [`derive`] with the rule pack named rather than assumed.
///
/// The kernel passes the pack it holds, which is [`rule_pack`]; the seam
/// exists so a pack revision whose rules collide can be driven through the
/// default cycle and shown to fail closed (ADR 0212). [`verify`] still
/// re-derives against this build's [`rule_pack`], so a record written under
/// any other pack does not replay as a proof.
pub fn derive_under(
    pack: &RulePack,
    input: SymbolicInput,
    causal: &CausalGraph,
    now: Timestamp,
) -> SymbolicDerivation {
    let rules = rules(pack, &input);
    let fact = |name: &str| {
        rules
            .verdict
            .derived()
            .is_some_and(|v| v.facts.get(name) == Some(&Value::Bool(true)))
    };
    let (eligible, halted) = (fact("eligible"), fact("halted") || input.kill_switch);
    let temporal = temporal(&input, now);
    let nodes = network_nodes(input.anomaly.confidence);
    let posterior = posterior_of(&nodes, input.contested);
    let lifecycle = Step::of(hypothesis_lifecycle().judge(&["draft", input.status.as_str()]));
    let abduction = Step::of(causal.abduce(
        &Surprise {
            target: input.subject.clone(),
            expected: input.anomaly.expected,
            observed: input.anomaly.observed,
        },
        &input.moves,
        now,
    ));
    // The counterfactual half of an explanation: an abduction says which
    // cause best accounts for the surprise; the counterfactual says what the
    // subject would have done had that cause stayed flat, over every path
    // and not only the edge the candidate was ranked on. A cause nobody saw
    // move has no observed value to hold hypothetical, and a cause the
    // graph refuses (a loop, no path) is left out rather than invented.
    let counterfactuals: Vec<CounterfactualExplanation> = abduction
        .derived()
        .map(|a| {
            a.candidates
                .iter()
                .filter_map(|c| c.observed_move.map(|m| (c.edge.cause.clone(), m)))
                .filter_map(|(cause, observed_move)| {
                    causal
                        .counterfactual(
                            &cause,
                            observed_move,
                            0.0,
                            &input.subject,
                            input.anomaly.observed,
                            now,
                        )
                        .ok()
                        .map(|cf| CounterfactualExplanation {
                            cause,
                            observed_move,
                            observed_subject: cf.observed_outcome,
                            counterfactual_subject: cf.counterfactual_outcome,
                            total_effect: cf.total_effect,
                            paths: cf.paths.len(),
                        })
                })
                .take(COUNTERFACTUALS_KEPT)
                .collect()
        })
        .unwrap_or_default();
    let intervention =
        Step::of(
            causal
                .intervene(&input.subject, now)
                .map(|i| InterventionSummary {
                    affected: i.affected,
                    paths: i.paths.len(),
                    kept: i.paths.into_iter().take(INTERVENTION_PATHS_KEPT).collect(),
                    counterfactuals,
                }),
        );
    let acting = Step::of(acting(eligible, halted));
    let subject_type = SubjectTyping {
        typed: Step::of(type_subject(input.subject_class.as_deref())),
        class: input.subject_class,
    };
    let violated_assumptions = violated_assumptions(&rules, &temporal, &acting);
    let uncertainty = uncertainty_of(&posterior);
    let feasibility = feasibility_of(pack, &input.hypothesis_id, &rules, &acting);
    SymbolicDerivation {
        cycle: input.cycle,
        hypothesis_id: input.hypothesis_id,
        subject: input.subject,
        at: now,
        rules,
        temporal,
        network_nodes: nodes,
        posterior,
        acting,
        lifecycle,
        abduction,
        intervention,
        violated_assumptions,
        uncertainty,
        subject_type: Some(subject_type),
        feasibility: Some(feasibility),
    }
}

/// Every derivation the log holds, oldest first, each re-derived by
/// [`verify`] against the platform's causal graph: a derivation that does
/// not reproduce is refused, because the trace is only a proof if re-running
/// the reasoners it cites yields it.
pub fn replay(platform: &crate::platform::Platform) -> Result<Vec<SymbolicDerivation>> {
    let envelopes = platform.replay_journal(
        &qip_events::EventFilter::new().topic(<SymbolicDerivation as EventBody>::TOPIC),
    )?;
    let world = platform.world();
    let mut out = Vec::new();
    for envelope in envelopes {
        // The envelope's key is `<topic name>:<body key>`; read the body's
        // key exactly rather than searching for the prefix anywhere in it.
        let key = envelope.idempotency_key();
        let ours = key
            .strip_prefix(<SymbolicDerivation as EventBody>::TOPIC.name())
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some_and(|k| k.starts_with(IDEMPOTENCY_PREFIX));
        if !ours {
            continue;
        }
        let derivation = envelope.decode::<SymbolicDerivation>()?.body;
        verify(&derivation, world.causal())?;
        out.push(derivation);
    }
    Ok(out)
}

/// [`replay`] over the event log instead of the cycle journal: every
/// derivation the log on disk holds, oldest first, each re-derived by
/// [`verify`] against the platform's causal graph.
///
/// The cycle journal is an in-memory transport that holds what this process
/// journaled; a platform assembled on a copy of a journal file — which is
/// what `qip replay` does — has an empty one, and [`replay`] on it re-derives
/// nothing and says so by returning nothing. An operator checking a proof
/// needs the record that outlives the process, which is the event log, so
/// this reads the log's own frames through the same two decoding steps the
/// kernel takes (R12-REASON-9005). A frame under the topic that is not a
/// derivation — the topic carries other evidence bodies — is skipped by its
/// key, as [`replay`] skips it.
pub fn replay_recorded(platform: &crate::platform::Platform) -> Result<Vec<SymbolicDerivation>> {
    let world = platform.world();
    let mut out = Vec::new();
    for frame in platform
        .event_log()
        .by_topic(<SymbolicDerivation as EventBody>::TOPIC)
    {
        let envelope = qip_streaming::envelope::StreamEnvelope::from_frame(frame)?;
        let key = envelope.idempotency_key();
        let ours = key
            .strip_prefix(<SymbolicDerivation as EventBody>::TOPIC.name())
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some_and(|k| k.starts_with(IDEMPOTENCY_PREFIX));
        if !ours {
            continue;
        }
        let derivation = envelope.decode::<SymbolicDerivation>()?.body;
        verify(&derivation, world.causal())?;
        out.push(derivation);
    }
    Ok(out)
}

/// The independent check (R12-REASON-9005): re-run each reasoner a
/// derivation cites on what the record itself holds, and refuse the record
/// at the first part that does not reproduce.
///
/// Before this only the rule verdict was re-checked, so a journaled
/// timeline, posterior, constraint answer or intervention could be edited —
/// or written by a build whose reasoner had drifted — and still replay as a
/// proof. Each part is checked on its own recorded inputs:
///
/// - the rule verdict, by [`RulePack::verify`] on the recorded facts;
/// - the timeline, rebuilt by the same construction from
///   [`TemporalInputs`] at the record's instant (R12-REASON-1133);
/// - the posterior, recomputed over the recorded network with the panel
///   evidence the recorded `contested` fact implies (R12-REASON-1134);
/// - the acting problem, re-solved from the verdict's `eligible`/`halted`
///   and the recorded `kill_switch` (R12-REASON-1136);
/// - the intervention, re-run on `causal` at the record's instant
///   (R12-REASON-1132). The graph answers point-in-time — an edge recorded
///   after the instant or retired by it is not walked — so a later claim
///   cannot change what the replay reaches. What is compared is the paths'
///   identity (edge index, cause, effect, mechanism) and, for evidence,
///   that what the record cites is a prefix of what the edge holds now:
///   re-estimation appends evidence to a live edge and never removes it.
///
/// - the violated assumptions and the uncertainty, recomputed from the
///   parts above (R12-REASON-3062);
/// - the feasibility mark, recomputed from the rule and acting parts under
///   this build's pack (ADR 0212).
///
/// The posterior and acting checks need the rule facts; a record whose rule
/// inputs were refused (a confidence outside `[0, 1]`) carries none, and its
/// posterior and acting are left unchecked rather than checked against an
/// invented `contested` or `kill_switch`.
pub fn verify(derivation: &SymbolicDerivation, causal: &CausalGraph) -> Result<()> {
    let id = &derivation.hypothesis_id;
    if let Some(typing) = &derivation.subject_type
        && Step::of(type_subject(typing.class.as_deref())) != typing.typed
    {
        return Err(Error::invalid(format!(
            "derivation for {id}: re-typed against this build's ontology (v{ONTOLOGY_VERSION}), \
             the subject does not match the journaled typing; the subject typing is not \
             reproducible"
        )));
    }
    let pack = rule_pack();
    if let Step::Derived(verdict) = &derivation.rules.verdict {
        if verdict.pack != pack.name || verdict.version != pack.version {
            return Err(Error::invalid(format!(
                "derivation for {id} cites rule pack {} v{}, which this build does not carry; \
                 replay it with the build that wrote it",
                verdict.pack, verdict.version
            )));
        }
        pack.verify(&derivation.rules.inputs, verdict)?;
    }
    // A timeline with no recorded instants is not a proof (R12-REASON-1133):
    // passing over it reported such a record as identical when nothing about
    // its ordering had been checked, so it is refused by name instead.
    let Some(inputs) = &derivation.temporal.inputs else {
        return Err(Error::invalid(format!(
            "derivation for {id}: the record carries no instants to rebuild its timeline from, \
             so the temporal derivation is not reproducible"
        )));
    };
    let again = timeline_of(id, inputs.clone(), derivation.at);
    if again != derivation.temporal {
        return Err(Error::invalid(format!(
            "derivation for {id}: the timeline rebuilt from its recorded instants does not \
             match the journaled one; the temporal derivation is not reproducible"
        )));
    }
    let fact = |name: &str| {
        derivation
            .rules
            .inputs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v == &Value::Bool(true))
    };
    if let (Some(contested), Some(kill_switch)) = (fact("contested"), fact("kill_switch")) {
        if posterior_of(&derivation.network_nodes, contested) != derivation.posterior {
            return Err(Error::invalid(format!(
                "derivation for {id}: the posterior recomputed over its recorded network does \
                 not match the journaled one; the posterior is not reproducible"
            )));
        }
        let verdict_fact = |name: &str| {
            derivation
                .rules
                .verdict
                .derived()
                .is_some_and(|v| v.facts.get(name) == Some(&Value::Bool(true)))
        };
        let (eligible, halted) = (
            verdict_fact("eligible"),
            verdict_fact("halted") || kill_switch,
        );
        if Step::of(acting(eligible, halted)) != derivation.acting {
            return Err(Error::invalid(format!(
                "derivation for {id}: re-solving the acting problem from its recorded verdict \
                 does not give the journaled answer; the constraint derivation is not \
                 reproducible"
            )));
        }
    }
    // After the parts they are computed from, so a refusal here means the
    // summary was edited, not that a part beneath it was.
    if violated_assumptions(&derivation.rules, &derivation.temporal, &derivation.acting)
        != derivation.violated_assumptions
    {
        return Err(Error::invalid(format!(
            "derivation for {id}: the refusals its parts record do not match its journaled \
             violated assumptions; the violated-assumptions list is not reproducible"
        )));
    }
    if let Some(recorded) = &derivation.feasibility
        && feasibility_of(&pack, id, &derivation.rules, &derivation.acting) != *recorded
    {
        return Err(Error::invalid(format!(
            "derivation for {id}: the contradictions its parts hold, or the pack's revision \
             search over its facts, do not match its journaled feasibility; the feasibility \
             mark is not reproducible"
        )));
    }
    if uncertainty_of(&derivation.posterior) != derivation.uncertainty {
        return Err(Error::invalid(format!(
            "derivation for {id}: the spread of its recorded posterior does not match its \
             journaled uncertainty; the uncertainty is not reproducible"
        )));
    }
    let again = causal.intervene(&derivation.subject, derivation.at);
    let reproduces = match (&derivation.intervention, &again) {
        (Step::Derived(recorded), Ok(rerun)) => {
            recorded.affected == rerun.affected
                && recorded.paths == rerun.paths.len()
                && recorded.kept.len() == rerun.paths.len().min(INTERVENTION_PATHS_KEPT)
                && recorded.kept.iter().zip(&rerun.paths).all(|(kept, path)| {
                    kept.target == path.target
                        && kept.edges.len() == path.edges.len()
                        && kept.edges.iter().zip(&path.edges).all(|(a, b)| {
                            a.index == b.index
                                && a.cause == b.cause
                                && a.effect == b.effect
                                && a.mechanism == b.mechanism
                                && b.evidence.starts_with(&a.evidence)
                        })
                })
        }
        (Step::Refused(_), Err(_)) => true,
        _ => false,
    };
    if !reproduces {
        return Err(Error::invalid(format!(
            "derivation for {id}: re-running the intervention on {} at the record's instant does \
             not reach what the journal says it reached; the intervention is not reproducible",
            derivation.subject
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(1_700_000_000 + secs)
    }

    fn input() -> SymbolicInput {
        SymbolicInput {
            cycle: 3,
            hypothesis_id: "hyp-3-AAA".into(),
            subject: "AAA".into(),
            status: "approved".into(),
            confidence: 0.62,
            contested: false,
            dissenters: 0,
            evidence_items: 3,
            kill_switch: false,
            live_authority: false,
            anomaly: AnomalyFacts {
                detector: "zscore".into(),
                detected_at: at(0),
                observed: 105.0,
                expected: 100.0,
                confidence: 0.7,
            },
            catalyst: None,
            formed_at: at(10),
            horizon: Duration::from_days(1),
            settlement: None,
            moves: BTreeMap::new(),
            subject_class: Some("equity".into()),
            lot_headroom: Some(5_000),
            cash_headroom: Some(900_000),
            contract_expiry_margin_days: None,
        }
    }

    /// REASON-3057, contracts: a hypothesis on a dated contract that expires
    /// before the claim's horizon is ruled against by the pack — the
    /// position could not be carried to the deadline — and the journaled
    /// record replays to the same verdict; a contract that outlives the
    /// horizon, or one expiring on the deadline's own day, is eligible, as
    /// is a subject with no expiry at all. Mutations, both run: drop the
    /// `expiry_margin_days` condition from `r10-eligible` — the expiring
    /// case derives `eligible` and the invariant `i40` refuses the pack, so
    /// `derived()` is `None` and the expect on it fails; drop `i40` as well
    /// — the expiring case is eligible and the `assert_ne!` fails. Each was
    /// restored byte for byte.
    #[test]
    fn a_hypothesis_whose_contract_expires_before_its_horizon_is_ruled_against_and_replays() {
        let eligible = |d: &SymbolicDerivation| {
            d.rules
                .verdict
                .derived()
                .and_then(|v| v.facts.get("eligible"))
                == Some(&Value::Bool(true))
        };
        // Premise: the same hypothesis on a contract with a month to run
        // is eligible, and the margin was read into the pack's inputs.
        let healthy = derive(
            SymbolicInput {
                contract_expiry_margin_days: Some(30),
                ..input()
            },
            &CausalGraph::default(),
            at(20),
        );
        assert!(eligible(&healthy), "{:?}", healthy.rules.verdict);
        assert!(
            healthy
                .rules
                .inputs
                .iter()
                .any(|(name, value)| name == "expiry_margin_days" && *value == Value::Int(30)),
            "the margin did not reach the pack: {:?}",
            healthy.rules.inputs
        );

        let expiring = derive(
            SymbolicInput {
                contract_expiry_margin_days: Some(-1),
                ..input()
            },
            &CausalGraph::default(),
            at(20),
        );
        let verdict = expiring
            .rules
            .verdict
            .derived()
            .expect("the pack evaluated");
        assert_ne!(verdict.facts.get("eligible"), Some(&Value::Bool(true)));
        assert_eq!(
            verdict.facts.get("contract_expires_before_horizon"),
            Some(&Value::Bool(true))
        );
        assert!(
            verdict
                .trace
                .iter()
                .any(|step| step.rule == "r70-contract-expires-before-horizon"),
            "{:?}",
            verdict.trace
        );
        assert!(expiring.rules.verified);
        assert!(!expiring.rules.agrees_with_review);
        assert!(
            matches!(expiring.acting, Step::Derived(Answer::Unsatisfiable { .. })),
            "{:?}",
            expiring.acting
        );
        verify(&expiring, &CausalGraph::default()).expect("the record replays");

        // On the deadline's day the contract still exists: eligible. And an
        // undated subject withholds nothing.
        for margin in [Some(0), None] {
            let d = derive(
                SymbolicInput {
                    contract_expiry_margin_days: margin,
                    ..input()
                },
                &CausalGraph::default(),
                at(20),
            );
            assert!(eligible(&d), "margin {margin:?}: {:?}", d.rules.verdict);
        }
    }

    /// REASON-3057: a hypothesis on an instrument whose smallest lot the book
    /// could not pay for, or may not hold, is ruled against by the pack, and
    /// the journaled record replays to the same verdict. Mutation: drop the
    /// two headroom conditions from `r10-eligible` — both breaking cases turn
    /// eligible and the first assertion of each fails.
    #[test]
    fn a_hypothesis_whose_smallest_lot_breaks_the_budget_or_the_cash_is_ruled_against_and_replays()
    {
        // Premise: the same hypothesis with room is eligible.
        let healthy = derive(input(), &CausalGraph::default(), at(20));
        assert_eq!(
            healthy
                .rules
                .verdict
                .derived()
                .and_then(|v| v.facts.get("eligible")),
            Some(&Value::Bool(true))
        );
        for (name, breaking, fact) in [
            (
                "lot",
                SymbolicInput {
                    lot_headroom: Some(-1),
                    ..input()
                },
                "lot_exceeds_budget",
            ),
            (
                "cash",
                SymbolicInput {
                    cash_headroom: Some(-1),
                    ..input()
                },
                "cash_short",
            ),
        ] {
            let d = derive(breaking, &CausalGraph::default(), at(20));
            let verdict = d.rules.verdict.derived().expect("the pack evaluated");
            assert_ne!(
                verdict.facts.get("eligible"),
                Some(&Value::Bool(true)),
                "{name}"
            );
            assert_eq!(verdict.facts.get(fact), Some(&Value::Bool(true)), "{name}");
            assert!(d.rules.verified, "{name}");
            // The review approved it and the pack did not: the record says so.
            assert!(!d.rules.agrees_with_review, "{name}");
            verify(&d, &CausalGraph::default()).expect("the record replays");
        }
        // An unreadable headroom is left out and withholds nothing.
        let unread = derive(
            SymbolicInput {
                lot_headroom: None,
                cash_headroom: None,
                contract_expiry_margin_days: None,
                ..input()
            },
            &CausalGraph::default(),
            at(20),
        );
        assert_eq!(
            unread
                .rules
                .verdict
                .derived()
                .and_then(|v| v.facts.get("eligible")),
            Some(&Value::Bool(true))
        );
    }

    #[test]
    fn an_approved_confident_uncontested_hypothesis_is_eligible_and_the_verdict_reproduces() {
        let d = derive(input(), &CausalGraph::default(), at(20));
        let verdict = d.rules.verdict.derived().expect("the pack evaluated");
        assert_eq!(verdict.facts.get("eligible"), Some(&Value::Bool(true)));
        assert_eq!(
            verdict.trace.first().map(|s| s.rule.as_str()),
            Some("r10-eligible")
        );
        assert!(d.rules.verified);
        assert!(d.rules.agrees_with_review);
        assert!(matches!(d.acting, Step::Derived(Answer::Satisfied(_))));
    }

    #[test]
    fn a_tripped_kill_switch_makes_the_hypothesis_ineligible_and_names_the_constraint_that_blocks_acting()
     {
        let mut i = input();
        i.kill_switch = true;
        let d = derive(i, &CausalGraph::default(), at(20));
        let verdict = d.rules.verdict.derived().expect("the pack evaluated");
        // Premise: the switch was read.
        assert_eq!(verdict.facts.get("halted"), Some(&Value::Bool(true)));
        assert_eq!(verdict.facts.get("eligible"), None);
        assert!(!d.rules.agrees_with_review);
        match &d.acting {
            Step::Derived(Answer::Unsatisfiable { conflict }) => {
                assert!(
                    conflict.contains(&"posed-acting".to_string())
                        && (conflict.contains(&"acting-requires-eligibility".to_string())
                            || conflict.contains(&"a-halt-forbids-acting".to_string())),
                    "{conflict:?}"
                );
            }
            other => panic!("expected a blocked act, got {other:?}"),
        }
    }

    #[test]
    fn live_authority_never_yields_eligibility() {
        let mut i = input();
        i.live_authority = true;
        let d = derive(i, &CausalGraph::default(), at(20));
        let verdict = d.rules.verdict.derived().expect("the pack evaluated");
        assert_eq!(verdict.facts.get("eligible"), None);
    }

    #[test]
    fn a_hypothesis_formed_before_its_anomaly_was_detected_is_refused_by_the_timeline() {
        let mut i = input();
        i.anomaly.detected_at = at(30);
        let d = derive(i, &CausalGraph::default(), at(40));
        let link = d
            .temporal
            .links
            .iter()
            .find(|l| l.cause.starts_with("anomaly:"))
            .expect("the anomaly link was claimed");
        assert!(matches!(link.admitted, Step::Refused(_)), "{link:?}");
        assert_eq!(d.temporal.anomaly_to_hypothesis, Order::After);
    }

    #[test]
    fn a_claim_that_resolves_before_its_trade_could_settle_records_the_settlement_link_as_refused()
    {
        // R12-REASON-3055. The failure this guards: the timeline ordered the
        // anomaly, the hypothesis and its deadline, and never the trade's
        // settlement, so a one-day claim on a T+2 instrument read as
        // coherent although no position entered on it could have settled
        // before reality answered.
        let at = |days: i64| Timestamp::from_secs(1_700_000_000 + days * 86_400);
        let mut t2 = input();
        t2.settlement = Some(("T+2".into(), Duration::from_days(2)));
        let derivation = derive(t2.clone(), &CausalGraph::new(), at(5));
        let settlement = format!("settlement:T+2:{}", t2.hypothesis_id);
        let deadline = format!("deadline:{}", t2.hypothesis_id);
        let link = derivation
            .temporal
            .links
            .iter()
            .find(|l| l.cause == settlement && l.effect == deadline)
            .expect("the settlement was posed against the deadline");
        assert!(matches!(link.admitted, Step::Refused(_)), "{link:?}");
        let token = format!("link {settlement} -> {deadline}: ");
        assert!(
            derivation
                .violated_assumptions
                .iter()
                .any(|v| v.starts_with(&token)),
            "{:?}",
            derivation.violated_assumptions
        );
        // The same claim on a same-day convention settles inside its
        // horizon and the link is admitted, so the refusal above is the
        // lag's and not the link's existence.
        let mut t0 = input();
        t0.settlement = Some(("T+0".into(), Duration::ZERO));
        let same_day = derive(t0.clone(), &CausalGraph::new(), at(5));
        let admitted = same_day
            .temporal
            .links
            .iter()
            .find(|l| l.cause == format!("settlement:T+0:{}", t0.hypothesis_id))
            .expect("posed");
        assert!(
            matches!(admitted.admitted, Step::Derived(())),
            "{admitted:?}"
        );
    }

    #[test]
    fn a_contested_panel_lowers_the_posterior_that_the_claim_holds() {
        let calm = derive(input(), &CausalGraph::default(), at(20));
        let mut i = input();
        i.contested = true;
        let contested = derive(i, &CausalGraph::default(), at(20));
        let p = |d: &SymbolicDerivation| d.posterior.derived().and_then(|m| m.get("true").copied());
        let (calm, contested) = (
            p(&calm).expect("posterior"),
            p(&contested).expect("posterior"),
        );
        assert!(calm > contested, "{calm} vs {contested}");
    }

    #[test]
    fn a_status_the_lifecycle_does_not_reach_from_draft_is_named_as_the_first_break() {
        let mut i = input();
        i.status = "active".into();
        let d = derive(i, &CausalGraph::default(), at(20));
        let a = d.lifecycle.derived().expect("judged");
        assert_eq!(
            a.first_break.as_ref().map(|b| b.to.as_str()),
            Some("active")
        );
    }

    #[test]
    fn a_surprise_with_no_causal_claim_into_it_is_abduced_to_no_candidates_and_kept() {
        let d = derive(input(), &CausalGraph::default(), at(20));
        let a = d.abduction.derived().expect("there was a surprise");
        assert!(a.candidates.is_empty());
        let mut i = input();
        i.anomaly.observed = i.anomaly.expected;
        let none = derive(i, &CausalGraph::default(), at(20));
        assert!(matches!(none.abduction, Step::Refused(_)));
    }
}
