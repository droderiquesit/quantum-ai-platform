//! Dense vector helpers.
//!
//! Free functions over `&[f64]` rather than a wrapper type: the callers are
//! quant code that already holds `Vec<f64>`, and an extra newtype would only
//! add conversions.

/// Convenience alias used where a signature reads better with a name.
pub type Vector = Vec<f64>;

/// Inner product. Returns 0.0 for mismatched lengths' overlap only, so callers
/// must check lengths first where a mismatch is a bug.
pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Euclidean norm.
pub fn norm2(a: &[f64]) -> f64 {
    dot(a, a).sqrt()
}

/// Sum of absolute values.
pub fn norm1(a: &[f64]) -> f64 {
    a.iter().map(|x| x.abs()).sum()
}

/// Largest absolute value.
pub fn norm_inf(a: &[f64]) -> f64 {
    a.iter().fold(0.0_f64, |m, x| m.max(x.abs()))
}

pub fn sum(a: &[f64]) -> f64 {
    a.iter().sum()
}

/// `y += alpha * x`, in place.
pub fn axpy(alpha: f64, x: &[f64], y: &mut [f64]) {
    for (yi, xi) in y.iter_mut().zip(x) {
        *yi += alpha * xi;
    }
}

pub fn scale(a: &[f64], factor: f64) -> Vec<f64> {
    a.iter().map(|x| x * factor).collect()
}

pub fn add(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

pub fn sub(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b).map(|(x, y)| x - y).collect()
}

/// Element-wise product.
pub fn hadamard(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b).map(|(x, y)| x * y).collect()
}

/// Clamp each element into `[lo, hi]`.
pub fn clamp(a: &[f64], lo: f64, hi: f64) -> Vec<f64> {
    a.iter().map(|x| x.clamp(lo, hi)).collect()
}

/// Rescale so the elements sum to `target`. Returns `None` when the current sum
/// is zero, since no scaling can reach a non-zero target.
pub fn normalise_sum(a: &[f64], target: f64) -> Option<Vec<f64>> {
    let s: f64 = a.iter().sum();
    if s.abs() < 1e-15 {
        return None;
    }
    Some(a.iter().map(|x| x * target / s).collect())
}

/// Project onto the simplex `{x : sum(x) = total, x >= 0}` in Euclidean norm.
///
/// The standard sort-and-threshold algorithm (Duchi et al.). Used to enforce
/// long-only, fully-invested portfolio constraints inside iterative optimisers.
pub fn project_simplex(v: &[f64], total: f64) -> Vec<f64> {
    let n = v.len();
    if n == 0 || total <= 0.0 {
        return vec![0.0; n];
    }
    let mut sorted: Vec<f64> = v.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    let mut cumulative = 0.0;
    let mut theta = 0.0;
    let mut rho = 0usize;
    for (i, &value) in sorted.iter().enumerate() {
        cumulative += value;
        let candidate = (cumulative - total) / (i + 1) as f64;
        if value - candidate > 0.0 {
            rho = i + 1;
            theta = candidate;
        }
    }
    if rho == 0 {
        // Every coordinate is dominated; fall back to an equal split.
        return vec![total / n as f64; n];
    }
    v.iter().map(|x| (x - theta).max(0.0)).collect()
}

/// Project onto a box `[lo_i, hi_i]` intersected with `sum(x) = total`.
///
/// Bisects the Lagrange multiplier on the equality constraint. Returns `None`
/// when the box cannot reach `total` at all — an infeasible constraint set the
/// caller must report rather than silently approximate.
pub fn project_box_sum(v: &[f64], lo: &[f64], hi: &[f64], total: f64) -> Option<Vec<f64>> {
    let n = v.len();
    if n == 0 {
        return Some(Vec::new());
    }
    let min_reachable: f64 = lo.iter().sum();
    let max_reachable: f64 = hi.iter().sum();
    if total < min_reachable - 1e-9 || total > max_reachable + 1e-9 {
        return None;
    }

    let clamp_at = |tau: f64| -> f64 {
        v.iter()
            .zip(lo)
            .zip(hi)
            .map(|((x, l), h)| (x - tau).clamp(*l, *h))
            .sum::<f64>()
    };

    // The clamped sum decreases monotonically in tau, so bisection converges.
    let mut low = v.iter().cloned().fold(f64::INFINITY, f64::min) - total.abs() - 1.0;
    let mut high = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max) + total.abs() + 1.0;
    for _ in 0..200 {
        let mid = 0.5 * (low + high);
        if clamp_at(mid) > total {
            low = mid;
        } else {
            high = mid;
        }
    }
    let tau = 0.5 * (low + high);
    Some(
        v.iter()
            .zip(lo)
            .zip(hi)
            .map(|((x, l), h)| (x - tau).clamp(*l, *h))
            .collect(),
    )
}

/// Index of the largest element, or `None` when empty or all-NaN.
pub fn argmax(a: &[f64]) -> Option<usize> {
    a.iter()
        .enumerate()
        .filter(|(_, x)| !x.is_nan())
        .max_by(|(_, x), (_, y)| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
}

/// Index of the smallest element, or `None` when empty or all-NaN.
pub fn argmin(a: &[f64]) -> Option<usize> {
    a.iter()
        .enumerate()
        .filter(|(_, x)| !x.is_nan())
        .min_by(|(_, x), (_, y)| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
}

/// True when every element is finite.
pub fn all_finite(a: &[f64]) -> bool {
    a.iter().all(|x| x.is_finite())
}
