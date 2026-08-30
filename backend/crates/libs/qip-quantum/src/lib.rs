//! `qip-quantum` — quantum optimisation, honestly scoped.
//!
//! A statevector simulator, a QAOA implementation on top of it, and a provider
//! port whose hosted adapter reaches IBM Quantum over REST — or reports exactly
//! what a deployment is missing, rather than pretending to be usable.
//!
//! The rule this crate exists to make enforceable is that **no quantum result
//! is used without a classical baseline solved on the same problem**. That is
//! not implemented here — it belongs to the compute router in
//! `qip-optimization-engine`, which is the only thing that calls a provider —
//! but everything here is shaped to make it possible: a
//! [`qaoa::QaoaResult`] is a candidate to be checked, and
//! [`provider::ProviderCapabilities::simulated`] means a simulated result can
//! never be reported as hardware evidence. The hosted adapter keeps that flag
//! honest the only way a remote device permits: it asks the service whether the
//! backend is a simulator and **refuses to return a result** when the service
//! says yes, rather than running and relabelling itself. See
//! [`provider::HostedProvider`].
//!
//! Simulating a QAOA circuit costs more than solving the problem it encodes.
//! Running one against this simulator therefore proves the formulation is
//! right and proves nothing whatever about advantage.
//!
//! ## The three-way comparison
//!
//! [`solver`] puts three things behind one trait — the classical searches the
//! platform already had, a genuine quantum-inspired search (path-integral
//! quantum annealing over the same QUBO), and the IBM Quantum port — and
//! [`benchmark`] runs them on one problem. Two rules are enforced there by
//! type rather than by convention: a [`benchmark::BenchmarkReport`] carries
//! its classical baseline in a non-optional field, and the only usable answer
//! is a [`benchmark::ValidatedSolution`], whose sole constructor re-evaluates
//! the assignment classically and refuses a claim that does not match.

pub mod benchmark;
pub mod provider;
pub mod qaoa;
pub mod solver;
pub mod statevector;

pub use benchmark::{
    BenchmarkReport, ClassicalValidation, ClassicalValidator, QualityMeasure, Reliability,
    SolverBenchmark, SolverRecord, ValidatedSolution,
};
pub use provider::{
    ConfirmedDevice, HostedConfig, HostedProvider, HostedStats, HostedToken, HostedTransport,
    ProviderCapabilities, QuantumProvider, SimulatedProvider,
};
pub use qaoa::{QaoaResult, QaoaSettings};
pub use solver::{
    ClassicalSearch, ClassicalSolver, IbmQuantumConfig, IbmQuantumSolver, ProviderSolver,
    QuantumInspiredSolver, QuboSolver, QueuePolicy, SearchTrace, SolverCandidate, SolverCostModel,
    SolverEffort, SolverKind,
};
pub use statevector::{Complex, MAX_QUBITS, StateVector};
