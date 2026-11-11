//! The four promotion tests (ADR 0392, R12-MODEL-10202): replay,
//! calibration, classical baseline and envelope.
//!
//! The registry's own suite (`qip-ai/tests/model_infrastructure.rs`) proves
//! that a card lacking any of the four is refused. This suite proves the
//! other half: that the kernel computes each verdict from a recorded window
//! and a served artifact, that each test can fail for its own reason, and
//! that a promotion carries all four on its signed record.

#![allow(clippy::panic_in_result_fn)]

use qip_ai::registry::{
    CALIBRATION_TEST, CLASSICAL_BASELINE_TEST, ModelRegistry, PROMOTION_TESTS, PromotionTests,
    REPLAY_TEST, RISK_ENVELOPE_TEST,
};
use qip_ai::serving::{ModelArtifact, ModelFormat, ModelProvider, ServedModel};
use qip_core::error::Result;
use qip_core::{Context, Duration, Timestamp};
use qip_financial::universe::Universe;
use qip_kernel::central::models::{register_fit, run_promotion_tests};
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_training::dataset::TrainingDataset;
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, SkillPolicy, TrainedTeacher};
use qip_training::serve::InTreeProvider;
use std::sync::atomic::{AtomicU64, Ordering};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// Rows `from..to` of one process: two features and the value each row's
/// next bar realised, a linear function of them plus a small residual.
fn window(name: &str, from: usize, to: usize) -> Result<TrainingDataset> {
    window_with(name, from, to, |a, b, residual| {
        0.5 * a - 0.25 * b + 0.1 + residual
    })
}

fn window_with(
    name: &str,
    from: usize,
    to: usize,
    realised: impl Fn(f64, f64, f64) -> f64,
) -> Result<TrainingDataset> {
    let mut rows = Vec::new();
    let mut targets = Vec::new();
    let mut times = Vec::new();
    for i in from..to {
        let a = (i as f64 * 0.37).sin();
        let b = (i as f64 * 0.11).cos();
        let residual = ((i * 7919) % 13) as f64 / 13.0 * 0.02 - 0.01;
        rows.push(vec![a, b]);
        targets.push(realised(a, b, residual));
        times.push(start().saturating_add(Duration::from_mins(i as i64)));
    }
    TrainingDataset::new(
        name,
        vec!["a".to_string(), "b".to_string()],
        rows,
        targets,
        times,
    )
}

fn teacher(name: &str) -> Result<TrainedTeacher> {
    let data = window(&format!("{name}-data"), 0, 200)?;
    let spec = TrainingSpec::new(
        name,
        "0.1.0",
        "promotion-tests",
        data.name(),
        ModelFamily::Linear { ridge: 1e-6 },
    )
    .with_holdout(0.25);
    LocalTrainer::new().fit(&spec, &data, start())
}

/// A registry holding `teacher`'s card with a passing evaluation and no
/// promotion tests yet.
fn registry_with(teacher: &TrainedTeacher) -> Result<ModelRegistry> {
    let mut registry = ModelRegistry::new();
    let registration = register_fit(
        &mut registry,
        teacher,
        &SkillPolicy::default(),
        "promotion-tests",
        start(),
    )?;
    assert!(
        registration.passed,
        "premise: the fit cleared the skill bar"
    );
    Ok(registry)
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
    platform.name_model_desk("promotion-tests/model-desk")?;
    Ok(platform)
}

fn passed(tests: &PromotionTests, name: &str) -> bool {
    tests
        .results
        .get(name)
        .unwrap_or_else(|| panic!("the {name} test must be on record"))
        .passed
}

fn evidence<'a>(tests: &'a PromotionTests, name: &str) -> &'a str {
    &tests
        .results
        .get(name)
        .unwrap_or_else(|| panic!("the {name} test must be on record"))
        .evidence
}

#[test]
fn a_candidate_with_no_promotion_tests_is_refused_and_one_tested_carries_all_four_on_its_signed_record()
-> Result<()> {
    // The failure this prevents (R12-MODEL-10202): a passing evaluation
    // alone moved the production alias. The same card is tried twice, and
    // the only difference between the two tries is the tests.
    let mut platform = platform()?;
    let teacher = teacher("bar-linear-tested")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let mut registry = registry_with(&teacher)?;

    let error = platform
        .promote_model(&mut registry, &artifact, None, &[], start())
        .expect_err("an untested candidate must be refused");
    assert!(
        error
            .message()
            .contains("it has no promotion tests on record"),
        "the refusal must name the missing tests: {}",
        error.message()
    );
    assert!(
        platform.model_promotions().is_empty(),
        "a refused promotion must leave no promotion"
    );
    assert!(
        platform.model_promotion_history()?.is_empty(),
        "a refused promotion must leave no record on the log"
    );

    let recorded = window("bar-linear-tested-recorded", 200, 260)?;
    let tests = platform.test_model_for_promotion(&mut registry, &artifact, &recorded, start())?;
    assert_eq!(tests.gap(&artifact.digest), None, "premise: all four pass");
    platform.promote_model(&mut registry, &artifact, None, &[], start())?;

    let history = platform.model_promotion_history()?;
    assert_eq!(history.len(), 1, "premise: one promotion is on the log");
    let record = &history[0];
    for name in PROMOTION_TESTS {
        let prefix = format!("{name}: ");
        let lines: Vec<&String> = record
            .stages
            .iter()
            .filter(|stage| stage.starts_with(&prefix))
            .collect();
        assert_eq!(
            lines.len(),
            1,
            "the record must carry exactly one {name} stage: {:?}",
            record.stages
        );
        assert!(
            lines[0].contains(evidence(&tests, name)) && lines[0].ends_with(recorded.name()),
            "the {name} stage must carry what was measured and the window: {}",
            lines[0]
        );
    }
    assert!(
        platform.promotion_verifies(record),
        "the record with its test stages must carry the centre's signature"
    );
    Ok(())
}

#[test]
fn the_four_tests_are_measured_on_the_recorded_window_and_bound_to_the_artifact_tested()
-> Result<()> {
    let teacher = teacher("bar-linear-measured")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let mut registry = registry_with(&teacher)?;
    let recorded = window("bar-linear-measured-recorded", 200, 260)?;

    let tests = run_promotion_tests(
        &InTreeProvider,
        &mut registry,
        &artifact,
        &recorded,
        start(),
    )?;

    assert_eq!(tests.artifact_digest, artifact.digest);
    assert_eq!(tests.window, recorded.name());
    assert_eq!(tests.rows, 60);
    assert_eq!(
        tests.results.keys().map(String::as_str).collect::<Vec<_>>(),
        {
            let mut names = PROMOTION_TESTS.to_vec();
            names.sort_unstable();
            names
        },
        "exactly the four tests are recorded"
    );
    assert_eq!(tests.gap(&artifact.digest), None);
    assert_eq!(
        registry
            .get(&artifact.reference)
            .and_then(|card| card.promotion_tests.clone()),
        Some(tests),
        "the results are written on the card the registry holds"
    );
    Ok(())
}

#[test]
fn a_model_no_better_than_predicting_no_move_fails_its_classical_baseline_test() -> Result<()> {
    // The window realises the opposite of what the model was fitted to
    // predict, so its error is larger than the error of predicting nothing.
    let teacher = teacher("bar-linear-wrong-way")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let mut registry = registry_with(&teacher)?;
    let against = window_with("wrong-way-recorded", 200, 260, |a, b, residual| {
        -(0.5 * a - 0.25 * b + 0.1 + residual)
    })?;

    let tests = run_promotion_tests(&InTreeProvider, &mut registry, &artifact, &against, start())?;

    assert!(passed(&tests, REPLAY_TEST), "premise: the replay still ran");
    assert!(
        !passed(&tests, CLASSICAL_BASELINE_TEST),
        "a model worse than the zero-move predictor must fail: {}",
        evidence(&tests, CLASSICAL_BASELINE_TEST)
    );
    assert!(
        tests
            .gap(&artifact.digest)
            .is_some_and(|gap| gap.contains("it failed its classical_baseline test")),
        "the gap must name the baseline test"
    );
    Ok(())
}

#[test]
fn a_model_whose_output_does_not_vary_on_the_window_fails_its_calibration_test() -> Result<()> {
    // Sixty copies of one row: the output is one number sixty times, and no
    // scale maps one number onto what was realised.
    let teacher = teacher("bar-linear-flat")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let mut registry = registry_with(&teacher)?;
    let flat = TrainingDataset::new(
        "flat-recorded",
        vec!["a".to_string(), "b".to_string()],
        vec![vec![0.3, 0.7]; 60],
        (0..60).map(|i| 0.01 * f64::from(i % 5)).collect(),
        (0..60)
            .map(|i| start().saturating_add(Duration::from_mins(i)))
            .collect(),
    )?;

    let tests = run_promotion_tests(&InTreeProvider, &mut registry, &artifact, &flat, start())?;

    assert!(passed(&tests, REPLAY_TEST), "premise: the replay still ran");
    assert!(
        passed(&tests, RISK_ENVELOPE_TEST),
        "premise: every output is finite"
    );
    assert!(
        !passed(&tests, CALIBRATION_TEST),
        "an output that does not vary has no calibration: {}",
        evidence(&tests, CALIBRATION_TEST)
    );
    Ok(())
}

/// A provider whose served model answers by a rule the test chooses,
/// whatever the artifact holds: the way to show a test firing on a served
/// model that misbehaves, which the in-tree provider never does.
#[derive(Debug)]
struct Misbehaving {
    answer: fn(u64) -> f64,
}

#[derive(Debug)]
struct MisbehavingModel {
    reference: String,
    calls: AtomicU64,
    answer: fn(u64) -> f64,
}

impl ModelProvider for Misbehaving {
    fn name(&self) -> &str {
        "the misbehaving provider"
    }
    fn serves(&self) -> &[ModelFormat] {
        &[ModelFormat::DistilledLinear]
    }
    fn serve(&self, artifact: &ModelArtifact) -> Result<Box<dyn ServedModel>> {
        Ok(Box::new(MisbehavingModel {
            reference: artifact.reference.clone(),
            calls: AtomicU64::new(0),
            answer: self.answer,
        }))
    }
}

impl ServedModel for MisbehavingModel {
    fn reference(&self) -> &str {
        &self.reference
    }
    fn format(&self) -> ModelFormat {
        ModelFormat::DistilledLinear
    }
    fn arity(&self) -> usize {
        2
    }
    fn cost(&self) -> usize {
        1
    }
    fn score(&self, _inputs: &[f64]) -> Result<f64> {
        Ok((self.answer)(self.calls.fetch_add(1, Ordering::SeqCst)))
    }
}

#[test]
fn a_served_model_that_does_not_repeat_its_own_output_fails_its_replay_test() -> Result<()> {
    // Two served instances of one artifact, and the second answers
    // differently from the first on the same row: each instance counts its
    // own calls, and the answer depends on a clock nobody recorded.
    static INSTANCES: AtomicU64 = AtomicU64::new(0);
    let teacher = teacher("bar-linear-unrepeatable")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let mut registry = registry_with(&teacher)?;
    let recorded = window("unrepeatable-recorded", 200, 260)?;
    let provider = Misbehaving {
        answer: |call| (call + INSTANCES.fetch_add(1, Ordering::SeqCst)) as f64,
    };

    let tests = run_promotion_tests(&provider, &mut registry, &artifact, &recorded, start())?;

    assert!(
        !passed(&tests, REPLAY_TEST),
        "an output that differs between two servings must fail the replay"
    );
    assert!(
        evidence(&tests, REPLAY_TEST).contains("recorded row 0 scored"),
        "the failure must name the row: {}",
        evidence(&tests, REPLAY_TEST)
    );
    for name in [
        CALIBRATION_TEST,
        CLASSICAL_BASELINE_TEST,
        RISK_ENVELOPE_TEST,
    ] {
        assert!(
            !passed(&tests, name),
            "{name} cannot pass on outputs the replay could not establish"
        );
    }
    Ok(())
}

#[test]
fn a_served_model_whose_score_is_not_finite_fails_its_envelope_test() -> Result<()> {
    // The same NaN from both instances: the replay sees the same bits and
    // passes, so the refusal has to be the envelope's. A NaN compares false
    // against every threshold a deterministic check holds.
    let teacher = teacher("bar-linear-nan")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let mut registry = registry_with(&teacher)?;
    let recorded = window("nan-recorded", 200, 260)?;
    let provider = Misbehaving {
        answer: |call| if call == 7 { f64::NAN } else { call as f64 },
    };

    let tests = run_promotion_tests(&provider, &mut registry, &artifact, &recorded, start())?;

    assert!(
        passed(&tests, REPLAY_TEST),
        "premise: both instances gave the same bits, so the replay passed"
    );
    assert!(
        !passed(&tests, RISK_ENVELOPE_TEST),
        "a score that is not finite must fail the envelope test"
    );
    assert!(
        evidence(&tests, RISK_ENVELOPE_TEST).contains("recorded row 7 scored NaN"),
        "the failure must name the row: {}",
        evidence(&tests, RISK_ENVELOPE_TEST)
    );
    assert!(
        !passed(&tests, CALIBRATION_TEST),
        "a calibration cannot be measured through a NaN"
    );
    Ok(())
}

#[test]
fn the_tests_refuse_to_run_on_a_window_of_other_features_and_write_nothing() -> Result<()> {
    let teacher = teacher("bar-linear-window")?;
    let artifact = InTreeProvider::pack(&teacher)?;
    let mut registry = registry_with(&teacher)?;
    let other = TrainingDataset::new(
        "other-features",
        vec!["a".to_string(), "c".to_string()],
        vec![vec![0.1, 0.2], vec![0.3, 0.4]],
        vec![0.0, 0.1],
        vec![start(), start().saturating_add(Duration::from_mins(1))],
    )?;
    let error = run_promotion_tests(&InTreeProvider, &mut registry, &artifact, &other, start())
        .expect_err("a window of other features must be refused");
    assert!(
        error.message().contains("reads [a, b]") && error.message().contains("holds [a, c]"),
        "the refusal must name both feature lists: {}",
        error.message()
    );

    assert_eq!(
        registry
            .get(&artifact.reference)
            .and_then(|card| card.promotion_tests.clone()),
        None,
        "a refused run writes nothing on the card"
    );
    Ok(())
}
