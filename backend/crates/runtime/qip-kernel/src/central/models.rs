//! Where a fitted model becomes a model the platform may decide with.
//!
//! The companion to [`crate::central::foundry`], and the same kind of gap:
//! `qip-training` fits models and [`ModelRegistry`] governs which ones may
//! inform a decision, and nothing joined them. A fit could be excellent or
//! worthless and the registry would never learn which.
//!
//! # A fit with no skill must not become a decision-eligible model
//!
//! `qip-training` is built around one refusal.
//! [`FitDiagnostics::skill_verdict`] exists because a boosted ensemble drives
//! *training* error to nothing on pure noise, so in-sample error is not
//! evidence of anything; only the held-out tail is. A model with no signal
//! scores slightly below zero out of sample, which is why the default bar sits
//! comfortably above it.
//!
//! [`ModelCard::decision_eligibility`] is built around the matching
//! requirement: a card with no passed evaluation may not inform a decision.
//!
//! Both halves were already right. What was missing is that nothing obliged
//! the evaluation on the card to be *the holdout verdict of the fit it
//! describes*. A caller could fit a noise model, write a card asserting
//! `passed: true`, and the registry would allow it to make decisions — every
//! individual component behaving exactly as designed.
//!
//! So this module never takes `passed` as an argument. It reads the fit's own
//! diagnostics, applies the skill policy, and writes the verdict it gets. A
//! caller cannot assert skill, which means a caller cannot assert it falsely.
//!
//! # What it deliberately does not do
//!
//! * **It does not promote to production.** A registered card enters at
//!   [`ModelStage::Development`] with its evaluation attached. Moving a model
//!   to a stage that allows decisions stays a governed act elsewhere; this
//!   module only ensures the evidence is true when that decision is taken.
//! * **It does not fit.** Training happens in `qip-training`, against a
//!   dataset this module never sees. A seam that also fitted would be marking
//!   its own homework, exactly as the foundry would if it scored its own
//!   candidates.

use qip_ai::registry::{
    CALIBRATION_TEST, CLASSICAL_BASELINE_TEST, EvaluationRecord, ModelCard, ModelRegistry,
    PromotionTest, PromotionTests, REPLAY_TEST, RISK_ENVELOPE_TEST, ResourceBudget,
};
use qip_ai::serving::{ModelArtifact, ModelProvider};
use qip_core::error::{Error, Result};
use qip_core::{ModelId, Timestamp};
use qip_training::dataset::TrainingDataset;
use qip_training::local::{SkillPolicy, TrainedTeacher};
use std::collections::BTreeMap;

/// The ceiling a fitted in-tree model is admitted under: one inference in a
/// millisecond and a megabyte resident. A declared limit rather than a
/// measurement, recorded so the serving layer has a number to hold the
/// model to. Nothing in the registry enforces it.
pub const DEFAULT_RESOURCE_BUDGET: ResourceBudget = ResourceBudget {
    max_inference_micros: 1_000,
    max_memory_bytes: 1_048_576,
};
const CALIBRATION_SCALE: &str = "calibration_scale";

/// The card parameter naming the one subject a model was fitted on
/// (R12-MODEL-10096), so the structural-break critic judges the model against
/// its own market's regime crossings and not every watched subject's.
pub const MODEL_SUBJECT: &str = "subject";

/// Record on `card` the subject it was fitted on. Refuses a blank subject,
/// and a card that already names a different one: a model is fitted on one
/// tape, and a card claiming two would be judged against whichever crossing
/// was read first.
pub fn record_subject(card: &mut ModelCard, subject: &str) -> Result<()> {
    if subject.trim().is_empty() {
        return Err(Error::invalid(format!(
            "{}: a model's subject must be named; leave it unrecorded instead, and the \
             card is judged against every subject's crossings",
            card.reference()
        )));
    }
    if let Some(held) = card.parameters.get(MODEL_SUBJECT)
        && held != subject
    {
        return Err(Error::invalid(format!(
            "{} already names subject {held}, so it cannot also be fitted on {subject}",
            card.reference()
        )));
    }
    card.parameters
        .insert(MODEL_SUBJECT.to_string(), subject.to_string());
    Ok(())
}

/// The subject `card` records it was fitted on, if any.
pub fn subject_of(card: &ModelCard) -> Option<&str> {
    card.parameters.get(MODEL_SUBJECT).map(String::as_str)
}
const CALIBRATION_OFFSET: &str = "calibration_offset";

/// Refuse to evaluate a candidate unless the classical baseline it is judged
/// against is on record with a fitted calibration (ADR 0006, MODEL-031).
///
/// A challenger scored against a baseline nobody recorded, or one left at the
/// identity calibration every fit starts with, is scored against a number the
/// record cannot reproduce — and "it beat the baseline" is then an assertion,
/// not a result.
pub fn require_calibrated_baseline(registry: &ModelRegistry, reference: &str) -> Result<()> {
    let card = registry.get(reference).ok_or_else(|| {
        Error::denied(format!(
            "no baseline {reference} is on record; fit and register the classical baseline \
             before evaluating any candidate against it"
        ))
    })?;
    let read = |key: &str| {
        card.parameters
            .get(key)
            .and_then(|text| text.parse::<f64>().ok())
    };
    match (read(CALIBRATION_SCALE), read(CALIBRATION_OFFSET)) {
        (Some(scale), Some(offset))
            if !qip_training::local::Calibration { scale, offset }.is_identity() =>
        {
            Ok(())
        }
        _ => Err(Error::denied(format!(
            "baseline {reference} is on record without a fitted calibration; calibrate it \
             (Calibration::fit) before evaluating any candidate against it"
        ))),
    }
}

/// The measured error a candidate must beat its baseline on: the holdout
/// root-mean-square error [`register_fit`] writes on every card. Lower is
/// better.
const COMPARED_METRIC: &str = "holdout_rmse";

/// Refuse a candidate whose measured result does not beat its calibrated
/// classical baseline (ADR 0006, R12-QUANT-4018).
///
/// [`require_calibrated_baseline`] proves the baseline is on record and
/// calibrated; it never compared the two, so a candidate that lost to its
/// baseline was promoted all the same and "it was evaluated against the
/// baseline" was true of a candidate the baseline beat. This compares each
/// card's latest [`COMPARED_METRIC`] and admits only a strictly lower error.
/// The two must be measured on the same dataset: an error on different data
/// is not a comparison. A missing card, evaluation or figure, or one that is
/// not finite, is refused rather than read as a win or a loss.
pub fn require_beats_baseline(
    registry: &ModelRegistry,
    candidate: &str,
    baseline: &str,
) -> Result<()> {
    let measured = |reference: &str| -> Result<(String, f64)> {
        let evaluation = registry
            .get(reference)
            .and_then(|card| card.latest_evaluation())
            .ok_or_else(|| {
                Error::denied(format!(
                    "{reference} has no evaluation on record, so it cannot be compared; \
                     register its fit before comparing it"
                ))
            })?;
        match evaluation.metrics.get(COMPARED_METRIC) {
            Some(value) if value.is_finite() => Ok((evaluation.dataset.clone(), *value)),
            _ => Err(Error::denied(format!(
                "{reference}'s latest evaluation states no finite {COMPARED_METRIC}, so it \
                 cannot be compared"
            ))),
        }
    };
    let (candidate_data, candidate_error) = measured(candidate)?;
    let (baseline_data, baseline_error) = measured(baseline)?;
    if candidate_data != baseline_data {
        return Err(Error::denied(format!(
            "{candidate} was measured on {candidate_data} and its baseline {baseline} on \
             {baseline_data}; measure both on the same dataset before comparing them"
        )));
    }
    if candidate_error < baseline_error {
        Ok(())
    } else {
        Err(Error::denied(format!(
            "{candidate} does not beat its baseline {baseline}: {COMPARED_METRIC} \
             {candidate_error} against {baseline_error} on {candidate_data}"
        )))
    }
}

/// What registering a fit produced.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRegistration {
    /// `{name}@{version}`, the reference a strategy candidate carries.
    pub reference: String,
    /// Whether the fit cleared the skill bar. A registration is recorded
    /// either way — a model that failed is a fact worth keeping, and deleting
    /// it would let a search retry until something passed without the failures
    /// being visible.
    pub passed: bool,
    /// Why, when it did not. `None` when it did.
    pub verdict: Option<String>,
}

impl ModelRegistration {
    pub fn summarise(&self) -> String {
        match &self.verdict {
            None => format!("{} cleared the skill bar", self.reference),
            Some(reason) => format!("{} did not clear the skill bar: {reason}", self.reference),
        }
    }
}

/// Register a fitted model with the platform's registry, carrying its own
/// holdout verdict as the evaluation.
///
/// There is deliberately no `passed` parameter. The verdict comes from
/// [`FitDiagnostics::skill_verdict`] against `policy`, so the evaluation on the
/// card is the fit's own out-of-sample result and not a claim about it.
///
/// A teacher whose structure was replaced and not refitted carries no
/// diagnostics at all, and is refused rather than registered as unevaluated: a
/// new functional form must not inherit the standing of the one it replaced,
/// and an unevaluated card is one `decision_eligibility` would reject later
/// with a message about a missing evaluation rather than about the real
/// problem.
pub fn register_fit(
    registry: &mut ModelRegistry,
    teacher: &TrainedTeacher,
    policy: &SkillPolicy,
    owner: impl Into<String>,
    now: Timestamp,
) -> Result<ModelRegistration> {
    let fit = teacher.fit().ok_or_else(|| {
        Error::invalid(format!(
            "{} carries no fit diagnostics: its structure was replaced and it has not been \
             refitted, so there is no out-of-sample result that describes this function",
            teacher.reference()
        ))
    })?;

    let verdict = fit.skill_verdict(policy).err();
    let passed = verdict.is_none();

    let mut metrics = BTreeMap::new();
    metrics.insert("holdout_r2".to_string(), fit.holdout_r2);
    metrics.insert("holdout_rmse".to_string(), fit.holdout_rmse);
    metrics.insert("holdout_correlation".to_string(), fit.holdout_correlation);
    // Recorded beside the holdout figures rather than omitted, because the gap
    // between them is the thing a reviewer should look at: a large in-sample
    // R² next to a negative holdout one is the signature of a fit that
    // memorised its training set.
    metrics.insert("in_sample_r2".to_string(), fit.in_sample_r2);
    metrics.insert("in_sample_rmse".to_string(), fit.in_sample_rmse);
    metrics.insert(
        "holdout_observations".to_string(),
        fit.holdout_observations as f64,
    );

    let card = ModelCard::new(
        ModelId::from_string(teacher.reference()),
        teacher.name(),
        teacher.version(),
        owner,
        now,
    )
    .with_features(teacher.feature_names().to_vec())
    .with_training_data(vec![teacher.dataset().to_string()])
    // The bar the verdict above is measured against, by name, so the card
    // says what the model was meant to beat and not only what it scored.
    .with_benchmark(format!(
        "holdout R-squared over the mean predictor on {}",
        teacher.dataset()
    ))
    .with_resource_budget(DEFAULT_RESOURCE_BUDGET)
    .with_purpose(format!(
        "learned function fitted on {}, held out on its last {} observations",
        teacher.dataset(),
        fit.holdout_observations
    ));

    let mut card = match &verdict {
        None => card,
        Some(reason) => card.with_limitation(format!("did not clear the skill bar: {reason}")),
    };
    // The calibration is part of the function the card describes: two fits
    // with the same coefficients and different calibrations predict
    // differently, and a baseline whose calibration is not on its card cannot
    // be shown to have been calibrated at all.
    card.parameters.insert(
        CALIBRATION_SCALE.to_string(),
        format!("{}", teacher.calibration().scale),
    );
    card.parameters.insert(
        CALIBRATION_OFFSET.to_string(),
        format!("{}", teacher.calibration().offset),
    );
    card.evaluations.push(EvaluationRecord {
        evaluated_at: now,
        dataset: teacher.dataset().to_string(),
        metrics,
        passed,
    });

    let reference = card.reference();
    registry.register(card)?;

    Ok(ModelRegistration {
        reference,
        passed,
        verdict,
    })
}

/// Run the four promotion tests on `artifact` over a recorded window and
/// write the results on its card (ADR 0392, R12-MODEL-10202).
///
/// The failure it prevents: a model was promoted on a passing evaluation
/// alone. Nothing had run the bytes again over recorded inputs, measured a
/// calibration for them, compared them with a classical predictor, or shown
/// that their output is a number a deterministic check can carry.
///
/// The caller supplies the record and nothing else: rows the process
/// observed, with the value each row's next bar realised. The verdicts are
/// computed here, through `provider` (the one that will serve the artifact,
/// so there is no second arithmetic), and bound to the artifact's digest.
///
/// - **replay**: two separately served instances score every recorded row
///   to the same bits.
/// - **calibration**: a least-squares scale and offset from the outputs to
///   the realised values can be measured (`Calibration::fit`). An output
///   that does not vary has none.
/// - **classical baseline**: the outputs' root-mean-square error is strictly
///   below the zero-move predictor's on the same rows. The zero-move
///   predictor is the classical no-skill forecast of a return; the window's
///   own mean would be computed from the values it is scored on.
/// - **envelope**: every output is finite. A non-finite score compares false
///   against every deterministic threshold, which reads as a quiet decision
///   and not as a breach. The largest output and the largest recorded move
///   are written beside it as a measurement and judged by nothing: a bound
///   on magnitude is a figure, and nobody has given one.
///
/// A failed test is a result written on the card, so the promotion refusal
/// names it. An `Err` is a window the tests cannot run on at all — no card,
/// a window naming other features, an artifact the provider will not serve
/// — and writes nothing. A `TrainingDataset` holds at least one row by
/// construction, so there is no empty window to refuse here.
pub fn run_promotion_tests(
    provider: &dyn ModelProvider,
    registry: &mut ModelRegistry,
    artifact: &ModelArtifact,
    recorded: &TrainingDataset,
    now: Timestamp,
) -> Result<PromotionTests> {
    let reference = &artifact.reference;
    let card = registry
        .get(reference)
        .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
    if card.features != recorded.feature_names() {
        return Err(Error::invalid(format!(
            "{reference} reads [{}] and the recorded window {} holds [{}]; the promotion tests \
             run on rows of the features the card names",
            card.features.join(", "),
            recorded.name(),
            recorded.feature_names().join(", ")
        )));
    }
    let rows = recorded.rows();
    let realised = recorded.targets();
    let first = provider.serve(artifact)?;
    let second = provider.serve(artifact)?;

    let mut outputs = Vec::with_capacity(rows.len());
    let mut replay_failure = None;
    for (index, row) in rows.iter().enumerate() {
        match (first.score(row), second.score(row)) {
            (Ok(one), Ok(other)) if one.to_bits() == other.to_bits() => outputs.push(one),
            (Ok(one), Ok(other)) => {
                replay_failure = Some(format!(
                    "recorded row {index} scored {one} and then {other}; a model that does not \
                     repeat its own output cannot be replayed"
                ));
                break;
            }
            (Err(error), _) | (_, Err(error)) => {
                replay_failure = Some(format!(
                    "recorded row {index} could not be scored: {}",
                    error.message()
                ));
                break;
            }
        }
    }

    let mut results = BTreeMap::new();
    let mut record = |name: &str, passed: bool, evidence: String| {
        results.insert(name.to_string(), PromotionTest { passed, evidence });
    };
    match replay_failure {
        Some(failure) => {
            let unmeasured =
                || "not measured: the replay did not score every recorded row".to_string();
            record(REPLAY_TEST, false, failure);
            record(CALIBRATION_TEST, false, unmeasured());
            record(CLASSICAL_BASELINE_TEST, false, unmeasured());
            record(RISK_ENVELOPE_TEST, false, unmeasured());
        }
        None => {
            let mut bits = Vec::with_capacity(outputs.len() * 8);
            for output in &outputs {
                bits.extend_from_slice(&output.to_bits().to_be_bytes());
            }
            record(
                REPLAY_TEST,
                true,
                format!(
                    "{} recorded row(s) of {} scored twice by {} to the same bits (outputs {})",
                    rows.len(),
                    recorded.name(),
                    provider.name(),
                    qip_core::hash::sha256_hex(&bits)
                ),
            );
            // The envelope before the two measurements that need arithmetic
            // on the outputs: a non-finite output makes both meaningless.
            let stray = outputs.iter().position(|output| !output.is_finite());
            let largest = |values: &[f64]| values.iter().fold(0.0_f64, |max, v| max.max(v.abs()));
            match stray {
                Some(index) => record(
                    RISK_ENVELOPE_TEST,
                    false,
                    format!(
                        "recorded row {index} scored {}; a score that is not finite compares \
                         false against every deterministic threshold",
                        outputs[index]
                    ),
                ),
                None => record(
                    RISK_ENVELOPE_TEST,
                    true,
                    format!(
                        "every output finite on {} row(s); largest output {:.6} beside a \
                         largest recorded move of {:.6} (measured, not judged)",
                        rows.len(),
                        largest(&outputs),
                        largest(realised)
                    ),
                ),
            }
            match qip_training::local::Calibration::fit(&outputs, realised) {
                Ok(calibration) if stray.is_none() => record(
                    CALIBRATION_TEST,
                    true,
                    format!(
                        "realised = {:.6} x output + {:.6} over {} row(s)",
                        calibration.scale,
                        calibration.offset,
                        rows.len()
                    ),
                ),
                Ok(_) => record(
                    CALIBRATION_TEST,
                    false,
                    "not measured: an output on the window is not finite".to_string(),
                ),
                Err(error) => record(CALIBRATION_TEST, false, error.message().to_string()),
            }
            let error_of = |scores: &[f64]| {
                let squared: f64 = scores
                    .iter()
                    .zip(realised)
                    .map(|(score, target)| (score - target).powi(2))
                    .sum();
                (squared / scores.len() as f64).sqrt()
            };
            let model_rmse = error_of(&outputs);
            let zero_move_rmse = error_of(&vec![0.0; outputs.len()]);
            // Strictly less, as a `partial_cmp`: an incomparable pair (a NaN
            // error) fails the test, never passes it.
            let beats = matches!(
                model_rmse.partial_cmp(&zero_move_rmse),
                Some(std::cmp::Ordering::Less)
            );
            record(
                CLASSICAL_BASELINE_TEST,
                beats,
                format!(
                    "rmse {model_rmse:.6} against the zero-move predictor's {zero_move_rmse:.6} \
                     on {} row(s){}",
                    rows.len(),
                    if beats {
                        ""
                    } else {
                        "; a model that is not strictly better than predicting no move has \
                         shown nothing over the classical forecast"
                    }
                ),
            );
        }
    }
    let tests = PromotionTests {
        artifact_digest: artifact.digest.clone(),
        window: recorded.name().to_string(),
        rows: rows.len(),
        ran_at: now,
        results,
    };
    let card = registry
        .get_mut(reference)
        .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
    card.promotion_tests = Some(tests.clone());
    Ok(tests)
}
