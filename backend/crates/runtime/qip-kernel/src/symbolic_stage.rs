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
//! # Advisory, and structurally so
//!
//! [`derive`] takes its inputs by value and the causal graph by shared
//! reference and returns a record. It holds nothing that reaches a thesis, a
//! size or an order, and the kernel calls it after REASON has already decided
//! — so a rule pack that disagrees with the review is *recorded* as
//! disagreeing (`agrees_with_review`), never acted on. Promoting any part of
//! this to a control is a sizing decision for an ADR, not an implication of a
//! derivation existing; pre-trade checks stay deterministic and stay where
//! `Determinism::Required` puts them.
//!
//! # Why every part refuses rather than guesses
//!
//! Each reasoner already refuses malformed input — an effect before its
//! cause, a posterior on impossible evidence, a surprise that is not one. A
//! refusal is a finding about this hypothesis and lands in the record as
//! [`Step::Refused`] with the reasoner's own sentence; the other parts still
//! run. A derivation that silently dropped the part that refused would read
//! as one where that reasoner agreed.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_reasoning_engine::constraint::{Answer, Constraint, Form, Problem, Variable};
use qip_reasoning_engine::lifecycle::{Admissibility, Lifecycle};
use qip_reasoning_engine::network::{Network, Node};
use qip_reasoning_engine::rules::{Cond, Invariant, Op, Rule, RulePack, Value, Verdict};
use qip_reasoning_engine::temporal::{self, Order, Timeline};
use qip_world_model::causal::CausalGraph;
use qip_world_model::inference::{Abduction, CausalPath, Surprise};
use serde::{Deserialize, Serialize};

/// The rule pack's name and version, cited by every verdict.
pub const RULE_PACK: &str = "reason/advisory-eligibility";
pub const RULE_PACK_VERSION: u32 = 1;

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

/// The temporal part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TemporalDerivation {
    pub links: Vec<LinkClaim>,
    /// How the anomaly and the hypothesis are ordered as of now.
    pub anomaly_to_hypothesis: Order,
    /// When the claim should have resolved.
    pub deadline: Timestamp,
    pub deadline_passed: bool,
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
}

impl EventBody for SymbolicDerivation {
    const TOPIC: Topic = Topic::EvidenceAttached;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{IDEMPOTENCY_PREFIX}{}", self.hypothesis_id))
    }
}

impl SymbolicDerivation {
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
        format!(
            "; symbolic: rules {eligibility}{review}, lifecycle {lifecycle}, timeline {temporal}, \
             P(claim) {posterior}, acting {acting}, abduction {abduction}"
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
    Ok(vec![
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
    ])
}

fn rules(input: &SymbolicInput) -> RuleDerivation {
    let pack = rule_pack();
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
    let anomaly = format!("anomaly:{}", input.anomaly.detector);
    let hypothesis = format!("hypothesis:{}", input.hypothesis_id);
    let deadline_id = format!("deadline:{}", input.hypothesis_id);
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
            input.anomaly.detected_at,
            input.anomaly.detected_at,
        ),
        record(&mut timeline, &hypothesis, input.formed_at, input.formed_at),
        // A deadline is known the instant the claim is made.
        record(&mut timeline, &deadline_id, deadline, input.formed_at),
    ];
    let mut chain = vec![
        (anomaly.clone(), hypothesis.clone()),
        (hypothesis.clone(), deadline_id),
    ];
    if let Some((id, known_at)) = &input.catalyst {
        let catalyst = format!("catalyst:{id}");
        recorded.push(record(&mut timeline, &catalyst, *known_at, *known_at));
        chain.insert(0, (catalyst, anomaly.clone()));
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
    let rules = rules(&input);
    let fact = |name: &str| {
        rules
            .verdict
            .derived()
            .is_some_and(|v| v.facts.get(name) == Some(&Value::Bool(true)))
    };
    let (eligible, halted) = (fact("eligible"), fact("halted") || input.kill_switch);
    let temporal = temporal(&input, now);
    let nodes = network_nodes(input.anomaly.confidence);
    let posterior = Step::of(Network::new(nodes.clone()).and_then(|network| {
        let evidence = BTreeMap::from([(
            "panel_supports".to_string(),
            if input.contested { "false" } else { "true" }.to_string(),
        )]);
        network.posterior("claim_holds", &evidence)
    }));
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
    SymbolicDerivation {
        cycle: input.cycle,
        hypothesis_id: input.hypothesis_id,
        subject: input.subject,
        at: now,
        rules,
        temporal,
        network_nodes: nodes,
        posterior,
        acting: Step::of(acting(eligible, halted)),
        lifecycle,
        abduction,
        intervention,
    }
}

/// Every derivation the log holds, oldest first, each re-derived where it can
/// be: the rule pack is re-evaluated on the recorded inputs and a verdict
/// that does not reproduce is refused, because the trace is only a proof if
/// re-running the pack it cites yields it.
pub fn replay(platform: &crate::platform::Platform) -> Result<Vec<SymbolicDerivation>> {
    let envelopes = platform.replay_journal(
        &qip_events::EventFilter::new().topic(<SymbolicDerivation as EventBody>::TOPIC),
    )?;
    let pack = rule_pack();
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
        if let Step::Derived(verdict) = &derivation.rules.verdict {
            if verdict.pack != pack.name || verdict.version != pack.version {
                return Err(Error::invalid(format!(
                    "derivation for {} cites rule pack {} v{}, which this build does not carry; \
                     replay it with the build that wrote it",
                    derivation.hypothesis_id, verdict.pack, verdict.version
                )));
            }
            pack.verify(&derivation.rules.inputs, verdict)?;
        }
        out.push(derivation);
    }
    Ok(out)
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
            moves: BTreeMap::new(),
        }
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
