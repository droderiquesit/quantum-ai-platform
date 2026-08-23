//! The Vertex AI training port.
//!
//! The interface is complete and everything downstream is written against it.
//! The transport is not: this build has no Google client, no credential and no
//! egress path, so every method reports itself unavailable and says exactly
//! what a deployment is missing.
//!
//! This is deliberate rather than a placeholder. A real client needs HTTP and
//! TLS, which `docs/adr/0009-tiered-dependency-policy.md` permits at the I/O
//! edge — but permitting it is not the same as having built it, and a fake
//! connection that appears to submit a job is worse than no connection at all:
//! it produces a model card recording a training run that never happened.
//!
//! [`VertexAiProvider::requirement`] is the text an operator needs. It names
//! five things, because a deployment that has four of them is still not a
//! deployment that can train a model.

use crate::dataset::TrainingDataset;
use crate::job::{TrainingArtifact, TrainingJob, TrainingProvider, TrainingSpec};
use qip_core::error::{Error, Result};
use qip_core::{JobId, Timestamp};
use serde::{Deserialize, Serialize};

/// How a Vertex AI training job would be configured.
///
/// Every field is a thing a deployment must supply. They are named separately
/// so the error says which one is missing rather than "not configured".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VertexAiConfig {
    /// The GCP project the training job is billed to and runs in.
    pub project_id: String,
    /// The region. Not interchangeable: a job cannot read a bucket or a
    /// dataset in another region without an egress charge and a latency cost,
    /// and some accelerator types exist in some regions only.
    pub region: String,
    /// The `gs://` bucket Vertex stages code, inputs and model output in.
    pub staging_bucket: String,
    /// What actually runs the fit.
    pub workload: VertexWorkload,
    /// The Kubernetes service account bound to a Google service account, and
    /// the roles that binding carries. Vertex jobs authenticate as a service
    /// account; a deployment with a token in an environment variable has not
    /// configured this, it has worked around it.
    pub workload_identity: WorkloadIdentityBinding,
}

/// What Vertex is asked to run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VertexWorkload {
    /// A custom training container: an image URI and the machine to run it on.
    CustomContainer {
        /// e.g. `europe-west4-docker.pkg.dev/PROJECT/REPO/trainer:TAG`.
        image_uri: String,
        machine_type: String,
        /// Accelerator type and count, where the fit needs one.
        accelerator: Option<String>,
    },
    /// AutoML: no container, and a target column and budget instead.
    AutoMl {
        objective: String,
        target_column: String,
        /// Node-hours the training budget allows.
        budget_node_hours: u32,
    },
}

impl VertexWorkload {
    pub fn describe(&self) -> String {
        match self {
            Self::CustomContainer {
                image_uri,
                machine_type,
                accelerator,
            } => format!(
                "the training container {image_uri} on a {machine_type}{}",
                accelerator
                    .as_ref()
                    .map_or_else(String::new, |a| format!(" with {a}"))
            ),
            Self::AutoMl {
                objective,
                target_column,
                budget_node_hours,
            } => format!(
                "an AutoML {objective} job on the target column {target_column} with a \
                 {budget_node_hours} node-hour budget"
            ),
        }
    }

    fn is_configured(&self) -> bool {
        match self {
            Self::CustomContainer {
                image_uri,
                machine_type,
                ..
            } => !image_uri.trim().is_empty() && !machine_type.trim().is_empty(),
            Self::AutoMl {
                objective,
                target_column,
                budget_node_hours,
            } => {
                !objective.trim().is_empty()
                    && !target_column.trim().is_empty()
                    && *budget_node_hours > 0
            }
        }
    }
}

/// The identity a Vertex job runs as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadIdentityBinding {
    /// The Kubernetes service account the platform's pods run as.
    pub kubernetes_service_account: String,
    /// The Google service account it impersonates.
    pub google_service_account: String,
    /// Roles that account must hold. Named because a binding with the wrong
    /// roles fails at the first API call with a message about a permission
    /// rather than about a binding.
    pub roles: Vec<String>,
}

impl WorkloadIdentityBinding {
    pub fn describe(&self) -> String {
        format!(
            "the Kubernetes service account {} bound to the Google service account {} holding {}",
            self.kubernetes_service_account,
            self.google_service_account,
            if self.roles.is_empty() {
                "no roles".to_string()
            } else {
                self.roles.join(", ")
            }
        )
    }

    fn is_configured(&self) -> bool {
        !self.kubernetes_service_account.trim().is_empty()
            && !self.google_service_account.trim().is_empty()
            && !self.roles.is_empty()
    }
}

/// An adapter to Vertex AI custom or AutoML training.
///
/// Reports unavailable, always, and names what is missing.
#[derive(Debug)]
pub struct VertexAiProvider {
    config: VertexAiConfig,
    /// Whether Application Default Credentials resolve in this environment.
    /// Injected rather than read from the environment here, so the same code
    /// path is exercised in a test.
    credentials_present: bool,
    /// Whether an HTTPS transport and a Vertex AI client exist in this build.
    /// Always false. The field exists so the availability logic is the real
    /// one rather than a hard-coded answer.
    transport_present: bool,
}

impl VertexAiProvider {
    pub fn new(config: VertexAiConfig) -> Self {
        Self {
            config,
            credentials_present: false,
            transport_present: false,
        }
    }

    /// Construct with credentials asserted present, to exercise the
    /// availability logic. The transport still is not.
    pub fn with_credentials(config: VertexAiConfig, credentials_present: bool) -> Self {
        Self {
            config,
            credentials_present,
            transport_present: false,
        }
    }

    pub fn config(&self) -> &VertexAiConfig {
        &self.config
    }

    /// Everything missing, named one item at a time.
    pub fn missing(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.config.project_id.trim().is_empty() {
            missing.push("a GCP project id to run and bill the job in".to_string());
        }
        if self.config.region.trim().is_empty() {
            missing.push(
                "a region: Vertex is regional, and a job cannot read a dataset or a bucket in \
                 another one without an egress charge"
                    .to_string(),
            );
        }
        if !self.config.staging_bucket.starts_with("gs://") {
            missing.push(
                "a gs:// staging bucket for the job's inputs, code and model output".to_string(),
            );
        }
        if !self.config.workload.is_configured() {
            missing.push(format!(
                "a fully specified workload: {}",
                self.config.workload.describe()
            ));
        }
        if !self.config.workload_identity.is_configured() {
            missing.push(format!(
                "a workload-identity binding: {}",
                self.config.workload_identity.describe()
            ));
        }
        if !self.credentials_present {
            missing.push(
                "resolvable Application Default Credentials for the bound service account"
                    .to_string(),
            );
        }
        if !self.transport_present {
            missing.push(
                "an HTTPS transport and a Vertex AI client, neither of which is present in this \
                 build; ADR 0009 permits both at the I/O edge and neither has been built"
                    .to_string(),
            );
        }
        missing
    }

    /// The text an operator needs.
    pub fn requirement(&self) -> String {
        format!(
            "Vertex AI training in project {} ({}) is not usable: it needs {}. The platform \
             trains locally instead, which fits a smaller model on data that fits in memory and \
             is not a substitute for a managed run",
            if self.config.project_id.trim().is_empty() {
                "<unset>"
            } else {
                &self.config.project_id
            },
            if self.config.region.trim().is_empty() {
                "<unset>"
            } else {
                &self.config.region
            },
            self.missing().join("; and ")
        )
    }
}

impl TrainingProvider for VertexAiProvider {
    fn name(&self) -> &str {
        "vertex-ai"
    }

    fn is_available(&self) -> bool {
        self.credentials_present && self.transport_present && self.missing().is_empty()
    }

    fn requirement(&self) -> String {
        Self::requirement(self)
    }

    fn submit(
        &mut self,
        _spec: TrainingSpec,
        _data: &TrainingDataset,
        _at: Timestamp,
    ) -> Result<TrainingJob> {
        Err(Error::unavailable(Self::requirement(self)))
    }

    fn poll(&mut self, _job: &JobId, _at: Timestamp) -> Result<TrainingJob> {
        Err(Error::unavailable(Self::requirement(self)))
    }

    fn artifact(&self, _job: &JobId) -> Result<TrainingArtifact> {
        Err(Error::unavailable(Self::requirement(self)))
    }

    fn cancel(&mut self, _job: &JobId, _at: Timestamp) -> Result<TrainingJob> {
        Err(Error::unavailable(Self::requirement(self)))
    }
}
