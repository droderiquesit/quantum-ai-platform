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

use qip_ai::registry::{EvaluationRecord, ModelCard, ModelRegistry};
use qip_core::error::{Error, Result};
use qip_core::{ModelId, Timestamp};
use qip_training::local::{SkillPolicy, TrainedTeacher};
use std::collections::BTreeMap;

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
    .with_purpose(format!(
        "learned function fitted on {}, held out on its last {} observations",
        teacher.dataset(),
        fit.holdout_observations
    ));

    let mut card = match &verdict {
        None => card,
        Some(reason) => card.with_limitation(format!("did not clear the skill bar: {reason}")),
    };
    card.evaluations.push(EvaluationRecord {
        evaluated_at: now,
        dataset: teacher.dataset().to_string(),
        metrics,
        passed,
    });

    let reference = card.reference();
    registry.register(card);

    Ok(ModelRegistration {
        reference,
        passed,
        verdict,
    })
}
