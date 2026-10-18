//! QUANT-014: Quantum processing never confers authenticity or truth on evidence
//!
//! Verifies that quantum result output cannot modify evidence authenticity,
//! reliability or trust status. A quantum result is typed as an advisory
//! candidate that must be independently verified and cannot set evidence fields.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::quantum::{SolverKind, SolverResult};

/// QUANT-014-1: Quantum results remain typed as advisory candidates.
///
/// Mutation: Change `needs_classical_baseline()` to return false for quantum
/// results. The test catches if quantum results lose their advisor-only typing.
#[test]
fn quantum_results_are_typed_as_advisory_candidates() {
    // A quantum result must carry its kind so it can be verified
    let quantum_result = SolverResult::new(
        SolverKind::Quantum,
        vec![true, false, true],
        42.5,
        1_000_000,
        "test_problem",
    )
    .expect("valid quantum result");

    // The key invariant: quantum results need classical baseline verification
    assert!(
        quantum_result.kind().needs_classical_baseline(),
        "quantum results must be marked as needing classical baseline verification"
    );

    // This type constraint ensures that quantum results cannot become
    // direct evidence without passing through classical validation
    assert_eq!(
        quantum_result.kind(),
        SolverKind::Quantum,
        "result type must indicate it is a quantum suggestion, not proven truth"
    );
}

/// QUANT-014-2: Quantum-inspired results also need classical verification.
///
/// Mutation: Change `needs_classical_baseline()` to return false for
/// quantum-inspired results. The test catches if quantum-inspired results
/// bypass classical comparison.
#[test]
fn quantum_inspired_results_remain_advisory() {
    let qi_result = SolverResult::new(
        SolverKind::QuantumInspired,
        vec![false, true, false, true],
        88.3,
        500_000,
        "qi_problem",
    )
    .expect("valid qi result");

    // Quantum-inspired solvers also need classical baseline comparison
    assert!(
        qi_result.kind().needs_classical_baseline(),
        "quantum-inspired results must be compared against classical solver"
    );
}

/// QUANT-014-3: Only classical results bypass baseline requirement.
///
/// Mutation: Change classical results to also need baseline. The test
/// ensures that classical results are the ground truth and do not need
/// comparison (though the same instance might be compared for other reasons).
#[test]
fn classical_results_are_the_baseline() {
    let classical_result = SolverResult::new(
        SolverKind::Classical,
        vec![true, true, false],
        50.0,
        250_000,
        "classical_problem",
    )
    .expect("valid classical result");

    // Classical results are the baseline and do not need verification
    // against another solver
    assert!(
        !classical_result.kind().needs_classical_baseline(),
        "classical results are the baseline itself"
    );
}

/// QUANT-014-4: Quantum results cannot be used without explicit validation.
///
/// Mutation: Remove the `is_valid_for` check or make it always pass.
/// The test ensures quantum results must match their request and cannot
/// be applied to arbitrary problems (which would hide evidence modification).
#[test]
fn quantum_results_must_be_validated_against_request() {
    use qip_contracts::quantum::DecisionRequest;

    let request = DecisionRequest::new(
        "test_q",
        3,
        vec![(0, 0, 1.0), (1, 1, 2.0), (0, 1, 3.0)],
        0.0,
    )
    .expect("valid request");

    // Valid result for this request
    let valid = SolverResult::new(
        SolverKind::Quantum,
        vec![true, false, true],
        4.0,
        1_000_000,
        "test_q",
    )
    .expect("valid quantum result");

    assert!(
        valid.is_valid_for(&request),
        "correctly formed result must validate"
    );

    // Invalid result: wrong request ID
    let wrong_id = SolverResult::new(
        SolverKind::Quantum,
        vec![true, false, true],
        4.0,
        1_000_000,
        "wrong_request",
    )
    .expect("valid result structure");

    assert!(
        !wrong_id.is_valid_for(&request),
        "result with wrong request ID must fail validation"
    );

    // Invalid result: wrong assignment size
    let wrong_size = SolverResult::new(
        SolverKind::Quantum,
        vec![true, false], // Only 2 bits, need 3
        4.0,
        1_000_000,
        "test_q",
    )
    .expect("valid result structure");

    assert!(
        !wrong_size.is_valid_for(&request),
        "result with wrong size must fail validation"
    );

    // This validation ensures quantum results cannot be applied to the wrong
    // problem, which would allow misuse as evidence modification.
}

/// The result and routing types of the quantum contract, and the crate that
/// computes them. Whole words.
const QUANTUM_RESULT_NAMES: [&str; 6] = [
    "SolverResult",
    "SolverKind",
    "DecisionRequest",
    "RoutingDecision",
    "ChosenPath",
    "qip_quantum",
];

fn quantum_names_in(text: &str) -> Vec<&'static str> {
    let words: std::collections::BTreeSet<&str> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    QUANTUM_RESULT_NAMES
        .into_iter()
        .filter(|name| words.contains(name))
        .collect()
}

#[test]
fn evidence_the_world_model_and_the_reasoning_engine_do_not_name_a_quantum_result_type() {
    // R12-EVID-2040 (QUANT-014): no path from a quantum result type to an
    // authenticity or belief field. `architecture.rs` proves these crates do
    // not link the solvers; this proves they also do not name the contract
    // types a result travels in, which they could do through `qip-contracts`
    // alone. A `fn from(result: SolverResult) -> Belief` would need only that.
    let root = qip_acceptance::repository_root();
    let mut scanned = 0usize;
    let mut offenders: Vec<(String, Vec<&'static str>)> = Vec::new();
    for crate_dir in [
        "backend/crates/libs/qip-evidence",
        "backend/crates/services/qip-world-model",
        "backend/crates/services/qip-reasoning-engine",
    ] {
        let mut files = qip_acceptance::files_with_extension(&format!("{crate_dir}/src"), "rs");
        files.push(root.join(crate_dir).join("Cargo.toml"));
        for file in files {
            scanned += 1;
            let text = std::fs::read_to_string(&file)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
            let manifest = file.extension().is_some_and(|e| e == "toml");
            let found = if manifest {
                // A manifest names the crate with a hyphen.
                let hyphenated = text.replace("qip-quantum", "qip_quantum");
                quantum_names_in(&hyphenated)
            } else {
                quantum_names_in(&qip_acceptance::production_text(&text))
            };
            if !found.is_empty() {
                offenders.push((
                    file.strip_prefix(&root)
                        .unwrap_or(&file)
                        .to_string_lossy()
                        .into_owned(),
                    found,
                ));
            }
        }
    }
    assert!(scanned > 30, "only {scanned} files were scanned");
    assert_eq!(
        offenders,
        Vec::<(String, Vec<&'static str>)>::new(),
        "a crate that holds evidence or belief names a quantum result type"
    );

    // The scan sees a type as a word and a dependency by either spelling.
    assert_eq!(
        quantum_names_in("fn f(r: SolverResult) -> ChosenPath { todo }"),
        vec!["SolverResult", "ChosenPath"]
    );
    assert!(quantum_names_in("let solver_results = 1;").is_empty());
}
