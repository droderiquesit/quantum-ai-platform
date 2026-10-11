//! The blueprint v12.0 §27 contracts between brains that were MISSING in the
//! register (R12-CONTRACT-7062, 7065, 7068, 7074, 7075, 7076, 7077, 7082,
//! 7088, 7091, 7092): for each, a byte-identical round trip, a refusal of
//! every missing field on decode, and a refusal — on construction and on
//! decode — of every invariant its `check` names.
//!
//! Mutation for every `*_refuses_*` test here: delete any one guard in the
//! contract's `check` — the matching labelled case is then admitted and the
//! test fails naming the label. For every `*_round_trips_*` test: add
//! `#[serde(skip_serializing)]` to any field — the missing-field half fails.

#![allow(clippy::panic_in_result_fn, clippy::expect_used)]

mod common;

use common::{
    DIGEST_A, DIGEST_B, each_refused, refuses_missing_unknown_and_foreign_version, round_trips,
};
use qip_contracts::action_intent::{
    ActingIdentity, ActionIntent, ActionIntentFields, Disclosure, PolicyDecision, Verdict,
};
use qip_contracts::control_pack::{
    ArtifactKind, ControlPackAnnouncement, ControlPackAnnouncementFields, RollbackParent,
};
use qip_contracts::evidence_packet::{
    EvidencePacket, EvidencePacketFields, LicenceTerms, Location, SourceRef,
};
use qip_contracts::forecast::{Calibration, OutcomeDistribution, OutcomeMass, QuantilePoint};
use qip_contracts::governance::Usage;
use qip_contracts::knowledge_delta::{
    ChangeOp, KnowledgeDelta, KnowledgeDeltaFields, KnowledgeTarget, MergePolicy,
};
use qip_contracts::microstructure_delta::{
    MicrostructureDelta, MicrostructureDeltaFields, Residual, ResidualKind,
};
use qip_contracts::mirror_checkpoint::{MirrorCheckpoint, MirrorCheckpointFields};
use qip_contracts::opportunity_token::{
    ExecutionPolicy, LegSide, OpportunityToken, OpportunityTokenFields, TokenLeg, TokenLineage,
    UnwindPolicy,
};
use qip_contracts::signal::{OpportunityId, StrategyId};
use qip_contracts::strategy_pack::{
    CompiledStrategy, PerformanceEnvelope, RetirementRule, StrategyPack, StrategyPackFields,
};
use qip_contracts::symbolic::{
    Premise, ProofResult, SymbolicDerivation, SymbolicDerivationFields, TraceStep,
};
use qip_contracts::venue::VenueId;
use qip_contracts::world_model::{
    ArbitrationStatus, BranchParent, CausalEdge, DisputingView, ModelDisagreement,
    ModelDisagreementFields, StateVariable, WorldModelState, WorldModelStateFields,
};
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

fn measured() -> Calibration {
    Calibration::Measured {
        method: s("brier"),
        score: dec!("0.18"),
        resolved: 240,
        evaluated_at: t(-60),
    }
}

fn categorical() -> OutcomeDistribution {
    OutcomeDistribution::Categorical {
        outcomes: vec![
            OutcomeMass {
                outcome: s("up"),
                probability_bp: 6_000,
            },
            OutcomeMass {
                outcome: s("down"),
                probability_bp: 4_000,
            },
        ],
    }
}

// ---- EvidencePacket (7062) ---------------------------------------------

fn evidence() -> EvidencePacketFields {
    EvidencePacketFields {
        schema_version: 1,
        packet_id: s("ev-1"),
        claim: s("ACME guided Q3 revenue below consensus"),
        entities: strings(&["ACME"]),
        valid_at: t(0),
        location: Location::Place { name: s("US") },
        source: SourceRef {
            source_id: s("src-filings"),
            locator: s("filings/acme/8-k/2026-10-01"),
            retrieved_at: t(5),
        },
        licence: LicenceTerms {
            licence_id: s("lic-filings-public"),
            permitted: vec![Usage::Research, Usage::Derive],
        },
        confidence_bp: 8_500,
        contradicts: strings(&["ev-0"]),
        content_hash: s(DIGEST_A),
    }
}

#[test]
fn an_evidence_packet_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(evidence())?;
    refuses_missing_unknown_and_foreign_version(evidence());
    // An empty contradiction list is a statement, not an omission.
    round_trips(EvidencePacketFields {
        contradicts: vec![],
        location: Location::Unlocated,
        ..evidence()
    })?;
    let packet: EvidencePacket = EvidencePacket::new(evidence())?;
    assert_eq!(packet.content_hash, DIGEST_A);
    Ok(())
}

#[test]
fn an_evidence_packet_refuses_each_broken_invariant() {
    each_refused(
        &evidence(),
        vec![
            ("blank id", Box::new(|f| f.packet_id = s(" "))),
            ("blank claim", Box::new(|f| f.claim = s(""))),
            ("no entity", Box::new(|f| f.entities.clear())),
            (
                "repeated entity",
                Box::new(|f| f.entities = strings(&["ACME", "ACME"])),
            ),
            (
                "blank place",
                Box::new(|f| f.location = Location::Place { name: s("") }),
            ),
            ("blank source", Box::new(|f| f.source.source_id = s(""))),
            ("blank locator", Box::new(|f| f.source.locator = s(""))),
            ("blank licence", Box::new(|f| f.licence.licence_id = s(""))),
            (
                "licence grants nothing",
                Box::new(|f| f.licence.permitted.clear()),
            ),
            (
                "confidence above one",
                Box::new(|f| f.confidence_bp = 10_001),
            ),
            (
                "blank contradiction",
                Box::new(|f| f.contradicts = strings(&[""])),
            ),
            (
                "contradicts itself",
                Box::new(|f| f.contradicts = strings(&["ev-1"])),
            ),
            ("short hash", Box::new(|f| f.content_hash = s("abc"))),
            (
                "uppercase hash",
                Box::new(|f| f.content_hash = DIGEST_A.to_uppercase()),
            ),
            ("foreign version", Box::new(|f| f.schema_version = 2)),
        ],
    );
}

// ---- StrategyPack (7065) -----------------------------------------------

fn strategy() -> CompiledStrategy {
    CompiledStrategy {
        strategy: StrategyId::new("mean-revert-acme"),
        plan_digest: s(DIGEST_A),
        hypotheses: strings(&["h-mean-reversion"]),
        universes: strings(&["XNYS:ACME"]),
        dependencies: strings(&["feature:mid-price@1"]),
        capacity: dec!("250000"),
        envelope: PerformanceEnvelope {
            min_hit_rate_bp: 5_100,
            max_drawdown_bp: 800,
            min_net_edge_bp: 2,
        },
        retirement_rules: vec![
            RetirementRule::EnvelopeBreached {
                consecutive_passes: 3,
            },
            RetirementRule::HypothesisRefuted {
                hypothesis: s("h-mean-reversion"),
            },
            RetirementRule::Expired,
        ],
    }
}

fn strategy_pack() -> StrategyPackFields {
    StrategyPackFields {
        schema_version: 1,
        pack_id: s("sp-7"),
        issued_at: t(0),
        expires_at: t(3_600),
        strategies: vec![strategy()],
    }
}

#[test]
fn a_strategy_pack_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(strategy_pack())?;
    refuses_missing_unknown_and_foreign_version(strategy_pack());
    let pack: StrategyPack = StrategyPack::new(strategy_pack())?;
    assert_eq!(pack.strategies.len(), 1);
    Ok(())
}

#[test]
fn a_strategy_pack_refuses_each_broken_invariant() {
    each_refused(
        &strategy_pack(),
        vec![
            ("blank id", Box::new(|f| f.pack_id = s(""))),
            ("expiry at issue", Box::new(|f| f.expires_at = f.issued_at)),
            ("no strategy", Box::new(|f| f.strategies.clear())),
            (
                "strategy twice",
                Box::new(|f| f.strategies = vec![strategy(), strategy()]),
            ),
            (
                "bad plan digest",
                Box::new(|f| f.strategies[0].plan_digest = s("x")),
            ),
            (
                "no hypothesis",
                Box::new(|f| f.strategies[0].hypotheses.clear()),
            ),
            (
                "no universe",
                Box::new(|f| f.strategies[0].universes.clear()),
            ),
            (
                "blank dependency",
                Box::new(|f| f.strategies[0].dependencies = strings(&[""])),
            ),
            (
                "zero capacity",
                Box::new(|f| f.strategies[0].capacity = dec!("0")),
            ),
            (
                "hit rate above one",
                Box::new(|f| f.strategies[0].envelope.min_hit_rate_bp = 10_001),
            ),
            (
                "drawdown above one",
                Box::new(|f| f.strategies[0].envelope.max_drawdown_bp = 10_001),
            ),
            (
                "no retirement rule",
                Box::new(|f| f.strategies[0].retirement_rules.clear()),
            ),
            (
                "zero-pass breach",
                Box::new(|f| {
                    f.strategies[0].retirement_rules = vec![RetirementRule::EnvelopeBreached {
                        consecutive_passes: 0,
                    }]
                }),
            ),
            (
                "retires on a hypothesis it does not hold",
                Box::new(|f| {
                    f.strategies[0].retirement_rules = vec![RetirementRule::HypothesisRefuted {
                        hypothesis: s("h-other"),
                    }]
                }),
            ),
        ],
    );
}

// ---- OpportunityToken (7068) -------------------------------------------

fn leg(venue: &str, side: LegSide) -> TokenLeg {
    TokenLeg {
        venue: VenueId::new(venue),
        instrument: s("BTC-USD"),
        side,
        quantity: dec!("0.5"),
    }
}

fn token() -> OpportunityTokenFields {
    OpportunityTokenFields {
        schema_version: 1,
        opportunity: OpportunityId::new("arb-btc"),
        cycle_id: s("cycle-42"),
        version: 2,
        lineage: TokenLineage::Supersedes { version: 1 },
        legs: vec![leg("SIM-A", LegSide::Buy), leg("SIM-B", LegSide::Sell)],
        size: dec!("30000"),
        edge_bp: 12,
        issued_at: t(0),
        expires_at: t(2),
        reservations: strings(&["rsv-a", "rsv-b"]),
        execution: ExecutionPolicy::AllOrNone,
        unwind: UnwindPolicy::Flatten {
            max_loss: dec!("50"),
        },
    }
}

#[test]
fn an_opportunity_token_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(token())?;
    refuses_missing_unknown_and_foreign_version(token());
    round_trips(OpportunityTokenFields {
        version: 1,
        lineage: TokenLineage::Original,
        unwind: UnwindPolicy::Hedge {
            venue: VenueId::new("SIM-C"),
            instrument: s("BTC-PERP"),
        },
        ..token()
    })?;
    let token: OpportunityToken = OpportunityToken::new(token())?;
    assert_eq!(token.legs.len(), 2);
    Ok(())
}

#[test]
fn an_opportunity_token_refuses_each_broken_invariant() {
    each_refused(
        &token(),
        vec![
            (
                "blank opportunity",
                Box::new(|f| f.opportunity = OpportunityId::new("")),
            ),
            ("blank cycle", Box::new(|f| f.cycle_id = s(""))),
            ("one leg", Box::new(|f| f.legs.truncate(1))),
            (
                "zero quantity",
                Box::new(|f| f.legs[0].quantity = dec!("0")),
            ),
            (
                "blank venue",
                Box::new(|f| f.legs[1].venue = VenueId::new("")),
            ),
            ("zero size", Box::new(|f| f.size = dec!("0"))),
            ("zero edge", Box::new(|f| f.edge_bp = 0)),
            ("negative edge", Box::new(|f| f.edge_bp = -3)),
            ("expiry at issue", Box::new(|f| f.expires_at = f.issued_at)),
            (
                "blank reservation",
                Box::new(|f| f.reservations = strings(&[""])),
            ),
            (
                "negative unwind loss",
                Box::new(|f| {
                    f.unwind = UnwindPolicy::Flatten {
                        max_loss: dec!("-1"),
                    }
                }),
            ),
            (
                "blank hedge instrument",
                Box::new(|f| {
                    f.unwind = UnwindPolicy::Hedge {
                        venue: VenueId::new("SIM-C"),
                        instrument: s(""),
                    }
                }),
            ),
            (
                "original not at version 1",
                Box::new(|f| f.lineage = TokenLineage::Original),
            ),
            (
                "supersedes itself",
                Box::new(|f| f.lineage = TokenLineage::Supersedes { version: 2 }),
            ),
            (
                "supersedes a later version",
                Box::new(|f| f.lineage = TokenLineage::Supersedes { version: 3 }),
            ),
        ],
    );
}

// ---- MicrostructureDelta (7074) ----------------------------------------

fn residual(kind: ResidualKind) -> Residual {
    Residual {
        kind,
        instrument: s("XNYS:ACME"),
        modelled: dec!("1.5"),
        observed: dec!("2.25"),
        samples: 40,
    }
}

fn micro() -> MicrostructureDeltaFields {
    MicrostructureDeltaFields {
        schema_version: 1,
        delta_id: s("md-1"),
        cell: s("cell-us-east"),
        venue: VenueId::new("SIM-XNYS"),
        model_version: s("fill-model@3"),
        window_start: t(0),
        window_end: t(60),
        residuals: vec![
            residual(ResidualKind::Slippage),
            residual(ResidualKind::FillProbability),
            residual(ResidualKind::Impact),
            residual(ResidualKind::Liquidity),
            residual(ResidualKind::LeadLag),
            residual(ResidualKind::VenueBehaviour),
        ],
    }
}

#[test]
fn a_microstructure_delta_round_trips_and_its_residual_is_signed() -> Result<()> {
    round_trips(micro())?;
    refuses_missing_unknown_and_foreign_version(micro());
    let delta: MicrostructureDelta = MicrostructureDelta::new(micro())?;
    assert_eq!(delta.residuals[0].error(), dec!("0.75"));
    Ok(())
}

#[test]
fn a_microstructure_delta_refuses_each_broken_invariant() {
    each_refused(
        &micro(),
        vec![
            ("blank id", Box::new(|f| f.delta_id = s(""))),
            ("blank cell", Box::new(|f| f.cell = s(""))),
            ("blank venue", Box::new(|f| f.venue = VenueId::new(""))),
            ("blank model", Box::new(|f| f.model_version = s(""))),
            ("empty window", Box::new(|f| f.window_end = f.window_start)),
            ("no residual", Box::new(|f| f.residuals.clear())),
            (
                "blank instrument",
                Box::new(|f| f.residuals[0].instrument = s("")),
            ),
            ("zero samples", Box::new(|f| f.residuals[0].samples = 0)),
            (
                "same kind and instrument twice",
                Box::new(|f| f.residuals.push(residual(ResidualKind::Slippage))),
            ),
        ],
    );
}

// ---- WorldModelState (7075) --------------------------------------------

fn edge(cause: &str, effect: &str) -> CausalEdge {
    CausalEdge {
        cause: s(cause),
        effect: s(effect),
        strength_bp: -2_500,
    }
}

fn world() -> WorldModelStateFields {
    WorldModelStateFields {
        schema_version: 1,
        model: s("macro-rates@4"),
        scope: s("US rates"),
        horizon: Duration::from_days(30),
        state: vec![
            StateVariable {
                name: s("fed_funds_move"),
                distribution: categorical(),
            },
            StateVariable {
                name: s("ten_year_yield"),
                distribution: OutcomeDistribution::Quantiles {
                    points: vec![
                        QuantilePoint {
                            quantile_bp: 1_000,
                            value: dec!("3.9"),
                        },
                        QuantilePoint {
                            quantile_bp: 5_000,
                            value: dec!("4.2"),
                        },
                        QuantilePoint {
                            quantile_bp: 9_000,
                            value: dec!("4.6"),
                        },
                    ],
                },
            },
        ],
        causal_structure: vec![edge("cpi", "fed_funds_move")],
        evidence_lineage: strings(&["ev-cpi-sept"]),
        assumptions: strings(&["no fiscal shock"]),
        calibration: measured(),
        branch_parent: BranchParent::Root,
        updated_at: t(0),
        expires_at: t(86_400),
    }
}

#[test]
fn a_world_model_state_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(world())?;
    refuses_missing_unknown_and_foreign_version(world());
    round_trips(WorldModelStateFields {
        branch_parent: BranchParent::Branch {
            id: s("macro-rates@3"),
        },
        calibration: Calibration::Uncalibrated,
        causal_structure: vec![],
        ..world()
    })?;
    let state: WorldModelState = WorldModelState::new(world())?;
    assert_eq!(state.state.len(), 2);
    Ok(())
}

#[test]
fn a_world_model_state_refuses_each_broken_invariant() {
    each_refused(
        &world(),
        vec![
            ("blank model", Box::new(|f| f.model = s(""))),
            ("blank scope", Box::new(|f| f.scope = s(""))),
            ("zero horizon", Box::new(|f| f.horizon = Duration::ZERO)),
            ("no state", Box::new(|f| f.state.clear())),
            (
                "variable twice",
                Box::new(|f| f.state[1].name = s("fed_funds_move")),
            ),
            (
                "mass short of one",
                Box::new(|f| {
                    f.state[0].distribution = OutcomeDistribution::Categorical {
                        outcomes: vec![
                            OutcomeMass {
                                outcome: s("up"),
                                probability_bp: 6_000,
                            },
                            OutcomeMass {
                                outcome: s("down"),
                                probability_bp: 3_999,
                            },
                        ],
                    }
                }),
            ),
            (
                "decreasing quantile values",
                Box::new(|f| {
                    f.state[1].distribution = OutcomeDistribution::Quantiles {
                        points: vec![
                            QuantilePoint {
                                quantile_bp: 1_000,
                                value: dec!("4.9"),
                            },
                            QuantilePoint {
                                quantile_bp: 9_000,
                                value: dec!("4.6"),
                            },
                        ],
                    }
                }),
            ),
            (
                "quantile at one",
                Box::new(|f| {
                    f.state[1].distribution = OutcomeDistribution::Quantiles {
                        points: vec![
                            QuantilePoint {
                                quantile_bp: 5_000,
                                value: dec!("4.2"),
                            },
                            QuantilePoint {
                                quantile_bp: 10_000,
                                value: dec!("4.6"),
                            },
                        ],
                    }
                }),
            ),
            (
                "self-loop",
                Box::new(|f| f.causal_structure = vec![edge("cpi", "cpi")]),
            ),
            (
                "edge twice",
                Box::new(|f| f.causal_structure.push(edge("cpi", "fed_funds_move"))),
            ),
            (
                "strength beyond one",
                Box::new(|f| f.causal_structure[0].strength_bp = -10_001),
            ),
            ("no evidence", Box::new(|f| f.evidence_lineage.clear())),
            ("no assumption", Box::new(|f| f.assumptions.clear())),
            (
                "calibration over nothing",
                Box::new(|f| {
                    f.calibration = Calibration::Measured {
                        method: s("brier"),
                        score: dec!("0.1"),
                        resolved: 0,
                        evaluated_at: t(0),
                    }
                }),
            ),
            (
                "own parent",
                Box::new(|f| {
                    f.branch_parent = BranchParent::Branch {
                        id: s("macro-rates@4"),
                    }
                }),
            ),
            (
                "expiry at update",
                Box::new(|f| f.expires_at = f.updated_at),
            ),
        ],
    );
}

// ---- ModelDisagreement (7076) ------------------------------------------

fn view(id: &str) -> DisputingView {
    DisputingView {
        id: s(id),
        supporting_evidence: strings(&["ev-cpi-sept"]),
        calibration: measured(),
    }
}

fn disagreement() -> ModelDisagreementFields {
    ModelDisagreementFields {
        schema_version: 1,
        disagreement_id: s("dis-1"),
        views: vec![view("macro-rates@4"), view("market-implied@2")],
        disputed_propositions: strings(&["the Fed cuts in December"]),
        unresolved_assumptions: strings(&["no fiscal shock"]),
        status: ArbitrationStatus::Resolved {
            preferred: s("market-implied@2"),
            reason: s("better calibrated over the last 240 resolutions"),
        },
        raised_at: t(0),
    }
}

#[test]
fn a_model_disagreement_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(disagreement())?;
    refuses_missing_unknown_and_foreign_version(disagreement());
    for status in [
        ArbitrationStatus::Open,
        ArbitrationStatus::Branched,
        ArbitrationStatus::EvidenceRequested {
            request_id: s("ir-1"),
        },
    ] {
        round_trips(ModelDisagreementFields {
            status,
            ..disagreement()
        })?;
    }
    let d: ModelDisagreement = ModelDisagreement::new(disagreement())?;
    assert_eq!(d.views.len(), 2);
    Ok(())
}

#[test]
fn a_model_disagreement_refuses_each_broken_invariant() {
    each_refused(
        &disagreement(),
        vec![
            ("blank id", Box::new(|f| f.disagreement_id = s(""))),
            ("one view", Box::new(|f| f.views.truncate(1))),
            (
                "view twice",
                Box::new(|f| f.views[1].id = s("macro-rates@4")),
            ),
            (
                "view citing nothing",
                Box::new(|f| f.views[0].supporting_evidence.clear()),
            ),
            (
                "no proposition",
                Box::new(|f| f.disputed_propositions.clear()),
            ),
            (
                "blank assumption",
                Box::new(|f| f.unresolved_assumptions = strings(&[""])),
            ),
            (
                "resolved for a stranger",
                Box::new(|f| {
                    f.status = ArbitrationStatus::Resolved {
                        preferred: s("someone-else"),
                        reason: s("because"),
                    }
                }),
            ),
            (
                "resolved without a reason",
                Box::new(|f| {
                    f.status = ArbitrationStatus::Resolved {
                        preferred: s("macro-rates@4"),
                        reason: s(""),
                    }
                }),
            ),
            (
                "blank evidence request",
                Box::new(|f| f.status = ArbitrationStatus::EvidenceRequested { request_id: s("") }),
            ),
        ],
    );
}

// ---- SymbolicDerivation (7077) -----------------------------------------

fn derivation() -> SymbolicDerivationFields {
    SymbolicDerivationFields {
        schema_version: 1,
        derivation_id: s("sd-1"),
        goal: s("is the basket within the concentration limit"),
        conclusion: s("concentration 18% <= limit 20%"),
        rules: strings(&["rule:concentration<=20%"]),
        evidence: strings(&["ev-positions-close"]),
        solver: s("in-tree-checker@1"),
        result: ProofResult::Proved,
        violated_assumptions: vec![],
        uncertainty_bp: 0,
        trace: vec![
            TraceStep {
                statement: s("concentration is 18%"),
                inference: s("aggregate"),
                premises: vec![Premise::Evidence {
                    id: s("ev-positions-close"),
                }],
            },
            TraceStep {
                statement: s("concentration 18% <= limit 20%"),
                inference: s("compare"),
                premises: vec![
                    Premise::Step { index: 0 },
                    Premise::Rule {
                        id: s("rule:concentration<=20%"),
                    },
                ],
            },
        ],
    }
}

#[test]
fn a_symbolic_derivation_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()>
{
    round_trips(derivation())?;
    refuses_missing_unknown_and_foreign_version(derivation());
    let d: SymbolicDerivation = SymbolicDerivation::new(derivation())?;
    assert_eq!(d.trace.len(), 2);
    Ok(())
}

/// The trace is checked, not stored: a forward reference, an undeclared
/// premise or a trace that stops short of its conclusion is refused.
#[test]
fn a_symbolic_derivation_refuses_each_broken_invariant_including_an_uncheckable_trace() {
    each_refused(
        &derivation(),
        vec![
            ("blank id", Box::new(|f| f.derivation_id = s(""))),
            ("blank goal", Box::new(|f| f.goal = s(""))),
            ("blank solver", Box::new(|f| f.solver = s(""))),
            ("no rule", Box::new(|f| f.rules.clear())),
            (
                "uncertainty above one",
                Box::new(|f| f.uncertainty_bp = 10_001),
            ),
            (
                "blank violated assumption",
                Box::new(|f| f.violated_assumptions = strings(&[""])),
            ),
            ("no trace", Box::new(|f| f.trace.clear())),
            (
                "premise-less step",
                Box::new(|f| f.trace[0].premises.clear()),
            ),
            (
                "step citing itself",
                Box::new(|f| f.trace[1].premises[0] = Premise::Step { index: 1 }),
            ),
            (
                "step citing a later step",
                Box::new(|f| f.trace[0].premises = vec![Premise::Step { index: 1 }]),
            ),
            (
                "undeclared rule",
                Box::new(|f| {
                    f.trace[1].premises[1] = Premise::Rule {
                        id: s("rule:made-up"),
                    }
                }),
            ),
            (
                "undeclared evidence",
                Box::new(|f| f.trace[0].premises = vec![Premise::Evidence { id: s("ev-other") }]),
            ),
            (
                "trace stops short of the conclusion",
                Box::new(|f| f.conclusion = s("something else")),
            ),
        ],
    );
}

// ---- KnowledgeDelta (7088) ---------------------------------------------

fn knowledge() -> KnowledgeDeltaFields {
    KnowledgeDeltaFields {
        schema_version: 1,
        delta_id: s("kd-1"),
        target: KnowledgeTarget::Ontology,
        base_version: 7,
        new_version: 8,
        changes: vec![
            ChangeOp::Assert {
                key: s("ACME.sector"),
                value: s("industrials"),
            },
            ChangeOp::Retract {
                key: s("ACME.legacy_ticker"),
            },
        ],
        provenance: strings(&["ev-1"]),
        confidence_bp: 9_000,
        conflicts: strings(&["kd-0"]),
        originating_brain: s("deepbrain"),
        originating_region: s("europe-west1"),
        merge_policy: MergePolicy::ReviewRequired,
        created_at: t(0),
    }
}

#[test]
fn a_knowledge_delta_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(knowledge())?;
    refuses_missing_unknown_and_foreign_version(knowledge());
    round_trips(KnowledgeDeltaFields {
        conflicts: vec![],
        merge_policy: MergePolicy::FastForwardOnly,
        target: KnowledgeTarget::Memory,
        ..knowledge()
    })?;
    let d: KnowledgeDelta = KnowledgeDelta::new(knowledge())?;
    assert_eq!(d.new_version, 8);
    Ok(())
}

#[test]
fn a_knowledge_delta_refuses_each_broken_invariant() {
    each_refused(
        &knowledge(),
        vec![
            ("blank id", Box::new(|f| f.delta_id = s(""))),
            ("version gap", Box::new(|f| f.new_version = 9)),
            ("version rewind", Box::new(|f| f.new_version = 6)),
            ("version unchanged", Box::new(|f| f.new_version = 7)),
            (
                "version overflow",
                Box::new(|f| {
                    f.base_version = u64::MAX;
                    f.new_version = 0;
                }),
            ),
            ("no change", Box::new(|f| f.changes.clear())),
            (
                "blank key",
                Box::new(|f| f.changes = vec![ChangeOp::Retract { key: s("") }]),
            ),
            ("no provenance", Box::new(|f| f.provenance.clear())),
            (
                "confidence above one",
                Box::new(|f| f.confidence_bp = 10_001),
            ),
            ("blank conflict", Box::new(|f| f.conflicts = strings(&[""]))),
            ("blank brain", Box::new(|f| f.originating_brain = s(""))),
            ("blank region", Box::new(|f| f.originating_region = s(""))),
            (
                "fast-forward over a conflict",
                Box::new(|f| f.merge_policy = MergePolicy::FastForwardOnly),
            ),
        ],
    );
}

// ---- ControlPackAnnouncement (7091) ------------------------------------

fn announcement() -> ControlPackAnnouncementFields {
    ControlPackAnnouncementFields {
        schema_version: 1,
        kind: ArtifactKind::ModelPack,
        uri: format!("gs://qip-artifacts/sha256/{DIGEST_A}"),
        digest: s(DIGEST_A),
        version: 3,
        signer: s("release-signer"),
        attestation: s(DIGEST_B),
        target_regions: strings(&["us-east4", "europe-west1"]),
        target_nodes: strings(&["edge-us-1", "edge-eu-1"]),
        activate_from: t(0),
        activate_until: t(600),
        expires_at: t(86_400),
        rollback_parent: RollbackParent::Artifact {
            digest: s(DIGEST_B),
        },
        required_acks: 2,
    }
}

#[test]
fn a_control_pack_announcement_round_trips_and_refuses_a_missing_unknown_or_foreign_field()
-> Result<()> {
    round_trips(announcement())?;
    refuses_missing_unknown_and_foreign_version(announcement());
    round_trips(ControlPackAnnouncementFields {
        rollback_parent: RollbackParent::None,
        ..announcement()
    })?;
    let a: ControlPackAnnouncement = ControlPackAnnouncement::new(announcement())?;
    assert_eq!(a.required_acks, 2);
    Ok(())
}

#[test]
fn a_control_pack_announcement_refuses_each_broken_invariant() {
    each_refused(
        &announcement(),
        vec![
            ("bad digest", Box::new(|f| f.digest = s("nope"))),
            (
                "mutable locator",
                Box::new(|f| f.uri = s("gs://qip-artifacts/latest")),
            ),
            ("version zero", Box::new(|f| f.version = 0)),
            ("blank signer", Box::new(|f| f.signer = s(""))),
            ("bad attestation", Box::new(|f| f.attestation = s("sig"))),
            ("no region", Box::new(|f| f.target_regions.clear())),
            ("no node", Box::new(|f| f.target_nodes.clear())),
            (
                "empty window",
                Box::new(|f| f.activate_until = f.activate_from),
            ),
            (
                "expires inside the window",
                Box::new(|f| f.expires_at = t(300)),
            ),
            (
                "rolls back to itself",
                Box::new(|f| {
                    f.rollback_parent = RollbackParent::Artifact {
                        digest: s(DIGEST_A),
                    }
                }),
            ),
            ("zero acks", Box::new(|f| f.required_acks = 0)),
            ("more acks than nodes", Box::new(|f| f.required_acks = 3)),
        ],
    );
}

// ---- MirrorCheckpoint (7092) -------------------------------------------

fn checkpoint() -> MirrorCheckpointFields {
    MirrorCheckpointFields {
        schema_version: 1,
        source_region: s("us-east4"),
        destination_region: s("europe-west1"),
        stream: s("fills"),
        partition: 3,
        source_epoch: 9,
        source_offset: 1_000,
        source_high_water: 1_040,
        destination_offset: 512,
        lag_messages: 40,
        lag_time: Duration::from_millis(250),
        integrity_hash: s(DIGEST_A),
        last_success_at: t(0),
    }
}

#[test]
fn a_mirror_checkpoint_round_trips_and_refuses_a_missing_unknown_or_foreign_field() -> Result<()> {
    round_trips(checkpoint())?;
    refuses_missing_unknown_and_foreign_version(checkpoint());
    round_trips(MirrorCheckpointFields {
        source_offset: 1_040,
        lag_messages: 0,
        lag_time: Duration::ZERO,
        ..checkpoint()
    })?;
    let m: MirrorCheckpoint = MirrorCheckpoint::new(checkpoint())?;
    assert_eq!(m.lag_messages, 40);
    Ok(())
}

#[test]
fn a_mirror_checkpoint_refuses_each_broken_invariant() {
    each_refused(
        &checkpoint(),
        vec![
            ("blank source", Box::new(|f| f.source_region = s(""))),
            (
                "blank destination",
                Box::new(|f| f.destination_region = s("")),
            ),
            ("blank stream", Box::new(|f| f.stream = s(""))),
            (
                "mirror to itself",
                Box::new(|f| f.destination_region = s("us-east4")),
            ),
            (
                "past the high-water mark",
                Box::new(|f| {
                    f.source_offset = 1_041;
                    f.lag_messages = 0;
                }),
            ),
            ("lag disagrees", Box::new(|f| f.lag_messages = 39)),
            (
                "lag time with nothing behind",
                Box::new(|f| {
                    f.source_offset = 1_040;
                    f.lag_messages = 0;
                }),
            ),
            (
                "negative lag time",
                Box::new(|f| f.lag_time = Duration::from_millis(-1)),
            ),
            ("bad hash", Box::new(|f| f.integrity_hash = s(""))),
        ],
    );
}

// ---- ActionIntent, §27 (7082) ------------------------------------------

fn intent() -> ActionIntentFields {
    let payload = s(r#"{"to":"research-desk","body":"weekly summary"}"#);
    ActionIntentFields {
        schema_version: 1,
        intent_id: s("ai-1"),
        identity: ActingIdentity {
            actor: s("agency-brain"),
            on_behalf_of: s("research-desk"),
        },
        channel: s("tool:internal-mail"),
        payload_digest: ActionIntentFields::digest_payload(&payload),
        payload,
        provenance: strings(&["goal-7", "plan-7"]),
        disclosure: Disclosure::Required {
            statements: strings(&["generated by an automated research system"]),
        },
        policy_decisions: vec![PolicyDecision {
            policy: s("outbound-communication"),
            verdict: Verdict::Allowed,
            decided_at: t(0),
        }],
        created_at: t(0),
        expires_at: t(600),
        idempotency_key: s("ai-1#1"),
        audit_lineage: strings(&["goal-7", "plan-7", "approval-3"]),
    }
}

#[test]
fn a_section_27_action_intent_round_trips_and_refuses_a_missing_unknown_or_foreign_field()
-> Result<()> {
    round_trips(intent())?;
    refuses_missing_unknown_and_foreign_version(intent());
    round_trips(ActionIntentFields {
        disclosure: Disclosure::NoneRequired,
        ..intent()
    })?;
    let a: ActionIntent = ActionIntent::new(intent())?;
    assert_eq!(a.channel, "tool:internal-mail");
    Ok(())
}

/// The payload digest is recomputed: an intent whose payload was edited
/// after review carries a digest that no longer matches and is refused.
#[test]
fn a_section_27_action_intent_refuses_each_broken_invariant_including_an_edited_payload() {
    each_refused(
        &intent(),
        vec![
            ("blank id", Box::new(|f| f.intent_id = s(""))),
            ("blank actor", Box::new(|f| f.identity.actor = s(""))),
            (
                "blank principal",
                Box::new(|f| f.identity.on_behalf_of = s("")),
            ),
            ("blank channel", Box::new(|f| f.channel = s(""))),
            ("blank payload", Box::new(|f| f.payload = s(""))),
            (
                "payload edited after review",
                Box::new(|f| f.payload = s(r#"{"to":"everyone"}"#)),
            ),
            ("malformed digest", Box::new(|f| f.payload_digest = s("x"))),
            ("no provenance", Box::new(|f| f.provenance.clear())),
            (
                "required disclosure with no statement",
                Box::new(|f| f.disclosure = Disclosure::Required { statements: vec![] }),
            ),
            (
                "no policy decision",
                Box::new(|f| f.policy_decisions.clear()),
            ),
            (
                "denied by a policy",
                Box::new(|f| f.policy_decisions[0].verdict = Verdict::Denied),
            ),
            (
                "policy decided twice",
                Box::new(|f| f.policy_decisions.push(f.policy_decisions[0].clone())),
            ),
            (
                "expiry at creation",
                Box::new(|f| f.expires_at = f.created_at),
            ),
            (
                "blank idempotency key",
                Box::new(|f| f.idempotency_key = s("")),
            ),
            ("no audit lineage", Box::new(|f| f.audit_lineage.clear())),
        ],
    );
}
