//! The deep brain's platform assembly, callable by `main` and by a test
//! (R12-GCP-8106).
//!
//! The Risk brain runs as a warm Cloud Run service producing stress analysis
//! and central policy envelopes. That claim was only ever tested against
//! `Platform::new` in `qip-kernel`, because the real assembly lived inline in
//! `main`: a test under `qip-deepbrain/tests` could build its own platform and
//! so proved the kernel a second time. [`assemble`] is the platform half of
//! that composition — the model provider, the brain registry and the trust
//! root — moved here unchanged in behaviour, so `main` and the test run the
//! same function. Everything that reads the environment, binds a port or
//! opens storage stays in `main`; this takes resolved values.

use crate::brains;
use crate::brains::BrainRegistry;
use crate::trust::{self, KeyProvenance};
use qip_core::Context;
use qip_core::deployment::Deployment;
use qip_core::error::{Error, Result};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::sync::Arc;

/// Resolved inputs; nothing here is read from the environment.
pub struct Inputs {
    pub config: PlatformConfig,
    pub context: Context,
    pub telemetry: Telemetry,
    pub universe: Universe,
    pub limits: LimitSet,
    pub language_model: Arc<dyn qip_ai::language::LanguageModel>,
    /// The operator's envelope key, when the deployment provides one.
    pub envelope_key: Option<String>,
    pub deployment: Deployment,
}

/// What assembly produced.
pub struct Assembled {
    pub platform: Platform,
    pub brains: BrainRegistry,
    pub provenance: KeyProvenance,
}

/// Build the platform as the deployed node does, and harden its trust root.
pub fn assemble(inputs: Inputs) -> Result<Assembled> {
    let mut platform = Platform::with_model_provider(
        inputs.config,
        inputs.context,
        inputs.telemetry,
        inputs.universe,
        inputs.limits,
        inputs.language_model,
        Box::new(qip_training::serve::InTreeProvider),
    )?;
    let brains = brains::standard(platform.organisation())?;
    let provenance = trust::harden_central(
        &mut platform,
        inputs.envelope_key.as_deref(),
        inputs.deployment,
    )
    .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    Ok(Assembled {
        platform,
        brains,
        provenance,
    })
}

impl std::fmt::Debug for Inputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The envelope key is deliberately not printed.
        f.debug_struct("Inputs")
            .field("deployment", &self.deployment)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Assembled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assembled")
            .field("provenance", &self.provenance)
            .finish_non_exhaustive()
    }
}
