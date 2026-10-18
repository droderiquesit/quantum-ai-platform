//! R12-AGENCY-6083 / 6084 / 6087 / 6088 / 6102, and §24.3's nine axes:
//! interventions computed by graph surgery, effect sizes learned from data,
//! candidates the planner generates and simulates across competing models
//! and scenario branches, estimators that carry their identification, and a
//! ranker that weighs every axis it is given.
//!
//! Each test names the mutation that should make it fail.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use qip_agency::affordance::{AffordanceGraph, Method, ToolEdgeDraft};
use qip_agency::attribution::{Effect, EffectAttribution, HorizonObservation, Identification};
use qip_agency::causal::{
    Direction, Objective, Scenario, StructuralLink, StructuralModel, StructuralModelDecl, simulate,
};
use qip_agency::comparison::ComparisonDraft;
use qip_agency::engine::{
    Adapter, EdgeDecl, LeverEvidence, Outcome, ProposalDraft, Request, VariableDecl, run,
};
use qip_agency::estimate::{self, EstimatorKind};
use qip_agency::generate::{GenerateDecl, StructuralDecl};
use qip_agency::goal::{GoalClass, GoalSpecDraft};
use qip_agency::plan::{ActingIdentity, Candidate, PlanNode, Proposal, Step, select};
use qip_core::{Decimal, Error};
use std::collections::{BTreeMap, BTreeSet};

fn d(n: i64) -> Decimal {
    Decimal::from_int(n)
}

fn tenths(n: i128) -> Decimal {
    Decimal::from_scaled(n, 1).unwrap()
}

fn link(cause: &str, effect: &str, coefficient: f64) -> StructuralLink {
    StructuralLink {
        cause: cause.into(),
        effect: effect.into(),
        coefficient,
    }
}

fn model(name: &str, links: Vec<StructuralLink>, noise: &[(&str, f64)]) -> StructuralModel {
    StructuralModel::new(StructuralModelDecl {
        name: name.into(),
        links,
        variables: Vec::new(),
        intercepts: BTreeMap::new(),
        noise_variance: noise.iter().map(|(v, n)| (v.to_string(), *n)).collect(),
    })
    .unwrap()
}

// ---- R12-AGENCY-6084: do(x) by graph surgery ------------------------------

/// The failure prevented: a "lever effect" read off an association that a
/// confounder inflates. Z drives both X and Y; X's own effect on Y is 0.5,
/// and the association X shows with Y is 1.5.
///
/// Mutation: make `StructuralModel::evaluate` keep adding a fixed variable's
/// parents to its fixed value — `do(x)` that does not cut z -> x. The
/// surgery assertions at the end then fail: with z at 1 the intervened x
/// reads 4, not 3. (Skipping the `fixed` insert in `intervene` fails earlier,
/// at the clean-model premise, because the do-effect then reads 0.)
///
/// Until this round's mutation pass the test asserted only
/// `interventional_effect`, which evaluates with every exogenous term at zero
/// — so z is 0, the confounder never shows, and an uncut x gave the same 0.5.
/// The uncut-parents mutation passed it; only `coefficient`'s own read of the
/// `fixed` map stood in for surgery, which is a statement about bookkeeping
/// rather than about the world the intervened model computes.
#[test]
fn an_interventions_effect_is_computed_by_graph_surgery_not_read_off_the_association() {
    let noise = [("z", 1.0), ("x", 1.0), ("y", 1.0)];
    // Premise: with no confounder the two coincide, so a difference below is
    // the confounding and not an arithmetic quirk.
    let clean = model("clean", vec![link("x", "y", 0.5)], &noise[1..]);
    assert!((clean.interventional_effect("x", "y").unwrap() - 0.5).abs() < 1e-12);
    assert!((clean.observational_slope("x", "y").unwrap() - 0.5).abs() < 1e-12);

    let confounded = model(
        "confounded",
        vec![
            link("z", "x", 1.0),
            link("z", "y", 2.0),
            link("x", "y", 0.5),
        ],
        &noise,
    );
    let caused = confounded.interventional_effect("x", "y").unwrap();
    let associated = confounded.observational_slope("x", "y").unwrap();
    assert!((caused - 0.5).abs() < 1e-12, "do-effect {caused}");
    assert!((associated - 1.5).abs() < 1e-12, "association {associated}");

    // Surgery cuts x's incoming edge in the intervened model only.
    let cut = confounded
        .intervene(&BTreeMap::from([("x".to_string(), 3.0)]))
        .unwrap();
    assert_eq!(cut.coefficient("z", "x"), None);
    assert_eq!(confounded.coefficient("z", "x"), Some(1.0));
    // And the cut is in what the model computes, not only in what it reports:
    // with the confounder at 1, the untouched model moves x with it, and the
    // intervened one holds x at 3 whatever z does.
    let z_up = BTreeMap::from([("z".to_string(), 1.0)]);
    let factual = confounded.evaluate(&z_up);
    assert!((factual["x"] - 1.0).abs() < 1e-12, "premise: {factual:?}");
    let world = cut.evaluate(&z_up);
    assert!(
        (world["x"] - 3.0).abs() < 1e-12,
        "do(x = 3) held x: {world:?}"
    );
    assert!(
        (world["y"] - (2.0 * 1.0 + 0.5 * 3.0)).abs() < 1e-12,
        "y under do(x = 3): {world:?}"
    );
}

/// Abduction, action, prediction: the same world replayed under the
/// intervention. Mutation: have `counterfactual` evaluate with zero noise
/// instead of the abducted terms — y then loses its observed noise and the
/// counterfactual is not 0.5 above the factual.
#[test]
fn a_counterfactual_replays_the_observed_world_under_the_intervention() {
    let m = model(
        "m",
        vec![
            link("z", "x", 1.0),
            link("z", "y", 2.0),
            link("x", "y", 0.5),
        ],
        &[("z", 1.0), ("x", 1.0), ("y", 1.0)],
    );
    // z = 1, x = 1 + 0.3, y = 2 + 0.5 * 1.3 + 0.7
    let observed = BTreeMap::from([
        ("z".to_string(), 1.0),
        ("x".to_string(), 1.3),
        ("y".to_string(), 3.35),
    ]);
    let noise = m.abduce(&observed).unwrap();
    assert!((noise["y"] - 0.7).abs() < 1e-12, "premise: {noise:?}");
    let world = m
        .counterfactual(&observed, &BTreeMap::from([("x".to_string(), 2.3)]))
        .unwrap();
    assert!((world["y"] - 3.85).abs() < 1e-12, "{world:?}");
    assert!(
        (world["z"] - 1.0).abs() < 1e-12,
        "upstream of x is untouched"
    );
    assert!(
        m.abduce(&BTreeMap::new()).is_err(),
        "a partial world is refused"
    );
}

/// Mutation: drop the cycle check in `topological` — the cyclic model is
/// then built and this assertion fails.
#[test]
fn a_cyclic_or_malformed_structure_is_refused() {
    let decl = |links| StructuralModelDecl {
        name: "m".into(),
        links,
        variables: Vec::new(),
        intercepts: BTreeMap::new(),
        noise_variance: BTreeMap::new(),
    };
    assert!(
        StructuralModel::new(decl(vec![link("a", "b", 1.0)])).is_ok(),
        "premise"
    );
    let err =
        StructuralModel::new(decl(vec![link("a", "b", 1.0), link("b", "a", 1.0)])).unwrap_err();
    assert!(err.to_string().contains("cycle"), "{err}");
    assert!(StructuralModel::new(decl(vec![link("a", "a", 1.0)])).is_err());
    assert!(StructuralModel::new(decl(vec![link("a", "b", f64::NAN)])).is_err());
}

// ---- R12-AGENCY-6083: effect sizes learned, constraints still first -------

/// Deterministic noise in [-0.5, 0.5).
fn noise(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*seed >> 11) as f64 / (1u64 << 53) as f64) - 0.5
}

fn observations(rows: usize) -> Vec<BTreeMap<String, f64>> {
    let mut seed = 6083;
    (0..rows)
        .map(|_| {
            let z = noise(&mut seed) * 4.0;
            let x = 1.0 * z + noise(&mut seed) * 2.0;
            let y = 2.0 * z + 0.5 * x + noise(&mut seed) + 3.0;
            BTreeMap::from([
                ("z".to_string(), z),
                ("x".to_string(), x),
                ("y".to_string(), y),
            ])
        })
        .collect()
}

/// The structure is declared; the sizes are learned. Mutation: fit `y` on
/// `x` alone (drop `z` from its regressors) — the learned x→y coefficient
/// absorbs the confounder and lands near 2.1, not 0.5.
#[test]
fn effect_sizes_are_learned_from_data_over_the_declared_structure() {
    let structure = vec![
        ("z".to_string(), "x".to_string()),
        ("z".to_string(), "y".to_string()),
        ("x".to_string(), "y".to_string()),
    ];
    let data = observations(400);
    assert_eq!(data.len(), 400, "premise");
    let fitted = StructuralModel::fit("learned", &structure, &data).unwrap();
    // x->y is asserted before z->y is read, so the named mutation fails on
    // the confounded figure it produces rather than on a missing edge.
    let xy = fitted.coefficient("x", "y").unwrap();
    assert!((xy - 0.5).abs() < 0.1, "x->y learned {xy}");
    let zy = fitted.coefficient("z", "y").unwrap();
    assert!((zy - 2.0).abs() < 0.1, "z->y learned {zy}");
    assert!((fitted.interventional_effect("x", "y").unwrap() - xy).abs() < 1e-12);

    // Too few rows to fit two parents and leave a residual.
    assert!(StructuralModel::fit("thin", &structure, &data[..3]).is_err());
    // A row missing a variable is refused, not skipped.
    let mut holed = data.clone();
    holed[7].remove("z");
    assert!(StructuralModel::fit("holed", &structure, &holed).is_err());
}

// ---- R12-AGENCY-6088: one result per model and scenario --------------------

/// Mutation: in `simulate`, `break` after the first scenario (or the first
/// model) — the count is then below models × scenarios and the dissenting
/// model's branch is missing.
#[test]
fn a_candidate_is_simulated_once_per_competing_model_and_scenario_branch() {
    let optimistic = model("optimistic", vec![link("x", "y", 0.5)], &[]);
    let pessimistic = model("pessimistic", vec![link("x", "y", -0.2)], &[]);
    let scenarios: Vec<Scenario> = ["calm", "stressed", "gapped"]
        .iter()
        .enumerate()
        .map(|(i, name)| Scenario {
            name: name.to_string(),
            exogenous: BTreeMap::from([("y".to_string(), i as f64)]),
        })
        .collect();
    let objective = Objective {
        variable: "y".into(),
        direction: Direction::Increase,
    };
    let shift = BTreeMap::from([("x".to_string(), 2.0)]);
    let results = simulate(&[optimistic, pessimistic], &scenarios, &objective, &shift).unwrap();
    assert_eq!(results.len(), 6);
    let pairs: BTreeSet<(String, String)> = results
        .iter()
        .map(|r| (r.model.clone(), r.scenario.clone()))
        .collect();
    assert_eq!(pairs.len(), 6, "every (model, scenario) once");
    let dissent: Vec<f64> = results
        .iter()
        .filter(|r| r.model == "pessimistic")
        .map(|r| r.effect)
        .collect();
    assert_eq!(dissent.len(), 3);
    assert!(
        dissent.iter().all(|e| (*e + 0.4).abs() < 1e-12),
        "{dissent:?}"
    );
    // Each branch is judged against its own no-action world.
    assert!(
        results
            .iter()
            .any(|r| (r.no_action - results[0].no_action).abs() > 1e-12)
    );

    assert!(simulate(&[], &scenarios, &objective, &shift).is_err());
    let none: Vec<Scenario> = Vec::new();
    let one = model("one", vec![link("x", "y", 0.5)], &[]);
    assert!(simulate(&[one], &none, &objective, &shift).is_err());
}

// ---- R12-AGENCY-6087: the planner generates candidates --------------------

fn goal(envelope: i64) -> GoalSpecDraft {
    let s = |x: &str| Some(x.to_string());
    GoalSpecDraft {
        class: Some(GoalClass::Liquidity),
        target_state: s("spread under 5 bps"),
        entities_affected: Some(vec!["venue-a".into()]),
        time_horizon_secs: Some(3600),
        success_metric: s("median spread"),
        acceptable_uncertainty: Some(tenths(2)),
        budget: Some(d(100)),
        risk_envelope: Some(d(envelope)),
        jurisdictions: Some(vec!["US".into()]),
        acting_identity: s("desk-agent"),
        prohibited_side_effects: Some(vec![]),
        prohibited_methods: Some(vec![]),
        stop_conditions: Some(vec!["spread widens".into()]),
        rollback_conditions: Some(vec!["restore the prior quote width".into()]),
    }
}

fn edge(tool: &str, method: Method) -> ToolEdgeDraft {
    ToolEdgeDraft {
        tool: Some(tool.into()),
        method: Some(method),
        reversible: Some(true),
        latency_ms: Some(1500),
        cost: Some(d(10)),
        side_effects: Some(vec![]),
        dependencies: Some(vec![]),
        authority: Some("quote".into()),
    }
}

/// Two levers on the spread, three tools (one deceptive), and a structural
/// model in which both honest levers narrow it. No proposals.
fn generating_request(coefficient: f64, envelope: i64, exposure: i64) -> Request {
    let variable = |name: &str, controllable: bool| VariableDecl {
        name: name.into(),
        observable: true,
        controllable,
        owned: false,
    };
    Request {
        goal: goal(envelope).build().unwrap(),
        targets: vec!["spread".into()],
        variables: vec![
            variable("quote_size", true),
            variable("tick_rest", true),
            variable("spread", false),
        ],
        causes: vec![
            ("quote_size".into(), "spread".into()),
            ("tick_rest".into(), "spread".into()),
        ],
        tool_edges: vec![
            EdgeDecl {
                variable: "quote_size".into(),
                edge: edge("quoter", Method::Market),
            },
            EdgeDecl {
                variable: "tick_rest".into(),
                edge: edge("rester", Method::Market),
            },
            EdgeDecl {
                variable: "quote_size".into(),
                edge: edge("layerer", Method::Spoofing),
            },
        ],
        identity: ActingIdentity {
            name: "desk-agent".into(),
            authorities: BTreeSet::from(["quote".to_string()]),
        },
        evidence: BTreeMap::from([
            (
                "quote_size".to_string(),
                LeverEvidence {
                    identifiability: tenths(9),
                    experimentable: true,
                },
            ),
            (
                "tick_rest".to_string(),
                LeverEvidence {
                    identifiability: tenths(9),
                    experimentable: true,
                },
            ),
        ]),
        observed_facts: BTreeSet::new(),
        proposals: Vec::new(),
        guards: None,
        pass: None,
        structural: Some(StructuralDecl {
            models: vec![StructuralModelDecl {
                name: "desk".into(),
                links: vec![
                    link("quote_size", "spread", coefficient),
                    link("tick_rest", "spread", -0.1),
                ],
                variables: Vec::new(),
                intercepts: BTreeMap::from([("spread".to_string(), 6.0)]),
                noise_variance: BTreeMap::new(),
            }],
            fitted: Vec::new(),
            scenarios: vec![
                Scenario {
                    name: "calm".into(),
                    exogenous: BTreeMap::new(),
                },
                Scenario {
                    name: "stressed".into(),
                    exogenous: BTreeMap::from([("spread".to_string(), 3.0)]),
                },
            ],
            objective: Objective {
                variable: "spread".into(),
                direction: Direction::Decrease,
            },
            actuation: BTreeMap::from([
                ("quoter".to_string(), 10.0),
                ("rester".to_string(), 5.0),
                ("layerer".to_string(), 50.0),
            ]),
            generate: Some(GenerateDecl {
                exposure: BTreeMap::from([
                    ("quoter".to_string(), d(exposure)),
                    ("rester".to_string(), d(exposure)),
                    ("layerer".to_string(), d(exposure)),
                ]),
                max_steps: 2,
            }),
        }),
        touches: Vec::new(),
    }
}

struct Never;
impl Adapter for Never {
    fn call(&mut self, _: &Step) -> Result<(), Error> {
        Err(Error::guard("a shadow pass called an adapter"))
    }
}

/// Given a goal and graph with no proposals, the planner produces at least
/// two admissible candidates, simulates each, and never generates the
/// deceptive edge. Mutation: make `Structural::generate` return an empty
/// list (or only the first single) — the count assertions fail and the pass
/// stops at Predict.
#[test]
fn with_no_proposals_the_planner_generates_and_simulates_admissible_candidates() {
    let report = run(generating_request(-0.2, 50, 5), None, &mut Never).unwrap();
    // quoter, rester, and the pair: three, never the layerer.
    assert_eq!(report.candidates.len(), 3, "{:?}", report.candidates);
    assert!(report.candidates.iter().all(|c| !c.contains("layerer")));
    let admissible = report.candidates.len() - report.refused.len();
    assert!(admissible >= 2, "refused {:?}", report.refused);
    // Two scenarios and one model: two results per candidate.
    assert_eq!(report.simulations.len(), 3);
    assert!(report.simulations.iter().all(|s| s.results.len() == 2));
    let Outcome::Shadowed(steps) = &report.outcome else {
        panic!("{:?}", report.outcome)
    };
    // The pair moves the spread most: 10 * 0.2 + 5 * 0.1.
    assert_eq!(steps.len(), 2, "{steps:?}");
}

/// The failure prevented in 6083: a learned effect large enough to buy its
/// way past a hard constraint. Here the model says the quoter narrows the
/// spread by a billion; the plan's exposure exceeds the envelope, so nothing
/// is chosen. Mutation: in `plan::select`, read the score before
/// `InterventionPlan::plan` and admit when it is large — the outcome then is
/// a plan.
#[test]
fn a_learned_effect_however_large_never_buys_a_plan_past_a_hard_constraint() {
    // Premise: inside the envelope the same candidates are chosen.
    let inside = run(generating_request(-1e8, 50, 5), None, &mut Never).unwrap();
    assert!(matches!(inside.outcome, Outcome::Shadowed(_)), "premise");
    let outside = run(generating_request(-1e8, 50, 60), None, &mut Never).unwrap();
    assert_eq!(outside.outcome, Outcome::NoAction);
    assert_eq!(outside.refused.len(), outside.candidates.len());
    assert!(
        outside
            .refused
            .iter()
            .all(|(_, e)| e.to_string().contains("risk envelope")),
        "{:?}",
        outside.refused
    );
}

/// An operator's claim is held to the simulation. Mutation: delete the
/// `check_claim` call in the engine — the inflated proposal is then chosen.
#[test]
fn a_proposal_claiming_more_than_any_model_simulates_is_refused() {
    let mut request = generating_request(-0.2, 50, 5);
    request.structural.as_mut().unwrap().generate = None;
    let claim = |effect: i64| ProposalDraft {
        root: PlanNode::Step(Step {
            tool: "quoter".into(),
            variable: "quote_size".into(),
            exposure: d(5),
        }),
        comparison: ComparisonDraft {
            expected_causal_effect: Some(d(effect)),
            confidence: Some(tenths(5)),
            cost: Some(d(10)),
            capital_usage: Some(d(5)),
            time_to_effect_secs: Some(2),
            reversibility: Some(d(1)),
            legally_eligible: Some(true),
            conduct_risk: Some(d(0)),
            downside: Some(d(0)),
        },
    };
    // The model says 2; a claim of 1 is supported.
    request.proposals = vec![claim(1)];
    let honest = run(request.clone(), None, &mut Never).unwrap();
    assert!(matches!(honest.outcome, Outcome::Shadowed(_)), "premise");
    request.proposals = vec![claim(40)];
    let inflated = run(request, None, &mut Never).unwrap();
    assert_eq!(inflated.outcome, Outcome::NoAction);
    assert!(
        inflated.refused[0]
            .1
            .to_string()
            .contains("more than any model"),
        "{:?}",
        inflated.refused
    );
}

// ---- §24.3: all nine axes are weighed --------------------------------------

fn axes(effect: i64) -> ComparisonDraft {
    ComparisonDraft {
        expected_causal_effect: Some(d(effect)),
        confidence: Some(tenths(5)),
        cost: Some(d(1)),
        capital_usage: Some(d(1)),
        time_to_effect_secs: Some(60),
        reversibility: Some(d(1)),
        legally_eligible: Some(true),
        conduct_risk: Some(d(0)),
        downside: Some(d(2)),
    }
}

fn graph() -> AffordanceGraph {
    let mut g = AffordanceGraph::new();
    g.add_variable("quote_size", true, true).unwrap();
    g.add_variable("spread", true, false).unwrap();
    g.add_cause("quote_size", "spread").unwrap();
    g.add_tool_edge("quote_size", edge("quoter", Method::Market))
        .unwrap();
    g
}

/// Two plans, distinguishable by exposure, the first "worse" on one axis
/// only. Returns the exposure of the chosen one, or 0 for no action.
fn choose(first: ComparisonDraft, second: ComparisonDraft) -> i64 {
    let goal = goal(50).build().unwrap();
    let g = graph();
    let who = ActingIdentity {
        name: "desk-agent".into(),
        authorities: BTreeSet::from(["quote".to_string()]),
    };
    let levers = g.levers(&["spread"]);
    let plan = |exposure: i64, comparison: ComparisonDraft| Proposal {
        root: PlanNode::Step(Step {
            tool: "quoter".into(),
            variable: "quote_size".into(),
            exposure: d(exposure),
        }),
        comparison: comparison.build().unwrap(),
    };
    match select(
        &goal,
        &g,
        &who,
        &levers,
        vec![plan(1, first), plan(2, second)],
    )
    .unwrap()
    .chosen
    {
        Candidate::Plan(p) => {
            if p.total_exposure == d(1) {
                1
            } else {
                2
            }
        }
        Candidate::NoAction => 0,
    }
}

/// One row per axis: the first plan has the larger effect or is otherwise
/// equal, and loses on that axis alone. Mutation: delete any one term from
/// `Comparison::score`, `Comparison::rank` or `Comparison::refusal` — that
/// axis's row then picks plan 1.
#[test]
fn every_one_of_the_nine_axes_changes_which_candidate_is_chosen() {
    // Premise: with every axis equal, the larger effect wins.
    assert_eq!(choose(axes(6), axes(5)), 1, "premise: effect ranks");
    assert_eq!(choose(axes(5), axes(6)), 2, "premise: effect ranks");

    type Worse = fn(&mut ComparisonDraft);
    let rows: Vec<(&str, Worse)> = vec![
        ("confidence", |c| c.confidence = Some(tenths(1))),
        ("conduct_risk", |c| c.conduct_risk = Some(tenths(9))),
        ("legally_eligible", |c| c.legally_eligible = Some(false)),
        ("downside beyond the envelope", |c| c.downside = Some(d(51))),
        ("time beyond the horizon", |c| {
            c.time_to_effect_secs = Some(3601)
        }),
    ];
    for (axis, worse) in rows {
        let mut first = axes(6);
        worse(&mut first);
        assert_eq!(choose(first, axes(5)), 2, "{axis}");
    }
    // Tie-breaks: equal score, the first worse on one axis.
    let ties: Vec<(&str, Worse)> = vec![
        ("downside", |c| c.downside = Some(d(3))),
        ("reversibility", |c| c.reversibility = Some(d(0))),
        ("cost", |c| c.cost = Some(d(2))),
        ("capital_usage", |c| c.capital_usage = Some(d(2))),
        ("time_to_effect", |c| c.time_to_effect_secs = Some(61)),
    ];
    for (axis, worse) in ties {
        let mut first = axes(5);
        worse(&mut first);
        assert_eq!(choose(first, axes(5)), 2, "{axis}");
        // And the mirror: the same change on the second picks the first.
        let mut second = axes(5);
        worse(&mut second);
        assert_eq!(choose(axes(5), second), 1, "{axis} mirrored");
    }
}

/// Mutation: drop the range checks in `ComparisonDraft::build`.
#[test]
fn an_axis_outside_the_range_it_is_weighed_in_is_refused() {
    assert!(axes(1).build().is_ok(), "premise");
    let mut c = axes(1);
    c.confidence = Some(tenths(15));
    assert!(c.build().is_err());
    let mut c = axes(1);
    c.conduct_risk = Some(d(-1));
    assert!(c.build().is_err());
    let mut c = axes(1);
    c.downside = Some(d(-2));
    assert!(c.build().is_err());
}

// ---- R12-AGENCY-6102: estimators carry their identification ----------------

/// Each method computes its effect and its label together; only the
/// correlation is unidentified. Mutation: label `correlation` as
/// `Observational` — the last assertion block fails, and so does
/// `supports` refusing it as experiment evidence.
#[test]
fn each_estimator_produces_an_effect_with_the_identification_its_method_earns() {
    let rct = estimate::randomised_experiment(&[3.0, 4.0, 5.0], &[0.0, 1.0, 2.0]).unwrap();
    assert_eq!(rct.kind(), EstimatorKind::RandomisedExperiment);
    assert_eq!(rct.identification(), &Identification::Experiment);
    assert!((rct.effect() - 3.0).abs() < 1e-12);
    assert!(rct.standard_error().unwrap() > 0.0);

    let did = estimate::difference_in_differences(
        &[10.0, 11.0],
        &[15.0, 16.0],
        &[10.0, 11.0],
        &[12.0, 13.0],
    )
    .unwrap();
    assert!((did.effect() - 3.0).abs() < 1e-12);
    assert!(matches!(
        did.identification(),
        Identification::Observational { strategy } if strategy.contains("parallel trends")
    ));

    // Donor tracks the treated unit exactly before; after, the treated unit
    // is 2 above it.
    let synth = estimate::synthetic_control(
        &[1.0, 2.0, 3.0, 4.0],
        &[7.0, 8.0],
        &[vec![1.0, 2.0, 3.0, 4.0]],
        &[vec![5.0, 6.0]],
    )
    .unwrap();
    assert!((synth.effect() - 2.0).abs() < 1e-9, "{}", synth.effect());

    let z = [0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
    let x = [0.0, 1.0, 0.1, 0.9, 0.0, 1.0];
    let y: Vec<f64> = x.iter().map(|v| 2.0 * v).collect();
    let iv = estimate::instrumental_variable(&z, &x, &y).unwrap();
    assert!((iv.effect() - 2.0).abs() < 1e-9);
    assert!(estimate::instrumental_variable(&[1.0, 1.0, 1.0], &x[..3], &y[..3]).is_err());

    let m = model("m", vec![link("x", "y", 0.5)], &[]);
    let scm = estimate::structural(&m, "x", "y").unwrap();
    assert!((scm.effect() - 0.5).abs() < 1e-12);
    assert!(scm.identification().is_identified());

    let corr = estimate::correlation(&x, &y).unwrap();
    assert_eq!(corr.identification(), &Identification::NotIdentified);
    assert!(!corr.identification().is_identified());
}

fn attribution(identification: Identification, mean: f64) -> EffectAttribution {
    EffectAttribution {
        goal_id: "g1".into(),
        intervention_id: "i1".into(),
        action_id: "a1".into(),
        action_class: Method::Market,
        identification,
        observations: vec![HorizonObservation {
            horizon_ms: 60_000,
            value: mean,
        }],
        counterfactual_method: "randomised control".into(),
        effect: Effect { mean, std_dev: 0.5 },
        causal_chain: vec!["quote_size".into(), "spread".into()],
        confounders: vec!["none considered".into()],
        competing_explanations: vec!["seasonality".into()],
        side_effects: vec!["none observed".into()],
        identifiability: 0.8,
        resulting_updates: vec!["raise quote-size prior".into()],
    }
}

/// An attribution is evidence only when an estimator reproduces it.
/// Mutation: make `Estimate::supports` return `Ok(())` unconditionally —
/// both refusals below pass through.
#[test]
fn an_attribution_is_supported_only_by_an_estimate_with_its_label_and_its_effect() {
    let rct = estimate::randomised_experiment(&[3.0, 4.0, 5.0], &[0.0, 1.0, 2.0]).unwrap();
    assert!(
        rct.supports(&attribution(Identification::Experiment, 3.0))
            .is_ok(),
        "premise"
    );
    let wrong_effect = rct
        .supports(&attribution(Identification::Experiment, 4.0))
        .unwrap_err();
    assert!(
        wrong_effect.to_string().contains("computes"),
        "{wrong_effect}"
    );
    let corr = estimate::correlation(&[0.0, 1.0, 2.0], &[0.0, 3.0, 6.0]).unwrap();
    let relabelled = corr
        .supports(&attribution(Identification::Experiment, corr.effect()))
        .unwrap_err();
    assert!(
        relabelled.to_string().contains("earned by the method"),
        "{relabelled}"
    );
}
