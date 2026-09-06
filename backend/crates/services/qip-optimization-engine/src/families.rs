//! Family clustering — blueprint §23.1 LEVEL 1.
//!
//! The blueprint's decomposition allocates across families rather than across
//! strategies, on the argument that strategies inside a family are
//! near-substitutes and diversification is won or lost between families. That
//! argument only holds if the family boundary survives the moment
//! diversification is being asked for, and the blueprint's own note on the
//! capability (§50.2, "Family clustering reflecting stress correlation —
//! calm-market correlation understates stress correlation") says why it
//! usually does not.
//!
//! So this module refuses to cluster on the full sample. Every clustering is
//! keyed on the correlation measured **inside a stated stress window**, and
//! the calm complement is estimated alongside it only to report how far the
//! calm view would have been wrong. Two strategies that look independent in
//! calm markets and move together in a drawdown belong in one family: the
//! allocator must not be told it holds two bets when it holds one. A
//! clustering built on the full sample would place them apart, and the error
//! would surface exactly once, in the drawdown.
//!
//! There is no calm fallback. [`StressCorrelation::from_returns`] needs a
//! window with a usable sample on both sides and refuses without one, naming
//! what to supply, rather than quietly returning the clustering the blueprint
//! warns against.
//!
//! # Determinism
//!
//! Family assignment reaches a decision record and a replay that reorders is
//! not a replay, so nothing here may depend on the order the caller happened
//! to supply its strategies in, or on hash iteration order. Two properties
//! hold that line:
//!
//! * Strategies are sorted by [`StrategyId`] on the way in, so the internal
//!   index of a strategy is a function of the *set* supplied, not the
//!   sequence.
//! * The merge loop is agglomerative and has **no random initialisation** —
//!   nothing to seed, unlike the k-means the same job is usually done with.
//!   Equal-distance merges break by the scan order over that canonical
//!   index, so ties resolve the same way every run.
//!
//! Output containers are `BTreeMap`/`BTreeSet` for the same reason.
//!
//! # Bounds
//!
//! The linkage matrix is `n × n` of `f64` and the merge loop is cubic in the
//! strategy count, so the working set is bounded at [`MAX_STRATEGIES`]: 1024
//! strategies is an 8 MB matrix and roughly a billion comparisons, which suits
//! the blueprint's daily cadence in an optimised build. It is *not* cheap in a
//! debug build — a population one over the bound was measured at more than
//! twenty minutes of CPU with the bound removed — so a test that needs a large
//! population should assert the refusal rather than drive the merge.
//!
//! The blueprint's ~10,000 strategies do not fit that bound and are not made
//! to fit by relaxing it — they need a pre-partition (by horizon and venue,
//! say) ahead of this stage, which is not built here. An unbounded working set
//! is refused, not grown.

use qip_core::error::{Error, Result};
use qip_core::ids::StrategyId;
use qip_numerics::Matrix;
use qip_numerics::stats;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Largest strategy population this stage will cluster in one pass.
///
/// Not a performance hint: exceeding it is refused. See the module note on
/// bounds for the arithmetic.
pub const MAX_STRATEGIES: usize = 1_024;

/// Fewest observations either side of the stress window may carry.
///
/// A correlation from a handful of joint observations has a standard error
/// wider than the gap between "these two are the same bet" and "these two are
/// independent", so a family boundary drawn on it is drawn on noise. Refusing
/// is better than emitting families nobody can defend.
pub const MIN_WINDOW_OBSERVATIONS: usize = 12;

/// Tolerance for the positive semi-definiteness check on a supplied matrix.
///
/// The Jacobi eigensolver returns eigenvalues a few ulps either side of zero
/// for a genuinely singular matrix; this admits that and nothing more.
const PSD_TOLERANCE: f64 = 1e-9;

/// One strategy's return series, in the observation order shared by every
/// series in a clustering run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyReturns {
    strategy: StrategyId,
    returns: Vec<f64>,
}

impl StrategyReturns {
    /// Refuses a non-finite return rather than carrying it into a covariance.
    ///
    /// A single NaN propagates through the whole correlation matrix and comes
    /// out the far end as a clustering that looks plausible and is arbitrary.
    pub fn new(strategy: StrategyId, returns: Vec<f64>) -> Result<Self> {
        if returns.is_empty() {
            return Err(Error::invalid(format!(
                "strategy {strategy} has no returns; supply the series it is to be clustered on \
                 or leave it out of the population"
            )));
        }
        if let Some(index) = returns.iter().position(|r| !r.is_finite()) {
            return Err(Error::numeric(format!(
                "strategy {strategy} has a non-finite return at observation {index}; repair or \
                 drop the observation upstream — it cannot be carried into a correlation"
            )));
        }
        Ok(Self { strategy, returns })
    }

    pub fn strategy(&self) -> &StrategyId {
        &self.strategy
    }

    pub fn returns(&self) -> &[f64] {
        &self.returns
    }

    pub fn len(&self) -> usize {
        self.returns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.returns.is_empty()
    }
}

/// Which observations count as stress, and how that was decided.
///
/// Carried into the clustering record so a reader can tell whether the
/// families were keyed on a regime classifier's verdict, on a benchmark's own
/// tail, or on something an operator asserted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StressWindow {
    observations: usize,
    /// Ascending and unique.
    stress: Vec<usize>,
    provenance: String,
}

impl StressWindow {
    /// A window an upstream classifier chose.
    ///
    /// Refuses an out-of-range or repeated index: a repeated observation
    /// weights one day twice and silently makes it the family boundary.
    pub fn explicit(
        observations: usize,
        stress: Vec<usize>,
        provenance: impl Into<String>,
    ) -> Result<Self> {
        if observations == 0 {
            return Err(Error::invalid(
                "a stress window needs an observation count above zero",
            ));
        }
        let mut sorted = stress;
        sorted.sort_unstable();
        let unique = sorted.len();
        sorted.dedup();
        if sorted.len() != unique {
            return Err(Error::invalid(
                "the stress window repeats an observation; supply each index once, because a \
                 repeat weights that observation twice in the correlation",
            ));
        }
        if let Some(bad) = sorted.iter().find(|i| **i >= observations) {
            return Err(Error::invalid(format!(
                "stress observation {bad} is outside the {observations} observations supplied"
            )));
        }
        Self::build(observations, sorted, provenance.into())
    }

    /// The worst `quantile` fraction of a benchmark series.
    ///
    /// The benchmark is whatever the desk considers the stress axis — a
    /// drawdown series, a volatility index, a funding spread. Ties break by
    /// observation index so the same benchmark always yields the same window.
    pub fn worst_quantile(
        benchmark: &[f64],
        quantile: f64,
        provenance: impl Into<String>,
    ) -> Result<Self> {
        if !quantile.is_finite() || quantile <= 0.0 || quantile >= 1.0 {
            return Err(Error::invalid(format!(
                "a stress quantile must lie strictly between 0 and 1; {quantile} does not — a \
                 quantile of 0 selects nothing and 1 leaves no calm sample to compare against"
            )));
        }
        if benchmark.is_empty() {
            return Err(Error::invalid(
                "a stress quantile needs a benchmark series to take the tail of",
            ));
        }
        if let Some(index) = benchmark.iter().position(|v| !v.is_finite()) {
            return Err(Error::numeric(format!(
                "the benchmark has a non-finite value at observation {index}; a stress window \
                 cannot be chosen from a series with a hole in it"
            )));
        }
        let observations = benchmark.len();
        #[allow(clippy::cast_precision_loss)]
        let count = (observations as f64 * quantile).floor() as usize;
        let mut order: Vec<usize> = (0..observations).collect();
        // Sort by value, then by index: two identical benchmark readings must
        // not let the input's own ordering decide which one is "stress".
        order.sort_by(|a, b| {
            benchmark[*a]
                .partial_cmp(&benchmark[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(b))
        });
        let mut stress: Vec<usize> = order.into_iter().take(count).collect();
        stress.sort_unstable();
        Self::build(observations, stress, provenance.into())
    }

    fn build(observations: usize, stress: Vec<usize>, provenance: String) -> Result<Self> {
        if provenance.trim().is_empty() {
            return Err(Error::invalid(
                "a stress window needs a provenance; the record has to say how stress was decided",
            ));
        }
        let calm = observations - stress.len();
        if stress.len() < MIN_WINDOW_OBSERVATIONS {
            return Err(Error::invalid(format!(
                "the stress window holds {} observations, below the {MIN_WINDOW_OBSERVATIONS} a \
                 correlation can be estimated from; widen the window or supply a longer history \
                 rather than clustering on the calm sample",
                stress.len()
            )));
        }
        if calm < MIN_WINDOW_OBSERVATIONS {
            return Err(Error::invalid(format!(
                "the calm complement holds {calm} observations, below the \
                 {MIN_WINDOW_OBSERVATIONS} a correlation can be estimated from; narrow the \
                 window or supply a longer history — without a calm sample there is nothing to \
                 measure the stress understatement against"
            )));
        }
        Ok(Self {
            observations,
            stress,
            provenance,
        })
    }

    pub fn observations(&self) -> usize {
        self.observations
    }

    pub fn stress_indices(&self) -> &[usize] {
        &self.stress
    }

    /// Every observation not in the stress window, ascending.
    pub fn calm_indices(&self) -> Vec<usize> {
        let stress: BTreeSet<usize> = self.stress.iter().copied().collect();
        (0..self.observations)
            .filter(|i| !stress.contains(i))
            .collect()
    }

    pub fn provenance(&self) -> &str {
        &self.provenance
    }
}

/// The pair of correlation matrices a clustering is keyed on: the stress one
/// it uses, and the calm one it reports against.
///
/// Both are held because the calm matrix is the evidence for the design. A
/// caller can see, for its own population, how much the ordinary-period view
/// would have understated co-movement, instead of taking the blueprint's word
/// for it.
#[derive(Clone, Debug, PartialEq)]
pub struct StressCorrelation {
    /// Sorted and unique — the canonical index order everything downstream
    /// uses.
    strategies: Vec<StrategyId>,
    stress: Matrix,
    calm: Matrix,
    stress_observations: usize,
    calm_observations: usize,
    provenance: String,
}

impl StressCorrelation {
    /// Estimate both matrices from returns and a window.
    ///
    /// Refuses a strategy that does not move at all inside the window: a flat
    /// series has no measurable co-movement, and placing it in a family would
    /// assert a relationship nobody estimated.
    pub fn from_returns(series: &[StrategyReturns], window: &StressWindow) -> Result<Self> {
        if series.len() < 2 {
            return Err(Error::invalid(format!(
                "clustering needs at least two strategies to have a correlation between; {} \
                 supplied",
                series.len()
            )));
        }
        if series.len() > MAX_STRATEGIES {
            return Err(Error::invalid(format!(
                "{} strategies exceeds the {MAX_STRATEGIES} this stage clusters in one pass; \
                 pre-partition the population (by horizon or venue) and cluster within each part \
                 rather than raising the bound",
                series.len()
            )));
        }

        let mut ordered: Vec<&StrategyReturns> = series.iter().collect();
        // Canonical order: the clustering must be a function of the set of
        // strategies, never of the order the caller listed them in.
        ordered.sort_by(|a, b| a.strategy.cmp(&b.strategy));
        for pair in ordered.windows(2) {
            if pair[0].strategy == pair[1].strategy {
                return Err(Error::invalid(format!(
                    "strategy {} appears twice; supply one series per strategy, because a \
                     duplicate is perfectly correlated with itself and drags a family around it",
                    pair[0].strategy
                )));
            }
        }
        for entry in &ordered {
            if entry.returns.len() != window.observations {
                return Err(Error::invalid(format!(
                    "strategy {} has {} observations but the stress window covers {}; align the \
                     series to one calendar before clustering",
                    entry.strategy,
                    entry.returns.len(),
                    window.observations
                )));
            }
        }

        let stress_indices = window.stress_indices();
        let calm_indices = window.calm_indices();
        let slice = |entry: &StrategyReturns, indices: &[usize]| -> Vec<f64> {
            indices.iter().map(|i| entry.returns[*i]).collect()
        };

        let stress_slices: Vec<Vec<f64>> =
            ordered.iter().map(|e| slice(e, stress_indices)).collect();
        let calm_slices: Vec<Vec<f64>> = ordered.iter().map(|e| slice(e, &calm_indices)).collect();

        for (entry, values) in ordered.iter().zip(&stress_slices) {
            if stats::stddev(values) <= 0.0 {
                return Err(Error::numeric(format!(
                    "strategy {} does not move inside the stress window, so its stress \
                     correlation with anything is unmeasured; exclude it or widen the window \
                     rather than filing it in a family on no evidence",
                    entry.strategy
                )));
            }
        }

        let stress = Self::pairwise(&stress_slices);
        let calm = Self::pairwise(&calm_slices);
        let strategies: Vec<StrategyId> = ordered.iter().map(|e| e.strategy.clone()).collect();

        Self::assemble(
            strategies,
            stress,
            calm,
            stress_indices.len(),
            calm_indices.len(),
            format!("estimated from returns; window: {}", window.provenance()),
        )
    }

    /// Take matrices estimated elsewhere — a shrunk risk-engine estimate, say.
    ///
    /// Validated as hard as an estimated one: a supplied matrix is the case
    /// where a non-symmetric, out-of-range or indefinite input actually
    /// arrives, and an indefinite correlation matrix means the linkage
    /// distances it implies are not distances at all.
    pub fn from_matrices(
        strategies: Vec<StrategyId>,
        stress: Matrix,
        calm: Matrix,
        stress_observations: usize,
        calm_observations: usize,
    ) -> Result<Self> {
        let mut ordered = strategies;
        ordered.sort();
        Self::assemble(
            ordered,
            stress,
            calm,
            stress_observations,
            calm_observations,
            "supplied by the caller".to_string(),
        )
    }

    fn pairwise(slices: &[Vec<f64>]) -> Matrix {
        let n = slices.len();
        let mut m = Matrix::identity(n);
        for i in 0..n {
            for j in (i + 1)..n {
                let rho = stats::correlation(&slices[i], &slices[j]);
                m.set(i, j, rho);
                m.set(j, i, rho);
            }
        }
        m
    }

    fn assemble(
        strategies: Vec<StrategyId>,
        stress: Matrix,
        calm: Matrix,
        stress_observations: usize,
        calm_observations: usize,
        provenance: String,
    ) -> Result<Self> {
        let n = strategies.len();
        if n < 2 {
            return Err(Error::invalid(
                "clustering needs at least two strategies to have a correlation between",
            ));
        }
        if n > MAX_STRATEGIES {
            return Err(Error::invalid(format!(
                "{n} strategies exceeds the {MAX_STRATEGIES} this stage clusters in one pass; \
                 pre-partition the population and cluster within each part"
            )));
        }
        for (label, m) in [("stress", &stress), ("calm", &calm)] {
            if m.rows() != n || m.cols() != n {
                return Err(Error::invalid(format!(
                    "the {label} correlation matrix is {}x{} for {n} strategies; it must be {n}x{n}",
                    m.rows(),
                    m.cols()
                )));
            }
            if !m.all_finite() {
                return Err(Error::numeric(format!(
                    "the {label} correlation matrix holds a non-finite entry; repair the \
                     estimate upstream — a NaN here becomes an arbitrary family"
                )));
            }
            if !m.is_symmetric(1e-9) {
                return Err(Error::invalid(format!(
                    "the {label} correlation matrix is not symmetric; correlation is symmetric \
                     by definition, so an asymmetric one means the estimator wrote the wrong cell"
                )));
            }
            for i in 0..n {
                if (m.get(i, i) - 1.0).abs() > 1e-9 {
                    return Err(Error::invalid(format!(
                        "the {label} correlation matrix has {} on the diagonal at {i}; a \
                         correlation matrix has a unit diagonal — this looks like a covariance",
                        m.get(i, i)
                    )));
                }
                for j in 0..n {
                    if m.get(i, j).abs() > 1.0 + 1e-9 {
                        return Err(Error::invalid(format!(
                            "the {label} correlation matrix holds {} at ({i}, {j}); a \
                             correlation cannot exceed 1 in magnitude",
                            m.get(i, j)
                        )));
                    }
                }
            }
        }
        // Positive semi-definiteness is not decoration. The linkage distance
        // sqrt(2(1 - rho)) is a metric only on a Gram matrix; from an
        // indefinite input it can violate the triangle inequality, and an
        // agglomerative merge sequence over a non-metric is not the dendrogram
        // anybody thinks it is. Refuse rather than repair: `Matrix::nearest_psd`
        // exists, but applying it here would silently change the correlations
        // the families are keyed on.
        if !stress.is_positive_semidefinite(PSD_TOLERANCE) {
            return Err(Error::numeric(
                "the stress correlation matrix is not positive semi-definite, so the distances \
                 it implies are not distances; repair the estimate upstream (shrinkage, or a \
                 nearest-PSD projection whose adjustment is recorded) — this stage will not \
                 repair it silently",
            ));
        }
        if stress_observations < MIN_WINDOW_OBSERVATIONS
            || calm_observations < MIN_WINDOW_OBSERVATIONS
        {
            return Err(Error::invalid(format!(
                "the estimate rests on {stress_observations} stress and {calm_observations} calm \
                 observations; both must reach {MIN_WINDOW_OBSERVATIONS} before a family \
                 boundary drawn from them means anything"
            )));
        }
        Ok(Self {
            strategies,
            stress,
            calm,
            stress_observations,
            calm_observations,
            provenance,
        })
    }

    pub fn strategies(&self) -> &[StrategyId] {
        &self.strategies
    }

    pub fn len(&self) -> usize {
        self.strategies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strategies.is_empty()
    }

    pub fn stress(&self) -> &Matrix {
        &self.stress
    }

    pub fn calm(&self) -> &Matrix {
        &self.calm
    }

    pub fn provenance(&self) -> &str {
        &self.provenance
    }

    /// Mean over every off-diagonal pair of stress correlation less calm
    /// correlation.
    ///
    /// The blueprint's warning as a number for this population. Positive means
    /// the calm view understated co-movement, which is the case the clustering
    /// is built for; a negative value is worth a look, because it says this
    /// population decoupled under stress.
    pub fn mean_stress_excess(&self) -> f64 {
        let n = self.strategies.len();
        let mut total = 0.0;
        let mut pairs = 0usize;
        for i in 0..n {
            for j in (i + 1)..n {
                total += self.stress.get(i, j) - self.calm.get(i, j);
                pairs += 1;
            }
        }
        if pairs == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let denominator = pairs as f64;
        total / denominator
    }
}

/// How the distance between two clusters is measured.
///
/// Single linkage is deliberately absent. It joins two families through one
/// bridging strategy, and a bridge is exactly the merge that fails under
/// stress — the whole reason this module exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Linkage {
    /// Mean distance over all cross-cluster pairs. The default: it is the
    /// least sensitive of the two to one extreme pair.
    #[default]
    Average,
    /// The largest distance across the pair, so a family's diameter is
    /// bounded by the merge that formed it.
    Complete,
}

impl Linkage {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Average => "average",
            Self::Complete => "complete",
        }
    }
}

/// A family's identity: its position in the canonical family ordering.
///
/// Held as an index rather than a name so ordering is numeric and cannot drift
/// with a formatting change. [`FamilyId::name`] renders it in the character
/// set the lifecycle crate's family names accept, so a family can be carried
/// into trial accounting without a translation table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FamilyId(usize);

impl FamilyId {
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub const fn index(&self) -> usize {
        self.0
    }

    /// Zero-padded so a lexicographic sort of the names matches the numeric
    /// order of the ids.
    pub fn name(&self) -> String {
        format!("family-{:03}", self.0)
    }
}

impl std::fmt::Display for FamilyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name())
    }
}

/// What the clustering is being asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyClustering {
    target_families: usize,
    linkage: Linkage,
}

impl FamilyClustering {
    /// Refuses a target the population cannot supply.
    ///
    /// Asking for more families than there are strategies is not a request to
    /// be rounded down; it means the caller believes it has a population it
    /// does not have, and an allocator handed 128 families when 40 strategies
    /// exist would report a diversification it never held.
    pub fn new(target_families: usize) -> Result<Self> {
        if target_families == 0 {
            return Err(Error::invalid(
                "a clustering into zero families produces nothing to allocate across; ask for at \
                 least one",
            ));
        }
        Ok(Self {
            target_families,
            linkage: Linkage::Average,
        })
    }

    pub const fn with_linkage(mut self, linkage: Linkage) -> Self {
        self.linkage = linkage;
        self
    }

    pub const fn target_families(&self) -> usize {
        self.target_families
    }

    pub const fn linkage(&self) -> Linkage {
        self.linkage
    }

    /// Cluster the population, keyed on stress correlation.
    ///
    /// The calm matrix is clustered too, and only so the result can say how
    /// many pairs the calm view would have filed differently. The calm
    /// clustering is never returned.
    pub fn cluster(&self, correlation: &StressCorrelation) -> Result<FamilyAssignment> {
        let n = correlation.len();
        if self.target_families > n {
            return Err(Error::invalid(format!(
                "a target of {} families exceeds the {n} strategies available; supply more \
                 strategies or lower the target — a family cannot be empty",
                self.target_families
            )));
        }

        let stress_labels = self.merge(correlation.stress(), n)?;
        let calm_labels = self.merge(correlation.calm(), n)?;

        // Canonical family numbering: families are ordered by their smallest
        // member index, which is itself the sorted-id order. Two runs over the
        // same set therefore number the same family the same way.
        let mut first_seen: Vec<usize> = Vec::new();
        let mut renumber: BTreeMap<usize, usize> = BTreeMap::new();
        for label in &stress_labels {
            if !renumber.contains_key(label) {
                renumber.insert(*label, first_seen.len());
                first_seen.push(*label);
            }
        }

        let mut families: BTreeMap<FamilyId, BTreeSet<StrategyId>> = BTreeMap::new();
        let mut of_strategy: BTreeMap<StrategyId, FamilyId> = BTreeMap::new();
        for (index, strategy) in correlation.strategies().iter().enumerate() {
            let label = stress_labels[index];
            let family = FamilyId::new(*renumber.get(&label).unwrap_or(&0));
            families.entry(family).or_default().insert(strategy.clone());
            of_strategy.insert(strategy.clone(), family);
        }

        let diagnostics = Diagnostics::measure(correlation, &stress_labels, &calm_labels, self);

        Ok(FamilyAssignment {
            families,
            of_strategy,
            diagnostics,
        })
    }

    /// Agglomerative merging down to the target count. Returns one label per
    /// strategy, in the canonical index order.
    fn merge(&self, correlation: &Matrix, n: usize) -> Result<Vec<usize>> {
        // Correlation distance: 0 when two strategies are the same bet, 2 when
        // they are exact opposites. A metric on a positive semi-definite
        // correlation matrix, which `StressCorrelation` has already proved.
        let mut distance = Matrix::zeros(n, n);
        for i in 0..n {
            for j in (i + 1)..n {
                let d = (2.0 * (1.0 - correlation.get(i, j))).max(0.0).sqrt();
                distance.set(i, j, d);
                distance.set(j, i, d);
            }
        }

        let mut label: Vec<usize> = (0..n).collect();
        let mut active: Vec<bool> = vec![true; n];
        let mut size: Vec<usize> = vec![1; n];
        let mut clusters = n;

        while clusters > self.target_families {
            let mut best: Option<(usize, usize, f64)> = None;
            for i in 0..n {
                if !active[i] {
                    continue;
                }
                for (j, alive) in active.iter().enumerate().skip(i + 1) {
                    if !alive {
                        continue;
                    }
                    let d = distance.get(i, j);
                    // Strictly less, scanned in canonical index order: an
                    // equal-distance pair loses to the one seen first, so ties
                    // resolve identically on every run.
                    if best.is_none_or(|(_, _, current)| d < current) {
                        best = Some((i, j, d));
                    }
                }
            }
            let Some((a, b, _)) = best else {
                return Err(Error::numeric(format!(
                    "the merge loop found no candidate pair with {clusters} clusters still \
                     active; this is a bug in the clustering, not a bad input"
                )));
            };

            // Lance-Williams update in place. The merged cluster takes the
            // lower index, which preserves ordering by smallest member.
            let neighbours: Vec<usize> = active
                .iter()
                .enumerate()
                .filter(|(c, alive)| **alive && *c != a && *c != b)
                .map(|(c, _)| c)
                .collect();
            for c in neighbours {
                #[allow(clippy::cast_precision_loss)]
                let updated = match self.linkage {
                    Linkage::Average => {
                        let (wa, wb) = (size[a] as f64, size[b] as f64);
                        (wa * distance.get(a, c) + wb * distance.get(b, c)) / (wa + wb)
                    }
                    Linkage::Complete => distance.get(a, c).max(distance.get(b, c)),
                };
                distance.set(a, c, updated);
                distance.set(c, a, updated);
            }
            active[b] = false;
            size[a] += size[b];
            for entry in label.iter_mut() {
                if *entry == b {
                    *entry = a;
                }
            }
            clusters -= 1;
        }

        Ok(label)
    }
}

/// What the clustering measured on the way to its answer.
///
/// Every field is computed from the population supplied. None of it asserts
/// that the families mean anything about live markets — that is the caller's
/// evidence to produce, not this stage's.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub linkage: Linkage,
    pub target_families: usize,
    pub strategies: usize,
    pub stress_observations: usize,
    pub calm_observations: usize,
    /// Mean stress-less-calm correlation over every pair. Positive means the
    /// calm view understated co-movement in this population.
    pub mean_stress_excess: f64,
    /// Mean stress correlation between strategies placed in the same family.
    pub mean_intra_family_correlation: f64,
    /// Mean stress correlation between strategies placed in different
    /// families. A clustering that has done any work at all has this below the
    /// intra-family figure.
    pub mean_inter_family_correlation: f64,
    /// Unordered strategy pairs the calm-keyed clustering would have filed
    /// differently — together when stress separates them, or apart when
    /// stress joins them.
    ///
    /// This is the cost of the design decision, in pairs, for this population.
    /// Zero means calm and stress agreed here and the choice cost nothing.
    pub pairs_calm_would_have_misfiled: usize,
    pub pairs_total: usize,
}

impl Diagnostics {
    fn measure(
        correlation: &StressCorrelation,
        stress_labels: &[usize],
        calm_labels: &[usize],
        clustering: &FamilyClustering,
    ) -> Self {
        let n = correlation.len();
        let stress = correlation.stress();
        let (mut intra, mut intra_pairs) = (0.0, 0usize);
        let (mut inter, mut inter_pairs) = (0.0, 0usize);
        let mut disagreements = 0usize;
        let mut pairs = 0usize;
        for i in 0..n {
            for j in (i + 1)..n {
                pairs += 1;
                let together_in_stress = stress_labels[i] == stress_labels[j];
                let together_in_calm = calm_labels[i] == calm_labels[j];
                if together_in_stress != together_in_calm {
                    disagreements += 1;
                }
                if together_in_stress {
                    intra += stress.get(i, j);
                    intra_pairs += 1;
                } else {
                    inter += stress.get(i, j);
                    inter_pairs += 1;
                }
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let mean = |total: f64, count: usize| {
            if count == 0 {
                0.0
            } else {
                total / count as f64
            }
        };
        Self {
            linkage: clustering.linkage(),
            target_families: clustering.target_families(),
            strategies: n,
            stress_observations: correlation.stress_observations,
            calm_observations: correlation.calm_observations,
            mean_stress_excess: correlation.mean_stress_excess(),
            mean_intra_family_correlation: mean(intra, intra_pairs),
            mean_inter_family_correlation: mean(inter, inter_pairs),
            pairs_calm_would_have_misfiled: disagreements,
            pairs_total: pairs,
        }
    }
}

/// The families, and which strategy is in which.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FamilyAssignment {
    families: BTreeMap<FamilyId, BTreeSet<StrategyId>>,
    of_strategy: BTreeMap<StrategyId, FamilyId>,
    diagnostics: Diagnostics,
}

impl FamilyAssignment {
    pub fn families(&self) -> &BTreeMap<FamilyId, BTreeSet<StrategyId>> {
        &self.families
    }

    pub fn family_count(&self) -> usize {
        self.families.len()
    }

    pub fn members(&self, family: FamilyId) -> Option<&BTreeSet<StrategyId>> {
        self.families.get(&family)
    }

    pub fn family_of(&self, strategy: &StrategyId) -> Option<FamilyId> {
        self.of_strategy.get(strategy).copied()
    }

    pub fn strategies(&self) -> impl Iterator<Item = &StrategyId> {
        self.of_strategy.keys()
    }

    pub fn strategy_count(&self) -> usize {
        self.of_strategy.len()
    }

    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }
}
