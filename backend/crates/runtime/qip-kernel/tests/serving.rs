//! The promote and deploy stages of blueprint §21.2 as ADR 0083 §5 fixes
//! them: a model is promoted through the platform's provider, journalled
//! before the registry adopts it, named to the cells by the digest a cell
//! checks, and resumed from the log at the next assembly.
//!
//! Every test here builds a real fit through `LocalTrainer` and registers
//! it through `register_fit`, so the card's evaluation is the fit's own and
//! not a `passed: true` asserted by the test.

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

fn platform_serving(config: PlatformConfig) -> Result<Platform> {
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
