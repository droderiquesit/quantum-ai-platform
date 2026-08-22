//! Dense row-major matrices and the decompositions the risk and optimisation
//! stacks depend on.
//!
//! Covariance matrices estimated from real data are routinely indefinite
//! (short samples, missing observations, stale prices). Rather than let that
//! surface as a mysterious NaN inside an optimiser, [`Matrix::nearest_psd`]
//! repairs them explicitly and the caller records that a repair happened.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Row-major dense matrix of `f64`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Matrix {
    rows: usize,
    cols: usize,
    data: Vec<f64>,
}

impl Matrix {
    pub fn zeros(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            data: vec![0.0; rows * cols],
        }
    }

    pub fn identity(n: usize) -> Self {
        let mut m = Self::zeros(n, n);
        for i in 0..n {
            m.set(i, i, 1.0);
        }
        m
    }

    /// Build from row-major data. Errors when the length does not match.
    pub fn from_vec(rows: usize, cols: usize, data: Vec<f64>) -> Result<Self> {
        if data.len() != rows * cols {
            return Err(Error::invalid(format!(
                "matrix data length {} does not match {rows}x{cols}",
                data.len()
            )));
        }
        Ok(Self { rows, cols, data })
    }

    /// Build from a slice of rows.
    pub fn from_rows(rows: &[Vec<f64>]) -> Result<Self> {
        let nrows = rows.len();
        let ncols = rows.first().map_or(0, |r| r.len());
        if rows.iter().any(|r| r.len() != ncols) {
            return Err(Error::invalid("ragged rows"));
        }
        Ok(Self {
            rows: nrows,
            cols: ncols,
            data: rows.concat(),
        })
    }

    /// Diagonal matrix from a vector.
    pub fn diagonal(values: &[f64]) -> Self {
        let n = values.len();
        let mut m = Self::zeros(n, n);
        for (i, v) in values.iter().enumerate() {
            m.set(i, i, *v);
        }
        m
    }

    pub fn rows(&self) -> usize {
        self.rows
    }
    pub fn cols(&self) -> usize {
        self.cols
    }
    pub fn is_square(&self) -> bool {
        self.rows == self.cols
    }
    pub fn as_slice(&self) -> &[f64] {
        &self.data
    }

    #[inline]
    pub fn get(&self, r: usize, c: usize) -> f64 {
        self.data[r * self.cols + c]
    }

    #[inline]
    pub fn set(&mut self, r: usize, c: usize, v: f64) {
        self.data[r * self.cols + c] = v;
    }

    #[inline]
    pub fn add_to(&mut self, r: usize, c: usize, v: f64) {
        self.data[r * self.cols + c] += v;
    }

    pub fn row(&self, r: usize) -> &[f64] {
        &self.data[r * self.cols..(r + 1) * self.cols]
    }

    pub fn column(&self, c: usize) -> Vec<f64> {
        (0..self.rows).map(|r| self.get(r, c)).collect()
    }

    pub fn diagonal_values(&self) -> Vec<f64> {
        (0..self.rows.min(self.cols))
            .map(|i| self.get(i, i))
            .collect()
    }

    pub fn transpose(&self) -> Self {
        let mut out = Self::zeros(self.cols, self.rows);
        for r in 0..self.rows {
            for c in 0..self.cols {
                out.set(c, r, self.get(r, c));
            }
        }
        out
    }

    pub fn scale(&self, factor: f64) -> Self {
        Self {
            rows: self.rows,
            cols: self.cols,
            data: self.data.iter().map(|x| x * factor).collect(),
        }
    }

    pub fn add(&self, other: &Self) -> Result<Self> {
        self.require_same_shape(other)?;
        Ok(Self {
            rows: self.rows,
            cols: self.cols,
            data: self
                .data
                .iter()
                .zip(&other.data)
                .map(|(a, b)| a + b)
                .collect(),
        })
    }

    pub fn sub(&self, other: &Self) -> Result<Self> {
        self.require_same_shape(other)?;
        Ok(Self {
            rows: self.rows,
            cols: self.cols,
            data: self
                .data
                .iter()
                .zip(&other.data)
                .map(|(a, b)| a - b)
                .collect(),
        })
    }

    /// Matrix product.
    pub fn matmul(&self, other: &Self) -> Result<Self> {
        if self.cols != other.rows {
            return Err(Error::invalid(format!(
                "cannot multiply {}x{} by {}x{}",
                self.rows, self.cols, other.rows, other.cols
            )));
        }
        let mut out = Self::zeros(self.rows, other.cols);
        for i in 0..self.rows {
            for k in 0..self.cols {
                let a = self.get(i, k);
                if a == 0.0 {
                    continue;
                }
                for j in 0..other.cols {
                    out.add_to(i, j, a * other.get(k, j));
                }
            }
        }
        Ok(out)
    }

    /// Matrix-vector product.
    pub fn mul_vec(&self, x: &[f64]) -> Result<Vec<f64>> {
        if x.len() != self.cols {
            return Err(Error::invalid(format!(
                "vector length {} does not match {} columns",
                x.len(),
                self.cols
            )));
        }
        Ok((0..self.rows)
            .map(|r| crate::vector::dot(self.row(r), x))
            .collect())
    }

    /// Quadratic form `x' A x`.
    pub fn quadratic_form(&self, x: &[f64]) -> Result<f64> {
        let ax = self.mul_vec(x)?;
        Ok(crate::vector::dot(x, &ax))
    }

    pub fn trace(&self) -> f64 {
        self.diagonal_values().iter().sum()
    }

    /// Largest absolute deviation from symmetry.
    pub fn symmetry_error(&self) -> f64 {
        if !self.is_square() {
            return f64::INFINITY;
        }
        let mut worst: f64 = 0.0;
        for r in 0..self.rows {
            for c in (r + 1)..self.cols {
                worst = worst.max((self.get(r, c) - self.get(c, r)).abs());
            }
        }
        worst
    }

    pub fn is_symmetric(&self, tol: f64) -> bool {
        self.symmetry_error() <= tol
    }

    /// Average with the transpose, removing estimation asymmetry.
    pub fn symmetrised(&self) -> Self {
        let mut out = self.clone();
        for r in 0..self.rows {
            for c in 0..self.cols {
                let v = 0.5 * (self.get(r, c) + self.get(c, r));
                out.set(r, c, v);
            }
        }
        out
    }

    pub fn all_finite(&self) -> bool {
        self.data.iter().all(|x| x.is_finite())
    }

    /// Cholesky factor `L` with `A = L L'`.
    ///
    /// Errors when `A` is not positive definite — the caller decides whether to
    /// repair via [`Self::nearest_psd`] or reject the input.
    pub fn cholesky(&self) -> Result<Self> {
        if !self.is_square() {
            return Err(Error::invalid("cholesky requires a square matrix"));
        }
        let n = self.rows;
        let mut l = Self::zeros(n, n);
        for i in 0..n {
            for j in 0..=i {
                let mut sum = self.get(i, j);
                for k in 0..j {
                    sum -= l.get(i, k) * l.get(j, k);
                }
                if i == j {
                    if sum <= 0.0 {
                        return Err(Error::numeric(format!(
                            "matrix is not positive definite at pivot {i} (value {sum:.3e})"
                        )));
                    }
                    l.set(i, j, sum.sqrt());
                } else {
                    let denom = l.get(j, j);
                    if denom == 0.0 {
                        return Err(Error::numeric("zero pivot in cholesky"));
                    }
                    l.set(i, j, sum / denom);
                }
            }
        }
        Ok(l)
    }

    /// Solve `A x = b` by LU with partial pivoting.
    pub fn solve(&self, b: &[f64]) -> Result<Vec<f64>> {
        if !self.is_square() {
            return Err(Error::invalid("solve requires a square matrix"));
        }
        if b.len() != self.rows {
            return Err(Error::invalid("right-hand side length mismatch"));
        }
        let n = self.rows;
        let mut a = self.clone();
        let mut x = b.to_vec();

        for col in 0..n {
            // Partial pivoting keeps the factorisation stable for the
            // ill-conditioned covariance matrices this sees in practice.
            let mut pivot = col;
            let mut best = a.get(col, col).abs();
            for r in (col + 1)..n {
                let candidate = a.get(r, col).abs();
                if candidate > best {
                    best = candidate;
                    pivot = r;
                }
            }
            if best < 1e-14 {
                return Err(Error::numeric(format!(
                    "matrix is singular at column {col}"
                )));
            }
            if pivot != col {
                for c in 0..n {
                    let tmp = a.get(col, c);
                    a.set(col, c, a.get(pivot, c));
                    a.set(pivot, c, tmp);
                }
                x.swap(col, pivot);
            }
            for r in (col + 1)..n {
                let factor = a.get(r, col) / a.get(col, col);
                if factor == 0.0 {
                    continue;
                }
                for c in col..n {
                    a.add_to(r, c, -factor * a.get(col, c));
                }
                x[r] -= factor * x[col];
            }
        }

        for col in (0..n).rev() {
            let mut sum = x[col];
            for (c, xc) in x.iter().enumerate().skip(col + 1) {
                sum -= a.get(col, c) * xc;
            }
            x[col] = sum / a.get(col, col);
        }
        Ok(x)
    }

    /// Matrix inverse. Prefer [`Self::solve`] when only one right-hand side is
    /// needed — it is both faster and better conditioned.
    pub fn inverse(&self) -> Result<Self> {
        if !self.is_square() {
            return Err(Error::invalid("inverse requires a square matrix"));
        }
        let n = self.rows;
        let mut out = Self::zeros(n, n);
        for j in 0..n {
            let mut e = vec![0.0; n];
            e[j] = 1.0;
            let col = self.solve(&e)?;
            for (i, v) in col.into_iter().enumerate() {
                out.set(i, j, v);
            }
        }
        Ok(out)
    }

    /// Eigen-decomposition of a symmetric matrix by the cyclic Jacobi method.
    ///
    /// Returns eigenvalues in descending order together with the matching
    /// eigenvectors as columns. Jacobi is chosen over QR for its accuracy on
    /// small, near-degenerate covariance matrices and for being short enough to
    /// audit.
    pub fn symmetric_eigen(&self) -> Result<Eigen> {
        if !self.is_square() {
            return Err(Error::invalid("eigen requires a square matrix"));
        }
        if !self.all_finite() {
            return Err(Error::numeric("matrix contains non-finite values"));
        }
        let n = self.rows;
        let mut a = self.symmetrised();
        let mut v = Self::identity(n);

        for _sweep in 0..100 {
            let mut off = 0.0;
            for r in 0..n {
                for c in (r + 1)..n {
                    off += a.get(r, c) * a.get(r, c);
                }
            }
            if off.sqrt() < 1e-14 {
                break;
            }
            for p in 0..n {
                for q in (p + 1)..n {
                    let apq = a.get(p, q);
                    if apq.abs() < 1e-18 {
                        continue;
                    }
                    let app = a.get(p, p);
                    let aqq = a.get(q, q);
                    let theta = 0.5 * (aqq - app) / apq;
                    let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                    let c = 1.0 / (t * t + 1.0).sqrt();
                    let s = t * c;

                    for k in 0..n {
                        let akp = a.get(k, p);
                        let akq = a.get(k, q);
                        a.set(k, p, c * akp - s * akq);
                        a.set(k, q, s * akp + c * akq);
                    }
                    for k in 0..n {
                        let apk = a.get(p, k);
                        let aqk = a.get(q, k);
                        a.set(p, k, c * apk - s * aqk);
                        a.set(q, k, s * apk + c * aqk);
                    }
                    for k in 0..n {
                        let vkp = v.get(k, p);
                        let vkq = v.get(k, q);
                        v.set(k, p, c * vkp - s * vkq);
                        v.set(k, q, s * vkp + c * vkq);
                    }
                }
            }
        }

        let mut pairs: Vec<(f64, Vec<f64>)> = (0..n).map(|i| (a.get(i, i), v.column(i))).collect();
        pairs.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));

        let values: Vec<f64> = pairs.iter().map(|(l, _)| *l).collect();
        let mut vectors = Self::zeros(n, n);
        for (j, (_, vec)) in pairs.iter().enumerate() {
            for (i, value) in vec.iter().enumerate() {
                vectors.set(i, j, *value);
            }
        }
        Ok(Eigen { values, vectors })
    }

    /// Nearest positive semi-definite matrix by eigenvalue clipping.
    ///
    /// Returns the repaired matrix and the smallest eigenvalue that had to be
    /// lifted, so callers can log how far from valid the input was instead of
    /// silently proceeding.
    pub fn nearest_psd(&self, floor: f64) -> Result<(Self, f64)> {
        let eigen = self.symmetric_eigen()?;
        let smallest = eigen.values.last().copied().unwrap_or(0.0);
        let clipped: Vec<f64> = eigen.values.iter().map(|l| l.max(floor)).collect();
        let d = Self::diagonal(&clipped);
        let repaired = eigen
            .vectors
            .matmul(&d)?
            .matmul(&eigen.vectors.transpose())?;
        Ok((repaired.symmetrised(), smallest))
    }

    /// True when every eigenvalue is at least `-tol`.
    pub fn is_positive_semidefinite(&self, tol: f64) -> bool {
        self.symmetric_eigen()
            .map(|e| e.values.iter().all(|l| *l >= -tol))
            .unwrap_or(false)
    }

    /// Ratio of largest to smallest eigenvalue magnitude. `INFINITY` when
    /// singular — a signal that downstream results are untrustworthy.
    pub fn condition_number(&self) -> f64 {
        match self.symmetric_eigen() {
            Ok(e) => {
                let max = e.values.iter().fold(0.0_f64, |m, l| m.max(l.abs()));
                let min = e.values.iter().fold(f64::INFINITY, |m, l| m.min(l.abs()));
                if min <= 0.0 { f64::INFINITY } else { max / min }
            }
            Err(_) => f64::INFINITY,
        }
    }

    fn require_same_shape(&self, other: &Self) -> Result<()> {
        if self.rows != other.rows || self.cols != other.cols {
            return Err(Error::invalid(format!(
                "shape mismatch: {}x{} vs {}x{}",
                self.rows, self.cols, other.rows, other.cols
            )));
        }
        Ok(())
    }
}

impl fmt::Debug for Matrix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Matrix {}x{}", self.rows, self.cols)?;
        for r in 0..self.rows.min(8) {
            let row: Vec<String> = (0..self.cols.min(8))
                .map(|c| format!("{:>10.4}", self.get(r, c)))
                .collect();
            let ellipsis = if self.cols > 8 { " ..." } else { "" };
            writeln!(f, "  [{}]{}", row.join(", "), ellipsis)?;
        }
        if self.rows > 8 {
            writeln!(f, "  ...")?;
        }
        Ok(())
    }
}

/// Eigenvalues in descending order with eigenvectors as columns.
#[derive(Clone, Debug)]
pub struct Eigen {
    pub values: Vec<f64>,
    pub vectors: Matrix,
}

impl Eigen {
    /// Fraction of total variance explained by each component.
    pub fn explained_variance_ratio(&self) -> Vec<f64> {
        let total: f64 = self.values.iter().map(|l| l.max(0.0)).sum();
        if total <= 0.0 {
            return vec![0.0; self.values.len()];
        }
        self.values.iter().map(|l| l.max(0.0) / total).collect()
    }

    /// Number of leading components needed to explain `threshold` of variance.
    pub fn components_for(&self, threshold: f64) -> usize {
        let mut cumulative = 0.0;
        for (i, ratio) in self.explained_variance_ratio().iter().enumerate() {
            cumulative += ratio;
            if cumulative >= threshold {
                return i + 1;
            }
        }
        self.values.len()
    }
}
