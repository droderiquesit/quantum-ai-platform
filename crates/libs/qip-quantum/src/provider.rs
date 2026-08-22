//! The quantum provider port.
//!
//! Everything downstream is written against [`QuantumProvider`]. Two
//! implementations exist: [`SimulatedProvider`], which runs the in-tree
//! statevector simulator, and [`HostedProvider`], which is the shape of an
//! adapter to a real service and reports itself unavailable in this build
//! because no credential, egress path or vendor SDK is present.
//!
//! The unavailable adapter is deliberate rather than a placeholder. It states
//! exactly what a deployment is missing, so a platform configured to use
//! hardware fails at start-up with a legible message instead of at the first
//! optimisation with a confusing one. [`HostedProvider::requirement`] is the
//! text an operator needs.

use crate::qaoa::{QaoaResult, QaoaSettings, solve};
use qip_core::error::{Error, Result};
use qip_core::rng::Xoshiro256;
use qip_core::time::Duration;
use qip_numerics::anneal::Qubo;
use serde::{Deserialize, Serialize};
use std::fmt;

/// What a provider can do and what it costs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Largest problem the provider will accept.
    pub max_qubits: usize,
    /// Whether the provider is a simulator. Reported so a result can never be
    /// presented as hardware evidence when it is not.
    pub simulated: bool,
    /// Whether results carry device noise.
    pub noisy: bool,
    /// Typical queue delay before a job starts.
    pub typical_queue: Duration,
    /// Cost per job in USD micro-units.
    pub cost_per_job_micros: u64,
}

impl ProviderCapabilities {
    pub fn simulator(max_qubits: usize) -> Self {
        Self {
            max_qubits,
            simulated: true,
            noisy: false,
            typical_queue: Duration::ZERO,
            cost_per_job_micros: 0,
        }
    }
}

/// A quantum backend.
pub trait QuantumProvider: Send + Sync + fmt::Debug {
    fn name(&self) -> &str;

    /// Whether this deployment can actually reach the provider.
    fn is_available(&self) -> bool;

    fn capabilities(&self) -> ProviderCapabilities;

    /// Solve a QUBO, or explain why not.
    fn solve_qubo(&self, qubo: &Qubo, settings: &QaoaSettings) -> Result<QaoaResult>;

    /// What an operator would have to provide to make this usable.
    ///
    /// Empty when the provider is already available.
    fn requirement(&self) -> String {
        String::new()
    }
}

/// The in-tree statevector simulator.
#[derive(Debug)]
pub struct SimulatedProvider {
    seed: u64,
    max_qubits: usize,
}

impl SimulatedProvider {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            max_qubits: crate::statevector::MAX_QUBITS,
        }
    }

    /// Restrict the simulator further, e.g. to keep a test fast.
    pub fn with_max_qubits(mut self, max_qubits: usize) -> Self {
        self.max_qubits = max_qubits.min(crate::statevector::MAX_QUBITS);
        self
    }
}

impl QuantumProvider for SimulatedProvider {
    fn name(&self) -> &str {
        "statevector-simulator"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::simulator(self.max_qubits)
    }

    fn solve_qubo(&self, qubo: &Qubo, settings: &QaoaSettings) -> Result<QaoaResult> {
        if qubo.n > self.max_qubits {
            return Err(Error::invalid(format!(
                "{} variables exceeds the simulator's {} qubit limit; a statevector needs 2^n amplitudes",
                qubo.n, self.max_qubits
            )));
        }
        // Seeded per problem size so a repeated run reproduces exactly.
        let mut rng = Xoshiro256::seeded(self.seed).fork(&format!("qaoa-{}", qubo.n));
        solve(qubo, settings, &mut rng)
    }
}

/// How a hosted provider is configured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostedConfig {
    /// Vendor name, e.g. `ibm-quantum`.
    pub vendor: String,
    /// Backend or device name.
    pub backend: String,
    /// Environment variable holding the API token. The token itself is never
    /// stored in configuration or in a repository.
    pub credential_env: String,
    /// API endpoint.
    pub endpoint: String,
    pub max_qubits: usize,
    pub cost_per_job_micros: u64,
}

/// An adapter to a hosted quantum service.
///
/// The interface is complete and everything downstream is written against it.
/// The transport is not: this build has no vendor SDK, no credential and no
/// egress path, so the adapter reports itself unavailable and says what is
/// missing rather than pretending to submit a job.
#[derive(Debug)]
pub struct HostedProvider {
    config: HostedConfig,
    /// Whether a credential was found. Injected rather than read from the
    /// environment here, so the same code path is exercised in a test.
    credential_present: bool,
    /// Whether a transport to the vendor exists in this build. Always false;
    /// the field exists so the availability logic is the real one rather than
    /// a hard-coded `false`.
    transport_present: bool,
}

impl HostedProvider {
    pub fn new(config: HostedConfig) -> Self {
        Self {
            config,
            credential_present: false,
            transport_present: false,
        }
    }

    /// Construct with a credential, for testing the availability logic.
    pub fn with_credential(config: HostedConfig, present: bool) -> Self {
        Self {
            config,
            credential_present: present,
            transport_present: false,
        }
    }

    pub fn config(&self) -> &HostedConfig {
        &self.config
    }
}

impl QuantumProvider for HostedProvider {
    fn name(&self) -> &str {
        &self.config.backend
    }

    fn is_available(&self) -> bool {
        self.credential_present && self.transport_present
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            max_qubits: self.config.max_qubits,
            simulated: false,
            noisy: true,
            typical_queue: Duration::from_mins(30),
            cost_per_job_micros: self.config.cost_per_job_micros,
        }
    }

    fn solve_qubo(&self, _qubo: &Qubo, _settings: &QaoaSettings) -> Result<QaoaResult> {
        Err(Error::unavailable(self.requirement()))
    }

    fn requirement(&self) -> String {
        let mut missing = Vec::new();
        if !self.credential_present {
            missing.push(format!(
                "an API token in the environment variable {}",
                self.config.credential_env
            ));
        }
        if !self.transport_present {
            missing.push(format!(
                "an HTTPS transport and the {} client, neither of which is present in this build",
                self.config.vendor
            ));
        }
        format!(
            "{} on {} is not usable: missing {}. The platform falls back to the classical solver, which is the configured default.",
            self.config.backend,
            self.config.endpoint,
            missing.join("; and ")
        )
    }
}
