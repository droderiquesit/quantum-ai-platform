//! The in-tree trainer: a fit that actually fits.
//!
//! Two families, both fitted properly and neither a placeholder:
//!
//! * **Linear**, by ridge-regularised least squares. The normal equations are
//!   assembled and solved with [`qip_numerics`]; on data generated from known
//!   coefficients it recovers them.
//! * **Boosted stumps**, by greedy gradient boosting on squared error. Each
//!   round searches every feature and a set of candidate thresholds for the
//!   split that removes the most residual variance, exactly as a regression
//!   tree does, and adds it at a learning rate.
//!
//! The second family is the reason [`crate::distill`] exists. An ensemble of
//! sixty stumps is a perfectly good model and completely inadmissible on the
//! execution path: its cost grows with the number of rounds, which is a
//! hyperparameter rather than a bound. It is a teacher, not a strategy.
//!
//! **Every fit reports its holdout, and the holdout is the last slice in time.**
//! A model is not allowed to claim skill from its own training error, because
//! a boosted ensemble can drive training error to nothing on pure noise, and
//! [`FitDiagnostics::skill_verdict`] is what stops that reading as a discovery.
//!
//! Nothing here reads a clock or draws from an ambient source of randomness.
//! The fit is deterministic given the data and the specification; the
//! timestamps it records are the ones the caller passed in.

use crate::cadence::AuthorisedUpdate;
use crate::dataset::TrainingDataset;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_numerics::Matrix;
use qip_numerics::stats;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What kind of model to fit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFamily {
    /// Least squares with an L2 penalty. `ridge` of zero is ordinary least
    /// squares; anything above it trades a little bias for a lot of variance
    /// on collinear features, which financial features invariably are.
    Linear { ridge: f64 },
    /// Gradient boosting over depth-one regression trees.
    BoostedStumps {
        rounds: usize,
        learning_rate: f64,
        /// Smallest number of observations either side of a split. A split
        /// that isolates three rows has fitted three rows.
        min_samples_leaf: usize,
        /// Candidate thresholds examined per feature per round.
        candidate_splits: usize,
    },
}

impl ModelFamily {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Linear { .. } => "linear",
            Self::BoostedStumps { .. } => "boosted_stumps",
        }
    }

    /// A sensible boosted configuration, so a caller does not have to invent
    /// four numbers to get started.
    pub const fn boosted() -> Self {
        Self::BoostedStumps {
            rounds: 60,
            learning_rate: 0.1,
            min_samples_leaf: 12,
            candidate_splits: 16,
        }
    }

    fn record(&self, into: &mut BTreeMap<String, String>) {
        into.insert("family".to_string(), self.as_str().to_string());
        match self {
            Self::Linear { ridge } => {
                into.insert("ridge".to_string(), format!("{ridge}"));
            }
            Self::BoostedStumps {
                rounds,
                learning_rate,
                min_samples_leaf,
                candidate_splits,
            } => {
                into.insert("rounds".to_string(), rounds.to_string());
                into.insert("learning_rate".to_string(), format!("{learning_rate}"));
                into.insert("min_samples_leaf".to_string(), min_samples_leaf.to_string());
                into.insert("candidate_splits".to_string(), candidate_splits.to_string());
            }
        }
    }
}

/// One depth-one regression tree.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stump {
    pub feature: usize,
    pub threshold: f64,
    /// Fitted value where the feature is below the threshold.
    pub below: f64,
    /// Fitted value where it is at or above it.
    pub at_or_above: f64,
}

impl Stump {
    pub fn evaluate(&self, inputs: &[f64]) -> f64 {
        let value = inputs.get(self.feature).copied().unwrap_or(0.0);
        if value < self.threshold {
            self.below
        } else {
            self.at_or_above
        }
    }
}

/// The functional form a fit produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeacherForm {
    Linear {
        intercept: f64,
        coefficients: Vec<f64>,
    },
    /// `base + learning_rate · Σ stump(x)`.
    BoostedStumps {
        base: f64,
        learning_rate: f64,
        stumps: Vec<Stump>,
    },
}

impl TeacherForm {
    pub fn arity(&self) -> usize {
        match self {
            Self::Linear { coefficients, .. } => coefficients.len(),
            Self::BoostedStumps { stumps, .. } => {
                stumps.iter().map(|s| s.feature + 1).max().unwrap_or(0)
            }
        }
    }

    /// The values a weights-only update may replace, in a fixed order.
    ///
    /// For a linear model, its coefficients. For an ensemble, the two fitted
    /// values of each stump — *not* the thresholds or the features, which are
    /// where the ensemble's structure lives.
    pub fn weights(&self) -> Vec<f64> {
        match self {
            Self::Linear { coefficients, .. } => coefficients.clone(),
            Self::BoostedStumps { stumps, .. } => stumps
                .iter()
                .flat_map(|stump| [stump.below, stump.at_or_above])
                .collect(),
        }
    }

    fn set_weights(&mut self, weights: &[f64]) -> Result<()> {
        if weights.len() != self.weights().len() {
            return Err(Error::invalid(format!(
                "this model has {} weight(s) and the update carries {}; changing how many \
                 weights a model has is a change to its structure, not to its weights",
                self.weights().len(),
                weights.len()
            )));
        }
        if let Some(index) = weights.iter().position(|w| !w.is_finite()) {
            return Err(Error::numeric(format!("weight {index} is not finite")));
        }
        match self {
            Self::Linear { coefficients, .. } => {
                coefficients.copy_from_slice(weights);
            }
            Self::BoostedStumps { stumps, .. } => {
                for (stump, pair) in stumps.iter_mut().zip(weights.chunks_exact(2)) {
                    stump.below = pair[0];
                    stump.at_or_above = pair[1];
                }
            }
        }
        Ok(())
    }

    pub fn predict(&self, inputs: &[f64]) -> f64 {
        match self {
            Self::Linear {
                intercept,
                coefficients,
            } => {
                let mut total = *intercept;
                for (coefficient, input) in coefficients.iter().zip(inputs) {
                    total += coefficient * input;
                }
                total
            }
            Self::BoostedStumps {
                base,
                learning_rate,
                stumps,
            } => base + learning_rate * stumps.iter().map(|s| s.evaluate(inputs)).sum::<f64>(),
        }
    }

    /// Worst-case evaluation steps, in the same unit
    /// [`qip_strategy::model::DistilledModel::cost`] uses.
    pub fn cost(&self) -> usize {
        match self {
            Self::Linear { coefficients, .. } => coefficients.len() + 1,
            Self::BoostedStumps { stumps, .. } => stumps.len() + 1,
        }
    }
}

/// A scale and offset applied to a model's output after the fact.
///
/// Calibration is not refitting. It moves the output onto the scale the
/// decision layer expects without touching what the model computes, which is
/// exactly why a fast cadence may change it and may not change anything else.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    pub scale: f64,
    pub offset: f64,
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            scale: 1.0,
            offset: 0.0,
        }
    }
}

impl Calibration {
    pub fn apply(&self, value: f64) -> f64 {
        self.scale * value + self.offset
    }

    pub fn is_identity(&self) -> bool {
        (self.scale - 1.0).abs() < f64::EPSILON && self.offset.abs() < f64::EPSILON
    }
}

/// How well a fit did, in sample and out of it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FitDiagnostics {
    pub observations: usize,
    pub holdout_observations: usize,
    pub in_sample_r2: f64,
    pub in_sample_rmse: f64,
    /// Coefficient of determination on the held-out tail. Negative when the
    /// model does worse than predicting the mean, which is the normal result
    /// on data with no signal in it.
    pub holdout_r2: f64,
    pub holdout_rmse: f64,
    /// Correlation between prediction and outcome on the holdout.
    pub holdout_correlation: f64,
}

/// What a fit has to show before anyone may say it learned something.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkillPolicy {
    pub minimum_holdout_r2: f64,
    pub minimum_holdout_observations: usize,
}

impl Default for SkillPolicy {
    fn default() -> Self {
        // Five per cent of out-of-sample variance is a low bar and a real
        // one: a model with no signal averages slightly *below* zero out of
        // sample, so anything comfortably above zero is a claim, and anything
        // at or below it is not.
        Self {
            minimum_holdout_r2: 0.05,
            minimum_holdout_observations: 30,
        }
    }
}

impl FitDiagnostics {
    /// Whether this fit may be said to have learned anything, with the reason
    /// when it may not.
    pub fn skill_verdict(&self, policy: &SkillPolicy) -> std::result::Result<(), String> {
        if self.holdout_observations < policy.minimum_holdout_observations {
            return Err(format!(
                "{} held-out observation(s) is below the {} needed to say anything",
                self.holdout_observations, policy.minimum_holdout_observations
            ));
        }
        if self.holdout_r2 < policy.minimum_holdout_r2 {
            return Err(format!(
                "holdout R² of {:.4} is below the {:.4} bar; the in-sample R² of {:.4} describes \
                 the fit's memory of its own training set and nothing else",
                self.holdout_r2, policy.minimum_holdout_r2, self.in_sample_r2
            ));
        }
        Ok(())
    }

    pub fn claims_skill(&self, policy: &SkillPolicy) -> bool {
        self.skill_verdict(policy).is_ok()
    }

    /// How much of the in-sample fit survived out of sample.
    ///
    /// Reported rather than acted on: it is the number that makes an
    /// overfitted ensemble obvious at a glance.
    pub fn retention(&self) -> f64 {
        if self.in_sample_r2 <= 1e-12 {
            return 0.0;
        }
        self.holdout_r2 / self.in_sample_r2
    }

    pub fn summarise(&self) -> String {
        format!(
            "R² {:.4} in sample over {} observation(s), {:.4} on {} held out; RMSE {:.5} and \
             {:.5}; holdout correlation {:.4}",
            self.in_sample_r2,
            self.observations,
            self.holdout_r2,
            self.holdout_observations,
            self.in_sample_rmse,
            self.holdout_rmse,
            self.holdout_correlation
        )
    }
}

/// A fitted model, with everything needed to say where it came from.
///
/// Fields are private. The form can only be replaced through
/// [`Self::apply`], which takes an [`AuthorisedUpdate`] — so a model's
/// structure cannot be changed by a cadence that is not allowed to change it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainedTeacher {
    name: String,
    version: String,
    dataset: String,
    feature_names: Vec<String>,
    form: TeacherForm,
    calibration: Calibration,
    online_statistics: BTreeMap<String, f64>,
    hyperparameters: BTreeMap<String, String>,
    /// `None` once the structure has been replaced and before it has been
    /// refitted: a diagnostic that described a different function is not a
    /// diagnostic of this one.
    fit: Option<FitDiagnostics>,
    fitted_at: Timestamp,
    updated_at: Timestamp,
}

impl TrainedTeacher {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn reference(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    pub fn dataset(&self) -> &str {
        &self.dataset
    }

    pub fn feature_names(&self) -> &[String] {
        &self.feature_names
    }

    pub fn form(&self) -> &TeacherForm {
        &self.form
    }

    pub fn calibration(&self) -> Calibration {
        self.calibration
    }

    pub fn online_statistics(&self) -> &BTreeMap<String, f64> {
        &self.online_statistics
    }

    pub fn hyperparameters(&self) -> &BTreeMap<String, String> {
        &self.hyperparameters
    }

    /// The fit's diagnostics, or `None` when the structure has changed since
    /// they were measured.
    pub fn fit(&self) -> Option<&FitDiagnostics> {
        self.fit.as_ref()
    }

    pub fn fitted_at(&self) -> Timestamp {
        self.fitted_at
    }

    pub fn updated_at(&self) -> Timestamp {
        self.updated_at
    }

    pub fn arity(&self) -> usize {
        self.feature_names.len()
    }

    /// A stable description of the model's shape.
    ///
    /// Exactly the complement of [`TeacherForm::weights`]: it covers every
    /// part of the form a weights update may *not* replace — the coefficient
    /// count and the intercept for a linear model; the stump count, the base,
    /// the learning rate and each stump's feature and threshold for an
    /// ensemble. So the digest is invariant under every update a fast cadence
    /// is allowed to make, and changes under any replacement of the form that
    /// changes anything else. A structural change that left this string alone
    /// would be a structural change nobody could see.
    pub fn structure_digest(&self) -> String {
        match &self.form {
            TeacherForm::Linear {
                intercept,
                coefficients,
            } => {
                format!("linear:{}:{}", coefficients.len(), intercept.to_bits())
            }
            TeacherForm::BoostedStumps {
                base,
                learning_rate,
                stumps,
            } => {
                let mut parts = vec![format!(
                    "stumps:{}:{}:{}",
                    stumps.len(),
                    base.to_bits(),
                    learning_rate.to_bits()
                )];
                for stump in stumps {
                    parts.push(format!("{}@{}", stump.feature, stump.threshold.to_bits()));
                }
                parts.join("|")
            }
        }
    }

    /// Predict, applying the calibration.
    pub fn predict(&self, inputs: &[f64]) -> Result<f64> {
        if inputs.len() != self.arity() {
            return Err(Error::invalid(format!(
                "{} takes {} input(s), given {}",
                self.reference(),
                self.arity(),
                inputs.len()
            )));
        }
        if inputs.iter().any(|value| !value.is_finite()) {
            return Err(Error::numeric(format!(
                "{} was given a non-finite input",
                self.reference()
            )));
        }
        Ok(self.calibration.apply(self.form.predict(inputs)))
    }

    /// Predict for every row of a dataset.
    pub fn predict_all(&self, data: &TrainingDataset) -> Result<Vec<f64>> {
        data.rows().iter().map(|row| self.predict(row)).collect()
    }

    /// Apply an update that has already cleared its cadence.
    ///
    /// The signature is the enforcement: there is no way to reach the private
    /// `form` field except through an [`AuthorisedUpdate`], and an
    /// `AuthorisedUpdate` cannot be constructed for a scope its cadence does
    /// not permit.
    pub fn apply(&mut self, update: &AuthorisedUpdate) -> Result<()> {
        use crate::cadence::UpdatePayload;
        match update.payload() {
            UpdatePayload::Weights(weights) => self.form.set_weights(weights)?,
            UpdatePayload::Calibration { scale, offset } => {
                if !scale.is_finite() || !offset.is_finite() {
                    return Err(Error::numeric("a calibration must be finite"));
                }
                self.calibration = Calibration {
                    scale: *scale,
                    offset: *offset,
                };
            }
            UpdatePayload::OnlineStatistics(statistics) => {
                for (key, value) in statistics {
                    if !value.is_finite() {
                        return Err(Error::numeric(format!("statistic {key} is not finite")));
                    }
                    self.online_statistics.insert(key.clone(), *value);
                }
            }
            UpdatePayload::Hyperparameters(parameters) => {
                for (key, value) in parameters {
                    self.hyperparameters.insert(key.clone(), value.clone());
                }
            }
            UpdatePayload::FeatureSet(names) => {
                if names.len() != self.arity() {
                    return Err(Error::invalid(format!(
                        "the model reads {} feature(s) and the update names {}; changing how many \
                         features a model reads is a change to its structure",
                        self.arity(),
                        names.len()
                    )));
                }
                self.feature_names = names.clone();
            }
            UpdatePayload::Structure(form) => {
                if form.arity() > self.feature_names.len() {
                    return Err(Error::invalid(format!(
                        "the replacement form reads {} feature(s) and only {} are named",
                        form.arity(),
                        self.feature_names.len()
                    )));
                }
                self.form = form.clone();
                // A new function has not been evaluated. Keeping the old
                // diagnostics would let a structural change inherit an
                // evaluation it never earned, which is the exact route by
                // which an unreviewed model reaches production.
                self.fit = None;
            }
        }
        self.updated_at = update.at();
        Ok(())
    }
}

/// Fits models in this process, from data already in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTrainer;

impl Default for LocalTrainer {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalTrainer {
    pub const fn new() -> Self {
        Self
    }

    /// Fit a teacher and measure it against a held-out tail.
    pub fn fit(
        &self,
        spec: &crate::job::TrainingSpec,
        data: &TrainingDataset,
        at: Timestamp,
    ) -> Result<TrainedTeacher> {
        spec.validate()?;
        if data.arity() == 0 {
            return Err(Error::invalid("a fit needs at least one feature"));
        }
        let (fit_set, holdout) = data.split_at_fraction(spec.holdout_fraction)?;

        let form = fit_form(&spec.family, &fit_set)?;
        let diagnostics = diagnose(&form, &fit_set, &holdout);

        let mut hyperparameters = BTreeMap::new();
        spec.family.record(&mut hyperparameters);
        hyperparameters.insert(
            "holdout_fraction".to_string(),
            format!("{}", spec.holdout_fraction),
        );
        hyperparameters.insert("horizon".to_string(), spec.horizon.as_str().to_string());
        hyperparameters.insert("cadence".to_string(), spec.cadence.as_str().to_string());
        hyperparameters.insert("seed".to_string(), spec.seed.to_string());

        Ok(TrainedTeacher {
            name: spec.name.clone(),
            version: spec.version.clone(),
            dataset: data.name().to_string(),
            feature_names: data.feature_names().to_vec(),
            form,
            calibration: Calibration::default(),
            online_statistics: BTreeMap::new(),
            hyperparameters,
            fit: Some(diagnostics),
            fitted_at: at,
            updated_at: at,
        })
    }
}

/// Fit the requested family.
fn fit_form(family: &ModelFamily, data: &TrainingDataset) -> Result<TeacherForm> {
    match family {
        ModelFamily::Linear { ridge } => fit_linear(data, *ridge),
        ModelFamily::BoostedStumps {
            rounds,
            learning_rate,
            min_samples_leaf,
            candidate_splits,
        } => fit_boosted(
            data,
            *rounds,
            *learning_rate,
            *min_samples_leaf,
            *candidate_splits,
        ),
    }
}

/// Ridge-regularised least squares by the normal equations.
///
/// `(XᵀX + λI)β = Xᵀy`, with the intercept left out of the penalty — shrinking
/// it towards zero would bias every prediction towards zero rather than
/// towards the mean, which is not what a penalty is for.
fn fit_linear(data: &TrainingDataset, ridge: f64) -> Result<TeacherForm> {
    if ridge < 0.0 {
        return Err(Error::invalid("a ridge penalty cannot be negative"));
    }
    let n = data.len();
    let k = data.arity();
    if n <= k + 1 {
        return Err(Error::invalid(format!(
            "{n} observation(s) cannot determine {k} coefficient(s) and an intercept"
        )));
    }

    let mut design = Matrix::zeros(n, k + 1);
    for (row_index, row) in data.rows().iter().enumerate() {
        design.set(row_index, 0, 1.0);
        for (column, value) in row.iter().enumerate() {
            design.set(row_index, column + 1, *value);
        }
    }
    let transpose = design.transpose();
    let mut normal = transpose.matmul(&design)?;
    for index in 1..=k {
        normal.add_to(index, index, ridge);
    }
    let rhs = transpose.mul_vec(data.targets())?;
    let beta = normal.solve(&rhs)?;
    if beta.iter().any(|value| !value.is_finite()) {
        return Err(Error::numeric(
            "the normal equations produced a non-finite coefficient; the design is degenerate",
        ));
    }
    Ok(TeacherForm::Linear {
        intercept: beta[0],
        coefficients: beta[1..].to_vec(),
    })
}

/// Gradient boosting over depth-one regression trees.
fn fit_boosted(
    data: &TrainingDataset,
    rounds: usize,
    learning_rate: f64,
    min_samples_leaf: usize,
    candidate_splits: usize,
) -> Result<TeacherForm> {
    if rounds == 0 {
        return Err(Error::invalid("a boosted model needs at least one round"));
    }
    if !(0.0..=1.0).contains(&learning_rate) || learning_rate <= 0.0 {
        return Err(Error::invalid(format!(
            "a learning rate of {learning_rate} is outside (0, 1]"
        )));
    }
    // Zero is not "no minimum" and refusing it is the point: a caller that
    // passed zero either meant one and should say so, or built the value from
    // something upstream that computed zero by mistake — clamping either case
    // to one would fit a model on a configuration nobody asked for.
    if min_samples_leaf == 0 {
        return Err(Error::invalid(
            "a minimum leaf size of zero admits a split that isolates a single row into its own \
             leaf; every split this crate fits requires at least one observation on each side",
        ));
    }
    if candidate_splits == 0 {
        return Err(Error::invalid(
            "a round with zero candidate splits considers no threshold at all, so it can never \
             split; that is a caller error, not a configuration to fall back from",
        ));
    }
    if data.len() < 2 * min_samples_leaf {
        return Err(Error::invalid(format!(
            "{} observation(s) cannot be split with {min_samples_leaf} either side",
            data.len()
        )));
    }

    let base = stats::mean(data.targets());
    let mut residuals: Vec<f64> = data.targets().iter().map(|y| y - base).collect();
    let mut stumps = Vec::with_capacity(rounds);

    for _ in 0..rounds {
        let Some(stump) = best_stump(data, &residuals, min_samples_leaf, candidate_splits) else {
            // No split removes any residual variance. Adding rounds past this
            // point adds cost and no fit.
            break;
        };
        for (index, row) in data.rows().iter().enumerate() {
            residuals[index] -= learning_rate * stump.evaluate(row);
        }
        stumps.push(stump);
    }

    if stumps.is_empty() {
        return Err(Error::numeric(
            "no split removed any residual variance, so there is nothing to boost; the features \
             carry no information about the target at this resolution",
        ));
    }
    Ok(TeacherForm::BoostedStumps {
        base,
        learning_rate,
        stumps,
    })
}

/// The split that removes the most residual sum of squares.
fn best_stump(
    data: &TrainingDataset,
    residuals: &[f64],
    min_samples_leaf: usize,
    candidate_splits: usize,
) -> Option<Stump> {
    let total: f64 = residuals.iter().sum();
    let count = residuals.len() as f64;
    let mut best: Option<(f64, Stump)> = None;

    for feature in 0..data.arity() {
        let mut values: Vec<f64> = data.rows().iter().map(|row| row[feature]).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        values.dedup();
        if values.len() < 2 {
            continue;
        }

        // Candidate thresholds at evenly spaced order statistics. Sampling the
        // quantiles rather than every value keeps a round linear in the number
        // of candidates instead of in the number of distinct values, and the
        // difference in fit is negligible.
        let steps = candidate_splits.min(values.len() - 1).max(1);
        for step in 1..=steps {
            let position = (values.len() * step) / (steps + 1);
            let threshold = values[position.clamp(1, values.len() - 1)];

            let mut below_sum = 0.0;
            let mut below_count = 0usize;
            for (index, row) in data.rows().iter().enumerate() {
                if row[feature] < threshold {
                    below_sum += residuals[index];
                    below_count += 1;
                }
            }
            let above_count = residuals.len() - below_count;
            if below_count < min_samples_leaf || above_count < min_samples_leaf {
                continue;
            }
            let above_sum = total - below_sum;
            // Reduction in residual sum of squares from this split.
            let gain = below_sum * below_sum / below_count as f64
                + above_sum * above_sum / above_count as f64
                - total * total / count;
            if gain > 1e-12 && best.as_ref().is_none_or(|(current, _)| gain > *current) {
                best = Some((
                    gain,
                    Stump {
                        feature,
                        threshold,
                        below: below_sum / below_count as f64,
                        at_or_above: above_sum / above_count as f64,
                    },
                ));
            }
        }
    }
    best.map(|(_, stump)| stump)
}

/// Score a fitted form in sample and out of it.
fn diagnose(
    form: &TeacherForm,
    fit_set: &TrainingDataset,
    holdout: &TrainingDataset,
) -> FitDiagnostics {
    let (in_r2, in_rmse, _) = score(form, fit_set);
    let (out_r2, out_rmse, out_correlation) = score(form, holdout);
    FitDiagnostics {
        observations: fit_set.len(),
        holdout_observations: holdout.len(),
        in_sample_r2: in_r2,
        in_sample_rmse: in_rmse,
        holdout_r2: out_r2,
        holdout_rmse: out_rmse,
        holdout_correlation: out_correlation,
    }
}

/// `R²`, root mean squared error and the prediction-outcome correlation.
///
/// `R²` is computed against the *dataset's own mean*, so a negative value means
/// exactly what it says: the model did worse than a constant.
fn score(form: &TeacherForm, data: &TrainingDataset) -> (f64, f64, f64) {
    let predictions: Vec<f64> = data.rows().iter().map(|row| form.predict(row)).collect();
    let targets = data.targets();
    let mean = stats::mean(targets);
    let mut residual_sum = 0.0;
    let mut total_sum = 0.0;
    for (prediction, target) in predictions.iter().zip(targets) {
        residual_sum += (target - prediction) * (target - prediction);
        total_sum += (target - mean) * (target - mean);
    }
    let r2 = if total_sum > 1e-18 {
        1.0 - residual_sum / total_sum
    } else {
        0.0
    };
    let rmse = (residual_sum / targets.len().max(1) as f64).sqrt();
    let correlation = stats::correlation(&predictions, targets);
    (r2, rmse, correlation)
}
