//! What survives when the model registry does not (blueprint §21.2,
//! R12-MODEL-10044, R12-MODEL-9003).
//!
//! The registry is an in-process `ModelRegistry` the deep brain rebuilds
//! empty at start, so "the registry is unavailable" is, concretely, a
//! registry that no longer holds the card a promotion names. Two properties
//! follow, and each had only an adjacent test: a promotion against such a
//! registry is refused with nothing changed while the package already
//! promoted keeps being issued, and what a restarted process does know —
//! every promoted version, the evaluation evidence it was moved on and the
//! alias history — comes back from the log alone.

#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::ModelRegistry;
use qip_core::error::{Error, Result};
use qip_core::{Context, Timestamp};
use qip_events::Topic;
use qip_financial::universe::Universe;
use qip_kernel::central::models::register_fit;
use qip_kernel::model_serving::{MODEL_PROMOTION_ORIGIN, ModelPromotion};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_strategy::model::DistilledModel;
use qip_streaming::envelope::StreamEnvelope;
use qip_training::dataset::TrainingDataset;
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, SkillPolicy, TrainedTeacher};
use qip_training::serve::InTreeProvider;

const DESK: &str = "promotion-durability/model-desk";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform(config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new_serving(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
        Box::new(InTreeProvider),
    )?;
    platform.name_model_desk(DESK)?;
    Ok(platform)
}

fn teacher(name: &str, version: &str, ridge: f64) -> Result<TrainedTeacher> {
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
        version,
        "promotion-durability",
        data.name(),
        ModelFamily::Linear { ridge },
    )
    .with_holdout(0.25);
    LocalTrainer::new().fit(&spec, &data, start())
}

fn registered(registry: &mut ModelRegistry, teacher: &TrainedTeacher) -> Result<String> {
    let registration = register_fit(
        registry,
        teacher,
        &SkillPolicy::default(),
        "promotion-durability",
        start(),
    )?;
    assert!(
        registration.passed,
        "premise: the fit cleared the skill bar"
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

/// Every promotion the log holds, in log order, decoded from the log itself
/// and not from the process's projection of it.
fn promotion_history(platform: &Platform) -> Result<Vec<ModelPromotion>> {
    let mut history = Vec::new();
    for record in platform.event_log().records() {
        if record.event.topic == Topic::ModelEvaluated
            && record.event.lineage.producer == MODEL_PROMOTION_ORIGIN
        {
            history.push(
                StreamEnvelope::from_frame(&record.event)?
                    .decode::<ModelPromotion>()?
                    .body,
            );
        }
    }
    Ok(history)
}

/// R12-MODEL-10044. A registry that has lost the candidate's card — the
/// state a restarted deep brain's empty registry is in — refuses the
/// promotion before the log, the promoted set or the manifest move, and the
/// model already promoted is still the one issued to the cells. Once the
/// registry holds the card again the same promotion goes through, so the
/// refusal paused promotion rather than wedging it.
#[test]
fn a_promotion_against_a_registry_without_its_card_pauses_while_the_promoted_package_is_still_issued()
-> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let mut registry = ModelRegistry::new();
    let incumbent = teacher("outage-a", "0.1.0", 1e-6)?;
    let incumbent_reference = registered(&mut registry, &incumbent)?;
    // A distillate, so the manifest the cells read names a package.
    let student = DistilledModel::linear("outage-a", 0.1, vec![0.5, -0.25])?;
    platform.promote_model(
        &mut registry,
        &InTreeProvider::pack(&incumbent)?,
        Some(&student),
        &[],
        start(),
    )?;
    let manifest_before = platform.model_manifest()?.manifest().clone();
    let promotions_before = platform.model_promotions().clone();
    assert_eq!(promotion_records(&platform), 1, "premise: one promotion");
    assert!(
        promotions_before.contains_key(&incumbent_reference),
        "premise: the incumbent is promoted"
    );

    let candidate = teacher("outage-b", "0.1.0", 1e-6)?;
    let candidate_artifact = InTreeProvider::pack(&candidate)?;
    let mut unavailable = ModelRegistry::new();
    let later = start().saturating_add(qip_core::Duration::from_mins(1));
    let refused = platform
        .promote_model(&mut unavailable, &candidate_artifact, None, &[], later)
        .expect_err("a registry without the card cannot promote it");
    assert!(matches!(refused, Error::NotFound(_)), "{refused}");

    assert_eq!(
        promotion_records(&platform),
        1,
        "a refused promotion wrote a record"
    );
    assert_eq!(platform.model_promotions(), &promotions_before);
    assert_eq!(
        platform.model_manifest()?.manifest(),
        &manifest_before,
        "the package already promoted stopped being issued"
    );

    let candidate_reference = registered(&mut registry, &candidate)?;
    platform.promote_model(&mut registry, &candidate_artifact, None, &[], later)?;
    assert_eq!(promotion_records(&platform), 2);
    assert!(
        platform
            .model_promotions()
            .contains_key(&candidate_reference)
    );
    assert!(
        platform
            .model_promotions()
            .contains_key(&incumbent_reference)
    );
    Ok(())
}

/// R12-MODEL-9003. Two promotions — the second displacing the first — read
/// back by a second process over the first's file: both versions, the
/// evaluation evidence each alias move rested on, the desk that moved it and
/// the displacement are rebuilt from the log alone, with no registry at all.
#[test]
fn a_restarted_platform_rebuilds_every_promoted_version_with_its_evaluation_and_alias_history_from_the_log()
-> Result<()> {
    let directory = std::env::temp_dir().join(format!(
        "qip-kernel-promotion-durability-{}-{}",
        std::process::id(),
        start().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    let path = directory.join("events.jsonl");
    let old = teacher("durable-a", "0.1.0", 1e-6)?;
    let new = teacher("durable-a", "0.2.0", 1.0)?;
    let first_promotions;
    let old_reference;
    let new_reference;
    {
        let mut first = platform(PlatformConfig::default().with_event_log_file(&path))?;
        let mut registry = ModelRegistry::new();
        old_reference = registered(&mut registry, &old)?;
        new_reference = registered(&mut registry, &new)?;
        first.promote_model(
            &mut registry,
            &InTreeProvider::pack(&old)?,
            None,
            &[],
            start(),
        )?;
        first.promote_model(
            &mut registry,
            &InTreeProvider::pack(&new)?,
            None,
            std::slice::from_ref(&old_reference),
            start().saturating_add(qip_core::Duration::from_mins(1)),
        )?;
        first_promotions = first.model_promotions().clone();
    }
    let second = platform(PlatformConfig::default().with_event_log_file(&path))?;
    assert_eq!(
        promotion_records(&second),
        2,
        "premise: the second process read both promotions back"
    );
    assert_eq!(second.model_promotions(), &first_promotions);

    let history = promotion_history(&second)?;
    let versions: Vec<&str> = history
        .iter()
        .map(|record| record.reference.as_str())
        .collect();
    assert_eq!(
        versions,
        vec![old_reference.as_str(), new_reference.as_str()]
    );
    for record in &history {
        assert_eq!(record.moved_by.as_deref(), Some(DESK));
        let evidence = record.evidence.as_deref().unwrap_or_default();
        assert!(
            evidence.contains("passed") && evidence.contains("holdout_r2="),
            "{} came back without the evaluation it was promoted on: {evidence:?}",
            record.reference
        );
    }
    assert_eq!(history[1].displaced, vec![old_reference.clone()]);
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

/// R12-MODEL-9003: a registered card is journaled when it is new and again
/// when something the registry must keep changes, and not for a drift
/// reading alone. The deep brain re-measures drift on every earlier fit of a
/// subject each round, so a record per reading would grow the never-evicted
/// Learn group with the square of the rounds. Mutation: compare whole cards
/// in `Platform::journal_model_registry` (drop `same_but_drift`) — the drift
/// change is written and the second count fails.
#[test]
fn a_card_is_journaled_when_registered_and_on_a_kept_change_but_not_on_drift_alone() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let mut registry = ModelRegistry::new();
    let reference = registered(&mut registry, &teacher("journal-a", "0.1.0", 1e-6)?)?;
    assert_eq!(platform.journal_model_registry(&registry, start())?, 1);
    assert_eq!(
        platform.journal_model_registry(&registry, start())?,
        0,
        "premise: an unchanged card is not written twice"
    );

    registry.record_drift(&reference, 0.42)?;
    assert_eq!(
        platform.journal_model_registry(&registry, start())?,
        0,
        "a drift reading alone was journaled"
    );

    registry.retire(&reference, start())?;
    assert_eq!(
        platform.journal_model_registry(&registry, start())?,
        1,
        "a stage move was not journaled"
    );
    let card = platform
        .registered_model_cards()
        .find(|card| card.reference() == reference)
        .ok_or_else(|| Error::not_found("the journaled card"))?;
    assert_eq!(card.stage, qip_ai::registry::ModelStage::Retired);
    Ok(())
}
