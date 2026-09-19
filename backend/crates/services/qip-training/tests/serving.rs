//! The in-tree provider against what ADR 0083 requires of it.
//!
//! Each of the four forms must round-trip through `pack` and `serve` and
//! score *identically* to the form's own `predict`/`evaluate` on the same
//! inputs — identically, not approximately, because the served score is
//! computed by the same function and a difference of one ulp would mean a
//! second arithmetic had crept in. And each refusal the ADR names — wrong
//! arity, non-finite input, unknown format, wrong digest, a payload the
//! constructor refuses — must refuse before anything is scored.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_ai::serving::{ModelArtifact, ModelFormat, ModelProvider, ServedModel, digest_of};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_strategy::model::{DistilledModel, ModelForm, TreeNode};
use qip_training::dataset::TrainingDataset;
use qip_training::distill::{StudentForm, distil};
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, TrainedTeacher};
use qip_training::serve::{InTreeProvider, TeacherPayload};

fn day(n: i64) -> Timestamp {
    Timestamp::from_civil(2025, 1, 1).saturating_add(Duration::from_days(n))
}

/// A humped target over two features, so a linear teacher, a stump
/// ensemble, a linear student and a tree student all fit something
/// different from one another and none of them is a constant.
fn dataset(name: &str, rows: usize, seed: u64) -> Result<TrainingDataset> {
    let mut rng = Xoshiro256::seeded(seed);
    let mut inputs = Vec::with_capacity(rows);
    let mut targets = Vec::with_capacity(rows);
    let mut times = Vec::with_capacity(rows);
    for index in 0..rows {
        let row = vec![rng.uniform(-1.0, 1.0), rng.uniform(-1.0, 1.0)];
        let core = if row[0].abs() < 0.5 { 1.0 } else { -1.0 };
        targets.push(core + 0.3 * row[1] + rng.normal_with(0.0, 0.05));
        inputs.push(row);
        times.push(day(index as i64));
    }
    TrainingDataset::new(
        name,
        vec!["x0".to_string(), "x1".to_string()],
        inputs,
        targets,
        times,
    )
}

fn teacher(family: ModelFamily) -> Result<TrainedTeacher> {
    let spec = TrainingSpec::new("regime", "v7", "the-model-owner", "fixture", family);
    LocalTrainer::new().fit(&spec, &dataset("fixture", 240, 11)?, day(300))
}

fn linear_teacher() -> Result<TrainedTeacher> {
    teacher(ModelFamily::Linear { ridge: 1e-3 })
}

fn boosted_teacher() -> Result<TrainedTeacher> {
    teacher(ModelFamily::boosted())
}

fn student(form: StudentForm) -> Result<DistilledModel> {
    let teacher = boosted_teacher()?;
    let probe = dataset("probe", 200, 17)?;
    Ok(distil(&teacher, &probe, form, 0.0)?.student().clone())
}

/// Inputs on which no form is flat: a stump ensemble over `x0` steps at
/// thresholds inside `(-1, 1)`, so the points straddle them.
const PROBES: [[f64; 2]; 5] = [
    [0.1, 0.2],
    [-0.7, 0.4],
    [0.62, -0.9],
    [-0.3, 0.0],
    [0.95, 0.95],
];

// --- the four forms round-trip and score identically --------------------------

#[test]
fn a_linear_teacher_served_from_its_artifact_scores_exactly_what_it_predicts() -> Result<()> {
    let teacher = linear_teacher()?;
    let artifact = InTreeProvider::pack(&teacher)?;
    assert_eq!(artifact.format, ModelFormat::TeacherLinear);
    assert_eq!(artifact.reference, "regime@v7");
    let served = InTreeProvider.serve(&artifact)?;
    assert_eq!(served.arity(), teacher.arity());
    assert_eq!(served.cost(), teacher.form().cost());
    let mut distinct = std::collections::BTreeSet::new();
    for probe in PROBES {
        let expected = teacher.predict(&probe)?;
        // Premise: the served model is not a constant.
        distinct.insert(expected.to_bits());
        assert_eq!(served.score(&probe)?.to_bits(), expected.to_bits());
    }
    assert!(
        distinct.len() > 1,
        "the fitted teacher is flat over the probes"
    );
    Ok(())
}

#[test]
fn a_boosted_teacher_served_from_its_artifact_scores_exactly_what_it_predicts() -> Result<()> {
    let teacher = boosted_teacher()?;
    let artifact = InTreeProvider::pack(&teacher)?;
    assert_eq!(artifact.format, ModelFormat::TeacherBoostedStumps);
    let served = InTreeProvider.serve(&artifact)?;
    // The declared arity is the teacher's, which for an ensemble may exceed
    // the highest feature any stump consults; the served model must refuse
    // the same counts the teacher refuses, not fewer.
    assert_eq!(served.arity(), teacher.arity());
    assert_eq!(served.arity(), 2);
    let mut distinct = std::collections::BTreeSet::new();
    for probe in PROBES {
        let expected = teacher.predict(&probe)?;
        distinct.insert(expected.to_bits());
        assert_eq!(served.score(&probe)?.to_bits(), expected.to_bits());
    }
    assert!(
        distinct.len() > 1,
        "the fitted ensemble is flat over the probes"
    );
    Ok(())
}

#[test]
fn a_linear_student_served_from_its_artifact_scores_exactly_what_it_evaluates() -> Result<()> {
    let model = student(StudentForm::Linear { ridge: 1e-3 })?;
    assert!(matches!(model.form(), ModelForm::Linear { .. }));
    let artifact = InTreeProvider::pack_distilled("regime-student@v7", &model)?;
    assert_eq!(artifact.format, ModelFormat::DistilledLinear);
    let served = InTreeProvider.serve(&artifact)?;
    assert_eq!(served.arity(), model.arity());
    assert_eq!(served.cost(), model.cost());
    let mut distinct = std::collections::BTreeSet::new();
    for probe in PROBES {
        let expected = model.evaluate(&probe)?;
        distinct.insert(expected.to_bits());
        assert_eq!(served.score(&probe)?.to_bits(), expected.to_bits());
    }
    assert!(
        distinct.len() > 1,
        "the linear student is flat over the probes"
    );
    Ok(())
}

#[test]
fn a_tree_student_served_from_its_artifact_scores_exactly_what_it_evaluates() -> Result<()> {
    let model = student(StudentForm::shallow_tree())?;
    assert!(matches!(model.form(), ModelForm::Tree { .. }));
    let artifact = InTreeProvider::pack_distilled("regime-student@v7", &model)?;
    assert_eq!(artifact.format, ModelFormat::DistilledTree);
    let served = InTreeProvider.serve(&artifact)?;
    assert_eq!(served.arity(), model.arity());
    assert_eq!(served.cost(), model.cost());
    let mut distinct = std::collections::BTreeSet::new();
    for probe in PROBES {
        let expected = model.evaluate(&probe)?;
        distinct.insert(expected.to_bits());
        assert_eq!(served.score(&probe)?.to_bits(), expected.to_bits());
    }
    assert!(
        distinct.len() > 1,
        "the tree student is flat over the probes"
    );
    Ok(())
}

// --- refusals at score time ----------------------------------------------------

#[test]
fn a_served_model_refuses_the_wrong_number_of_inputs_naming_both_counts() -> Result<()> {
    let served: Vec<Box<dyn ServedModel>> = vec![
        InTreeProvider.serve(&InTreeProvider::pack(&linear_teacher()?)?)?,
        InTreeProvider.serve(&InTreeProvider::pack(&boosted_teacher()?)?)?,
        InTreeProvider.serve(&InTreeProvider::pack_distilled(
            "s@1",
            &student(StudentForm::Linear { ridge: 1e-3 })?,
        )?)?,
        InTreeProvider.serve(&InTreeProvider::pack_distilled(
            "s@1",
            &student(StudentForm::shallow_tree())?,
        )?)?,
    ];
    for model in &served {
        // Premise: the right count scores.
        assert!(model.score(&[0.1, 0.2]).is_ok(), "{model:?}");
        assert_eq!(model.arity(), 2);
        let error = model
            .score(&[0.1, 0.2, 0.3])
            .expect_err("three inputs to a two-input model were scored");
        let message = error.message();
        assert!(
            message.contains("2 input") && message.contains("given 3"),
            "{model:?}: the refusal did not name both counts: {message}"
        );
        // One too few is refused too — a short row is not padded.
        assert!(
            model.score(&[0.1]).is_err(),
            "{model:?}: a short row was scored"
        );
    }
    Ok(())
}

#[test]
fn a_served_model_refuses_a_non_finite_input_rather_than_scoring_it() -> Result<()> {
    let served: Vec<Box<dyn ServedModel>> = vec![
        InTreeProvider.serve(&InTreeProvider::pack(&linear_teacher()?)?)?,
        InTreeProvider.serve(&InTreeProvider::pack(&boosted_teacher()?)?)?,
        InTreeProvider.serve(&InTreeProvider::pack_distilled(
            "s@1",
            &student(StudentForm::Linear { ridge: 1e-3 })?,
        )?)?,
        InTreeProvider.serve(&InTreeProvider::pack_distilled(
            "s@1",
            &student(StudentForm::shallow_tree())?,
        )?)?,
    ];
    for model in &served {
        assert!(
            model.score(&[0.1, 0.2]).is_ok(),
            "the premise failed: {model:?}"
        );
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let error = model
                .score(&[bad, 0.2])
                .expect_err("a non-finite input was scored");
            assert!(
                error.message().contains("non-finite"),
                "{model:?}: {}",
                error.message()
            );
        }
    }
    Ok(())
}

// --- refusals at serve time -----------------------------------------------------

#[test]
fn an_artifact_of_an_unknown_format_is_refused_at_deserialisation_with_the_fixed_sentence()
-> Result<()> {
    let genuine = InTreeProvider::pack(&linear_teacher()?)?;
    let json = serde_json::to_string(&genuine)?;
    assert!(
        json.contains(r#""format":"teacher_linear""#),
        "the premise failed: {json}"
    );
    let foreign = json.replace(r#""format":"teacher_linear""#, r#""format":"onnx""#);
    assert_ne!(foreign, json);
    // The typed path: serde refuses the variant, no `#[serde(other)]`.
    assert!(
        serde_json::from_str::<ModelArtifact>(&foreign).is_err(),
        "an artifact naming `onnx` deserialised"
    );
    // The loader: the same refusal, in the sentence ADR 0083 fixes.
    let error = ModelArtifact::from_json(&foreign).expect_err("an unknown format loaded");
    assert!(
        error.message().contains(
            "model artifact `regime@v7` is `onnx`, which this platform does not serve \
             in-process; it serves `teacher_linear`, `teacher_boosted_stumps`, \
             `distilled_linear` and `distilled_tree`. Distil the model with \
             `qip_training::distill` into one of those, or mark its card research-only"
        ),
        "{}",
        error.message()
    );
    // And the genuine body loads and serves.
    let loaded = ModelArtifact::from_json(&json)?;
    assert_eq!(loaded, genuine);
    assert!(InTreeProvider.serve(&loaded).is_ok());
    Ok(())
}

#[test]
fn an_artifact_whose_digest_is_not_its_payloads_is_refused_naming_both_digests() -> Result<()> {
    let genuine = InTreeProvider::pack(&boosted_teacher()?)?;
    assert!(InTreeProvider.serve(&genuine).is_ok(), "the premise failed");

    // The payload was swapped after the digest was taken: nudge a stump.
    let mut swapped = genuine.clone();
    let stumps = swapped.payload["form"]["boosted_stumps"]["stumps"]
        .as_array_mut()
        .expect("an ensemble payload carries a stump array");
    stumps[0]["below"] = serde_json::json!(42.0);
    assert_ne!(swapped.payload, genuine.payload, "the premise failed");

    let error = InTreeProvider
        .serve(&swapped)
        .expect_err("a payload that does not match its digest was served");
    let message = error.message();
    assert!(message.contains(&genuine.digest), "{message}");
    assert!(message.contains(&digest_of(&swapped.payload)), "{message}");
    assert_eq!(error.code(), "denied");
    Ok(())
}

#[test]
fn a_payload_the_forms_constructor_refuses_is_refused_at_serve_not_at_score() -> Result<()> {
    // A tree that descends backwards is a loop, and a loop has no worst-case
    // cost. `DistilledModel::tree` refuses it; a deserialised value has been
    // through no constructor, so the provider rebuilds through it.
    let looping = serde_json::json!({
        "name": "loop",
        "form": {"tree": {"arity": 1, "nodes": [
            {"branch": {"input": 0, "threshold": 0.0, "below": 1, "at_or_above": 1}},
            {"branch": {"input": 0, "threshold": 0.0, "below": 0, "at_or_above": 0}}
        ]}}
    });
    let artifact = ModelArtifact::new("loop@1", ModelFormat::DistilledTree, looping);
    let error = InTreeProvider
        .serve(&artifact)
        .expect_err("a backwards tree was served");
    assert!(
        error.message().contains("not a later node"),
        "the refusal was not the constructor's: {}",
        error.message()
    );

    // An empty coefficient vector, likewise.
    let empty = serde_json::json!({"name": "empty", "form": {"linear": {"intercept": 0.0, "coefficients": []}}});
    let artifact = ModelArtifact::new("empty@1", ModelFormat::DistilledLinear, empty);
    assert!(
        InTreeProvider.serve(&artifact).is_err(),
        "an empty model was served"
    );

    // A teacher whose stump reads an input beyond its declared arity.
    let payload = TeacherPayload {
        arity: 1,
        form: qip_training::local::TeacherForm::BoostedStumps {
            base: 0.0,
            learning_rate: 0.1,
            stumps: vec![qip_training::local::Stump {
                feature: 3,
                threshold: 0.0,
                below: -1.0,
                at_or_above: 1.0,
            }],
        },
        calibration: qip_training::local::Calibration::default(),
    };
    let artifact = ModelArtifact::new(
        "wide@1",
        ModelFormat::TeacherBoostedStumps,
        serde_json::to_value(&payload)?,
    );
    let error = InTreeProvider
        .serve(&artifact)
        .expect_err("a stump reading past the arity was served");
    assert!(
        error.message().contains("reads input 3 of 1"),
        "{}",
        error.message()
    );

    // Premise: a well-formed hand-built tree of the same shape serves.
    let sound = DistilledModel::tree(
        "sound",
        1,
        vec![
            TreeNode::Branch {
                input: 0,
                threshold: 0.0,
                below: 1,
                at_or_above: 2,
            },
            TreeNode::Leaf { value: -1.0 },
            TreeNode::Leaf { value: 1.0 },
        ],
    )?;
    let artifact = InTreeProvider::pack_distilled("sound@1", &sound)?;
    assert_eq!(InTreeProvider.serve(&artifact)?.score(&[0.5])?, 1.0);
    Ok(())
}

#[test]
fn an_artifact_declaring_one_format_and_carrying_another_is_refused() -> Result<()> {
    let linear = InTreeProvider::pack(&linear_teacher()?)?;
    assert!(InTreeProvider.serve(&linear).is_ok(), "the premise failed");
    let mislabelled = ModelArtifact::new(
        linear.reference.clone(),
        ModelFormat::TeacherBoostedStumps,
        linear.payload.clone(),
    );
    let error = InTreeProvider
        .serve(&mislabelled)
        .expect_err("a linear payload served as an ensemble");
    assert!(
        error
            .message()
            .contains("declares `teacher_boosted_stumps` and its payload is `teacher_linear`"),
        "{}",
        error.message()
    );

    let tree = InTreeProvider::pack_distilled("s@1", &student(StudentForm::shallow_tree())?)?;
    let mislabelled = ModelArtifact::new("s@1", ModelFormat::DistilledLinear, tree.payload.clone());
    assert!(
        InTreeProvider.serve(&mislabelled).is_err(),
        "a tree payload served as a linear model"
    );
    // A teacher payload under a distilled format is refused as a payload the
    // distilled form cannot read, not scored as something else.
    let crossed = ModelArtifact::new("s@1", ModelFormat::DistilledLinear, linear.payload.clone());
    assert!(InTreeProvider.serve(&crossed).is_err());
    Ok(())
}

// --- the digest -------------------------------------------------------------------

#[test]
fn a_packed_artifacts_digest_is_a_recomputation_over_its_own_payload() -> Result<()> {
    for artifact in [
        InTreeProvider::pack(&linear_teacher()?)?,
        InTreeProvider::pack(&boosted_teacher()?)?,
        InTreeProvider::pack_distilled("s@1", &student(StudentForm::Linear { ridge: 1e-3 })?)?,
        InTreeProvider::pack_distilled("s@1", &student(StudentForm::shallow_tree())?)?,
    ] {
        assert_eq!(
            artifact.digest,
            digest_of(&artifact.payload),
            "{artifact:?}"
        );
        assert_eq!(artifact.digest.len(), 64);
        artifact.verify_digest()?;
        // The digest survives the wire: the same payload re-read from JSON,
        // whatever key order the writer used, digests the same.
        let reread: ModelArtifact = serde_json::from_str(&serde_json::to_string(&artifact)?)?;
        assert_eq!(reread.digest, digest_of(&reread.payload));
    }
    // And two different models do not share one — or the assertions above
    // would hold of a constant.
    let one = InTreeProvider::pack(&linear_teacher()?)?;
    let two = InTreeProvider::pack(&boosted_teacher()?)?;
    assert_ne!(one.digest, two.digest);
    Ok(())
}

#[test]
fn the_in_tree_provider_serves_every_format_the_enum_names_and_says_so() {
    assert_eq!(InTreeProvider.serves(), &ModelFormat::ALL);
    assert_eq!(InTreeProvider.name(), "the in-tree provider");
}
