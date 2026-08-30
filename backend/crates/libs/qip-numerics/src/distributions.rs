//! Probability distributions.
//!
//! Closed-form or high-accuracy rational approximations only — no table
//! lookups, no sampling — so every call is deterministic and cheap enough for
//! the real-time risk path.

use std::f64::consts::PI;

/// Standard normal probability density.
pub fn normal_pdf(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * PI).sqrt()
}

/// Standard normal cumulative distribution.
///
/// Split at zero and expressed through the incomplete gamma function so the
/// far left tail keeps full relative precision. `0.5 * (1 + erf(x))` loses that
/// to cancellation exactly where 99.9% VaR is evaluated.
pub fn normal_cdf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    let half_x2 = 0.5 * x * x;
    if x >= 0.0 {
        0.5 * (1.0 + lower_regularised_gamma(0.5, half_x2))
    } else {
        0.5 * upper_regularised_gamma(0.5, half_x2)
    }
}

/// Inverse standard normal CDF (Acklam's rational approximation, refined by one
/// Halley step to near machine precision).
pub fn normal_inv_cdf(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }

    const A: [f64; 6] = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383_577_518_672_69e2,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ];
    const P_LOW: f64 = 0.02425;

    let x = if p < P_LOW {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - P_LOW {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };

    // One Halley refinement removes the ~1e-9 residual of the raw approximation.
    let e = normal_cdf(x) - p;
    let u = e * (2.0 * PI).sqrt() * (x * x / 2.0).exp();
    x - u / (1.0 + x * u / 2.0)
}

/// Error function, as the regularised lower incomplete gamma `P(1/2, x^2)`.
pub fn erf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    let magnitude = lower_regularised_gamma(0.5, x * x);
    if x < 0.0 { -magnitude } else { magnitude }
}

/// Complementary error function, accurate in the tail.
pub fn erfc(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x >= 0.0 {
        upper_regularised_gamma(0.5, x * x)
    } else {
        1.0 + lower_regularised_gamma(0.5, x * x)
    }
}

/// Student-t density with `nu` degrees of freedom.
pub fn student_t_pdf(x: f64, nu: f64) -> f64 {
    let a = ln_gamma((nu + 1.0) / 2.0) - ln_gamma(nu / 2.0);
    let b = -0.5 * (nu * PI).ln();
    let c = -((nu + 1.0) / 2.0) * (1.0 + x * x / nu).ln();
    (a + b + c).exp()
}

/// Student-t CDF via the regularised incomplete beta function.
pub fn student_t_cdf(x: f64, nu: f64) -> f64 {
    if nu <= 0.0 {
        return f64::NAN;
    }
    let t = nu / (nu + x * x);
    let ib = 0.5 * regularised_incomplete_beta(nu / 2.0, 0.5, t);
    if x > 0.0 { 1.0 - ib } else { ib }
}

/// Inverse Student-t CDF by bisection on [`student_t_cdf`].
///
/// Bisection rather than a closed form: the t quantile is used for confidence
/// intervals and fat-tailed VaR, where robustness beats speed.
pub fn student_t_inv_cdf(p: f64, nu: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    let mut lo = -1e3;
    let mut hi = 1e3;
    for _ in 0..300 {
        let mid = 0.5 * (lo + hi);
        if student_t_cdf(mid, nu) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Chi-square CDF with `k` degrees of freedom.
pub fn chi_square_cdf(x: f64, k: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    lower_regularised_gamma(k / 2.0, x / 2.0)
}

/// Log of the gamma function (Lanczos approximation, g = 7, n = 9).
pub fn ln_gamma(x: f64) -> f64 {
    // Published Lanczos coefficients, transcribed at full precision. Regrouping
    // the digits to satisfy a lint would obscure the comparison against source.
    #[allow(clippy::inconsistent_digit_grouping)]
    const G: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // Reflection formula keeps accuracy for small and negative arguments.
        (PI / (PI * x).sin()).ln() - ln_gamma(1.0 - x)
    } else {
        let x = x - 1.0;
        let mut a = G[0];
        let t = x + 7.5;
        for (i, g) in G.iter().enumerate().skip(1) {
            a += g / (x + i as f64);
        }
        0.5 * (2.0 * PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
    }
}

/// Regularised lower incomplete gamma `P(a, x)`.
pub fn lower_regularised_gamma(a: f64, x: f64) -> f64 {
    if x < 0.0 || a <= 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return 0.0;
    }
    if x < a + 1.0 {
        // Series expansion converges fastest here.
        let mut sum = 1.0 / a;
        let mut term = sum;
        for n in 1..1000 {
            term *= x / (a + n as f64);
            sum += term;
            if term.abs() < sum.abs() * 1e-16 {
                break;
            }
        }
        sum * (-x + a * x.ln() - ln_gamma(a)).exp()
    } else {
        1.0 - upper_regularised_gamma_cf(a, x)
    }
}

/// Regularised upper incomplete gamma `Q(a, x) = 1 - P(a, x)`.
///
/// Evaluated directly by continued fraction in the tail, where computing it as
/// `1 - P` would cancel away the significant digits.
pub fn upper_regularised_gamma(a: f64, x: f64) -> f64 {
    if x < 0.0 || a <= 0.0 {
        return f64::NAN;
    }
    if x < a + 1.0 {
        1.0 - lower_regularised_gamma(a, x)
    } else {
        upper_regularised_gamma_cf(a, x)
    }
}

/// Continued-fraction evaluation of `Q(a, x)`, valid for `x >= a + 1`.
fn upper_regularised_gamma_cf(a: f64, x: f64) -> f64 {
    let mut b = x + 1.0 - a;
    let mut c = 1e300;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..1000 {
        let an = -(i as f64) * (i as f64 - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < 1e-300 {
            d = 1e-300;
        }
        c = b + an / c;
        if c.abs() < 1e-300 {
            c = 1e-300;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < 1e-16 {
            break;
        }
    }
    (-x + a * x.ln() - ln_gamma(a)).exp() * h
}

/// Regularised incomplete beta `I_x(a, b)`.
pub fn regularised_incomplete_beta(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let front =
        (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    // The continued fraction converges only on one side of the symmetry point.
    if x < (a + 1.0) / (a + b + 2.0) {
        front * beta_continued_fraction(a, b, x) / a
    } else {
        1.0 - front * beta_continued_fraction(b, a, 1.0 - x) / b
    }
}

fn beta_continued_fraction(a: f64, b: f64, x: f64) -> f64 {
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < 1e-300 {
        d = 1e-300;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..300 {
        let m_f = m as f64;
        let m2 = 2.0 * m_f;

        let aa = m_f * (b - m_f) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < 1e-300 {
            d = 1e-300;
        }
        c = 1.0 + aa / c;
        if c.abs() < 1e-300 {
            c = 1e-300;
        }
        d = 1.0 / d;
        h *= d * c;

        let aa = -(a + m_f) * (qab + m_f) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < 1e-300 {
            d = 1e-300;
        }
        c = 1.0 + aa / c;
        if c.abs() < 1e-300 {
            c = 1e-300;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < 1e-16 {
            break;
        }
    }
    h
}
