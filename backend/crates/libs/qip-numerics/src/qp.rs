//! Convex quadratic programming.
//!
//! Portfolio construction is a QP: minimise `½x'Qx + c'x` subject to box
//! bounds, budget equalities and exposure inequalities. The solver here is an
//! ADMM splitting in the style of OSQP — chosen because it handles equalities
//! and inequalities uniformly as a single `l ≤ Ax ≤ u` set, degrades
//! gracefully on the ill-conditioned covariance matrices real data produces,
//! and is short enough to audit line by line.

use crate::matrix::Matrix;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// One row of the constraint set: `lower ≤ coefficients · x ≤ upper`.
///
/// An equality is `lower == upper`; a one-sided bound uses an infinity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinearConstraint {
    pub coefficients: Vec<f64>,
    pub lower: f64,
    pub upper: f64,
    /// Human-readable reason, surfaced when a problem proves infeasible.
    pub label: String,
}

impl LinearConstraint {
    pub fn equality(coefficients: Vec<f64>, value: f64, label: impl Into<String>) -> Self {
        Self {
            coefficients,
            lower: value,
            upper: value,
            label: label.into(),
        }
    }

    pub fn at_most(coefficients: Vec<f64>, upper: f64, label: impl Into<String>) -> Self {
        Self {
            coefficients,
            lower: f64::NEG_INFINITY,
            upper,
            label: label.into(),
        }
    }

    pub fn at_least(coefficients: Vec<f64>, lower: f64, label: impl Into<String>) -> Self {
        Self {
            coefficients,
            lower,
            upper: f64::INFINITY,
            label: label.into(),
        }
    }

    pub fn between(
        coefficients: Vec<f64>,
        lower: f64,
        upper: f64,
        label: impl Into<String>,
    ) -> Self {
        Self {
            coefficients,
            lower,
            upper,
            label: label.into(),
        }
    }

    pub fn is_equality(&self) -> bool {
        (self.upper - self.lower).abs() < 1e-12
    }

    /// Signed violation of this constraint at `x`: 0.0 when satisfied.
    pub fn violation(&self, x: &[f64]) -> f64 {
        let value = crate::vector::dot(&self.coefficients, x);
        if value < self.lower {
            self.lower - value
        } else if value > self.upper {
            value - self.upper
        } else {
            0.0
        }
    }
}

/// `minimise ½x'Qx + c'x` subject to box bounds and linear constraints.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuadraticProgram {
    /// Symmetric positive semi-definite objective matrix.
    pub q: Matrix,
    pub c: Vec<f64>,
    pub lower_bounds: Vec<f64>,
    pub upper_bounds: Vec<f64>,
    pub constraints: Vec<LinearConstraint>,
}

impl QuadraticProgram {
    /// Unconstrained-but-bounded problem; add constraints afterwards.
    pub fn new(q: Matrix, c: Vec<f64>) -> Result<Self> {
        let n = c.len();
        if q.rows() != n || q.cols() != n {
            return Err(Error::invalid(
                "objective matrix must be n x n for an n-vector c",
            ));
        }
        Ok(Self {
            q,
            c,
            lower_bounds: vec![f64::NEG_INFINITY; n],
            upper_bounds: vec![f64::INFINITY; n],
            constraints: Vec::new(),
        })
    }

    pub fn dimension(&self) -> usize {
        self.c.len()
    }

    pub fn with_bounds(mut self, lower: Vec<f64>, upper: Vec<f64>) -> Result<Self> {
        if lower.len() != self.dimension() || upper.len() != self.dimension() {
            return Err(Error::invalid(
                "bound vectors must match the problem dimension",
            ));
        }
        if lower.iter().zip(&upper).any(|(l, u)| l > u) {
            return Err(Error::invalid("lower bound exceeds upper bound"));
        }
        self.lower_bounds = lower;
        self.upper_bounds = upper;
        Ok(self)
    }

    pub fn with_constraint(mut self, constraint: LinearConstraint) -> Result<Self> {
        if constraint.coefficients.len() != self.dimension() {
            return Err(Error::invalid(format!(
                "constraint `{}` has {} coefficients, expected {}",
                constraint.label,
                constraint.coefficients.len(),
                self.dimension()
            )));
        }
        self.constraints.push(constraint);
        Ok(self)
    }

    /// Budget constraint `sum(x) = total`.
    pub fn with_budget(self, total: f64) -> Result<Self> {
        let n = self.dimension();
        self.with_constraint(LinearConstraint::equality(vec![1.0; n], total, "budget"))
    }

    pub fn objective_at(&self, x: &[f64]) -> Result<f64> {
        Ok(0.5 * self.q.quadratic_form(x)? + crate::vector::dot(&self.c, x))
    }

    /// Largest constraint or bound violation at `x`.
    pub fn worst_violation(&self, x: &[f64]) -> (f64, String) {
        let mut worst = 0.0;
        let mut label = String::from("none");
        for (i, xi) in x.iter().enumerate() {
            let below = self.lower_bounds[i] - xi;
            let above = xi - self.upper_bounds[i];
            if below > worst {
                worst = below;
                label = format!("lower bound {i}");
            }
            if above > worst {
                worst = above;
                label = format!("upper bound {i}");
            }
        }
        for constraint in &self.constraints {
            let v = constraint.violation(x);
            if v > worst {
                worst = v;
                label = constraint.label.clone();
            }
        }
        (worst, label)
    }
}

/// Result of a QP solve, including the diagnostics needed to decide whether to
/// trust the answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QpSolution {
    pub x: Vec<f64>,
    pub objective: f64,
    pub iterations: usize,
    pub converged: bool,
    pub primal_residual: f64,
    pub dual_residual: f64,
    /// Set when the constraint set admits no feasible point.
    pub infeasible: bool,
}

/// Tunables for [`solve`].
#[derive(Clone, Debug)]
pub struct QpSettings {
    pub max_iterations: usize,
    pub primal_tolerance: f64,
    pub dual_tolerance: f64,
    /// ADMM step size. Larger values chase feasibility faster but oscillate.
    pub rho: f64,
    /// Proximal regularisation, which also makes a merely PSD `Q` invertible.
    pub sigma: f64,
    /// Over-relaxation factor; 1.6 is the usual sweet spot.
    pub alpha: f64,
}

impl Default for QpSettings {
    fn default() -> Self {
        Self {
            max_iterations: 6_000,
            primal_tolerance: 1e-9,
            dual_tolerance: 1e-9,
            rho: 1.0,
            sigma: 1e-8,
            alpha: 1.6,
        }
    }
}

/// Solve a convex QP by ADMM.
///
/// Box bounds are folded in as identity rows, so bounds and general constraints
/// share one projection step.
pub fn solve(problem: &QuadraticProgram, settings: &QpSettings) -> Result<QpSolution> {
    let n = problem.dimension();
    if n == 0 {
        return Ok(QpSolution {
            x: Vec::new(),
            objective: 0.0,
            iterations: 0,
            converged: true,
            primal_residual: 0.0,
            dual_residual: 0.0,
            infeasible: false,
        });
    }
    if !problem.q.all_finite() || !crate::vector::all_finite(&problem.c) {
        return Err(Error::numeric("objective contains non-finite values"));
    }

    // Stack constraints: identity rows for the box, then the explicit rows.
    let rows = n + problem.constraints.len();
    let mut a = Matrix::zeros(rows, n);
    let mut lower = vec![0.0; rows];
    let mut upper = vec![0.0; rows];
    for i in 0..n {
        a.set(i, i, 1.0);
        lower[i] = problem.lower_bounds[i];
        upper[i] = problem.upper_bounds[i];
    }
    for (k, constraint) in problem.constraints.iter().enumerate() {
        for (j, coefficient) in constraint.coefficients.iter().enumerate() {
            a.set(n + k, j, *coefficient);
        }
        lower[n + k] = constraint.lower;
        upper[n + k] = constraint.upper;
    }

    let at = a.transpose();
    let ata = at.matmul(&a)?;
    // KKT matrix: Q + sigma*I + rho*A'A. Positive definite for any PSD Q.
    let mut kkt = problem.q.symmetrised();
    for i in 0..n {
        kkt.add_to(i, i, settings.sigma);
    }
    kkt = kkt.add(&ata.scale(settings.rho))?;

    let mut x = vec![0.0; n];
    let mut z: Vec<f64> = (0..rows)
        .map(|i| clamp_finite(0.0, lower[i], upper[i]))
        .collect();
    let mut y = vec![0.0; rows];

    let mut primal_residual = f64::INFINITY;
    let mut dual_residual = f64::INFINITY;
    let mut iterations = 0;

    for iteration in 0..settings.max_iterations {
        iterations = iteration + 1;

        // x-update: solve the KKT system.
        let mut rhs = vec![0.0; n];
        for i in 0..n {
            rhs[i] = settings.sigma * x[i] - problem.c[i];
        }
        let combined: Vec<f64> = (0..rows).map(|i| settings.rho * z[i] - y[i]).collect();
        let at_combined = at.mul_vec(&combined)?;
        for i in 0..n {
            rhs[i] += at_combined[i];
        }
        let x_next = kkt.solve(&rhs)?;

        // z-update with over-relaxation, projecting onto [lower, upper].
        let ax = a.mul_vec(&x_next)?;
        let z_prev = z.clone();
        for i in 0..rows {
            let relaxed = settings.alpha * ax[i] + (1.0 - settings.alpha) * z_prev[i];
            z[i] = clamp_finite(relaxed + y[i] / settings.rho, lower[i], upper[i]);
        }

        // y-update (dual ascent).
        for i in 0..rows {
            let relaxed = settings.alpha * ax[i] + (1.0 - settings.alpha) * z_prev[i];
            y[i] += settings.rho * (relaxed - z[i]);
        }

        primal_residual = (0..rows).fold(0.0_f64, |m, i| m.max((ax[i] - z[i]).abs()));
        let dz: Vec<f64> = (0..rows).map(|i| z[i] - z_prev[i]).collect();
        let at_dz = at.mul_vec(&dz)?;
        dual_residual = crate::vector::norm_inf(&at_dz) * settings.rho;

        x = x_next;
        if primal_residual < settings.primal_tolerance && dual_residual < settings.dual_tolerance {
            break;
        }
    }

    // A large surviving primal residual with a settled dual means the
    // constraint set itself has no feasible point, not that we stopped early.
    let (violation, _) = problem.worst_violation(&x);
    let infeasible = violation > 1e-4 && dual_residual < 1e-6;
    let converged = primal_residual < settings.primal_tolerance * 1e3
        && dual_residual < settings.dual_tolerance * 1e3;

    // Snap to the box so the returned point never reports a bound violation
    // caused purely by the solver's own tolerance.
    for ((xi, lo), hi) in x
        .iter_mut()
        .zip(&problem.lower_bounds)
        .zip(&problem.upper_bounds)
    {
        *xi = clamp_finite(*xi, *lo, *hi);
    }

    Ok(QpSolution {
        objective: problem.objective_at(&x)?,
        x,
        iterations,
        converged,
        primal_residual,
        dual_residual,
        infeasible,
    })
}

/// Solve with default settings.
pub fn solve_default(problem: &QuadraticProgram) -> Result<QpSolution> {
    solve(problem, &QpSettings::default())
}

/// Projected-gradient solve for the common case: box bounds plus one budget
/// equality. Faster and needs no matrix factorisation, so it is the preferred
/// path for large long-only portfolios.
pub fn solve_box_budget(
    q: &Matrix,
    c: &[f64],
    lower: &[f64],
    upper: &[f64],
    budget: f64,
    max_iterations: usize,
) -> Result<QpSolution> {
    let n = c.len();
    if q.rows() != n || q.cols() != n {
        return Err(Error::invalid("objective matrix must match c"));
    }
    // Step size from the largest curvature; guarantees a non-expansive step.
    let eigen = q.symmetric_eigen()?;
    let l_max = eigen.values.first().copied().unwrap_or(1.0).abs().max(1e-8);
    let step = 1.0 / l_max;

    let Some(mut x) =
        crate::vector::project_box_sum(&vec![budget / n as f64; n], lower, upper, budget)
    else {
        return Ok(QpSolution {
            x: vec![0.0; n],
            objective: 0.0,
            iterations: 0,
            converged: false,
            primal_residual: f64::INFINITY,
            dual_residual: 0.0,
            infeasible: true,
        });
    };

    let mut iterations = 0;
    let mut movement = f64::INFINITY;
    for iteration in 0..max_iterations {
        iterations = iteration + 1;
        let qx = q.mul_vec(&x)?;
        let gradient: Vec<f64> = (0..n).map(|i| qx[i] + c[i]).collect();
        let candidate: Vec<f64> = (0..n).map(|i| x[i] - step * gradient[i]).collect();
        let Some(projected) = crate::vector::project_box_sum(&candidate, lower, upper, budget)
        else {
            break;
        };
        movement = crate::vector::norm_inf(&crate::vector::sub(&projected, &x));
        x = projected;
        if movement < 1e-12 {
            break;
        }
    }

    let objective = 0.5 * q.quadratic_form(&x)? + crate::vector::dot(c, &x);
    Ok(QpSolution {
        x,
        objective,
        iterations,
        converged: movement < 1e-9,
        primal_residual: 0.0,
        dual_residual: movement,
        infeasible: false,
    })
}

/// Clamp that treats infinite bounds as absent.
fn clamp_finite(v: f64, lo: f64, hi: f64) -> f64 {
    let mut out = v;
    if lo.is_finite() && out < lo {
        out = lo;
    }
    if hi.is_finite() && out > hi {
        out = hi;
    }
    out
}
