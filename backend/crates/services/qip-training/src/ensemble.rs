//! A weighted blend of fitted teachers (R12-MODEL-4006).
//!
//! One fitted function has no measure of its own uncertainty that it did not
//! produce itself. Two functions of different classes, fitted on the same
//! rows, disagree where the data did not decide between them, and that
//! disagreement is a reading neither could give alone. This module holds the
//! blend and its two readings: the weighted prediction, and how far the
//! members stand from it.
//!
//! # What it does not decide
//!
//! Which teachers are members, and at what weight, is the caller's: the
//! weights are handed in. The deep brain's learning desk reads them from its
//! board of class outcomes under the regime the platform classified, so the
//! selection follows the regime and not anything computed here. A blend that
//! chose its own weights on the rows it is then scored on would flatter
//! itself, which is the failure the split exists to prevent.
//!
//! # Not a served form
//!
//! An [`Ensemble`] is not a [`crate::local::TeacherForm`] and has no served
//! artifact: the platform's provider serves the two teacher forms, and the
//! reflex lane runs a linear distillate. The blend is a research-lane
//! reading. Its interval is calibrated by
//! [`crate::ConformalInterval::of_predictions`] on rows no member was fitted
//! on.

use qip_core::error::{Error, Result};

use crate::dataset::TrainingDataset;
use crate::local::TrainedTeacher;

/// Fitted teachers and the share of the blend each carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Ensemble {
    /// Each member with its share; the shares sum to one.
    members: Vec<(TrainedTeacher, f64)>,
}

impl Ensemble {
    /// Blend `members` in proportion to their weights.
    ///
    /// Refuses no member; a weight that is not finite or not above zero; two
    /// members of one reference; and members fitted on different feature
    /// lists. A zero or negative weight is refused rather than dropped: a
    /// caller that meant to leave a member out leaves it out, and one that
    /// computed a non-positive weight has a defect a silent drop would hide.
    /// The weights are divided by their sum, which is what a share is and
    /// corrects nothing.
    pub fn weighted(members: Vec<(TrainedTeacher, f64)>) -> Result<Self> {
        let Some((first, _)) = members.first() else {
            return Err(Error::invalid(
                "an ensemble needs at least one member; a blend of nothing predicts nothing",
            ));
        };
        let features = first.feature_names().to_vec();
        let mut total = 0.0;
        for (index, (teacher, weight)) in members.iter().enumerate() {
            if !weight.is_finite() || *weight <= 0.0 {
                return Err(Error::invalid(format!(
                    "{} was given weight {weight}; an ensemble weight is finite and above zero — \
                     leave a member out rather than weight it at nothing",
                    teacher.reference()
                )));
            }
            if teacher.feature_names() != features.as_slice() {
                return Err(Error::invalid(format!(
                    "{} was fitted on different features from {}; their predictions for one row \
                     are not predictions of the same thing",
                    teacher.reference(),
                    first.reference()
                )));
            }
            if members[..index]
                .iter()
                .any(|(earlier, _)| earlier.reference() == teacher.reference())
            {
                return Err(Error::invalid(format!(
                    "{} is a member twice; a model blended with itself is one model at a larger \
                     weight and no diversity",
                    teacher.reference()
                )));
            }
            total += weight;
        }
        if !total.is_finite() {
            return Err(Error::numeric("the ensemble's weights do not sum finitely"));
        }
        Ok(Self {
            members: members
                .into_iter()
                .map(|(teacher, weight)| (teacher, weight / total))
                .collect(),
        })
    }

    /// Each member's reference and its share of the blend, in the order given.
    pub fn shares(&self) -> Vec<(String, f64)> {
        self.members
            .iter()
            .map(|(teacher, share)| (teacher.reference(), *share))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Never true: [`Self::weighted`] refuses an empty blend.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The blend's prediction for one row of inputs: each member's calibrated
    /// prediction times its share, summed.
    pub fn predict(&self, inputs: &[f64]) -> Result<f64> {
        let mut blended = 0.0;
        for (teacher, share) in &self.members {
            blended += share * teacher.predict(inputs)?;
        }
        Ok(blended)
    }

    /// The blend's prediction for every row: each member's calibrated
    /// prediction times its share, summed.
    pub fn predict_all(&self, data: &TrainingDataset) -> Result<Vec<f64>> {
        let mut blended = vec![0.0; data.len()];
        for (teacher, share) in &self.members {
            for (total, prediction) in blended.iter_mut().zip(teacher.predict_all(data)?) {
                *total += share * prediction;
            }
        }
        Ok(blended)
    }

    /// How far the members stand from the blend on `data`: the share-weighted
    /// mean absolute distance of a member's prediction from the blended one,
    /// averaged over the rows. Zero for one member, and for members that
    /// agree everywhere — which is the reading that says the blend adds
    /// nothing a single member did not already say.
    ///
    /// Refuses an empty dataset: there is no row to disagree on, and zero
    /// would read as agreement.
    pub fn disagreement(&self, data: &TrainingDataset) -> Result<f64> {
        if data.is_empty() {
            return Err(Error::invalid(
                "an ensemble's disagreement needs at least one row to be measured on",
            ));
        }
        let blended = self.predict_all(data)?;
        let mut total = 0.0;
        for (teacher, share) in &self.members {
            for (blend, prediction) in blended.iter().zip(teacher.predict_all(data)?) {
                total += share * (prediction - blend).abs();
            }
        }
        Ok(total / data.len() as f64)
    }
}
