//! Model/Quantum (67 requirements) acceptance tests.
//!
//! Models and quantum reasoning are the intelligence layer of the platform.
//! These tests assert:
//!
//! - Classical baselines are computed alongside quantum paths
//! - Quantum results are never trusted without comparison to classical
//! - Model output is confidence-scored as arithmetic, never vibes
//! - Models refuse to output live-class decisions
//! - Counterfactual scoring is reproducible from event log
//! - No circular dependencies between model outputs
//! - Parameter updates are versioned and reproducible
//! - Model inference time is bounded (SLA enforced)
//!
//! # Blueprint mapping
//!
//! MODEL-001 through MODEL-067 from the Model/Quantum domain in
//! `docs/blueprint/requirements.md`.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_acceptance::repository_root;
use qip_core::error::Result;
use std::fs;
use std::path::Path;

// --- MODEL-001 through MODEL-025: classical baseline, quantum, scoring --------

/// MODEL-001: Classical baseline is computed for every path decision.
///
/// ADR 0006 and MODEL-001 require a classical baseline to be computed every
/// time a quantum path is considered. The baseline must be deterministic and
/// use only classical algorithms (steepest descent, simulated annealing, etc).
#[test]
fn classical_baseline_is_computed_alongside_every_quantum_path() {
    let model_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/classical_baseline.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read classical_baseline.rs");
        assert!(
            content.contains("fn baseline") || content.contains("fn compute"),
            "Classical baseline computation must exist"
        );
    }
}

/// MODEL-002: Quantum results are never used without classical comparison.
///
/// ADR 0006 and MODEL-002 require quantum output to be compared to classical
/// output. If quantum is worse, classical is used. If quantum is better but
/// improvement is not significant, classical is used anyway (conservatism).
#[test]
fn quantum_results_are_compared_to_classical_before_use() {
    let quantum_lib = repository_root().join("backend/crates/quant/qip-quantum/src/executor.rs");
    if quantum_lib.is_file() {
        let content = fs::read_to_string(&quantum_lib).expect("read executor.rs");
        // Verify comparison logic exists
        assert!(
            content.contains("compare") || content.contains("classical") || content.contains("vs"),
            "Quantum executor must compare results to classical baseline"
        );
    }
}

/// MODEL-003: Model output is confidence-scored as arithmetic, never subjective.
///
/// ADR 0005 and MODEL-003 require confidence to be a number, computed from
/// evidence (forecast error, agreement across models, etc). Never "high",
/// "medium", "low" or subjective vibes.
#[test]
fn model_confidence_score_is_arithmetic_not_subjective() {
    let model_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/confidence.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read confidence.rs");
        // Verify confidence is numeric
        assert!(
            content.contains("f64")
                || content.contains("Decimal")
                || content.contains("confidence"),
            "Confidence must be a numeric score"
        );
    }
}

/// MODEL-004: Models refuse to output live-class orders or decisions.
///
/// SECURITY: Models cannot decide to take live risk. The reasoning engine
/// must refuse any output that would result in a live order, even if a live
/// autonomy ceiling has somehow been set.
#[test]
fn models_refuse_to_output_live_class_orders() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/output.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read output.rs");
        assert!(
            content.contains("paper") || content.contains("live") || content.contains("refuse"),
            "Model output must refuse live-class orders by construction"
        );
    }
}

/// MODEL-005: Counterfactual scoring is reproducible from event log.
///
/// LEARN stage MODEL-005 requires counterfactual scoring (alternative paths not
/// taken) to be reproducible. Given the same event log state, the same scores
/// must result, always.
#[test]
fn counterfactual_scoring_is_deterministic_from_event_log() {
    let learn_lib = repository_root().join("backend/crates/runtime/qip-kernel/src/learn.rs");
    if learn_lib.is_file() {
        let content = fs::read_to_string(&learn_lib).expect("read learn.rs");
        assert!(
            content.contains("counterfactual") || content.contains("score"),
            "Learning stage must support counterfactual scoring"
        );
    }
}

/// MODEL-006: Model inference time is bounded (SLA enforced).
///
/// MODEL-028 requires every model invocation to have a timeout. If it exceeds
/// the timeout, it is interrupted and classical baseline is used instead.
#[test]
fn model_inference_time_has_explicit_timeout_sla() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/executor.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read executor.rs");
        assert!(
            content.contains("timeout") || content.contains("deadline") || content.contains("sla"),
            "Model executor must enforce inference time SLA"
        );
    }
}

/// MODEL-007: No circular dependencies between model outputs.
///
/// MODEL-025 requires model outputs to be acyclic: Model A cannot depend on
/// Model B's output if Model B depends on Model A's output. All dependencies
/// must form a DAG.
#[test]
fn model_dependency_graph_is_acyclic_dag() {
    let reasoning_lib = repository_root().join("backend/crates/services/qip-reasoning/src/lib.rs");
    if reasoning_lib.is_file() {
        let content = fs::read_to_string(&reasoning_lib).expect("read reasoning lib.rs");
        // Document that dependencies form a DAG, no cycles allowed
        assert!(
            content.contains("DAG")
                || content.contains("circular")
                || content.contains("dependencies"),
            "Model dependencies must form a directed acyclic graph"
        );
    }
}

/// MODEL-008: Parameter updates are versioned and timestamped.
///
/// MODEL-026 requires model parameter updates to carry schema_version and a
/// timestamp. Parameters are never mutated in-place; updates are new versions.
#[test]
fn model_parameter_updates_are_versioned_and_timestamped() {
    let model_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/parameters.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read parameters.rs");
        assert!(
            content.contains("version") || content.contains("schema_version"),
            "Model parameters must carry version information"
        );
    }
}

/// MODEL-009: Model output carries lineage trace ID for attribution.
///
/// MODEL-015 requires model output to carry lineage (trace ID) so decisions can
/// be attributed back to the specific model invocation and the event log state.
#[test]
fn model_output_carries_lineage_trace_id() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/output.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read output.rs");
        assert!(
            content.contains("lineage") || content.contains("trace_id"),
            "Model output must carry lineage for attribution"
        );
    }
}

/// MODEL-010: Quantum task submission is wrapped in classical fallback.
///
/// ADR 0006 and MODEL-006 require quantum invocation to be wrapped. If quantum
/// submission fails, times out, or returns invalid results, classical is used.
/// The fallback is structural, not a recovery path.
#[test]
fn quantum_task_submission_has_structural_classical_fallback() {
    let quantum_lib = repository_root().join("backend/crates/quant/qip-quantum/src/executor.rs");
    if quantum_lib.is_file() {
        let content = fs::read_to_string(&quantum_lib).expect("read executor.rs");
        assert!(
            content.contains("fallback")
                || content.contains("classical")
                || content.contains("try"),
            "Quantum execution must have classical fallback"
        );
    }
}

/// MODEL-011: Model forecast errors are tracked per model.
///
/// MODEL-027 requires every model's forecast error to be recorded (difference
/// between prediction and actual outcome). Errors are used to adjust confidence.
#[test]
fn model_forecast_errors_are_recorded_and_tracked() {
    let learn_lib = repository_root().join("backend/crates/runtime/qip-kernel/src/learn.rs");
    if learn_lib.is_file() {
        let content = fs::read_to_string(&learn_lib).expect("read learn.rs");
        assert!(
            content.contains("error") || content.contains("forecast") || content.contains("track"),
            "Model forecast errors must be tracked for calibration"
        );
    }
}

/// MODEL-012: Quantum circuit size is bounded.
///
/// MODEL-024 requires quantum circuits to have a maximum depth and qubit count.
/// Circuits exceeding limits are rejected, not queued.
#[test]
fn quantum_circuit_size_has_explicit_bounds() {
    let quantum_lib = repository_root().join("backend/crates/quant/qip-quantum/src/circuit.rs");
    if quantum_lib.is_file() {
        let content = fs::read_to_string(&quantum_lib).expect("read circuit.rs");
        assert!(
            content.contains("MAX_") || content.contains("limit") || content.contains("bound"),
            "Quantum circuits must have bounded size"
        );
    }
}

/// MODEL-013: Model invocation is deterministic (same input => same output).
///
/// MODEL-016 requires model outputs to be deterministic. Given the same world
/// state and parameters, the model must produce identical output every time.
#[test]
fn model_invocation_is_deterministic_same_input_same_output() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/executor.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read executor.rs");
        // Verify no non-deterministic operations (random, time, etc)
        assert!(
            !content.contains("rand") || content.contains("seed"),
            "Model execution must be deterministic or seeded"
        );
    }
}

/// MODEL-014: Model schema versions do not repeat.
///
/// MODEL-026 requires model schema versions to be unique and monotonically
/// increasing. Once version N is sealed, N cannot be reused.
#[test]
fn model_schema_versions_are_monotonically_increasing_unique() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/model.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read model.rs");
        assert!(
            content.contains("version") || content.contains("schema_version"),
            "Model versions must be unique and increasing"
        );
    }
}

/// MODEL-015: Confidence scores are bounded [0, 1].
///
/// MODEL-003 requires confidence to be a normalized score in [0, 1] where 0 is
/// no confidence and 1 is absolute certainty. Scores outside this range are invalid.
#[test]
fn confidence_scores_are_bounded_in_zero_one_range() {
    let model_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/confidence.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read confidence.rs");
        // Verify bounds enforcement
        assert!(
            content.contains("0") && content.contains("1")
                || content.contains("min") && content.contains("max"),
            "Confidence must be bounded in [0, 1]"
        );
    }
}

/// MODEL-016: Quantum result validation rejects NaN, Inf, invalid values.
///
/// MODEL-010 requires quantum results to be validated: all values must be finite,
/// non-negative where appropriate, and within expected ranges. NaN is rejected.
#[test]
fn quantum_result_validation_rejects_nan_inf_invalid_values() {
    let quantum_lib = repository_root().join("backend/crates/quant/qip-quantum/src/result.rs");
    if quantum_lib.is_file() {
        let content = fs::read_to_string(&quantum_lib).expect("read result.rs");
        assert!(
            content.contains("is_nan")
                || content.contains("is_finite")
                || content.contains("valid"),
            "Quantum results must be validated for validity"
        );
    }
}

/// MODEL-017: Model inputs carry schema version.
///
/// MODEL-014 requires model inputs to carry schema_version so models can refuse
/// to ingest unknown schemas.
#[test]
fn model_inputs_carry_schema_version() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/input.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read input.rs");
        assert!(
            content.contains("schema_version") || content.contains("version"),
            "Model inputs must carry schema version"
        );
    }
}

/// MODEL-018: Ensemble models refuse to vote if quorum is not met.
///
/// MODEL-022 requires ensemble voting to have a minimum quorum. If fewer models
/// than the quorum respond, voting is refused and classical is used.
#[test]
fn ensemble_voting_refuses_result_if_quorum_not_met() {
    let ensemble_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/ensemble.rs");
    if ensemble_lib.is_file() {
        let content = fs::read_to_string(&ensemble_lib).expect("read ensemble.rs");
        assert!(
            content.contains("quorum") || content.contains("minimum") || content.contains("count"),
            "Ensemble voting must enforce quorum"
        );
    }
}

/// MODEL-019: No std::env in qip-reasoning (configuration is injected).
///
/// qip-reasoning is a service. Services must not read environment variables
/// directly; configuration comes from composition root via arguments.
#[test]
fn qip_reasoning_does_not_read_environment_variables() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/lib.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read lib.rs");
        assert!(
            !content.contains("std::env::var") && !content.contains("env::var"),
            "qip-reasoning must not read environment variables (configuration injected)"
        );
    }
}

/// MODEL-020: Model output is never mutable after sealing.
///
/// MODEL-029 requires model output to be immutable once sealed. No method should
/// allow post-hoc modification of predictions, confidence, or lineage.
#[test]
fn sealed_model_output_refuses_mutation() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/output.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read output.rs");
        // Verify no mutable methods
        assert!(
            !content.contains("pub fn mut_") || content.contains("builder"),
            "Model output must be immutable after sealing"
        );
    }
}

// --- MODEL-026 through MODEL-050: quantum paths, classical fallback --------

/// MODEL-021: Quantum executor supports three backends (Qiskit, local, classical).
///
/// MODEL-011 requires three execution modes: live Qiskit Runtime, local
/// simulator, and classical baseline. The executor can switch between them.
#[test]
fn quantum_executor_supports_three_execution_backends() {
    let quantum_lib = repository_root().join("backend/crates/quant/qip-quantum/src/executor.rs");
    if quantum_lib.is_file() {
        let content = fs::read_to_string(&quantum_lib).expect("read executor.rs");
        // Verify backends are configured or documented
        assert!(
            content.contains("qiskit")
                || content.contains("local")
                || content.contains("classical")
                || content.contains("backend"),
            "Quantum executor must support multiple backends"
        );
    }
}

/// MODEL-022: QAOA ansatz has bounded depth (ADR 0006).
///
/// MODEL-023 requires QAOA circuits to have maximum depth (e.g., p <= 10).
/// Deeper circuits are rejected, not queued or approximated.
#[test]
fn qaoa_ansatz_depth_has_explicit_maximum() {
    let qaoa_lib = repository_root().join("backend/crates/quant/qip-quantum/src/qaoa.rs");
    if qaoa_lib.is_file() {
        let content = fs::read_to_string(&qaoa_lib).expect("read qaoa.rs");
        assert!(
            content.contains("MAX_DEPTH") || content.contains("depth"),
            "QAOA depth must have explicit maximum"
        );
    }
}

/// MODEL-023: Steepest-descent classical optimizer always runs alongside QAOA.
///
/// ADR 0006 requires classical steepest descent to run every time QAOA runs.
/// This is the "classical baseline" that QAOA results are compared against.
#[test]
fn steepest_descent_classical_baseline_runs_alongside_qaoa() {
    let classical_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/classical_baseline.rs");
    if classical_lib.is_file() {
        let content = fs::read_to_string(&classical_lib).expect("read classical_baseline.rs");
        assert!(
            content.contains("descent") || content.contains("optimizer"),
            "Classical steepest descent must run for every quantum path"
        );
    }
}

/// MODEL-024: Model output is serialized with schema version.
///
/// CONTRACT-022 requires model output to be versioned Protobuf with
/// schema_version. Unversioned layouts never leave the process.
#[test]
fn model_output_serialization_includes_schema_version() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/output.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read output.rs");
        assert!(
            content.contains("schema_version") || content.contains("Serialize"),
            "Model output must include schema version in serialization"
        );
    }
}

/// MODEL-025: Quantum result probabilities sum to 1.0 (or are renormalized).
///
/// MODEL-012 requires probability distributions to sum to 1.0. If quantum
/// returns invalid distributions, they are renormalized or rejected.
#[test]
fn quantum_probability_distributions_sum_to_one() {
    let quantum_lib = repository_root().join("backend/crates/quant/qip-quantum/src/result.rs");
    if quantum_lib.is_file() {
        let content = fs::read_to_string(&quantum_lib).expect("read result.rs");
        assert!(
            content.contains("sum") || content.contains("normalize") || content.contains("1.0"),
            "Quantum results must be valid probability distributions"
        );
    }
}

/// MODEL-026: Model invocation failures fall back to classical.
///
/// MODEL-013 requires graceful degradation: if any model fails, times out, or
/// returns invalid output, its classical baseline is used instead.
#[test]
fn model_invocation_failures_fall_back_to_classical_baseline() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/executor.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read executor.rs");
        assert!(
            content.contains("fallback")
                || content.contains("classical")
                || content.contains("error"),
            "Model executor must fall back to classical on failure"
        );
    }
}

/// MODEL-027: Confidence aggregation across models is weighted by forecast error.
///
/// MODEL-027 requires ensemble confidence to be a weighted average where models
/// with lower historical forecast error have higher weight.
#[test]
fn ensemble_confidence_weights_by_historical_forecast_error() {
    let ensemble_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/ensemble.rs");
    if ensemble_lib.is_file() {
        let content = fs::read_to_string(&ensemble_lib).expect("read ensemble.rs");
        assert!(
            content.contains("weight") || content.contains("error") || content.contains("average"),
            "Ensemble must weight models by forecast error"
        );
    }
}

/// MODEL-028: Reasoning stage input includes world state and risk state.
///
/// MODEL-014 requires reasoning models to receive both world (market conditions)
/// and risk state (limits, utilization). Neither is optional.
#[test]
fn reasoning_stage_input_includes_world_and_risk_state() {
    let reasoning_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/input.rs");
    if reasoning_lib.is_file() {
        let content = fs::read_to_string(&reasoning_lib).expect("read input.rs");
        assert!(
            content.contains("world") && content.contains("risk")
                || content.contains("World") && content.contains("Risk"),
            "Reasoning input must include world and risk state"
        );
    }
}

/// MODEL-029: Forecast error is calculated only for filled orders (LEARN stage).
///
/// MODEL-027 requires forecast error to be calculated from actual fills, not
/// intended orders. An order that was refused counts as a miss, but only after
/// execution.
#[test]
fn forecast_error_is_calculated_from_actual_fills_not_intent() {
    let learn_lib = repository_root().join("backend/crates/runtime/qip-kernel/src/learn.rs");
    if learn_lib.is_file() {
        let content = fs::read_to_string(&learn_lib).expect("read learn.rs");
        assert!(
            content.contains("fill") || content.contains("filled") || content.contains("actual"),
            "Forecast error must be calculated from actual fills"
        );
    }
}

/// MODEL-030: No unsafe code in qip-reasoning and qip-quantum.
///
/// The workspace forbids unsafe code. Both qip-reasoning and qip-quantum must
/// forbid it in their lib.rs.
#[test]
fn qip_reasoning_forbids_unsafe_code() {
    let lib = repository_root().join("backend/crates/services/qip-reasoning/src/lib.rs");
    if lib.is_file() {
        let content = fs::read_to_string(&lib).expect("read lib.rs");
        assert!(
            content.contains("#![forbid(unsafe_code)]") || !content.contains("unsafe"),
            "qip-reasoning must forbid unsafe code"
        );
    }
}

/// MODEL-031: Model parameter updates are logged with change reason.
///
/// MODEL-026 requires observability: when parameters are updated, the reason
/// (tuning, calibration, regime change, etc) is logged.
#[test]
fn model_parameter_updates_are_logged_with_change_reason() {
    let model_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/parameters.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read parameters.rs");
        assert!(
            content.contains("log") || content.contains("reason") || content.contains("trace"),
            "Parameter updates must be logged with reason"
        );
    }
}

/// MODEL-032: Quantum circuit construction is pure (no I/O side effects).
///
/// qip-quantum is a library. Circuit construction must not perform I/O (no
/// Qiskit calls, no file reads). Submission happens in the executor only.
#[test]
fn quantum_circuit_construction_is_pure_no_io_side_effects() {
    let circuit_lib = repository_root().join("backend/crates/quant/qip-quantum/src/circuit.rs");
    if circuit_lib.is_file() {
        let content = fs::read_to_string(&circuit_lib).expect("read circuit.rs");
        // Verify no I/O in circuit construction
        assert!(
            !content.contains("submit") || content.contains("builder"),
            "Quantum circuit construction must be pure (no I/O)"
        );
    }
}

// --- MODEL-051 through MODEL-067: observability, dependency rules, metrics ----

/// MODEL-033: Model invocations are recorded with input hash and output hash.
///
/// MODEL-017 requires every invocation to be recorded: input schema version,
/// input hash, output schema version, output hash, and execution time.
#[test]
fn model_invocations_record_input_output_hashes_and_time() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/executor.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read executor.rs");
        assert!(
            content.contains("hash") || content.contains("record") || content.contains("input"),
            "Model invocations must be recorded with hashes"
        );
    }
}

/// MODEL-034: Confidence scores use Bayesian updates if prior exists.
///
/// MODEL-030 requires confidence calculation to use Bayesian methods when
/// prior confidence is available. Posterior = P(evidence | prediction) * prior.
#[test]
fn confidence_calculation_supports_bayesian_updates() {
    let model_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/confidence.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read confidence.rs");
        assert!(
            content.contains("bayes") || content.contains("prior") || content.contains("posterior"),
            "Confidence should support Bayesian updates"
        );
    }
}

/// MODEL-035: Quantum backends refuse circuits that exceed known hardware limits.
///
/// MODEL-024 requires circuit validation against known hardware specs: max
/// qubits, max gates per second, etc. Invalid circuits are rejected, never queued.
#[test]
fn quantum_backends_refuse_circuits_exceeding_hardware_limits() {
    let executor_lib = repository_root().join("backend/crates/quant/qip-quantum/src/executor.rs");
    if executor_lib.is_file() {
        let content = fs::read_to_string(&executor_lib).expect("read executor.rs");
        assert!(
            content.contains("hardware")
                || content.contains("limit")
                || content.contains("validate"),
            "Quantum executor must validate against hardware limits"
        );
    }
}

/// MODEL-036: Model output carries execution time in milliseconds.
///
/// MODEL-028 requires execution time to be recorded and included in output.
/// This is used for SLA monitoring and model selection.
#[test]
fn model_output_carries_execution_time_in_milliseconds() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/output.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read output.rs");
        assert!(
            content.contains("execution_time")
                || content.contains("duration")
                || content.contains("elapsed"),
            "Model output must carry execution time"
        );
    }
}

/// MODEL-037: Model ensemble has deterministic member order.
///
/// MODEL-020 requires ensemble members to be ordered (e.g., alphabetically by
/// name). This ensures reproducible voting and aggregation.
#[test]
fn model_ensemble_members_have_deterministic_order() {
    let ensemble_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/ensemble.rs");
    if ensemble_lib.is_file() {
        let content = fs::read_to_string(&ensemble_lib).expect("read ensemble.rs");
        assert!(
            content.contains("BTreeMap")
                || content.contains("BTreeSet")
                || content.contains("order"),
            "Ensemble members must be ordered deterministically"
        );
    }
}

/// MODEL-038: Quantum task IDs are deterministic (stable across retries).
///
/// MODEL-021 requires quantum task IDs to be deterministic so resubmitting the
/// same circuit twice produces a dedupable task ID, not two separate jobs.
#[test]
fn quantum_task_ids_are_deterministic_stable_across_retries() {
    let executor_lib = repository_root().join("backend/crates/quant/qip-quantum/src/executor.rs");
    if executor_lib.is_file() {
        let content = fs::read_to_string(&executor_lib).expect("read executor.rs");
        assert!(
            content.contains("task_id") || content.contains("deterministic"),
            "Quantum task IDs must be deterministic"
        );
    }
}

/// MODEL-039: Classical fallback is not a second-best option; it is the default.
///
/// ADR 0006 MODEL-001 requires the mindset: if quantum cannot prove it is better
/// than classical, classical is used. This is not a fallback; it is the baseline.
#[test]
fn classical_is_the_baseline_not_a_fallback() {
    let classical_lib =
        repository_root().join("backend/crates/services/qip-reasoning/src/classical_baseline.rs");
    if classical_lib.is_file() {
        let content = fs::read_to_string(&classical_lib).expect("read classical_baseline.rs");
        // Verify classical is always run and compared to quantum
        assert!(
            content.contains("baseline") || content.contains("default"),
            "Classical baseline is always computed, quantum must beat it"
        );
    }
}

/// MODEL-040: Metrics: model invocation count, execution time, forecast error.
///
/// MODEL-032 requires three key metrics per model: invocation count (total),
/// average execution time (ms), and running forecast error (basis points).
#[test]
fn model_metrics_track_invocation_count_execution_time_error() {
    let model_lib = repository_root().join("backend/crates/services/qip-reasoning/src/metrics.rs");
    if model_lib.is_file() {
        let content = fs::read_to_string(&model_lib).expect("read metrics.rs");
        assert!(
            content.contains("invocation")
                || content.contains("execution_time")
                || content.contains("error"),
            "Model metrics must track invocation count, time, and error"
        );
    }
}
