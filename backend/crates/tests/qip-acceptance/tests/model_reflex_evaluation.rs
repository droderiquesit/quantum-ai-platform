//! MODEL-004: reflex cells evaluate models within latency budgets.
//!
//! The previous version of this file built one two-leaf tree and called
//! `evaluate` on it under five test names — "order-book state", "fill
//! probability", "adverse selection", "short-horizon signals", "opportunity
//! scoring" — none of which touched a cell, a plan or a budget. It passed with
//! the cell unable to run a model at all. These tests put a model into a
//! strategy plan, promote it through the cell's model manifest, and watch the
//! cell's pass act on the model's output; and they drive the compiler's
//! latency budget with a model too expensive to admit.
//!
//! **Gap:** the five named purposes are not a production concept. Nothing in
//! `qip-strategy` or `qip-edge` types a model by purpose, so "a model for fill
//! probability" and "a model for opportunity scoring" are indistinguishable
//! to the cell. That is not asserted here, because an absence of a type has no
//! behaviour to drive; it is reported.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{ModelManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompilerLimits, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::model::{DistilledModel, TreeNode};
use std::collections::BTreeMap;

const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-model-004";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-model-004";
const CELL: &str = "london-1";
const STRATEGY: &str = "modelled-1";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn grant() -> VerifiedEnvelope {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            dec!("1000"),
            dec!("400"),
            dec!("50000"),
            vec![VenueId::new("XLON")],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned").unwrap();
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature).unwrap(), ENVELOPE_KEY, CELL, t(10)).unwrap()
}

/// A rule that enters when `model`, fed the constant input `1.0`, scores
/// above zero. A constant input means the model's own output is the only
/// thing that can differ between two runs.
fn spec_with(model: DistilledModel) -> StrategySpec {
    spec_with_arity(model)
}

/// A cell whose manifest promotes `model` at its digest, running a plan that
/// carries it.
fn cell_running(model: DistilledModel) -> Cell {
    let config = CellConfig::new(CELL, "europe-west2").with_venue(VenueId::new("XLON"));
    let mut cell = Cell::new(
        config,
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
    )
    .unwrap();
    let mut payload = PolicyPayload::unproduced(1, CELL, t(10));
    payload.trained_models = Slot::produced(
        ModelManifest::unreleased(BTreeMap::from([(model.name().to_string(), model.digest())])),
        t(10),
    );
    let policy =
        VerifiedPolicy::verify(payload.signed(POLICY_KEY).unwrap(), POLICY_KEY, CELL, t(10))
            .unwrap();
    cell.apply_policy(policy, t(11)).unwrap();
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let compiled = compiler.compile(&spec_with(model)).unwrap();
    cell.deploy(compiled, compiler.into_program(), grant())
        .unwrap();
    assert_eq!(
        cell.deployed_strategies(),
        vec![STRATEGY],
        "premise: deployed"
    );
    cell
}

/// A paper gateway that accepts nothing in particular; these tests read
/// signals, not orders.
#[derive(Debug, Default)]
struct PaperGateway;

impl Placer for PaperGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: qip_contracts::message::BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> qip_core::error::Result<()> {
        Ok(())
    }
}

/// The cell evaluates the promoted model inside its pass, and the model's
/// output — nothing else — decides whether the rule fires. Two cells differ
/// only in the model's intercept: `0.1 + 0.75·1.0 = 0.85` enters, `-1.0 +
/// 0.75·1.0 = -0.25` does not.
///
/// Mutation: in `qip-strategy`'s runtime, make `Op::Model` evaluate to a
/// constant (e.g. `Ok(Value::Statistic(1.0))`) instead of calling the model —
/// both cells then raise the signal and the second assertion fires. Making it
/// evaluate to `0.0` instead fails the first.
#[test]
fn a_reflex_cell_evaluates_a_promoted_model_in_its_pass_and_the_models_output_decides_the_signal() {
    let positive = DistilledModel::linear("regime", 0.1, vec![0.75]).unwrap();
    let negative = DistilledModel::linear("regime", -1.0, vec![0.75]).unwrap();
    assert!(positive.evaluate(&[1.0]).unwrap() > 0.0, "premise");
    assert!(negative.evaluate(&[1.0]).unwrap() < 0.0, "premise");
    assert_ne!(positive.digest(), negative.digest(), "premise: two models");

    let mut entering = cell_running(positive);
    let report = entering.work(t(20), &mut PaperGateway).unwrap();
    assert!(
        report.signals.iter().any(|s| s.kind == SignalKind::Enter),
        "the positive model did not raise the signal: {:?}",
        report.refusals
    );

    let mut holding = cell_running(negative);
    let report = holding.work(t(20), &mut PaperGateway).unwrap();
    assert!(
        report.signals.iter().all(|s| s.kind != SignalKind::Enter),
        "the negative model raised the signal anyway: {:?}",
        report.signals
    );
}

/// "Within latency budgets": a model is charged its worst-case cost against
/// the compiler's node budget, and a plan whose model exceeds it is refused
/// before it can reach a cell.
///
/// Mutation: in `StrategyCompiler::measure`, charge `Expr::Model` as `1`
/// instead of `model.cost()` — the 600-coefficient model compiles and the
/// refusal assertion fires.
#[test]
fn a_model_whose_worst_case_cost_exceeds_the_reflex_budget_is_refused_at_compile() {
    let limits = CompilerLimits::default();
    let small = DistilledModel::linear("small", 0.0, vec![0.1; 4]).unwrap();
    // A chain tree over ONE input: its cost is its node count, while the
    // plan around it carries a single input node. The previous fixture was a
    // 600-coefficient linear model, whose 600 input nodes alone exceed the
    // budget, so the refusal held with the model's own cost charged as `1`.
    let nodes: Vec<TreeNode> = (0..600)
        .map(|index| {
            if index == 599 {
                TreeNode::Leaf { value: 1.0 }
            } else {
                TreeNode::Branch {
                    input: 0,
                    threshold: 0.5,
                    below: index + 1,
                    at_or_above: index + 1,
                }
            }
        })
        .collect();
    let large = DistilledModel::tree("large", 1, nodes).unwrap();
    // Premises: one fits, one does not, by the model's own declared cost, and
    // the large one is refused for that cost and not for its inputs.
    assert!(small.cost() < limits.max_nodes);
    assert!(large.cost() > limits.max_nodes);
    assert_eq!(large.arity(), 1, "premise: the plan around it is tiny");
    assert!(
        StrategyCompiler::with_limits(FeatureCatalogue::new(), limits)
            .compile(&spec_with_arity(small))
            .is_ok(),
        "premise: a model inside the budget compiles"
    );
    let refused = StrategyCompiler::with_limits(FeatureCatalogue::new(), limits)
        .compile(&spec_with_arity(large))
        .expect_err("a model over the latency budget compiled");
    assert!(refused.to_string().contains("budget"), "{refused}");
}

/// A rule entering when `model`, fed `1.0` on every input, scores above zero.
fn spec_with_arity(model: DistilledModel) -> StrategySpec {
    let inputs = vec![Expr::Statistic(1.0); model.arity()];
    StrategySpec::new(
        StrategyId::new(STRATEGY),
        ObjectId::from_string("obj-ACME"),
        Duration::from_secs(30),
    )
    .with_rule(Rule::new(
        "enter-when-the-model-says-so",
        SignalKind::Enter,
        Expr::Model { model, inputs }.greater_than(Expr::Statistic(0.0)),
        Expr::Exact(dec!("1")),
        Expr::Statistic(0.5),
        100,
    ))
}
