//! Three solvers, one abstraction, and an honest account of what each is.
//!
//! [`QuboSolver`] is the shape every entrant in [`crate::benchmark`] takes:
//! a classical search, a quantum-inspired search, and the port to a real
//! quantum device. Putting them behind one trait is what makes the comparison
//! possible at all — the same problem, the same scoring function, the same
//! record of what each cost.
//!
//! The three are genuinely three things, not one thing relabelled:
//!
//! * [`ClassicalSolver`] is exhaustive enumeration, Metropolis annealing from
//!   [`qip_numerics::anneal`], or steepest-descent local search. Nothing here
//!   is new; it is the platform's existing classical machinery given a common
//!   interface.
//! * [`QuantumInspiredSolver`] is discrete-time path-integral quantum
//!   annealing: `P` Trotter replicas of the same problem, coupled along the
//!   imaginary-time direction with a strength derived from a transverse field
//!   that is lowered over the run. It is a *classical* Monte Carlo algorithm
//!   whose dynamics are borrowed from a transverse-field Ising model, and it
//!   is not evidence of anything quantum. What it is, is a different search:
//!   its moves are accepted against a cross-replica coupling that Metropolis
//!   annealing has no term for, and at strong coupling the ensemble crosses a
//!   barrier as one object rather than one spin at a time.
//! * [`IbmQuantumSolver`] is the port: it reports itself unavailable and names
//!   exactly what a deployment is missing, because *this type* holds no
//!   transport and no credential. A deployment that has both reaches the device
//!   through [`crate::provider::HostedProvider::connected`] wrapped in a
//!   [`ProviderSolver`], which enters the same benchmark under the same
//!   [`SolverKind::Quantum`] and therefore under the same requirement for a
//!   classical baseline. This type stays as the description of what such a
//!   deployment has to supply.
//!
//! **Runtime here is modelled, not measured.** The platform forbids reaching
//! for an ambient clock, and a benchmark that called one would produce a
//! different answer on every machine and every run. Each solver instead
//! declares a [`SolverCostModel`] — nanoseconds per objective evaluation, a
//! queue delay, a price per job — and the reported runtime is that model
//! applied to the work the solver actually did. It is comparable and
//! reproducible; it is not a stopwatch, and nothing here pretends it is.

use crate::provider::{ProviderCapabilities, QuantumProvider};
use crate::qaoa::QaoaSettings;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::Duration;
use qip_numerics::anneal::{AnnealSettings, Qubo, anneal, solve_exact};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// What kind of machine produced an answer.
///
/// Recorded on every result so a quantum-inspired heuristic can never be
/// reported as a quantum one. The two words differ by a hyphen and by
/// everything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolverKind {
    /// Runs on the same CPU as everything else and claims nothing.
    Classical,
    /// A classical algorithm whose dynamics are borrowed from quantum
    /// mechanics. Still classical.
    QuantumInspired,
    /// A quantum device, or a simulation of one.
    Quantum,
}

impl SolverKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Classical => "classical",
            Self::QuantumInspired => "quantum_inspired",
            Self::Quantum => "quantum",
        }
    }

    /// Whether a result from this kind needs a classical baseline beside it.
    ///
    /// True for everything that is not itself classical, including the
    /// quantum-inspired search: a heuristic borrowed from physics has exactly
    /// as much claim on being believed without a baseline as a device does.
    pub const fn needs_a_classical_baseline(&self) -> bool {
        !matches!(self, Self::Classical)
    }

    pub const fn is_quantum(&self) -> bool {
        matches!(self, Self::Quantum)
    }
}

/// How much work a solver may do.
///
/// The seed lives here rather than only on the solver so the same solver can
/// be run repeatedly under different streams — which is how
/// [`crate::benchmark`] measures reliability — without any of the runs
/// depending on an ambient source of randomness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolverEffort {
    /// Sweeps over the variables. One sweep proposes one move per variable.
    pub sweeps: usize,
    /// Independent restarts, for the searches that restart.
    pub restarts: usize,
    /// The stream this run draws from.
    pub seed: u64,
}

impl Default for SolverEffort {
    fn default() -> Self {
        Self {
            sweeps: 400,
            restarts: 4,
            seed: 0x5EED_0000_0000_0001,
        }
    }
}

impl SolverEffort {
    pub const fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }
}

/// What a solver's work costs, in time and in money.
///
/// Declared rather than measured. See the module note: an ambient clock would
/// make this benchmark irreproducible, which would defeat the point of running
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolverCostModel {
    /// Modelled cost of one objective evaluation.
    pub nanos_per_evaluation: i64,
    /// Delay before the work starts at all — zero in process, substantial on a
    /// shared device.
    pub queue: Duration,
    /// Price of one job in USD micro-units. Converted to an exact [`Decimal`]
    /// rather than kept as a float, because it is money.
    pub price_micros_per_job: u64,
}

impl SolverCostModel {
    /// An in-process solver: no queue, no invoice, a few nanoseconds a step.
    pub const fn in_process(nanos_per_evaluation: i64) -> Self {
        Self {
            nanos_per_evaluation,
            queue: Duration::ZERO,
            price_micros_per_job: 0,
        }
    }

    /// Modelled wall time for a run that did `evaluations` units of work.
    pub fn runtime(&self, evaluations: usize) -> Duration {
        let work = i64::try_from(evaluations).unwrap_or(i64::MAX);
        Duration::from_nanos(
            self.queue
                .as_nanos()
                .saturating_add(work.saturating_mul(self.nanos_per_evaluation)),
        )
    }

    /// Price of one job, exact.
    pub fn price(&self) -> Decimal {
        Decimal::from_scaled(i128::from(self.price_micros_per_job), 6).unwrap_or(Decimal::ZERO)
    }
}

/// What a search actually did, in enough detail to tell two searches apart.
///
/// The `family` string is the honest part. Two solvers reporting the same
/// family are the same algorithm with different parameters, whatever their
/// type names say, and the benchmark's distinctness check reads this field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchTrace {
    /// The search procedure's name, e.g. `path-integral-quantum-annealing`.
    pub family: String,
    pub moves_proposed: usize,
    pub moves_accepted: usize,
    /// Accepted moves that made the objective worse. Zero for a pure descent,
    /// and the whole reason the other searches escape a local optimum.
    pub uphill_accepted: usize,
    pub restarts: usize,
    /// Coupled copies of the problem searched together. One for every
    /// single-configuration search; more only for the path-integral solver.
    pub replicas: usize,
    /// Whether the returned assignment is a local optimum under single-bit
    /// flips. A search that stops elsewhere stopped because it ran out of
    /// budget.
    pub local_optimum: bool,
}

impl SearchTrace {
    fn new(family: &str) -> Self {
        Self {
            family: family.to_string(),
            moves_proposed: 0,
            moves_accepted: 0,
            uphill_accepted: 0,
            restarts: 0,
            replicas: 1,
            local_optimum: false,
        }
    }

    /// Whether two runs came from genuinely different searches.
    ///
    /// Deliberately strict about the family name: a solver that renamed
    /// itself but kept the same move generator is not a second solver, and
    /// this is the check that says so.
    pub fn is_distinct_from(&self, other: &Self) -> bool {
        self.family != other.family
    }
}

/// One solver's answer, before anyone has checked it.
///
/// `claimed_objective` is what the solver says its assignment is worth. It is
/// named a claim because that is what it is: until
/// [`crate::benchmark::ClassicalValidator`] re-evaluates the assignment, no
/// part of the platform may act on it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolverCandidate {
    pub solver: String,
    pub kind: SolverKind,
    pub assignment: Vec<u8>,
    /// The objective value the solver reports for its own answer.
    pub claimed_objective: f64,
    /// Objective evaluations spent. The unit the cost model is applied to.
    pub evaluations: usize,
    pub trace: SearchTrace,
}

/// A solver of binary quadratic problems.
pub trait QuboSolver: fmt::Debug + Send + Sync {
    fn name(&self) -> &str;

    fn kind(&self) -> SolverKind;

    /// Whether this deployment can actually run it.
    fn is_available(&self) -> bool;

    /// What an operator would have to provide to make it usable. Empty when it
    /// already is.
    fn requirement(&self) -> String {
        String::new()
    }

    fn cost_model(&self) -> SolverCostModel;

    /// Solve, or explain why not.
    fn solve(&self, qubo: &Qubo, effort: &SolverEffort) -> Result<SolverCandidate>;
}

/// The typical magnitude of the objective's coefficients.
///
/// Temperatures and fields are meaningless in the abstract: a schedule tuned
/// for coefficients around one does nothing on a problem whose coefficients
/// are around a thousand. Every schedule below is expressed as a multiple of
/// this.
fn energy_scale(qubo: &Qubo) -> f64 {
    if qubo.n == 0 || qubo.entries.is_empty() {
        return 1.0;
    }
    let total: f64 = qubo.entries.iter().map(|(_, _, w)| w.abs()).sum();
    (total / qubo.n as f64).max(1e-12)
}

/// Steepest-descent local search: take the best improving single flip until
/// none exists.
///
/// Returns the local optimum, its objective, and the evaluations spent. This
/// is the honest floor every other search is built on, and the thing a
/// deceptive landscape traps.
fn descend(qubo: &Qubo, mut x: Vec<u8>, evaluations: &mut usize) -> (Vec<u8>, f64) {
    let mut energy = qubo.evaluate(&x);
    *evaluations += 1;
    loop {
        let mut chosen = None;
        // Strictly negative, so a flat move is not taken: cycling between two
        // equal configurations would never terminate.
        let mut best_delta = -1e-12;
        for k in 0..qubo.n {
            let delta = qubo.delta(&x, k);
            *evaluations += 1;
            if delta < best_delta {
                best_delta = delta;
                chosen = Some(k);
            }
        }
        match chosen {
            Some(k) => {
                x[k] ^= 1;
                energy += best_delta;
            }
            None => break,
        }
    }
    (x, energy)
}

/// Whether an assignment is a local optimum under single-bit flips.
///
/// Used to fill in [`SearchTrace::local_optimum`], and worth having as a
/// public check: it is the precise statement of "this search is trapped here".
pub fn is_local_optimum(qubo: &Qubo, assignment: &[u8]) -> bool {
    (0..qubo.n).all(|k| qubo.delta(assignment, k) >= -1e-12)
}

/// How the classical baseline is computed.
#[derive(Clone, Debug)]
pub enum ClassicalSearch {
    /// Every assignment. Exact, and only viable below `limit` variables.
    Exhaustive { limit: usize },
    /// Metropolis simulated annealing over single-bit flips.
    Annealing(AnnealSettings),
    /// Steepest descent from random starts. The cheapest classical search,
    /// and the one a latency-bound path would actually run.
    Descent { restarts: usize },
    /// Exact below `exact_limit` variables, annealing above it. The default,
    /// because an exact baseline is worth far more than a heuristic one and is
    /// affordable exactly while the problem is small.
    Automatic {
        exact_limit: usize,
        settings: AnnealSettings,
    },
}

/// The classical baseline.
///
/// Every benchmark computes one of these, always, whatever else it runs.
#[derive(Clone, Debug)]
pub struct ClassicalSolver {
    search: ClassicalSearch,
    seed: u64,
    cost: SolverCostModel,
}

impl ClassicalSolver {
    /// The default: exact while the problem is small, annealing above that.
    pub fn new(seed: u64) -> Self {
        Self {
            search: ClassicalSearch::Automatic {
                exact_limit: 20,
                settings: AnnealSettings::default(),
            },
            seed,
            cost: SolverCostModel::in_process(40),
        }
    }

    pub fn annealing(seed: u64, settings: AnnealSettings) -> Self {
        Self {
            search: ClassicalSearch::Annealing(settings),
            seed,
            cost: SolverCostModel::in_process(40),
        }
    }

    /// Steepest descent, restarted. Trapped by construction on a landscape
    /// with a strict local optimum, which is what makes it the right baseline
    /// for showing that another search escapes one.
    pub fn descent(seed: u64, restarts: usize) -> Self {
        Self {
            search: ClassicalSearch::Descent {
                restarts: restarts.max(1),
            },
            seed,
            cost: SolverCostModel::in_process(40),
        }
    }

    pub fn exhaustive(limit: usize) -> Self {
        Self {
            search: ClassicalSearch::Exhaustive { limit },
            seed: 0,
            cost: SolverCostModel::in_process(40),
        }
    }

    pub fn with_cost(mut self, cost: SolverCostModel) -> Self {
        self.cost = cost;
        self
    }

    pub fn search(&self) -> &ClassicalSearch {
        &self.search
    }

    /// Whether this configuration returns a provably optimal answer for a
    /// problem of this size.
    pub fn is_exact_for(&self, variables: usize) -> bool {
        match &self.search {
            ClassicalSearch::Exhaustive { limit } => variables <= *limit,
            ClassicalSearch::Automatic { exact_limit, .. } => variables <= *exact_limit,
            ClassicalSearch::Annealing(_) | ClassicalSearch::Descent { .. } => false,
        }
    }

    fn run_descent(&self, qubo: &Qubo, effort: &SolverEffort, restarts: usize) -> SolverCandidate {
        let mut rng = Xoshiro256::seeded(self.seed)
            .fork(&format!("descent-{}-{}-{restarts}", qubo.n, effort.seed));
        let mut evaluations = 0usize;
        let mut best = vec![0u8; qubo.n];
        let mut best_energy = f64::INFINITY;
        let mut trace = SearchTrace::new("steepest-descent");
        trace.restarts = restarts;
        for _ in 0..restarts {
            let start: Vec<u8> = (0..qubo.n).map(|_| u8::from(rng.bernoulli(0.5))).collect();
            let (x, energy) = descend(qubo, start, &mut evaluations);
            if energy < best_energy {
                best_energy = energy;
                best = x;
            }
        }
        trace.moves_proposed = evaluations;
        trace.local_optimum = is_local_optimum(qubo, &best);
        SolverCandidate {
            solver: self.name().to_string(),
            kind: SolverKind::Classical,
            claimed_objective: best_energy,
            assignment: best,
            evaluations,
            trace,
        }
    }

    fn run_anneal(
        &self,
        qubo: &Qubo,
        effort: &SolverEffort,
        settings: &AnnealSettings,
    ) -> SolverCandidate {
        let mut rng =
            Xoshiro256::seeded(self.seed).fork(&format!("anneal-{}-{}", qubo.n, effort.seed));
        let result = anneal(qubo, settings, &mut rng);
        // Annealing proposes one move per variable per sweep per restart, and
        // each move is one incremental evaluation.
        let evaluations = result
            .sweeps_run
            .saturating_mul(qubo.n)
            .saturating_mul(result.restarts_run);
        let mut trace = SearchTrace::new("metropolis-annealing");
        trace.restarts = result.restarts_run;
        trace.moves_proposed = evaluations;
        trace.local_optimum = is_local_optimum(qubo, &result.assignment);
        SolverCandidate {
            solver: self.name().to_string(),
            kind: SolverKind::Classical,
            claimed_objective: result.energy,
            assignment: result.assignment,
            evaluations,
            trace,
        }
    }

    fn run_exhaustive(&self, qubo: &Qubo) -> Result<SolverCandidate> {
        let result = solve_exact(qubo).ok_or_else(|| {
            Error::invalid(format!(
                "{} variables is beyond what exhaustive enumeration will attempt",
                qubo.n
            ))
        })?;
        let mut trace = SearchTrace::new("exhaustive-enumeration");
        trace.restarts = 1;
        trace.moves_proposed = 1usize << qubo.n.min(31);
        trace.local_optimum = true;
        Ok(SolverCandidate {
            solver: self.name().to_string(),
            kind: SolverKind::Classical,
            claimed_objective: result.energy,
            assignment: result.assignment,
            evaluations: 1usize << qubo.n.min(31),
            trace,
        })
    }
}

impl QuboSolver for ClassicalSolver {
    fn name(&self) -> &str {
        match &self.search {
            ClassicalSearch::Exhaustive { .. } => "classical-exhaustive",
            ClassicalSearch::Annealing(_) => "classical-annealing",
            ClassicalSearch::Descent { .. } => "classical-descent",
            ClassicalSearch::Automatic { .. } => "classical-automatic",
        }
    }

    fn kind(&self) -> SolverKind {
        SolverKind::Classical
    }

    fn is_available(&self) -> bool {
        // The classical solver is always available. That is the whole reason
        // it is the baseline: a deployment with no device configured behaves
        // exactly like one whose device is unreachable.
        true
    }

    fn cost_model(&self) -> SolverCostModel {
        self.cost
    }

    fn solve(&self, qubo: &Qubo, effort: &SolverEffort) -> Result<SolverCandidate> {
        if qubo.n == 0 {
            return Err(Error::invalid(
                "a QUBO with no variables has nothing to solve",
            ));
        }
        match &self.search {
            ClassicalSearch::Exhaustive { limit } => {
                if qubo.n > *limit {
                    return Err(Error::invalid(format!(
                        "{} variables exceeds the {limit}-variable enumeration limit",
                        qubo.n
                    )));
                }
                self.run_exhaustive(qubo)
            }
            ClassicalSearch::Annealing(settings) => Ok(self.run_anneal(qubo, effort, settings)),
            ClassicalSearch::Descent { restarts } => Ok(self.run_descent(qubo, effort, *restarts)),
            ClassicalSearch::Automatic {
                exact_limit,
                settings,
            } => {
                if qubo.n <= *exact_limit {
                    self.run_exhaustive(qubo)
                } else {
                    Ok(self.run_anneal(qubo, effort, settings))
                }
            }
        }
    }
}

/// Discrete-time path-integral quantum annealing.
///
/// `P` Trotter replicas of the same problem are searched at once. Each
/// replica's own objective is scaled by `1/P`, and neighbouring replicas are
/// coupled along a ring with a strength
///
/// ```text
/// J⊥(Γ) = -(P·T/2)·ln tanh(Γ / (P·T))
/// ```
///
/// derived from the transverse field `Γ`, which is lowered geometrically over
/// the run. At large `Γ` the coupling vanishes and the replicas explore
/// independently at an effective temperature `P·T` — broad, cheap exploration.
/// At small `Γ` the coupling is large, the replicas lock together, and flipping
/// a variable costs the *whole* objective change at temperature `T` — which is
/// cold. The ensemble therefore stops behaving like `P` independent walkers
/// and starts behaving like one object crossing a barrier as a unit.
///
/// **This is a classical algorithm.** It borrows its dynamics from the
/// transverse-field Ising model and runs entirely on a CPU. Nothing it
/// produces is evidence of quantum advantage, and [`SolverKind::QuantumInspired`]
/// exists so a result from it can never be reported as though it were.
#[derive(Clone, Debug)]
pub struct QuantumInspiredSolver {
    seed: u64,
    replicas: usize,
    /// Transverse field at the start, as a multiple of the problem's energy
    /// scale.
    field_start: f64,
    /// Transverse field at the end. Small, so the replicas finish locked.
    field_end: f64,
    /// Effective temperature, as a multiple of the energy scale.
    temperature: f64,
    cost: SolverCostModel,
}

impl QuantumInspiredSolver {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            // Eight replicas: enough that the imaginary-time direction is a
            // direction rather than a pair, and few enough that a sweep costs
            // eight times a classical one rather than a hundred.
            replicas: 8,
            field_start: 3.0,
            field_end: 0.02,
            temperature: 0.12,
            cost: SolverCostModel::in_process(45),
        }
    }

    /// Set the number of Trotter replicas. Two is the minimum at which the
    /// imaginary-time coupling exists at all.
    pub fn with_replicas(mut self, replicas: usize) -> Self {
        self.replicas = replicas.max(2);
        self
    }

    pub fn with_schedule(mut self, field_start: f64, field_end: f64, temperature: f64) -> Self {
        self.field_start = field_start;
        self.field_end = field_end;
        self.temperature = temperature;
        self
    }

    pub fn with_cost(mut self, cost: SolverCostModel) -> Self {
        self.cost = cost;
        self
    }

    pub fn replicas(&self) -> usize {
        self.replicas
    }
}

/// `+1` for a set bit, `-1` for a clear one.
fn spin(bit: u8) -> f64 {
    if bit == 1 { 1.0 } else { -1.0 }
}

impl QuboSolver for QuantumInspiredSolver {
    fn name(&self) -> &str {
        "quantum-inspired-path-integral"
    }

    fn kind(&self) -> SolverKind {
        SolverKind::QuantumInspired
    }

    fn is_available(&self) -> bool {
        true
    }

    fn cost_model(&self) -> SolverCostModel {
        self.cost
    }

    fn solve(&self, qubo: &Qubo, effort: &SolverEffort) -> Result<SolverCandidate> {
        let n = qubo.n;
        if n == 0 {
            return Err(Error::invalid(
                "a QUBO with no variables has nothing to solve",
            ));
        }
        let replicas = self.replicas.max(2);
        let scale = energy_scale(qubo);
        let temperature = (self.temperature * scale).max(1e-12);
        let field_start = (self.field_start * scale).max(1e-9);
        let field_end = (self.field_end * scale).max(1e-12);
        let sweeps = effort.sweeps.max(1);

        let mut rng = Xoshiro256::seeded(self.seed)
            .fork(&format!("path-integral-{n}-{replicas}-{}", effort.seed));
        let mut slices: Vec<Vec<u8>> = (0..replicas)
            .map(|_| (0..n).map(|_| u8::from(rng.bernoulli(0.5))).collect())
            .collect();
        let mut energies: Vec<f64> = slices.iter().map(|s| qubo.evaluate(s)).collect();

        let mut trace = SearchTrace::new("path-integral-quantum-annealing");
        trace.replicas = replicas;
        trace.restarts = effort.restarts.max(1);
        let mut evaluations = replicas;

        let mut best = slices[0].clone();
        let mut best_energy = energies[0];
        for (slice, energy) in slices.iter().zip(&energies) {
            if *energy < best_energy {
                best_energy = *energy;
                best.copy_from_slice(slice);
            }
        }

        let replica_count = replicas as f64;
        for sweep in 0..sweeps {
            let progress = sweep as f64 / sweeps as f64;
            // Geometric schedule: the field spends most of the run small,
            // which is where the collective moves happen.
            let field = field_start * (field_end / field_start).powf(progress);
            // The imaginary-time coupling. Diverges as the field vanishes,
            // which is exactly the freezing that ends the run.
            let coupling = -(replica_count * temperature / 2.0)
                * (field / (replica_count * temperature)).tanh().ln();

            for index in 0..replicas {
                let previous = (index + replicas - 1) % replicas;
                let next = (index + 1) % replicas;
                for _ in 0..n {
                    let k = rng.below(n as u64) as usize;
                    let potential = qubo.delta(&slices[index], k);
                    evaluations += 1;
                    trace.moves_proposed += 1;
                    let current = spin(slices[index][k]);
                    let neighbours = spin(slices[previous][k]) + spin(slices[next][k]);
                    // The potential term is shared across replicas; the
                    // kinetic term is what makes this search different from
                    // annealing, and it is the only term that knows the other
                    // replicas exist.
                    let delta = potential / replica_count + 2.0 * coupling * current * neighbours;
                    let accept = delta <= 0.0 || rng.next_f64() < (-delta / temperature).exp();
                    if accept {
                        slices[index][k] ^= 1;
                        energies[index] += potential;
                        trace.moves_accepted += 1;
                        if potential > 0.0 {
                            trace.uphill_accepted += 1;
                        }
                        if energies[index] < best_energy {
                            best_energy = energies[index];
                            best.copy_from_slice(&slices[index]);
                        }
                    }
                }
            }
        }

        // Every replica is polished to a local optimum. A path-integral run
        // ends at a finite temperature, so its final configurations are near
        // a minimum rather than at one, and returning a near-miss would be
        // reporting the schedule's residual noise as the answer.
        for slice in &slices {
            let (polished, energy) = descend(qubo, slice.clone(), &mut evaluations);
            if energy < best_energy {
                best_energy = energy;
                best = polished;
            }
        }

        trace.local_optimum = is_local_optimum(qubo, &best);
        Ok(SolverCandidate {
            solver: self.name().to_string(),
            kind: SolverKind::QuantumInspired,
            claimed_objective: best_energy,
            assignment: best,
            evaluations,
            trace,
        })
    }
}

/// How an IBM Quantum job would be submitted.
///
/// Every field is something a deployment must supply and this build does not
/// have. They are named individually rather than rolled into one "not
/// configured" string so an operator reading the error knows which four things
/// to go and get.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IbmQuantumConfig {
    /// Environment variable holding the IBM Quantum API token. The token is
    /// never stored in configuration or in a repository.
    pub api_token_env: String,
    /// The service instance, as a Cloud Resource Name:
    /// `crn:v1:bluemix:public:quantum-computing:<region>:a/<account>:<instance>::`.
    /// Without it a token authenticates but addresses nothing.
    pub instance_crn: String,
    /// Channel the runtime is reached through, e.g. `ibm_quantum_platform`.
    pub channel: String,
    /// Backend name, e.g. `ibm_torino`. Not interchangeable: coupling map,
    /// basis gates and error rates all differ between devices.
    pub backend: String,
    /// What the deployment is willing to wait and spend for a slot.
    pub queue: QueuePolicy,
    pub max_qubits: usize,
    /// Price of one job in USD micro-units, for the benchmark's cost column.
    pub price_micros_per_job: u64,
}

/// What a deployment will accept from a shared device's queue.
///
/// A queue policy is not optional detail. A job submitted with no bound on
/// wait, shots or retries is a job that can sit for hours and then bill for a
/// result nobody is still waiting for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuePolicy {
    /// Longest queue wait the deployment will accept before abandoning a job.
    pub maximum_wait: Duration,
    /// Shots per circuit.
    pub shots: usize,
    /// Circuits per job.
    pub maximum_circuits: usize,
    /// Execution mode: `job`, `session` or `batch`. A session holds the device
    /// between circuits and is charged for the hold.
    pub mode: String,
}

impl QueuePolicy {
    pub fn describe(&self) -> String {
        format!(
            "{} mode, {} shot(s) per circuit, at most {} circuit(s) per job, abandoning a job that \
             queues longer than {:.0} minute(s)",
            self.mode,
            self.shots,
            self.maximum_circuits,
            self.maximum_wait.as_secs_f64() / 60.0
        )
    }
}

/// The IBM Quantum port.
///
/// Implements both [`QuboSolver`], so it can enter a benchmark, and
/// [`QuantumProvider`], so it can be handed to the compute router that already
/// exists. Both report unavailable, and both say the same thing about why.
///
/// This is not a stub. The interface is the one a real adapter would present,
/// and the reason it cannot run is stated rather than hidden: a deployment
/// that configures hardware fails at start-up with a legible message instead
/// of at the first optimisation with a confusing one.
#[derive(Clone, Debug)]
pub struct IbmQuantumSolver {
    config: IbmQuantumConfig,
    token_present: bool,
    instance_present: bool,
    /// Whether an HTTPS transport and a Qiskit Runtime client exist in this
    /// build. Always false. The field exists so the availability logic is the
    /// real one rather than a hard-coded answer.
    transport_present: bool,
}

impl IbmQuantumSolver {
    pub fn new(config: IbmQuantumConfig) -> Self {
        Self {
            config,
            token_present: false,
            instance_present: false,
            transport_present: false,
        }
    }

    /// Construct with credentials asserted present, to exercise the
    /// availability logic in a test. The transport still is not.
    pub fn with_credentials(
        config: IbmQuantumConfig,
        token_present: bool,
        instance_present: bool,
    ) -> Self {
        Self {
            config,
            token_present,
            instance_present,
            transport_present: false,
        }
    }

    pub fn config(&self) -> &IbmQuantumConfig {
        &self.config
    }

    /// Everything missing, named.
    fn missing(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if !self.token_present {
            missing.push(format!(
                "an IBM Quantum API token in the environment variable {}",
                self.config.api_token_env
            ));
        }
        if !self.instance_present {
            missing.push(format!(
                "the service instance CRN {} to address it against",
                self.config.instance_crn
            ));
        }
        if !self.transport_present {
            missing.push(format!(
                "an HTTPS transport and a Qiskit Runtime client for the {} channel, neither of \
                 which is present in this build",
                self.config.channel
            ));
        }
        missing
    }

    /// The text an operator needs, naming every missing item.
    ///
    /// Inherent rather than only on the traits, and named for what it is, so
    /// both trait implementations delegate to one sentence instead of drifting
    /// into two.
    pub fn production_requirement(&self) -> String {
        format!(
            "the IBM Quantum backend {} is not usable: it needs {}. The queue policy it would run \
             under is {}. Until all of that is present the platform uses the classical solver, \
             which is the configured default.",
            self.config.backend,
            self.missing().join("; and "),
            self.config.queue.describe()
        )
    }
}

impl QuboSolver for IbmQuantumSolver {
    fn name(&self) -> &str {
        &self.config.backend
    }

    fn kind(&self) -> SolverKind {
        SolverKind::Quantum
    }

    fn is_available(&self) -> bool {
        self.token_present && self.instance_present && self.transport_present
    }

    fn requirement(&self) -> String {
        self.production_requirement()
    }

    fn cost_model(&self) -> SolverCostModel {
        SolverCostModel {
            // A device is not charged by objective evaluation; the queue and
            // the per-job price are what a run actually costs.
            nanos_per_evaluation: 0,
            queue: self.config.queue.maximum_wait,
            price_micros_per_job: self.config.price_micros_per_job,
        }
    }

    fn solve(&self, _qubo: &Qubo, _effort: &SolverEffort) -> Result<SolverCandidate> {
        Err(Error::unavailable(self.production_requirement()))
    }
}

impl QuantumProvider for IbmQuantumSolver {
    fn name(&self) -> &str {
        &self.config.backend
    }

    fn is_available(&self) -> bool {
        self.token_present && self.instance_present && self.transport_present
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            max_qubits: self.config.max_qubits,
            simulated: false,
            noisy: true,
            typical_queue: self.config.queue.maximum_wait,
            cost_per_job_micros: self.config.price_micros_per_job,
        }
    }

    fn solve_qubo(
        &self,
        _qubo: &Qubo,
        _settings: &QaoaSettings,
    ) -> Result<crate::qaoa::QaoaResult> {
        Err(Error::unavailable(self.production_requirement()))
    }

    fn requirement(&self) -> String {
        self.production_requirement()
    }
}

/// Any [`QuantumProvider`] as a [`QuboSolver`].
///
/// The bridge between the port that already existed and the abstraction the
/// benchmark runs against, so the in-tree statevector simulator enters a
/// benchmark as a quantum entrant without being reimplemented.
#[derive(Clone, Debug)]
pub struct ProviderSolver {
    provider: Arc<dyn QuantumProvider>,
    settings: QaoaSettings,
    cost: Option<SolverCostModel>,
}

impl ProviderSolver {
    pub fn new(provider: Arc<dyn QuantumProvider>, settings: QaoaSettings) -> Self {
        Self {
            provider,
            settings,
            cost: None,
        }
    }

    pub fn with_cost(mut self, cost: SolverCostModel) -> Self {
        self.cost = Some(cost);
        self
    }

    pub fn provider(&self) -> &Arc<dyn QuantumProvider> {
        &self.provider
    }
}

impl QuboSolver for ProviderSolver {
    fn name(&self) -> &str {
        self.provider.name()
    }

    fn kind(&self) -> SolverKind {
        SolverKind::Quantum
    }

    fn is_available(&self) -> bool {
        self.provider.is_available()
    }

    fn requirement(&self) -> String {
        self.provider.requirement()
    }

    fn cost_model(&self) -> SolverCostModel {
        self.cost.unwrap_or_else(|| {
            let capabilities = self.provider.capabilities();
            SolverCostModel {
                // Simulating a QAOA circuit costs more per evaluation than
                // solving the problem classically, and the model says so
                // rather than flattering the simulator.
                nanos_per_evaluation: if capabilities.simulated { 5_000 } else { 0 },
                queue: capabilities.typical_queue,
                price_micros_per_job: capabilities.cost_per_job_micros,
            }
        })
    }

    fn solve(&self, qubo: &Qubo, effort: &SolverEffort) -> Result<SolverCandidate> {
        let mut settings = self.settings;
        // The caller's budget governs the angle search: it is what a QAOA
        // attempt actually costs.
        settings.optimiser_iterations = settings.optimiser_iterations.max(effort.sweeps);
        let result = self.provider.solve_qubo(qubo, &settings)?;
        let mut trace = SearchTrace::new("qaoa");
        trace.moves_proposed = result.evaluations;
        trace.restarts = 1;
        trace.local_optimum = is_local_optimum(qubo, &result.assignment);
        Ok(SolverCandidate {
            solver: self.provider.name().to_string(),
            kind: SolverKind::Quantum,
            claimed_objective: result.energy,
            assignment: result.assignment,
            evaluations: result.evaluations,
            trace,
        })
    }
}
