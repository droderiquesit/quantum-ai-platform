//! The training-provider port: submit, poll, fetch, cancel.
//!
//! Everything downstream is written against [`TrainingProvider`]. Two
//! implementations exist — [`LocalTrainingProvider`], which genuinely fits a
//! model in this process, and [`crate::vertex::VertexAiProvider`], which is
//! the shape of an adapter to Vertex AI and reports itself unavailable because
//! this build has no HTTPS transport, no Google client and no credential.
//!
//! The state machine is deliberately explicit even for the local provider,
//! which could have returned a finished model from `submit`. A caller written
//! against a provider that answers immediately will not work against one that
//! queues, and the whole point of the port is that the caller does not know
//! which it has.
//!
//! No clock is read anywhere. Every entry point takes the instant it is
//! reasoning about.

use crate::cadence::TrainingCadence;
use crate::dataset::TrainingDataset;
use crate::local::{LocalTrainer, ModelFamily, TrainedTeacher};
use qip_core::error::{Error, Result};
use qip_core::ids::IdGenerator;
use qip_core::{JobId, Timestamp};
use qip_quant::signal::Horizon;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// What to fit, on what, and how the result will be governed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingSpec {
    pub name: String,
    pub version: String,
    pub owner: String,
    pub purpose: String,
    /// The dataset's name, recorded on the model card.
    pub dataset: String,
    pub family: ModelFamily,
    /// The rate at which this model is expected to be updated, which decides
    /// what an update may change. See [`crate::cadence`].
    pub cadence: TrainingCadence,
    /// The horizon the target is measured over. Sets the label horizon that
    /// cross-validation purges, so it is not decoration: a label spanning five
    /// days leaks five days across every fold boundary unless the split knows.
    pub horizon: Horizon,
    /// Fraction of the tail held out of fitting, by time.
    pub holdout_fraction: f64,
    /// The stream any stochastic part of the fit draws from. Recorded so a fit
    /// can be reproduced.
    pub seed: u64,
}

impl TrainingSpec {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        owner: impl Into<String>,
        dataset: impl Into<String>,
        family: ModelFamily,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            owner: owner.into(),
            purpose: String::new(),
            dataset: dataset.into(),
            family,
            cadence: TrainingCadence::DailyToWeekly,
            horizon: Horizon::ShortTerm,
            holdout_fraction: 0.25,
            seed: 0x5EED_0000_0000_0001,
        }
    }

    pub fn with_purpose(mut self, purpose: impl Into<String>) -> Self {
        self.purpose = purpose.into();
        self
    }

    pub fn with_cadence(mut self, cadence: TrainingCadence) -> Self {
        self.cadence = cadence;
        self
    }

    pub fn with_horizon(mut self, horizon: Horizon) -> Self {
        self.horizon = horizon;
        self
    }

    pub fn with_holdout(mut self, fraction: f64) -> Self {
        self.holdout_fraction = fraction;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn reference(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    /// Observations a label spans, from the horizon.
    ///
    /// Rounded up, and never zero: a label that spans no time at all is a
    /// label computed from data the decision could not have had.
    pub fn label_horizon(&self) -> usize {
        (self.horizon.typical_holding_days().ceil() as usize).max(1)
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::invalid("a training job needs a model name"));
        }
        if self.version.trim().is_empty() {
            return Err(Error::invalid("a training job needs a model version"));
        }
        if self.owner.trim().is_empty() {
            return Err(Error::invalid(
                "a training job needs a named owner; a model nobody is accountable for cannot be \
                 retired by anyone either",
            ));
        }
        if !(0.0..1.0).contains(&self.holdout_fraction) {
            return Err(Error::invalid(format!(
                "a holdout fraction of {} is not between zero and one",
                self.holdout_fraction
            )));
        }
        Ok(())
    }
}

/// Where a job is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Accepted, not started.
    Queued,
    /// Running.
    Running,
    /// Finished; an artifact is available.
    Succeeded,
    /// Finished without an artifact, with the reason.
    Failed(String),
    /// Stopped by the caller, with the reason.
    Cancelled(String),
}

impl JobState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed(_) => "failed",
            Self::Cancelled(_) => "cancelled",
        }
    }

    /// Whether the job will not change again.
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed(_) | Self::Cancelled(_))
    }

    pub const fn produced_an_artifact(&self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

/// A submitted training job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingJob {
    pub id: JobId,
    /// Which provider is running it, so a job can be traced to a machine.
    pub provider: String,
    pub spec: TrainingSpec,
    pub state: JobState,
    pub submitted_at: Timestamp,
    pub updated_at: Timestamp,
    /// Observations the job was given.
    pub rows: usize,
}

/// A finished fit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrainingArtifact {
    pub job: JobId,
    pub teacher: TrainedTeacher,
    pub dataset: String,
    pub rows: usize,
    pub produced_at: Timestamp,
}

/// Somewhere a model can be trained.
pub trait TrainingProvider: fmt::Debug {
    fn name(&self) -> &str;

    /// Whether this deployment can actually reach it.
    fn is_available(&self) -> bool;

    /// What an operator would have to provide to make it usable. Empty when it
    /// already is.
    fn requirement(&self) -> String {
        String::new()
    }

    fn submit(
        &mut self,
        spec: TrainingSpec,
        data: &TrainingDataset,
        at: Timestamp,
    ) -> Result<TrainingJob>;

    /// Advance and report a job's state.
    fn poll(&mut self, job: &JobId, at: Timestamp) -> Result<TrainingJob>;

    /// The fitted model, once there is one.
    fn artifact(&self, job: &JobId) -> Result<TrainingArtifact>;

    fn cancel(&mut self, job: &JobId, at: Timestamp) -> Result<TrainingJob>;
}

/// One job as the local provider holds it.
#[derive(Clone, Debug)]
struct LocalJob {
    job: TrainingJob,
    data: TrainingDataset,
    artifact: Option<TrainingArtifact>,
}

/// Trains in this process, from data already in memory.
///
/// The state machine is real: `submit` queues, the first `poll` starts the
/// run, the second finishes it. A caller that never polls never gets an
/// artifact, which is the behaviour it would get from a managed service and
/// therefore the behaviour it has to be written for.
#[derive(Debug)]
pub struct LocalTrainingProvider {
    trainer: LocalTrainer,
    ids: IdGenerator,
    jobs: BTreeMap<String, LocalJob>,
}

impl LocalTrainingProvider {
    /// Seeded, so the job identifiers a run mints are reproducible.
    pub fn new(seed: u64) -> Self {
        Self {
            trainer: LocalTrainer::new(),
            ids: IdGenerator::new(seed),
            jobs: BTreeMap::new(),
        }
    }

    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    fn get(&self, job: &JobId) -> Result<&LocalJob> {
        self.jobs
            .get(job.as_str())
            .ok_or_else(|| Error::not_found(format!("no training job {job}")))
    }

    fn get_mut(&mut self, job: &JobId) -> Result<&mut LocalJob> {
        self.jobs
            .get_mut(job.as_str())
            .ok_or_else(|| Error::not_found(format!("no training job {job}")))
    }
}

impl TrainingProvider for LocalTrainingProvider {
    fn name(&self) -> &str {
        "local-trainer"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn submit(
        &mut self,
        spec: TrainingSpec,
        data: &TrainingDataset,
        at: Timestamp,
    ) -> Result<TrainingJob> {
        spec.validate()?;
        if data.arity() == 0 {
            return Err(Error::invalid("a training job needs at least one feature"));
        }
        let id: JobId = self.ids.generate(at);
        let job = TrainingJob {
            id: id.clone(),
            provider: self.name().to_string(),
            spec,
            state: JobState::Queued,
            submitted_at: at,
            updated_at: at,
            rows: data.len(),
        };
        self.jobs.insert(
            id.as_str().to_string(),
            LocalJob {
                job: job.clone(),
                data: data.clone(),
                artifact: None,
            },
        );
        Ok(job)
    }

    fn poll(&mut self, job: &JobId, at: Timestamp) -> Result<TrainingJob> {
        let trainer = self.trainer;
        let record = self.get_mut(job)?;
        match record.job.state {
            JobState::Queued => {
                record.job.state = JobState::Running;
                record.job.updated_at = at;
            }
            JobState::Running => {
                // The fit happens here, on the transition a caller can
                // observe, rather than inside `submit` where a caller written
                // for a queueing provider would never see it.
                match trainer.fit(&record.job.spec, &record.data, at) {
                    Ok(teacher) => {
                        record.artifact = Some(TrainingArtifact {
                            job: job.clone(),
                            teacher,
                            dataset: record.data.name().to_string(),
                            rows: record.data.len(),
                            produced_at: at,
                        });
                        record.job.state = JobState::Succeeded;
                    }
                    Err(error) => {
                        record.job.state = JobState::Failed(error.message().to_string());
                    }
                }
                record.job.updated_at = at;
            }
            JobState::Succeeded | JobState::Failed(_) | JobState::Cancelled(_) => {}
        }
        Ok(record.job.clone())
    }

    fn artifact(&self, job: &JobId) -> Result<TrainingArtifact> {
        let record = self.get(job)?;
        record.artifact.clone().ok_or_else(|| {
            Error::not_found(format!(
                "training job {job} is {} and has produced no artifact",
                record.job.state.as_str()
            ))
        })
    }

    fn cancel(&mut self, job: &JobId, at: Timestamp) -> Result<TrainingJob> {
        let record = self.get_mut(job)?;
        if record.job.state.is_terminal() {
            return Err(Error::denied(format!(
                "training job {job} is already {} and cannot be cancelled",
                record.job.state.as_str()
            )));
        }
        record.job.state = JobState::Cancelled("cancelled by the caller".to_string());
        record.job.updated_at = at;
        Ok(record.job.clone())
    }
}

/// Run a job to completion against any provider.
///
/// Polls until the job is terminal or the budget runs out. Written once here
/// rather than at every call site, because "poll until done" is where a caller
/// most easily writes a loop with no bound on it.
pub fn run_to_completion(
    provider: &mut dyn TrainingProvider,
    spec: TrainingSpec,
    data: &TrainingDataset,
    at: Timestamp,
    maximum_polls: usize,
) -> Result<TrainingArtifact> {
    let job = provider.submit(spec, data, at)?;
    for poll in 0..maximum_polls {
        let state = provider.poll(&job.id, at)?;
        if state.state.produced_an_artifact() {
            return provider.artifact(&job.id);
        }
        if state.state.is_terminal() {
            return Err(Error::io(format!(
                "training job {} finished as {} after {} poll(s)",
                job.id,
                state.state.as_str(),
                poll + 1
            )));
        }
    }
    Err(Error::timeout(format!(
        "training job {} did not finish within {maximum_polls} poll(s)",
        job.id
    )))
}
