//! The promote and deploy stages of blueprint §21.2 as ADR 0083 §5 fixes
//! them: a model is promoted through the platform's provider, journalled
//! before the registry adopts it, named to the cells by the digest a cell
//! checks, and resumed from the log at the next assembly.
//!
//! Every test here builds a real fit through `LocalTrainer` and registers
//! it through `register_fit`, so the card's evaluation is the fit's own and
//! not a `passed: true` asserted by the test.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::{EvaluationRecord, ModelCard, ModelRegistry, ModelStage};
use qip_ai::serving::ModelProvider;
use qip_core::error::{Error, Result};
use qip_core::{Context, ModelId, Timestamp};
use qip_events::Topic;
use qip_financial::universe::Universe;
use qip_kernel::central::models::register_fit;
use qip_kernel::model_serving::MODEL_PROMOTION_ORIGIN;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_strategy::model::DistilledModel;
use qip_training::dataset::TrainingDataset;
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, SkillPolicy, TrainedTeacher};
use qip_training::serve::InTreeProvider;
use std::collections::BTreeMap;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// The model desk every promoting platform here is named for (MODEL-057).
const DESK: &str = "serving-tests/model-desk";

fn platform_serving(config: PlatformConfig) -> Result<Platform> {
    let mut platform = platform_serving_with_no_desk_named(config)?;
    platform.name_model_desk(DESK)?;
    Ok(platform)
}

/// A serving platform whose composition root named no model desk: it can
/// serve, and may promote nothing.
fn platform_serving_with_no_desk_named(config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new_serving(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
        Box::new(InTreeProvider),
    )
}

fn platform_without_a_provider() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

/// A dataset a linear model genuinely explains: `y = 0.5 a - 0.25 b + c`,
/// with a small deterministic residual so the holdout R² is high and the
/// fit clears the skill bar honestly.
fn dataset(name: &str) -> Result<TrainingDataset> {
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
    TrainingDataset::new(
        name,
        vec!["a".to_string(), "b".to_string()],
        rows,
        targets,
        times,
    )
}

fn teacher(name: &str, version: &str) -> Result<TrainedTeacher> {
    let data = dataset(&format!("{name}-data"))?;
    let spec = TrainingSpec::new(
        name,
        version,
        "serving-tests",
        data.name(),
        ModelFamily::Linear { ridge: 1e-6 },
    )
    .with_holdout(0.25);
    LocalTrainer::new().fit(&spec, &data, start())
}

/// Register a real fit and assert the premise every test here rests on:
/// the fit cleared the skill bar on its own holdout, so a refusal further
/// on is about the promotion and not about the evidence.
fn registered(registry: &mut ModelRegistry, teacher: &TrainedTeacher) -> Result<String> {
    let registration = register_fit(
        registry,
        teacher,
        &SkillPolicy::default(),
        "serving-tests",
        start(),
    )?;
    assert!(
        registration.passed,
        "premise: the fit cleared the skill bar ({:?})",
        registration.verdict
    );
    Ok(registration.reference)
}

fn promotion_records(platform: &Platform) -> usize {
    platform
        .event_log()
        .records()
        .iter()
        .filter(|record| {
            record.event.topic == Topic::ModelEvaluated
                && record.event.lineage.producer == MODEL_PROMOTION_ORIGIN
        })
        .count()
}

#[test]
fn a_platform_assembled_without_a_provider_promotes_nothing_and_writes_no_record() -> Result<()> {
    // `Platform::new` holds the null provider. A promotion asked of it must
    // refuse with that provider's own sentence, and — the half that
    // matters — leave the registry and the log exactly as they were, because
    // a card promoted with no record is a model a restart forgets.
    let mut platform = platform_without_a_provider()?;
    let mut registry = ModelRegistry::new();
    let teacher = teacher("bar-linear-null", "0.1.0")?;
    let reference = registered(&mut registry, &teacher)?;
    let artifact = InTreeProvider::pack(&teacher)?;

    let error = platform
        .promote_model(&mut registry, &artifact, None, &[], start())
        .expect_err("the null provider promoted a model");
    assert!(
        error.message().contains("the null provider"),
        "the refusal does not name the provider: {}",
        error.message()
    );
    assert_eq!(
        registry
            .get(&reference)
            .map(|card| card.stage)
            .ok_or_else(|| Error::not_found("the card"))?,
        ModelStage::Development,
        "the registry adopted a promotion the platform refused"
    );
    assert!(platform.model_promotions().is_empty());
    assert_eq!(
        promotion_records(&platform),
        0,
        "a refused promotion reached the log"
    );
    assert!(
        platform.model_manifest().is_err(),
        "a manifest was produced from no promotion"
    );
    Ok(())
}

#[test]
fn a_promotion_is_journalled_before_the_registry_adopts_it_and_names_the_distillate_by_the_digest_a_cell_checks()
-> Result<()> {
    let mut platform = platform_serving(PlatformConfig::default())?;
    let mut registry = ModelRegistry::new();
    let teacher = teacher("bar-linear-aaa", "0.1.0")?;
    let reference = registered(&mut registry, &teacher)?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let student = DistilledModel::linear("bar-linear-aaa", 0.1, vec![0.5, -0.25])?;
    // The premise the register got wrong: the digest a cell checks on
    // install is the distillate's own, and it is not the artifact's SHA-256.
    // A manifest filled from the card's `artifact_digest` would name the
    // model at a digest no cell recognises.
    assert_ne!(
        student.digest(),
        artifact.digest,
        "premise: the two digests differ"
    );

    let published =
        platform.promote_model(&mut registry, &artifact, Some(&student), &[], start())?;
    assert_eq!(published.file_name, format!("{}.json", artifact.digest));

    let card = registry
        .get(&reference)
        .ok_or_else(|| Error::not_found("the card"))?;
    assert_eq!(card.stage, ModelStage::Production);
    assert_eq!(
        card.artifact_digest.as_deref(),
        Some(artifact.digest.as_str())
    );

    let issue = platform.model_manifest()?;
    assert_eq!(
        issue.manifest().models.get(student.name()),
        Some(&student.digest()),
        "the manifest does not name the distillate by the digest `Cell::check_models_promoted` compares"
    );
    assert_eq!(issue.slot().produced_at(), Some(start()));

    assert_eq!(promotion_records(&platform), 1, "one promotion, one record");
    let record = platform
        .model_promotions()
        .get(&reference)
        .ok_or_else(|| Error::not_found("the promotion"))?;
    assert_eq!(record.artifact_digest, artifact.digest);
    assert_eq!(
        record.distilled.as_ref().map(|entry| entry.digest.clone()),
        Some(student.digest())
    );
    Ok(())
}

#[test]
fn a_restarted_platform_resumes_its_promotions_from_the_log_alone() -> Result<()> {
    // The promoted set is a projection of the log, not a second source of
    // truth: a second process over the first's file must ship the same
    // manifest without having promoted anything itself.
    let directory = std::env::temp_dir().join(format!(
        "qip-kernel-serving-{}-{}",
        std::process::id(),
        start().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    let path = directory.join("events.jsonl");
    let teacher = teacher("bar-linear-bbb", "0.1.0")?;
    let student = DistilledModel::linear("bar-linear-bbb", 0.1, vec![0.5, -0.25])?;
    let first_promotions;
    let first_manifest;
    {
        let mut first = platform_serving(PlatformConfig::default().with_event_log_file(&path))?;
        let mut registry = ModelRegistry::new();
        registered(&mut registry, &teacher)?;
        let artifact = InTreeProvider::pack(&teacher)?;
        first.promote_model(&mut registry, &artifact, Some(&student), &[], start())?;
        first_promotions = first.model_promotions().clone();
        first_manifest = first.model_manifest()?.manifest().clone();
        assert_eq!(
            first_promotions.len(),
            1,
            "premise: the first process promoted"
        );
    }
    let second = platform_serving(PlatformConfig::default().with_event_log_file(&path))?;
    assert_eq!(
        promotion_records(&second),
        1,
        "premise: the second process read the first's log back"
    );
    assert_eq!(second.model_promotions(), &first_promotions);
    assert_eq!(second.model_manifest()?.manifest(), &first_manifest);
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

#[test]
fn an_artifact_whose_arity_disagrees_with_its_card_is_refused_before_anything_changes() -> Result<()>
{
    // A card describing three features and an artifact reading two are two
    // claims about one function. The provider serves the artifact happily —
    // it is a well-formed linear model — so the refusal has to be the
    // platform's, and it has to come before the registry, the log or the
    // promoted set move.
    let mut platform = platform_serving(PlatformConfig::default())?;
    let teacher = teacher("bar-linear-ccc", "0.1.0")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let mut registry = ModelRegistry::new();
    let mut card = ModelCard::new(
        ModelId::from_string(teacher.reference()),
        teacher.name(),
        teacher.version(),
        "serving-tests",
        start(),
    )
    .with_features(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    card.evaluations.push(EvaluationRecord {
        evaluated_at: start(),
        dataset: "hand-written".to_string(),
        metrics: BTreeMap::new(),
        passed: true,
    });
    registry.register(card);
    assert!(
        InTreeProvider.serve(&artifact).is_ok(),
        "premise: the provider serves the artifact on its own terms"
    );

    let error = platform
        .promote_model(&mut registry, &artifact, None, &[], start())
        .expect_err("a card and an artifact of different arity were promoted together");
    assert!(
        error.message().contains("different functions"),
        "{}",
        error.message()
    );
    assert_eq!(
        registry.get(&teacher.reference()).map(|card| card.stage),
        Some(ModelStage::Development)
    );
    assert!(platform.model_promotions().is_empty());
    assert_eq!(promotion_records(&platform), 0);
    Ok(())
}

#[test]
fn a_displaced_incumbent_is_retired_on_the_same_record_and_leaves_the_manifest() -> Result<()> {
    // A promotion that beats an incumbent retires it in the same act: one
    // record or none. A cell holding the old distillate inline must refuse
    // it on the next manifest, so the displaced entry leaves the manifest
    // too — and after a restart, from the log alone.
    let directory = std::env::temp_dir().join(format!(
        "qip-kernel-serving-displaced-{}-{}",
        std::process::id(),
        start().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    let path = directory.join("events.jsonl");
    let incumbent = teacher("bar-linear-ddd", "0.1.0")?;
    let successor = teacher("bar-linear-ddd", "0.2.0")?;
    let old_student = DistilledModel::linear("bar-linear-ddd", 0.1, vec![0.5, -0.25])?;
    let new_student = DistilledModel::linear("bar-linear-ddd", 0.1, vec![0.51, -0.24])?;
    assert_ne!(
        old_student.digest(),
        new_student.digest(),
        "premise: two distillates"
    );
    {
        let mut platform = platform_serving(PlatformConfig::default().with_event_log_file(&path))?;
        let mut registry = ModelRegistry::new();
        let old_reference = registered(&mut registry, &incumbent)?;
        let new_reference = registered(&mut registry, &successor)?;
        platform.promote_model(
            &mut registry,
            &InTreeProvider::pack(&incumbent)?,
            Some(&old_student),
            &[],
            start(),
        )?;
        assert_eq!(
            platform
                .model_manifest()?
                .manifest()
                .models
                .get(old_student.name()),
            Some(&old_student.digest()),
            "premise: the incumbent's distillate is in the manifest"
        );
        let later = start().saturating_add(qip_core::Duration::from_hours(1));
        platform.promote_model(
            &mut registry,
            &InTreeProvider::pack(&successor)?,
            Some(&new_student),
            std::slice::from_ref(&old_reference),
            later,
        )?;
        assert_eq!(
            registry.get(&old_reference).map(|card| card.stage),
            Some(ModelStage::Retired),
            "the displaced incumbent was not retired"
        );
        assert_eq!(
            registry.get(&new_reference).map(|card| card.stage),
            Some(ModelStage::Production)
        );
        let manifest = platform.model_manifest()?;
        assert_eq!(manifest.manifest().models.len(), 1);
        assert_eq!(
            manifest.manifest().models.get(new_student.name()),
            Some(&new_student.digest())
        );
        assert_eq!(manifest.slot().produced_at(), Some(later));
        assert!(!platform.model_promotions().contains_key(&old_reference));
        assert_eq!(promotion_records(&platform), 2);
    }
    let restarted = platform_serving(PlatformConfig::default().with_event_log_file(&path))?;
    assert_eq!(
        promotion_records(&restarted),
        2,
        "premise: both records read back"
    );
    let manifest = restarted.model_manifest()?;
    assert_eq!(manifest.manifest().models.len(), 1);
    assert_eq!(
        manifest.manifest().models.get(new_student.name()),
        Some(&new_student.digest())
    );
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

// --- one version, one artifact (MODEL-034) -----------------------------------

#[test]
fn over_generated_publish_sequences_a_version_always_resolves_to_the_digest_first_published_under_it()
-> Result<()> {
    use qip_core::{Rng, Xoshiro256};
    let directory = std::env::temp_dir().join(format!(
        "qip-kernel-versions-{}-{}",
        std::process::id(),
        start().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    let path = directory.join("events.jsonl");

    // The same version carries different bytes by changing only the ridge.
    let variant = |version: &str, ridge: f64| -> Result<TrainedTeacher> {
        let data = dataset("versioned-data")?;
        let spec = TrainingSpec::new(
            "bar-linear-versioned",
            version,
            "serving-tests",
            data.name(),
            ModelFamily::Linear { ridge },
        )
        .with_holdout(0.25);
        LocalTrainer::new().fit(&spec, &data, start())
    };
    let ridges = [1e-6, 1e-2, 1.0];
    let versions = ["0.1.0", "0.2.0", "0.3.0"];
    // Premise: the variants really are different bytes, or a refusal below
    // could never be told from an admission.
    let digests: Vec<String> = ridges
        .iter()
        .map(|ridge| Ok(InTreeProvider::pack(&variant("0.1.0", *ridge)?)?.digest))
        .collect::<Result<_>>()?;
    assert!(digests[0] != digests[1] && digests[1] != digests[2] && digests[0] != digests[2]);

    let mut rng = Xoshiro256::seeded(34);
    let mut first: BTreeMap<String, String> = BTreeMap::new();
    let (mut refused, mut admitted, mut restarts) = (0, 0, 0);
    let mut platform = platform_serving(PlatformConfig::default().with_event_log_file(&path))?;
    for step in 0..36 {
        if step % 9 == 8 {
            // A restart over the same log: a new process, a new registry and a
            // fit counter that begins again at the same versions.
            drop(platform);
            platform = platform_serving(PlatformConfig::default().with_event_log_file(&path))?;
            restarts += 1;
        }
        let version = versions[(rng.next_f64() * 3.0) as usize % 3];
        let ridge = ridges[(rng.next_f64() * 3.0) as usize % 3];
        let teacher = variant(version, ridge)?;
        let mut registry = ModelRegistry::new();
        let reference = registered(&mut registry, &teacher)?;
        let artifact = InTreeProvider::pack(&teacher)?;
        let now = start().saturating_add(qip_core::Duration::from_mins(step + 1));
        let outcome = platform.promote_model(&mut registry, &artifact, None, &[], now);
        match first.get(&reference) {
            Some(digest) if *digest != artifact.digest => {
                assert!(
                    outcome.is_err(),
                    "{reference} was republished with other bytes"
                );
                refused += 1;
            }
            _ => {
                outcome?;
                first
                    .entry(reference)
                    .or_insert_with(|| artifact.digest.clone());
                admitted += 1;
            }
        }
    }
    // The sequence exercised what it claims to: admissions, refusals, restarts.
    assert!(
        admitted > 0 && refused > 0 && restarts > 0,
        "{admitted}/{refused}/{restarts}"
    );
    // And the platform resolves every version to its first digest.
    for (reference, digest) in &first {
        let held = platform.model_promotions().get(reference);
        assert!(
            held.is_none_or(|record| &record.artifact_digest == digest),
            "{reference} resolves to a later digest"
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

// --- rollback to the last known-good (MODEL-045) -----------------------------

fn teacher_with_ridge(name: &str, version: &str, ridge: f64) -> Result<TrainedTeacher> {
    let data = dataset(&format!("{name}-data"))?;
    let spec = TrainingSpec::new(
        name,
        version,
        "serving-tests",
        data.name(),
        ModelFamily::Linear { ridge },
    )
    .with_holdout(0.25);
    LocalTrainer::new().fit(&spec, &data, start())
}

#[test]
fn a_degraded_model_is_retired_and_the_one_it_displaced_returns_at_the_digest_that_passed()
-> Result<()> {
    let directory = std::env::temp_dir().join(format!(
        "qip-kernel-rollback-{}-{}",
        std::process::id(),
        start().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    let path = directory.join("events.jsonl");
    let mut registry = ModelRegistry::new();
    let (first_manifest, first_digest, second_reference, first_reference);
    {
        let mut platform = platform_serving(PlatformConfig::default().with_event_log_file(&path))?;
        let old = teacher_with_ridge("bar-linear-rb", "0.1.0", 1e-6)?;
        let new = teacher_with_ridge("bar-linear-rb", "0.2.0", 1.0)?;
        first_reference = registered(&mut registry, &old)?;
        second_reference = registered(&mut registry, &new)?;
        let old_artifact = InTreeProvider::pack(&old)?;
        let new_artifact = InTreeProvider::pack(&new)?;
        assert_ne!(
            old_artifact.digest, new_artifact.digest,
            "premise: the two versions are different bytes"
        );
        let old_student = DistilledModel::linear("bar-linear-rb", 0.1, vec![0.5, -0.25])?;
        let new_student = DistilledModel::linear("bar-linear-rb", 0.1, vec![0.4, -0.2])?;
        platform.promote_model(
            &mut registry,
            &old_artifact,
            Some(&old_student),
            &[],
            start(),
        )?;
        first_manifest = platform.model_manifest()?.manifest().clone();
        let later = start().saturating_add(qip_core::Duration::from_mins(1));
        platform.promote_model(
            &mut registry,
            &new_artifact,
            Some(&new_student),
            std::slice::from_ref(&first_reference),
            later,
        )?;
        // Premise: the successor is what stands, and the manifest says so, so
        // a rollback has something to undo.
        assert_ne!(platform.model_manifest()?.manifest(), &first_manifest);
        assert_eq!(
            registry.get(&first_reference).map(|card| card.stage),
            Some(ModelStage::Retired)
        );
        first_digest = old_artifact.digest.clone();

        let reactivated = platform.rollback_model(
            &mut registry,
            &second_reference,
            later.saturating_add(qip_core::Duration::from_mins(1)),
        )?;
        assert_eq!(reactivated, first_reference);
        assert_eq!(
            registry.get(&second_reference).map(|card| card.stage),
            Some(ModelStage::Retired)
        );
        let card = registry
            .get(&first_reference)
            .ok_or_else(|| Error::not_found("the card"))?;
        assert_eq!(card.stage, ModelStage::Production);
        assert_eq!(card.artifact_digest.as_deref(), Some(first_digest.as_str()));
        assert_eq!(
            platform.model_manifest()?.manifest(),
            &first_manifest,
            "the cells are told the old distillate again"
        );
        assert!(!platform.model_promotions().contains_key(&second_reference));
        assert_eq!(promotion_records(&platform), 3);
    }
    // The rollback is a record, so a restart over the log lands in the same place.
    let mut restart_config = PlatformConfig::default().with_event_log_file(&path);
    // A resumed deterministic run mints the ids a fresh one would at the same
    // instant; a different seed keeps the restart off the first run's ids.
    restart_config.seed ^= 1;
    let restarted = platform_serving(restart_config)?;
    assert_eq!(restarted.model_manifest()?.manifest(), &first_manifest);
    assert!(restarted.model_promotions().contains_key(&first_reference));
    assert!(!restarted.model_promotions().contains_key(&second_reference));
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

#[test]
fn a_rollback_refuses_a_predecessor_whose_card_no_longer_carries_the_digest_that_passed()
-> Result<()> {
    let mut platform = platform_serving(PlatformConfig::default())?;
    let mut registry = ModelRegistry::new();
    let old = teacher_with_ridge("bar-linear-rb-edit", "0.1.0", 1e-6)?;
    let new = teacher_with_ridge("bar-linear-rb-edit", "0.2.0", 1.0)?;
    let old_reference = registered(&mut registry, &old)?;
    let new_reference = registered(&mut registry, &new)?;
    platform.promote_model(
        &mut registry,
        &InTreeProvider::pack(&old)?,
        None,
        &[],
        start(),
    )?;
    let later = start().saturating_add(qip_core::Duration::from_mins(1));
    platform.promote_model(
        &mut registry,
        &InTreeProvider::pack(&new)?,
        None,
        std::slice::from_ref(&old_reference),
        later,
    )?;
    registry
        .get_mut(&old_reference)
        .ok_or_else(|| Error::not_found("the old card"))?
        .artifact_digest = Some("0".repeat(64));
    let refused = platform
        .rollback_model(&mut registry, &new_reference, later)
        .unwrap_err();
    assert!(
        refused.message().contains("first recorded"),
        "the refusal does not name the digest mismatch: {refused}"
    );
    assert_eq!(
        registry.get(&new_reference).map(|card| card.stage),
        Some(ModelStage::Production),
        "a refused rollback retired the live model"
    );
    assert_eq!(promotion_records(&platform), 2);
    Ok(())
}

#[test]
fn a_rollback_with_no_known_good_predecessor_is_refused_and_changes_nothing() -> Result<()> {
    let mut platform = platform_serving(PlatformConfig::default())?;
    let mut registry = ModelRegistry::new();
    let teacher = teacher_with_ridge("bar-linear-rb-alone", "0.1.0", 1e-6)?;
    let reference = registered(&mut registry, &teacher)?;
    let artifact = InTreeProvider::pack(&teacher)?;
    platform.promote_model(&mut registry, &artifact, None, &[], start())?;
    let refused = platform
        .rollback_model(&mut registry, &reference, start())
        .unwrap_err();
    assert!(refused.message().contains("known-good"), "{refused}");
    assert_eq!(
        registry.get(&reference).map(|card| card.stage),
        Some(ModelStage::Production),
        "a refused rollback retired the only model"
    );
    assert_eq!(promotion_records(&platform), 1);
    assert!(platform.model_promotions().contains_key(&reference));
    Ok(())
}

// --- the producer of a candidate (MODEL-067) --------------------------------

#[test]
fn a_candidate_produced_by_the_quantum_gateway_is_refused_and_leaves_no_record() -> Result<()> {
    use qip_kernel::ModelProducer;
    let mut platform = platform_serving(PlatformConfig::default())?;
    let mut registry = ModelRegistry::new();
    let teacher = teacher("bar-linear-qgw", "0.1.0")?;
    let reference = registered(&mut registry, &teacher)?;
    let artifact = InTreeProvider::pack(&teacher)?;

    // Premise: the very same artifact is promotable when the training
    // pipeline produced it, so the refusal is about the producer alone.
    let mut elsewhere = registry.clone();
    platform_serving(PlatformConfig::default())?.promote_model_from(
        ModelProducer::TrainingPipeline,
        &mut elsewhere,
        &artifact,
        None,
        &[],
        start(),
    )?;

    let refused = platform
        .promote_model_from(
            ModelProducer::QuantumGateway,
            &mut registry,
            &artifact,
            None,
            &[],
            start(),
        )
        .unwrap_err();
    assert!(refused.message().contains("quantum gateway"), "{refused}");
    assert_eq!(
        registry.get(&reference).map(|card| card.stage),
        Some(ModelStage::Development)
    );
    assert_eq!(promotion_records(&platform), 0);
    Ok(())
}

#[test]
fn a_quantum_informed_candidate_is_admitted_only_with_its_calibrated_baseline_attached()
-> Result<()> {
    use qip_kernel::ModelProducer;
    let mut platform = platform_serving(PlatformConfig::default())?;
    let mut registry = ModelRegistry::new();
    let candidate = teacher("bar-linear-qi", "0.1.0")?;
    let reference = registered(&mut registry, &candidate)?;
    let artifact = InTreeProvider::pack(&candidate)?;

    let baseline = teacher("bar-baseline", "0.1.0")?;
    let baseline_reference = baseline.reference();
    let informed = || ModelProducer::QuantumInformed {
        baseline: baseline_reference.clone(),
    };

    // No baseline on record: refused.
    let refused = platform
        .promote_model_from(informed(), &mut registry, &artifact, None, &[], start())
        .unwrap_err();
    assert!(refused.message().contains("baseline"), "{refused}");

    // On record but never calibrated: refused.
    registered(&mut registry, &baseline)?;
    assert!(
        platform
            .promote_model_from(informed(), &mut registry, &artifact, None, &[], start())
            .is_err()
    );
    assert_eq!(promotion_records(&platform), 0, "a refusal reached the log");

    // Calibrated and on record: admitted, and the log names the producer.
    let data = dataset("bar-baseline-data")?;
    let (fit_set, _) = data.split_at_fraction(0.25)?;
    registered(&mut registry, &baseline.calibrated_on(&fit_set)?)?;
    platform.promote_model_from(informed(), &mut registry, &artifact, None, &[], start())?;
    assert_eq!(
        platform
            .model_promotions()
            .get(&reference)
            .map(|record| record.producer.clone()),
        Some(informed())
    );
    Ok(())
}

// --- Model Pack lineage (MODEL-037) -----------------------------------------

#[test]
fn every_accepted_model_makes_a_pack_naming_its_predecessor_and_delta_and_walking_the_lineage_rebuilds_each_membership()
-> Result<()> {
    // The promoted set had deltas in the log and no version identity: nobody
    // could say which pack a cell had been told about, what it superseded, or
    // rebuild an earlier pack without replaying records by hand. A pack that
    // named its delta wrongly would pass any test that only read the newest
    // membership, so every pack here is checked against the membership the
    // test itself built from what it promoted.
    use qip_kernel::model_serving::pack_membership;
    let directory = std::env::temp_dir().join(format!(
        "qip-kernel-pack-{}-{}",
        std::process::id(),
        start().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    let path = directory.join("events.jsonl");
    let minute = |n: i64| start().saturating_add(qip_core::Duration::from_mins(n));

    let first = teacher_with_ridge("pack-a", "0.1.0", 1e-6)?;
    let second = teacher_with_ridge("pack-a", "0.2.0", 1.0)?;
    let other = teacher("pack-b", "0.1.0")?;
    let unaccepted = teacher("pack-c", "0.1.0")?;
    let first_artifact = InTreeProvider::pack(&first)?;
    let second_artifact = InTreeProvider::pack(&second)?;
    let other_artifact = InTreeProvider::pack(&other)?;
    assert_ne!(
        first_artifact.digest, second_artifact.digest,
        "premise: the two versions are different bytes"
    );
    let student = DistilledModel::linear("pack-a", 0.1, vec![0.5, -0.25])?;
    let next_student = DistilledModel::linear("pack-a", 0.1, vec![0.4, -0.2])?;

    let mut registry = ModelRegistry::new();
    let first_reference = registered(&mut registry, &first)?;
    let second_reference = registered(&mut registry, &second)?;
    let other_reference = registered(&mut registry, &other)?;
    // The unaccepted model: a real fit held to a bar no fit clears, so its
    // card carries an evaluation that did not pass — no acceptance record.
    let impossible = SkillPolicy {
        minimum_holdout_r2: 2.0,
        ..SkillPolicy::default()
    };
    let rejected = register_fit(
        &mut registry,
        &unaccepted,
        &impossible,
        "serving-tests",
        start(),
    )?;
    assert!(
        !rejected.passed,
        "premise: the fit did not clear its bar, so its card holds no acceptance"
    );

    let member = |entries: &[(&String, &String)]| -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(reference, digest)| ((*reference).clone(), (*digest).clone()))
            .collect()
    };
    let names = |entries: &[&String]| -> std::collections::BTreeSet<String> {
        entries.iter().map(|entry| (*entry).clone()).collect()
    };
    // What the promoted set holds after each change, written from what this
    // test promoted and not read back from the platform.
    let expected = [
        member(&[(&first_reference, &first_artifact.digest)]),
        member(&[
            (&first_reference, &first_artifact.digest),
            (&other_reference, &other_artifact.digest),
        ]),
        member(&[
            (&second_reference, &second_artifact.digest),
            (&other_reference, &other_artifact.digest),
        ]),
        member(&[
            (&first_reference, &first_artifact.digest),
            (&other_reference, &other_artifact.digest),
        ]),
    ];

    let lineage;
    {
        let mut platform = platform_serving(PlatformConfig::default().with_event_log_file(&path))?;
        // Premise: no pack exists before a promotion, so every pack below
        // was made by one.
        assert!(platform.model_pack().is_none());
        assert!(platform.model_pack_lineage()?.is_empty());

        platform.promote_model(
            &mut registry,
            &first_artifact,
            Some(&student),
            &[],
            minute(0),
        )?;
        platform.promote_model(&mut registry, &other_artifact, None, &[], minute(1))?;
        platform.promote_model(
            &mut registry,
            &second_artifact,
            Some(&next_student),
            std::slice::from_ref(&first_reference),
            minute(2),
        )?;

        // A model with no acceptance record is refused and makes no pack.
        let before = platform.model_pack().cloned();
        let refused = platform
            .promote_model(
                &mut registry,
                &InTreeProvider::pack(&unaccepted)?,
                None,
                &[],
                minute(3),
            )
            .expect_err("a model with no passing evaluation joined a pack");
        assert!(
            refused.message().contains("without a passing evaluation"),
            "{refused}"
        );
        assert_eq!(platform.model_pack().cloned(), before);
        assert_eq!(platform.model_pack_lineage()?.len(), 3);

        // A rollback changes the set too, so it is a pack like any other.
        platform.rollback_model(&mut registry, &second_reference, minute(4))?;

        lineage = platform.model_pack_lineage()?;
        assert_eq!(lineage.len(), 4, "one pack per record that changed the set");

        // Each pack names the one before it, and the first names none.
        assert_eq!(lineage[0].predecessor, None);
        for pair in lineage.windows(2) {
            assert_eq!(pair[1].predecessor.as_ref(), Some(&pair[0].id));
        }
        // The second and fourth packs hold the same members and are
        // different packs: an id names a membership *and* its history.
        assert_eq!(expected[1], expected[3]);
        assert_ne!(lineage[1].id, lineage[3].id);

        // The delta of each, exactly.
        assert_eq!(
            lineage[0].added,
            member(&[(&first_reference, &first_artifact.digest)])
        );
        assert!(lineage[0].kept.is_empty() && lineage[0].removed.is_empty());

        assert_eq!(
            lineage[1].added,
            member(&[(&other_reference, &other_artifact.digest)])
        );
        assert_eq!(lineage[1].kept, names(&[&first_reference]));
        assert!(lineage[1].removed.is_empty());

        assert_eq!(
            lineage[2].added,
            member(&[(&second_reference, &second_artifact.digest)])
        );
        assert_eq!(lineage[2].kept, names(&[&other_reference]));
        assert_eq!(lineage[2].removed, names(&[&first_reference]));

        assert_eq!(
            lineage[3].added,
            member(&[(&first_reference, &first_artifact.digest)])
        );
        assert_eq!(lineage[3].kept, names(&[&other_reference]));
        assert_eq!(lineage[3].removed, names(&[&second_reference]));

        // Walking the lineage rebuilds every pack's membership.
        for (pack, members) in lineage.iter().zip(&expected) {
            assert_eq!(&pack_membership(&lineage, &pack.id)?, members);
        }

        // The pack in force is the lineage's newest, it is the promoted set
        // the platform ships from, and the shipping line names it.
        let head = platform
            .model_pack()
            .cloned()
            .ok_or_else(|| Error::not_found("the pack in force"))?;
        assert_eq!(Some(&head), lineage.last());
        let held: BTreeMap<String, String> = platform
            .model_promotions()
            .iter()
            .map(|(reference, record)| (reference.clone(), record.artifact_digest.clone()))
            .collect();
        assert_eq!(pack_membership(&lineage, &head.id)?, held);
        let issue = platform.model_manifest()?;
        assert_eq!(issue.pack(), Some(&head));
        let line = issue.describe();
        assert!(
            line.contains(&format!(
                "model pack {} supersedes {} (+1 added, 1 kept, -1 removed)",
                &head.id[..12],
                &lineage[2].id[..12]
            )),
            "the shipping line does not name the pack and its predecessor: {line}"
        );
    }

    // The lineage is a projection of the log: a restart holds the same pack
    // and reads the same lineage, having promoted nothing itself.
    let mut restart_config = PlatformConfig::default().with_event_log_file(&path);
    restart_config.seed ^= 1;
    let restarted = platform_serving(restart_config)?;
    assert_eq!(restarted.model_pack(), lineage.last());
    assert_eq!(restarted.model_pack_lineage()?, lineage);

    // A lineage changed after the fact is refused rather than walked. Three
    // edits, each caught by a different check.
    let newest = &lineage[3].id;
    // (1) A member's bytes swapped: the deltas still line up, the id does not.
    let mut swapped = lineage.clone();
    swapped[1]
        .added
        .insert(other_reference.clone(), second_artifact.digest.clone());
    let error = pack_membership(&swapped, newest).expect_err("a swapped digest was walked");
    assert!(
        error.message().contains("does not digest to its own id"),
        "{error}"
    );
    // (2) A pack claiming to keep a version its predecessor never held.
    let mut padded = lineage.clone();
    padded[2].kept.insert("pack-z@9.9.9".to_string());
    let error = pack_membership(&padded, newest).expect_err("a padded delta was walked");
    assert!(error.message().contains("says it keeps"), "{error}");
    // (3) A pack missing from the middle: the pointer leads nowhere.
    let mut gapped = lineage.clone();
    gapped.remove(1);
    let error = pack_membership(&gapped, newest).expect_err("a gapped lineage was walked");
    assert!(error.message().contains("holds no model pack"), "{error}");

    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

// --- who moved the production alias, and on what evidence (MODEL-057) -------

#[test]
fn moving_the_production_alias_records_the_desk_that_moved_it_and_the_evidence_in_the_registry_and_the_log()
-> Result<()> {
    // A promoted card said when it was deployed and nothing about who
    // decided or what they were looking at, and the promotion record named
    // no mover either: an alias found on the wrong version could be traced
    // to nobody. Every move below — a first promotion, a displacement, a
    // rollback — must name the desk and the evidence, on the card and on the
    // journalled record alike.
    use qip_ai::registry::PRODUCTION_ALIAS;
    let mut platform = platform_serving(PlatformConfig::default())?;
    assert_eq!(platform.model_desk(), Some(DESK));
    let mut registry = ModelRegistry::new();
    let old = teacher_with_ridge("alias-a", "0.1.0", 1e-6)?;
    let new = teacher_with_ridge("alias-a", "0.2.0", 1.0)?;
    let old_reference = registered(&mut registry, &old)?;
    let new_reference = registered(&mut registry, &new)?;
    let card = |registry: &ModelRegistry, reference: &str| -> Result<ModelCard> {
        registry
            .get(reference)
            .cloned()
            .ok_or_else(|| Error::not_found("the card"))
    };
    let last_move = |registry: &ModelRegistry, reference: &str| {
        card(registry, reference)?
            .alias_moves
            .last()
            .cloned()
            .ok_or_else(|| Error::not_found("an alias move on the card"))
    };

    // Premise: nothing holds the alias and no move is on record, so every
    // move read below was written by the act the test performs.
    assert!(registry.aliases(&old_reference)?.is_empty());
    assert!(card(&registry, &old_reference)?.alias_moves.is_empty());
    assert!(card(&registry, &new_reference)?.alias_moves.is_empty());

    // --- the first promotion -------------------------------------------------
    platform.promote_model(
        &mut registry,
        &InTreeProvider::pack(&old)?,
        None,
        &[],
        start(),
    )?;
    // For a promoted model the registry returns its version, its evaluation
    // results, its lineage, its current aliases and the gate's verdict.
    let promoted = card(&registry, &old_reference)?;
    assert_eq!(promoted.version, "0.1.0");
    let evaluation = promoted
        .latest_evaluation()
        .ok_or_else(|| Error::not_found("the evaluation"))?;
    assert!(evaluation.passed, "the gate's verdict");
    assert!(evaluation.metrics.contains_key("holdout_r2"));
    assert_eq!(promoted.training_datasets, vec!["alias-a-data".to_string()]);
    assert_eq!(registry.aliases(&old_reference)?, vec![PRODUCTION_ALIAS]);
    // And the move itself: who, on what evidence, when.
    let moved = last_move(&registry, &old_reference)?;
    assert_eq!(moved.alias, PRODUCTION_ALIAS);
    assert!(moved.assigned);
    assert_eq!(moved.moved_by, DESK);
    assert_eq!(moved.at, start());
    assert!(
        moved.evidence.contains("alias-a-data")
            && moved.evidence.contains("passed")
            && moved.evidence.contains("holdout_r2="),
        "the evidence does not name the evaluation the move rested on: {}",
        moved.evidence
    );
    let record = platform
        .model_promotions()
        .get(&old_reference)
        .cloned()
        .ok_or_else(|| Error::not_found("the promotion record"))?;
    assert_eq!(record.moved_by.as_deref(), Some(DESK));
    assert_eq!(record.evidence.as_deref(), Some(moved.evidence.as_str()));

    // --- the alias moves to a successor --------------------------------------
    let later = start().saturating_add(qip_core::Duration::from_mins(1));
    platform.promote_model(
        &mut registry,
        &InTreeProvider::pack(&new)?,
        None,
        std::slice::from_ref(&old_reference),
        later,
    )?;
    assert!(registry.aliases(&old_reference)?.is_empty());
    assert_eq!(registry.aliases(&new_reference)?, vec![PRODUCTION_ALIAS]);
    assert_eq!(
        card(&registry, &new_reference)?.rollback_parent.as_ref(),
        Some(&old_reference),
        "the successor's lineage names what it displaced"
    );
    let taken = last_move(&registry, &old_reference)?;
    assert!(!taken.assigned, "the alias left the displaced version");
    assert_eq!(taken.moved_by, DESK);
    assert_eq!(taken.at, later);
    assert!(
        taken.evidence.contains(&new_reference),
        "the removal does not say what displaced it: {}",
        taken.evidence
    );
    assert_eq!(
        card(&registry, &old_reference)?.alias_moves.len(),
        2,
        "on and off"
    );
    let given = last_move(&registry, &new_reference)?;
    assert!(given.assigned && given.moved_by == DESK && given.at == later);

    // --- a rollback moves it back, and says what reading it acted on ---------
    registry.record_drift(&new_reference, 0.9)?;
    let degraded = card(&registry, &new_reference)?;
    assert!(
        degraded.drift_score > degraded.drift_threshold,
        "premise: the successor has drifted past its threshold"
    );
    let rolled_at = later.saturating_add(qip_core::Duration::from_mins(1));
    platform.rollback_model(&mut registry, &new_reference, rolled_at)?;
    assert_eq!(registry.aliases(&old_reference)?, vec![PRODUCTION_ALIAS]);
    assert!(registry.aliases(&new_reference)?.is_empty());
    let returned = last_move(&registry, &old_reference)?;
    assert!(returned.assigned && returned.moved_by == DESK && returned.at == rolled_at);
    assert!(
        returned.evidence.contains("drift 0.900") && returned.evidence.contains("alias-a-data"),
        "the rollback does not name the drift reading and the evaluation it returned to: {}",
        returned.evidence
    );
    let retired = last_move(&registry, &new_reference)?;
    assert!(!retired.assigned && retired.moved_by == DESK && retired.at == rolled_at);
    let record = platform
        .model_promotions()
        .get(&old_reference)
        .cloned()
        .ok_or_else(|| Error::not_found("the rollback record"))?;
    assert_eq!(record.moved_by.as_deref(), Some(DESK));
    assert_eq!(record.evidence.as_deref(), Some(returned.evidence.as_str()));

    // --- a desk nobody named moves nothing ------------------------------------
    let mut unnamed = platform_serving_with_no_desk_named(PlatformConfig::default())?;
    assert_eq!(unnamed.model_desk(), None, "premise: no desk is named");
    let mut elsewhere = ModelRegistry::new();
    let orphan = teacher("alias-b", "0.1.0")?;
    let orphan_reference = registered(&mut elsewhere, &orphan)?;
    let artifact = InTreeProvider::pack(&orphan)?;
    let refused = unnamed
        .promote_model(&mut elsewhere, &artifact, None, &[], start())
        .expect_err("a platform with no named desk promoted a model");
    assert!(
        refused.message().contains("name_model_desk"),
        "the refusal does not say what to do: {refused}"
    );
    let untouched = card(&elsewhere, &orphan_reference)?;
    assert_eq!(untouched.stage, ModelStage::Development);
    assert!(untouched.alias_moves.is_empty());
    assert_eq!(promotion_records(&unnamed), 0, "a refusal reached the log");
    // A blank name is no name, and a second name is refused once one stands.
    assert!(unnamed.name_model_desk("  ").is_err());
    assert_eq!(unnamed.model_desk(), None);
    unnamed.name_model_desk(DESK)?;
    unnamed.name_model_desk(DESK)?;
    assert!(unnamed.name_model_desk("another-desk").is_err());
    assert_eq!(unnamed.model_desk(), Some(DESK));
    // Named, the very same promotion goes through — the refusal above was
    // about the missing name and nothing else.
    unnamed.promote_model(&mut elsewhere, &artifact, None, &[], start())?;
    assert_eq!(last_move(&elsewhere, &orphan_reference)?.moved_by, DESK);
    Ok(())
}
