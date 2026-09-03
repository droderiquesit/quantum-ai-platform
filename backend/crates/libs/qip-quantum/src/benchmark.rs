//! Running three solvers on one problem, and refusing to believe any of them
//! without a check.
//!
//! Two rules hold here, and both are structural rather than advisory.
//!
//! **A classical baseline is always computed.** [`BenchmarkReport`] carries
//! its baseline in a non-optional field, so a report *without* one has no
//! representation; [`SolverBenchmark::run`] computes the baseline first and
//! returns an error if it cannot, before any other solver is asked for
//! anything. There is no path through this module that produces a quantum
//! number with nothing beside it.
//!
//! **A quantum solution is classically validated before it can be used.** A
//! solver returns a [`crate::solver::SolverCandidate`], which carries a
//! *claimed* objective. The only way to obtain a [`ValidatedSolution`] — the
//! only type this module will report as usable — is
//! [`ClassicalValidator::validate`], which throws the claim away, re-evaluates
//! the assignment with the classical evaluator, and refuses if the two
//! disagree. `ValidatedSolution` has private fields, one constructor and
//! deliberately no `Deserialize`: it cannot be built by hand and it cannot be
//! read back in from JSON, so a validation record cannot be forged by writing
//! one out.
//!
//! The comparison itself is on four axes: solution quality against the
//! baseline, modelled runtime, exact money, and reliability measured by
//! repeating each solver under different streams. Quality is the only one that
//! decides anything; the other three are what a decision to keep paying for a
//! device would rest on.

use crate::solver::{ClassicalSolver, QuboSolver, SolverCandidate, SolverEffort, SolverKind};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use qip_numerics::anneal::Qubo;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The record of one classical re-evaluation of somebody else's answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClassicalValidation {
    /// What the classical evaluator computed from the assignment itself.
    pub recomputed_objective: f64,
    /// What the solver claimed the assignment was worth.
    pub claimed_objective: f64,
    /// The absolute gap between the two.
    pub discrepancy: f64,
    /// The gap that was allowed. Finite arithmetic reorders sums, so an exact
    /// match is not the right bar; a claim that misses by more than this was
    /// not arithmetic.
    pub tolerance: f64,
    /// What did the re-evaluation, recorded so the check is attributable.
    pub evaluator: String,
    pub variables: usize,
}

impl ClassicalValidation {
    pub fn summarise(&self) -> String {
        format!(
            "{} re-evaluated the assignment at {:.6}; the solver claimed {:.6}, a gap of {:.3e} \
             against a {:.3e} tolerance",
            self.evaluator,
            self.recomputed_objective,
            self.claimed_objective,
            self.discrepancy,
            self.tolerance
        )
    }
}

/// A solution that has been re-evaluated classically and may therefore be used.
///
/// Private fields, one constructor, and no `Deserialize` — see the module
/// note. The objective it reports is the *recomputed* one, never the claim,
/// so even a validated solution cannot carry a solver's number forward.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ValidatedSolution {
    solver: String,
    kind: SolverKind,
    assignment: Vec<u8>,
    objective: f64,
    validation: ClassicalValidation,
}

impl ValidatedSolution {
    pub fn solver(&self) -> &str {
        &self.solver
    }

    pub fn kind(&self) -> SolverKind {
        self.kind
    }

    pub fn assignment(&self) -> &[u8] {
        &self.assignment
    }

    /// The objective the classical evaluator computed. Not the claim.
    pub fn objective(&self) -> f64 {
        self.objective
    }

    pub fn validation(&self) -> &ClassicalValidation {
        &self.validation
    }
}

/// Re-evaluates somebody else's answer before anyone acts on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClassicalValidator {
    tolerance: f64,
}

impl Default for ClassicalValidator {
    fn default() -> Self {
        // Loose enough that summation order does not fail an honest solver,
        // tight enough that a fabricated objective cannot hide inside it.
        Self { tolerance: 1e-6 }
    }
}

impl ClassicalValidator {
    /// Build a validator with an explicit tolerance.
    ///
    /// Refuses rather than repairs a bad tolerance, because the value gates
    /// the one check this module exists to enforce. A negative tolerance was
    /// once silently made positive with `.abs()`; that hid the caller's bug
    /// instead of naming it. Worse, a non-finite tolerance was not caught at
    /// all: `discrepancy > f64::NAN` is `false` for every `discrepancy`, so a
    /// validator built with a NaN tolerance would validate *any* claim,
    /// however large the gap between it and the classical recomputation —
    /// exactly the unchecked quantum number ADR 0006 forbids.
    pub fn new(tolerance: f64) -> Result<Self> {
        if !tolerance.is_finite() || tolerance < 0.0 {
            return Err(Error::invalid(format!(
                "a classical validation tolerance must be finite and non-negative; {tolerance} \
                 would either fail to bound the check or admit every claim regardless of how far \
                 it is from the classical recomputation"
            )));
        }
        Ok(Self { tolerance })
    }

    pub fn tolerance(&self) -> f64 {
        self.tolerance
    }

    /// Check a candidate against the problem it claims to have solved.
    ///
    /// Four things are checked, and any one of them refuses the answer
    /// whatever it claimed to be worth: the assignment is the right length,
    /// every entry is a bit, the recomputed objective is finite, and the
    /// solver's claim matches the recomputation.
    pub fn validate(&self, qubo: &Qubo, candidate: &SolverCandidate) -> Result<ValidatedSolution> {
        if candidate.assignment.len() != qubo.n {
            return Err(Error::guard(format!(
                "{} returned {} variables for a {}-variable problem, so its answer is not a \
                 solution to the problem that was posed",
                candidate.solver,
                candidate.assignment.len(),
                qubo.n
            )));
        }
        if let Some(position) = candidate.assignment.iter().position(|bit| *bit > 1) {
            return Err(Error::guard(format!(
                "{} returned {} at position {position}, which is not a bit",
                candidate.solver, candidate.assignment[position]
            )));
        }

        // The claim is discarded here. Everything downstream reads the number
        // this line produces.
        let recomputed = qubo.evaluate(&candidate.assignment);
        if !recomputed.is_finite() {
            return Err(Error::guard(format!(
                "the classical evaluator scored {}'s answer at {recomputed}, which is not a number",
                candidate.solver
            )));
        }

        let discrepancy = (recomputed - candidate.claimed_objective).abs();
        let validation = ClassicalValidation {
            recomputed_objective: recomputed,
            claimed_objective: candidate.claimed_objective,
            discrepancy,
            tolerance: self.tolerance,
            evaluator: "qubo-objective".to_string(),
            variables: qubo.n,
        };
        // Written as two explicit conditions rather than a negated `<=`: a
        // claim of NaN produces a NaN discrepancy, and every comparison
        // against NaN is false, so a negated comparison would quietly admit
        // exactly the claim that has no value at all.
        if !discrepancy.is_finite() || discrepancy > self.tolerance {
            return Err(Error::guard(format!(
                "{} claimed {:.6} but its assignment is worth {:.6}: {}. The answer is refused \
                 however good the claim was",
                candidate.solver,
                candidate.claimed_objective,
                recomputed,
                validation.summarise()
            )));
        }

        Ok(ValidatedSolution {
            solver: candidate.solver.clone(),
            kind: candidate.kind,
            assignment: candidate.assignment.clone(),
            objective: recomputed,
            validation,
        })
    }
}

/// How a solver's answer compared to the baseline.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct QualityMeasure {
    /// The validated objective. Minimised, so lower is better.
    pub objective: f64,
    /// The baseline's validated objective.
    pub baseline_objective: f64,
    /// Fractional improvement over the baseline. Negative means worse.
    pub improvement_over_baseline: f64,
    /// Ratio to a proven optimum, where the baseline was exact. `None`
    /// otherwise, rather than a ratio against another heuristic's guess
    /// dressed up as an optimum.
    pub approximation_ratio: Option<f64>,
}

/// How often a solver did the job when asked repeatedly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reliability {
    /// Runs attempted, each on its own stream.
    pub attempts: usize,
    /// Runs that returned an answer at all.
    pub produced: usize,
    /// Runs whose answer survived classical validation.
    pub validated: usize,
    /// Runs that reached the best objective any solver reached.
    pub reached_best: usize,
}

impl Reliability {
    /// Fraction of attempts that produced a validated answer as good as the
    /// best anyone found. The number that matters: a solver that is right one
    /// time in five is not a solver you can put a problem to once.
    pub fn success_rate(&self) -> f64 {
        if self.attempts == 0 {
            return 0.0;
        }
        self.reached_best as f64 / self.attempts as f64
    }

    /// Fraction of produced answers that survived validation.
    pub fn validation_rate(&self) -> f64 {
        if self.produced == 0 {
            return 0.0;
        }
        self.validated as f64 / self.produced as f64
    }
}

/// Everything one solver did on one problem.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SolverRecord {
    pub solver: String,
    pub kind: SolverKind,
    pub available: bool,
    /// What the deployment is missing, when it is missing something.
    pub requirement: String,
    /// The best validated answer, if any survived validation. `None` is the
    /// only representation of an unusable result — there is no field holding
    /// an unvalidated one that a caller could read by mistake.
    pub solution: Option<ValidatedSolution>,
    /// Why there is no usable answer, when there is not.
    pub refusal: Option<String>,
    pub quality: Option<QualityMeasure>,
    /// Modelled, not measured. See [`crate::solver`].
    pub runtime: Duration,
    /// Exact. Money is never an `f64`.
    pub price: Decimal,
    pub reliability: Reliability,
    /// The search that produced the best run, so two entrants can be shown to
    /// be different searches rather than one search twice.
    pub trace: Option<crate::solver::SearchTrace>,
}

impl SolverRecord {
    /// The answer, if it may be used.
    ///
    /// The only accessor. A caller cannot reach an unvalidated assignment
    /// through this type, because none is kept.
    pub fn usable_solution(&self) -> Option<&ValidatedSolution> {
        self.solution.as_ref()
    }

    pub fn objective(&self) -> Option<f64> {
        self.solution.as_ref().map(ValidatedSolution::objective)
    }
}

/// The comparison.
///
/// `classical_baseline` is not an `Option`. That is the enforcement of the
/// first rule: this value cannot exist without a baseline, so no code path can
/// report a quantum result and omit one.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BenchmarkReport {
    pub variables: usize,
    /// Always present, by type.
    pub classical_baseline: SolverRecord,
    /// Whether the baseline is a proven optimum rather than a good answer.
    pub baseline_is_exact: bool,
    /// Every other entrant, in the order they were registered.
    pub entrants: Vec<SolverRecord>,
    /// The solver whose validated answer is best. Always names a solver,
    /// because the baseline is always validated or the run failed.
    pub chosen: String,
    /// Improvement of the chosen answer over the baseline, as a fraction.
    pub improvement_over_baseline: f64,
    /// Notes an operator should read: what was unavailable, what was refused.
    pub notes: Vec<String>,
}

impl BenchmarkReport {
    /// Every record, baseline first.
    pub fn records(&self) -> impl Iterator<Item = &SolverRecord> {
        std::iter::once(&self.classical_baseline).chain(self.entrants.iter())
    }

    pub fn record_for(&self, solver: &str) -> Option<&SolverRecord> {
        self.records().find(|record| record.solver == solver)
    }

    /// The baseline's objective. Always a number.
    pub fn baseline_objective(&self) -> Option<f64> {
        self.classical_baseline.objective()
    }

    /// The best validated answer across every entrant, baseline included.
    pub fn best_usable(&self) -> Option<&ValidatedSolution> {
        self.records()
            .filter_map(SolverRecord::usable_solution)
            .min_by(|a, b| {
                a.objective()
                    .partial_cmp(&b.objective())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Whether a non-classical solver beat the baseline by more than `margin`.
    ///
    /// The only statement this crate will make about a non-classical method,
    /// and it is per-problem. A method that helped on one instance has not
    /// been shown to help in general, and the phrasing of
    /// [`Self::claim`] says so.
    pub fn measured_advantage(&self, margin: f64) -> Option<(&SolverRecord, f64)> {
        let baseline = self.baseline_objective()?;
        self.entrants
            .iter()
            .filter(|record| record.kind.needs_a_classical_baseline())
            .filter_map(|record| {
                let objective = record.objective()?;
                let improvement = if baseline.abs() < 1e-12 {
                    0.0
                } else {
                    (baseline - objective) / baseline.abs()
                };
                (improvement > margin).then_some((record, improvement))
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// A sentence that always quotes the baseline.
    ///
    /// Written as a method rather than left to callers because the failure
    /// mode this whole module exists to prevent is a quantum number quoted on
    /// its own.
    pub fn claim(&self) -> String {
        let baseline = self
            .baseline_objective()
            .map_or_else(|| "no validated value".to_string(), |v| format!("{v:.6}"));
        match self.measured_advantage(0.0) {
            Some((record, improvement)) => format!(
                "{} reached {:.6} against the classical baseline's {baseline}, an improvement of \
                 {:.2}% on this instance and on no other; it is a {} solver and the improvement is \
                 evidence about this problem, not about the method",
                record.solver,
                record.objective().unwrap_or(f64::NAN),
                improvement * 100.0,
                record.kind.as_str()
            ),
            None => format!(
                "the classical baseline's {baseline} was not beaten; {} answer(s) were compared \
                 against it",
                self.entrants.len()
            ),
        }
    }
}

/// Runs every solver on one problem and compares them.
#[derive(Debug)]
pub struct SolverBenchmark {
    /// The baseline. A concrete type, not a trait object and not an option:
    /// the benchmark cannot be constructed without one.
    classical: ClassicalSolver,
    entrants: Vec<Arc<dyn QuboSolver>>,
    effort: SolverEffort,
    validator: ClassicalValidator,
    /// Runs per solver. Reliability is not observable from one run.
    repeats: usize,
}

impl SolverBenchmark {
    /// A benchmark with only the classical baseline in it.
    pub fn new(classical: ClassicalSolver) -> Self {
        Self {
            classical,
            entrants: Vec::new(),
            effort: SolverEffort::default(),
            validator: ClassicalValidator::default(),
            repeats: 3,
        }
    }

    /// Add an entrant. The baseline is not one of these and cannot be
    /// displaced by one.
    pub fn with_solver(mut self, solver: Arc<dyn QuboSolver>) -> Self {
        self.entrants.push(solver);
        self
    }

    pub fn with_effort(mut self, effort: SolverEffort) -> Self {
        self.effort = effort;
        self
    }

    pub fn with_validator(mut self, validator: ClassicalValidator) -> Self {
        self.validator = validator;
        self
    }

    pub fn with_repeats(mut self, repeats: usize) -> Self {
        self.repeats = repeats.max(1);
        self
    }

    pub fn effort(&self) -> SolverEffort {
        self.effort
    }

    pub fn classical(&self) -> &ClassicalSolver {
        &self.classical
    }

    /// Run everything on the same problem.
    ///
    /// The baseline is computed first and its failure is the run's failure.
    /// That ordering is the point: there is no state of this function in which
    /// a quantum solver has produced a number and the baseline has not been
    /// attempted.
    pub fn run(&self, qubo: &Qubo) -> Result<BenchmarkReport> {
        if qubo.n == 0 {
            return Err(Error::invalid(
                "a QUBO with no variables has nothing to benchmark",
            ));
        }

        let mut notes = Vec::new();
        let mut baseline = self.record(&self.classical, qubo, &mut notes);
        let baseline_objective = baseline.objective().ok_or_else(|| {
            Error::numeric(format!(
                "the classical baseline produced no validated answer ({}), so there is nothing to \
                 measure any other solver against and no result may be reported",
                baseline
                    .refusal
                    .clone()
                    .unwrap_or_else(|| "no reason recorded".to_string())
            ))
        })?;

        let mut entrants: Vec<SolverRecord> = self
            .entrants
            .iter()
            .map(|solver| self.record(solver.as_ref(), qubo, &mut notes))
            .collect();

        let baseline_is_exact = self.classical.is_exact_for(qubo.n);
        // The best objective anyone reached, used both for the choice and for
        // each solver's reliability.
        let best_objective = std::iter::once(baseline_objective)
            .chain(entrants.iter().filter_map(SolverRecord::objective))
            .fold(baseline_objective, f64::min);

        let optimum = baseline_is_exact.then_some(baseline_objective);
        for record in std::iter::once(&mut baseline).chain(entrants.iter_mut()) {
            record.quality = record.objective().map(|objective| QualityMeasure {
                objective,
                baseline_objective,
                improvement_over_baseline: if baseline_objective.abs() < 1e-12 {
                    0.0
                } else {
                    (baseline_objective - objective) / baseline_objective.abs()
                },
                approximation_ratio: optimum
                    .and_then(|optimum| (optimum.abs() > 1e-12).then_some(objective / optimum)),
            });
        }

        let chosen = std::iter::once(&baseline)
            .chain(entrants.iter())
            .filter(|record| record.solution.is_some())
            .min_by(|a, b| {
                a.objective()
                    .unwrap_or(f64::INFINITY)
                    .partial_cmp(&b.objective().unwrap_or(f64::INFINITY))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or_else(|| baseline.solver.clone(), |record| record.solver.clone());

        let improvement = if baseline_objective.abs() < 1e-12 {
            0.0
        } else {
            (baseline_objective - best_objective) / baseline_objective.abs()
        };

        Ok(BenchmarkReport {
            variables: qubo.n,
            classical_baseline: baseline,
            baseline_is_exact,
            entrants,
            chosen,
            improvement_over_baseline: improvement,
            notes,
        })
    }

    /// Run one solver `repeats` times and keep the best validated answer.
    fn record(
        &self,
        solver: &dyn QuboSolver,
        qubo: &Qubo,
        notes: &mut Vec<String>,
    ) -> SolverRecord {
        let cost = solver.cost_model();
        if !solver.is_available() {
            let requirement = solver.requirement();
            notes.push(format!("{} did not run: {requirement}", solver.name()));
            return unavailable_record(solver, requirement);
        }

        let mut best: Option<(ValidatedSolution, SolverCandidate)> = None;
        let mut reliability = Reliability {
            attempts: self.repeats,
            produced: 0,
            validated: 0,
            reached_best: 0,
        };
        let mut objectives: Vec<f64> = Vec::new();
        let mut refusal: Option<String> = None;

        for repeat in 0..self.repeats {
            // Each repeat gets its own stream, derived from the benchmark's
            // seed rather than drawn from anywhere ambient.
            let effort = self.effort.with_seed(
                self.effort
                    .seed
                    .wrapping_add(repeat as u64)
                    .wrapping_mul(0x9E37_79B9),
            );
            let candidate = match solver.solve(qubo, &effort) {
                Ok(candidate) => candidate,
                Err(error) => {
                    refusal.get_or_insert_with(|| error.message().to_string());
                    notes.push(format!("{} failed: {}", solver.name(), error.message()));
                    continue;
                }
            };
            reliability.produced += 1;

            match self.validator.validate(qubo, &candidate) {
                Ok(validated) => {
                    reliability.validated += 1;
                    objectives.push(validated.objective());
                    let better = best
                        .as_ref()
                        .is_none_or(|(current, _)| validated.objective() < current.objective());
                    if better {
                        best = Some((validated, candidate));
                    }
                }
                Err(error) => {
                    // A refused answer is not kept anywhere. It is named in
                    // the notes and discarded.
                    refusal.get_or_insert_with(|| error.message().to_string());
                    notes.push(format!(
                        "{}'s answer was refused by classical validation: {}",
                        solver.name(),
                        error.message()
                    ));
                }
            }
        }

        if let Some(best_objective) = objectives.iter().copied().reduce(f64::min) {
            reliability.reached_best = objectives
                .iter()
                .filter(|objective| **objective <= best_objective + 1e-9)
                .count();
        }

        let (solution, trace, evaluations) = match best {
            Some((validated, candidate)) => (
                Some(validated),
                Some(candidate.trace),
                candidate.evaluations,
            ),
            None => (None, None, 0),
        };

        SolverRecord {
            solver: solver.name().to_string(),
            kind: solver.kind(),
            available: true,
            requirement: String::new(),
            refusal: if solution.is_none() {
                refusal.or_else(|| Some("the solver produced no validated answer".to_string()))
            } else {
                None
            },
            solution,
            quality: None,
            runtime: cost.runtime(evaluations),
            price: cost.price(),
            reliability,
            trace,
        }
    }
}

/// The record for a solver this deployment cannot run.
fn unavailable_record(solver: &dyn QuboSolver, requirement: String) -> SolverRecord {
    SolverRecord {
        solver: solver.name().to_string(),
        kind: solver.kind(),
        available: false,
        refusal: Some(requirement.clone()),
        requirement,
        solution: None,
        quality: None,
        // Nothing ran, so nothing was spent. Reporting the queue delay of a
        // job that was never submitted would put a number in the runtime
        // column that describes nothing.
        runtime: Duration::ZERO,
        price: Decimal::ZERO,
        reliability: Reliability {
            attempts: 0,
            produced: 0,
            validated: 0,
            reached_best: 0,
        },
        trace: None,
    }
}
