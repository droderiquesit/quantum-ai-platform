//! The router's decision, stated in the solver-agnostic contract
//! (R12-QUANT-4063).
//!
//! `qip_contracts::quantum` is the vocabulary every solver and every reader
//! of the log shares: a [`DecisionRequest`] (the QUBO), a [`SolverResult`] per
//! solver, and a [`qip_contracts::quantum::RoutingDecision`] that cannot exist
//! without a classical result on the same request. The compute router speaks
//! its own types, so until this module nothing in production built any of the
//! three, and the log held the router's account of a solve but not the one
//! the contract was written for.
//!
//! This is a conversion and nothing else. It recomputes each result's
//! objective from the QUBO rather than copying the router's figure, because
//! the contract's whole point is that a claimed objective is a number anyone
//! can re-derive from the assignment.

use qip_contracts::quantum::{
    ChosenPath, DecisionRequest, RoutingDecision, SolverKind, SolverResult,
};
use qip_core::error::{Error, Result};
use qip_events::{EventBody, Topic};
use qip_optimization_engine::router::RoutedQubo;
use qip_optimization_engine::{RoutingDecision as RouterDecision, Solver, SolverRun};
use serde::{Deserialize, Serialize};

/// The producer on every contract decision record.
pub const QUANTUM_ROUTING_ORIGIN: &str = "kernel/quantum-routing";

/// One cycle's solve, as the contract states it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuantumRoutingRecorded {
    /// The cycle whose DECIDE stage ran the solve.
    pub cycle: u64,
    /// The binary problem every result below was scored on.
    pub request: DecisionRequest,
    /// The classical baseline, always, and the quantum or quantum-inspired
    /// result beside it when one ran.
    pub decision: RoutingDecision,
}

impl EventBody for QuantumRoutingRecorded {
    const TOPIC: Topic = Topic::OptimizationCompleted;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("quantum-routing:{}", self.cycle))
    }
}

/// The contract's account of `routing`, or `None` for a continuous problem,
/// which has no QUBO and so nothing the contract can state.
pub fn contract_of(cycle: u64, routing: &RouterDecision) -> Result<Option<QuantumRoutingRecorded>> {
    let Some(qubo) = routing.qubo.as_ref() else {
        return Ok(None);
    };
    let request = DecisionRequest::new(
        format!("portfolio-allocation:{cycle}"),
        qubo.variables,
        qubo.entries.clone(),
        qubo.offset,
    )?;
    let classical = routing
        .runs
        .first()
        .ok_or_else(|| Error::invalid("a routing decision with no run has no baseline"))?;
    let classical = result_of(SolverKind::Classical, classical, qubo, &request)?;
    let quantum = routing
        .run_for(Solver::Quantum)
        .map(|run| (SolverKind::Quantum, run))
        .or_else(|| {
            routing
                .run_for(Solver::QuantumInspired)
                .map(|run| (SolverKind::QuantumInspired, run))
        })
        .map(|(kind, run)| result_of(kind, run, qubo, &request))
        .transpose()?;
    // The contract has two paths. The quantum-inspired search is a classical
    // algorithm and is never "the quantum path", so a decision that chose it
    // is recorded as the classical path with the inspired result beside it.
    let chosen = if routing.chosen == Solver::Quantum {
        ChosenPath::Quantum
    } else {
        ChosenPath::Classical
    };
    let decision = RoutingDecision::new(classical, quantum, chosen)?;
    Ok(Some(QuantumRoutingRecorded {
        cycle,
        request,
        decision,
    }))
}

/// One run as a contract result. The assignment is the one the run recorded
/// and the objective is recomputed from it.
///
/// `cost_nanos` is zero: the router reads no clock, so it measured no
/// compute time, and the field is modelled, not measured.
fn result_of(
    kind: SolverKind,
    run: &SolverRun,
    qubo: &RoutedQubo,
    request: &DecisionRequest,
) -> Result<SolverResult> {
    let assignment = run.assignment.clone().ok_or_else(|| {
        Error::invalid(format!(
            "the {} run over a QUBO recorded no assignment, so the contract cannot state it",
            run.solver.as_str()
        ))
    })?;
    let objective =
        SolverResult::recompute_objective(&assignment, request.qubo_terms(), request.constant());
    debug_assert_eq!(assignment.len(), qubo.variables);
    SolverResult::new(kind, assignment, objective, 0, request.id())
}
