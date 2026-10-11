//! ADR 0110 at the deploy stage: once a composition root admits signed
//! release packs, the `trained_models` slot names to a cell only the
//! distillates whose artifact a verified, two-person pack covers.
//!
//! Every promotion here is a real fit through `LocalTrainer`, registered and
//! promoted through the platform's provider, so the manifest being gated is
//! the one a deployment ships and not a hand-built map.

#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::ModelRegistry;
use qip_contracts::policy::ModelPack;
use qip_contracts::trust::{ArtefactClass, ArtefactVerifier, Publication};
use qip_core::error::{Error, Result};
use qip_core::{Context, Timestamp};
use qip_financial::universe::Universe;
use qip_kernel::central::models::register_fit;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_strategy::model::DistilledModel;
use qip_training::dataset::TrainingDataset;
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, SkillPolicy, TrainedTeacher};
use qip_training::serve::InTreeProvider;
use std::collections::BTreeSet;

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
    platform.name_model_desk("release-pack-tests/model-desk")?;
    Ok(platform)
}

fn teacher(name: &str) -> Result<TrainedTeacher> {
    let mut rows = Vec::new();
    let mut targets = Vec::new();
    let mut times = Vec::new();
    for i in 0..200usize {
        let a = (i as f64 * 0.37).sin();
        let b = (i as f64 * 0.11).cos();
        let residual = ((i * 7919) % 13) as f64 / 13.0 * 0.02 - 0.01;
        rows.push(vec![a, b]);
        targets.push(0.5 * a - 0.25 * b + 0.1 + residual);
        times.push(start().saturating_add(qip_core::Duration::from_mins(i as i64)));
    }
    let data = TrainingDataset::new(
        format!("{name}-data"),
        vec!["a".to_string(), "b".to_string()],
        rows,
        targets,
        times,
    )?;
    let spec = TrainingSpec::new(
        name,
        "0.1.0",
        "release-pack-tests",
        data.name(),
        ModelFamily::Linear { ridge: 1e-6 },
    )
    .with_holdout(0.25);
    LocalTrainer::new().fit(&spec, &data, start())
}

/// Admits a pack whose signature is exactly the one given, standing in for
/// the bundle verifier so this test is about the kernel's gate and not the
/// curve (which `qip-transport`'s own tests prove).
#[derive(Debug)]
struct Admits(&'static str);

impl ArtefactVerifier for Admits {
    fn verify(
        &self,
        class: ArtefactClass,
        _canonical: &[u8],
        signature: &str,
        _now: Timestamp,
    ) -> Result<()> {
        if class == ArtefactClass::ModelPack && signature == self.0 {
            Ok(())
        } else {
            Err(Error::denied("does not verify"))
        }
    }
}

fn pack_for(digest: &str) -> ModelPack {
    ModelPack {
        artifact_digest: digest.to_string(),
        signature: "released".to_string(),
        features: vec!["a".to_string(), "b".to_string()],
        calibration: 0.9,
        allowed_universes: BTreeSet::from(["XNYS:ACME".to_string()]),
        budget_microseconds: 50,
        expires_at: 1_900_000_000,
        rollback_parent: String::new(),
        publication: Publication {
            author: "author".to_string(),
            approver: "approver".to_string(),
            approved_at: start(),
        },
    }
}

/// Mutation: drop the `release_packs` arm in `Platform::model_manifest`
/// and the unreleased distillate is named to every cell; this fails.
#[test]
fn a_promoted_distillate_reaches_the_manifest_only_when_a_verified_release_pack_names_its_artifact()
-> Result<()> {
    let mut platform = platform()?;
    let mut registry = ModelRegistry::new();
    let fitted = teacher("release-linear")?;
    let registration = register_fit(
        &mut registry,
        &fitted,
        &SkillPolicy::default(),
        "release-pack-tests",
        start(),
    )?;
    assert!(
        registration.passed,
        "premise: the fit cleared the skill bar"
    );
    let artifact = InTreeProvider::pack(&fitted)?;
    let student = DistilledModel::linear("release-linear", 0.1, vec![0.5, -0.25])?;
    platform.promote_model(&mut registry, &artifact, Some(&student), &[], start())?;

    // Premise: ungated, the promotion is named to the cells.
    assert!(!platform.model_packs_gate_the_manifest());
    assert_eq!(
        platform
            .model_manifest()?
            .manifest()
            .models
            .get(student.name()),
        Some(&student.digest())
    );

    // A release that covers some other artifact: the promotion is withheld.
    platform.admit_model_packs(
        vec![pack_for("not-this-artifact")],
        &Admits("released"),
        start(),
    )?;
    let refused = platform
        .model_manifest()
        .expect_err("a distillate no release pack names was named to a cell");
    assert!(
        refused.message().contains("release pack"),
        "{}",
        refused.message()
    );

    // The release that covers it: named again.
    platform.admit_model_packs(
        vec![pack_for(&artifact.digest)],
        &Admits("released"),
        start(),
    )?;
    let issue = platform.model_manifest()?;
    assert_eq!(
        issue.manifest().models.get(student.name()),
        Some(&student.digest())
    );
    assert_eq!(issue.unreleased(), 0);

    // A pack that does not verify is refused whole and leaves the gate as it
    // was, rather than half-replacing it.
    assert!(
        platform
            .admit_model_packs(vec![pack_for(&artifact.digest)], &Admits("other"), start())
            .is_err()
    );
    assert!(platform.model_manifest().is_ok());
    Ok(())
}
