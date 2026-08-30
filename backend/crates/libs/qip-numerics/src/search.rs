//! Scalar root finding and derivative-free minimisation.
//!
//! Used for implied volatility, yield-to-maturity, break-even analysis and for
//! calibrating the small parameter sets in the detection models.

use qip_core::error::{Error, Result};

/// Bisection on a sign-changing bracket. Always converges; never overshoots.
pub fn bisect<F: Fn(f64) -> f64>(f: F, mut lo: f64, mut hi: f64, tol: f64) -> Result<f64> {
    let mut f_lo = f(lo);
    let f_hi = f(hi);
    if !f_lo.is_finite() || !f_hi.is_finite() {
        return Err(Error::numeric("bracket endpoints are not finite"));
    }
    if f_lo == 0.0 {
        return Ok(lo);
    }
    if f_hi == 0.0 {
        return Ok(hi);
    }
    if f_lo * f_hi > 0.0 {
        return Err(Error::numeric(format!(
            "bracket does not straddle a root: f({lo})={f_lo}, f({hi})={f_hi}"
        )));
    }
    for _ in 0..500 {
        let mid = 0.5 * (lo + hi);
        let f_mid = f(mid);
        if (hi - lo).abs() < tol || f_mid == 0.0 {
            return Ok(mid);
        }
        if f_lo * f_mid < 0.0 {
            hi = mid;
        } else {
            lo = mid;
            f_lo = f_mid;
        }
    }
    Ok(0.5 * (lo + hi))
}

/// Newton-Raphson with a numerical derivative and a bisection safety net.
///
/// Falls back to the bracket whenever a Newton step leaves it — implied
/// volatility solves diverge for deep out-of-the-money options otherwise.
pub fn newton_bracketed<F: Fn(f64) -> f64>(
    f: F,
    guess: f64,
    lo: f64,
    hi: f64,
    tol: f64,
) -> Result<f64> {
    let mut x = guess.clamp(lo, hi);
    let mut low = lo;
    let mut high = hi;
    for _ in 0..200 {
        let fx = f(x);
        if fx.abs() < tol {
            return Ok(x);
        }
        if fx > 0.0 {
            high = x
        } else {
            low = x
        }

        let step = (1e-7 * x.abs()).max(1e-9);
        let derivative = (f(x + step) - f(x - step)) / (2.0 * step);
        let next = if derivative.abs() > 1e-12 {
            x - fx / derivative
        } else {
            f64::NAN
        };
        x = if next.is_finite() && next > low && next < high {
            next
        } else {
            0.5 * (low + high)
        };
    }
    Ok(x)
}

/// Golden-section minimisation of a unimodal function.
pub fn golden_section<F: Fn(f64) -> f64>(f: F, mut lo: f64, mut hi: f64, tol: f64) -> f64 {
    const INV_PHI: f64 = 0.618_033_988_749_895;
    let mut c = hi - INV_PHI * (hi - lo);
    let mut d = lo + INV_PHI * (hi - lo);
    let mut fc = f(c);
    let mut fd = f(d);
    for _ in 0..500 {
        if (hi - lo).abs() < tol {
            break;
        }
        if fc < fd {
            hi = d;
            d = c;
            fd = fc;
            c = hi - INV_PHI * (hi - lo);
            fc = f(c);
        } else {
            lo = c;
            c = d;
            fc = fd;
            d = lo + INV_PHI * (hi - lo);
            fd = f(d);
        }
    }
    0.5 * (lo + hi)
}

/// Nelder-Mead simplex minimisation for small parameter vectors.
///
/// Derivative-free by design: the objectives it is pointed at (backtest score,
/// detector calibration) are not differentiable.
pub fn nelder_mead<F: Fn(&[f64]) -> f64>(
    f: F,
    start: &[f64],
    step: f64,
    max_iterations: usize,
    tol: f64,
) -> (Vec<f64>, f64) {
    let n = start.len();
    if n == 0 {
        return (Vec::new(), f64::INFINITY);
    }

    let mut simplex: Vec<Vec<f64>> = Vec::with_capacity(n + 1);
    simplex.push(start.to_vec());
    for i in 0..n {
        let mut point = start.to_vec();
        point[i] += if point[i].abs() > 1e-12 {
            point[i] * step
        } else {
            step
        };
        simplex.push(point);
    }
    let mut values: Vec<f64> = simplex.iter().map(|p| f(p)).collect();

    for _ in 0..max_iterations {
        let mut order: Vec<usize> = (0..=n).collect();
        order.sort_by(|a, b| {
            values[*a]
                .partial_cmp(&values[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        simplex = order.iter().map(|i| simplex[*i].clone()).collect();
        values = order.iter().map(|i| values[*i]).collect();

        if (values[n] - values[0]).abs() < tol {
            break;
        }

        // Centroid of everything but the worst vertex.
        let mut centroid = vec![0.0; n];
        for point in simplex.iter().take(n) {
            for (c, v) in centroid.iter_mut().zip(point) {
                *c += v / n as f64;
            }
        }

        let reflected: Vec<f64> = (0..n)
            .map(|i| centroid[i] + (centroid[i] - simplex[n][i]))
            .collect();
        let f_reflected = f(&reflected);

        if f_reflected < values[0] {
            let expanded: Vec<f64> = (0..n)
                .map(|i| centroid[i] + 2.0 * (centroid[i] - simplex[n][i]))
                .collect();
            let f_expanded = f(&expanded);
            if f_expanded < f_reflected {
                simplex[n] = expanded;
                values[n] = f_expanded;
            } else {
                simplex[n] = reflected;
                values[n] = f_reflected;
            }
        } else if f_reflected < values[n - 1] {
            simplex[n] = reflected;
            values[n] = f_reflected;
        } else {
            let contracted: Vec<f64> = (0..n)
                .map(|i| centroid[i] + 0.5 * (simplex[n][i] - centroid[i]))
                .collect();
            let f_contracted = f(&contracted);
            if f_contracted < values[n] {
                simplex[n] = contracted;
                values[n] = f_contracted;
            } else {
                // Shrink the whole simplex toward the best vertex.
                let (best, rest) = simplex.split_at_mut(1);
                for (offset, vertex) in rest.iter_mut().enumerate() {
                    for (coordinate, anchor) in vertex.iter_mut().zip(&best[0]) {
                        *coordinate = anchor + 0.5 * (*coordinate - anchor);
                    }
                    values[offset + 1] = f(vertex);
                }
            }
        }
    }

    let best = values
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map_or(0, |(i, _)| i);
    (simplex[best].clone(), values[best])
}
