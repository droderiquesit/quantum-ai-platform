//! Solvers: quadratic programming, linear programming, annealing, search,
//! interpolation. These back portfolio construction, so a wrong answer here is
//! a wrong trade.

// Exact float comparison is deliberate here: these assertions cover degenerate
// cases and identities where the function must return an exact sentinel value,
// not merely something close to it.
#![allow(clippy::float_cmp)]

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::approx_eq;
use qip_numerics::anneal::{self, AnnealSettings, Qubo};
use qip_numerics::interpolate::{Curve, Method};
use qip_numerics::lp::{self, LinearProgram, LpStatus, Sense};
use qip_numerics::matrix::Matrix;
use qip_numerics::qp::{self, LinearConstraint, QpSettings, QuadraticProgram};
use qip_numerics::search;
use qip_numerics::vector;

fn covariance() -> Matrix {
    Matrix::from_rows(&[
        vec![0.0400, 0.0180, 0.0024],
        vec![0.0180, 0.0225, 0.0030],
        vec![0.0024, 0.0030, 0.0100],
    ])
    .unwrap()
}

// --- quadratic programming --------------------------------------------------

#[test]
fn unconstrained_qp_matches_the_analytic_solution() {
    // min 1/2 x'Qx + c'x has the stationary point x = -Q^-1 c.
    let q = covariance();
    let c = vec![-0.02, -0.015, -0.008];
    let expected = q.inverse().unwrap().mul_vec(&c).unwrap();
    let expected: Vec<f64> = expected.iter().map(|v| -v).collect();

    let problem = QuadraticProgram::new(q, c).unwrap();
    let solution = qp::solve_default(&problem).unwrap();
    assert!(solution.converged, "did not converge: {solution:?}");
    for (got, want) in solution.x.iter().zip(&expected) {
        assert!(
            approx_eq(*got, *want, 1e-6),
            "{:?} vs {expected:?}",
            solution.x
        );
    }
}

#[test]
fn minimum_variance_portfolio_matches_the_closed_form() {
    // With only a budget constraint, argmin x'Sx s.t. 1'x = 1 is
    // S^-1 1 / (1' S^-1 1).
    let sigma = covariance();
    let ones = vec![1.0; 3];
    let s_inv_one = sigma.inverse().unwrap().mul_vec(&ones).unwrap();
    let denominator: f64 = s_inv_one.iter().sum();
    let expected: Vec<f64> = s_inv_one.iter().map(|v| v / denominator).collect();

    let problem = QuadraticProgram::new(sigma, vec![0.0; 3])
        .unwrap()
        .with_budget(1.0)
        .unwrap();
    let solution = qp::solve_default(&problem).unwrap();

    assert!(solution.converged, "{solution:?}");
    let sum: f64 = solution.x.iter().sum();
    assert!(approx_eq(sum, 1.0, 1e-7), "budget violated: {sum}");
    for (got, want) in solution.x.iter().zip(&expected) {
        assert!(
            approx_eq(*got, *want, 1e-5),
            "got {:?}, want {expected:?}",
            solution.x
        );
    }
}

#[test]
fn box_bounds_are_respected_and_bind() {
    let sigma = covariance();
    let problem = QuadraticProgram::new(sigma, vec![0.0; 3])
        .unwrap()
        .with_bounds(vec![0.0; 3], vec![0.4, 0.4, 0.4])
        .unwrap()
        .with_budget(1.0)
        .unwrap();
    let solution = qp::solve_default(&problem).unwrap();

    let sum: f64 = solution.x.iter().sum();
    assert!(approx_eq(sum, 1.0, 1e-6), "budget {sum}");
    for (i, w) in solution.x.iter().enumerate() {
        assert!(*w >= -1e-9, "weight {i} negative: {w}");
        assert!(*w <= 0.4 + 1e-7, "weight {i} above cap: {w}");
    }
    // The cap must actually bind: unconstrained this portfolio puts >40% in
    // the low-variance third asset.
    assert!(
        solution.x[2] > 0.39,
        "cap should bind, got {:?}",
        solution.x
    );
}

#[test]
fn inequality_constraints_are_enforced() {
    let sigma = covariance();
    // Force at least 50% into the first two assets.
    let problem = QuadraticProgram::new(sigma, vec![0.0; 3])
        .unwrap()
        .with_bounds(vec![0.0; 3], vec![1.0; 3])
        .unwrap()
        .with_budget(1.0)
        .unwrap()
        .with_constraint(LinearConstraint::at_least(
            vec![1.0, 1.0, 0.0],
            0.5,
            "sector floor",
        ))
        .unwrap();
    let solution = qp::solve_default(&problem).unwrap();
    let sector = solution.x[0] + solution.x[1];
    assert!(sector >= 0.5 - 1e-6, "sector floor violated: {sector}");
    let (violation, label) = problem.worst_violation(&solution.x);
    assert!(violation < 1e-5, "violation {violation} on {label}");
}

#[test]
fn infeasible_constraints_are_reported_not_approximated() {
    let sigma = covariance();
    // Budget of 1.0 with every weight capped at 0.2 across three assets.
    let problem = QuadraticProgram::new(sigma, vec![0.0; 3])
        .unwrap()
        .with_bounds(vec![0.0; 3], vec![0.2; 3])
        .unwrap()
        .with_budget(1.0)
        .unwrap();
    let solution = qp::solve(&problem, &QpSettings::default()).unwrap();
    let (violation, _) = problem.worst_violation(&solution.x);
    assert!(
        solution.infeasible || violation > 1e-3,
        "an unreachable budget must surface as infeasible, got {solution:?}"
    );
}

#[test]
fn bound_inversion_is_rejected_at_construction() {
    let problem = QuadraticProgram::new(covariance(), vec![0.0; 3]).unwrap();
    assert!(problem.with_bounds(vec![0.5; 3], vec![0.1; 3]).is_err());
}

#[test]
fn constraint_width_mismatch_is_rejected() {
    let problem = QuadraticProgram::new(covariance(), vec![0.0; 3]).unwrap();
    assert!(
        problem
            .with_constraint(LinearConstraint::at_most(vec![1.0, 1.0], 1.0, "short"))
            .is_err()
    );
}

#[test]
fn projected_gradient_agrees_with_admm() {
    let sigma = covariance();
    let c = vec![-0.01, -0.008, -0.004];
    let lower = vec![0.0; 3];
    let upper = vec![0.6; 3];

    let admm = qp::solve_default(
        &QuadraticProgram::new(sigma.clone(), c.clone())
            .unwrap()
            .with_bounds(lower.clone(), upper.clone())
            .unwrap()
            .with_budget(1.0)
            .unwrap(),
    )
    .unwrap();
    let projected = qp::solve_box_budget(&sigma, &c, &lower, &upper, 1.0, 20_000).unwrap();

    for (a, b) in admm.x.iter().zip(&projected.x) {
        assert!(approx_eq(*a, *b, 1e-4), "{:?} vs {:?}", admm.x, projected.x);
    }
    assert!(approx_eq(admm.objective, projected.objective, 1e-7));
}

#[test]
fn qp_handles_the_empty_problem() {
    let problem = QuadraticProgram::new(Matrix::zeros(0, 0), Vec::new()).unwrap();
    let solution = qp::solve_default(&problem).unwrap();
    assert!(solution.converged && solution.x.is_empty());
}

#[test]
fn qp_rejects_non_finite_input() {
    let mut q = covariance();
    q.set(0, 0, f64::NAN);
    let problem = QuadraticProgram::new(q, vec![0.0; 3]).unwrap();
    assert!(qp::solve_default(&problem).is_err());
}

// --- linear programming -----------------------------------------------------

#[test]
fn lp_solves_a_known_program() {
    // max 3x + 5y  s.t. x <= 4, 2y <= 12, 3x + 2y <= 18  -> (2, 6), value 36.
    // Expressed as a minimisation of the negated objective.
    let program = LinearProgram::minimise(vec![-3.0, -5.0])
        .subject_to(vec![1.0, 0.0], Sense::LessOrEqual, 4.0)
        .unwrap()
        .subject_to(vec![0.0, 2.0], Sense::LessOrEqual, 12.0)
        .unwrap()
        .subject_to(vec![3.0, 2.0], Sense::LessOrEqual, 18.0)
        .unwrap();
    let solution = lp::solve(&program).unwrap();
    assert_eq!(solution.status, LpStatus::Optimal, "{solution:?}");
    assert!(approx_eq(solution.x[0], 2.0, 1e-7), "{:?}", solution.x);
    assert!(approx_eq(solution.x[1], 6.0, 1e-7), "{:?}", solution.x);
    assert!(approx_eq(solution.objective, -36.0, 1e-7));
}

#[test]
fn lp_handles_equalities_and_lower_bounds() {
    // min x + y s.t. x + y = 10, x >= 3  -> objective 10.
    let program = LinearProgram::minimise(vec![1.0, 1.0])
        .subject_to(vec![1.0, 1.0], Sense::Equal, 10.0)
        .unwrap()
        .subject_to(vec![1.0, 0.0], Sense::GreaterOrEqual, 3.0)
        .unwrap();
    let solution = lp::solve(&program).unwrap();
    assert_eq!(solution.status, LpStatus::Optimal, "{solution:?}");
    assert!(approx_eq(solution.objective, 10.0, 1e-7));
    assert!(solution.x[0] >= 3.0 - 1e-7, "{:?}", solution.x);
}

#[test]
fn lp_detects_infeasibility() {
    // x >= 5 and x <= 2 cannot both hold.
    let program = LinearProgram::minimise(vec![1.0])
        .subject_to(vec![1.0], Sense::GreaterOrEqual, 5.0)
        .unwrap()
        .subject_to(vec![1.0], Sense::LessOrEqual, 2.0)
        .unwrap();
    assert_eq!(lp::solve(&program).unwrap().status, LpStatus::Infeasible);
}

#[test]
fn lp_detects_unboundedness() {
    // min -x with only x >= 1 is unbounded below.
    let program = LinearProgram::minimise(vec![-1.0])
        .subject_to(vec![1.0], Sense::GreaterOrEqual, 1.0)
        .unwrap();
    assert_eq!(lp::solve(&program).unwrap().status, LpStatus::Unbounded);
}

#[test]
fn lp_handles_a_negative_right_hand_side() {
    // -x <= -4 is x >= 4; the solver must normalise the sign.
    let program = LinearProgram::minimise(vec![1.0])
        .subject_to(vec![-1.0], Sense::LessOrEqual, -4.0)
        .unwrap();
    let solution = lp::solve(&program).unwrap();
    assert_eq!(solution.status, LpStatus::Optimal, "{solution:?}");
    assert!(approx_eq(solution.x[0], 4.0, 1e-7), "{:?}", solution.x);
}

#[test]
fn lp_rejects_a_mismatched_constraint_width() {
    let program = LinearProgram::minimise(vec![1.0, 1.0]);
    assert!(program.subject_to(vec![1.0], Sense::Equal, 1.0).is_err());
}

// --- annealing / QUBO -------------------------------------------------------

#[test]
fn annealing_finds_the_exact_optimum_on_small_instances() {
    let mut rng = Xoshiro256::seeded(17);
    for instance in 0..25 {
        let n = 6 + (instance % 4);
        let mut qubo = Qubo::new(n);
        for i in 0..n {
            qubo.add_linear(i, rng.uniform(-2.0, 2.0));
            for j in (i + 1)..n {
                qubo.add(i, j, rng.uniform(-1.5, 1.5));
            }
        }
        let exact = anneal::solve_exact(&qubo).expect("small instance");
        let annealed = anneal::anneal(&qubo, &AnnealSettings::default(), &mut rng);
        assert!(
            annealed.energy <= exact.energy + 1e-9,
            "instance {instance}: annealed {} vs exact {}",
            annealed.energy,
            exact.energy
        );
        assert!(approx_eq(
            annealed.energy,
            qubo.evaluate(&annealed.assignment),
            1e-9
        ));
    }
}

#[test]
fn qubo_delta_matches_a_full_re_evaluation() {
    let mut rng = Xoshiro256::seeded(23);
    let n = 8;
    let mut qubo = Qubo::new(n);
    for i in 0..n {
        qubo.add_linear(i, rng.uniform(-1.0, 1.0));
        for j in (i + 1)..n {
            qubo.add(i, j, rng.uniform(-1.0, 1.0));
        }
    }
    let mut x: Vec<u8> = (0..n).map(|_| u8::from(rng.bernoulli(0.5))).collect();
    for k in 0..n {
        let before = qubo.evaluate(&x);
        let delta = qubo.delta(&x, k);
        x[k] ^= 1;
        let after = qubo.evaluate(&x);
        assert!(
            approx_eq(after - before, delta, 1e-12),
            "bit {k}: {} vs {delta}",
            after - before
        );
        x[k] ^= 1;
    }
}

#[test]
fn ising_conversion_preserves_energy() {
    let mut rng = Xoshiro256::seeded(31);
    let n = 7;
    let mut qubo = Qubo::new(n);
    for i in 0..n {
        qubo.add_linear(i, rng.uniform(-1.0, 1.0));
        for j in (i + 1)..n {
            qubo.add(i, j, rng.uniform(-1.0, 1.0));
        }
    }
    let (h, j_terms, constant) = qubo.to_ising();

    for _ in 0..50 {
        let x: Vec<u8> = (0..n).map(|_| u8::from(rng.bernoulli(0.5))).collect();
        let spins: Vec<f64> = x.iter().map(|b| if *b == 1 { 1.0 } else { -1.0 }).collect();
        let mut ising_energy = constant;
        for (i, hi) in h.iter().enumerate() {
            ising_energy += hi * spins[i];
        }
        for &(i, j, w) in &j_terms {
            ising_energy += w * spins[i] * spins[j];
        }
        assert!(
            approx_eq(ising_energy, qubo.evaluate(&x), 1e-10),
            "ising {ising_energy} vs qubo {}",
            qubo.evaluate(&x)
        );
    }
}

#[test]
fn exact_solver_declines_oversized_problems() {
    assert!(anneal::solve_exact(&Qubo::new(30)).is_none());
    assert!(anneal::solve_exact(&Qubo::new(4)).is_some());
}

// --- scalar search ----------------------------------------------------------

#[test]
fn bisection_finds_a_root_and_rejects_a_bad_bracket() {
    let root = search::bisect(|x| x * x - 2.0, 0.0, 2.0, 1e-12).unwrap();
    assert!(approx_eq(root, std::f64::consts::SQRT_2, 1e-10), "{root}");
    assert!(search::bisect(|x| x * x + 1.0, 0.0, 2.0, 1e-12).is_err());
}

#[test]
fn bracketed_newton_converges_where_plain_newton_would_diverge() {
    // A function with a near-flat region that sends a raw Newton step to infinity.
    let f = |x: f64| (x - 3.0).powi(3);
    let root = search::newton_bracketed(f, 100.0, -10.0, 200.0, 1e-10).unwrap();
    assert!(approx_eq(root, 3.0, 1e-3), "{root}");
}

#[test]
fn golden_section_minimises_a_unimodal_function() {
    let x = search::golden_section(|x| (x - 1.7) * (x - 1.7) + 3.0, -10.0, 10.0, 1e-10);
    assert!(approx_eq(x, 1.7, 1e-6), "{x}");
}

#[test]
fn nelder_mead_minimises_rosenbrock() {
    let rosenbrock = |v: &[f64]| {
        let (x, y) = (v[0], v[1]);
        (1.0 - x).powi(2) + 100.0 * (y - x * x).powi(2)
    };
    let (point, value) = search::nelder_mead(rosenbrock, &[-1.2, 1.0], 0.5, 20_000, 1e-14);
    assert!(value < 1e-8, "value {value} at {point:?}");
    assert!(
        approx_eq(point[0], 1.0, 1e-3) && approx_eq(point[1], 1.0, 1e-3),
        "{point:?}"
    );
}

// --- interpolation ----------------------------------------------------------

#[test]
fn every_method_passes_through_its_knots() {
    let xs = vec![0.25, 1.0, 2.0, 5.0, 10.0, 30.0];
    let ys = vec![0.0525, 0.0490, 0.0445, 0.0420, 0.0435, 0.0460];
    for method in [Method::Linear, Method::MonotoneCubic, Method::NaturalCubic] {
        let curve = Curve::new(xs.clone(), ys.clone(), method).unwrap();
        for (x, y) in xs.iter().zip(&ys) {
            assert!(
                approx_eq(curve.value_at(*x), *y, 1e-12),
                "{method:?} at {x}"
            );
        }
    }
}

#[test]
fn linear_interpolation_hits_the_midpoint() {
    let curve = Curve::linear(vec![0.0, 2.0], vec![10.0, 20.0]).unwrap();
    assert!(approx_eq(curve.value_at(1.0), 15.0, 1e-12));
}

#[test]
fn monotone_cubic_never_overshoots_a_step() {
    // A step in the data is exactly where a natural spline invents an
    // arbitrage; the monotone interpolant must not.
    let xs = vec![0.0, 1.0, 2.0, 3.0];
    let ys = vec![0.0, 0.0, 1.0, 1.0];
    let monotone = Curve::new(xs.clone(), ys.clone(), Method::MonotoneCubic).unwrap();
    for i in 0..=300 {
        let x = i as f64 * 0.01;
        let v = monotone.value_at(x);
        assert!((-1e-12..=1.0 + 1e-12).contains(&v), "overshoot at {x}: {v}");
    }
    let natural = Curve::new(xs, ys, Method::NaturalCubic).unwrap();
    let overshoots = (0..=300).any(|i| {
        let v = natural.value_at(i as f64 * 0.01);
        !(-1e-9..=1.0 + 1e-9).contains(&v)
    });
    assert!(overshoots, "natural spline is expected to overshoot here");
}

#[test]
fn extrapolation_is_flat() {
    let curve = Curve::monotone(vec![1.0, 2.0, 3.0], vec![0.05, 0.04, 0.045]).unwrap();
    assert!(approx_eq(curve.value_at(-100.0), 0.05, 1e-12));
    assert!(approx_eq(curve.value_at(100.0), 0.045, 1e-12));
}

#[test]
fn curve_construction_validates_its_inputs() {
    assert!(
        Curve::linear(vec![1.0, 1.0], vec![1.0, 2.0]).is_err(),
        "duplicate knots"
    );
    assert!(
        Curve::linear(vec![2.0, 1.0], vec![1.0, 2.0]).is_err(),
        "unsorted knots"
    );
    assert!(
        Curve::linear(vec![1.0], vec![1.0, 2.0]).is_err(),
        "length mismatch"
    );
    assert!(Curve::linear(Vec::new(), Vec::new()).is_err(), "empty");
    assert!(
        Curve::linear(vec![1.0, 2.0], vec![f64::NAN, 2.0]).is_err(),
        "non-finite"
    );
}

#[test]
fn integral_and_derivative_behave_on_a_known_line() {
    // y = 2x on [0, 4]: integral is 16, derivative is 2.
    let curve = Curve::linear(vec![0.0, 4.0], vec![0.0, 8.0]).unwrap();
    assert!(
        approx_eq(curve.integral(0.0, 4.0), 16.0, 1e-9),
        "{}",
        curve.integral(0.0, 4.0)
    );
    assert!(approx_eq(curve.derivative_at(2.0), 2.0, 1e-6));
    assert_eq!(curve.integral(4.0, 0.0), 0.0, "reversed range yields zero");
}

#[test]
fn single_point_curve_is_constant() {
    let curve = Curve::linear(vec![1.0], vec![0.05]).unwrap();
    assert!(approx_eq(curve.value_at(-5.0), 0.05, 1e-15));
    assert!(approx_eq(curve.value_at(50.0), 0.05, 1e-15));
}

#[test]
fn vector_helpers_used_by_the_solvers_are_sane() {
    assert_eq!(vector::argmax(&[1.0, 9.0, 3.0]), Some(1));
    assert_eq!(vector::argmin(&[1.0, 9.0, 3.0]), Some(0));
    assert_eq!(vector::argmax(&[]), None);
    assert!(!vector::all_finite(&[1.0, f64::NAN]));
    assert_eq!(
        vector::normalise_sum(&[1.0, 3.0], 1.0),
        Some(vec![0.25, 0.75])
    );
    assert_eq!(vector::normalise_sum(&[0.0, 0.0], 1.0), None);
}
