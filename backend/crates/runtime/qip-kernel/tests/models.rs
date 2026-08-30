//! Fitted models reaching the registry, with their holdout verdict attached.
//!
//! The failure these are built against is the one where every component is
//! behaving exactly as designed. `qip-training` refuses to call a fit skilful
//! without an out-of-sample result; `qip_ai::registry` refuses to let a model
//! inform a decision without a passed evaluation. Both correct, and between
//! them a gap: nothing obliged the evaluation on the card to be the verdict of
//! the fit it describes. Write `passed: true` by hand and a noise model is
//! decision-eligible.
//!
//! So the load-bearing test here is the one that fits a model on pure noise
//! and checks it cannot come out the other side claiming skill.

#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::{ModelRegistry, ModelStage};
use qip_core::error::Result;
use qip_core::{Rng, Timestamp, Xoshiro256};
use qip_kernel::central::models::register_fit;
use qip_training::cadence::{TrainingCadence, UpdatePayload};
use qip_training::dataset::TrainingDataset;
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, SkillPolicy, TeacherForm, TrainedTeacher};

const OBSERVATIONS: usize = 400;

fn now() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn at(index: usize) -> Timestamp {
    Timestamp::from_secs(1_700_000_000 + index as i64 * 86_400)
}

/// A dataset whose target is a real linear function of its features, plus a
/// little noise. A fit should find this.
fn signal_dataset() -> Result<TrainingDataset> {
    let mut rng = Xoshiro256::seeded(17);
    let mut rows = Vec::with_capacity(OBSERVATIONS);
    let mut targets = Vec::with_capacity(OBSERVATIONS);
    let mut times = Vec::with_capacity(OBSERVATIONS);
    for index in 0..OBSERVATIONS {
        let a = rng.next_f64() * 2.0 - 1.0;
        let b = rng.next_f64() * 2.0 - 1.0;
        let noise = (rng.next_f64() * 2.0 - 1.0) * 0.1;
        rows.push(vec![a, b]);
        targets.push(0.8 * a - 0.5 * b + noise);
        times.push(at(index));
    }
    TrainingDataset::new(
        "signal-set",
        vec!["momentum".to_string(), "imbalance".to_string()],
        rows,
        targets,
        times,
    )
}

/// A dataset whose target has nothing to do with its features. A fit will
/// still reduce training error on this — that is exactly the problem.
fn noise_dataset() -> Result<TrainingDataset> {
    let mut rng = Xoshiro256::seeded(29);
    let mut rows = Vec::with_capacity(OBSERVATIONS);
    let mut targets = Vec::with_capacity(OBSERVATIONS);
    let mut times = Vec::with_capacity(OBSERVATIONS);
    for index in 0..OBSERVATIONS {
        rows.push(vec![
            rng.next_f64() * 2.0 - 1.0,
            rng.next_f64() * 2.0 - 1.0,
            rng.next_f64() * 2.0 - 1.0,
        ]);
        targets.push(rng.next_f64() * 2.0 - 1.0);
        times.push(at(index));
    }
    TrainingDataset::new(
        "noise-set",
        vec![
            "spurious-a".to_string(),
            "spurious-b".to_string(),
            "spurious-c".to_string(),
        ],
        rows,
        targets,
        times,
    )
}

/// Boosted stumps, deliberately: this family will drive training error to
/// nothing on the noise set, which is what makes it the honest adversary.
fn boosted(name: &str, dataset: &str) -> TrainingSpec {
    TrainingSpec::new(
        name,
        "1.0.0",
        "research",
        dataset,
        ModelFamily::BoostedStumps {
            // Deliberately high capacity: depth-one stumps are weak learners, and
            // the point of this family here is to be able to memorise a training
            // set. A gentler configuration would fail the skill bar because it
            // learned nothing at all, which would pass the test below for the
            // wrong reason entirely.
            rounds: 800,
            learning_rate: 0.3,
            min_samples_leaf: 1,
            candidate_splits: 64,
        },
    )
}

fn fit(spec: &TrainingSpec, data: &TrainingDataset) -> Result<TrainedTeacher> {
    LocalTrainer::new().fit(spec, data, now())
}

// --- the one that matters ---------------------------------------------------

#[test]
fn a_model_fitted_on_noise_cannot_be_registered_as_having_skill() -> Result<()> {
    let data = noise_dataset()?;
    let teacher = fit(&boosted("noise-model", "noise-set"), &data)?;
    let diagnostics = teacher
        .fit()
        .ok_or_else(|| qip_core::error::Error::not_found("fit diagnostics"))?;

    // The premise of the test: this fit really did memorise its training set.
    // If it had not, the test would be passing for the wrong reason.
    // The signature this test exists to catch, asserted as a gap rather than
    // an absolute level: the fit explains a real share of its *training*
    // variance while doing worse than the mean out of sample. A single
    // threshold on either number alone would pass for the wrong reason — a fit
    // that simply learned nothing also fails the skill bar, and would prove
    // nothing about whether the seam works.
    assert!(
        diagnostics.in_sample_r2 > 0.2,
        "the fit did not memorise its training set (in-sample R² {:.3}), so this test is not \
         exercising the failure it was written for",
        diagnostics.in_sample_r2
    );
    assert!(
        diagnostics.holdout_r2 <= 0.0,
        "the noise set produced positive out-of-sample R² ({:.3}); the data is not noise",
        diagnostics.holdout_r2
    );
    assert!(
        diagnostics.in_sample_r2 - diagnostics.holdout_r2 > 0.25,
        "in-sample {:.3} and holdout {:.3} are too close to demonstrate overfitting",
        diagnostics.in_sample_r2,
        diagnostics.holdout_r2
    );

    let mut registry = ModelRegistry::new();
    let registration = register_fit(
        &mut registry,
        &teacher,
        &SkillPolicy::default(),
        "research",
        now(),
    )?;

    assert!(
        !registration.passed,
        "a model fitted on pure noise was registered as having cleared the skill bar"
    );
    let verdict = registration
        .verdict
        .as_deref()
        .expect("a failing registration states why");
    assert!(
        verdict.contains("holdout"),
        "the verdict does not point at the out-of-sample result: {verdict}"
    );

    // And the registry agrees: whatever else is true of this card, it may not
    // inform a decision.
    let card = registry
        .get(&registration.reference)
        .ok_or_else(|| qip_core::error::Error::not_found("the registered card"))?;
    assert!(
        card.decision_eligibility(now()).is_err(),
        "a card built from a skill-less fit was eligible to inform decisions"
    );

    // The failure is recorded rather than hidden. Deleting it would let a
    // search retry until something passed with none of the attempts visible.
    assert_eq!(registry.len(), 1, "the failing model was not kept");
    assert!(
        card.limitations
            .iter()
            .any(|note: &String| note.contains("skill bar")),
        "the card does not carry the reason it failed: {:?}",
        card.limitations
    );
    Ok(())
}

#[test]
fn a_model_with_real_out_of_sample_skill_is_registered_as_having_it() -> Result<()> {
    // The other half. A gate that refused everything would pass the test above
    // and be worthless.
    let data = signal_dataset()?;
    let teacher = fit(&boosted("signal-model", "signal-set"), &data)?;

    let mut registry = ModelRegistry::new();
    let registration = register_fit(
        &mut registry,
        &teacher,
        &SkillPolicy::default(),
        "research",
        now(),
    )?;

    assert!(
        registration.passed,
        "a model that genuinely predicts its target was refused: {:?}",
        registration.verdict
    );
    assert!(registration.verdict.is_none());

    let card = registry
        .get(&registration.reference)
        .ok_or_else(|| qip_core::error::Error::not_found("the registered card"))?;
    let evaluation = card
        .latest_evaluation()
        .ok_or_else(|| qip_core::error::Error::not_found("the evaluation"))?;
    assert!(evaluation.passed);
    assert!(
        evaluation
            .metrics
            .get("holdout_r2")
            .is_some_and(|r2| *r2 > 0.05),
        "the recorded holdout R² does not support the pass: {:?}",
        evaluation.metrics
    );
    Ok(())
}

// --- what the card carries --------------------------------------------------

#[test]
fn the_evaluation_records_both_sides_so_a_reviewer_can_see_the_gap() -> Result<()> {
    // A large in-sample R² beside a negative holdout one is the signature of a
    // memorised training set. Recording only the holdout figure would hide the
    // most legible evidence of what went wrong.
    let data = noise_dataset()?;
    let teacher = fit(&boosted("noise-model", "noise-set"), &data)?;
    let mut registry = ModelRegistry::new();
    let registration = register_fit(
        &mut registry,
        &teacher,
        &SkillPolicy::default(),
        "research",
        now(),
    )?;

    let card = registry
        .get(&registration.reference)
        .ok_or_else(|| qip_core::error::Error::not_found("the card"))?;
    let metrics = &card
        .latest_evaluation()
        .ok_or_else(|| qip_core::error::Error::not_found("the evaluation"))?
        .metrics;

    for key in [
        "holdout_r2",
        "holdout_rmse",
        "holdout_correlation",
        "in_sample_r2",
        "in_sample_rmse",
        "holdout_observations",
    ] {
        assert!(metrics.contains_key(key), "the evaluation omits {key}");
    }
    assert!(
        metrics["in_sample_r2"] > metrics["holdout_r2"],
        "the noise fit does not show the in-sample/holdout gap this test describes"
    );

    // The card says what it was fitted on and with which features, so the
    // dataset behind a decision is recoverable from the registry alone.
    assert_eq!(card.training_datasets, vec!["noise-set".to_string()]);
    assert_eq!(card.features.len(), 3);
    assert_eq!(
        card.stage,
        ModelStage::Development,
        "a fresh fit entered above development"
    );
    Ok(())
}

#[test]
fn a_teacher_whose_structure_was_replaced_is_refused_rather_than_registered_unevaluated()
-> Result<()> {
    // Replacing a model's functional form clears its diagnostics, so that a
    // new function cannot inherit the evaluation of the one it replaced.
    // Registering it anyway would produce a card whose later refusal cited a
    // missing evaluation rather than the real problem.
    let data = signal_dataset()?;
    let mut teacher = fit(&boosted("replaced-model", "signal-set"), &data)?;

    // Through the governed path, which is the only one there is: the form is
    // private and an AuthorisedUpdate cannot be built for a scope its cadence
    // does not permit, so a structural change must come from the slowest
    // cadence — the one a person reviews.
    let update = TrainingCadence::DailyToWeekly.authorise(
        UpdatePayload::Structure(TeacherForm::Linear {
            intercept: 0.0,
            coefficients: vec![0.0, 0.0],
        }),
        now(),
        None,
        "a different functional form",
    )?;
    teacher.apply(&update)?;
    assert!(
        teacher.fit().is_none(),
        "replacing the form kept the diagnostics"
    );

    let mut registry = ModelRegistry::new();
    let error = register_fit(
        &mut registry,
        &teacher,
        &SkillPolicy::default(),
        "research",
        now(),
    )
    .expect_err("a teacher with no diagnostics was registered");
    assert!(
        error.message().contains("refitted"),
        "the refusal does not explain that the function changed: {}",
        error.message()
    );
    assert_eq!(registry.len(), 0);
    Ok(())
}

#[test]
fn a_stricter_policy_refuses_what_the_default_admits() -> Result<()> {
    // The bar is a parameter, not a constant, and the seam applies whichever
    // one it is given rather than a policy of its own.
    let data = signal_dataset()?;
    let teacher = fit(&boosted("signal-model", "signal-set"), &data)?;

    let mut lenient = ModelRegistry::new();
    assert!(
        register_fit(
            &mut lenient,
            &teacher,
            &SkillPolicy::default(),
            "research",
            now()
        )?
        .passed
    );

    let mut strict = ModelRegistry::new();
    let registration = register_fit(
        &mut strict,
        &teacher,
        &SkillPolicy {
            minimum_holdout_r2: 0.999,
            minimum_holdout_observations: 30,
        },
        "research",
        now(),
    )?;
    assert!(
        !registration.passed,
        "a policy demanding an implausible R² still admitted the fit"
    );
    Ok(())
}
