//! ADR 0480 (R12-MODEL-7022), across the seam: a microstructure model the
//! kernel trained from the cells' folded residuals reaches a cell only as a
//! registry-signed release pack, verified by the cell itself under its trust
//! bundle.
//!
//! The kernel half (`qip-kernel/tests/microstructure_release.rs`) proves the
//! centre withholds the model until a verified pack names it. This half
//! proves the other side: the manifest the kernel issues, carried in a
//! payload signed by the per-cycle `policy` key, installs the trained model
//! in a bundle cell, which evaluates it — and the same model named without
//! its pack, which is what a compromised issuer would send, is refused by
//! the cell whatever the payload's own signature says.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_ai::registry::ModelRegistry;
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::feature::FeatureKey;
use qip_contracts::microstructure_delta::{Residual, ResidualKind};
use qip_contracts::policy::{InlineModel, ModelManifest, ModelPack, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::trust::{ArtefactClass, ArtefactSigner, Publication, TrustBundle, TrustedKey};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::TrustRoot;
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_financial::universe::Universe;
use qip_kernel::microstructure_model::MicrostructureFold;
use qip_kernel::microstructure_training::{self, MODELLED_FEATURE};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::model::DistilledModel;
use qip_strategy::program::Program;
use qip_training::local::SkillPolicy;
use qip_training::serve::InTreeProvider;
use qip_transport::trust::{BundleVerifier, LocalSigner};
use std::collections::{BTreeMap, BTreeSet};

const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const CELL: &str = "london-1";
const STRATEGY: &str = "microstructure-aware-1";

const VENUE: &str = "XLON";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new_serving(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
        Box::new(InTreeProvider),
    )?;
    platform.name_model_desk("microstructure-release-tests/model-desk")?;
    Ok(platform)
}

/// Fold records as LEARN journals them: one per concluded delta, eight
/// instruments each, the venue's observed slippage running at
/// `1.3 · modelled + 2` bps with a small deterministic residual. `from..to`
/// numbers the deltas; each ends a minute after the last, before `start()`.
fn folds(from: usize, to: usize) -> Result<Vec<MicrostructureFold>> {
    let mut folds = Vec::new();
    for d in from..to {
        let ends = start().saturating_sub(Duration::from_mins((1_000 - d) as i64));
        let mut residuals = Vec::new();
        for i in 0..8usize {
            let modelled = ((d * 8 + i) as f64 * 0.37).sin() * 10.0 + 12.0;
            let noise = (((d * 8 + i) * 7919) % 13) as f64 / 13.0 * 0.2 - 0.1;
            let observed = 1.3 * modelled + 2.0 + noise;
            let decimal = |value: f64| {
                Decimal::from_f64((value * 1_000.0).round() / 1_000.0)
                    .ok_or_else(|| Error::numeric("a fixture figure has no decimal form"))
            };
            residuals.push(Residual {
                kind: ResidualKind::Slippage,
                instrument: format!("I{i}"),
                modelled: decimal(modelled)?,
                observed: decimal(observed)?,
                samples: 4,
            });
        }
        folds.push(MicrostructureFold {
            venue: VENUE.to_string(),
            cell: "london-1".to_string(),
            delta_id: format!("delta-{d}"),
            model_version: "cell-model-1".to_string(),
            window_end: ends,
            residuals,
            at: ends,
        });
    }
    Ok(folds)
}

struct Keys {
    policy: LocalSigner,
    model_pack: LocalSigner,
}

fn keys() -> Result<Keys> {
    Ok(Keys {
        policy: LocalSigner::generate(ArtefactClass::Policy, "test/policy/1")?,
        model_pack: LocalSigner::generate(ArtefactClass::ModelPack, "test/model-pack/1")?,
    })
}

fn bundle(keys: &Keys) -> Result<BundleVerifier> {
    let entries = [&keys.policy, &keys.model_pack]
        .into_iter()
        .map(|signer| TrustedKey {
            class: signer.class(),
            key_version: signer.key_version().to_string(),
            public_key: signer.public_key_hex(),
            not_after: Timestamp::from_secs(4_000_000_000),
        })
        .collect();
    Ok(BundleVerifier::new(TrustBundle::new(
        entries,
        BTreeSet::new(),
    )?))
}

/// Train, register, test, trial and promote the microstructure model
/// through the kernel, as `qip-kernel/tests/microstructure_release.rs`
/// does, and return the platform, the artifact digest and the distillate.
fn promoted() -> Result<(Platform, String, DistilledModel)> {
    let mut platform = platform()?;
    let mut registry = ModelRegistry::new();
    let teacher = microstructure_training::train(
        &folds(0, 30)?,
        VENUE,
        ResidualKind::Slippage,
        "1.0.0",
        start(),
    )?;
    let registration = microstructure_training::register(
        &mut registry,
        &teacher,
        &SkillPolicy::default(),
        "microstructure-pack-tests",
        start(),
    )?;
    assert!(registration.passed, "premise: {:?}", registration.verdict);
    let artifact = InTreeProvider::pack(&teacher)?;
    let recorded =
        microstructure_training::dataset(&folds(30, 40)?, VENUE, ResidualKind::Slippage)?;
    let tests = qip_kernel::central::models::run_promotion_tests(
        &InTreeProvider,
        &mut registry,
        &artifact,
        &recorded,
        start(),
    )?;
    assert_eq!(tests.gap(&artifact.digest), None, "premise: tests passed");
    for round in 1..=i64::from(qip_ai::registry::MIN_TRIAL_ROUNDS) {
        registry.record_trial_round(
            &artifact.reference,
            qip_ai::registry::TrialVenue::Paper,
            "staged by the test",
            start().saturating_sub(Duration::from_secs(
                i64::from(qip_ai::registry::MIN_TRIAL_ROUNDS) + 1 - round,
            )),
        )?;
    }
    let student = microstructure_training::distil(&teacher)?;
    platform.promote_model(&mut registry, &artifact, Some(&student), &[], start())?;
    Ok((platform, artifact.digest, student))
}

fn release(keys: &Keys, artifact: &str, student: &DistilledModel) -> Result<ModelPack> {
    ModelPack {
        artifact_digest: artifact.to_string(),
        signature: String::new(),
        features: vec![MODELLED_FEATURE.to_string()],
        calibration: 0.9,
        allowed_universes: BTreeSet::from(["XLON:ACME".to_string()]),
        budget_microseconds: 50,
        expires_at: 1_900_000_000,
        rollback_parent: String::new(),
        publication: Publication {
            author: "author@example.com".to_string(),
            approver: "approver@example.com".to_string(),
            approved_at: start(),
        },
        inline_model: Some(InlineModel {
            name: student.name().to_string(),
            digest: student.digest(),
        }),
    }
    .signed_by(&keys.model_pack)
}

fn cell() -> Result<Cell> {
    let config = CellConfig::new(CELL, "europe-west2").with_venue(VenueId::new(VENUE));
    let engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    Cell::new(config, engine)
}

fn grant() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            dec!("1000"),
            dec!("400"),
            dec!("50000"),
            vec![VenueId::new(VENUE)],
            start(),
            start().saturating_add(Duration::from_secs(3600)),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(
        build(&signature)?,
        ENVELOPE_KEY,
        CELL,
        start().saturating_add(Duration::from_secs(10)),
    )
}

/// A strategy whose entry reads the trained model over a declared feature,
/// so the compiled plan carries it inline as `Op::Model`.
fn strategy_with(model: DistilledModel) -> Result<(CompiledStrategy, Program)> {
    let subject = ObjectId::from_string("obj-ACME");
    let modelled = FeatureKey::new("modelled-slippage", subject.clone());
    let condition = Expr::Model {
        model,
        inputs: vec![Expr::feature(modelled.clone())],
    }
    .greater_than(Expr::Statistic(0.0));
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), subject, Duration::from_secs(30))
        .with_rule(Rule::new(
            "enter",
            SignalKind::Enter,
            condition,
            Expr::Exact(dec!("1")),
            Expr::Statistic(0.5),
            100,
        ));
    let catalogue = FeatureCatalogue::new().declaring([(modelled, Type::Statistic)])?;
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn issued(keys: &Keys, manifest: ModelManifest) -> Result<PolicyPayload> {
    let mut payload = PolicyPayload::unproduced(7, CELL, start());
    payload.trained_models = Slot::produced(manifest, start());
    payload.signed_by(&keys.policy)
}

/// Mutations, each restored byte for byte: (1) drop the ADR 0480 arm in
/// `Platform::model_manifest_targeted` and the kernel names the model with
/// no release admitted; the first assertion fails. (2) Delete the
/// `check_releases` call in `VerifiedPolicy::verify_with` and the cell
/// admits the model named without its pack; the refusal assertion fails.
#[test]
fn a_trained_microstructure_model_reaches_a_bundle_cell_only_as_a_signed_release_pack() -> Result<()>
{
    let keys = keys()?;
    let (mut platform, artifact, student) = promoted()?;
    let now = start().saturating_add(Duration::from_secs(11));

    // The centre withholds the model while no release is admitted.
    assert!(
        platform.model_manifest().is_err(),
        "the kernel named a microstructure model with no release pack admitted"
    );

    // A compromised issuer names it anyway, without the pack, under the
    // genuine per-cycle key: the bundle cell refuses the payload.
    let unreleased = ModelManifest::unreleased(BTreeMap::from([(
        student.name().to_string(),
        student.digest(),
    )]));
    let refused = VerifiedPolicy::verify_with(
        issued(&keys, unreleased)?,
        &TrustRoot::bundle(bundle(&keys)?),
        CELL,
        now,
    )
    .expect_err("a bundle cell admitted a microstructure model with no release pack");
    assert!(
        refused.message().contains("carries no release pack"),
        "{}",
        refused.message()
    );
    let mut bare = cell()?;
    let (strategy, program) = strategy_with(student.clone())?;
    assert!(
        bare.deploy(strategy, program, grant()?).is_err(),
        "a cell holding no manifest installed the trained model"
    );

    // The release: signed under the model-pack key, admitted by the kernel
    // through the same bundle the cell holds, and issued.
    platform.admit_model_packs(
        vec![release(&keys, &artifact, &student)?],
        &bundle(&keys)?,
        start(),
    )?;
    let manifest = platform.model_manifest()?.manifest().clone();
    assert!(manifest.packs.contains_key(student.name()));
    let admitted = VerifiedPolicy::verify_with(
        issued(&keys, manifest)?,
        &TrustRoot::bundle(bundle(&keys)?),
        CELL,
        now,
    )?;
    let mut cell = cell()?;
    cell.apply_policy(admitted, now)?;
    let (strategy, program) = strategy_with(student.clone())?;
    cell.deploy(strategy, program, grant()?)?;
    assert_eq!(cell.deployed_strategies(), vec![STRATEGY]);

    // What the cell runs is the trained fit: observed slippage at
    // 1.3 · modelled + 2, within the fixture's noise.
    let predicted = student.evaluate(&[10.0])?;
    assert!(
        (predicted - 15.0).abs() < 0.2,
        "the distillate does not compute the trained fit: {predicted}"
    );
    Ok(())
}
