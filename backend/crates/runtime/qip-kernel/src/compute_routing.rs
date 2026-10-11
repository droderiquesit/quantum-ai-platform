//! Compute routing state: which arms a deployment attaches to the compute
//! router, and what the router resumes from the event log at start-up
//! (§11.1–11.2; R12-QUANT-4047, 4052, 7016, 7206, 1076).
//!
//! Two failures this module exists to close.
//!
//! **The quantum arm was unreachable.** `PlatformConfig::quantum_enabled`
//! defaulted to false and no composition root ever set it, so the
//! asynchronous gateway, the classical verification of a returned candidate
//! and the dispatch gate in front of both ran in tests and in no shipped
//! binary. [`ComputeSettings`] is what a composition root reads — from the
//! lookup it is handed, never from the process itself, because a library
//! that reads its own environment cannot be deployed twice with different
//! settings — and [`ComputeSettings::apply`] is the one place those settings
//! reach a [`PlatformConfig`]. Absent, everything stays off: the default is
//! the classical router every deployment already runs.
//!
//! **The router forgot everything at every restart.** Its learning — which
//! substrate wins at which size, how often its answers verify, how long they
//! take, what they cost, how well it predicted that — lived in a mutex, so a
//! deep brain that had watched an arm lose nineteen times running read it as
//! a coin flip again after a rollout and paid for it again. Every DECIDE
//! solve already journals its comparison as a [`SolverBenchmarked`] record
//! carrying the compute decision and, since this change, one measurement per
//! run. [`resume`] folds those records back into the router through
//! [`ComputeRouter::replay`] — the same fold the live solve applies — before
//! the router sizes anything, so the learning a restarted process holds is
//! the learning the log says it earned.

use crate::config::PlatformConfig;
use crate::platform::SolverBenchmarked;
use qip_core::error::{Error, Result};
use qip_events::EventBody;
use qip_events::log::EventLog;
use qip_optimization_engine::router::ComputeRouter;
use qip_quantum::qaoa::QaoaSettings;
use qip_streaming::envelope::StreamEnvelope;

/// The producer on every solver benchmark record, so a replay picks out the
/// kernel's own comparisons by origin as every other resume does.
pub const SOLVER_BENCHMARK_ORIGIN: &str = "kernel/solver-benchmark";

/// The variable a composition root reads for the quantum provider.
pub const QUANTUM_PROVIDER_VARIABLE: &str = "QIP_QUANTUM_PROVIDER";

/// The variable a composition root reads for the quantum-inspired rung.
pub const QUANTUM_INSPIRED_VARIABLE: &str = "QIP_QUANTUM_INSPIRED";

/// The QAOA budget the in-tree simulator is given when a deployment attaches
/// it. One layer and a twenty-step angle search: the default policy's three
/// layers and three hundred steps at the 23 qubits the default size threshold
/// admits is hours of statevector arithmetic on a worker thread, which would
/// be abandoned on patience every time and teach the router only that the
/// simulator is slow. A simulator's answer validates a formulation and is
/// never evidence of advantage, so a shallow circuit loses nothing that
/// could have counted.
pub const SIMULATOR_QAOA: QaoaSettings = QaoaSettings {
    layers: 1,
    optimiser_iterations: 20,
    shots: 0,
};

/// Which quantum provider the deployment attaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuantumProviderChoice {
    /// No provider. The default, and every production environment.
    Off,
    /// The in-tree statevector simulator, behind the asynchronous gateway.
    Simulated,
}

impl QuantumProviderChoice {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Simulated => "simulated",
        }
    }
}

/// The compute-routing settings a composition root reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComputeSettings {
    pub provider: QuantumProviderChoice,
    pub quantum_inspired: bool,
}

impl Default for ComputeSettings {
    /// Both off: the classical router every deployment has always run.
    fn default() -> Self {
        Self {
            provider: QuantumProviderChoice::Off,
            quantum_inspired: false,
        }
    }
}

/// The value, or `None` where it is unset or blank — a manifest that renders
/// an empty value has not chosen anything.
fn read(lookup: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    lookup(name).filter(|value| !value.trim().is_empty())
}

impl ComputeSettings {
    /// Parse from the lookup a composition root hands in.
    ///
    /// Refuses anything but the exact words, naming the variable and the
    /// words it accepts, with the `configuration:` prefix the roots exit 78
    /// on. `hosted` is refused by name: the IBM Quantum adapter needs a
    /// credential, an egress route and a cost budget no root yet supplies,
    /// and a value that read as hardware while the router ran a simulator
    /// would be the worst outcome available here.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let provider = match read(lookup, QUANTUM_PROVIDER_VARIABLE).as_deref() {
            None | Some("off") => QuantumProviderChoice::Off,
            Some("simulated") => QuantumProviderChoice::Simulated,
            Some("hosted") => {
                return Err(Error::invalid(format!(
                    "configuration: {QUANTUM_PROVIDER_VARIABLE}=hosted names the IBM Quantum \
                     adapter, which no composition root wires yet: it needs a credential file, \
                     an egress route and a per-job cost budget. Use off or simulated"
                )));
            }
            Some(other) => {
                return Err(Error::invalid(format!(
                    "configuration: {QUANTUM_PROVIDER_VARIABLE}={other:?} is not one of off, \
                     simulated; set one of those or leave it unset for off"
                )));
            }
        };
        let quantum_inspired = match read(lookup, QUANTUM_INSPIRED_VARIABLE).as_deref() {
            None | Some("off") => false,
            Some("on") => true,
            Some(other) => {
                return Err(Error::invalid(format!(
                    "configuration: {QUANTUM_INSPIRED_VARIABLE}={other:?} is not one of on, off; \
                     set one of those or leave it unset for off"
                )));
            }
        };
        Ok(Self {
            provider,
            quantum_inspired,
        })
    }

    /// The one place these settings reach the platform's configuration.
    pub fn apply(&self, mut config: PlatformConfig) -> PlatformConfig {
        if self.provider == QuantumProviderChoice::Simulated {
            config = config.with_quantum();
            config.routing.qaoa = SIMULATOR_QAOA;
        }
        if self.quantum_inspired {
            config.routing.quantum_inspired = true;
        }
        config
    }

    /// One line for a start-up banner.
    pub fn describe(&self) -> String {
        let provider = match self.provider {
            QuantumProviderChoice::Off => format!(
                "quantum provider off ({QUANTUM_PROVIDER_VARIABLE} unset or off); the classical \
                 baseline answers every solve"
            ),
            QuantumProviderChoice::Simulated => format!(
                "quantum provider: the in-tree simulator behind the asynchronous gateway \
                 ({QUANTUM_PROVIDER_VARIABLE}=simulated); a classical baseline is solved first \
                 every time, and a simulated answer is never evidence of advantage"
            ),
        };
        let inspired = if self.quantum_inspired {
            "quantum-inspired rung on"
        } else {
            "quantum-inspired rung off"
        };
        format!("{provider}; {inspired}")
    }
}

/// Fold the solver benchmark records the log holds back into the router, in
/// log order, and hand it back. Refuses a record that does not decode: a
/// learning rebuilt over a record it could not read is a history with a hole
/// in it, presented as whole.
pub fn resume(router: ComputeRouter, log: &EventLog) -> Result<ComputeRouter> {
    let mut decisions = Vec::new();
    for event in log.events() {
        if event.topic != SolverBenchmarked::TOPIC
            || event.lineage.producer != SOLVER_BENCHMARK_ORIGIN
        {
            continue;
        }
        let record = StreamEnvelope::from_frame(event)
            .and_then(|envelope| envelope.decode::<SolverBenchmarked>())?
            .body;
        if let Some(compute) = record.comparison.compute {
            decisions.push(compute);
        }
    }
    router.replay(decisions.iter())?;
    Ok(router)
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::platform::{PORTFOLIO_ALLOCATION_FAMILY, Platform, SolverRoutingJournal};
    use qip_core::Context;
    use qip_core::Timestamp;
    use qip_financial::universe::Universe;
    use qip_numerics::anneal::Qubo;
    use qip_observability::Telemetry;
    use qip_optimization_engine::Substrate;
    use qip_optimization_engine::problem::{Objective, PortfolioProblem};
    use qip_optimization_engine::router::RoutingPolicy;
    use qip_quantum::provider::{ProviderCapabilities, QuantumProvider};
    use qip_quantum::qaoa::QaoaResult;
    use qip_risk::limits::LimitSet;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    /// The configuration a composition root reads attaches the simulated
    /// provider and the quantum-inspired rung only when asked, refuses a
    /// value it does not know, and leaves everything off when unset.
    ///
    /// Mutation: in `ComputeSettings::apply`, delete `config =
    /// config.with_quantum();` — `quantum_enabled` stays false and the
    /// simulated assertion fires. Make the `Some(other)` arm return `Off` —
    /// the refusal assertion fires.
    #[test]
    fn the_compute_settings_attach_only_what_is_asked_and_refuse_what_they_do_not_know()
    -> Result<()> {
        let unset = ComputeSettings::from_lookup(&lookup(&[]))?;
        assert_eq!(unset, ComputeSettings::default());
        let off = unset.apply(PlatformConfig::default());
        assert!(!off.quantum_enabled && !off.routing.quantum_inspired);

        let on = ComputeSettings::from_lookup(&lookup(&[
            (QUANTUM_PROVIDER_VARIABLE, "simulated"),
            (QUANTUM_INSPIRED_VARIABLE, "on"),
        ]))?
        .apply(PlatformConfig::default());
        assert!(
            on.quantum_enabled,
            "the simulated provider was not attached"
        );
        assert!(on.routing.quantum_inspired);
        assert_eq!(on.routing.qaoa, SIMULATOR_QAOA);

        for bad in ["hosted", "SIMULATED", "true"] {
            let refusal =
                ComputeSettings::from_lookup(&lookup(&[(QUANTUM_PROVIDER_VARIABLE, bad)]))
                    .expect_err("an unknown provider was accepted");
            assert!(
                refusal.message().starts_with("configuration:"),
                "{}",
                refusal.message()
            );
        }
        Ok(())
    }

    /// Returns the baseline's own selection: a tie, so every trial loses.
    #[derive(Debug)]
    struct Tying(Vec<u8>);

    impl QuantumProvider for Tying {
        fn name(&self) -> &str {
            "tying"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                max_qubits: 64,
                simulated: true,
                noisy: false,
                typical_queue: qip_core::time::Duration::ZERO,
                cost_per_job_micros: 0,
            }
        }
        fn solve_qubo(&self, qubo: &Qubo, _settings: &QaoaSettings) -> Result<QaoaResult> {
            Ok(QaoaResult {
                assignment: self.0.clone(),
                energy: qubo.evaluate(&self.0),
                expectation: 0.0,
                success_probability: 1.0,
                layers: 1,
                angles: Vec::new(),
                evaluations: 1,
            })
        }
    }

    fn problem() -> Result<PortfolioProblem> {
        let n = 12;
        let assets: Vec<String> = (0..n).map(|i| format!("obj-A{i}")).collect();
        let covariance: Vec<Vec<f64>> = (0..n)
            .map(|i| (0..n).map(|j| if i == j { 0.04 } else { 0.012 }).collect())
            .collect();
        Ok(PortfolioProblem::new(assets, covariance)?
            .with_objective(Objective::MeanVariance)
            .with_expected_returns((0..n).map(|i| 0.05 + 0.01 * i as f64).collect())
            .with_risk_aversion(5.0)
            .with_bounds(vec![0.0; n], vec![0.3; n])
            .fully_invested(1.0)
            .with_cardinality(4))
    }

    fn log_path() -> std::path::PathBuf {
        // `/tmp` stated directly rather than looked up: the architecture
        // suite scans every `src/` file's text for a process-environment
        // read, tests included.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::path::PathBuf::from("/tmp").join(format!(
            "qip-kernel-compute-routing-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("events.jsonl")
    }

    fn boot(path: &std::path::Path) -> Result<Platform> {
        let config = PlatformConfig::default().with_event_log_file(path);
        let (context, _clock) = Context::deterministic(start(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
    }

    /// R12-QUANT-4047: a restarted platform's router holds the learning its
    /// previous process journaled, read back from the event log at assembly.
    ///
    /// Mutation: in `Platform::new`, replace
    /// `crate::compute_routing::resume(router, &event_log)?` with `router` —
    /// the restarted router's record is empty and the equality fires. In
    /// `resume`, drop the producer filter's `!=` for `==` — nothing is
    /// folded and the same assertion fires.
    #[test]
    fn a_restarted_platform_resumes_its_routing_learning_from_the_event_log() -> Result<()> {
        let path = log_path();
        let problem = problem()?;
        let policy = RoutingPolicy {
            minimum_assets_for_quantum: 4,
            exact_enumeration_limit: 2,
            ..RoutingPolicy::default()
        };
        let baseline = ComputeRouter::classical(1)
            .with_policy(policy)?
            .solve(&problem)?;
        let tie: Vec<u8> = baseline
            .weights
            .iter()
            .map(|w| u8::from(w.abs() > 1e-9))
            .collect();
        let live = ComputeRouter::classical(1)
            .with_policy(policy)?
            .with_quantum(Arc::new(Tying(tie)))?;

        {
            let mut first = boot(&path)?;
            for cycle in 0..3u64 {
                let decision = live.solve(&problem)?;
                if decision.quantum_dispatch.submitted.is_some() {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
                    while live.quantum_job_ready()? != Some(true) {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "the job never returned"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
                let comparison = SolverRoutingJournal {
                    chosen: decision.chosen.as_str().to_string(),
                    ran: decision
                        .runs
                        .iter()
                        .map(|run| run.solver.as_str().to_string())
                        .collect(),
                    objective: decision.objective,
                    classical_objective: decision.classical_objective,
                    improvement_over_classical: decision.improvement_over_classical(),
                    measured_quantum_advantage: decision.measured_quantum_advantage(),
                    quantum_note: decision.quantum_note.clone(),
                    quantum_dispatch: decision.quantum_dispatch.clone(),
                    compute: decision.compute.clone(),
                };
                first.journal_record(
                    SolverBenchmarked {
                        cycle,
                        family: PORTFOLIO_ALLOCATION_FAMILY.to_string(),
                        instance_size: decision.weights.len(),
                        comparison,
                    },
                    SOLVER_BENCHMARK_ORIGIN,
                    start(),
                )?;
            }
        }
        let journaled = live.learned_record(Substrate::Qpu, 12)?;
        assert_eq!(
            journaled.trials, 2,
            "premise: the journaled cycles carried two judged quantum trials"
        );

        let restarted = boot(&path)?;
        assert_eq!(
            restarted
                .compute_router()
                .learned_record(Substrate::Qpu, 12)?,
            journaled,
            "the restarted router forgot what its log says it learned"
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("the log has a directory"));
        Ok(())
    }
}
