//! The versions in force at an order, recovered from the journal alone.
//!
//! The chain recorded the policy sequence and the grants an order drew on but
//! not which plan, or which model weights, produced the signal behind it, so
//! a replay could name the payload and not the computation: a model swapped
//! under an unchanged name read the same in the chain. A plan is now an
//! entry of its own, and `qip_edge::replay` reads the versions back.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::feature::FeatureKey;
use qip_contracts::policy::{ModelManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::{Decision, Journal};
use qip_edge::policy::VerifiedPolicy;
use qip_edge::replay::versions_in_force;
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
    payload.trained_models = Slot::produced(ModelManifest::unreleased(models), t(10));
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, t(10))
}

fn order(id: &str, grants: Vec<(String, String)>) -> Decision {
    Decision::OrderSent {
        order_id: id.to_string(),
        venue: "XLON".to_string(),
        quantity: "1".to_string(),
        simulated: true,
        release_at: None,
        equalised: false,
        grants,
    }
}

#[test]
fn deploying_a_plan_with_a_model_journals_the_plan_digest_and_the_model_digest() -> Result<()> {
    let model = regime_model(0.1)?;
    let mut cell = cell()?;
    cell.apply_policy(
        manifest_policy(1, BTreeMap::from([("regime".to_string(), model.digest())]))?,
        t(11),
    )?;
    let (strategy, program) = strategy_with(Some(model.clone()))?;
    cell.deploy(strategy, program, grant()?)?;

    let admitted: Vec<_> = cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::PlanAdmitted {
                strategy,
                digest,
                models,
            } => Some((strategy.clone(), digest.clone(), models.clone())),
            _ => None,
        })
        .collect();
    // The premise: one admission, for the one deployment.
    assert_eq!(admitted.len(), 1, "{admitted:?}");
    let (strategy, digest, models) = &admitted[0];
    assert_eq!(strategy, STRATEGY);
    // The digest is the cell's own answer to "what runs", not a second hash.
    assert_eq!(
        Some(digest.clone()),
        cell.deployment_digest(STRATEGY)?,
        "the journal named a digest the cell does not give for what runs"
    );
    assert_eq!(models, &vec![("regime".to_string(), model.digest())]);

    // Reclassifying changes the digest, so it is admitted again at the new one.
    cell.classify(
        STRATEGY,
        qip_contracts::degradation::StrategyClass::EventDriven,
    )?;
    let latest = cell
        .journal()
        .entries()
        .iter()
        .rev()
        .find_map(|entry| match &entry.decision {
            Decision::PlanAdmitted { digest, .. } => Some(digest.clone()),
            _ => None,
        });
    assert_ne!(
        latest.as_deref(),
        Some(digest.as_str()),
        "the premise failed"
    );
    assert_eq!(latest, cell.deployment_digest(STRATEGY)?);
    Ok(())
}

#[test]
fn the_versions_in_force_at_each_order_are_named_from_the_journal_alone() {
    let mut journal = Journal::new();
    let plan = |strategy: &str, digest: &str, model: &str| Decision::PlanAdmitted {
        strategy: strategy.to_string(),
        digest: digest.to_string(),
        models: vec![("regime".to_string(), model.to_string())],
    };
    journal.record(
        Decision::PolicyApplied {
            sequence: 7,
            halted: false,
            narrowed: Vec::new(),
        },
        t(1),
    );
    journal.record(plan("alpha", "plan-a1", "model-m1"), t(2));
    journal.record(order("o-1", vec![("alpha".into(), "sig-1".into())]), t(3));
    // The plan is replaced at a new digest and weights, a newer payload lands,
    // and a second strategy joins.
    journal.record(plan("alpha", "plan-a2", "model-m2"), t(4));
    journal.record(plan("beta", "plan-b1", "model-m2"), t(4));
    journal.record(
        Decision::PolicyApplied {
            sequence: 8,
            halted: false,
            narrowed: Vec::new(),
        },
        t(5),
    );
    journal.record(order("o-2", Vec::new()), t(6));
    journal.record(
        Decision::StrategyWithdrawn {
            strategy: "alpha".into(),
        },
        t(7),
    );
    journal.record(order("o-3", Vec::new()), t(8));

    let named = versions_in_force(journal.entries());
    let ids: Vec<&str> = named.iter().map(|v| v.order_id.as_str()).collect();
    assert_eq!(ids, vec!["o-1", "o-2", "o-3"], "one record per OrderSent");

    assert_eq!(named[0].policy_sequence, Some(7));
    assert_eq!(
        named[0].plans,
        BTreeMap::from([("alpha".to_string(), "plan-a1".to_string())])
    );
    assert_eq!(
        named[0].models,
        BTreeMap::from([("regime".to_string(), "model-m1".to_string())])
    );
    assert_eq!(
        named[0].grants,
        vec![("alpha".to_string(), "sig-1".to_string())]
    );

    assert_eq!(named[1].policy_sequence, Some(8));
    assert_eq!(
        named[1].plans,
        BTreeMap::from([
            ("alpha".to_string(), "plan-a2".to_string()),
            ("beta".to_string(), "plan-b1".to_string())
        ])
    );
    assert_eq!(
        named[1].models,
        BTreeMap::from([("regime".to_string(), "model-m2".to_string())])
    );

    // alpha withdrawn: only beta remains in force.
    assert_eq!(
        named[2].plans,
        BTreeMap::from([("beta".to_string(), "plan-b1".to_string())])
    );
}

#[test]
fn an_order_before_any_policy_or_plan_is_named_with_no_version_rather_than_a_guess() {
    let mut journal = Journal::new();
    journal.record(order("o-0", Vec::new()), t(1));
    let named = versions_in_force(journal.entries());
    assert_eq!(named.len(), 1, "the premise failed");
    assert_eq!(named[0].policy_sequence, None);
    assert!(named[0].plans.is_empty() && named[0].models.is_empty());
}
