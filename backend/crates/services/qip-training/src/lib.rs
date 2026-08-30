//! `qip-training` — where a model is fitted, shrunk to something the hot path
//! may run, and governed by the rate at which it is allowed to change.
//!
//! The platform's learned functions come from here and nowhere else. Three
//! questions decide the shape of the crate, and each of them is answered with
//! a type rather than with a convention.
//!
//! ## A fit is not a discovery until it has been measured out of sample
//!
//! [`local::LocalTrainer`] fits two families for real — ridge-regularised
//! least squares and gradient-boosted stumps — and neither is allowed to
//! report its training error as skill. Every fit holds out the *last slice in
//! time* ([`dataset::TrainingDataset::split_at_fraction`], never a random
//! subset, because a randomly chosen holdout of a time series is one whose
//! neighbours the fit has already seen), scores itself on it, and carries the
//! result as [`local::FitDiagnostics`]. A boosted ensemble will drive training
//! error to nothing on pure noise; [`local::FitDiagnostics::skill_verdict`] is
//! what stops that reading as a discovery.
//!
//! The holdout also survives a structural change. Replacing a model's
//! functional form clears its diagnostics to `None`, so a new function cannot
//! inherit the evaluation of the one it replaced.
//!
//! ## A fast update may not change what a model *is*
//!
//! The platform learns at four rates, and they are not four settings of one
//! dial — they differ in how much review each has had. A millisecond-cadence
//! update runs thousands of times a day with nobody watching any individual
//! one, so it may replace weights inside an approved structure and nothing
//! else. If it could replace the structure, the model running against capital
//! tomorrow would be one no human ever reviewed.
//!
//! That is enforced by construction. [`cadence::AuthorisedUpdate`] has private
//! fields and one constructor, [`cadence::TrainingCadence::authorise`], and
//! [`local::TrainedTeacher::apply`] accepts nothing else — so the only route
//! to a model's private form runs through the check:
//!
//! ```
//! # use qip_core::Timestamp;
//! # use qip_training::cadence::{TrainingCadence, UpdatePayload};
//! # use qip_training::local::TeacherForm;
//! let structural = UpdatePayload::Structure(TeacherForm::Linear {
//!     intercept: 0.0,
//!     coefficients: vec![1.0, 2.0],
//! });
//! let refused = TrainingCadence::Millisecond.authorise(
//!     structural,
//!     Timestamp::from_secs(1_000),
//!     None,
//!     "a fast loop replacing the model",
//! );
//! assert!(refused.is_err());
//! ```
//!
//! There is no value of type `AuthorisedUpdate` describing a millisecond
//! update that replaces a form, so `apply` has no such case to guard against.
//! The slowest cadence, which is the reviewed one, may:
//!
//! ```
//! # use qip_core::Timestamp;
//! # use qip_training::cadence::{TrainingCadence, UpdatePayload};
//! # use qip_training::local::TeacherForm;
//! # let structural = UpdatePayload::Structure(TeacherForm::Linear {
//! #     intercept: 0.0,
//! #     coefficients: vec![1.0, 2.0],
//! # });
//! let allowed = TrainingCadence::DailyToWeekly.authorise(
//!     structural,
//!     Timestamp::from_secs(1_000),
//!     None,
//!     "the weekly retrain",
//! );
//! assert!(allowed.is_ok());
//! ```
//!
//! ## The model the hot path runs is not the model that was fitted
//!
//! An ensemble of sixty stumps is a perfectly good teacher and completely
//! inadmissible on the execution path: its cost grows with a hyperparameter
//! rather than with a bound. [`distill`] fits a
//! [`qip_strategy::model::DistilledModel`] to the teacher's *outputs* — soft
//! targets, not the observed labels — and measures the gap on four axes,
//! because a student can track the teacher's level and rank inputs
//! differently, or rank them identically and sit on the wrong side of the
//! decision threshold every time. [`distill::Distillation`] has private fields
//! and one constructor, [`distill::distil`], which measures before it returns,
//! so an unmeasured student is not a state anything downstream has to check
//! for.
//!
//! # What this crate deliberately does not do
//!
//! **It does not read a clock or draw from ambient randomness.** Every entry
//! point takes the [`qip_core::Timestamp`] it is reasoning about; every
//! stochastic part of the pipeline draws from a seed recorded on the job. Two
//! runs of the same fit on the same data produce the same model, which is the
//! only way a model card means anything.
//!
//! **It does not train on Vertex AI, and does not pretend to.** [`vertex`] is
//! a port: the request shape is complete and everything downstream is written
//! against [`job::TrainingProvider`], but this build has no HTTPS transport,
//! no Google client and no credential, so every method returns an error naming
//! what is missing. A fake connection that appeared to submit a job would be
//! worse than no connection at all, because it would produce a model card
//! recording a training run that never happened.
//!
//! **It does not promote anything.** Nothing here writes to a registry,
//! deploys a model, or decides that a model may influence capital.
//! [`distill::Distillation::approved_student`] will refuse to hand out a
//! student whose fidelity falls short, and that is the extent of it — the
//! gates a model passes on the way to production live in `qip-lifecycle`, and
//! a crate that both trains a model and clears it for use is a crate that
//! grades its own homework.
//!
//! **It does not engineer features.** A [`dataset::TrainingDataset`] arrives
//! with its columns already named, ordered in time and checked finite. Where
//! those columns came from is the feature layer's problem, and the invariants
//! are checked once here rather than assumed at every fit.
//!
//! **It does not hold money.** Nothing in this crate is a price, a size or a
//! P&L, so nothing here is a [`qip_core::Decimal`]. Every number is a
//! statistic — an R², a residual, a correlation — and is `f64` for that reason
//! and no other.

pub mod cadence;
pub mod dataset;
pub mod distill;
pub mod job;
pub mod local;
pub mod vertex;

pub use cadence::{AuthorisedUpdate, TrainingCadence, UpdatePayload, UpdateScope};
pub use dataset::TrainingDataset;
pub use distill::{Distillation, FidelityPolicy, FidelityReport, StudentForm, distil};
pub use job::{
    JobState, LocalTrainingProvider, TrainingArtifact, TrainingJob, TrainingProvider, TrainingSpec,
    run_to_completion,
};
pub use local::{
    Calibration, FitDiagnostics, LocalTrainer, ModelFamily, SkillPolicy, Stump, TeacherForm,
    TrainedTeacher,
};
pub use vertex::{VertexAiConfig, VertexAiProvider, VertexWorkload, WorkloadIdentityBinding};
