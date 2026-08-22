//! `qip-quantum` — quantum optimisation, honestly scoped.
//!
//! A statevector simulator, a QAOA implementation on top of it, and a provider
//! port with a hosted adapter that reports exactly what a deployment is
//! missing rather than pretending to be usable.
//!
//! The rule this crate exists to make enforceable is that **no quantum result
//! is used without a classical baseline solved on the same problem**. That is
//! not implemented here — it belongs to the compute router in
//! `qip-optimization-engine`, which is the only thing that calls a provider —
//! but everything here is shaped to make it possible: a
//! [`qaoa::QaoaResult`] is a candidate to be checked, and
//! [`provider::ProviderCapabilities::simulated`] means a simulated result can
//! never be reported as hardware evidence.
//!
//! Simulating a QAOA circuit costs more than solving the problem it encodes.
//! Running one against this simulator therefore proves the formulation is
//! right and proves nothing whatever about advantage.

pub mod provider;
pub mod qaoa;
pub mod statevector;

pub use provider::{
    HostedConfig, HostedProvider, ProviderCapabilities, QuantumProvider, SimulatedProvider,
};
pub use qaoa::{QaoaResult, QaoaSettings};
pub use statevector::{Complex, MAX_QUBITS, StateVector};
