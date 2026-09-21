//! Estimators maintained per observation, in memory that does not move with
//! the stream (§22.2's third, sixth and seventh rows).
//!
//! §22.2's table names seven sufficient statistics and the streaming method
//! each is to be computed by. Three of them are here:
//!
//! * "Covariance and correlation — exponentially weighted online update —
//!   O(k²)" is [`EwCovariance`].
//! * "Linear and logistic models — recursive least squares, online gradient —
//!   O(k) parameters" is [`RecursiveLeastSquares`].
//! * "Factor structure — streaming PCA, Oja's rule or incremental SVD —
//!   O(k × components)" is [`StreamingPca`].
//!
//! The other four were already in the tree: [`crate::stats::RunningStats`] for
//! moments, [`crate::streaming::TDigest`] for quantiles,
//! [`crate::streaming::HyperLogLog`] and [`crate::sketch::CountMinSketch`] for
//! cardinality and frequency, and [`crate::streaming::Reservoir`] for
//! representative samples.
//!
//! # Why these are not the batch functions with a window bolted on
//!
//! [`crate::stats::covariance`] and [`crate::stats::ols`] both take slices, so
//! a caller that wants either over a moving history has to *keep the history*.
//! That is the archive §21.1 exists to avoid, and the cost is not only memory:
//! a batch covariance recomputed each cycle costs O(n k²) per cycle where
//! these cost O(k²) per observation, and it answers a different question —
//! every point in the window counts the same, so a regime the series left
//! three hundred observations ago is still a third of the estimate until it
//! falls off the end in one step. An exponentially weighted estimate forgets
//! continuously, and [`ForgettingFactor::effective_window`] says how fast in
//! observations rather than in a setting nobody can read.
//!
//! # Why the mean is never subtracted from a raw second moment
//!
//! The textbook covariance `E[xy] − E[x]E[y]` is the one form that must not be
//! used here. On a series centred far from zero — an index level around 5,000,
//! a notional in the millions — the two terms agree to as many significant
//! digits as the ratio of mean to standard deviation, and an `f64` carries
//! about sixteen. A variance of 1 around a mean of 1e9 needs eighteen, so the
//! subtraction returns noise, and the noise is as often negative as positive.
//! A negative variance is not a small error: its square root is `NaN`, and a
//! covariance matrix holding one is not positive semi-definite, so
//! [`crate::Matrix::cholesky`] fails and every risk figure built on it fails
//! with it.
//!
//! Every update here is therefore a Welford-style form — the deviation from
//! the mean is formed first and the products of deviations are accumulated, so
//! no large quantity is ever subtracted from another large quantity.
//! `an_online_covariance_recovers_a_variance_the_textbook_form_loses` is that
//! property as a test: it asserts the textbook form fails on the series before
//! asserting this one does not.
//!
//! # Money
//!
//! Nothing here takes a `Decimal`, and nothing here should. These are
//! statistics, which this platform computes in `f64`; a price or a notional
//! becomes an `f64` in the caller that feeds one in, and that caller is where
//! the crossing point is stated.
//!
//! # Determinism
//!
//! Like the rest of this crate: the same observations in the same order
//! produce identical bits. [`StreamingPca`]'s starting basis is drawn from a
//! seed the caller passes and never from the operating system, so a factor
//! structure recorded in the log can be recomputed rather than merely
//! believed. Every estimator is keyed by name and reads its observations in
//! label order, so a caller whose columns arrive in a different order gets the
//! same estimate rather than a silently transposed one.

use crate::Matrix;
use crate::sketch::splitmix64;
use qip_core::error::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

/// The most dimensions an estimator here may carry.
///
/// §22.2 budgets the covariance row at "O(k²) — 200 features is ~320 KB",
/// which is `200² × 8` bytes to the byte. The ceiling sits just above that at
/// a power of two, so a caller feeding a wider frame is refused at
/// construction rather than quietly spending 512 KB a matrix, and refused
/// where it can still choose a different frame rather than an hour into a run.
pub const MAX_DIMENSION: usize = 256;

/// How much of what an estimator already believes survives one observation.
///
/// One forgets nothing and is the right choice for a series believed
/// stationary; below one, the estimator weights roughly the last
/// [`Self::effective_window`] observations. Shared by all three estimators in
/// this module so that "the forgetting factor" means one thing and is refused
/// in one place.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct ForgettingFactor {
    value: f64,
}

impl ForgettingFactor {
    /// Refuses anything outside `(0, 1]`, and does not clamp.
    ///
    /// Clamping is the tempting thing and the wrong thing: a caller that
    /// passed 1.5 believed the estimator would weight new observations more
    /// heavily than the whole of history, which is not something this
    /// estimator can do, and a silently corrected 1.0 leaves that belief in
    /// the caller where it will be wrong about something else next. Zero is
    /// refused for the same reason — an estimator that keeps nothing of what
    /// it has seen has no covariance to report, only its last observation.
    pub fn new(value: f64) -> Result<Self> {
        if !value.is_finite() || value <= 0.0 || value > 1.0 {
            return Err(Error::invalid(format!(
                "a forgetting factor must lie in (0, 1], not {value}; pass 1.0 for a series you \
                 believe stationary, or 1 - 1/w for one you want weighted over roughly its last \
                 w observations. It is refused rather than clamped: a factor outside the interval \
                 is a caller believing something the estimator cannot do"
            )));
        }
        Ok(Self { value })
    }

    /// No forgetting: every observation weighs the same for ever.
    pub const fn none() -> Self {
        Self { value: 1.0 }
    }

    pub const fn value(&self) -> f64 {
        self.value
    }

    /// Roughly how many recent observations the estimate is about: `1/(1-λ)`.
    ///
    /// Infinite for [`Self::none`], which is the honest answer — a factor of
    /// one is about every observation ever made.
    pub fn effective_window(&self) -> f64 {
        if self.value >= 1.0 {
            f64::INFINITY
        } else {
            1.0 / (1.0 - self.value)
        }
    }
}

/// Labels for an estimator's dimensions, in ascending order, or a refusal
/// naming what is wrong with them.
fn dimensions_of(names: &BTreeSet<String>, estimator: &str) -> Result<Vec<String>> {
    if names.is_empty() {
        return Err(Error::invalid(format!(
            "{estimator} needs at least one named dimension and was given none; name the series \
             it is to be built over"
        )));
    }
    if names.len() > MAX_DIMENSION {
        return Err(Error::invalid(format!(
            "{estimator} was given {} dimension(s) and the ceiling is {MAX_DIMENSION}; §22.2 \
             budgets the covariance row at 200 features, so a wider frame spends several times \
             the memory the blueprint sizes a node for. Summarise the frame, or build one \
             estimator per group",
            names.len()
        )));
    }
    if names.iter().any(|name| name.trim().is_empty()) {
        return Err(Error::invalid(format!(
            "{estimator} was given a dimension with a blank name; a statistic nobody can name is \
             a statistic nobody can read back out of the log"
        )));
    }
    Ok(names.iter().cloned().collect())
}

/// One observation as a row in label order, or a refusal naming the label that
/// is missing or the value that is not a number.
///
/// Keyed by name rather than by position on purpose. A caller whose columns
/// arrive in a different order than the one the estimator was built on would
/// otherwise get a covariance matrix whose rows and columns are permuted,
/// which is symmetric, positive semi-definite, entirely plausible, and about
/// the wrong instruments.
fn row_in_label_order(
    labels: &[String],
    observation: &BTreeMap<String, f64>,
    estimator: &str,
) -> Result<Vec<f64>> {
    if observation.len() != labels.len() {
        return Err(Error::invalid(format!(
            "{estimator} was built over {} named dimension(s) and this observation names {}; an \
             observation must name every dimension exactly once. A series missing from a frame is \
             a gap to be reported, not a zero to be imputed",
            labels.len(),
            observation.len()
        )));
    }
    let mut row = Vec::with_capacity(labels.len());
    for label in labels {
        let Some(value) = observation.get(label) else {
            return Err(Error::invalid(format!(
                "{estimator} was built over {label:?} and this observation does not name it; feed \
                 every dimension the estimator was built on, or build one over the dimensions the \
                 feed actually produces"
            )));
        };
        if !value.is_finite() {
            return Err(Error::invalid(format!(
                "{estimator} was given {value} for {label:?}; one non-finite observation makes \
                 every figure the estimator reports afterwards non-finite and it keeps no record \
                 of which observation did it. Drop the observation at the source, where the gap \
                 can be reported"
            )));
        }
        row.push(*value);
    }
    Ok(row)
}

/// The index into a row-major `k × k` matrix held as a flat vector.
const fn at(k: usize, row: usize, col: usize) -> usize {
    row * k + col
}

/// An exponentially weighted covariance, updated once per observation and
/// never recomputed over history.
///
/// §22.2's "Covariance and correlation — exponentially weighted online update
/// — O(k²)". The state is a mean vector and a co-moment matrix; both are sized
/// at construction and neither grows, so an estimator that has absorbed a
/// million observations costs exactly what it cost after one.
///
/// # The update
///
/// With `λ` the forgetting factor, `W` the accumulated weight and `m` the
/// mean, each observation `x` does:
///
/// ```text
/// W  ← λW + 1
/// δ  ← x − m                (deviation from the mean as it stood)
/// m  ← m + δ/W
/// δ' ← x − m                (deviation from the mean as it now stands)
/// S  ← λS + δ ⊗ δ'
/// ```
///
/// and the covariance is `S/W`. That is Welford's update with a decay on the
/// accumulator, and at `λ = 1` it is exactly Welford: `W` is then the
/// observation count and `S/W` the population covariance. Nothing large is
/// subtracted from anything large — see this module's header for what happens
/// to `E[xy] − E[x]E[y]` on a series centred away from zero.
///
/// # Symmetry is computed, not hoped for
///
/// `δ ⊗ δ'` is symmetric in exact arithmetic because `δ' = δ(1 − 1/W)`, but
/// `δᵢδ'ⱼ` and `δⱼδ'ᵢ` are two different floating-point expressions and need
/// not agree in their last bits. The upper triangle is computed and mirrored,
/// so the matrix is symmetric to the bit: a replay that produced a matrix
/// differing from the original in one off-diagonal pair is not a replay, and
/// an asymmetry of a few ulps is enough to fail a positive-definiteness test
/// on a nearly singular book.
#[derive(Clone, Debug)]
pub struct EwCovariance {
    labels: Vec<String>,
    decay: ForgettingFactor,
    observations: u64,
    /// `W`: the accumulated weight the estimate is divided by. Also what
    /// debiases the warm-up — after one observation `W` is 1, not `1/(1-λ)`,
    /// so the mean is that observation rather than a fraction of it.
    weight: f64,
    /// `Σw²`, carried only so [`Self::effective_observations`] can report
    /// Kish's effective sample size. A covariance over an effective two
    /// observations is arithmetic rather than evidence, and a consumer that
    /// cannot tell is a consumer that will size against noise.
    weight_squared: f64,
    mean: Vec<f64>,
    comoment: Vec<f64>,
}

impl EwCovariance {
    /// An estimator over `names`, forgetting at `decay`.
    pub fn new(names: &BTreeSet<String>, decay: ForgettingFactor) -> Result<Self> {
        let labels = dimensions_of(names, "an exponentially weighted covariance")?;
        let k = labels.len();
        Ok(Self {
            labels,
            decay,
            observations: 0,
            weight: 0.0,
            weight_squared: 0.0,
            mean: vec![0.0; k],
            comoment: vec![0.0; k * k],
        })
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    pub const fn dimension(&self) -> usize {
        self.labels.len()
    }

    pub const fn observations(&self) -> u64 {
        self.observations
    }

    pub const fn decay(&self) -> ForgettingFactor {
        self.decay
    }

    /// Kish's effective sample size, `W²/Σw²`.
    ///
    /// The number of equally weighted observations this estimate is worth. It
    /// is the count during warm-up and settles at `(1+λ)/(1−λ)` thereafter —
    /// about twice [`ForgettingFactor::effective_window`], because an
    /// exponential weighting has a long thin tail of observations that still
    /// count for a little. This is the figure a consumer should judge "is this
    /// covariance evidence" against, not [`Self::observations`], which keeps
    /// rising long after the early ones have stopped counting for anything.
    pub fn effective_observations(&self) -> f64 {
        if self.weight_squared <= 0.0 {
            0.0
        } else {
            self.weight * self.weight / self.weight_squared
        }
    }

    /// The memory the estimator occupies, which does not move with the stream:
    /// the mean vector, the co-moment matrix and the labels.
    pub fn bytes(&self) -> usize {
        let floats = (self.mean.len() + self.comoment.len()) * std::mem::size_of::<f64>();
        let labels: usize = self.labels.iter().map(String::len).sum();
        floats + labels + std::mem::size_of::<Self>()
    }

    /// Absorb one observation, which must name every dimension exactly once.
    pub fn observe(&mut self, observation: &BTreeMap<String, f64>) -> Result<()> {
        let row = row_in_label_order(
            &self.labels,
            observation,
            "an exponentially weighted covariance",
        )?;
        let k = self.labels.len();
        let lambda = self.decay.value();

        self.weight = lambda * self.weight + 1.0;
        self.weight_squared = lambda * lambda * self.weight_squared + 1.0;

        let weight = self.weight;
        let before: Vec<f64> = row
            .iter()
            .zip(&self.mean)
            .map(|(value, mean)| value - mean)
            .collect();
        for (mean, delta) in self.mean.iter_mut().zip(&before) {
            *mean += delta / weight;
        }
        let after: Vec<f64> = row
            .iter()
            .zip(&self.mean)
            .map(|(value, mean)| value - mean)
            .collect();

        for (i, delta_before) in before.iter().enumerate() {
            for (j, delta_after) in after.iter().enumerate().skip(i) {
                let updated = lambda * self.comoment[at(k, i, j)] + delta_before * delta_after;
                self.comoment[at(k, i, j)] = updated;
                // Mirrored rather than computed twice: see the type's note on
                // why a few ulps of asymmetry is not a rounding detail.
                self.comoment[at(k, j, i)] = updated;
            }
        }
        self.observations = self.observations.saturating_add(1);
        Ok(())
    }

    fn index_of(&self, label: &str) -> Result<usize> {
        self.labels
            .iter()
            .position(|held| held == label)
            .ok_or_else(|| {
                Error::not_found(format!(
                    "this covariance was built over {:?} and {label:?} is not among them; build an \
                     estimator naming it, or read one of the dimensions it holds",
                    self.labels
                ))
            })
    }

    /// The exponentially weighted covariance of two named dimensions.
    ///
    /// Zero before the first observation, which is the only honest answer: an
    /// estimator that has seen nothing has no dispersion to report, and a
    /// refusal would force every caller to special-case the first cycle.
    pub fn covariance(&self, a: &str, b: &str) -> Result<f64> {
        let (i, j) = (self.index_of(a)?, self.index_of(b)?);
        if self.weight <= 0.0 {
            return Ok(0.0);
        }
        Ok(self.comoment[at(self.labels.len(), i, j)] / self.weight)
    }

    /// The exponentially weighted variance of one named dimension.
    pub fn variance(&self, label: &str) -> Result<f64> {
        self.covariance(label, label)
    }

    /// The exponentially weighted correlation of two named dimensions.
    ///
    /// Zero when either dimension has not moved, matching
    /// [`crate::stats::correlation`]: a correlation with a constant is
    /// undefined, and zero is the value that makes a caller treat it as no
    /// relationship rather than as a `NaN` that poisons every sum it reaches.
    pub fn correlation(&self, a: &str, b: &str) -> Result<f64> {
        let va = self.variance(a)?;
        let vb = self.variance(b)?;
        if va <= 0.0 || vb <= 0.0 {
            return Ok(0.0);
        }
        Ok((self.covariance(a, b)? / (va * vb).sqrt()).clamp(-1.0, 1.0))
    }

    /// The whole covariance matrix, rows and columns in label order.
    pub fn matrix(&self) -> Result<Matrix> {
        let k = self.labels.len();
        let scale = if self.weight <= 0.0 {
            0.0
        } else {
            1.0 / self.weight
        };
        Matrix::from_vec(
            k,
            k,
            self.comoment.iter().map(|value| value * scale).collect(),
        )
    }

    /// The exponentially weighted mean of one named dimension.
    pub fn mean(&self, label: &str) -> Result<f64> {
        let index = self.index_of(label)?;
        Ok(self.mean[index])
    }
}

/// A linear model fitted one observation at a time, with a forgetting factor.
///
/// §22.2's "Linear and logistic models — recursive least squares, online
/// gradient — O(k) parameters". The parameters are O(k) and the state that
/// carries them is O(k²); neither moves with the stream.
///
/// At a forgetting factor of one this converges to exactly the ordinary least
/// squares fit over everything it has seen —
/// `recursive_least_squares_agrees_with_ordinary_least_squares_on_the_same_rows`
/// asserts that against [`crate::stats::ols`] — without keeping the rows.
/// Below one it weights recent rows more heavily, which is what makes it
/// usable on a relationship that moves.
///
/// # No intercept is added
///
/// [`crate::stats::ols`] prepends a column of ones. This does not, and the
/// difference is deliberate: a caller that wants an intercept names a
/// dimension and feeds it 1.0, so the fitted constant appears in
/// [`Self::parameters`] under a name the caller chose rather than at an index
/// the caller has to remember. A model whose first coefficient means something
/// different from the rest is a model somebody will eventually read off by
/// position.
///
/// # Covariance windup, stated rather than hidden
///
/// With a forgetting factor below one and a regressor that stops varying, the
/// precision matrix grows without bound — the estimator is dividing by the
/// excitation it is no longer getting. This is the known failure of recursive
/// least squares and it is not silently absorbed here: when the update stops
/// producing finite numbers, [`Self::observe`] refuses and names the two
/// things that fix it. [`Self::precision_trace`] is the number that rises
/// before it happens, so a caller can see it coming.
#[derive(Clone, Debug)]
pub struct RecursiveLeastSquares {
    labels: Vec<String>,
    forgetting: ForgettingFactor,
    parameters: Vec<f64>,
    /// `P`, the inverse of the exponentially weighted Gram matrix. Starts at
    /// `prior_variance · I`: a large prior variance is a weak prior and fast
    /// early movement, a small one is a ridge of `1/prior_variance` holding
    /// the parameters near zero until the data outweighs it.
    precision: Vec<f64>,
    observations: u64,
}

impl RecursiveLeastSquares {
    /// A fit over `names`, forgetting at `forgetting`, starting from
    /// parameters of zero and a precision of `prior_variance · I`.
    pub fn new(
        names: &BTreeSet<String>,
        forgetting: ForgettingFactor,
        prior_variance: f64,
    ) -> Result<Self> {
        let labels = dimensions_of(names, "a recursive least squares fit")?;
        if !prior_variance.is_finite() || prior_variance <= 0.0 {
            return Err(Error::invalid(format!(
                "a recursive least squares fit needs a positive, finite prior variance and was \
                 given {prior_variance}; pass a large value (1e4 is usual) to let the first rows \
                 move the parameters freely, or a small one to hold them near zero until the data \
                 outweighs the prior. A prior variance of zero is a fit that can never learn \
                 anything"
            )));
        }
        let k = labels.len();
        let mut precision = vec![0.0; k * k];
        for i in 0..k {
            precision[at(k, i, i)] = prior_variance;
        }
        Ok(Self {
            labels,
            forgetting,
            parameters: vec![0.0; k],
            precision,
            observations: 0,
        })
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    pub const fn dimension(&self) -> usize {
        self.labels.len()
    }

    pub const fn observations(&self) -> u64 {
        self.observations
    }

    pub const fn forgetting(&self) -> ForgettingFactor {
        self.forgetting
    }

    /// The trace of the precision matrix — the number that climbs when a
    /// regressor stops varying and the fit is winding up.
    pub fn precision_trace(&self) -> f64 {
        let k = self.labels.len();
        (0..k).map(|i| self.precision[at(k, i, i)]).sum()
    }

    /// The memory the fit occupies, which does not move with the stream.
    pub fn bytes(&self) -> usize {
        let floats = (self.parameters.len() + self.precision.len()) * std::mem::size_of::<f64>();
        let labels: usize = self.labels.iter().map(String::len).sum();
        floats + labels + std::mem::size_of::<Self>()
    }

    /// The fitted coefficients by name, in label order.
    pub fn parameters(&self) -> BTreeMap<String, f64> {
        self.labels
            .iter()
            .cloned()
            .zip(self.parameters.iter().copied())
            .collect()
    }

    /// One fitted coefficient, or a refusal naming the dimensions this fit
    /// actually holds.
    pub fn parameter(&self, label: &str) -> Result<f64> {
        self.labels
            .iter()
            .position(|held| held == label)
            .map(|index| self.parameters[index])
            .ok_or_else(|| {
                Error::not_found(format!(
                    "this fit was built over {:?} and {label:?} is not among them; refit over the \
                     dimensions you mean to read",
                    self.labels
                ))
            })
    }

    /// What the fit predicts for a row, which must name every dimension.
    pub fn predict(&self, row: &BTreeMap<String, f64>) -> Result<f64> {
        let x = row_in_label_order(&self.labels, row, "a recursive least squares fit")?;
        let prediction: f64 = (0..x.len()).map(|i| self.parameters[i] * x[i]).sum();
        if !prediction.is_finite() {
            return Err(Error::numeric(format!(
                "this fit predicted {prediction} for a finite row; the parameters have diverged, \
                 which recursive least squares does when a forgetting factor below one meets a \
                 regressor that has stopped varying. Fit a fresh estimator, at a forgetting \
                 factor nearer one"
            )));
        }
        Ok(prediction)
    }

    /// Absorb one row and its response, returning the error the fit made on
    /// that row **before** absorbing it.
    ///
    /// The a-priori error rather than the a-posteriori one, because it is the
    /// only one that is out-of-sample: the a-posteriori error is measured
    /// after the row has already moved the parameters towards itself, and a
    /// caller scoring a model on it is scoring the fit's ability to fit the
    /// point it has just been told the answer to.
    pub fn observe(&mut self, row: &BTreeMap<String, f64>, response: f64) -> Result<f64> {
        let x = row_in_label_order(&self.labels, row, "a recursive least squares fit")?;
        if !response.is_finite() {
            return Err(Error::invalid(format!(
                "a recursive least squares fit was given a response of {response}; one non-finite \
                 response makes every parameter non-finite for ever. Drop the observation at the \
                 source, where the gap can be reported"
            )));
        }
        let k = self.labels.len();
        let lambda = self.forgetting.value();

        // P·x, the direction this row informs.
        let px: Vec<f64> = (0..k)
            .map(|i| (0..k).map(|j| self.precision[at(k, i, j)] * x[j]).sum())
            .collect();
        let denominator = lambda + (0..k).map(|i| x[i] * px[i]).sum::<f64>();
        if !denominator.is_finite() || denominator <= 0.0 {
            return Err(Error::numeric(format!(
                "a recursive least squares update divided by {denominator}, which means the \
                 precision matrix is no longer positive definite — the fit has wound up, as \
                 recursive least squares does when a forgetting factor below one meets a \
                 regressor that has stopped varying. Fit a fresh estimator, at a forgetting \
                 factor nearer one, or stop feeding the dimension that no longer moves"
            )));
        }

        let error = response - (0..k).map(|i| self.parameters[i] * x[i]).sum::<f64>();
        for (parameter, direction) in self.parameters.iter_mut().zip(&px) {
            *parameter += (direction / denominator) * error;
        }

        // P ← (P − (P x)(P x)ᵀ / denominator) / λ. Written from `px` alone so
        // that the subtracted term is symmetric to the bit; an asymmetric `P`
        // loses positive definiteness within a few hundred updates and the fit
        // diverges without anything having gone visibly wrong.
        for i in 0..k {
            for j in i..k {
                let updated = (self.precision[at(k, i, j)] - px[i] * px[j] / denominator) / lambda;
                self.precision[at(k, i, j)] = updated;
                self.precision[at(k, j, i)] = updated;
            }
        }

        if !self.parameters.iter().all(|value| value.is_finite())
            || !self.precision.iter().all(|value| value.is_finite())
        {
            return Err(Error::numeric(
                "a recursive least squares update produced a parameter or a precision that is \
                 not finite; the fit has diverged and every figure it reports from here is \
                 meaningless. Fit a fresh estimator, at a forgetting factor nearer one",
            ));
        }
        self.observations = self.observations.saturating_add(1);
        Ok(error)
    }
}

/// The leading factors of a stream, by Sanger's generalised Hebbian rule.
///
/// §22.2's "Factor structure — streaming PCA, Oja's rule or incremental SVD —
/// O(k × components)". The state is `components × k` weights, a mean vector
/// for centring and one accumulated projection power per component. None of it
/// moves with the stream.
///
/// # Sanger's rule rather than Oja's alone, and why that matters here
///
/// Oja's rule applied to several components at once converges to the subspace
/// the leading factors span, and not to the factors themselves: the components
/// drift within that subspace and any two of them are as good an answer as any
/// other two. That is enough to project onto, and it is not enough for what
/// §22.2 asks "factor structure" for — naming the dominant factor and saying
/// how much of the variance it carries. Sanger's rule adds the deflation that
/// makes component `r` see only what components `0..r` have not already
/// explained, so the components come out ordered by the variance they carry,
/// as the eigenvectors of a batch decomposition are.
///
/// # The learning rate
///
/// `η_t = rate / (1 + rate·t)`. It decays like `1/t`, so the sum diverges and
/// the sum of squares converges — the Robbins–Monro conditions, which is what
/// makes the sequence converge at all rather than rattling around the answer
/// for ever at a fixed step. A constant rate is the usual mistake: it never
/// settles, and the component it reports is a function of the last few
/// observations rather than of the stream.
#[derive(Clone, Debug)]
pub struct StreamingPca {
    labels: Vec<String>,
    components: usize,
    /// `components × k`, row-major: one unit vector per component.
    weights: Vec<f64>,
    /// The exponentially weighted mean the observations are centred on.
    /// Uncentred data has its first component pointing at the mean, which is a
    /// fact about where the series lives rather than about how it varies.
    mean: Vec<f64>,
    /// Exponentially weighted mean of each component's squared projection —
    /// the variance along that component, since the components are unit
    /// vectors.
    power: Vec<f64>,
    decay: ForgettingFactor,
    rate: f64,
    weight: f64,
    observations: u64,
}

impl StreamingPca {
    /// An estimator for the `components` leading factors of `names`, centring
    /// at `decay`, stepping at `rate`, starting from a basis derived from
    /// `seed`.
    ///
    /// `seed` is a parameter rather than a call to an entropy source because
    /// §21.1's estimators have to be reproducible from the log: a factor
    /// structure that cannot be recomputed is a factor structure nobody can
    /// check.
    pub fn new(
        names: &BTreeSet<String>,
        components: usize,
        decay: ForgettingFactor,
        rate: f64,
        seed: u64,
    ) -> Result<Self> {
        let labels = dimensions_of(names, "a streaming principal component estimator")?;
        let k = labels.len();
        if components == 0 {
            return Err(Error::invalid(
                "a streaming principal component estimator was asked for zero components; ask for \
                 at least one, or do not build the estimator",
            ));
        }
        if components > k {
            return Err(Error::invalid(format!(
                "a streaming principal component estimator was asked for {components} \
                 component(s) over {k} dimension(s); {k} dimensions span at most {k} independent \
                 directions and any further component is a direction along which the data cannot \
                 vary. Ask for at most {k}, or build the estimator over more dimensions"
            )));
        }
        if !rate.is_finite() || rate <= 0.0 || rate > 1.0 {
            return Err(Error::invalid(format!(
                "a streaming principal component estimator needs a learning rate in (0, 1] and \
                 was given {rate}; a rate above one takes a step longer than the deviation it is \
                 stepping along and diverges, and a rate of zero never moves off its starting \
                 basis. It is refused rather than clamped"
            )));
        }
        let mut estimator = Self {
            labels,
            components,
            weights: vec![0.0; components * k],
            mean: vec![0.0; k],
            power: vec![0.0; components],
            decay,
            rate,
            weight: 0.0,
            observations: 0,
        };
        estimator.seed_basis(seed);
        Ok(estimator)
    }

    /// The starting basis: the canonical axes, nudged by a seeded
    /// pseudo-random perturbation, then orthonormalised.
    ///
    /// The axes alone would be a valid start and a bad one on axis-aligned
    /// data, where the projection of a component onto the deflated residual
    /// can be exactly zero and the rule never moves. The perturbation is small
    /// enough (0.1 in norm) that the rows stay far from parallel, so the
    /// Gram–Schmidt pass below is well conditioned and cannot produce a
    /// near-zero row to normalise.
    fn seed_basis(&mut self, seed: u64) {
        let k = self.labels.len();
        let scale = 0.1 / (k as f64).sqrt();
        for r in 0..self.components {
            for c in 0..k {
                // u64 → f64 at the statistics boundary: a hash becomes a
                // perturbation in [-1, 1], which is a number and not a count.
                let bits = splitmix64(seed ^ ((r as u64) << 32) ^ (c as u64).wrapping_mul(0x9E37));
                let unit = (bits >> 11) as f64 / ((1u64 << 53) as f64);
                self.weights[at(k, r, c)] = scale * (2.0 * unit - 1.0);
            }
            self.weights[at(k, r, r)] += 1.0;
        }
        for r in 0..self.components {
            for prior in 0..r {
                let projection: f64 = (0..k)
                    .map(|c| self.weights[at(k, r, c)] * self.weights[at(k, prior, c)])
                    .sum();
                for c in 0..k {
                    self.weights[at(k, r, c)] -= projection * self.weights[at(k, prior, c)];
                }
            }
            let norm: f64 = (0..k)
                .map(|c| self.weights[at(k, r, c)] * self.weights[at(k, r, c)])
                .sum::<f64>()
                .sqrt();
            if norm > 0.0 {
                for c in 0..k {
                    self.weights[at(k, r, c)] /= norm;
                }
            }
        }
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    pub const fn dimension(&self) -> usize {
        self.labels.len()
    }

    pub const fn components(&self) -> usize {
        self.components
    }

    pub const fn observations(&self) -> u64 {
        self.observations
    }

    /// The memory the estimator occupies, which does not move with the
    /// stream: `components × k` weights and two short vectors.
    pub fn bytes(&self) -> usize {
        let floats =
            (self.weights.len() + self.mean.len() + self.power.len()) * std::mem::size_of::<f64>();
        let labels: usize = self.labels.iter().map(String::len).sum();
        floats + labels + std::mem::size_of::<Self>()
    }

    /// Absorb one observation.
    pub fn observe(&mut self, observation: &BTreeMap<String, f64>) -> Result<()> {
        let row = row_in_label_order(
            &self.labels,
            observation,
            "a streaming principal component estimator",
        )?;
        let k = self.labels.len();
        let lambda = self.decay.value();

        self.weight = lambda * self.weight + 1.0;
        let weight = self.weight;
        for (mean, value) in self.mean.iter_mut().zip(&row) {
            *mean += (value - *mean) / weight;
        }
        let centred: Vec<f64> = row
            .iter()
            .zip(&self.mean)
            .map(|(value, mean)| value - mean)
            .collect();

        // Every projection is taken against the basis as it stood at the top
        // of this observation. Updating a component and then projecting the
        // next one against the updated basis makes the deflation depend on the
        // order the loop happens to run in, which is not the rule and does not
        // converge to the same answer.
        let projections: Vec<f64> = (0..self.components)
            .map(|r| (0..k).map(|c| self.weights[at(k, r, c)] * centred[c]).sum())
            .collect();

        // u64 → f64 at the statistics boundary: the observation count becomes
        // a step size, which is a statistic rather than a count.
        let step = self.rate / (1.0 + self.rate * self.observations as f64);

        for r in 0..self.components {
            // Sanger's deflation: what the components up to this one have
            // already claimed is removed, so this component learns only the
            // residual. Without it every component converges on the leading
            // factor and the estimator reports one factor several times, each
            // claiming the same variance —
            // `streaming_pca_keeps_its_components_apart` is that failure.
            //
            // The `..=r` rather than `..r` includes Sanger's own term at
            // `j == r`, which shrinks the row back towards unit length. That
            // half is belt and braces here, because the row is renormalised
            // below: a mutation to `..r` changed no answer any test could
            // see, which is worth saying rather than claiming a necessity the
            // code does not have. It is kept so that the update is the rule
            // as Sanger states it rather than a variant of it that happens to
            // agree today.
            let mut residual = centred.clone();
            for (j, projection) in projections.iter().enumerate().take(r + 1) {
                for (c, component) in residual.iter_mut().enumerate() {
                    *component -= projection * self.weights[at(k, j, c)];
                }
            }
            let gain = step * projections[r];
            for (c, component) in residual.iter().enumerate() {
                self.weights[at(k, r, c)] += gain * component;
            }
            let norm: f64 = (0..k)
                .map(|c| self.weights[at(k, r, c)] * self.weights[at(k, r, c)])
                .sum::<f64>()
                .sqrt();
            if !norm.is_finite() || norm <= 0.0 {
                return Err(Error::numeric(format!(
                    "component {r} of a streaming principal component estimator reached a norm of \
                     {norm} and can no longer be normalised; the step size has outrun the data. \
                     Start a fresh estimator at a smaller learning rate"
                )));
            }
            // Renormalised every observation. Sanger's rule drives the rows
            // towards unit length on its own, asymptotically; "asymptotically"
            // is not a property an explained-variance figure can be read off,
            // because a component of norm 1.02 reports four percent more
            // variance than it carries.
            for c in 0..k {
                self.weights[at(k, r, c)] /= norm;
            }
            self.power[r] += (projections[r] * projections[r] - self.power[r]) / self.weight;
        }
        self.observations = self.observations.saturating_add(1);
        Ok(())
    }

    fn check_component(&self, index: usize) -> Result<()> {
        if index >= self.components {
            return Err(Error::not_found(format!(
                "this estimator holds {} component(s) and component {index} was asked for; \
                 components are numbered from zero, and the leading one is component 0",
                self.components
            )));
        }
        Ok(())
    }

    /// One component's loadings by dimension name, in label order.
    pub fn component(&self, index: usize) -> Result<BTreeMap<String, f64>> {
        self.check_component(index)?;
        let k = self.labels.len();
        Ok(self
            .labels
            .iter()
            .cloned()
            .enumerate()
            .map(|(c, label)| (label, self.weights[at(k, index, c)]))
            .collect())
    }

    /// One component's loadings as a unit vector in label order.
    pub fn loadings(&self, index: usize) -> Result<&[f64]> {
        self.check_component(index)?;
        let k = self.labels.len();
        Ok(&self.weights[at(k, index, 0)..at(k, index, 0) + k])
    }

    /// The variance the stream carries along one component.
    ///
    /// The exponentially weighted mean of the squared projection. Because the
    /// components are unit vectors this is a variance in the units the
    /// observations arrived in, and the ratio of two of them is the ratio of
    /// the factors' importance.
    pub fn explained_variance(&self, index: usize) -> Result<f64> {
        self.check_component(index)?;
        Ok(self.power[index])
    }

    /// Project one observation onto the components, in component order.
    pub fn project(&self, observation: &BTreeMap<String, f64>) -> Result<Vec<f64>> {
        let row = row_in_label_order(
            &self.labels,
            observation,
            "a streaming principal component estimator",
        )?;
        let k = self.labels.len();
        Ok((0..self.components)
            .map(|r| {
                (0..k)
                    .map(|c| self.weights[at(k, r, c)] * (row[c] - self.mean[c]))
                    .sum()
            })
            .collect())
    }
}
