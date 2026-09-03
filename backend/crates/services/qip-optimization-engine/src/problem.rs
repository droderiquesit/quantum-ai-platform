//! Portfolio optimisation problems.
//!
//! One problem type, expressible as either a quadratic program or a QUBO, so
//! the classical and quantum paths solve the *same* problem rather than two
//! similar ones. That is what makes the comparison in
//! [`crate::router::ComputeRouter`] meaningful: a quantum result that beat a
//! classical result on a slightly different formulation would be evidence of
//! nothing.

use qip_core::error::{Error, Result};
use qip_numerics::Matrix;
use qip_numerics::anneal::Qubo;
use qip_numerics::qp::{LinearConstraint, QuadraticProgram};
use serde::{Deserialize, Serialize};

/// What the optimiser is being asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Objective {
    /// Maximise expected return less a risk penalty.
    MeanVariance,
    /// Minimise variance, ignoring expected return.
    MinimumVariance,
    /// Equalise each holding's contribution to total risk.
    RiskParity,
    /// Get as close as possible to a target set of weights.
    TrackTarget,
}

impl Objective {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::MeanVariance => "mean_variance",
            Self::MinimumVariance => "minimum_variance",
            Self::RiskParity => "risk_parity",
            Self::TrackTarget => "track_target",
        }
    }
}

/// A constraint the optimiser must respect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PortfolioConstraint {
    pub label: String,
    /// Per-asset coefficients.
    pub coefficients: Vec<f64>,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
}

impl PortfolioConstraint {
    pub fn equals(label: impl Into<String>, coefficients: Vec<f64>, value: f64) -> Self {
        Self {
            label: label.into(),
            coefficients,
            lower: Some(value),
            upper: Some(value),
        }
    }

    pub fn at_most(label: impl Into<String>, coefficients: Vec<f64>, upper: f64) -> Self {
        Self {
            label: label.into(),
            coefficients,
            lower: None,
            upper: Some(upper),
        }
    }

    pub fn at_least(label: impl Into<String>, coefficients: Vec<f64>, lower: f64) -> Self {
        Self {
            label: label.into(),
            coefficients,
            lower: Some(lower),
            upper: None,
        }
    }

    fn to_linear(&self) -> Result<LinearConstraint> {
        match (self.lower, self.upper) {
            (Some(lower), Some(upper)) if (upper - lower).abs() < 1e-12 => Ok(
                LinearConstraint::equality(self.coefficients.clone(), lower, self.label.clone()),
            ),
            (Some(lower), Some(upper)) => Ok(LinearConstraint::between(
                self.coefficients.clone(),
                lower,
                upper,
                self.label.clone(),
            )),
            (None, Some(upper)) => Ok(LinearConstraint::at_most(
                self.coefficients.clone(),
                upper,
                self.label.clone(),
            )),
            (Some(lower), None) => Ok(LinearConstraint::at_least(
                self.coefficients.clone(),
                lower,
                self.label.clone(),
            )),
            (None, None) => Err(Error::invalid(format!(
                "constraint {} bounds nothing",
                self.label
            ))),
        }
    }
}

/// The optimisation problem.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PortfolioProblem {
    pub objective: Objective,
    /// Instrument ids, in the order the vectors use.
    pub assets: Vec<String>,
    /// Expected returns per period. Empty for objectives that ignore them.
    pub expected_returns: Vec<f64>,
    /// Covariance, row-major and symmetric.
    pub covariance: Vec<Vec<f64>>,
    /// Risk aversion. Higher means the variance term dominates.
    pub risk_aversion: f64,
    pub lower_bounds: Vec<f64>,
    pub upper_bounds: Vec<f64>,
    pub constraints: Vec<PortfolioConstraint>,
    /// Maximum number of holdings. This is what makes the problem discrete and
    /// therefore what makes a quantum formulation worth discussing at all — a
    /// continuous mean-variance problem is solved classically in microseconds.
    pub cardinality: Option<usize>,
    /// Weights currently held, for a turnover penalty.
    pub current_weights: Vec<f64>,
    /// Cost per unit of turnover.
    pub turnover_penalty: f64,
    /// Weights to track, for `TrackTarget`.
    pub target_weights: Vec<f64>,
}

impl PortfolioProblem {
    pub fn new(assets: Vec<String>, covariance: Vec<Vec<f64>>) -> Result<Self> {
        let n = assets.len();
        if n == 0 {
            return Err(Error::invalid("an optimisation needs at least one asset"));
        }
        if covariance.len() != n || covariance.iter().any(|row| row.len() != n) {
            return Err(Error::invalid(format!(
                "covariance must be {n}x{n} to match {n} assets"
            )));
        }
        Ok(Self {
            objective: Objective::MeanVariance,
            assets,
            expected_returns: vec![0.0; n],
            covariance,
            risk_aversion: 5.0,
            lower_bounds: vec![0.0; n],
            upper_bounds: vec![1.0; n],
            constraints: Vec::new(),
            cardinality: None,
            current_weights: vec![0.0; n],
            turnover_penalty: 0.0,
            target_weights: Vec::new(),
        })
    }

    pub fn dimension(&self) -> usize {
        self.assets.len()
    }

    pub fn with_objective(mut self, objective: Objective) -> Self {
        self.objective = objective;
        self
    }

    pub fn with_expected_returns(mut self, returns: Vec<f64>) -> Self {
        self.expected_returns = returns;
        self
    }

    pub fn with_risk_aversion(mut self, risk_aversion: f64) -> Self {
        self.risk_aversion = risk_aversion;
        self
    }

    pub fn with_bounds(mut self, lower: Vec<f64>, upper: Vec<f64>) -> Self {
        self.lower_bounds = lower;
        self.upper_bounds = upper;
        self
    }

    pub fn with_constraint(mut self, constraint: PortfolioConstraint) -> Self {
        self.constraints.push(constraint);
        self
    }

    /// Require the weights to sum to `total`.
    pub fn fully_invested(self, total: f64) -> Self {
        let n = self.dimension();
        self.with_constraint(PortfolioConstraint::equals("budget", vec![1.0; n], total))
    }

    pub fn with_cardinality(mut self, maximum: usize) -> Self {
        self.cardinality = Some(maximum);
        self
    }

    pub fn with_turnover_penalty(mut self, current: Vec<f64>, penalty: f64) -> Self {
        self.current_weights = current;
        self.turnover_penalty = penalty;
        self
    }

    pub fn tracking(mut self, target: Vec<f64>) -> Self {
        self.objective = Objective::TrackTarget;
        self.target_weights = target;
        self
    }

    /// Whether the problem has discrete structure.
    ///
    /// The only honest reason to consider a quantum method: a continuous
    /// convex problem has a classical solution in microseconds.
    pub fn is_discrete(&self) -> bool {
        self.cardinality.is_some()
    }

    pub fn validate(&self) -> Result<()> {
        let n = self.dimension();
        if self.lower_bounds.len() != n || self.upper_bounds.len() != n {
            return Err(Error::invalid("bounds must have one entry per asset"));
        }
        for i in 0..n {
            if self.lower_bounds[i] > self.upper_bounds[i] {
                return Err(Error::invalid(format!(
                    "asset {} has a lower bound above its upper bound",
                    self.assets[i]
                )));
            }
        }
        if self.objective == Objective::MeanVariance && self.expected_returns.len() != n {
            return Err(Error::invalid(
                "mean-variance needs an expected return per asset",
            ));
        }
        if self.objective == Objective::TrackTarget && self.target_weights.len() != n {
            return Err(Error::invalid("tracking needs a target weight per asset"));
        }
        for constraint in &self.constraints {
            if constraint.coefficients.len() != n {
                return Err(Error::invalid(format!(
                    "constraint {} has {} coefficients for {n} assets",
                    constraint.label,
                    constraint.coefficients.len()
                )));
            }
        }
        if let Some(cardinality) = self.cardinality {
            if cardinality == 0 {
                return Err(Error::invalid("a cardinality limit of zero holds nothing"));
            }
            if cardinality > n {
                return Err(Error::invalid(format!(
                    "a cardinality limit of {cardinality} exceeds the {n} assets available"
                )));
            }
        }
        // A covariance matrix must be symmetric. An asymmetric one is a bug
        // upstream, and it makes the quadratic form meaningless.
        for i in 0..n {
            for j in (i + 1)..n {
                if (self.covariance[i][j] - self.covariance[j][i]).abs() > 1e-9 {
                    return Err(Error::invalid(format!(
                        "covariance is not symmetric at ({i}, {j})"
                    )));
                }
            }
            if self.covariance[i][i] < 0.0 {
                return Err(Error::numeric(format!(
                    "asset {} has negative variance",
                    self.assets[i]
                )));
            }
        }
        Ok(())
    }

    /// The objective value of a weight vector.
    pub fn objective_at(&self, weights: &[f64]) -> f64 {
        let n = self.dimension();
        if weights.len() != n {
            return f64::NAN;
        }
        let variance: f64 = (0..n)
            .map(|i| {
                (0..n)
                    .map(|j| weights[i] * self.covariance[i][j] * weights[j])
                    .sum::<f64>()
            })
            .sum();

        let base = match self.objective {
            Objective::MeanVariance => {
                let expected: f64 = (0..n).map(|i| weights[i] * self.expected_returns[i]).sum();
                0.5 * self.risk_aversion * variance - expected
            }
            Objective::MinimumVariance | Objective::RiskParity => 0.5 * variance,
            Objective::TrackTarget => {
                // Tracking error against the target, as a plain squared
                // deviation: the covariance-weighted version is available via
                // MinimumVariance on the difference.
                0.5 * (0..n)
                    .map(|i| (weights[i] - self.target_weights[i]).powi(2))
                    .sum::<f64>()
            }
        };

        let turnover: f64 = if self.turnover_penalty > 0.0 {
            (0..n)
                .map(|i| (weights[i] - self.current_weights.get(i).copied().unwrap_or(0.0)).abs())
                .sum::<f64>()
        } else {
            0.0
        };

        base + self.turnover_penalty * turnover
    }

    /// Express as a quadratic program for the classical solver.
    ///
    /// The cardinality constraint cannot be expressed here — it is not convex —
    /// so the QP is a *relaxation* when one is set. The router knows this and
    /// reports it, because a relaxation presented as the answer is how a
    /// portfolio ends up with sixty names against a thirty-name mandate.
    pub fn to_quadratic_program(&self) -> Result<QuadraticProgram> {
        self.validate()?;
        let n = self.dimension();

        let q = match self.objective {
            Objective::TrackTarget => {
                // Identity: the tracking objective is a plain squared distance.
                let mut rows = vec![vec![0.0; n]; n];
                for (i, row) in rows.iter_mut().enumerate() {
                    row[i] = 1.0;
                }
                Matrix::from_rows(&rows)?
            }
            _ => {
                let scale = if self.objective == Objective::MeanVariance {
                    self.risk_aversion
                } else {
                    1.0
                };
                let scaled: Vec<Vec<f64>> = self
                    .covariance
                    .iter()
                    .map(|row| row.iter().map(|v| v * scale).collect())
                    .collect();
                Matrix::from_rows(&scaled)?
            }
        };

        let c: Vec<f64> = match self.objective {
            Objective::MeanVariance => self.expected_returns.iter().map(|r| -r).collect(),
            Objective::TrackTarget => self.target_weights.iter().map(|w| -w).collect(),
            _ => vec![0.0; n],
        };

        let mut program = QuadraticProgram::new(q, c)?
            .with_bounds(self.lower_bounds.clone(), self.upper_bounds.clone())?;
        for constraint in &self.constraints {
            program = program.with_constraint(constraint.to_linear()?)?;
        }
        Ok(program)
    }

    /// Express as a QUBO for an annealer or QAOA.
    ///
    /// Each asset gets one binary variable meaning "hold at the discretised
    /// weight". That is a coarse encoding, and deliberately so: a
    /// multi-bit-per-asset encoding needs `assets × bits` qubits, and at
    /// today's qubit counts the problem stops fitting long before it stops
    /// being trivial classically. [`QuboEncoding`] records what was lost.
    pub fn to_qubo(&self, encoding: &QuboEncoding) -> Result<Qubo> {
        self.validate()?;
        let n = self.dimension();
        let weight = encoding.weight_per_holding;
        let mut qubo = Qubo::new(n);

        // Variance: w² · Σᵢⱼ, since each selected asset carries the same weight.
        for i in 0..n {
            for j in 0..n {
                let scale = if self.objective == Objective::MeanVariance {
                    self.risk_aversion
                } else {
                    1.0
                };
                qubo.add(i, j, 0.5 * scale * weight * weight * self.covariance[i][j]);
            }
        }

        // Expected return, negated because a QUBO is minimised.
        if self.objective == Objective::MeanVariance {
            for i in 0..n {
                qubo.add_linear(i, -weight * self.expected_returns[i]);
            }
        }

        // Cardinality as a penalty: λ(Σxᵢ − k)². Expanding gives a linear term
        // of λ(1 − 2k) on each variable and a quadratic λ on each pair.
        if let Some(cardinality) = self.cardinality {
            let lambda = encoding.cardinality_penalty;
            let k = cardinality as f64;
            for i in 0..n {
                qubo.add_linear(i, lambda * (1.0 - 2.0 * k));
                for j in (i + 1)..n {
                    qubo.add(i, j, 2.0 * lambda);
                }
            }
        }

        Ok(qubo)
    }
}

/// How a continuous problem was reduced to binary variables.
///
/// Carried alongside every QUBO result so the reduction is visible: a quantum
/// result on a coarsened problem is not comparable to a classical result on
/// the real one, and the router refuses to compare them without this.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuboEncoding {
    /// Weight each selected asset receives.
    pub weight_per_holding: f64,
    /// Penalty coefficient on the cardinality constraint.
    ///
    /// Must exceed the objective's own scale or the optimiser will happily pay
    /// the penalty to get a better objective, silently violating the mandate.
    pub cardinality_penalty: f64,
}

impl QuboEncoding {
    /// An encoding for `k` equally weighted holdings out of `n`.
    pub fn equal_weight(cardinality: usize, gross: f64) -> Self {
        Self::equal_weight_bounded(cardinality, gross, f64::INFINITY)
    }

    /// An encoding that also respects a per-holding cap.
    ///
    /// `gross / cardinality` can exceed the mandate's own position limit — five
    /// names fully invested is twenty percent each, which breaches a fifteen
    /// percent cap. Without the cap here every candidate the encoding produces
    /// would be infeasible and the whole discrete path would return nothing
    /// usable, which is a confusing way to discover that two mandate
    /// parameters contradict each other. Capping instead keeps the answer
    /// feasible and lets the shortfall against the gross target be visible.
    pub fn equal_weight_bounded(cardinality: usize, gross: f64, maximum_weight: f64) -> Self {
        let weight = if cardinality == 0 {
            0.0
        } else {
            (gross / cardinality as f64).min(maximum_weight.max(0.0))
        };
        Self {
            weight_per_holding: weight,
            // Ten times the largest plausible objective contribution, so
            // violating the cardinality is never worth it.
            cardinality_penalty: 10.0,
        }
    }

    /// Gross exposure this encoding reaches at full cardinality.
    pub fn achievable_gross(&self, cardinality: usize) -> f64 {
        self.weight_per_holding * cardinality as f64
    }

    pub fn with_penalty(mut self, penalty: f64) -> Self {
        self.cardinality_penalty = penalty;
        self
    }

    /// Turn a binary assignment back into weights.
    pub fn to_weights(&self, assignment: &[u8]) -> Vec<f64> {
        assignment
            .iter()
            .map(|bit| {
                if *bit == 1 {
                    self.weight_per_holding
                } else {
                    0.0
                }
            })
            .collect()
    }

    pub fn describe(&self) -> String {
        format!(
            "one binary variable per asset at a fixed {:.4} weight; continuous sizing within a holding is not represented",
            self.weight_per_holding
        )
    }
}
