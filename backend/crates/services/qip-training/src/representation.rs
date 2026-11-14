//! A learned microstructure representation between replayed books and
//! distillation (R12-TICK-1009).
//!
//! Raw touch features went from the replay straight into a fit: nothing
//! learned what the book's own structure looks like first. [`MicroEmbedding`]
//! is that stage. It standardises the features of [`MicroExample`]s built from
//! replayed books and keeps the leading principal directions of their
//! covariance, so a reflex-model student can be distilled over a short, decorrelated
//! vector rather than over columns that move together.
//!
//! Fitting is deterministic: a fixed start vector and a fixed iteration count,
//! with no clock and no randomness, so the same replayed examples always give
//! the same embedding. Invalid input is refused rather than repaired: a
//! constant column has no scale to standardise by, and silently giving it one
//! would hand every later stage a number nobody measured.

use crate::microstructure::MicroExample;
use qip_core::error::{Error, Result};

/// Fixed power-iteration count; a bound, not a tolerance, so two runs agree.
const ITERATIONS: usize = 500;

/// Standardisation plus the leading principal directions of the replayed
/// feature covariance.
#[derive(Clone, Debug, PartialEq)]
pub struct MicroEmbedding {
    mean: Vec<f64>,
    scale: Vec<f64>,
    components: Vec<Vec<f64>>,
    variance: Vec<f64>,
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

impl MicroEmbedding {
    /// Fit `dims` components on the features of `examples`.
    pub fn fit(examples: &[MicroExample], dims: usize) -> Result<Self> {
        if examples.len() < 2 {
            return Err(Error::invalid(
                "a representation needs at least two examples; supply replayed examples",
            ));
        }
        let width = examples[0].features.len();
        if width == 0 {
            return Err(Error::invalid("examples carry no features to represent"));
        }
        if dims == 0 || dims > width {
            return Err(Error::invalid(format!(
                "dims must be between 1 and the feature width {width}, got {dims}"
            )));
        }
        for example in examples {
            if example.features.len() != width {
                return Err(Error::invalid(
                    "examples disagree on feature width; build them with one plan",
                ));
            }
            if example.features.iter().any(|f| !f.is_finite()) {
                return Err(Error::invalid(
                    "a feature is not finite; refuse the example upstream",
                ));
            }
        }
        let n = examples.len() as f64;
        let mut mean = vec![0.0; width];
        for example in examples {
            for (m, f) in mean.iter_mut().zip(&example.features) {
                *m += f / n;
            }
        }
        let mut scale = vec![0.0; width];
        for example in examples {
            for (i, f) in example.features.iter().enumerate() {
                scale[i] += (f - mean[i]) * (f - mean[i]) / n;
            }
        }
        for (i, s) in scale.iter_mut().enumerate() {
            *s = s.sqrt();
            if *s <= 0.0 || !s.is_finite() {
                return Err(Error::invalid(format!(
                    "feature column {i} is constant and has no scale; drop it before fitting"
                )));
            }
        }
        let mut cov = vec![vec![0.0; width]; width];
        for example in examples {
            let z: Vec<f64> = example
                .features
                .iter()
                .enumerate()
                .map(|(i, f)| (f - mean[i]) / scale[i])
                .collect();
            for i in 0..width {
                for j in 0..width {
                    cov[i][j] += z[i] * z[j] / n;
                }
            }
        }
        let mut components: Vec<Vec<f64>> = Vec::new();
        let mut variance = Vec::new();
        for _ in 0..dims {
            let mut v: Vec<f64> = (0..width).map(|i| 1.0 + 0.1 * i as f64).collect();
            for _ in 0..ITERATIONS {
                let mut w: Vec<f64> = cov.iter().map(|row| dot(row, &v)).collect();
                for c in &components {
                    let p = dot(&w, c);
                    for (wi, ci) in w.iter_mut().zip(c) {
                        *wi -= p * ci;
                    }
                }
                let norm = dot(&w, &w).sqrt();
                if norm <= 1e-12 || !norm.is_finite() {
                    return Err(Error::numeric(
                        "the features have no variance left for another component; lower dims",
                    ));
                }
                v = w.iter().map(|x| x / norm).collect();
            }
            let lead = v
                .iter()
                .copied()
                .fold(0.0_f64, |a, x| if x.abs() > a.abs() { x } else { a });
            if lead < 0.0 {
                for x in &mut v {
                    *x = -*x;
                }
            }
            let cv: Vec<f64> = cov.iter().map(|row| dot(row, &v)).collect();
            variance.push(dot(&v, &cv));
            components.push(v);
        }
        Ok(Self {
            mean,
            scale,
            components,
            variance,
        })
    }

    /// Number of output dimensions.
    pub fn dims(&self) -> usize {
        self.components.len()
    }

    /// Feature width the embedding was fitted on.
    pub fn width(&self) -> usize {
        self.mean.len()
    }

    /// Variance each component explains, in component order.
    pub fn explained_variance(&self) -> &[f64] {
        &self.variance
    }

    /// Project one feature vector. Refuses a wrong width or a non-finite value.
    pub fn embed(&self, features: &[f64]) -> Result<Vec<f64>> {
        if features.len() != self.width() {
            return Err(Error::invalid(format!(
                "expected {} features, got {}",
                self.width(),
                features.len()
            )));
        }
        if features.iter().any(|f| !f.is_finite()) {
            return Err(Error::invalid("a feature is not finite"));
        }
        let z: Vec<f64> = features
            .iter()
            .enumerate()
            .map(|(i, f)| (f - self.mean[i]) / self.scale[i])
            .collect();
        Ok(self.components.iter().map(|c| dot(c, &z)).collect())
    }
}
