//! ADR 0480 (R12-MODEL-7022): a microstructure model trained from the
//! cells' folded residuals reaches the `trained_models` slot only when a
//! verified release pack names its artifact and its inline model — and,
//! unlike every other distillate, not when no release pack was admitted at
//! all.
//!
//! Every step is the production path: the fold records LEARN journals, the
//! trainer's dataset and `LocalTrainer` fit, the platform's registration,
//! the four promotion tests and a completed trial, `Platform::promote_model`,
//! and `Platform::model_manifest`. Nothing here hand-builds a manifest.

#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::ModelRegistry;
use qip_contracts::microstructure_delta::{Residual, ResidualKind};
use qip_contracts::policy::{InlineModel, ModelPack};
use qip_contracts::trust::{ArtefactClass, ArtefactVerifier, Publication};
use qip_core::error::{Error, Result};
use qip_core::{Context, Decimal, Duration, Timestamp};
use qip_financial::universe::Universe;
use qip_kernel::microstructure_model::MicrostructureFold;
use qip_kernel::microstructure_training::{
    self, MICROSTRUCTURE_PURPOSE, MODELLED_FEATURE, RELEASE_PACK_REQUIRED_STAGE,
};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_training::local::SkillPolicy;
use qip_training::serve::InTreeProvider;
use std::collections::BTreeSet;

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

/// Admits a pack whose signature is exactly the one given, standing in for
/// the bundle verifier so this test is about the kernel's gate and not the
/// curve (`qip-transport`'s own tests prove that).
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

fn pack_for(artifact: &str, name: &str, digest: &str, signature: &str) -> ModelPack {
    ModelPack {
        artifact_digest: artifact.to_string(),
        signature: signature.to_string(),
        features: vec![MODELLED_FEATURE.to_string()],
        calibration: 0.9,
        allowed_universes: BTreeSet::from(["XLON:ACME".to_string()]),
        budget_microseconds: 50,
        expires_at: 1_900_000_000,
        rollback_parent: String::new(),
        publication: Publication {
            author: "author".to_string(),
            approver: "approver".to_string(),
            approved_at: start(),
        },
        inline_model: Some(InlineModel {
            name: name.to_string(),
            digest: digest.to_string(),
        }),
    }
}

/// Mutations, each restored byte for byte, run 2026-11-13: (1) the ADR 0480
/// arm in `Platform::model_manifest_targeted` made `if false` — the unpacked
/// model is named with no release admitted and the `expect_err` on the
/// first manifest fails. (2) The stage push in `promote_model_from` deleted —
/// the signed-record premise fails, because nothing marks the promotion and
/// so nothing can withhold it. (3) The purpose assignment in
/// `microstructure_training::register` deleted — the card premise fails,
/// and with it the only thing the promotion reads.
#[test]
fn a_trained_microstructure_model_reaches_the_manifest_only_under_a_verified_release_pack()
-> Result<()> {
    let mut platform = platform()?;
    let mut registry = ModelRegistry::new();

    // Train on the cells' folded residuals and register the fit.
    let training = folds(0, 30)?;
    let teacher =
        microstructure_training::train(&training, VENUE, ResidualKind::Slippage, "1.0.0", start())?;
    let registration = microstructure_training::register(
        &mut registry,
        &teacher,
        &SkillPolicy::default(),
        "microstructure-release-tests",
        start(),
    )?;
    assert!(
        registration.passed,
        "premise: the fit cleared the skill bar: {:?}",
        registration.verdict
    );
    assert_eq!(
        registry
            .get(&registration.reference)
            .map(|card| card.purpose.as_str()),
        Some(MICROSTRUCTURE_PURPOSE),
        "premise: the card carries the microstructure purpose"
    );

    // The four promotion tests on a later recorded window, and a completed
    // paper trial before the promotion — so a refusal further on is about
    // the release, not about either.
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
    // The distillate is the fit: it computes what the teacher predicts.
    assert!((student.evaluate(&[10.0])? - teacher.predict(&[10.0])?).abs() < 1e-9);
    platform.promote_model(&mut registry, &artifact, Some(&student), &[], start())?;
    assert!(
        platform.model_promotions()[&artifact.reference]
            .stages
            .iter()
            .any(|stage| stage == RELEASE_PACK_REQUIRED_STAGE),
        "premise: the signed promotion record carries the requirement"
    );

    // No release admitted: every other distillate would be named
    // (release_packs.rs proves that); this one is withheld.
    assert!(!platform.model_packs_gate_the_manifest());
    let withheld = platform
        .model_manifest()
        .expect_err("a microstructure model reached the manifest with no release pack");
    assert!(
        withheld.message().contains("release pack"),
        "{}",
        withheld.message()
    );

    // A pack the verifier refuses is not admitted, and the model stays out.
    assert!(
        platform
            .admit_model_packs(
                vec![pack_for(
                    &artifact.digest,
                    student.name(),
                    &student.digest(),
                    "forged"
                )],
                &Admits("released"),
                start(),
            )
            .is_err(),
        "an unsigned pack was admitted"
    );
    assert!(platform.model_manifest().is_err());

    // A verified release of another artifact: still withheld.
    platform.admit_model_packs(
        vec![pack_for(
            "other-artifact",
            student.name(),
            &student.digest(),
            "released",
        )],
        &Admits("released"),
        start(),
    )?;
    assert!(platform.model_manifest().is_err());

    // The admitting half: a verified release naming this artifact at this
    // inline model. Named, with the pack beside it for the cell to verify.
    platform.admit_model_packs(
        vec![pack_for(
            &artifact.digest,
            student.name(),
            &student.digest(),
            "released",
        )],
        &Admits("released"),
        start(),
    )?;
    let issue = platform.model_manifest()?;
    let manifest = issue.manifest();
    assert_eq!(manifest.models.get(student.name()), Some(&student.digest()));
    assert_eq!(
        manifest
            .packs
            .get(student.name())
            .map(|pack| pack.artifact_digest.as_str()),
        Some(artifact.digest.as_str())
    );
    Ok(())
}

/// The trainer reads its data from the platform's own log, so a platform
/// whose cells have concluded nothing has nothing to train — refused, not a
/// model fitted on zero rows.
#[test]
fn a_platform_with_no_folded_residual_trains_no_microstructure_model() -> Result<()> {
    let platform = platform()?;
    let refused = platform
        .train_microstructure_model(VENUE, ResidualKind::Slippage, "1.0.0", start())
        .expect_err("a model was trained on no folds");
    assert!(
        refused.message().contains("no folded residual"),
        "{}",
        refused.message()
    );
    Ok(())
}
