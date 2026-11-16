//! R12-QUANT-4029: the only path that reaches a quantum provider submits a
//! job and returns.
//!
//! `QuantumProvider::solve_qubo` is a public trait method, so the boundary the
//! gateway draws is a convention until something reads the source. The failure
//! this prevents: a new caller invokes `provider.solve_qubo(..)` on the cycle
//! thread and a hosted queue stops DECIDE. The gateway runs the call on a
//! worker thread; `ProviderSolver` refuses a non-simulated provider before the
//! call (`solvers.rs`). Any other production call site is a path the
//! slow-provider tests do not cover, and fails here.

#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::{files_with_extension, production_text};

/// The only files that may name the call, and what makes each safe.
const ALLOWED: [&str; 2] = [
    // Runs on the gateway's worker thread; the cycle polls with try_recv.
    "libs/qip-quantum/src/gateway.rs",
    // ProviderSolver::solve returns Error::denied before this line for any
    // provider whose capabilities().simulated is false.
    "libs/qip-quantum/src/solver.rs",
];

#[test]
fn no_production_code_calls_a_provider_except_the_gateway_and_the_simulated_only_solver() {
    let mut allowed_hits = 0usize;
    let mut strangers = Vec::new();
    for path in files_with_extension("backend/crates", "rs") {
        let text = path.to_string_lossy().replace('\\', "/");
        if text.contains("/tests/") || text.contains("/target/") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("a readable source file");
        let production = production_text(&source);
        let calls = production.matches(".solve_qubo(").count();
        if calls == 0 {
            continue;
        }
        if ALLOWED.iter().any(|allowed| text.ends_with(allowed)) {
            allowed_hits += calls;
        } else {
            strangers.push(format!("{text}: {calls} call(s)"));
        }
    }
    // The premise: the two sanctioned sites exist, so an empty scan cannot
    // pass for a boundary that held.
    assert!(
        allowed_hits >= 2,
        "the gateway and the solver no longer call solve_qubo ({allowed_hits}); the scan reads nothing"
    );
    assert!(
        strangers.is_empty(),
        "a quantum provider is called outside the gateway: {strangers:?}; submit through \
         QuantumGateway::submit and poll instead"
    );
}
