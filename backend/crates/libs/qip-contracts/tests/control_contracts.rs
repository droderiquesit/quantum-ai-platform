//! The seven blueprint v12.0 §24.9 superintelligence control contracts
//! (R12-CONTRACT-6109..6115) and the §9.14 `Forecast` (R12-CONTRACT-3117):
//! for each, a byte-identical round trip, a refusal of every missing field on
//! decode, and a refusal — on construction and on decode — of every
//! invariant its `check` names.
//!
//! Mutation for every `*_refuses_*` test here: delete any one guard in the
//! contract's `check` — the matching labelled case is then admitted and the
//! test fails naming the label.

#![allow(clippy::panic_in_result_fn, clippy::expect_used)]

mod common;

use common::{each_refused, refuses_missing_unknown_and_foreign_version, round_trips};
use qip_contracts::capital::{CapitalGrant, CapitalGrantFields, CollateralClass, GrantMode};
use qip_contracts::control::{
    ComputeBackend, ComputeCandidate, ComputePlan, ComputePlanFields, Exposure, HedgeInstrument,
    HedgeLiquidity, HedgePlan, HedgePlanFields, HedgeRollback, HedgeSide, InformationRequest,
    InformationRequestFields, ModelVote, ModelVoteFields, ScorePoint,
};
use qip_contracts::forecast::{
    Calibration, Forecast, ForecastFields, ForecastState, ForecastStateFields, OutcomeDistribution,
    OutcomeMass, QuantilePoint, TailMass, TailSide, UncertaintyDecomposition,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_contracts::world_model::{BranchParent, CausalEdge, WorldBranch, WorldBranchFields};
use qip_core::error::Result;
use qip_core::{Duration, Timestamp, dec};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn s(v: &str) -> String {
    v.to_string()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn quantiles() -> OutcomeDistribution {
    OutcomeDistribution::Quantiles {
        points: vec![
            QuantilePoint {
                quantile_bp: 500,
                value: dec!("98.0"),
            },
            QuantilePoint {
                quantile_bp: 5_000,
                value: dec!("100.5"),
            },
            QuantilePoint {
                quantile_bp: 9_500,
                value: dec!("103.0"),
            },
        ],
    }
}

fn uncertainty() -> UncertaintyDecomposition {
    UncertaintyDecomposition {
        epistemic: dec!("0.8"),
        aleatoric: dec!("1.6"),
    }
}

// ---- Forecast (3117) ---------------------------------------------------

fn forecast() -> ForecastFields {
    ForecastFields {
        schema_version: 1,
        forecast_id: s("fc-1"),
        model: s("mid-price-gbm@5"),
        target: s("XNYS:ACME/mid_price"),
        issued_at: t(0),
        horizon: Duration::from_hours(1),
        distribution: quantiles(),
        uncertainty: uncertainty(),
        tail: TailMass {
            side: TailSide::Below,
            threshold: dec!("97.0"),
            probability_bp: 200,
        },
        models_consulted: 3,
        model_disagreement: dec!("0.4"),
        regime_confidence_bp: 7_500,
        expected_impact: dec!("-1250.00"),
        calibration: Calibration::Measured {
            method: s("crps"),
            score: dec!("0.31"),
            resolved: 1_200,
            evaluated_at: t(-3_600),
        },
        evidence_lineage: strings(&["feature:mid-price@1", "ev-1"]),
        expires_at: t(3_600),
    }
}

#[test]
fn a_forecast_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(forecast())?;
    refuses_missing_unknown_and_foreign_version(forecast());
    round_trips(ForecastFields {
        distribution: OutcomeDistribution::Categorical {
            outcomes: vec![
                OutcomeMass {
                    outcome: s("up"),
                    probability_bp: 5_500,
                },
                OutcomeMass {
                    outcome: s("flat"),
                    probability_bp: 1_000,
                },
                OutcomeMass {
                    outcome: s("down"),
                    probability_bp: 3_500,
                },
            ],
        },
        models_consulted: 1,
        model_disagreement: dec!("0"),
        calibration: Calibration::Uncalibrated,
        ..forecast()
    })?;
    Ok(())
}

/// §9.14's decomposition is carried and summed exactly, and a forecast
/// projects to the compact §24.9 `ForecastState` with its target, horizon,
/// distribution, uncertainty and expiry intact.
///
/// Mutation: make `Forecast::state` pass `Duration::ZERO` as the horizon —
/// the projection is then refused and the test fails.
#[test]
fn a_forecast_decomposes_its_uncertainty_and_projects_to_a_forecast_state() -> Result<()> {
    let f = Forecast::new(forecast())?;
    assert_eq!(f.uncertainty.total()?, dec!("2.4"));
    let state: ForecastState = f.state()?;
    assert_eq!(state.target, f.target);
    assert_eq!(state.horizon, f.horizon);
    assert_eq!(state.distribution, f.distribution);
    assert_eq!(state.uncertainty, f.uncertainty);
    assert_eq!(state.expires_at, f.expires_at);
    assert_eq!(state.confidence_bp, 7_500);
    Ok(())
}

/// Mutation: change `>=` to `>` in `Forecast::read_at` — a read at the
/// expiry instant is then admitted and the second assertion fails.
#[test]
fn a_forecast_read_at_or_after_its_expiry_is_refused() -> Result<()> {
    let f = Forecast::new(forecast())?;
    assert!(
        f.read_at(t(3_599)).is_ok(),
        "premise: a live forecast reads"
    );
    assert_eq!(
        f.read_at(t(3_600)).map(|_| ()).unwrap_err().code(),
        "denied"
    );
    assert!(f.read_at(t(7_200)).is_err());
    Ok(())
}

#[test]
fn a_forecast_refuses_each_broken_invariant() {
    each_refused(
        &forecast(),
        vec![
            ("blank id", Box::new(|f| f.forecast_id = s(""))),
            ("blank model", Box::new(|f| f.model = s(""))),
            ("blank target", Box::new(|f| f.target = s(""))),
            ("zero horizon", Box::new(|f| f.horizon = Duration::ZERO)),
            (
                "one quantile",
                Box::new(|f| {
                    f.distribution = OutcomeDistribution::Quantiles {
                        points: vec![QuantilePoint {
                            quantile_bp: 5_000,
                            value: dec!("100"),
                        }],
                    }
                }),
            ),
            (
                "repeated quantile",
                Box::new(|f| {
                    f.distribution = OutcomeDistribution::Quantiles {
                        points: vec![
                            QuantilePoint {
                                quantile_bp: 5_000,
                                value: dec!("100"),
                            },
                            QuantilePoint {
                                quantile_bp: 5_000,
                                value: dec!("101"),
                            },
                        ],
                    }
                }),
            ),
            (
                "quantile at zero",
                Box::new(|f| {
                    f.distribution = OutcomeDistribution::Quantiles {
                        points: vec![
                            QuantilePoint {
                                quantile_bp: 0,
                                value: dec!("90"),
                            },
                            QuantilePoint {
                                quantile_bp: 5_000,
                                value: dec!("100"),
                            },
                        ],
                    }
                }),
            ),
            (
                "categorical certainty",
                Box::new(|f| {
                    f.distribution = OutcomeDistribution::Categorical {
                        outcomes: vec![OutcomeMass {
                            outcome: s("up"),
                            probability_bp: 10_000,
                        }],
                    }
                }),
            ),
            (
                "outcome twice",
                Box::new(|f| {
                    f.distribution = OutcomeDistribution::Categorical {
                        outcomes: vec![
                            OutcomeMass {
                                outcome: s("up"),
                                probability_bp: 5_000,
                            },
                            OutcomeMass {
                                outcome: s("up"),
                                probability_bp: 5_000,
                            },
                        ],
                    }
                }),
            ),
            (
                "mass over one",
                Box::new(|f| {
                    f.distribution = OutcomeDistribution::Categorical {
                        outcomes: vec![
                            OutcomeMass {
                                outcome: s("up"),
                                probability_bp: 6_000,
                            },
                            OutcomeMass {
                                outcome: s("down"),
                                probability_bp: 4_001,
                            },
                        ],
                    }
                }),
            ),
            (
                "negative epistemic",
                Box::new(|f| f.uncertainty.epistemic = dec!("-0.1")),
            ),
            (
                "negative aleatoric",
                Box::new(|f| f.uncertainty.aleatoric = dec!("-0.1")),
            ),
            (
                "tail above one",
                Box::new(|f| f.tail.probability_bp = 10_001),
            ),
            ("no model", Box::new(|f| f.models_consulted = 0)),
            (
                "negative disagreement",
                Box::new(|f| f.model_disagreement = dec!("-0.1")),
            ),
            (
                "one model disagreeing with itself",
                Box::new(|f| f.models_consulted = 1),
            ),
            (
                "regime confidence above one",
                Box::new(|f| f.regime_confidence_bp = 10_001),
            ),
            (
                "calibration with no method",
                Box::new(|f| {
                    f.calibration = Calibration::Measured {
                        method: s(""),
                        score: dec!("0.3"),
                        resolved: 10,
                        evaluated_at: t(0),
                    }
                }),
            ),
            (
                "negative calibration score",
                Box::new(|f| {
                    f.calibration = Calibration::Measured {
                        method: s("brier"),
                        score: dec!("-0.3"),
                        resolved: 10,
                        evaluated_at: t(0),
                    }
                }),
            ),
            ("no lineage", Box::new(|f| f.evidence_lineage.clear())),
            ("expiry at issue", Box::new(|f| f.expires_at = f.issued_at)),
        ],
    );
}

// ---- ForecastState (6109) ----------------------------------------------

fn forecast_state() -> ForecastStateFields {
    ForecastStateFields {
        schema_version: 1,
        target: s("XNYS:ACME/mid_price"),
        horizon: Duration::from_hours(1),
        distribution: quantiles(),
        confidence_bp: 7_000,
        uncertainty: uncertainty(),
        expires_at: t(3_600),
    }
}

#[test]
fn a_forecast_state_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(forecast_state())?;
    refuses_missing_unknown_and_foreign_version(forecast_state());
    Ok(())
}

#[test]
fn a_forecast_state_refuses_each_broken_invariant() {
    each_refused(
        &forecast_state(),
        vec![
            ("blank target", Box::new(|f| f.target = s(" "))),
            ("zero horizon", Box::new(|f| f.horizon = Duration::ZERO)),
            (
                "negative horizon",
                Box::new(|f| f.horizon = Duration::from_secs(-1)),
            ),
            (
                "confidence above one",
                Box::new(|f| f.confidence_bp = 10_001),
            ),
            (
                "negative epistemic",
                Box::new(|f| f.uncertainty.epistemic = dec!("-1")),
            ),
            (
                "bad distribution",
                Box::new(|f| f.distribution = OutcomeDistribution::Quantiles { points: vec![] }),
            ),
        ],
    );
}

// ---- WorldBranch (6110) ------------------------------------------------

fn branch() -> WorldBranchFields {
    WorldBranchFields {
        schema_version: 1,
        branch_id: s("br-hawkish"),
        parent: BranchParent::Branch { id: s("br-base") },
        assumptions: strings(&["core CPI above 3%"]),
        probability_bp: 3_000,
        causal_deltas: vec![CausalEdge {
            cause: s("cpi"),
            effect: s("fed_funds_move"),
            strength_bp: 4_000,
        }],
        unresolved_evidence: strings(&["ev-cpi-oct"]),
    }
}

#[test]
fn a_world_branch_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(branch())?;
    refuses_missing_unknown_and_foreign_version(branch());
    round_trips(WorldBranchFields {
        parent: BranchParent::Root,
        unresolved_evidence: vec![],
        ..branch()
    })?;
    let b: WorldBranch = WorldBranch::new(branch())?;
    assert_eq!(b.probability_bp, 3_000);
    Ok(())
}

#[test]
fn a_world_branch_refuses_each_broken_invariant() {
    each_refused(
        &branch(),
        vec![
            ("blank id", Box::new(|f| f.branch_id = s(""))),
            (
                "own parent",
                Box::new(|f| {
                    f.parent = BranchParent::Branch {
                        id: s("br-hawkish"),
                    }
                }),
            ),
            (
                "blank parent",
                Box::new(|f| f.parent = BranchParent::Branch { id: s("") }),
            ),
            ("no assumption", Box::new(|f| f.assumptions.clear())),
            (
                "probability above one",
                Box::new(|f| f.probability_bp = 10_001),
            ),
            ("no causal delta", Box::new(|f| f.causal_deltas.clear())),
            (
                "self-loop delta",
                Box::new(|f| f.causal_deltas[0].effect = s("cpi")),
            ),
            (
                "repeated unresolved evidence",
                Box::new(|f| f.unresolved_evidence = strings(&["ev", "ev"])),
            ),
        ],
    );
}

// ---- InformationRequest (6111) -----------------------------------------

fn info_request() -> InformationRequestFields {
    InformationRequestFields {
        schema_version: 1,
        request_id: s("ir-1"),
        question: s("does the October CPI print confirm the hawkish branch"),
        expected_information_value: dec!("1800"),
        issued_at: t(0),
        deadline: t(7_200),
        allowed_sources: strings(&["src-bls", "src-filings"]),
        budget: dec!("25"),
    }
}

#[test]
fn an_information_request_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()>
{
    round_trips(info_request())?;
    refuses_missing_unknown_and_foreign_version(info_request());
    let r: InformationRequest = InformationRequest::new(info_request())?;
    assert_eq!(r.allowed_sources.len(), 2);
    Ok(())
}

#[test]
fn an_information_request_refuses_each_broken_invariant() {
    each_refused(
        &info_request(),
        vec![
            ("blank id", Box::new(|f| f.request_id = s(""))),
            ("blank question", Box::new(|f| f.question = s(""))),
            (
                "zero information value",
                Box::new(|f| f.expected_information_value = dec!("0")),
            ),
            ("deadline at issue", Box::new(|f| f.deadline = f.issued_at)),
            ("no allowed source", Box::new(|f| f.allowed_sources.clear())),
            ("zero budget", Box::new(|f| f.budget = dec!("0"))),
            ("negative budget", Box::new(|f| f.budget = dec!("-5"))),
        ],
    );
}

// ---- ModelVote (6112) --------------------------------------------------

fn vote() -> ModelVoteFields {
    ModelVoteFields {
        schema_version: 1,
        model: s("macro-rates"),
        version: s("4.1.0"),
        proposition: s("the Fed cuts in December"),
        probability_bp: 6_200,
        regime: s("disinflation"),
        score_history: vec![
            ScorePoint {
                at: t(-7_200),
                score: dec!("0.21"),
            },
            ScorePoint {
                at: t(-3_600),
                score: dec!("0.19"),
            },
        ],
        compute_cost: dec!("0.04"),
        cast_at: t(0),
    }
}

#[test]
fn a_model_vote_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(vote())?;
    refuses_missing_unknown_and_foreign_version(vote());
    let v: ModelVote = ModelVote::new(vote())?;
    assert_eq!(v.score_history.len(), 2);
    Ok(())
}

#[test]
fn a_model_vote_refuses_each_broken_invariant() {
    each_refused(
        &vote(),
        vec![
            ("blank model", Box::new(|f| f.model = s(""))),
            ("blank version", Box::new(|f| f.version = s(""))),
            ("blank proposition", Box::new(|f| f.proposition = s(""))),
            (
                "probability above one",
                Box::new(|f| f.probability_bp = 10_001),
            ),
            ("blank regime", Box::new(|f| f.regime = s(""))),
            ("no history", Box::new(|f| f.score_history.clear())),
            (
                "history out of order",
                Box::new(|f| f.score_history.reverse()),
            ),
            (
                "negative score",
                Box::new(|f| f.score_history[0].score = dec!("-0.1")),
            ),
            (
                "score from after the vote",
                Box::new(|f| f.score_history[1].at = t(60)),
            ),
            (
                "negative cost",
                Box::new(|f| f.compute_cost = dec!("-0.01")),
            ),
        ],
    );
}

// ---- ComputePlan (6113) ------------------------------------------------

fn candidate(id: &str, backend: ComputeBackend, cost: &str) -> ComputeCandidate {
    ComputeCandidate {
        id: s(id),
        backend,
        solver: s("solver"),
        estimated_cost: qip_core::Decimal::parse(cost).expect("fixture decimal"),
        estimated_latency: Duration::from_millis(200),
    }
}

fn compute_plan() -> ComputePlanFields {
    ComputePlanFields {
        schema_version: 1,
        plan_id: s("cp-1"),
        task: s("qubo:rebalance-42"),
        classical_baseline: s("cpu-sd"),
        candidates: vec![
            candidate("cpu-sd", ComputeBackend::Cpu, "0.01"),
            candidate("gpu-anneal", ComputeBackend::Gpu, "0.20"),
            candidate("tpu-net", ComputeBackend::Tpu, "0.30"),
            candidate("qpu-qaoa", ComputeBackend::Qpu, "4.00"),
        ],
        budget: dec!("5"),
        verifier: s("objective-reevaluated-classically"),
    }
}

#[test]
fn a_compute_plan_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(compute_plan())?;
    refuses_missing_unknown_and_foreign_version(compute_plan());
    let p: ComputePlan = ComputePlan::new(compute_plan())?;
    assert_eq!(p.candidates.len(), 4);
    Ok(())
}

/// ADR 0006 as a decode refusal: a plan without a classical baseline it can
/// afford never reaches a router.
#[test]
fn a_compute_plan_refuses_each_broken_invariant_including_a_missing_or_quantum_baseline() {
    each_refused(
        &compute_plan(),
        vec![
            ("blank id", Box::new(|f| f.plan_id = s(""))),
            ("blank task", Box::new(|f| f.task = s(""))),
            ("blank verifier", Box::new(|f| f.verifier = s(""))),
            ("zero budget", Box::new(|f| f.budget = dec!("0"))),
            ("no candidate", Box::new(|f| f.candidates.clear())),
            (
                "candidate twice",
                Box::new(|f| f.candidates[1].id = s("cpu-sd")),
            ),
            (
                "negative cost",
                Box::new(|f| f.candidates[1].estimated_cost = dec!("-1")),
            ),
            (
                "negative latency",
                Box::new(|f| f.candidates[1].estimated_latency = Duration::from_millis(-1)),
            ),
            (
                "baseline not a candidate",
                Box::new(|f| f.classical_baseline = s("missing")),
            ),
            (
                "quantum baseline",
                Box::new(|f| f.classical_baseline = s("qpu-qaoa")),
            ),
            (
                "unaffordable baseline",
                Box::new(|f| f.candidates[0].estimated_cost = dec!("6")),
            ),
        ],
    );
}

// ---- HedgePlan (6114) --------------------------------------------------

fn hedge() -> HedgePlanFields {
    HedgePlanFields {
        schema_version: 1,
        plan_id: s("hp-1"),
        exposures: vec![Exposure {
            risk_factor: s("equity-beta"),
            amount: dec!("1200000"),
        }],
        scenarios_protected: strings(&["sc-equity-down-10"]),
        instruments: vec![HedgeInstrument {
            instrument: s("ES-DEC"),
            venue: VenueId::new("SIM-CME"),
            side: HedgeSide::Sell,
            quantity: dec!("4"),
        }],
        cost: dec!("950"),
        liquidity: HedgeLiquidity {
            max_adv_share_bp: 5,
            time_to_exit: Duration::from_mins(5),
        },
        rollback: HedgeRollback {
            trigger: s("equity beta below 0.2"),
            steps: strings(&["buy back ES-DEC at market"]),
        },
    }
}

#[test]
fn a_hedge_plan_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(hedge())?;
    refuses_missing_unknown_and_foreign_version(hedge());
    let h: HedgePlan = HedgePlan::new(hedge())?;
    assert_eq!(h.instruments.len(), 1);
    Ok(())
}

#[test]
fn a_hedge_plan_refuses_each_broken_invariant() {
    each_refused(
        &hedge(),
        vec![
            ("blank id", Box::new(|f| f.plan_id = s(""))),
            ("no exposure", Box::new(|f| f.exposures.clear())),
            (
                "zero exposure",
                Box::new(|f| f.exposures[0].amount = dec!("0")),
            ),
            (
                "blank risk factor",
                Box::new(|f| f.exposures[0].risk_factor = s("")),
            ),
            ("no scenario", Box::new(|f| f.scenarios_protected.clear())),
            ("no instrument", Box::new(|f| f.instruments.clear())),
            (
                "zero quantity",
                Box::new(|f| f.instruments[0].quantity = dec!("0")),
            ),
            (
                "blank venue",
                Box::new(|f| f.instruments[0].venue = VenueId::new("")),
            ),
            ("negative cost", Box::new(|f| f.cost = dec!("-1"))),
            (
                "ADV share above one",
                Box::new(|f| f.liquidity.max_adv_share_bp = 10_001),
            ),
            (
                "zero exit time",
                Box::new(|f| f.liquidity.time_to_exit = Duration::ZERO),
            ),
            (
                "no rollback trigger",
                Box::new(|f| f.rollback.trigger = s("")),
            ),
            ("no rollback step", Box::new(|f| f.rollback.steps.clear())),
        ],
    );
}

// ---- CapitalGrant (6115, 7066) -----------------------------------------

fn grant() -> CapitalGrantFields {
    CapitalGrantFields {
        schema_version: 1,
        grant_id: s("cg-1"),
        mode: GrantMode::Paper,
        owner: s("research-desk"),
        mandate: s("mandate-paper-2026"),
        strategy: StrategyId::new("mean-revert-acme"),
        region: s("us-east4"),
        venue: VenueId::new("SIM-XNYS"),
        amount: dec!("100000"),
        collateral: CollateralClass::Cash,
        max_leverage: dec!("2"),
        risk_budget: dec!("5000"),
        max_drawdown_bp: 800,
        liquidity_reserve: dec!("10000"),
        max_adv_share_bp: 100,
        granted_at: t(0),
        expires_at: t(86_400),
    }
}

#[test]
fn a_capital_grant_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(grant())?;
    refuses_missing_unknown_and_foreign_version(grant());
    let g: CapitalGrant = CapitalGrant::new(grant())?;
    assert_eq!(g.deployable(), dec!("90000"));
    assert!(g.is_live(t(0)) && !g.is_live(t(86_400)) && !g.is_live(t(-1)));
    Ok(())
}

/// The paper boundary, held by the contract's type: `mode` has one variant,
/// so a grant naming any other mode does not decode at all.
///
/// Mutation: add a `Live` variant to `GrantMode` — the `"live"` record then
/// decodes and this test fails.
#[test]
fn a_capital_grant_naming_any_mode_but_paper_does_not_decode() -> Result<()> {
    let mut value = serde_json::to_value(grant()).expect("serialises");
    assert!(
        serde_json::from_value::<CapitalGrant>(value.clone()).is_ok(),
        "premise: the paper grant decodes"
    );
    for mode in ["live", "supervised_live", "autonomous_live", ""] {
        value["mode"] = serde_json::Value::from(mode);
        assert!(
            serde_json::from_value::<CapitalGrant>(value.clone()).is_err(),
            "a grant in mode `{mode}` decoded"
        );
    }
    Ok(())
}

#[test]
fn a_capital_grant_refuses_each_broken_invariant() {
    each_refused(
        &grant(),
        vec![
            ("blank id", Box::new(|f| f.grant_id = s(""))),
            ("blank owner", Box::new(|f| f.owner = s(""))),
            ("blank mandate", Box::new(|f| f.mandate = s(""))),
            (
                "blank strategy",
                Box::new(|f| f.strategy = StrategyId::new("")),
            ),
            ("blank region", Box::new(|f| f.region = s(""))),
            ("blank venue", Box::new(|f| f.venue = VenueId::new(""))),
            ("zero amount", Box::new(|f| f.amount = dec!("0"))),
            (
                "leverage below one",
                Box::new(|f| f.max_leverage = dec!("0.5")),
            ),
            ("zero risk budget", Box::new(|f| f.risk_budget = dec!("0"))),
            (
                "risk budget above amount",
                Box::new(|f| f.risk_budget = dec!("100001")),
            ),
            ("zero drawdown", Box::new(|f| f.max_drawdown_bp = 0)),
            (
                "drawdown above one",
                Box::new(|f| f.max_drawdown_bp = 10_001),
            ),
            (
                "negative reserve",
                Box::new(|f| f.liquidity_reserve = dec!("-1")),
            ),
            (
                "reserve of the whole amount",
                Box::new(|f| f.liquidity_reserve = dec!("100000")),
            ),
            ("zero ADV share", Box::new(|f| f.max_adv_share_bp = 0)),
            (
                "ADV share above one",
                Box::new(|f| f.max_adv_share_bp = 10_001),
            ),
            ("expiry at grant", Box::new(|f| f.expires_at = f.granted_at)),
        ],
    );
}
