//! The in-tree provider: the four forms this platform trains, served
//! in-process behind [`qip_ai::serving::ModelProvider`] (ADR 0083).
//!
//! No inference crate is taken. The forms a provider must serve are the two
//! teachers this crate fits and the two distillates it produces, all of them
//! kilobytes of `f64` with a `predict` or `evaluate` the platform already
//! reads. This module packs each into a [`ModelArtifact`] and unpacks it
//! again, and the score a served model returns is computed by the form's own
//! function — [`TeacherForm::predict_calibrated`], which
//! [`TrainedTeacher::predict`] also calls, and [`DistilledModel::evaluate`] —
//! so there is no second arithmetic that a test would have to keep equal to
//! the first.
//!
//! Three refusals happen on `serve`, before a model exists to score with: a
//! digest that is not the digest of the payload (the artifact is not the one
//! that was promoted), a payload the form's own constructor refuses (a tree
//! that descends backwards, an empty coefficient vector, a non-finite
//! weight), and a payload whose form is not the format the artifact declares.
//! The last matters because the format is what a caller checks against
//! `serves()` before asking; a `distilled_tree` artifact carrying a linear
//! payload would pass that check and serve something else.
//!
//! Off the hot path only. Nothing here is reachable from `qip-strategy`, which
//! keeps its own inline `DistilledModel` as the one learned function a cell
//! evaluates; a `Box<dyn ServedModel>` is a virtual call and an allocation,
//! and that is acceptable exactly where this crate sits.

use crate::local::{Calibration, TeacherForm, TrainedTeacher};
use qip_ai::serving::{ModelArtifact, ModelFormat, ModelProvider, ServedModel};
use qip_core::error::{Error, Result};
use qip_strategy::model::{DistilledModel, ModelForm};
use serde::{Deserialize, Serialize};

/// The payload of a teacher artifact.
///
/// The arity is carried rather than derived because a stump ensemble's
/// [`TeacherForm::arity`] is the highest feature any stump consults plus one,
/// which can be fewer than the features the teacher was fitted on; a served
/// ensemble must refuse the same input counts the teacher refuses, not
/// fewer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeacherPayload {
    pub arity: usize,
    pub form: TeacherForm,
    pub calibration: Calibration,
}

impl TeacherPayload {
    /// The format this payload's form is, so an artifact's declared format
    /// can be checked against what it actually carries.
    pub const fn format(&self) -> ModelFormat {
        match self.form {
            TeacherForm::Linear { .. } => ModelFormat::TeacherLinear,
            TeacherForm::BoostedStumps { .. } => ModelFormat::TeacherBoostedStumps,
        }
    }

    /// The same refusals [`crate::local::LocalTrainer::fit`] could never
    /// produce, applied to a payload that arrived from outside it: a
    /// deserialised form has been through no constructor, and a served
    /// model over a non-finite weight scores `NaN` against every threshold
    /// and reads as a quiet decision not to trade.
    pub fn validate(&self, reference: &str) -> Result<()> {
        if self.arity == 0 {
            return Err(Error::invalid(format!(
                "teacher artifact `{reference}` declares no inputs; a model over nothing \
                 cannot be served"
            )));
        }
        if !self.calibration.scale.is_finite() || !self.calibration.offset.is_finite() {
            return Err(Error::invalid(format!(
                "teacher artifact `{reference}` carries a non-finite calibration"
            )));
        }
        match &self.form {
            TeacherForm::Linear {
                intercept,
                coefficients,
            } => {
                if coefficients.len() != self.arity {
                    return Err(Error::invalid(format!(
                        "teacher artifact `{reference}` declares {} input(s) and carries {} \
                         coefficient(s); the payload does not describe one model",
                        self.arity,
                        coefficients.len()
                    )));
                }
                if !intercept.is_finite() || coefficients.iter().any(|c| !c.is_finite()) {
                    return Err(Error::invalid(format!(
                        "teacher artifact `{reference}` carries a non-finite coefficient"
                    )));
                }
            }
            TeacherForm::BoostedStumps {
                base,
                learning_rate,
                stumps,
            } => {
                if stumps.is_empty() {
                    return Err(Error::invalid(format!(
                        "teacher artifact `{reference}` is an ensemble of no stumps"
                    )));
                }
                if !base.is_finite() || !learning_rate.is_finite() {
                    return Err(Error::invalid(format!(
                        "teacher artifact `{reference}` carries a non-finite base or learning rate"
                    )));
                }
                for (index, stump) in stumps.iter().enumerate() {
                    if stump.feature >= self.arity {
                        return Err(Error::invalid(format!(
                            "teacher artifact `{reference}` stump {index} reads input {} of {}",
                            stump.feature, self.arity
                        )));
                    }
                    if !stump.threshold.is_finite()
                        || !stump.below.is_finite()
                        || !stump.at_or_above.is_finite()
                    {
                        return Err(Error::invalid(format!(
                            "teacher artifact `{reference}` stump {index} carries a non-finite value"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// A teacher a provider has loaded. Scores through
/// [`TeacherForm::predict_calibrated`] and nothing else.
#[derive(Debug)]
struct ServedTeacher {
    reference: String,
    payload: TeacherPayload,
}

impl ServedModel for ServedTeacher {
    fn reference(&self) -> &str {
        &self.reference
    }

    fn format(&self) -> ModelFormat {
        self.payload.format()
    }

    fn arity(&self) -> usize {
        self.payload.arity
    }

    fn cost(&self) -> usize {
        self.payload.form.cost()
    }

    fn score(&self, inputs: &[f64]) -> Result<f64> {
        self.payload.form.predict_calibrated(
            self.payload.arity,
            self.payload.calibration,
            &self.reference,
            inputs,
        )
    }
}

/// A distillate a provider has loaded. Scores through
/// [`DistilledModel::evaluate`] and nothing else.
#[derive(Debug)]
struct ServedDistilled {
    reference: String,
    model: DistilledModel,
}

impl ServedModel for ServedDistilled {
    fn reference(&self) -> &str {
        &self.reference
    }

    fn format(&self) -> ModelFormat {
        distilled_format(&self.model)
    }

    fn arity(&self) -> usize {
        self.model.arity()
    }

    fn cost(&self) -> usize {
        self.model.cost()
    }

    fn score(&self, inputs: &[f64]) -> Result<f64> {
        self.model.evaluate(inputs)
    }
}

const fn distilled_format(model: &DistilledModel) -> ModelFormat {
    match model.form() {
        ModelForm::Linear { .. } => ModelFormat::DistilledLinear,
        ModelForm::Tree { .. } => ModelFormat::DistilledTree,
    }
}

/// Rebuild a deserialised distillate through its own constructors, so a
/// payload nobody constructed — a tree pointing backwards, an empty
/// coefficient vector — is refused with the constructor's own sentence.
fn rebuilt(model: DistilledModel) -> Result<DistilledModel> {
    match model.form() {
        ModelForm::Linear {
            intercept,
            coefficients,
        } => DistilledModel::linear(model.name(), *intercept, coefficients.clone()),
        ModelForm::Tree { arity, nodes } => {
            DistilledModel::tree(model.name(), *arity, nodes.clone())
        }
    }
}

/// The provider for everything this crate trains.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InTreeProvider;

impl InTreeProvider {
    pub const NAME: &'static str = "the in-tree provider";

    /// Pack a fitted teacher, computing the digest once from the payload
    /// this function serialised. `serve` recomputes it over the same
    /// canonical form, so the two can only agree or refuse.
    pub fn pack(teacher: &TrainedTeacher) -> Result<ModelArtifact> {
        let payload = TeacherPayload {
            arity: teacher.arity(),
            form: teacher.form().clone(),
            calibration: teacher.calibration(),
        };
        let format = payload.format();
        let value = serde_json::to_value(&payload).map_err(|error| {
            Error::invalid(format!(
                "teacher {} could not be serialised: {error}",
                teacher.reference()
            ))
        })?;
        Ok(ModelArtifact::new(teacher.reference(), format, value))
    }

    /// Pack a distillate under the card reference it is registered as. A
    /// `DistilledModel` carries a name and no version, so the reference is
    /// the caller's: it is whatever `ModelCard::reference()` says for the
    /// card that owns this model.
    pub fn pack_distilled(
        reference: impl Into<String>,
        model: &DistilledModel,
    ) -> Result<ModelArtifact> {
        let reference = reference.into();
        let value = serde_json::to_value(model).map_err(|error| {
            Error::invalid(format!(
                "distilled model {} could not be serialised: {error}",
                model.name()
            ))
        })?;
        Ok(ModelArtifact::new(
            reference,
            distilled_format(model),
            value,
        ))
    }

    fn form_mismatch(artifact: &ModelArtifact, carried: ModelFormat) -> Error {
        Error::invalid(format!(
            "model artifact `{}` declares `{}` and its payload is `{}`; the artifact does \
             not describe one model — re-pack it from the form it actually carries",
            artifact.reference,
            artifact.format.as_str(),
            carried.as_str()
        ))
    }
}

impl ModelProvider for InTreeProvider {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn serves(&self) -> &[ModelFormat] {
        &ModelFormat::ALL
    }

    fn serve(&self, artifact: &ModelArtifact) -> Result<Box<dyn ServedModel>> {
        self.admit(artifact)?;
        match artifact.format {
            ModelFormat::TeacherLinear | ModelFormat::TeacherBoostedStumps => {
                let payload: TeacherPayload = serde_json::from_value(artifact.payload.clone())
                    .map_err(|error| {
                        Error::invalid(format!(
                            "model artifact `{}` does not carry a teacher payload: {error}",
                            artifact.reference
                        ))
                    })?;
                if payload.format() != artifact.format {
                    return Err(Self::form_mismatch(artifact, payload.format()));
                }
                payload.validate(&artifact.reference)?;
                Ok(Box::new(ServedTeacher {
                    reference: artifact.reference.clone(),
                    payload,
                }))
            }
            ModelFormat::DistilledLinear | ModelFormat::DistilledTree => {
                let model: DistilledModel = serde_json::from_value(artifact.payload.clone())
                    .map_err(|error| {
                        Error::invalid(format!(
                            "model artifact `{}` does not carry a distilled model: {error}",
                            artifact.reference
                        ))
                    })?;
                let model = rebuilt(model)?;
                if distilled_format(&model) != artifact.format {
                    return Err(Self::form_mismatch(artifact, distilled_format(&model)));
                }
                Ok(Box::new(ServedDistilled {
                    reference: artifact.reference.clone(),
                    model,
                }))
            }
        }
    }
}
