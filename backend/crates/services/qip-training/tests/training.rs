//! Tests for model training, distillation and the managed-training port.
//!
//! The assertions worth having here are not the ones that re-state what the
//! code does. They are the ones that would catch a fit reporting its training
//! error as skill, a fast update quietly replacing a model's structure, a
//! student secretly fitted to the observed labels rather than to its teacher,
//! and a provider that cannot reach Vertex AI reporting that it trained
//! something anyway.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::{Property, approx_eq};
use qip_core::time::{Duration, Timestamp};
use qip_quant::signal::Horizon;
use qip_strategy::model::ModelForm;
use qip_training::cadence::{TrainingCadence, UpdatePayload, UpdateScope};
use qip_training::dataset::TrainingDataset;
use qip_training::distill::{Distillation, FidelityPolicy, StudentForm, distil};
use qip_training::job::{
    JobState, LocalTrainingProvider, TrainingProvider, TrainingSpec, run_to_completion,
};
use qip_training::local::{
    Calibration, LocalTrainer, ModelFamily, SkillPolicy, TeacherForm, TrainedTeacher,
};
use qip_training::vertex::{
    VertexAiConfig, VertexAiProvider, VertexWorkload, WorkloadIdentityBinding,
};
use std::collections::BTreeMap;

// --- fixtures ---------------------------------------------------------------

/// Whole days, so a timestamp survives every serialization in these tests and
/// the assertion measures the record rather than the encoding.
fn day(n: i64) -> Timestamp {
    Timestamp::from_civil(2025, 1, 1).saturating_add(Duration::from_days(n))
}

/// The instant every fit in these tests is stamped with. Passed in everywhere;
/// nothing in the crate is allowed to read a clock of its own.
fn fitted_at() -> Timestamp {
    day(100)
}

fn names(arity: usize) -> Vec<String> {
    (0..arity).map(|i| format!("feature_{i}")).collect()
}

/// A dataset whose target is whatever `label` computes from the row, ordered in
/// time, one observation a day.
fn dataset_of(
    name: &str,
    rows_wanted: usize,
    arity: usize,
    seed: u64,
    mut label: impl FnMut(&[f64], &mut Xoshiro256) -> f64,
) -> Result<TrainingDataset> {
    let mut rng = Xoshiro256::seeded(seed);
    let mut rows = Vec::with_capacity(rows_wanted);
    let mut targets = Vec::with_capacity(rows_wanted);
    let mut times = Vec::with_capacity(rows_wanted);
    for index in 0..rows_wanted {
        let row: Vec<f64> = (0..arity).map(|_| rng.uniform(-1.0, 1.0)).collect();
        targets.push(label(&row, &mut rng));
        rows.push(row);
        times.push(day(index as i64));
    }
    TrainingDataset::new(name, names(arity), rows, targets, times)
}

/// `intercept + Σ cᵢxᵢ`, exactly. Used where the point is that a fit recovers
/// something known.
fn linear_dataset(
    name: &str,
    rows: usize,
    intercept: f64,
    coefficients: &[f64],
    noise: f64,
    seed: u64,
) -> Result<TrainingDataset> {
    let coefficients = coefficients.to_vec();
    dataset_of(name, rows, coefficients.len(), seed, move |row, rng| {
        let mut total = intercept;
        for (c, x) in coefficients.iter().zip(row) {
            total += c * x;
        }
        if noise > 0.0 {
            total += rng.normal_with(0.0, noise);
        }
        total
    })
}

/// A target no linear model can represent: high in the middle, low at both
/// ends. A single coefficient on `x₀` explains none of it.
fn humped_dataset(name: &str, rows: usize, seed: u64) -> Result<TrainingDataset> {
    dataset_of(name, rows, 2, seed, |row, rng| {
        let core = if row[0].abs() < 0.5 { 1.0 } else { -1.0 };
        core + rng.normal_with(0.0, 0.05)
    })
}

/// A target with no relationship to the features at all.
fn noise_dataset(name: &str, rows: usize, arity: usize, seed: u64) -> Result<TrainingDataset> {
    dataset_of(name, rows, arity, seed, |_, rng| rng.normal_with(0.0, 1.0))
}

fn spec(name: &str, family: ModelFamily) -> TrainingSpec {
    TrainingSpec::new(name, "v1", "the-model-owner", "fixture", family)
}

fn fit(family: ModelFamily, data: &TrainingDataset) -> Result<TrainedTeacher> {
    LocalTrainer::new().fit(&spec("fixture-model", family), data, fitted_at())
}

const ALL_SCOPES: [UpdateScope; 6] = [
    UpdateScope::Weights,
    UpdateScope::CalibrationConstants,
    UpdateScope::OnlineStatistics,
    UpdateScope::Hyperparameters,
    UpdateScope::FeatureSet,
    UpdateScope::Structure,
];

const ALL_CADENCES: [TrainingCadence; 4] = [
    TrainingCadence::Millisecond,
    TrainingCadence::SecondsToMinutes,
    TrainingCadence::Hourly,
    TrainingCadence::DailyToWeekly,
];

/// A payload of each scope, sized for a one-feature linear model.
fn payload_of(scope: UpdateScope) -> UpdatePayload {
    match scope {
        UpdateScope::Weights => UpdatePayload::Weights(vec![1.0]),
        UpdateScope::CalibrationConstants => UpdatePayload::Calibration {
            scale: 1.0,
            offset: 0.0,
        },
        UpdateScope::OnlineStatistics => UpdatePayload::OnlineStatistics(BTreeMap::new()),
        UpdateScope::Hyperparameters => UpdatePayload::Hyperparameters(BTreeMap::new()),
        UpdateScope::FeatureSet => UpdatePayload::FeatureSet(vec!["feature_0".to_string()]),
        UpdateScope::Structure => UpdatePayload::Structure(TeacherForm::Linear {
            intercept: 0.0,
            coefficients: vec![1.0],
        }),
    }
}

fn vertex_config() -> VertexAiConfig {
    VertexAiConfig {
        project_id: "qip-research".to_string(),
        region: "europe-west4".to_string(),
        staging_bucket: "gs://qip-research-training".to_string(),
        workload: VertexWorkload::CustomContainer {
            image_uri: "europe-west4-docker.pkg.dev/qip-research/trainers/teacher:2025-08".into(),
            machine_type: "n1-standard-8".to_string(),
            accelerator: None,
        },
        workload_identity: WorkloadIdentityBinding {
            kubernetes_service_account: "qip-training".to_string(),
            google_service_account: "qip-training@qip-research.iam.gserviceaccount.com".to_string(),
            roles: vec!["roles/aiplatform.user".to_string()],
        },
    }
}

// --- the dataset boundary ---------------------------------------------------

#[test]
fn a_non_finite_feature_never_reaches_a_fit() {
    let bad = TrainingDataset::new(
        "with-a-nan",
        names(2),
        vec![vec![1.0, 2.0], vec![f64::NAN, 0.5]],
        vec![0.1, 0.2],
        vec![day(0), day(1)],
    );
    let error = bad.expect_err("a NaN feature must be refused at the boundary");
    assert_eq!(error.code(), "numeric");
    // The message has to name the column, because "invalid input" sends the
    // reader back to the data with nothing to look for.
    assert!(
        error.message().contains("feature_0"),
        "the refusal should name the offending feature: {}",
        error.message()
    );
}

#[test]
fn observations_out_of_time_order_are_refused() {
    let scrambled = TrainingDataset::new(
        "out-of-order",
        names(1),
        vec![vec![0.0], vec![1.0], vec![2.0]],
        vec![0.0, 1.0, 2.0],
        vec![day(0), day(9), day(4)],
    );
    assert!(
        scrambled.is_err(),
        "a series whose rows are not in time order cannot be purged or embargoed"
    );
}

#[test]
fn a_selection_that_reorders_time_is_refused() -> Result<()> {
    let data = linear_dataset("ordered", 40, 0.0, &[1.0], 0.0, 11)?;
    // A fold is a set of indices. Handing them over shuffled would build a
    // dataset whose timestamps run backwards, and every leakage control
    // downstream assumes they do not.
    assert!(data.select(&[5, 2, 9]).is_err());
    assert!(data.select(&[2, 5, 9]).is_ok());
    Ok(())
}

#[test]
fn ragged_rows_and_mismatched_columns_are_refused() {
    assert!(
        TrainingDataset::new(
            "ragged",
            names(2),
            vec![vec![1.0, 2.0], vec![3.0]],
            vec![0.0, 0.0],
            vec![day(0), day(1)],
        )
        .is_err()
    );
    assert!(
        TrainingDataset::new(
            "short-targets",
            names(1),
            vec![vec![1.0], vec![2.0]],
            vec![0.0],
            vec![day(0), day(1)],
        )
        .is_err()
    );
}

#[test]
fn every_split_is_a_partition_of_the_tail_from_the_head() {
    // The property, over many lengths and fractions at once: the two sides are
    // non-empty, they add up to the whole, and the holdout is the *tail* —
    // never a random subset, whose neighbours the fit has already seen.
    Property::new("split_at_fraction partitions by time")
        .cases(400)
        .for_all(
            |rng| {
                let rows = 2 + rng.below(60) as usize;
                let fraction = rng.uniform(0.0, 1.0);
                (rows, fraction)
            },
            |(rows, fraction)| {
                let data = linear_dataset("split", *rows, 0.0, &[1.0], 0.0, 7)
                    .map_err(|e| e.message().to_string())?;
                let (head, tail) = data
                    .split_at_fraction(*fraction)
                    .map_err(|e| format!("refused a legal fraction: {}", e.message()))?;
                if head.is_empty() || tail.is_empty() {
                    return Err("a split left one side empty".to_string());
                }
                if head.len() + tail.len() != data.len() {
                    return Err(format!(
                        "{} + {} rows is not the {} the dataset holds",
                        head.len(),
                        tail.len(),
                        data.len()
                    ));
                }
                if head.times().last() > tail.times().first() {
                    return Err("the holdout is not strictly after the fit set".to_string());
                }
                if tail.times() != &data.times()[head.len()..] {
                    return Err("the holdout is not the tail of the series".to_string());
                }
                Ok(())
            },
        );
}

#[test]
fn a_single_observation_cannot_be_split_and_says_so() -> Result<()> {
    let one_row = linear_dataset("lonely", 1, 0.0, &[1.0], 0.0, 3)?;
    // Both sides of a split have to be non-empty. The interesting part is that
    // this is an error rather than a panic: a fit is reachable from a training
    // job, and a job that aborts the process is not a job that failed.
    let error = one_row
        .split_at_fraction(0.25)
        .expect_err("one observation cannot be split");
    assert_eq!(error.code(), "invalid");
    assert!(fit(ModelFamily::Linear { ridge: 0.0 }, &one_row).is_err());
    Ok(())
}

#[test]
fn relabelling_keeps_the_inputs_and_only_moves_the_targets() -> Result<()> {
    let data = linear_dataset("original", 30, 0.5, &[1.0, -1.0], 0.0, 5)?;
    let relabelled = data.with_targets(vec![7.0; data.len()])?;
    assert_eq!(relabelled.rows(), data.rows());
    assert_eq!(relabelled.times(), data.times());
    assert_eq!(relabelled.feature_names(), data.feature_names());
    assert!(relabelled.targets().iter().all(|t| approx_eq(*t, 7.0, 0.0)));
    // A relabelled set is not the original set, and the name says so — a model
    // card naming the source data when it was fitted on soft targets is a
    // model card that misdescribes the fit.
    assert_ne!(relabelled.name(), data.name());
    Ok(())
}

// --- the fit ----------------------------------------------------------------

#[test]
fn least_squares_recovers_coefficients_it_was_generated_from() -> Result<()> {
    let truth = [1.5, -2.0, 0.25];
    let data = linear_dataset("noiseless", 200, 0.75, &truth, 0.0, 21)?;
    let teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &data)?;

    let TeacherForm::Linear {
        intercept,
        coefficients,
    } = teacher.form()
    else {
        panic!("a linear family must produce a linear form");
    };
    assert!(approx_eq(*intercept, 0.75, 1e-8), "intercept {intercept}");
    for (recovered, expected) in coefficients.iter().zip(truth) {
        assert!(
            approx_eq(*recovered, expected, 1e-8),
            "recovered {recovered} for a true coefficient of {expected}"
        );
    }

    // And the holdout agrees, which is the part that says the fit generalises
    // rather than that it memorised.
    let diagnostics = teacher.fit().expect("a fresh fit carries its diagnostics");
    assert!(
        diagnostics.holdout_r2 > 0.999,
        "{}",
        diagnostics.summarise()
    );
    Ok(())
}

#[test]
fn a_stronger_ridge_shrinks_the_coefficients_and_never_the_intercept() -> Result<()> {
    let data = linear_dataset("shrinkable", 200, 5.0, &[2.0, -3.0], 0.1, 33)?;

    let norm_at = |ridge: f64| -> Result<(f64, f64)> {
        let teacher = fit(ModelFamily::Linear { ridge }, &data)?;
        let TeacherForm::Linear {
            intercept,
            coefficients,
        } = teacher.form()
        else {
            panic!("a linear family must produce a linear form");
        };
        let norm = coefficients.iter().map(|c| c * c).sum::<f64>().sqrt();
        Ok((*intercept, norm))
    };

    let (_, none) = norm_at(0.0)?;
    let (_, some) = norm_at(1.0)?;
    let (heavy_intercept, heavy) = norm_at(1e6)?;
    assert!(
        none > some && some > heavy,
        "an L2 penalty must shrink the coefficients monotonically: {none} {some} {heavy}"
    );
    assert!(heavy < 0.01, "a huge penalty should flatten the slopes");

    // The intercept is deliberately outside the penalty. Were it inside,
    // a large ridge would drag every prediction towards zero instead of
    // towards the mean, and this assertion is what notices.
    assert!(
        approx_eq(heavy_intercept, 5.0, 0.05),
        "the intercept was shrunk to {heavy_intercept}; it must stay near the target's mean"
    );
    Ok(())
}

#[test]
fn boosted_stumps_learn_a_shape_a_line_cannot() -> Result<()> {
    let data = humped_dataset("humped", 500, 41)?;

    let linear = fit(ModelFamily::Linear { ridge: 0.0 }, &data)?;
    let boosted = fit(ModelFamily::boosted(), &data)?;

    let straight = linear.fit().expect("diagnostics").holdout_r2;
    let stumps = boosted.fit().expect("diagnostics").holdout_r2;
    assert!(
        straight < 0.10,
        "a line should explain almost none of a non-monotone target, got {straight}"
    );
    assert!(
        stumps > 0.60,
        "an ensemble of stumps should recover it, got {stumps}"
    );
    assert!(
        boosted
            .fit()
            .expect("diagnostics")
            .claims_skill(&SkillPolicy::default())
    );
    Ok(())
}

#[test]
fn a_fit_on_pure_noise_is_not_allowed_to_claim_skill() -> Result<()> {
    let data = noise_dataset("no-signal", 400, 4, 77)?;
    // Deliberately over-parameterised: enough rounds and enough freedom per
    // split to memorise its own training set. This is the failure mode the
    // holdout exists for, so the test has to actually produce it.
    let greedy = ModelFamily::BoostedStumps {
        rounds: 400,
        learning_rate: 0.5,
        min_samples_leaf: 1,
        candidate_splits: 32,
    };
    let teacher = fit(greedy, &data)?;
    let diagnostics = *teacher.fit().expect("diagnostics");

    // The sharp version of the claim: the in-sample R2 clears the very bar the
    // policy sets, so a verdict read off the training error would call this a
    // discovery. It is pure noise.
    let policy = SkillPolicy::default();
    assert!(
        diagnostics.in_sample_r2 > policy.minimum_holdout_r2,
        "the ensemble should have fitted its own training set past the bar: {}",
        diagnostics.summarise()
    );
    assert!(
        diagnostics.holdout_r2 < 0.0,
        "none of a fit on noise should survive out of sample: {}",
        diagnostics.summarise()
    );
    assert!(
        !diagnostics.claims_skill(&SkillPolicy::default()),
        "there is no signal in this data: {}",
        diagnostics.summarise()
    );
    let reason = diagnostics
        .skill_verdict(&SkillPolicy::default())
        .expect_err("a noise fit must be refused");
    assert!(
        reason.contains("holdout"),
        "the refusal must point at the holdout, not the training error: {reason}"
    );
    assert!(
        diagnostics.retention() < 0.5,
        "almost none of the in-sample fit should survive: {}",
        diagnostics.summarise()
    );

    // The bar is a bar and not a blanket refusal: the same policy passes a fit
    // on data that does carry a signal.
    let real = humped_dataset("has-signal", 400, 78)?;
    assert!(
        fit(greedy, &real)?
            .fit()
            .expect("diagnostics")
            .claims_skill(&SkillPolicy::default())
    );
    Ok(())
}

#[test]
fn a_fit_is_a_function_of_its_inputs_and_of_nothing_else() -> Result<()> {
    let data = linear_dataset("repeatable", 120, 1.0, &[0.5, 0.5], 0.2, 91)?;
    let family = ModelFamily::boosted();
    let first = fit(family, &data)?;
    let second = fit(family, &data)?;
    // No clock, no ambient randomness: the same specification over the same
    // data is the same model, or a model card records nothing reproducible.
    assert_eq!(first, second);
    assert_eq!(first.fitted_at(), fitted_at());
    assert_eq!(first.updated_at(), fitted_at());
    Ok(())
}

#[test]
fn a_fit_refuses_data_that_cannot_determine_it() -> Result<()> {
    let data = linear_dataset("underdetermined", 8, 0.0, &[1.0; 8], 0.0, 13)?;
    let error = fit(ModelFamily::Linear { ridge: 0.0 }, &data)
        .expect_err("eight features cannot be fitted from six rows");
    assert_eq!(error.code(), "invalid");
    Ok(())
}

#[test]
fn a_teacher_refuses_inputs_it_cannot_read() -> Result<()> {
    let data = linear_dataset("arity", 60, 0.0, &[1.0, 2.0], 0.0, 17)?;
    let teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &data)?;
    assert!(teacher.predict(&[1.0]).is_err(), "too few inputs");
    assert!(
        teacher.predict(&[1.0, 2.0, 3.0]).is_err(),
        "too many inputs"
    );
    let nan = teacher
        .predict(&[f64::NAN, 0.0])
        .expect_err("a NaN input must be refused");
    // A NaN score compares false against every threshold, so it reads as a
    // quiet decision not to trade rather than as a failure.
    assert_eq!(nan.code(), "numeric");
    assert!(teacher.predict(&[0.25, -0.25]).is_ok());
    Ok(())
}

#[test]
fn a_teacher_survives_serialization_with_its_predictions_intact() -> Result<()> {
    let data = humped_dataset("round-trip", 300, 55)?;
    let teacher = fit(ModelFamily::boosted(), &data)?;
    let encoded = serde_json::to_string(&teacher).expect("a teacher must serialise");
    let decoded: TrainedTeacher = serde_json::from_str(&encoded).expect("and deserialise");

    // Everything discrete comes back identical, so no field is silently
    // dropped by the encoding.
    assert_eq!(decoded.reference(), teacher.reference());
    assert_eq!(decoded.dataset(), teacher.dataset());
    assert_eq!(decoded.feature_names(), teacher.feature_names());
    assert_eq!(decoded.hyperparameters(), teacher.hyperparameters());
    assert_eq!(decoded.fitted_at(), teacher.fitted_at());
    assert_eq!(decoded.updated_at(), teacher.updated_at());
    assert_eq!(
        decoded.form().weights().len(),
        teacher.form().weights().len()
    );

    // The floats come back to within an ulp rather than bit-for-bit: this
    // workspace takes `serde_json` without its `float_roundtrip` feature, so
    // parsing is the fast approximate path and the last bit of a
    // seventeen-digit literal is not preserved. Two consequences, both
    // deliberate here. Predictions are asserted with a tolerance rather than
    // with `assert_eq!`, which is harmless for a model whose output is
    // compared against a threshold. And `structure_digest` — which is
    // bit-exact by design, so that two different models never collide — is a
    // within-process identity and is *not* asserted across the wire.
    for row in data.rows() {
        let (there, back) = (teacher.predict(row)?, decoded.predict(row)?);
        assert!(
            approx_eq(back, there, 1e-12),
            "a round trip moved a prediction from {there} to {back}"
        );
    }
    let (before, after) = (
        teacher.fit().expect("diagnostics"),
        decoded.fit().expect("diagnostics"),
    );
    assert_eq!(after.observations, before.observations);
    assert_eq!(after.holdout_observations, before.holdout_observations);
    assert!(approx_eq(after.holdout_r2, before.holdout_r2, 1e-12));
    Ok(())
}

// --- cadence: what an update at a given rate may change ---------------------

#[test]
fn a_cadence_permits_exactly_the_scopes_at_or_below_its_own_review() {
    // Three functions have to agree, and this is what ties them together:
    // `permits`, the ordering on cadences, and the floor each scope names.
    for cadence in ALL_CADENCES {
        for scope in ALL_SCOPES {
            let permitted = cadence.permits(scope);
            assert_eq!(
                permitted,
                cadence >= scope.required_cadence(),
                "{} vs {}: permission and the required floor disagree",
                cadence.as_str(),
                scope.as_str()
            );
            let authorised = cadence.authorise(
                payload_of(scope),
                day(1),
                None,
                "a test of the permission matrix",
            );
            assert_eq!(
                authorised.is_ok(),
                permitted,
                "{} authorising {} did not match its own permission list",
                cadence.as_str(),
                scope.as_str()
            );
        }
    }
}

#[test]
fn permitted_and_forbidden_scopes_partition_every_scope() {
    for cadence in ALL_CADENCES {
        let permitted = cadence.permitted_scopes();
        let forbidden = cadence.forbidden_scopes();
        assert_eq!(permitted.len() + forbidden.len(), ALL_SCOPES.len());
        for scope in ALL_SCOPES {
            assert_ne!(
                permitted.contains(&scope),
                forbidden.contains(&scope),
                "{} is on both lists or on neither for {}",
                scope.as_str(),
                cadence.as_str()
            );
        }
        // The description an operator reads has to name both halves, because a
        // permission list alone makes them work out the important one.
        let described = cadence.describe();
        for scope in permitted {
            assert!(described.contains(scope.as_str()));
        }
    }
}

#[test]
fn the_fastest_cadence_cannot_replace_a_model_and_the_slowest_can() -> Result<()> {
    let data = linear_dataset("governed", 80, 0.0, &[1.0, -1.0], 0.05, 23)?;
    let mut teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &data)?;
    let replacement = TeacherForm::Linear {
        intercept: 99.0,
        coefficients: vec![0.0, 0.0],
    };

    let refused = TrainingCadence::Millisecond.authorise(
        UpdatePayload::Structure(replacement.clone()),
        day(101),
        None,
        "a fast loop trying to replace the model",
    );
    let error = refused.expect_err("a millisecond update must not carry a structure");
    assert_eq!(error.code(), "denied");
    assert!(
        error.message().contains("daily_to_weekly"),
        "the refusal must name the cadence that could do it: {}",
        error.message()
    );

    let before = teacher.structure_digest();
    let allowed = TrainingCadence::DailyToWeekly.authorise(
        UpdatePayload::Structure(replacement),
        day(101),
        None,
        "the weekly retrain",
    )?;
    teacher.apply(&allowed)?;
    assert_ne!(teacher.structure_digest(), before);
    assert_eq!(teacher.updated_at(), day(101));
    Ok(())
}

#[test]
fn replacing_a_structure_discards_the_evaluation_it_did_not_earn() -> Result<()> {
    let data = linear_dataset("evaluated", 80, 0.0, &[1.0, -1.0], 0.0, 29)?;
    let mut teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &data)?;
    assert!(teacher.fit().is_some());

    let update = TrainingCadence::DailyToWeekly.authorise(
        UpdatePayload::Structure(TeacherForm::Linear {
            intercept: 0.0,
            coefficients: vec![3.0, 3.0],
        }),
        day(101),
        None,
        "a new functional form",
    )?;
    teacher.apply(&update)?;
    // A new function has not been measured. Inheriting the old diagnostics is
    // how an unreviewed model acquires an evaluation.
    assert!(
        teacher.fit().is_none(),
        "a replaced structure must not keep the previous fit's diagnostics"
    );
    Ok(())
}

#[test]
fn no_non_structural_update_can_change_what_the_model_is() -> Result<()> {
    let data = linear_dataset("stable", 80, 0.0, &[1.0, -1.0], 0.05, 31)?;
    let teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &data)?;
    let digest = teacher.structure_digest();
    let probe = [0.4, -0.6];
    let baseline = teacher.predict(&probe)?;

    for scope in ALL_SCOPES.into_iter().filter(|s| !s.changes_structure()) {
        let mut copy = teacher.clone();
        let payload = match scope {
            // Sized for this model rather than for the one-feature fixture.
            UpdateScope::Weights => UpdatePayload::Weights(vec![9.0, 9.0]),
            other => payload_of(other),
        };
        let update = scope.required_cadence().authorise(
            payload,
            day(101),
            None,
            "a non-structural update",
        )?;
        copy.apply(&update)?;
        assert_eq!(
            copy.structure_digest(),
            digest,
            "{} changed the model's shape",
            scope.as_str()
        );
    }

    // Weights are still allowed to change what it *says*, which is the whole
    // point of separating the two.
    let mut retuned = teacher.clone();
    let update = TrainingCadence::Millisecond.authorise(
        UpdatePayload::Weights(vec![9.0, 9.0]),
        day(101),
        None,
        "a fast retune",
    )?;
    retuned.apply(&update)?;
    assert!(!approx_eq(retuned.predict(&probe)?, baseline, 1e-9));
    Ok(())
}

#[test]
fn a_weights_update_of_the_wrong_width_is_refused_at_every_cadence() -> Result<()> {
    let data = linear_dataset("two-wide", 80, 0.0, &[1.0, -1.0], 0.05, 37)?;
    let teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &data)?;

    for cadence in ALL_CADENCES {
        let mut copy = teacher.clone();
        // The cadence check passes — this *is* a weights update — and the
        // second, independent guard is what catches it. Changing how many
        // coefficients a model has is a structural change wearing a weights
        // update's clothes, and the slowest cadence does not excuse it either.
        let update = cadence.authorise(
            UpdatePayload::Weights(vec![1.0, 2.0, 3.0]),
            day(101),
            None,
            "three weights for a two-coefficient model",
        )?;
        let error = copy
            .apply(&update)
            .expect_err("a mis-sized weights update must be refused");
        assert_eq!(error.code(), "invalid");
        assert_eq!(copy, teacher, "a refused update must leave the model alone");
    }
    Ok(())
}

#[test]
fn an_update_arriving_faster_than_its_cadence_is_refused() {
    for cadence in ALL_CADENCES {
        let previous = day(200);
        let floor = cadence.minimum_interval();

        let on_time = cadence.authorise(
            UpdatePayload::Weights(vec![1.0]),
            previous.saturating_add(floor),
            Some(previous),
            "exactly at the floor",
        );
        assert!(
            on_time.is_ok(),
            "{} should accept an update exactly one interval later",
            cadence.as_str()
        );

        let too_soon = cadence.authorise(
            UpdatePayload::Weights(vec![1.0]),
            previous
                .saturating_add(floor)
                .saturating_sub(Duration::from_nanos(1)),
            Some(previous),
            "one nanosecond early",
        );
        let error = too_soon.expect_err("an update faster than its cadence is not at that cadence");
        assert_eq!(error.code(), "denied");

        let backdated = cadence.authorise(
            UpdatePayload::Weights(vec![1.0]),
            previous.saturating_sub(Duration::from_nanos(1)),
            Some(previous),
            "dated before the update it follows",
        );
        assert!(
            backdated.is_err(),
            "{} accepted an update dated before the previous one",
            cadence.as_str()
        );
    }
}

#[test]
fn a_cadence_is_overdue_only_past_its_own_ceiling() {
    for cadence in ALL_CADENCES {
        let last = day(200);
        let ceiling = cadence.maximum_interval();
        assert!(
            !cadence.is_overdue(last, last.saturating_add(ceiling)),
            "{} is not overdue at exactly its ceiling",
            cadence.as_str()
        );
        assert!(
            cadence.is_overdue(
                last,
                last.saturating_add(ceiling)
                    .saturating_add(Duration::from_nanos(1))
            ),
            "{} should be overdue one nanosecond past its ceiling",
            cadence.as_str()
        );
        // Being early is not being late. A backdated update is a different
        // complaint, and `authorise` is where it is made.
        assert!(!cadence.is_overdue(last, last.saturating_sub(ceiling)));
        // The floor is never above the ceiling, or no update could ever be
        // both timely and on time.
        assert!(cadence.minimum_interval() <= ceiling);
    }
}

#[test]
fn a_calibration_moves_the_output_without_touching_the_function() -> Result<()> {
    let data = linear_dataset("calibrated", 80, 0.0, &[1.0, -1.0], 0.0, 43)?;
    let mut teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &data)?;
    let probe = [0.4, -0.6];
    let raw = teacher.predict(&probe)?;
    assert!(teacher.calibration().is_identity());

    let update = TrainingCadence::SecondsToMinutes.authorise(
        UpdatePayload::Calibration {
            scale: 2.0,
            offset: 0.5,
        },
        day(101),
        None,
        "recalibrating onto the decision layer's scale",
    )?;
    teacher.apply(&update)?;
    assert!(approx_eq(teacher.predict(&probe)?, 2.0 * raw + 0.5, 1e-12));
    assert_eq!(
        teacher.calibration(),
        Calibration {
            scale: 2.0,
            offset: 0.5
        }
    );
    // Calibration is not refitting: the diagnostics still describe the same
    // function, so they are still the diagnostics of this model.
    assert!(teacher.fit().is_some());

    let non_finite = TrainingCadence::SecondsToMinutes.authorise(
        UpdatePayload::Calibration {
            scale: f64::NAN,
            offset: 0.0,
        },
        day(102),
        None,
        "a broken calibration",
    )?;
    assert!(teacher.apply(&non_finite).is_err());
    Ok(())
}

// --- distillation -----------------------------------------------------------

#[test]
fn a_student_is_fitted_to_its_teacher_and_never_to_the_labels() -> Result<()> {
    let training = humped_dataset("teacher-training", 400, 61)?;
    let teacher = fit(ModelFamily::boosted(), &training)?;

    // Two probe sets with identical features and times, and targets that could
    // not be more different. If the student were fitted to the observed labels
    // rather than to the teacher's outputs, these two would not agree.
    let honest = humped_dataset("probe", 300, 67)?;
    let inverted = honest.with_targets(honest.targets().iter().map(|t| -100.0 * t).collect())?;

    let from_honest = distil(&teacher, &honest, StudentForm::shallow_tree(), 0.0)?;
    let from_inverted = distil(&teacher, &inverted, StudentForm::shallow_tree(), 0.0)?;

    assert_eq!(
        from_honest.student().digest(),
        from_inverted.student().digest(),
        "the student depends on the probe's labels, so it was not distilled"
    );
    assert!(approx_eq(
        from_honest.fidelity().agreement_r2,
        from_inverted.fidelity().agreement_r2,
        0.0
    ));
    Ok(())
}

#[test]
fn a_linear_teacher_distils_into_a_faithful_linear_student() -> Result<()> {
    let training = linear_dataset("linear-teacher", 300, 0.25, &[1.5, -0.75], 0.05, 71)?;
    let teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &training)?;
    let probe = linear_dataset("linear-probe", 200, 0.25, &[1.5, -0.75], 0.05, 73)?;

    let distillation = distil(&teacher, &probe, StudentForm::Linear { ridge: 0.0 }, 0.0)?;
    let report = distillation.fidelity();
    assert_eq!(report.probe_samples, probe.len());
    assert!(report.agreement_r2 > 0.9999, "{}", report.summarise());
    assert!(report.rank_correlation > 0.999, "{}", report.summarise());
    assert!(report.decision_agreement > 0.98, "{}", report.summarise());
    assert!(report.maximum_absolute_gap < 1e-6, "{}", report.summarise());

    let student = distillation.approved_student(&FidelityPolicy::default())?;
    assert_eq!(student.arity(), teacher.arity());
    assert!(matches!(student.form(), ModelForm::Linear { .. }));
    // The cost is a property of the value, which is what makes it admissible
    // inside a latency budget at all.
    assert_eq!(student.cost(), report.student_cost);
    Ok(())
}

#[test]
fn decision_agreement_is_measured_at_the_threshold_it_was_given() -> Result<()> {
    let training = linear_dataset("threshold", 300, 0.0, &[1.0, 0.0], 0.02, 79)?;
    let teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &training)?;
    let probe = linear_dataset("threshold-probe", 200, 0.0, &[1.0, 0.0], 0.02, 83)?;

    let outputs = teacher.predict_all(&probe)?;
    let far_above = outputs.iter().cloned().fold(f64::NEG_INFINITY, f64::max) + 10.0;

    let at_zero = distil(&teacher, &probe, StudentForm::Linear { ridge: 0.0 }, 0.0)?;
    let above_everything = distil(
        &teacher,
        &probe,
        StudentForm::Linear { ridge: 0.0 },
        far_above,
    )?;

    // Every probe row sits below a threshold nothing reaches, so teacher and
    // student agree on all of them — a different number from the same models
    // at zero, which is the point: agreement is only meaningful at the
    // threshold the strategy will actually use.
    assert!(above_everything.fidelity().decision_agreement >= 1.0);
    assert!(approx_eq(
        above_everything.fidelity().decision_threshold,
        far_above,
        0.0
    ));
    assert!(at_zero.fidelity().decision_agreement <= 1.0);
    // The student is the same either way; only the measurement moved.
    assert_eq!(
        at_zero.student().digest(),
        above_everything.student().digest()
    );
    Ok(())
}

#[test]
fn a_student_too_expensive_for_the_hot_path_is_refused_by_name() -> Result<()> {
    let training = humped_dataset("costly", 400, 89)?;
    let teacher = fit(ModelFamily::boosted(), &training)?;
    let probe = humped_dataset("costly-probe", 300, 97)?;
    let distillation = distil(&teacher, &probe, StudentForm::shallow_tree(), 0.0)?;

    let budget = FidelityPolicy {
        maximum_student_cost: 1,
        ..FidelityPolicy::default()
    };
    assert!(!distillation.is_promotable(&budget));
    let error = distillation
        .approved_student(&budget)
        .expect_err("a student over budget must not be handed out");
    assert_eq!(error.code(), "denied");
    assert!(
        error.message().contains("step(s)"),
        "the refusal must name the budget it broke: {}",
        error.message()
    );

    // The same student under a policy it meets is handed over, so the refusal
    // above is the policy talking and not a broken distillation.
    let generous = FidelityPolicy {
        minimum_agreement_r2: -1.0,
        minimum_rank_correlation: -1.0,
        minimum_decision_agreement: 0.0,
        minimum_probe_samples: 1,
        ..FidelityPolicy::default()
    };
    assert!(distillation.approved_student(&generous).is_ok());
    Ok(())
}

#[test]
fn a_fidelity_measured_on_too_little_data_is_not_a_measurement() -> Result<()> {
    let training = linear_dataset("small-probe", 200, 0.0, &[1.0, 1.0], 0.01, 101)?;
    let teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &training)?;
    let probe = linear_dataset("tiny", 20, 0.0, &[1.0, 1.0], 0.01, 103)?;
    let distillation = distil(&teacher, &probe, StudentForm::Linear { ridge: 0.0 }, 0.0)?;

    // The student is a near-perfect copy, and it still may not stand in: the
    // gap was measured on twenty rows.
    assert!(distillation.fidelity().agreement_r2 > 0.999);
    assert!(!distillation.is_promotable(&FidelityPolicy::default()));
    let error = distillation
        .approved_student(&FidelityPolicy::default())
        .expect_err("twenty rows do not measure a fidelity");
    assert!(error.message().contains("20 row(s)"), "{}", error.message());
    Ok(())
}

#[test]
fn a_faithful_copy_of_an_unevaluated_teacher_is_still_unevaluated() -> Result<()> {
    let training = linear_dataset("swapped", 300, 0.0, &[1.0, -1.0], 0.01, 107)?;
    let mut teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &training)?;
    let probe = linear_dataset("swapped-probe", 200, 0.0, &[1.0, -1.0], 0.01, 109)?;

    let update = TrainingCadence::DailyToWeekly.authorise(
        UpdatePayload::Structure(TeacherForm::Linear {
            intercept: 0.0,
            coefficients: vec![4.0, 4.0],
        }),
        day(101),
        None,
        "a form nobody has measured",
    )?;
    teacher.apply(&update)?;

    let distillation = distil(&teacher, &probe, StudentForm::Linear { ridge: 0.0 }, 0.0)?;
    // The distillation itself is perfect — the student copies the teacher
    // exactly — and that is precisely the trap. Fidelity to an unevaluated
    // function is not evidence about the function.
    assert!(distillation.fidelity().agreement_r2 > 0.9999);
    assert!(!distillation.teacher_was_evaluated());
    let error = distillation
        .approved_student(&FidelityPolicy::default())
        .expect_err("a copy of an unevaluated model must not be promotable");
    assert_eq!(error.code(), "denied");
    assert!(
        error.message().contains("no measured fit"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_probe_set_of_the_wrong_shape_is_refused() -> Result<()> {
    let training = linear_dataset("shape", 200, 0.0, &[1.0, -1.0], 0.01, 113)?;
    let teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &training)?;
    let wrong_arity = linear_dataset("three-wide", 200, 0.0, &[1.0, -1.0, 0.5], 0.01, 127)?;

    let error = distil(
        &teacher,
        &wrong_arity,
        StudentForm::Linear { ridge: 0.0 },
        0.0,
    )
    .expect_err("a probe set of a different width is not a probe set");
    assert_eq!(error.code(), "invalid");

    let probe = linear_dataset("right-width", 200, 0.0, &[1.0, -1.0], 0.01, 131)?;
    assert!(
        distil(
            &teacher,
            &probe,
            StudentForm::Linear { ridge: 0.0 },
            f64::NAN
        )
        .is_err(),
        "a non-finite decision threshold measures nothing"
    );
    Ok(())
}

#[test]
fn a_distilled_tree_is_bounded_and_evaluable_everywhere() -> Result<()> {
    let training = humped_dataset("tree-teacher", 500, 137)?;
    let teacher = fit(ModelFamily::boosted(), &training)?;
    let probe = humped_dataset("tree-probe", 400, 139)?;
    let distillation = distil(
        &teacher,
        &probe,
        StudentForm::Tree {
            max_depth: 3,
            min_samples_leaf: 20,
            candidate_splits: 24,
        },
        0.0,
    )?;
    let student = distillation.student();

    // A depth-three tree has at most fifteen nodes, and the cost is the node
    // count — that bound is what a latency budget is charged against.
    assert!(student.cost() <= 15, "cost {}", student.cost());
    assert!(matches!(student.form(), ModelForm::Tree { .. }));

    // Every descent terminates. The `DistilledModel::tree` constructor only
    // accepts forward-pointing branches, so this is really a check that the
    // grower produced a tree that constructor would accept — it did, or
    // `distil` would have failed above.
    for row in probe.rows() {
        let value = student.evaluate(row)?;
        assert!(value.is_finite());
    }
    // The distillation is a real fit, not a constant.
    assert!(distillation.fidelity().agreement_r2 > 0.5);
    Ok(())
}

#[test]
fn a_distillation_carries_the_provenance_of_both_ends() -> Result<()> {
    let training = linear_dataset("provenance", 200, 0.0, &[1.0, 1.0], 0.01, 149)?;
    let teacher = fit(ModelFamily::Linear { ridge: 0.0 }, &training)?;
    let probe = linear_dataset("provenance-probe", 200, 0.0, &[1.0, 1.0], 0.01, 151)?;
    let distillation = distil(&teacher, &probe, StudentForm::Linear { ridge: 0.0 }, 0.0)?;

    assert_eq!(distillation.teacher_reference(), teacher.reference());
    assert_eq!(distillation.teacher_dataset(), training.name());
    assert_eq!(distillation.probe_dataset(), probe.name());
    assert!(distillation.student().name().contains(&teacher.reference()));

    // A distillation serialises, so the record of what was measured outlives
    // the process that measured it.
    let encoded = serde_json::to_string(&distillation).expect("a distillation must serialise");
    let seen: serde_json::Value = serde_json::from_str(&encoded).expect("valid json");
    assert!(seen.get("fidelity").is_some());
    Ok(())
}

// --- the provider port ------------------------------------------------------

#[test]
fn a_job_produces_no_artifact_until_it_has_actually_run() -> Result<()> {
    let data = humped_dataset("job-data", 300, 157)?;
    let mut provider = LocalTrainingProvider::new(0xA11CE);
    let submitted = provider.submit(spec("queued-model", ModelFamily::boosted()), &data, day(1))?;

    assert_eq!(submitted.state, JobState::Queued);
    assert_eq!(submitted.rows, data.len());
    assert_eq!(submitted.provider, "local-trainer");
    // Nothing has run. A caller written against a queueing provider must not
    // be able to read a model out of a job that has not started.
    assert!(provider.artifact(&submitted.id).is_err());

    let running = provider.poll(&submitted.id, day(2))?;
    assert_eq!(running.state, JobState::Running);
    assert!(provider.artifact(&submitted.id).is_err());

    let done = provider.poll(&submitted.id, day(3))?;
    assert_eq!(done.state, JobState::Succeeded);
    let artifact = provider.artifact(&submitted.id)?;
    assert_eq!(artifact.job, submitted.id);
    assert_eq!(artifact.rows, data.len());
    assert_eq!(artifact.dataset, data.name());
    // Every timestamp is one the caller passed in.
    assert_eq!(artifact.produced_at, day(3));
    assert_eq!(done.submitted_at, day(1));
    assert_eq!(done.updated_at, day(3));
    Ok(())
}

#[test]
fn a_terminal_job_stops_moving() -> Result<()> {
    let data = humped_dataset("terminal", 300, 163)?;
    let mut provider = LocalTrainingProvider::new(7);
    let job = provider.submit(
        spec("terminal-model", ModelFamily::boosted()),
        &data,
        day(1),
    )?;
    provider.poll(&job.id, day(2))?;
    let finished = provider.poll(&job.id, day(3))?;
    assert!(finished.state.is_terminal());

    let again = provider.poll(&job.id, day(9))?;
    assert_eq!(again.state, JobState::Succeeded);
    assert_eq!(
        again.updated_at,
        day(3),
        "polling a finished job must not restamp it"
    );
    let error = provider
        .cancel(&job.id, day(9))
        .expect_err("a finished job cannot be cancelled");
    assert_eq!(error.code(), "denied");
    Ok(())
}

#[test]
fn a_cancelled_job_yields_no_model() -> Result<()> {
    let data = humped_dataset("cancelled", 300, 167)?;
    let mut provider = LocalTrainingProvider::new(11);
    let job = provider.submit(
        spec("cancelled-model", ModelFamily::boosted()),
        &data,
        day(1),
    )?;
    let cancelled = provider.cancel(&job.id, day(2))?;
    assert!(matches!(cancelled.state, JobState::Cancelled(_)));

    // And polling does not quietly restart it.
    let polled = provider.poll(&job.id, day(3))?;
    assert!(matches!(polled.state, JobState::Cancelled(_)));
    assert!(provider.artifact(&job.id).is_err());
    Ok(())
}

#[test]
fn a_failed_fit_is_reported_as_a_failure_and_not_as_a_model() -> Result<()> {
    // Eight features and eight rows: after the holdout, six observations for
    // eight coefficients. The fit cannot be determined, and the job has to say
    // so rather than produce something.
    let data = linear_dataset("undetermined", 8, 0.0, &[1.0; 8], 0.0, 173)?;
    let mut provider = LocalTrainingProvider::new(13);
    let job = provider.submit(
        spec("doomed", ModelFamily::Linear { ridge: 0.0 }),
        &data,
        day(1),
    )?;
    provider.poll(&job.id, day(2))?;
    let failed = provider.poll(&job.id, day(3))?;
    assert!(matches!(failed.state, JobState::Failed(_)));
    assert!(!failed.state.produced_an_artifact());
    assert!(provider.artifact(&job.id).is_err());

    let error = run_to_completion(
        &mut provider,
        spec("doomed-again", ModelFamily::Linear { ridge: 0.0 }),
        &data,
        day(1),
        8,
    )
    .expect_err("a failing fit must not return an artifact");
    assert!(error.message().contains("failed"), "{}", error.message());
    Ok(())
}

#[test]
fn running_to_completion_is_bounded() -> Result<()> {
    let data = humped_dataset("bounded", 300, 179)?;
    let mut provider = LocalTrainingProvider::new(17);

    let timed_out = run_to_completion(
        &mut provider,
        spec("impatient", ModelFamily::boosted()),
        &data,
        day(1),
        1,
    )
    .expect_err("one poll is not enough for a provider that queues");
    assert_eq!(timed_out.code(), "timeout");

    let artifact = run_to_completion(
        &mut provider,
        spec("patient", ModelFamily::boosted()),
        &data,
        day(1),
        8,
    )?;
    assert_eq!(artifact.rows, data.len());
    assert!(artifact.teacher.fit().is_some());
    assert_eq!(provider.job_count(), 2);
    Ok(())
}

#[test]
fn job_identifiers_come_from_the_seed_and_not_from_the_environment() -> Result<()> {
    let data = humped_dataset("ids", 200, 181)?;
    let ids_for = |seed: u64| -> Result<Vec<String>> {
        let mut provider = LocalTrainingProvider::new(seed);
        let mut out = Vec::new();
        for index in 0..3 {
            let job = provider.submit(
                spec("identified", ModelFamily::boosted()),
                &data,
                day(index),
            )?;
            out.push(job.id.as_str().to_string());
        }
        Ok(out)
    };

    assert_eq!(ids_for(0x5EED)?, ids_for(0x5EED)?);
    assert_ne!(ids_for(0x5EED)?, ids_for(0x5EEE)?);
    Ok(())
}

#[test]
fn a_job_specification_is_validated_before_anything_is_queued() -> Result<()> {
    let data = humped_dataset("validated", 200, 191)?;
    let mut provider = LocalTrainingProvider::new(19);

    let unowned = TrainingSpec::new("nameless", "v1", "  ", "fixture", ModelFamily::boosted());
    let error = provider
        .submit(unowned, &data, day(1))
        .expect_err("a model nobody owns cannot be retired by anyone either");
    assert!(error.message().contains("owner"), "{}", error.message());
    assert_eq!(provider.job_count(), 0, "a refused job must not be queued");

    let bad_holdout = spec("odd-holdout", ModelFamily::boosted()).with_holdout(1.5);
    assert!(provider.submit(bad_holdout, &data, day(1)).is_err());
    assert_eq!(provider.job_count(), 0);
    Ok(())
}

#[test]
fn the_label_horizon_is_never_zero() {
    for horizon in [
        Horizon::Intraday,
        Horizon::ShortTerm,
        Horizon::MediumTerm,
        Horizon::LongTerm,
    ] {
        let configured = spec("horizoned", ModelFamily::boosted()).with_horizon(horizon);
        // A label spanning no time at all is a label computed from data the
        // decision could not have had, so the purge width a caller derives
        // from this can never be nothing.
        assert!(configured.label_horizon() >= 1);
        assert!(configured.label_horizon() as f64 >= horizon.typical_holding_days());
    }
}

#[test]
fn a_job_record_survives_serialization() -> Result<()> {
    let data = humped_dataset("serialised", 200, 193)?;
    let mut provider = LocalTrainingProvider::new(23);
    let job = provider.submit(
        spec("serialised-model", ModelFamily::boosted())
            .with_cadence(TrainingCadence::Hourly)
            .with_purpose("a record of what was asked for"),
        &data,
        day(1),
    )?;
    let encoded = serde_json::to_string(&job).expect("a job must serialise");
    let decoded: qip_training::job::TrainingJob =
        serde_json::from_str(&encoded).expect("and deserialise");
    assert_eq!(decoded, job);
    Ok(())
}

// --- the Vertex AI port -----------------------------------------------------

#[test]
fn vertex_is_unavailable_even_when_everything_configurable_is_configured() {
    let provider = VertexAiProvider::with_credentials(vertex_config(), true);
    // The config is complete and the credentials are asserted present. The
    // transport still does not exist in this build, and the port says so
    // rather than answering as though it might work.
    assert!(!provider.is_available());
    let missing = provider.missing();
    assert_eq!(
        missing.len(),
        1,
        "only the transport should be outstanding: {missing:?}"
    );
    assert!(missing[0].contains("HTTPS transport"), "{}", missing[0]);
}

#[test]
fn an_unconfigured_vertex_deployment_is_told_every_missing_piece() {
    let empty = VertexAiConfig {
        project_id: String::new(),
        region: String::new(),
        staging_bucket: "s3://wrong-cloud".to_string(),
        workload: VertexWorkload::AutoMl {
            objective: String::new(),
            target_column: String::new(),
            budget_node_hours: 0,
        },
        workload_identity: WorkloadIdentityBinding {
            kubernetes_service_account: String::new(),
            google_service_account: String::new(),
            roles: Vec::new(),
        },
    };
    let provider = VertexAiProvider::new(empty);
    let missing = provider.missing();
    // A deployment holding four of the seven is still not a deployment that
    // can train a model, so the list is itemised rather than summarised.
    assert_eq!(missing.len(), 7, "{missing:?}");
    let text = provider.requirement();
    for fragment in [
        "project id",
        "region",
        "gs://",
        "workload",
        "workload-identity",
        "Application Default Credentials",
        "HTTPS transport",
    ] {
        assert!(
            text.contains(fragment),
            "requirement omits {fragment}: {text}"
        );
    }
    // And it says what happens instead, without dressing it up as equivalent.
    assert!(text.contains("not a substitute"), "{text}");
}

#[test]
fn vertex_refuses_every_call_rather_than_pretending_to_train() -> Result<()> {
    let data = humped_dataset("vertex-data", 200, 197)?;
    let mut provider = VertexAiProvider::with_credentials(vertex_config(), true);
    let job_id = LocalTrainingProvider::new(29)
        .submit(spec("borrowed-id", ModelFamily::boosted()), &data, day(1))?
        .id;

    for error in [
        provider
            .submit(spec("would-be", ModelFamily::boosted()), &data, day(1))
            .expect_err("submit"),
        provider.poll(&job_id, day(2)).expect_err("poll"),
        provider.artifact(&job_id).expect_err("artifact"),
        provider.cancel(&job_id, day(2)).expect_err("cancel"),
    ] {
        assert_eq!(
            error.code(),
            "unavailable",
            "a missing dependency is not an invalid request"
        );
        assert!(
            error.message().contains("HTTPS transport"),
            "the refusal must name the missing prerequisite: {}",
            error.message()
        );
    }
    Ok(())
}

#[test]
fn a_caller_written_against_the_port_gets_the_same_refusal() -> Result<()> {
    let data = humped_dataset("port", 200, 199)?;
    let mut vertex = VertexAiProvider::with_credentials(vertex_config(), true);

    // Through the trait object, which is how everything downstream holds it.
    // `requirement` is reachable both as an inherent method and through the
    // trait's default, and the trait impl must reach the real text rather
    // than an empty string or itself.
    let as_port: &dyn TrainingProvider = &vertex;
    assert_eq!(as_port.name(), "vertex-ai");
    assert!(!as_port.is_available());
    assert!(as_port.requirement().contains("HTTPS transport"));

    let error = run_to_completion(
        &mut vertex,
        spec("never-ran", ModelFamily::boosted()),
        &data,
        day(1),
        8,
    )
    .expect_err("an unavailable provider must not fall back to a local fit");
    assert_eq!(error.code(), "unavailable");

    // A provider that *is* available answers the identical call. The port is
    // the only thing the caller knows about either of them.
    let mut local = LocalTrainingProvider::new(31);
    let available: &dyn TrainingProvider = &local;
    assert!(available.is_available());
    assert!(available.requirement().is_empty());
    let artifact = run_to_completion(
        &mut local,
        spec("did-run", ModelFamily::boosted()),
        &data,
        day(1),
        8,
    )?;
    assert_eq!(artifact.rows, data.len());
    Ok(())
}

#[test]
fn a_vertex_request_describes_itself_completely_enough_to_be_reviewed() {
    let config = vertex_config();
    let described = config.workload.describe();
    assert!(described.contains("n1-standard-8"), "{described}");
    assert!(
        described.contains("trainers/teacher:2025-08"),
        "{described}"
    );

    let automl = VertexWorkload::AutoMl {
        objective: "regression".to_string(),
        target_column: "forward_return_5d".to_string(),
        budget_node_hours: 12,
    };
    let described = automl.describe();
    for fragment in ["regression", "forward_return_5d", "12 node-hour"] {
        assert!(described.contains(fragment), "{described}");
    }

    // The request round-trips, so a review can be held over the exact bytes a
    // deployment would send if the transport existed.
    let encoded = serde_json::to_string(&config).expect("a request must serialise");
    let decoded: VertexAiConfig = serde_json::from_str(&encoded).expect("and deserialise");
    assert_eq!(decoded, config);
}

// --- the shape of the crate's own guarantees --------------------------------

#[test]
fn a_distillation_only_exists_with_its_fidelity_already_measured() -> Result<()> {
    // There is no constructor but `distil`, and `distil` measures before it
    // returns. The runtime half of that claim is that every `Distillation`
    // reachable in this test suite carries a report over the probe it names.
    let training = humped_dataset("measured", 400, 211)?;
    let teacher = fit(ModelFamily::boosted(), &training)?;
    let probe = humped_dataset("measured-probe", 250, 223)?;

    for form in [
        StudentForm::Linear { ridge: 0.0 },
        StudentForm::Linear { ridge: 1.0 },
        StudentForm::shallow_tree(),
    ] {
        let distillation: Distillation = distil(&teacher, &probe, form, 0.0)?;
        let report = distillation.fidelity();
        assert_eq!(report.probe_samples, probe.len());
        assert_eq!(distillation.form(), form);
        assert!(report.mean_absolute_gap >= 0.0);
        assert!(report.maximum_absolute_gap >= report.mean_absolute_gap);
        assert!(report.decision_agreement >= 0.0 && report.decision_agreement <= 1.0);
        assert!(report.student_cost > 0);
    }
    Ok(())
}
