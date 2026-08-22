//! Linear programming by the two-phase primal simplex.
//!
//! Needed for CVaR optimisation (the Rockafellar-Uryasev formulation is an LP)
//! and for the transaction-cost and turnover subproblems in trade scheduling.
//! Bland's rule is used on the pivot choice, which trades a little speed for a
//! guarantee against cycling — worth it in a system that must not hang.

use crate::matrix::Matrix;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// Sense of a constraint row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sense {
    LessOrEqual,
    Equal,
    GreaterOrEqual,
}

/// `minimise c'x` subject to the rows of `a` and `x >= 0`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinearProgram {
    pub objective: Vec<f64>,
    pub rows: Vec<Vec<f64>>,
    pub senses: Vec<Sense>,
    pub rhs: Vec<f64>,
}

impl LinearProgram {
    pub fn minimise(objective: Vec<f64>) -> Self {
        Self {
            objective,
            rows: Vec::new(),
            senses: Vec::new(),
            rhs: Vec::new(),
        }
    }

    pub fn subject_to(mut self, row: Vec<f64>, sense: Sense, rhs: f64) -> Result<Self> {
        if row.len() != self.objective.len() {
            return Err(Error::invalid(
                "constraint width does not match the objective",
            ));
        }
        self.rows.push(row);
        self.senses.push(sense);
        self.rhs.push(rhs);
        Ok(self)
    }

    pub fn dimension(&self) -> usize {
        self.objective.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LpStatus {
    Optimal,
    Infeasible,
    Unbounded,
    IterationLimit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LpSolution {
    pub status: LpStatus,
    pub x: Vec<f64>,
    pub objective: f64,
    pub iterations: usize,
}

/// Solve with the two-phase primal simplex.
pub fn solve(program: &LinearProgram) -> Result<LpSolution> {
    let n = program.dimension();
    let m = program.rows.len();
    if n == 0 {
        return Ok(LpSolution {
            status: LpStatus::Optimal,
            x: Vec::new(),
            objective: 0.0,
            iterations: 0,
        });
    }
    if program.senses.len() != m || program.rhs.len() != m {
        return Err(Error::invalid(
            "constraint rows, senses and rhs must agree in length",
        ));
    }

    // Convert to standard form: rows become equalities via slack/surplus, and
    // every right-hand side is made non-negative first so the artificial basis
    // is feasible.
    let mut slack_count = 0usize;
    for sense in &program.senses {
        if *sense != Sense::Equal {
            slack_count += 1;
        }
    }
    let total_structural = n + slack_count;
    let width = total_structural + m; // structural + slacks + artificials

    let mut tableau = Matrix::zeros(m + 1, width + 1);
    let mut slack_cursor = 0usize;
    let mut basis: Vec<usize> = Vec::with_capacity(m);

    for r in 0..m {
        let flip = program.rhs[r] < 0.0;
        let sign = if flip { -1.0 } else { 1.0 };
        for c in 0..n {
            tableau.set(r, c, sign * program.rows[r][c]);
        }
        let effective_sense = if flip {
            match program.senses[r] {
                Sense::LessOrEqual => Sense::GreaterOrEqual,
                Sense::GreaterOrEqual => Sense::LessOrEqual,
                Sense::Equal => Sense::Equal,
            }
        } else {
            program.senses[r]
        };
        match effective_sense {
            Sense::LessOrEqual => {
                tableau.set(r, n + slack_cursor, 1.0);
                slack_cursor += 1;
            }
            Sense::GreaterOrEqual => {
                tableau.set(r, n + slack_cursor, -1.0);
                slack_cursor += 1;
            }
            Sense::Equal => {}
        }
        // Every row gets an artificial so phase one starts from a basis.
        tableau.set(r, total_structural + r, 1.0);
        basis.push(total_structural + r);
        tableau.set(r, width, sign * program.rhs[r]);
    }

    // Phase one: drive the artificials to zero.
    for c in 0..=width {
        let mut acc = 0.0;
        for r in 0..m {
            acc -= tableau.get(r, c);
        }
        tableau.set(m, c, acc);
    }
    for r in 0..m {
        let artificial = total_structural + r;
        tableau.add_to(m, artificial, 1.0);
    }

    let mut iterations = 0usize;
    let phase_one = run_simplex(&mut tableau, &mut basis, width, m, &mut iterations)?;
    if phase_one == LpStatus::IterationLimit {
        return Ok(LpSolution {
            status: LpStatus::IterationLimit,
            x: extract(&tableau, &basis, n, m),
            objective: 0.0,
            iterations,
        });
    }
    if -tableau.get(m, width) > 1e-7 {
        return Ok(LpSolution {
            status: LpStatus::Infeasible,
            x: vec![0.0; n],
            objective: 0.0,
            iterations,
        });
    }

    // Phase two: restate the real objective in terms of the current basis.
    for c in 0..=width {
        tableau.set(m, c, 0.0);
    }
    for (c, coefficient) in program.objective.iter().enumerate() {
        tableau.set(m, c, *coefficient);
    }
    // Artificials are barred from re-entering the basis.
    for r in 0..m {
        tableau.set(m, total_structural + r, f64::INFINITY);
    }
    for (r, &basic) in basis.iter().enumerate() {
        let coefficient = tableau.get(m, basic);
        if !coefficient.is_finite() || coefficient == 0.0 {
            continue;
        }
        for c in 0..=width {
            let value = tableau.get(m, c) - coefficient * tableau.get(r, c);
            tableau.set(m, c, value);
        }
    }

    let status = run_simplex(&mut tableau, &mut basis, width, m, &mut iterations)?;
    let x = extract(&tableau, &basis, n, m);
    let objective = crate::vector::dot(&program.objective, &x);
    Ok(LpSolution {
        status,
        x,
        objective,
        iterations,
    })
}

/// Primal simplex iterations on a prepared tableau.
fn run_simplex(
    tableau: &mut Matrix,
    basis: &mut [usize],
    width: usize,
    m: usize,
    iterations: &mut usize,
) -> Result<LpStatus> {
    const MAX_ITERATIONS: usize = 20_000;
    loop {
        if *iterations >= MAX_ITERATIONS {
            return Ok(LpStatus::IterationLimit);
        }
        *iterations += 1;

        // Bland's rule: lowest index with a negative reduced cost.
        let mut entering = None;
        for c in 0..width {
            let reduced = tableau.get(m, c);
            if reduced.is_finite() && reduced < -1e-9 {
                entering = Some(c);
                break;
            }
        }
        let Some(entering) = entering else {
            return Ok(LpStatus::Optimal);
        };

        // Ratio test, breaking ties on the lowest basis index (also Bland).
        let mut leaving = None;
        let mut best_ratio = f64::INFINITY;
        for r in 0..m {
            let pivot = tableau.get(r, entering);
            if pivot <= 1e-11 {
                continue;
            }
            let ratio = tableau.get(r, width) / pivot;
            let better = ratio < best_ratio - 1e-12
                || (ratio <= best_ratio + 1e-12
                    && leaving.is_some_and(|l: usize| basis[r] < basis[l]));
            if better {
                best_ratio = ratio;
                leaving = Some(r);
            }
        }
        let Some(leaving) = leaving else {
            return Ok(LpStatus::Unbounded);
        };

        // Pivot.
        let pivot_value = tableau.get(leaving, entering);
        for c in 0..=width {
            tableau.set(leaving, c, tableau.get(leaving, c) / pivot_value);
        }
        for r in 0..=m {
            if r == leaving {
                continue;
            }
            let factor = tableau.get(r, entering);
            if factor == 0.0 || !factor.is_finite() {
                continue;
            }
            for c in 0..=width {
                let value = tableau.get(r, c) - factor * tableau.get(leaving, c);
                tableau.set(r, c, value);
            }
        }
        basis[leaving] = entering;
    }
}

fn extract(tableau: &Matrix, basis: &[usize], n: usize, m: usize) -> Vec<f64> {
    let width = tableau.cols() - 1;
    let mut x = vec![0.0; n];
    for (r, &basic) in basis.iter().enumerate().take(m) {
        if basic < n {
            x[basic] = tableau.get(r, width).max(0.0);
        }
    }
    x
}
