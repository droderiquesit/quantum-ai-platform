//! The data a fit is made from, with the properties a fit depends on checked
//! once at the boundary.
//!
//! Four invariants hold for every [`TrainingDataset`], and each of them is a
//! bug that would otherwise surface as a plausible-looking model:
//!
//! * Every row has the same width as the feature list, so a coefficient always
//!   belongs to a named feature.
//! * Every value is finite. A NaN feature propagates silently through least
//!   squares and comes out as a NaN coefficient, which then scores every
//!   candidate as "do not trade".
//! * Observations are in time order and carry their timestamps. Purging and
//!   embargoing a fold is meaningless on rows whose order is arbitrary, and
//!   the validation this crate defers to assumes the order is real.
//! * The dataset knows its own name, because a model card that cannot say what
//!   it was fitted on is not a record of anything.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_numerics::Matrix;
use serde::{Deserialize, Serialize};

/// Features, targets and the instants they belong to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingDataset {
    name: String,
    feature_names: Vec<String>,
    rows: Vec<Vec<f64>>,
    targets: Vec<f64>,
    times: Vec<Timestamp>,
}

impl TrainingDataset {
    /// Build a dataset, checking every invariant the module note lists.
    pub fn new(
        name: impl Into<String>,
        feature_names: Vec<String>,
        rows: Vec<Vec<f64>>,
        targets: Vec<f64>,
        times: Vec<Timestamp>,
    ) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "a training dataset needs a name; a model card that cannot say what it was \
                 fitted on records nothing",
            ));
        }
        if feature_names.is_empty() {
            return Err(Error::invalid(
                "a training dataset needs at least one feature",
            ));
        }
        if rows.is_empty() {
            return Err(Error::invalid(
                "a training dataset needs at least one observation",
            ));
        }
        if rows.len() != targets.len() || rows.len() != times.len() {
            return Err(Error::invalid(format!(
                "{} row(s), {} target(s) and {} timestamp(s) do not line up",
                rows.len(),
                targets.len(),
                times.len()
            )));
        }
        for (index, row) in rows.iter().enumerate() {
            if row.len() != feature_names.len() {
                return Err(Error::invalid(format!(
                    "row {index} has {} value(s) for {} named feature(s)",
                    row.len(),
                    feature_names.len()
                )));
            }
            if let Some(position) = row.iter().position(|value| !value.is_finite()) {
                return Err(Error::numeric(format!(
                    "row {index} holds a non-finite value for {}",
                    feature_names[position]
                )));
            }
        }
        if let Some(index) = targets.iter().position(|value| !value.is_finite()) {
            return Err(Error::numeric(format!("target {index} is not finite")));
        }
        if times.windows(2).any(|pair| pair[1] < pair[0]) {
            return Err(Error::invalid(
                "observations are not in time order; purging and embargoing folds of an \
                 arbitrarily ordered series removes nothing",
            ));
        }
        Ok(Self {
            name,
            feature_names,
            rows,
            targets,
            times,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn feature_names(&self) -> &[String] {
        &self.feature_names
    }

    /// How many features each observation carries.
    pub fn arity(&self) -> usize {
        self.feature_names.len()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn rows(&self) -> &[Vec<f64>] {
        &self.rows
    }

    pub fn targets(&self) -> &[f64] {
        &self.targets
    }

    pub fn times(&self) -> &[Timestamp] {
        &self.times
    }

    pub fn row(&self, index: usize) -> Result<&[f64]> {
        self.rows
            .get(index)
            .map(Vec::as_slice)
            .ok_or_else(|| Error::not_found(format!("row {index} is outside the dataset")))
    }

    pub fn target(&self, index: usize) -> Result<f64> {
        self.targets
            .get(index)
            .copied()
            .ok_or_else(|| Error::not_found(format!("target {index} is outside the dataset")))
    }

    /// The rows at the given indices, in the order given.
    ///
    /// The operation cross-validation is built from: a fold is a set of
    /// indices, and this is what turns one into a dataset that can be fitted.
    pub fn select(&self, indices: &[usize]) -> Result<Self> {
        if indices.is_empty() {
            return Err(Error::invalid("an empty selection is not a dataset"));
        }
        let mut rows = Vec::with_capacity(indices.len());
        let mut targets = Vec::with_capacity(indices.len());
        let mut times = Vec::with_capacity(indices.len());
        for index in indices {
            let row = self
                .rows
                .get(*index)
                .ok_or_else(|| Error::not_found(format!("row {index} is outside the dataset")))?;
            rows.push(row.clone());
            targets.push(self.targets[*index]);
            times.push(self.times[*index]);
        }
        Self::new(
            self.name.clone(),
            self.feature_names.clone(),
            rows,
            targets,
            times,
        )
    }

    /// Split into a leading fit set and a trailing holdout.
    ///
    /// By time, always. A randomly chosen holdout of a time series is a
    /// holdout that the fit has already seen the neighbours of, and it will
    /// report a skill the model does not have.
    pub fn split_at_fraction(&self, holdout_fraction: f64) -> Result<(Self, Self)> {
        if !(0.0..1.0).contains(&holdout_fraction) {
            return Err(Error::invalid(format!(
                "a holdout fraction of {holdout_fraction} is not between zero and one"
            )));
        }
        // Both sides must be non-empty, so a split needs at least two rows.
        // Checked before the arithmetic rather than after it: the clamp below
        // has no valid range on a one-row dataset.
        if self.len() < 2 {
            return Err(Error::invalid(format!(
                "a dataset of {} observation(s) cannot be split into a fit set and a holdout; \
                 one of the two would be empty",
                self.len()
            )));
        }
        let holdout = ((self.len() as f64) * holdout_fraction).round() as usize;
        // At least one row held out, and at least one left to fit on. A
        // holdout fraction rounding to zero or to the whole dataset is a
        // fraction that asks for a split there is no data for, not a licence
        // to score a model on its own training set.
        let holdout = holdout.clamp(1, self.len() - 1);
        let cut = self.len() - holdout;
        let fit: Vec<usize> = (0..cut).collect();
        let test: Vec<usize> = (cut..self.len()).collect();
        Ok((self.select(&fit)?, self.select(&test)?))
    }

    /// The design matrix, without an intercept column.
    ///
    /// The intercept is added by whichever fit needs one, so a caller cannot
    /// end up with two of them.
    pub fn design_matrix(&self) -> Result<Matrix> {
        let mut flat = Vec::with_capacity(self.len() * self.arity());
        for row in &self.rows {
            flat.extend_from_slice(row);
        }
        Matrix::from_vec(self.len(), self.arity(), flat)
    }

    /// Replace the targets, keeping features, names and times.
    ///
    /// This is what distillation needs: the same inputs, relabelled with the
    /// teacher's output instead of the observed one.
    pub fn with_targets(&self, targets: Vec<f64>) -> Result<Self> {
        Self::new(
            format!("{}#relabelled", self.name),
            self.feature_names.clone(),
            self.rows.clone(),
            targets,
            self.times.clone(),
        )
    }
}
