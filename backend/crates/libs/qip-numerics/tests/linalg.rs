//! Matrix decompositions and their invariants.

// Exact float comparison is deliberate here: these assertions cover degenerate
// cases and identities where the function must return an exact sentinel value,
// not merely something close to it.
#![allow(clippy::float_cmp)]

use qip_core::testing::{Property, approx_eq};
use qip_numerics::matrix::Matrix;
use qip_numerics::vector;

fn sample_covariance() -> Matrix {
    // A realistic 4-asset covariance: correlated equities, a bond, a commodity.
    Matrix::from_rows(&[
        vec![0.0400, 0.0240, -0.0040, 0.0080],
        vec![0.0240, 0.0360, -0.0030, 0.0060],
        vec![-0.0040, -0.0030, 0.0025, -0.0005],
        vec![0.0080, 0.0060, -0.0005, 0.0900],
    ])
    .unwrap()
}

#[test]
fn identity_is_multiplicative_unit() {
    let a = sample_covariance();
    let i = Matrix::identity(4);
    assert_eq!(a.matmul(&i).unwrap(), a);
    assert_eq!(i.matmul(&a).unwrap(), a);
}

#[test]
fn transpose_is_an_involution() {
    let a = sample_covariance();
    assert_eq!(a.transpose().transpose(), a);
}

#[test]
fn matmul_rejects_shape_mismatch() {
    let a = Matrix::zeros(2, 3);
    let b = Matrix::zeros(2, 3);
    assert!(a.matmul(&b).is_err());
    assert!(a.mul_vec(&[1.0, 2.0]).is_err());
}

#[test]
fn solve_recovers_the_right_hand_side() {
    let a = sample_covariance();
    let b = vec![0.01, 0.02, 0.003, 0.05];
    let x = a.solve(&b).unwrap();
    let recovered = a.mul_vec(&x).unwrap();
    for (got, want) in recovered.iter().zip(&b) {
        assert!(approx_eq(*got, *want, 1e-10), "{got} != {want}");
    }
}

#[test]
fn solve_needs_pivoting_when_the_first_pivot_is_zero() {
    // Without partial pivoting this matrix divides by zero on the first column.
    let a = Matrix::from_rows(&[vec![0.0, 1.0], vec![1.0, 0.0]]).unwrap();
    let x = a.solve(&[2.0, 3.0]).unwrap();
    assert!(
        approx_eq(x[0], 3.0, 1e-12) && approx_eq(x[1], 2.0, 1e-12),
        "{x:?}"
    );
}

#[test]
fn singular_matrices_are_reported_not_silently_wrong() {
    let singular = Matrix::from_rows(&[vec![1.0, 2.0], vec![2.0, 4.0]]).unwrap();
    assert!(singular.solve(&[1.0, 2.0]).is_err());
    assert!(singular.inverse().is_err());
}

#[test]
fn inverse_times_original_is_identity() {
    let a = sample_covariance();
    let inv = a.inverse().unwrap();
    let product = a.matmul(&inv).unwrap();
    for r in 0..4 {
        for c in 0..4 {
            let want = if r == c { 1.0 } else { 0.0 };
            assert!(
                approx_eq(product.get(r, c), want, 1e-9),
                "({r},{c}) = {}",
                product.get(r, c)
            );
        }
    }
}

#[test]
fn cholesky_factor_reconstructs_the_matrix() {
    let a = sample_covariance();
    let l = a.cholesky().unwrap();
    let reconstructed = l.matmul(&l.transpose()).unwrap();
    for r in 0..4 {
        for c in 0..4 {
            assert!(
                approx_eq(reconstructed.get(r, c), a.get(r, c), 1e-12),
                "({r},{c}): {} vs {}",
                reconstructed.get(r, c),
                a.get(r, c)
            );
        }
    }
    // Lower triangular.
    for r in 0..4 {
        for c in (r + 1)..4 {
            assert_eq!(l.get(r, c), 0.0);
        }
    }
}

#[test]
fn cholesky_refuses_an_indefinite_matrix() {
    let indefinite = Matrix::from_rows(&[vec![1.0, 2.0], vec![2.0, 1.0]]).unwrap();
    let err = indefinite.cholesky().unwrap_err();
    assert!(err.to_string().contains("positive definite"), "{err}");
}

#[test]
fn eigen_decomposition_reconstructs_and_is_orthogonal() {
    let a = sample_covariance();
    let eigen = a.symmetric_eigen().unwrap();

    // Descending order.
    for w in eigen.values.windows(2) {
        assert!(w[0] >= w[1], "eigenvalues not sorted: {:?}", eigen.values);
    }
    // V D V' == A.
    let d = Matrix::diagonal(&eigen.values);
    let reconstructed = eigen
        .vectors
        .matmul(&d)
        .unwrap()
        .matmul(&eigen.vectors.transpose())
        .unwrap();
    for r in 0..4 {
        for c in 0..4 {
            assert!(
                approx_eq(reconstructed.get(r, c), a.get(r, c), 1e-9),
                "({r},{c}): {} vs {}",
                reconstructed.get(r, c),
                a.get(r, c)
            );
        }
    }
    // V'V == I.
    let vtv = eigen.vectors.transpose().matmul(&eigen.vectors).unwrap();
    for r in 0..4 {
        for c in 0..4 {
            let want = if r == c { 1.0 } else { 0.0 };
            assert!(
                approx_eq(vtv.get(r, c), want, 1e-9),
                "V'V({r},{c}) = {}",
                vtv.get(r, c)
            );
        }
    }
}

#[test]
fn eigenvalues_match_a_known_diagonal() {
    let a = Matrix::diagonal(&[3.0, 1.0, 2.0]);
    let eigen = a.symmetric_eigen().unwrap();
    assert!(approx_eq(eigen.values[0], 3.0, 1e-12));
    assert!(approx_eq(eigen.values[1], 2.0, 1e-12));
    assert!(approx_eq(eigen.values[2], 1.0, 1e-12));
}

#[test]
fn eigen_satisfies_the_defining_equation() {
    let a = sample_covariance();
    let eigen = a.symmetric_eigen().unwrap();
    for j in 0..4 {
        let v = eigen.vectors.column(j);
        let av = a.mul_vec(&v).unwrap();
        for i in 0..4 {
            assert!(
                approx_eq(av[i], eigen.values[j] * v[i], 1e-9),
                "A v != lambda v for component {i} of eigenvector {j}"
            );
        }
    }
}

#[test]
fn nearest_psd_repairs_an_indefinite_covariance() {
    // A correlation matrix that cannot exist: three assets all -0.9 correlated.
    let broken = Matrix::from_rows(&[
        vec![1.0, -0.9, -0.9],
        vec![-0.9, 1.0, -0.9],
        vec![-0.9, -0.9, 1.0],
    ])
    .unwrap();
    assert!(
        !broken.is_positive_semidefinite(1e-10),
        "input should be indefinite"
    );

    let (repaired, smallest) = broken.nearest_psd(1e-8).unwrap();
    assert!(
        smallest < 0.0,
        "should report the negative eigenvalue, got {smallest}"
    );
    assert!(repaired.is_positive_semidefinite(1e-9), "repair failed");
    assert!(repaired.is_symmetric(1e-12));
    // Repair should stay close to the input.
    for r in 0..3 {
        for c in 0..3 {
            assert!((repaired.get(r, c) - broken.get(r, c)).abs() < 0.5);
        }
    }
}

#[test]
fn condition_number_flags_near_singularity() {
    let well = Matrix::identity(3);
    assert!(well.condition_number() < 1.5);

    let ill = Matrix::diagonal(&[1.0, 1e-12, 1.0]);
    assert!(
        ill.condition_number() > 1e10,
        "got {}",
        ill.condition_number()
    );
}

#[test]
fn explained_variance_sums_to_one() {
    let eigen = sample_covariance().symmetric_eigen().unwrap();
    let ratios = eigen.explained_variance_ratio();
    let total: f64 = ratios.iter().sum();
    assert!(approx_eq(total, 1.0, 1e-12), "ratios sum to {total}");
    assert!(eigen.components_for(0.99) <= 4);
    assert!(eigen.components_for(0.0) >= 1);
}

// --- vector helpers ---------------------------------------------------------

#[test]
fn simplex_projection_lands_on_the_simplex() {
    Property::new("simplex projection").cases(300).for_all(
        |r| {
            use qip_core::rng::Rng;
            let n = 2 + r.below(8) as usize;
            (0..n).map(|_| r.uniform(-3.0, 3.0)).collect::<Vec<f64>>()
        },
        |v| {
            let p = vector::project_simplex(v, 1.0);
            let sum: f64 = p.iter().sum();
            if !approx_eq(sum, 1.0, 1e-9) {
                return Err(format!("sum = {sum}"));
            }
            if p.iter().any(|x| *x < -1e-12) {
                return Err(format!("negative weight in {p:?}"));
            }
            Ok(())
        },
    );
}

#[test]
fn simplex_projection_preserves_an_already_valid_point() {
    let already = vec![0.25, 0.25, 0.5];
    let projected = vector::project_simplex(&already, 1.0);
    for (a, b) in projected.iter().zip(&already) {
        assert!(approx_eq(*a, *b, 1e-12), "{projected:?}");
    }
}

#[test]
fn box_sum_projection_respects_bounds_and_budget() {
    let v = vec![0.9, 0.05, 0.05, -0.4];
    let lo = vec![0.0; 4];
    let hi = vec![0.4; 4];
    let p = vector::project_box_sum(&v, &lo, &hi, 1.0).unwrap();
    let sum: f64 = p.iter().sum();
    assert!(approx_eq(sum, 1.0, 1e-8), "sum = {sum}");
    for (i, x) in p.iter().enumerate() {
        assert!(*x >= -1e-9 && *x <= 0.4 + 1e-9, "weight {i} = {x}");
    }
}

#[test]
fn box_sum_projection_reports_infeasibility() {
    // Four weights capped at 0.2 cannot sum to 1.0.
    let infeasible = vector::project_box_sum(&[0.25; 4], &[0.0; 4], &[0.2; 4], 1.0);
    assert!(infeasible.is_none());
}

#[test]
fn norms_agree_with_definitions() {
    let v = vec![3.0, -4.0];
    assert!(approx_eq(vector::norm2(&v), 5.0, 1e-12));
    assert!(approx_eq(vector::norm1(&v), 7.0, 1e-12));
    assert!(approx_eq(vector::norm_inf(&v), 4.0, 1e-12));
    assert!(approx_eq(vector::dot(&v, &v), 25.0, 1e-12));
}
