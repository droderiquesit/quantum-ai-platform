//! Contract tests for the online estimators of §22.2 — the exponentially
//! weighted covariance, recursive least squares, and streaming PCA.
//!
//! Three properties are load-bearing across the whole file and each has its
//! own test rather than being asserted in passing:
//!
//! * **The estimate is the same one a batch computation would give.** An
//!   online estimator that is merely self-consistent is an online estimator
//!   nobody can check. At a forgetting factor of one, the covariance is the
//!   population covariance and the fit is the least squares fit, and both are
//!   compared against this crate's own batch functions.
//! * **It is stable where the batch form is not.**
//!   `an_online_covariance_recovers_a_variance_the_textbook_form_loses`
//!   asserts the textbook `E[x²] − E[x]²` fails on the series *before*
//!   asserting the Welford form does not, because a test that only shows the
//!   good form working does not show that the bad one was the problem.
//! * **Memory does not move with the stream.** A streaming estimator that
//!   grows is the defect and not the feature, and
//!   `no_estimator_here_grows_with_its_stream` is what would catch it.

use qip_numerics::Matrix;
use qip_numerics::online::{EwCovariance, ForgettingFactor, RecursiveLeastSquares, StreamingPca};
use qip_numerics::stats;
use std::collections::{BTreeMap, BTreeSet};

/// A deterministic stream. Nothing here draws from the operating system: a
/// test whose data changes between runs cannot be debugged when it fails once.
struct Stream(u64);

impl Stream {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Irwin–Hall: twelve uniforms less six is a standard normal to well
    /// inside anything asserted here, and needs no transcendental function.
    fn normal(&mut self) -> f64 {
        (0..12).map(|_| self.unit()).sum::<f64>() - 6.0
    }
}

fn names(labels: &[&str]) -> BTreeSet<String> {
    labels.iter().map(|name| (*name).to_string()).collect()
}

fn row(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), *value))
        .collect()
}

fn close(actual: f64, expected: f64, tolerance: f64) -> bool {
    (actual - expected).abs() <= tolerance
}

#[test]
fn an_online_covariance_with_no_forgetting_matches_the_batch_covariance_over_the_same_series() {
    // The anchor for everything else in this file. At a forgetting factor of
    // one the rank-one update is Welford's, so it must agree with the batch
    // covariance this crate already has — to about the precision of the
    // arithmetic, not merely "closely".
    let mut stream = Stream::new(0x1234_5678);
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for _ in 0..400 {
        let x = stream.normal();
        let y = 0.6 * x + 0.8 * stream.normal();
        xs.push(x);
        ys.push(y);
    }

    let batch_sample = stats::covariance(&xs, &ys);
    // The premise: the two series genuinely covary, so an estimator that
    // returned zero would be wrong rather than accidentally right.
    assert!(
        batch_sample.abs() > 0.3,
        "the generated series must covary for this comparison to mean anything, and the batch \
         covariance is {batch_sample}"
    );
    // `stats::covariance` divides by n-1 and the estimator reports the
    // population figure, which is what `S/W` is at a forgetting factor of one.
    let n = xs.len() as f64;
    let expected = batch_sample * (n - 1.0) / n;

    let mut estimator = EwCovariance::new(&names(&["x", "y"]), ForgettingFactor::none())
        .expect("two named dimensions and no forgetting are a valid estimator");
    for (x, y) in xs.iter().zip(&ys) {
        estimator
            .observe(&row(&[("x", *x), ("y", *y)]))
            .expect("a finite observation naming both dimensions is absorbed");
    }

    let online = estimator.covariance("x", "y").expect("both are named");
    assert!(
        close(online, expected, 1e-12),
        "the online covariance is {online} and the batch population covariance is {expected}"
    );
    let online_variance = estimator.variance("x").expect("x is named");
    let expected_variance = stats::variance(&xs) * (n - 1.0) / n;
    assert!(
        close(online_variance, expected_variance, 1e-12),
        "the online variance is {online_variance} and the batch one is {expected_variance}"
    );
    let online_mean = estimator.mean("x").expect("x is named");
    assert!(
        close(online_mean, stats::mean(&xs), 1e-12),
        "the online mean is {online_mean} and the batch one is {}",
        stats::mean(&xs)
    );
}

#[test]
fn an_online_covariance_recovers_a_variance_the_textbook_form_loses() {
    // The reason the update is written the way it is, as a test rather than
    // as a comment. A series of unit dispersion around a mean of a billion is
    // an ordinary thing to meet — an index level, a notional — and
    // `E[x²] − E[x]²` cannot survive it: the two terms agree to eighteen
    // significant digits and an f64 carries about sixteen, so what comes out
    // is rounding noise, frequently negative. A negative variance's square
    // root is NaN and a covariance matrix holding one fails Cholesky, which
    // is how this arrives at the risk engine rather than at a log line.
    const OFFSET: f64 = 1e9;
    let mut stream = Stream::new(0xBEEF_0001);
    let noise: Vec<f64> = (0..2000).map(|_| stream.normal()).collect();
    let n = noise.len() as f64;
    // Computed at unit magnitude, where f64 has all the precision anyone
    // needs, and therefore the figure both forms are judged against.
    let truth = stats::variance(&noise) * (n - 1.0) / n;
    assert!(
        truth > 0.5,
        "the generated noise must have real dispersion for this test to mean anything, and its \
         variance is {truth}"
    );

    let shifted: Vec<f64> = noise.iter().map(|value| OFFSET + value).collect();
    let naive = {
        let mean_of_squares = shifted.iter().map(|v| v * v).sum::<f64>() / n;
        let mean = shifted.iter().sum::<f64>() / n;
        mean_of_squares - mean * mean
    };
    // The premise, asserted before the thing under test: the textbook form
    // really does fail here. Without this the test would pass just as happily
    // on a series where both forms work.
    assert!(
        (naive - truth).abs() > truth,
        "the textbook form was supposed to lose this variance and returned {naive} against a true \
         {truth}; if it now works, this test is no longer testing anything"
    );

    let mut estimator = EwCovariance::new(&names(&["level"]), ForgettingFactor::none())
        .expect("one named dimension is a valid estimator");
    for value in &shifted {
        estimator
            .observe(&row(&[("level", *value)]))
            .expect("a finite observation is absorbed");
    }
    let online = estimator.variance("level").expect("level is named");
    // A part in a million rather than to the last bit, and the reason is the
    // honest limit of the method: the deviation `x − m` is still formed at a
    // magnitude of 1e9, where an f64 ulp is about 2.4e-7, so the recovered
    // variance cannot be better than a part in ten million however the sum is
    // arranged. The finding is that it is right to seven digits where the
    // textbook form has none, not that it is exact.
    assert!(
        close(online, truth, truth * 1e-6),
        "the Welford form was supposed to recover {truth} from a series offset by {OFFSET} and \
         returned {online}"
    );
}

#[test]
fn an_online_covariance_forgets_the_regime_it_has_left() {
    // What distinguishes this from `stats::covariance` over a retained
    // window, and the reason §22.2 asks for the exponentially weighted form:
    // an estimate that keeps everything equally is still mostly about a
    // regime that ended. Both estimators see exactly the same stream.
    let mut stream = Stream::new(0x0C0F_FEE5);
    let mut forgetting = EwCovariance::new(
        &names(&["r"]),
        ForgettingFactor::new(0.99).expect("0.99 is inside (0, 1]"),
    )
    .expect("one named dimension is a valid estimator");
    let mut remembering = EwCovariance::new(&names(&["r"]), ForgettingFactor::none())
        .expect("one named dimension is a valid estimator");

    for index in 0..4000 {
        // Unit variance for the first half, a hundred times that for the
        // second: a volatility regime change, which is the thing a desk needs
        // an estimator to notice rather than to average away.
        let scale = if index < 2000 { 1.0 } else { 10.0 };
        let observation = row(&[("r", scale * stream.normal())]);
        forgetting
            .observe(&observation)
            .expect("a finite observation is absorbed");
        remembering
            .observe(&observation)
            .expect("a finite observation is absorbed");
    }

    let moved = forgetting.variance("r").expect("r is named");
    let averaged = remembering.variance("r").expect("r is named");
    // Bounded on both sides, and the upper bound is not decoration: this
    // assertion read `moved > 70.0` until a mutation that removed the decay
    // from the co-moment accumulator *survived* it. An estimator whose
    // variance grows without bound is as wrong as one that never moves, and a
    // one-sided assertion calls it a pass.
    assert!(
        (70.0..140.0).contains(&moved),
        "an estimator weighting its last ~100 observations should report the current regime's \
         variance of about 100 and reported {moved}"
    );
    // The premise that makes the first assertion a finding: the same stream
    // through an estimator that forgets nothing lands near the average of the
    // two regimes, about 50, and not near the current one.
    assert!(
        averaged < 60.0,
        "an estimator that forgets nothing should still be reporting the average of both regimes, \
         about 50, and reported {averaged}"
    );
    let effective = forgetting.effective_observations();
    assert!(
        close(effective, 199.0, 1.0),
        "Kish's effective sample size at a factor of 0.99 is (1+λ)/(1-λ) = 199 and the estimator \
         reports {effective}"
    );
}

#[test]
fn an_online_covariance_is_symmetric_to_the_bit() {
    // Not a rounding detail. `δᵢδ'ⱼ` and `δⱼδ'ᵢ` are equal in exact
    // arithmetic and need not be in floating point, so the upper triangle is
    // computed and mirrored. A matrix whose off-diagonal pair differs by an
    // ulp replays differently and can fail a positive-definiteness test on a
    // nearly singular book, which is a risk figure that vanishes for a reason
    // nobody can reproduce.
    let mut stream = Stream::new(0xA5A5_1234);
    let mut estimator = EwCovariance::new(
        &names(&["a", "b", "c"]),
        ForgettingFactor::new(0.97).expect("0.97 is inside (0, 1]"),
    )
    .expect("three named dimensions are a valid estimator");
    for _ in 0..500 {
        let a = 1e6 + stream.normal();
        let b = -3e5 + 0.5 * a + stream.normal();
        let c = 0.25 * b - 0.75 * a + stream.normal();
        estimator
            .observe(&row(&[("a", a), ("b", b), ("c", c)]))
            .expect("a finite observation naming every dimension is absorbed");
    }

    let matrix = estimator.matrix().expect("a k×k matrix over k labels");
    for i in 0..3 {
        for j in 0..3 {
            assert_eq!(
                matrix.get(i, j).to_bits(),
                matrix.get(j, i).to_bits(),
                "({i},{j}) is {} and ({j},{i}) is {}",
                matrix.get(i, j),
                matrix.get(j, i)
            );
        }
    }
    assert!(
        matrix.is_positive_semidefinite(1e-9),
        "a covariance matrix that is not positive semi-definite fails Cholesky and every risk \
         figure built on it: {matrix:?}"
    );
}

#[test]
fn an_online_covariance_reports_a_correlation_of_one_for_a_pair_that_moves_together() {
    let mut stream = Stream::new(0x7777);
    let mut estimator = EwCovariance::new(
        &names(&["lead", "follow"]),
        ForgettingFactor::new(0.98).expect("0.98 is inside (0, 1]"),
    )
    .expect("two named dimensions are a valid estimator");
    for _ in 0..300 {
        let lead = stream.normal();
        estimator
            .observe(&row(&[("lead", lead), ("follow", 2.0 * lead + 3.0)]))
            .expect("a finite observation is absorbed");
    }
    let correlation = estimator
        .correlation("lead", "follow")
        .expect("both are named");
    assert!(
        close(correlation, 1.0, 1e-9),
        "an exactly affine pair correlates at one and the estimator reports {correlation}"
    );
    // A dimension that never moved has no correlation with anything, and zero
    // is the answer that keeps a NaN out of the caller's sums.
    let mut flat = EwCovariance::new(&names(&["moves", "still"]), ForgettingFactor::none())
        .expect("two named dimensions are a valid estimator");
    for _ in 0..50 {
        flat.observe(&row(&[("moves", stream.normal()), ("still", 4.0)]))
            .expect("a finite observation is absorbed");
    }
    let undefined = flat.correlation("moves", "still").expect("both are named");
    assert!(
        close(undefined, 0.0, 0.0),
        "a correlation with a constant is undefined and is reported as zero, not {undefined}"
    );
}

#[test]
fn a_forgetting_factor_outside_the_unit_interval_is_refused_rather_than_clamped() {
    for bad in [0.0, -0.5, 1.000_000_1, 2.0, f64::NAN, f64::INFINITY] {
        let refused = ForgettingFactor::new(bad);
        let error = refused.expect_err("a factor outside (0, 1] must be refused");
        assert_eq!(error.code(), "invalid", "for {bad}");
        assert!(
            error.message().contains("(0, 1]"),
            "the refusal must name the interval the caller has to pass instead: {}",
            error.message()
        );
    }
    for good in [1.0, 0.999, 0.94, 0.5, 1e-6] {
        let accepted = ForgettingFactor::new(good).expect("a factor inside (0, 1] is accepted");
        assert!(
            close(accepted.value(), good, 0.0),
            "a factor inside the interval is kept as passed, and {good} became {}",
            accepted.value()
        );
    }
    let window = ForgettingFactor::new(0.99)
        .expect("0.99 is inside (0, 1]")
        .effective_window();
    assert!(
        close(window, 100.0, 1e-9),
        "a factor of 0.99 weights roughly its last hundred observations, not {window}"
    );
    assert!(
        ForgettingFactor::none().effective_window().is_infinite(),
        "a factor of one is about every observation ever made, and saying so as infinity is more \
         honest than a large number"
    );
}

#[test]
fn an_estimator_refuses_an_observation_that_does_not_name_every_dimension() {
    // The refusal that stops a transposed covariance. A frame arriving with a
    // column missing, or with a column the estimator was not built over, is a
    // caller bug; imputing a zero for it would produce a plausible matrix
    // about the wrong instruments.
    let mut estimator = EwCovariance::new(&names(&["a", "b"]), ForgettingFactor::none())
        .expect("two named dimensions are a valid estimator");

    let short = estimator.observe(&row(&[("a", 1.0)]));
    let error = short.expect_err("an observation naming one of two dimensions must be refused");
    assert_eq!(error.code(), "invalid");
    assert!(
        error.message().contains("not a zero to be imputed"),
        "the refusal must say why a gap is not filled in: {}",
        error.message()
    );

    let extra = estimator.observe(&row(&[("a", 1.0), ("b", 2.0), ("c", 3.0)]));
    assert_eq!(
        extra
            .expect_err("an observation naming a dimension the estimator does not hold is refused")
            .code(),
        "invalid"
    );

    let renamed = estimator.observe(&row(&[("a", 1.0), ("z", 2.0)]));
    let error = renamed.expect_err(
        "an observation of the right width naming the wrong dimension \
                                    must still be refused",
    );
    assert!(
        error.message().contains("\"b\""),
        "the refusal must name the dimension that is missing: {}",
        error.message()
    );

    // Nothing was absorbed by any of the three, so a caller that catches the
    // refusal and carries on has an estimator describing only good data.
    assert_eq!(estimator.observations(), 0);

    let unknown = estimator.covariance("a", "q");
    assert_eq!(
        unknown
            .expect_err("a dimension the estimator was not built over is not answered")
            .code(),
        "not_found"
    );
}

#[test]
fn an_estimator_refuses_a_non_finite_observation_and_keeps_what_it_had() {
    // One NaN makes every figure afterwards NaN and the estimator keeps no
    // record of which observation did it, so it is refused at the door rather
    // than absorbed and explained later.
    let mut covariance = EwCovariance::new(&names(&["a"]), ForgettingFactor::none())
        .expect("one named dimension is a valid estimator");
    covariance
        .observe(&row(&[("a", 2.0)]))
        .expect("a finite observation is absorbed");
    covariance
        .observe(&row(&[("a", 4.0)]))
        .expect("a finite observation is absorbed");
    let before = covariance.mean("a").expect("a is named");

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let error = covariance
            .observe(&row(&[("a", bad)]))
            .expect_err("a non-finite observation must be refused");
        assert_eq!(error.code(), "invalid", "for {bad}");
    }
    assert_eq!(
        covariance.observations(),
        2,
        "a refused observation must not be counted"
    );
    let after = covariance.mean("a").expect("a is named");
    assert!(
        close(after, before, 0.0),
        "a refused observation must leave the estimate exactly as it was: {before} became {after}"
    );

    let mut fit = RecursiveLeastSquares::new(&names(&["x"]), ForgettingFactor::none(), 1e4)
        .expect("one named dimension and a positive prior are a valid fit");
    let error = fit
        .observe(&row(&[("x", 1.0)]), f64::NAN)
        .expect_err("a non-finite response must be refused");
    assert_eq!(error.code(), "invalid");
    assert_eq!(fit.observations(), 0);

    let mut pca = StreamingPca::new(&names(&["x", "y"]), 1, ForgettingFactor::none(), 0.05, 7)
        .expect("one component over two dimensions is a valid estimator");
    let error = pca
        .observe(&row(&[("x", 1.0), ("y", f64::INFINITY)]))
        .expect_err("a non-finite observation must be refused");
    assert_eq!(error.code(), "invalid");
    assert_eq!(pca.observations(), 0);
}

#[test]
fn no_estimator_here_grows_with_its_stream() {
    // The property that makes these estimators the answer to §21.1 rather
    // than an archive with extra steps. An estimator whose memory moves with
    // the stream is the defect, not the feature.
    let labels = names(&["a", "b", "c"]);
    let mut covariance = EwCovariance::new(&labels, ForgettingFactor::none())
        .expect("three named dimensions are a valid estimator");
    let mut fit = RecursiveLeastSquares::new(&labels, ForgettingFactor::none(), 1e4)
        .expect("three named dimensions and a positive prior are a valid fit");
    let mut pca = StreamingPca::new(&labels, 2, ForgettingFactor::none(), 0.05, 11)
        .expect("two components over three dimensions are a valid estimator");

    let mut stream = Stream::new(0xDEAD_BEEF);
    let first = row(&[
        ("a", stream.normal()),
        ("b", stream.normal()),
        ("c", stream.normal()),
    ]);
    covariance.observe(&first).expect("absorbed");
    fit.observe(&first, 1.0).expect("absorbed");
    pca.observe(&first).expect("absorbed");
    let (c0, f0, p0) = (covariance.bytes(), fit.bytes(), pca.bytes());

    for _ in 0..100_000 {
        let observation = row(&[
            ("a", stream.normal()),
            ("b", stream.normal()),
            ("c", stream.normal()),
        ]);
        covariance.observe(&observation).expect("absorbed");
        fit.observe(&observation, stream.normal())
            .expect("absorbed");
        pca.observe(&observation).expect("absorbed");
    }

    assert_eq!(covariance.observations(), 100_001);
    assert_eq!(
        (covariance.bytes(), fit.bytes(), pca.bytes()),
        (c0, f0, p0),
        "a hundred thousand observations later every estimator must cost exactly what it cost \
         after one"
    );
    // The covariance's budget is the one §22.2 sizes: O(k²), 200 features at
    // ~320 KB. Three dimensions is nine f64s of matrix and three of mean.
    assert!(
        covariance.bytes() < 1024,
        "three dimensions cost {} bytes, which is not the O(k²) §22.2 budgets",
        covariance.bytes()
    );
}

#[test]
fn recursive_least_squares_agrees_with_ordinary_least_squares_on_the_same_rows() {
    // At a forgetting factor of one, recursive least squares is least
    // squares — the same answer without keeping the rows. Checked against
    // this crate's own batch fit, because an online fit that is only
    // self-consistent is one nobody can check.
    let mut stream = Stream::new(0x1111_2222);
    let mut design_rows = Vec::new();
    let mut responses = Vec::new();
    let mut fit = RecursiveLeastSquares::new(
        &names(&["constant", "x1", "x2"]),
        ForgettingFactor::none(),
        1e8,
    )
    .expect("three named dimensions and a positive prior are a valid fit");

    for _ in 0..400 {
        let x1 = stream.normal();
        let x2 = 0.3 * x1 + stream.normal();
        let y = 0.75 - 1.5 * x1 + 2.25 * x2 + 0.1 * stream.normal();
        design_rows.push(vec![x1, x2]);
        responses.push(y);
        fit.observe(&row(&[("constant", 1.0), ("x1", x1), ("x2", x2)]), y)
            .expect("a finite row and response are absorbed");
    }

    let design = Matrix::from_rows(&design_rows).expect("400 rows of two regressors");
    let batch = stats::ols(&design, &responses).expect("400 rows over two regressors is a fit");
    // The premise: the batch fit recovered the coefficients the data was
    // generated from, so agreeing with it is agreeing with something right.
    assert!(
        close(batch.coefficients[1], -1.5, 0.02) && close(batch.coefficients[2], 2.25, 0.02),
        "the batch fit must recover the generating coefficients for this comparison to mean \
         anything: {:?}",
        batch.coefficients
    );

    let online = fit.parameters();
    let expected = [
        ("constant", batch.coefficients[0]),
        ("x1", batch.coefficients[1]),
        ("x2", batch.coefficients[2]),
    ];
    for (name, value) in expected {
        let held = online
            .get(name)
            .copied()
            .unwrap_or_else(|| panic!("{name} is a named dimension of the fit"));
        assert!(
            close(held, value, 1e-6),
            "the recursive fit has {name} at {held} and the batch fit has it at {value}"
        );
    }
    assert_eq!(fit.observations(), 400);
    let prediction = fit
        .predict(&row(&[("constant", 1.0), ("x1", 1.0), ("x2", 1.0)]))
        .expect("a finite row is predicted");
    assert!(
        close(
            prediction,
            batch.coefficients[0] + batch.coefficients[1] + batch.coefficients[2],
            1e-6
        ),
        "the prediction must be the parameters applied to the row, and it is {prediction}"
    );
}

#[test]
fn recursive_least_squares_tracks_a_coefficient_that_moves_and_a_fit_that_forgets_nothing_does_not()
{
    // Why the forgetting factor is there. Both fits see exactly the same
    // rows; one weights its recent history and finds the relationship that
    // holds now, the other averages two relationships into one that never
    // held.
    let mut stream = Stream::new(0x5150_5150);
    let dimensions = names(&["x"]);
    let mut tracking = RecursiveLeastSquares::new(
        &dimensions,
        ForgettingFactor::new(0.98).expect("0.98 is inside (0, 1]"),
        1e4,
    )
    .expect("one named dimension and a positive prior are a valid fit");
    let mut averaging = RecursiveLeastSquares::new(&dimensions, ForgettingFactor::none(), 1e4)
        .expect("one named dimension and a positive prior are a valid fit");

    for index in 0..3000 {
        let slope = if index < 1500 { 3.0 } else { -2.0 };
        let x = stream.normal();
        let y = slope * x + 0.05 * stream.normal();
        let observation = row(&[("x", x)]);
        tracking
            .observe(&observation, y)
            .expect("a finite row and response are absorbed");
        averaging
            .observe(&observation, y)
            .expect("a finite row and response are absorbed");
    }

    let tracked = tracking.parameter("x").expect("x is named");
    assert!(
        close(tracked, -2.0, 0.1),
        "a fit weighting its last ~50 rows should have found the current slope of -2 and has \
         {tracked}"
    );
    // The premise: the same stream through a fit that forgets nothing lands
    // near the average of the two slopes, +0.5, which is a slope that was
    // never true at any point in the stream.
    let averaged = averaging.parameter("x").expect("x is named");
    assert!(
        averaged > 0.0,
        "a fit that forgets nothing should still be reporting an average of the two regimes, \
         about +0.5, and reports {averaged}"
    );
}

#[test]
fn recursive_least_squares_reports_the_error_it_made_before_it_learned_from_the_row() {
    // The a-priori error is the only one that is out-of-sample. A fit
    // reporting the a-posteriori error is reporting how well it fits a point
    // it has just been told the answer to, which always looks good and means
    // nothing — a scorer reading it would never see a model fail.
    let mut fit = RecursiveLeastSquares::new(&names(&["x"]), ForgettingFactor::none(), 1e4)
        .expect("one named dimension and a positive prior are a valid fit");
    let observation = row(&[("x", 2.0)]);

    let first = fit
        .observe(&observation, 10.0)
        .expect("a finite row and response are absorbed");
    assert!(
        close(first, 10.0, 1e-12),
        "the parameters start at zero, so the first error is the whole response, not {first}"
    );
    let second = fit
        .observe(&observation, 10.0)
        .expect("a finite row and response are absorbed");
    assert!(
        second.abs() < 1e-3,
        "having seen the row once, the fit should predict it almost exactly and the second error \
         is {second}"
    );
    assert!(
        second.abs() > 0.0,
        "and not exactly, because the a-posteriori error would be exactly zero here and that is \
         the number this test exists to keep out"
    );
}

#[test]
fn recursive_least_squares_refuses_a_prior_variance_that_could_never_start_a_fit() {
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let error = RecursiveLeastSquares::new(&names(&["x"]), ForgettingFactor::none(), bad)
            .expect_err("a prior variance that is not positive and finite must be refused");
        assert_eq!(error.code(), "invalid", "for {bad}");
        assert!(
            error.message().contains("prior variance"),
            "the refusal must name what to pass instead: {}",
            error.message()
        );
    }
    RecursiveLeastSquares::new(&names(&["x"]), ForgettingFactor::none(), 1e4)
        .expect("a positive finite prior variance is accepted");
}

#[test]
fn streaming_pca_finds_the_direction_a_correlated_pair_actually_varies_along() {
    // Two series that move together vary along the 45° line, and the leading
    // component has to be that line rather than either axis. Sign is not
    // asserted: an eigenvector and its negation are the same direction, and a
    // test that demanded one of them would be asserting a convention.
    let mut stream = Stream::new(0x3141_5926);
    let mut pca = StreamingPca::new(&names(&["x", "y"]), 1, ForgettingFactor::none(), 0.1, 99)
        .expect("one component over two dimensions is a valid estimator");
    for _ in 0..20_000 {
        let common = 3.0 * stream.normal();
        pca.observe(&row(&[
            ("x", common + 0.1 * stream.normal()),
            ("y", common + 0.1 * stream.normal()),
        ]))
        .expect("a finite observation is absorbed");
    }

    let loadings = pca.loadings(0).expect("component 0 exists");
    let root_half = 0.5f64.sqrt();
    assert!(
        close(loadings[0].abs(), root_half, 0.02) && close(loadings[1].abs(), root_half, 0.02),
        "the leading component of a pair that moves together is (±0.707, ±0.707) and it is \
         {loadings:?}"
    );
    assert!(
        loadings[0] * loadings[1] > 0.0,
        "the two loadings must share a sign, because the series move together: {loadings:?}"
    );
    let named = pca.component(0).expect("component 0 exists");
    assert!(
        close(
            *named.get("x").expect("x is a named dimension"),
            loadings[0],
            0.0
        ),
        "the named loadings and the vector must be the same numbers in label order"
    );
    // The variance along that direction is about 2×9 = 18: two series of
    // dispersion 3 moving together project onto the 45° line at √2 times each.
    let carried = pca.explained_variance(0).expect("component 0 exists");
    assert!(
        carried > 12.0,
        "the leading component should carry most of the pair's variance and carries {carried}"
    );
}

#[test]
fn streaming_pca_agrees_with_the_batch_eigenvector_of_the_same_sample() {
    // The anchor, as for the other two estimators: the streaming answer is
    // checked against the batch decomposition this crate already has, over
    // the same observations.
    let mut stream = Stream::new(0x2718_2818);
    let mut rows = Vec::new();
    for _ in 0..30_000 {
        let factor = 5.0 * stream.normal();
        rows.push(vec![
            2.0f64.mul_add(factor, 0.3 * stream.normal()),
            (-1.0f64).mul_add(factor, 0.3 * stream.normal()),
            0.5f64.mul_add(factor, 0.3 * stream.normal()),
        ]);
    }

    let mut pca = StreamingPca::new(
        &names(&["a", "b", "c"]),
        1,
        ForgettingFactor::none(),
        0.1,
        5,
    )
    .expect("one component over three dimensions is a valid estimator");
    for values in &rows {
        pca.observe(&row(&[
            ("a", values[0]),
            ("b", values[1]),
            ("c", values[2]),
        ]))
        .expect("a finite observation is absorbed");
    }

    let observations = Matrix::from_rows(&rows).expect("30,000 rows of three columns");
    let batch = stats::covariance_matrix(&observations).expect("three columns give a 3×3");
    let eigen = batch
        .symmetric_eigen()
        .expect("a covariance matrix decomposes");
    let leading: Vec<f64> = (0..3).map(|i| eigen.vectors.get(i, 0)).collect();
    // The premise: the batch decomposition found a dominant factor, so
    // agreeing with its leading eigenvector is agreeing with something that
    // means anything.
    assert!(
        eigen.values[0] > 10.0 * eigen.values[1],
        "the generated data must have one dominant factor for this comparison to mean anything, \
         and the eigenvalues are {:?}",
        eigen.values
    );

    let streaming = pca.loadings(0).expect("component 0 exists");
    let alignment: f64 = (0..3).map(|i| streaming[i] * leading[i]).sum::<f64>().abs();
    assert!(
        alignment > 0.999,
        "the streaming component {streaming:?} and the batch eigenvector {leading:?} should be \
         the same direction, and their alignment is {alignment}"
    );
}

#[test]
fn streaming_pca_keeps_its_components_apart() {
    // Two components that drift onto each other are one component reported
    // twice, and the second's explained variance is then the first's counted
    // again — a factor model that double-counts the market.
    let mut stream = Stream::new(0x1614_1592);
    let mut pca = StreamingPca::new(
        &names(&["a", "b", "c"]),
        2,
        ForgettingFactor::none(),
        0.1,
        3,
    )
    .expect("two components over three dimensions are a valid estimator");
    for _ in 0..20_000 {
        let first = 4.0 * stream.normal();
        let second = 1.5 * stream.normal();
        pca.observe(&row(&[
            ("a", first + second),
            ("b", first - second),
            ("c", 0.2 * stream.normal()),
        ]))
        .expect("a finite observation is absorbed");
    }

    let leading = pca.loadings(0).expect("component 0 exists").to_vec();
    let second = pca.loadings(1).expect("component 1 exists").to_vec();
    let overlap: f64 = (0..3).map(|i| leading[i] * second[i]).sum::<f64>().abs();
    assert!(
        overlap < 0.01,
        "Sanger's deflation is what keeps the components apart, and these overlap at {overlap}: \
         {leading:?} against {second:?}"
    );
    let first_variance = pca.explained_variance(0).expect("component 0 exists");
    let second_variance = pca.explained_variance(1).expect("component 1 exists");
    assert!(
        first_variance > second_variance,
        "the components come out ordered by the variance they carry, and these are \
         {first_variance} then {second_variance}"
    );
}

#[test]
fn streaming_pca_refuses_more_components_than_the_data_has_dimensions() {
    // The refusal is not only about a meaningless extra component. Mutation
    // showed what it actually stands in front of: `seed_basis` seeds row `r`
    // from the canonical axis `r`, so a fourth component over three
    // dimensions indexes past the end of the weight matrix. Relaxing the
    // ceiling turns a caller's arithmetic error into a panic inside a
    // library, which is why the bound is checked before anything is
    // allocated.
    let three = names(&["a", "b", "c"]);
    for asked in [4, 10, 256] {
        let error = StreamingPca::new(&three, asked, ForgettingFactor::none(), 0.05, 1)
            .expect_err("more components than dimensions must be refused");
        assert_eq!(error.code(), "invalid", "for {asked}");
        assert!(
            error.message().contains("at most 3"),
            "the refusal must name the number the caller can ask for instead: {}",
            error.message()
        );
    }
    let error = StreamingPca::new(&three, 0, ForgettingFactor::none(), 0.05, 1)
        .expect_err("zero components must be refused");
    assert_eq!(error.code(), "invalid");
    // The boundary is admitted, which is the half of a gate that proves it is
    // a gate rather than a refusal of everything.
    let full = StreamingPca::new(&three, 3, ForgettingFactor::none(), 0.05, 1)
        .expect("as many components as dimensions is exactly allowed");
    assert_eq!(full.components(), 3);
    assert_eq!(
        full.component(3)
            .expect_err("a component beyond the ones held is not answered")
            .code(),
        "not_found"
    );
}

#[test]
fn streaming_pca_refuses_a_learning_rate_outside_the_unit_interval() {
    for bad in [0.0, -0.1, 1.5, f64::NAN, f64::INFINITY] {
        let error = StreamingPca::new(&names(&["a", "b"]), 1, ForgettingFactor::none(), bad, 1)
            .expect_err("a learning rate outside (0, 1] must be refused");
        assert_eq!(error.code(), "invalid", "for {bad}");
        assert!(
            error.message().contains("(0, 1]"),
            "the refusal must name the interval: {}",
            error.message()
        );
    }
    StreamingPca::new(&names(&["a", "b"]), 1, ForgettingFactor::none(), 1.0, 1)
        .expect("a rate of exactly one is inside the interval");
}

#[test]
fn an_estimator_over_no_dimensions_or_a_blank_one_is_refused() {
    let empty = BTreeSet::new();
    assert_eq!(
        EwCovariance::new(&empty, ForgettingFactor::none())
            .expect_err("an estimator over nothing is refused")
            .code(),
        "invalid"
    );
    assert_eq!(
        RecursiveLeastSquares::new(&empty, ForgettingFactor::none(), 1.0)
            .expect_err("a fit over nothing is refused")
            .code(),
        "invalid"
    );
    assert_eq!(
        StreamingPca::new(&empty, 1, ForgettingFactor::none(), 0.05, 1)
            .expect_err("a decomposition of nothing is refused")
            .code(),
        "invalid"
    );

    let blank = names(&["a", "   "]);
    let error = EwCovariance::new(&blank, ForgettingFactor::none())
        .expect_err("a dimension nobody can name is refused");
    assert!(
        error.message().contains("blank name"),
        "the refusal must say what is wrong: {}",
        error.message()
    );

    let too_many: BTreeSet<String> = (0..=256).map(|index| format!("f{index:04}")).collect();
    let error = EwCovariance::new(&too_many, ForgettingFactor::none())
        .expect_err("a frame wider than the ceiling is refused at construction");
    assert!(
        error.message().contains("256"),
        "the refusal must name the ceiling: {}",
        error.message()
    );
}
