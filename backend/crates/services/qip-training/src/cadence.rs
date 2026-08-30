//! How often a model may be updated, and what an update at that rate may
//! change.
//!
//! The platform learns at four rates, and they are not four settings of one
//! dial. They differ in what review each has had:
//!
//! | cadence | interval | what it updates |
//! |---|---|---|
//! | [`TrainingCadence::Millisecond`] | 1ms – 1s | weights and policy values inside an already-approved structure |
//! | [`TrainingCadence::SecondsToMinutes`] | 1s – 5min | calibration constants and online statistics |
//! | [`TrainingCadence::Hourly`] | 1h – 24h | hyperparameters, and the model evaluation that justifies them |
//! | [`TrainingCadence::DailyToWeekly`] | 1d – 7d | deep retraining: the feature set and the functional form |
//!
//! The rule that matters is the one about *structure*. A millisecond-cadence
//! update runs thousands of times a day with nobody watching any individual
//! one. If such an update could change a model's functional form — its arity,
//! its node count, which features it reads — then the model running against
//! capital tomorrow would be a model no human reviewed, arrived at by a path
//! nobody can reconstruct. That is how an unreviewed model reaches production,
//! and it is prevented here by construction rather than by policy.
//!
//! The enforcement is structural in two places at once.
//!
//! 1. [`AuthorisedUpdate`] has private fields and one constructor,
//!    [`TrainingCadence::authorise`], which refuses a payload whose scope the
//!    cadence does not permit. There is no value representing "a millisecond
//!    update that replaces the model's form".
//! 2. [`crate::local::TrainedTeacher::apply`] takes an `AuthorisedUpdate` and
//!    nothing else, and the teacher's form is a private field. So the only
//!    route to a model's structure runs through the check, and a call site
//!    cannot forget it because there is no other call to make.
//!
//! A second, independent guard sits underneath: even an authorised *weights*
//! update is refused if it carries a different number of weights, because
//! changing how many coefficients a model has is a change to its structure
//! wearing a weights update's clothes.
//!
//! Cadence also bounds *rate*. An update applied faster than its cadence's
//! floor is not an update at that cadence; a "deep retrain" run every second
//! is a fast loop that has borrowed the authority of a slow one.
//!
//! One last door is shut on purpose. [`AuthorisedUpdate`] implements
//! `Serialize`, so an update that was applied can be written into an audit
//! record, and deliberately not `Deserialize` — an authorisation is a fact
//! about a check that ran, not a shape a file may assert. There is no route
//! from bytes on disk to a value carrying the authority of a review:
//!
//! ```compile_fail
//! fn only_if_deserializable<T: serde::de::DeserializeOwned>() {}
//! // `AuthorisedUpdate` does not implement `Deserialize`, so this does not
//! // compile — a cadence check cannot be parsed into existence.
//! only_if_deserializable::<qip_training::cadence::AuthorisedUpdate>();
//! ```
//!
//! The payload it carries is ordinary data and does deserialize, which is what
//! makes the line above a statement about authorisation rather than about
//! serialisation in general:
//!
//! ```
//! fn only_if_deserializable<T: serde::de::DeserializeOwned>() {}
//! only_if_deserializable::<qip_training::cadence::UpdatePayload>();
//! ```

use crate::local::TeacherForm;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What an update touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateScope {
    /// Coefficient values inside an already-approved structure.
    Weights,
    /// A scale and offset applied to the model's output.
    CalibrationConstants,
    /// Rolling summary statistics the surrounding system keeps.
    OnlineStatistics,
    /// Depth, learning rate, regularisation — the numbers that decide what the
    /// next fit will be, rather than what this model computes now.
    Hyperparameters,
    /// Which features the model reads.
    FeatureSet,
    /// The functional form itself.
    Structure,
}

impl UpdateScope {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Weights => "weights",
            Self::CalibrationConstants => "calibration_constants",
            Self::OnlineStatistics => "online_statistics",
            Self::Hyperparameters => "hyperparameters",
            Self::FeatureSet => "feature_set",
            Self::Structure => "structure",
        }
    }

    /// Whether a change of this scope changes what the model *is* rather than
    /// what it currently says.
    pub const fn changes_structure(&self) -> bool {
        matches!(self, Self::FeatureSet | Self::Structure)
    }

    /// The fastest cadence permitted to make a change of this scope, which is
    /// the same thing as the least review the change has to have had.
    ///
    /// Every slower cadence permits it too, so this is a floor rather than a
    /// selection.
    pub const fn required_cadence(&self) -> TrainingCadence {
        match self {
            Self::Weights => TrainingCadence::Millisecond,
            Self::CalibrationConstants | Self::OnlineStatistics => {
                TrainingCadence::SecondsToMinutes
            }
            Self::Hyperparameters => TrainingCadence::Hourly,
            Self::FeatureSet | Self::Structure => TrainingCadence::DailyToWeekly,
        }
    }
}

/// The four rates the platform learns at.
///
/// Ordered from fastest to slowest, and the ordering is meaningful: a slower
/// cadence permits everything a faster one does, because it has had at least
/// as much review.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingCadence {
    /// Milliseconds: weights and policy values, inside a structure that was
    /// approved elsewhere.
    Millisecond,
    /// Seconds to minutes: calibration and online statistics.
    SecondsToMinutes,
    /// Hours: hyperparameters and the model evaluation behind them.
    Hourly,
    /// Daily to weekly: deep retraining, including a new structure.
    DailyToWeekly,
}

impl TrainingCadence {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Millisecond => "millisecond",
            Self::SecondsToMinutes => "seconds_to_minutes",
            Self::Hourly => "hourly",
            Self::DailyToWeekly => "daily_to_weekly",
        }
    }

    /// Shortest gap between two updates at this cadence.
    ///
    /// A shorter gap means the update is not happening at this cadence,
    /// whatever it is labelled.
    pub const fn minimum_interval(&self) -> Duration {
        match self {
            Self::Millisecond => Duration::from_millis(1),
            Self::SecondsToMinutes => Duration::from_secs(1),
            Self::Hourly => Duration::from_hours(1),
            Self::DailyToWeekly => Duration::from_days(1),
        }
    }

    /// Longest gap before the update is overdue.
    pub const fn maximum_interval(&self) -> Duration {
        match self {
            Self::Millisecond => Duration::from_secs(1),
            Self::SecondsToMinutes => Duration::from_mins(5),
            Self::Hourly => Duration::from_hours(24),
            Self::DailyToWeekly => Duration::from_days(7),
        }
    }

    /// Whether an update at this cadence is overdue.
    ///
    /// The ceiling is not enforced in [`Self::authorise`], and deliberately
    /// cannot be: an update that arrives late is still a valid update, and
    /// refusing it would leave the model staler still. So it is a question a
    /// caller asks about a deployment rather than a check on the update in
    /// hand — a weekly retrain that last ran nine days ago is a model running
    /// on parameters nobody has revisited, which is worth an alert and is not
    /// a reason to reject the retrain when it finally comes.
    ///
    /// An update dated before the one it follows is not overdue; it is
    /// backdated, and [`Self::authorise`] is what refuses that.
    pub fn is_overdue(&self, last_applied: Timestamp, now: Timestamp) -> bool {
        now.since(last_applied) > self.maximum_interval()
    }

    /// Everything an update at this cadence may change.
    pub const fn permitted_scopes(&self) -> &'static [UpdateScope] {
        match self {
            Self::Millisecond => &[UpdateScope::Weights],
            Self::SecondsToMinutes => &[
                UpdateScope::Weights,
                UpdateScope::CalibrationConstants,
                UpdateScope::OnlineStatistics,
            ],
            Self::Hourly => &[
                UpdateScope::Weights,
                UpdateScope::CalibrationConstants,
                UpdateScope::OnlineStatistics,
                UpdateScope::Hyperparameters,
            ],
            Self::DailyToWeekly => &[
                UpdateScope::Weights,
                UpdateScope::CalibrationConstants,
                UpdateScope::OnlineStatistics,
                UpdateScope::Hyperparameters,
                UpdateScope::FeatureSet,
                UpdateScope::Structure,
            ],
        }
    }

    pub fn permits(&self, scope: UpdateScope) -> bool {
        self.permitted_scopes().contains(&scope)
    }

    /// Everything an update at this cadence may *not* change.
    ///
    /// Stated as its own method because it is the half a reader needs and the
    /// half a permission list makes them work out.
    pub fn forbidden_scopes(&self) -> Vec<UpdateScope> {
        [
            UpdateScope::Weights,
            UpdateScope::CalibrationConstants,
            UpdateScope::OnlineStatistics,
            UpdateScope::Hyperparameters,
            UpdateScope::FeatureSet,
            UpdateScope::Structure,
        ]
        .into_iter()
        .filter(|scope| !self.permits(*scope))
        .collect()
    }

    pub fn describe(&self) -> String {
        format!(
            "{} cadence (every {:.3}s to {:.3}s): may update {}; may not update {}",
            self.as_str(),
            self.minimum_interval().as_secs_f64(),
            self.maximum_interval().as_secs_f64(),
            self.permitted_scopes()
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            if self.forbidden_scopes().is_empty() {
                "nothing".to_string()
            } else {
                self.forbidden_scopes()
                    .iter()
                    .map(|scope| scope.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )
    }

    /// Authorise an update, or refuse it with the reason.
    ///
    /// The only constructor of [`AuthorisedUpdate`]. Two checks: the cadence
    /// permits the payload's scope, and the update is not arriving faster than
    /// the cadence it claims.
    pub fn authorise(
        self,
        payload: UpdatePayload,
        at: Timestamp,
        last_applied: Option<Timestamp>,
        reason: impl Into<String>,
    ) -> Result<AuthorisedUpdate> {
        let scope = payload.scope();
        if !self.permits(scope) {
            return Err(Error::denied(format!(
                "a {} update may not change {}: that needs the {} cadence, which is the slowest \
                 one and the only one a person reviews. A fast update that could change a \
                 model's structure is how an unreviewed model reaches production",
                self.as_str(),
                scope.as_str(),
                scope.required_cadence().as_str()
            )));
        }
        if let Some(previous) = last_applied {
            if at < previous {
                return Err(Error::invalid(format!(
                    "the update is dated {} but the previous one is dated {}",
                    at.to_rfc3339(),
                    previous.to_rfc3339()
                )));
            }
            let elapsed = at.since(previous);
            if elapsed < self.minimum_interval() {
                return Err(Error::denied(format!(
                    "{}ns after the previous update is faster than the {} cadence's {}ns floor; \
                     an update arriving at a faster rate than its cadence has borrowed the \
                     authority of a slower one",
                    elapsed.as_nanos(),
                    self.as_str(),
                    self.minimum_interval().as_nanos()
                )));
            }
        }
        Ok(AuthorisedUpdate {
            cadence: self,
            payload,
            at,
            reason: reason.into(),
        })
    }
}

/// What an update carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePayload {
    /// Replacement values for the weights the model already has, in the order
    /// [`TeacherForm::weights`] reports them.
    Weights(Vec<f64>),
    /// A scale and offset on the output.
    Calibration { scale: f64, offset: f64 },
    /// Rolling statistics, by name.
    OnlineStatistics(BTreeMap<String, f64>),
    /// Hyperparameters for the next fit, by name.
    Hyperparameters(BTreeMap<String, String>),
    /// A different set of feature names.
    FeatureSet(Vec<String>),
    /// A different functional form.
    Structure(TeacherForm),
}

impl UpdatePayload {
    pub const fn scope(&self) -> UpdateScope {
        match self {
            Self::Weights(_) => UpdateScope::Weights,
            Self::Calibration { .. } => UpdateScope::CalibrationConstants,
            Self::OnlineStatistics(_) => UpdateScope::OnlineStatistics,
            Self::Hyperparameters(_) => UpdateScope::Hyperparameters,
            Self::FeatureSet(_) => UpdateScope::FeatureSet,
            Self::Structure(_) => UpdateScope::Structure,
        }
    }
}

/// An update that has already cleared its cadence.
///
/// Private fields and one constructor. See the module note: this type is the
/// reason a millisecond update cannot change a model's structure, and the
/// reason is that no such value can be built.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AuthorisedUpdate {
    cadence: TrainingCadence,
    payload: UpdatePayload,
    at: Timestamp,
    reason: String,
}

impl AuthorisedUpdate {
    pub fn cadence(&self) -> TrainingCadence {
        self.cadence
    }

    pub fn payload(&self) -> &UpdatePayload {
        &self.payload
    }

    pub fn scope(&self) -> UpdateScope {
        self.payload.scope()
    }

    pub fn at(&self) -> Timestamp {
        self.at
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}
