//! The problem compiler (R12-QUANT-4048, 4049, 4050).

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::Duration;
use qip_optimization_engine::{Formulation, TaskClass, TaskSpec};

/// Removes one declared property from a spec.
type Strip = fn(&mut TaskSpec);

fn complete(class: TaskClass, discrete: bool) -> TaskSpec {
    TaskSpec {
        class: Some(class),
        variables: Some(12),
        constraints: Some(vec!["budget".to_string()]),
        objective: Some("mean-variance".to_string()),
        tolerance: Some(1e-6),
        deadline: Some(Duration::from_secs(60)),
        verification: Some(vec!["re-scored on the real problem".to_string()]),
        classical: Some(Formulation::SimulatedAnnealing),
        discrete,
    }
}

#[test]
fn a_task_missing_any_declared_property_is_refused_by_name_rather_than_defaulted() -> Result<()> {
    // Premise: the complete spec compiles, so each refusal below is the one
    // field taken away.
    complete(TaskClass::CombinatorialOptimisation, true).compile()?;

    let cases: [(&str, Strip); 8] = [
        ("a task class", |s| s.class = None),
        ("a variable count", |s| s.variables = None),
        ("its constraints", |s| s.constraints = None),
        ("an objective", |s| s.objective = None),
        ("a tolerance", |s| s.tolerance = None),
        ("a deadline", |s| s.deadline = None),
        ("its verification properties", |s| s.verification = None),
        ("a classical formulation", |s| s.classical = None),
    ];
    for (name, strip) in cases {
        let mut spec = complete(TaskClass::CombinatorialOptimisation, true);
        strip(&mut spec);
        let refusal = spec
            .compile()
            .expect_err("a task with a missing property compiled");
        assert!(
            refusal.message().contains(&format!("without {name};")),
            "removing {name} gave: {}",
            refusal.message()
        );
    }
    Ok(())
}

#[test]
fn a_compiled_task_always_has_a_classical_formulation_and_a_quantum_one_only_where_eligible()
-> Result<()> {
    // Every class compiles, and each one names a variant: eight, none of them
    // a catch-all.
    assert_eq!(TaskClass::ALL.len(), 8);
    for class in TaskClass::ALL {
        let task = complete(class, true).compile()?;
        assert!(!task.classical.is_quantum());
        let eligible = matches!(
            class,
            TaskClass::CombinatorialOptimisation | TaskClass::GraphSearch
        );
        assert_eq!(task.quantum.is_some(), eligible, "{class:?}");
    }
    // A continuous instance of an eligible class has no quantum formulation.
    let continuous = complete(TaskClass::CombinatorialOptimisation, false).compile()?;
    assert!(continuous.quantum.is_none());

    // And a quantum form offered as the baseline is refused (ADR 0006).
    let mut spec = complete(TaskClass::CombinatorialOptimisation, true);
    spec.classical = Some(Formulation::Qubo);
    assert!(spec.compile().is_err());
    Ok(())
}

#[test]
fn a_tolerance_or_deadline_that_cannot_mean_anything_is_refused() {
    for tolerance in [0.0, -1.0, f64::INFINITY, f64::NAN] {
        let mut spec = complete(TaskClass::Sampling, false);
        spec.tolerance = Some(tolerance);
        assert!(spec.compile().is_err(), "tolerance {tolerance} compiled");
    }
    let mut spec = complete(TaskClass::Sampling, false);
    spec.deadline = Some(Duration::ZERO);
    assert!(spec.compile().is_err(), "a zero deadline compiled");
    let mut spec = complete(TaskClass::Sampling, false);
    spec.verification = Some(Vec::new());
    assert!(
        spec.compile().is_err(),
        "a task with nothing to verify compiled"
    );
}
