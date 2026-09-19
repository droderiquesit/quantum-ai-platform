//! ADR 0083's deploy stage, from outside the cell: a plan carrying a learned
//! function inline installs only when the manifest the cell holds names that
//! model at its own digest.
//!
//! The failure this closes is the one the ADR names last: a cell that
//! installs a plan whose inline model is not in its manifest has a model
//! nobody promoted, and the deploy stage is then a formality that reads as a
//! control. Both halves are proven — the refusal, and the admission of the
//! genuine article beside it — because a gate that refuses everything is
//! indistinguishable from a working one until something is supposed to pass.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::feature::FeatureKey;
use qip_contracts::policy::{ModelManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::model::DistilledModel;
use qip_strategy::program::Program;
use std::collections::BTreeMap;

const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-tests";
const CELL: &str = "london-1";
const STRATEGY: &str = "mean-reversion-1";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn cell() -> Result<Cell> {
    let config = CellConfig::new(CELL, "europe-west2").with_venue(VenueId::new("XLON"));
    let engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    Cell::new(config, engine)
}

/// An envelope signed the way the central allocator would sign it.
fn grant() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            Decimal::parse("1000").expect("a decimal literal"),
            Decimal::parse("400").expect("a decimal literal"),
            dec!("50000"),
            vec![VenueId::new("XLON")],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(10))
}

/// A strategy whose entry condition reads `model` over a declared feature,
/// so the compiled plan carries it inline as `Op::Model`. Over a literal
/// input the compiler folds the model to a constant — a first draft of this
/// fixture did that and the premise assertion below caught it.
fn strategy_with(model: Option<DistilledModel>) -> Result<(CompiledStrategy, Program)> {
    let subject = ObjectId::from_string("obj-ACME");
    let pressure = FeatureKey::new("pressure", subject.clone());
    let condition = match model {
        Some(model) => Expr::Model {
            model,
            inputs: vec![Expr::feature(pressure.clone())],
        }
        .greater_than(Expr::Statistic(0.0)),
        None => Expr::Flag(false),
    };
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), subject, Duration::from_secs(30))
        .with_rule(Rule::new(
            "enter",
            SignalKind::Enter,
            condition,
            Expr::Exact(dec!("1")),
            Expr::Statistic(0.5),
            100,
        ));
    let catalogue = FeatureCatalogue::new().declaring([(pressure, Type::Statistic)])?;
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn regime_model(intercept: f64) -> Result<DistilledModel> {
    DistilledModel::linear("regime", intercept, vec![0.75])
}

/// A signed policy whose only produced slot is the model manifest.
fn manifest_policy(sequence: u64, models: BTreeMap<String, String>) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(sequence, CELL, t(10));
    payload.trained_models = Slot::produced(ModelManifest { models }, t(10));
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, t(10))
}

#[test]
fn a_cell_holding_no_manifest_refuses_a_plan_with_an_inline_model_and_admits_one_without()
-> Result<()> {
    // The state every cell is in today: the `trained_models` slot is produced
    // nowhere. The safe reading is that a cell with no list of promoted
    // models has no grounds to say a model was promoted — so a plan carrying
    // one is refused, and a plan carrying none is untouched by the check.
    let mut plain = cell()?;
    assert!(
        plain.policy_sequence().is_none(),
        "the premise failed: a policy is held"
    );
    let (strategy, program) = strategy_with(None)?;
    plain.deploy(strategy, program, grant()?)?;
    assert_eq!(plain.deployed_strategies(), vec![STRATEGY]);

    let mut modelled = cell()?;
    let model = regime_model(0.1)?;
    let (strategy, program) = strategy_with(Some(model.clone()))?;
    // Premise: the plan really does carry the model inline.
    assert!(
        program
            .reachable_from(strategy.plan())
            .iter()
            .filter_map(|node| program.node(*node))
            .any(|node| matches!(&node.op, qip_strategy::program::Op::Model { .. })),
        "the premise failed: the compiled plan carries no model"
    );
    let error = modelled
        .deploy(strategy, program, grant()?)
        .expect_err("a plan carrying a model deployed into a cell with no manifest");
    assert_eq!(error.code(), "denied");
    assert!(
        error.message().contains("holds no model manifest")
            && error.message().contains("model `regime`"),
        "the refusal did not name the gap and the model: {}",
        error.message()
    );
    assert!(
        modelled.deployed_strategies().is_empty(),
        "a refused deployment was still recorded"
    );
    Ok(())
}

#[test]
fn a_plan_is_admitted_only_when_the_manifest_names_its_model_at_its_own_digest() -> Result<()> {
    let model = regime_model(0.1)?;
    let other = regime_model(0.2)?;
    assert_ne!(model.digest(), other.digest(), "the premise failed");

    // A manifest naming the model under its name at *another* digest —
    // the promoted weights are not these weights.
    let mut cell = cell()?;
    cell.apply_policy(
        manifest_policy(1, BTreeMap::from([("regime".to_string(), other.digest())]))?,
        t(11),
    )?;
    let (strategy, program) = strategy_with(Some(model.clone()))?;
    let error = cell
        .deploy(strategy, program, grant()?)
        .expect_err("a model at an unpromoted digest deployed");
    assert_eq!(error.code(), "denied");
    assert!(
        error
            .message()
            .contains(&format!("at digest {}", model.digest()))
            && error.message().contains("does not name it"),
        "{}",
        error.message()
    );
    assert!(cell.deployed_strategies().is_empty());

    // A manifest naming the digest under *another* name — the same weights
    // promoted as something else are not this model.
    cell.apply_policy(
        manifest_policy(
            2,
            BTreeMap::from([("other-model".to_string(), model.digest())]),
        )?,
        t(12),
    )?;
    let (strategy, program) = strategy_with(Some(model.clone()))?;
    assert!(
        cell.deploy(strategy, program, grant()?).is_err(),
        "a digest promoted under another name admitted the model"
    );
    assert!(cell.deployed_strategies().is_empty());

    // The genuine article: name and digest both match, and the plan installs.
    cell.apply_policy(
        manifest_policy(3, BTreeMap::from([("regime".to_string(), model.digest())]))?,
        t(13),
    )?;
    let (strategy, program) = strategy_with(Some(model))?;
    cell.deploy(strategy, program, grant()?)?;
    assert_eq!(cell.deployed_strategies(), vec![STRATEGY]);
    Ok(())
}

#[test]
fn a_produced_but_empty_manifest_still_admits_a_plan_that_carries_no_model() -> Result<()> {
    // The check is about the models a plan carries, not about whether the
    // centre has promoted anything: a cell with a manifest naming nothing
    // runs model-free strategies exactly as before.
    let mut cell = cell()?;
    cell.apply_policy(manifest_policy(1, BTreeMap::new())?, t(11))?;
    let (strategy, program) = strategy_with(None)?;
    cell.deploy(strategy, program, grant()?)?;
    assert_eq!(cell.deployed_strategies(), vec![STRATEGY]);
    Ok(())
}
