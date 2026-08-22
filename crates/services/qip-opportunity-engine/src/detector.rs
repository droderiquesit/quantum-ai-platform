//! Detectors.
//!
//! Each reports a z-score rather than a boolean, so how unusual something is
//! survives into the ranking. Each also declares its expected false-positive
//! rate at its threshold, which is what lets the engine reason about how much
//! of its queue is noise instead of discovering it by working through it.

use qip_core::{Duration, ObjectId, Timestamp};
use qip_numerics::hmm::GaussianHmm;
use qip_numerics::stats;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What an anomaly is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyKind {
    /// A price move far outside its recent distribution.
    PriceMove,
    /// A change in realised volatility.
    VolatilityShift,
    /// Trading volume far above or below normal.
    VolumeSpike,
    /// A persistent shift in the mean, as opposed to a single outlier.
    StructuralBreak,
    /// The volatility regime changed.
    RegimeChange,
    /// A pair's correlation departed from its long-run relationship.
    CorrelationBreakdown,
    /// Spread widening or depth thinning.
    LiquidityDeterioration,
    /// A reported figure far from consensus.
    FundamentalSurprise,
    /// A macro release far from consensus.
    MacroSurprise,
    /// Unusual tone or volume of coverage.
    SentimentShift,
    /// An alternative-data reading diverging from its proxy.
    AlternativeDataDivergence,
}

impl AnomalyKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PriceMove => "price_move",
            Self::VolatilityShift => "volatility_shift",
            Self::VolumeSpike => "volume_spike",
            Self::StructuralBreak => "structural_break",
            Self::RegimeChange => "regime_change",
            Self::CorrelationBreakdown => "correlation_breakdown",
            Self::LiquidityDeterioration => "liquidity_deterioration",
            Self::FundamentalSurprise => "fundamental_surprise",
            Self::MacroSurprise => "macro_surprise",
            Self::SentimentShift => "sentiment_shift",
            Self::AlternativeDataDivergence => "alternative_data_divergence",
        }
    }

    /// How much an anomaly of this kind matters if it is real, in `[0, 1]`.
    ///
    /// A structural break or a regime change is a statement about the world
    /// having changed; a single large price move usually is not.
    pub fn base_importance(&self) -> f64 {
        match self {
            Self::StructuralBreak | Self::RegimeChange => 0.85,
            Self::FundamentalSurprise | Self::MacroSurprise => 0.80,
            Self::CorrelationBreakdown => 0.75,
            Self::LiquidityDeterioration => 0.65,
            Self::VolatilityShift => 0.60,
            Self::PriceMove => 0.50,
            Self::AlternativeDataDivergence => 0.55,
            Self::VolumeSpike | Self::SentimentShift => 0.40,
        }
    }

    /// Investigations this kind of anomaly implies.
    pub fn implied_investigation(&self) -> Vec<crate::opportunity::Investigation> {
        use crate::opportunity::Investigation as I;
        match self {
            Self::PriceMove | Self::VolumeSpike => vec![I::DataVerification, I::CausalAnalysis],
            Self::VolatilityShift => vec![I::CausalAnalysis, I::DerivativesAnalysis],
            Self::StructuralBreak => vec![I::CausalAnalysis, I::HistoricalValidation],
            Self::RegimeChange => vec![I::MacroAnalysis, I::HistoricalValidation],
            Self::CorrelationBreakdown => vec![I::CausalAnalysis, I::HistoricalValidation],
            Self::LiquidityDeterioration => vec![I::MicrostructureAnalysis],
            Self::FundamentalSurprise => vec![I::Valuation, I::SupplyChainAnalysis],
            Self::MacroSurprise => vec![I::MacroAnalysis, I::CreditAnalysis],
            Self::SentimentShift => vec![I::DataVerification, I::CausalAnalysis],
            Self::AlternativeDataDivergence => vec![I::DataVerification, I::HistoricalValidation],
        }
    }
}

/// One detection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anomaly {
    pub kind: AnomalyKind,
    /// Instrument or series the anomaly concerns.
    pub subject: String,
    /// The detector that produced it.
    pub detector: String,
    /// How many standard deviations from normal, signed.
    pub z_score: f64,
    /// The value observed.
    pub observed: f64,
    /// What was expected.
    pub expected: f64,
    /// Observations behind the expectation. A z-score from twenty points is
    /// not a z-score from a thousand, and the difference must survive.
    pub sample_size: usize,
    pub detected_at: Timestamp,
    /// Human-readable statement of what was noticed.
    pub description: String,
}

impl Anomaly {
    /// Confidence the anomaly is real, from its magnitude and sample size.
    ///
    /// A large z-score on a short history is mostly evidence that the history
    /// is short.
    pub fn confidence(&self) -> f64 {
        let magnitude = (self.z_score.abs() / 4.0).clamp(0.0, 1.0);
        let sample = (self.sample_size as f64 / (self.sample_size as f64 + 60.0)).clamp(0.0, 1.0);
        magnitude * sample
    }

    /// Importance if real: the kind's base importance scaled by magnitude.
    pub fn importance(&self) -> f64 {
        let magnitude = (self.z_score.abs() / 3.0).clamp(0.3, 1.5);
        (self.kind.base_importance() * magnitude).clamp(0.0, 1.0)
    }
}

/// What a detector is given to work with.
#[derive(Debug, Default)]
pub struct DetectionContext {
    pub as_of: Timestamp,
    /// Close price history per instrument, oldest first.
    pub prices: BTreeMap<String, Vec<f64>>,
    /// Volume history per instrument.
    pub volumes: BTreeMap<String, Vec<f64>>,
    /// Quoted spread in basis points per instrument.
    pub spreads: BTreeMap<String, Vec<f64>>,
    /// Named scalar observations: surprises, sentiment, alternative data.
    pub observations: BTreeMap<String, Vec<f64>>,
}

impl DetectionContext {
    pub fn new(as_of: Timestamp) -> Self {
        Self {
            as_of,
            ..Self::default()
        }
    }

    pub fn with_prices(mut self, subject: impl Into<String>, prices: Vec<f64>) -> Self {
        self.prices.insert(subject.into(), prices);
        self
    }

    pub fn with_volumes(mut self, subject: impl Into<String>, volumes: Vec<f64>) -> Self {
        self.volumes.insert(subject.into(), volumes);
        self
    }

    pub fn with_spreads(mut self, subject: impl Into<String>, spreads: Vec<f64>) -> Self {
        self.spreads.insert(subject.into(), spreads);
        self
    }

    pub fn with_observations(mut self, name: impl Into<String>, values: Vec<f64>) -> Self {
        self.observations.insert(name.into(), values);
        self
    }
}

/// Something that notices when the world is unusual.
pub trait Detector: std::fmt::Debug + Send + Sync {
    fn name(&self) -> &str;

    /// The z-score at which the detector fires.
    fn threshold(&self) -> f64;

    /// Expected false positives per thousand observations at the threshold.
    ///
    /// Declared rather than measured after the fact, so the engine can reason
    /// about how much of its queue is noise before working through it.
    fn expected_false_positive_rate(&self) -> f64;

    fn detect(&self, context: &DetectionContext) -> Vec<Anomaly>;
}

/// Flags returns far outside their recent distribution.
///
/// Uses robust statistics: with a plain mean and standard deviation, the very
/// outlier being looked for inflates the deviation and hides itself.
#[derive(Debug, Clone)]
pub struct ReturnAnomalyDetector {
    pub window: usize,
    pub threshold: f64,
}

impl Default for ReturnAnomalyDetector {
    fn default() -> Self {
        Self {
            window: 60,
            threshold: 3.0,
        }
    }
}

impl Detector for ReturnAnomalyDetector {
    fn name(&self) -> &str {
        "return-anomaly"
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }

    fn expected_false_positive_rate(&self) -> f64 {
        // Returns are fat-tailed, so a 3-sigma threshold fires far more often
        // than a normal distribution implies. Stating the realistic rate is the
        // point: a detector claiming 0.3 per thousand would be lying.
        6.0
    }

    fn detect(&self, context: &DetectionContext) -> Vec<Anomaly> {
        let mut anomalies = Vec::new();
        for (subject, prices) in &context.prices {
            if prices.len() < self.window + 2 {
                continue;
            }
            let returns = stats::log_returns(prices);
            let history =
                &returns[returns.len().saturating_sub(self.window + 1)..returns.len() - 1];
            let latest = returns[returns.len() - 1];
            if history.len() < 10 {
                continue;
            }

            let centre = stats::median(history);
            let scale = stats::median_absolute_deviation(history);
            if scale <= 1e-12 {
                continue;
            }
            let z = (latest - centre) / scale;
            if z.abs() < self.threshold {
                continue;
            }

            anomalies.push(Anomaly {
                kind: AnomalyKind::PriceMove,
                subject: subject.clone(),
                detector: self.name().to_string(),
                z_score: z,
                observed: latest,
                expected: centre,
                sample_size: history.len(),
                detected_at: context.as_of,
                description: format!(
                    "{subject} returned {:+.2}% against a typical {:+.2}%, {z:+.1} robust sigma",
                    latest * 100.0,
                    centre * 100.0
                ),
            });
        }
        anomalies
    }
}

/// Flags a change in realised volatility between two adjacent windows.
#[derive(Debug, Clone)]
pub struct VolatilityShiftDetector {
    pub window: usize,
    pub threshold: f64,
}

impl Default for VolatilityShiftDetector {
    fn default() -> Self {
        Self {
            window: 20,
            threshold: 2.5,
        }
    }
}

impl Detector for VolatilityShiftDetector {
    fn name(&self) -> &str {
        "volatility-shift"
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }

    fn expected_false_positive_rate(&self) -> f64 {
        4.0
    }

    fn detect(&self, context: &DetectionContext) -> Vec<Anomaly> {
        let mut anomalies = Vec::new();
        for (subject, prices) in &context.prices {
            let returns = stats::log_returns(prices);
            if returns.len() < self.window * 3 {
                continue;
            }
            let recent = &returns[returns.len() - self.window..];
            let baseline = &returns[..returns.len() - self.window];

            let recent_volatility = stats::stddev(recent);
            let baseline_volatility = stats::stddev(baseline);
            if baseline_volatility <= 1e-12 {
                continue;
            }

            // The log ratio of volatilities is roughly normal with standard
            // error 1/sqrt(2n), which is what makes this a z-score rather than
            // an arbitrary multiple.
            let log_ratio = (recent_volatility / baseline_volatility).ln();
            let standard_error = (1.0 / (2.0 * recent.len() as f64)).sqrt();
            let z = log_ratio / standard_error;
            if z.abs() < self.threshold {
                continue;
            }

            anomalies.push(Anomaly {
                kind: AnomalyKind::VolatilityShift,
                subject: subject.clone(),
                detector: self.name().to_string(),
                z_score: z,
                observed: recent_volatility,
                expected: baseline_volatility,
                sample_size: returns.len(),
                detected_at: context.as_of,
                description: format!(
                    "{subject} volatility moved from {:.2}% to {:.2}% per period",
                    baseline_volatility * 100.0,
                    recent_volatility * 100.0
                ),
            });
        }
        anomalies
    }
}

/// Flags a persistent shift in the mean, using a cumulative sum.
///
/// Distinct from an outlier detector: a single large move is not a break, and a
/// series of small moves in the same direction is. CUSUM accumulates the
/// deviation, so it detects the second and ignores the first.
#[derive(Debug, Clone)]
pub struct StructuralBreakDetector {
    /// Deviation, in standard deviations, treated as noise and not accumulated.
    pub slack: f64,
    /// Accumulated deviation at which a break is declared.
    pub threshold: f64,
    pub minimum_history: usize,
    /// Largest per-observation contribution, in standard deviations.
    ///
    /// Without this a single large outlier accumulates more than the whole
    /// threshold in one step, and the detector reports a "structural break"
    /// from one day's move — which is exactly what it is supposed to
    /// distinguish itself from. Capping the contribution forces a run of
    /// deviations in the same direction before anything is declared.
    pub winsorise_at: f64,
}

impl Default for StructuralBreakDetector {
    fn default() -> Self {
        // Slack 0.5 with a threshold of 8 is a standard CUSUM operating point:
        // the average run length to a false alarm is in the thousands of
        // observations, while a sustained one-sigma shift in the mean is caught
        // in roughly sixteen. Winsorising at two sigma means a single outlier
        // contributes at most 1.5, so no one observation can trip it.
        Self {
            slack: 0.5,
            threshold: 8.0,
            minimum_history: 60,
            winsorise_at: 2.0,
        }
    }
}

impl Detector for StructuralBreakDetector {
    fn name(&self) -> &str {
        "structural-break"
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }

    fn expected_false_positive_rate(&self) -> f64 {
        2.0
    }

    fn detect(&self, context: &DetectionContext) -> Vec<Anomaly> {
        let mut anomalies = Vec::new();
        let series = context
            .prices
            .iter()
            .map(|(k, v)| (k, stats::log_returns(v)));

        for (subject, returns) in series {
            if returns.len() < self.minimum_history {
                continue;
            }
            let centre = stats::mean(&returns);
            let scale = stats::stddev(&returns);
            if scale <= 1e-12 {
                continue;
            }

            // Two-sided CUSUM: positive and negative drift tracked separately,
            // each reset to zero when the accumulated deviation falls back.
            let mut high = 0.0f64;
            let mut low = 0.0f64;
            let mut peak_high = 0.0f64;
            let mut peak_low = 0.0f64;
            let mut break_index = 0usize;
            for (index, value) in returns.iter().enumerate() {
                let standardised =
                    ((value - centre) / scale).clamp(-self.winsorise_at, self.winsorise_at);
                high = (high + standardised - self.slack).max(0.0);
                low = (low - standardised - self.slack).max(0.0);
                if high > peak_high {
                    peak_high = high;
                    break_index = index;
                }
                if low > peak_low {
                    peak_low = low;
                    break_index = index;
                }
            }

            let magnitude = peak_high.max(peak_low);
            if magnitude < self.threshold {
                continue;
            }
            let direction = if peak_high >= peak_low { 1.0 } else { -1.0 };

            anomalies.push(Anomaly {
                kind: AnomalyKind::StructuralBreak,
                subject: subject.clone(),
                detector: self.name().to_string(),
                z_score: direction * magnitude,
                observed: magnitude,
                expected: self.threshold,
                sample_size: returns.len(),
                detected_at: context.as_of,
                description: format!(
                    "{subject} shows a persistent {} drift accumulating to {magnitude:.1} over \
                     at least {} observations, strongest around observation {break_index}",
                    if direction > 0.0 {
                        "upward"
                    } else {
                        "downward"
                    },
                    (self.threshold / (self.winsorise_at - self.slack).max(0.01)).ceil() as usize
                ),
            });
        }
        anomalies
    }
}

/// Infers the volatility regime and flags a change.
#[derive(Debug, Clone)]
pub struct RegimeDetector {
    pub minimum_history: usize,
    /// Probability of the high-volatility state above which a change is called.
    pub probability_threshold: f64,
    /// Shortest expected regime duration worth reporting, in periods.
    ///
    /// A regime lasting two days is not a regime; reporting one would have a
    /// strategy switching constantly on noise.
    pub minimum_persistence: f64,
    /// How recently the transition must have happened to still be news.
    ///
    /// A regime that turned two months ago is the current state of the world,
    /// not a discovery. Set too tight, though, and nothing ever fires: the
    /// filtered probability takes several observations to cross, so the window
    /// has to be wider than the crossing itself.
    pub transition_lookback: usize,
}

impl Default for RegimeDetector {
    fn default() -> Self {
        Self {
            minimum_history: 120,
            probability_threshold: 0.75,
            minimum_persistence: 5.0,
            transition_lookback: 15,
        }
    }
}

impl Detector for RegimeDetector {
    fn name(&self) -> &str {
        "regime-change"
    }

    fn threshold(&self) -> f64 {
        self.probability_threshold
    }

    fn expected_false_positive_rate(&self) -> f64 {
        1.5
    }

    fn detect(&self, context: &DetectionContext) -> Vec<Anomaly> {
        let mut anomalies = Vec::new();
        for (subject, prices) in &context.prices {
            let returns = stats::log_returns(prices);
            if returns.len() < self.minimum_history {
                continue;
            }
            let Ok(model) = GaussianHmm::fit(&returns, 120, 1e-7) else {
                continue;
            };
            if !model.states_are_persistent(self.minimum_persistence) {
                continue;
            }

            let filtered = model.filter(&returns);
            if filtered.len() < 2 {
                continue;
            }
            let current = filtered[filtered.len() - 1];
            let stressed = model.high_volatility_state();
            let probability = current[stressed];

            if probability < self.probability_threshold {
                continue;
            }
            // A change means the probability *crossed* recently, not merely
            // that it is high now — an established regime is the state of the
            // world, not a discovery.
            let Some(last_calm) = filtered.iter().rposition(|p| p[stressed] < 0.5) else {
                continue; // never calm in the sample: no transition to report
            };
            if filtered.len() - 1 - last_calm > self.transition_lookback {
                continue;
            }

            let calm_volatility = model.variances[1 - stressed].sqrt();
            let stressed_volatility = model.variances[stressed].sqrt();
            anomalies.push(Anomaly {
                kind: AnomalyKind::RegimeChange,
                subject: subject.clone(),
                detector: self.name().to_string(),
                // Expressed as a z-score by mapping the probability through the
                // normal quantile, so it ranks alongside the other detectors.
                z_score: qip_numerics::distributions::normal_inv_cdf(probability.min(0.9999)),
                observed: probability,
                expected: 0.5,
                sample_size: returns.len(),
                detected_at: context.as_of,
                description: format!(
                    "{subject} entered its high-volatility regime with probability \
                     {probability:.2}; volatility {:.2}% against {:.2}% in the calm state, \
                     expected to persist about {:.0} periods",
                    stressed_volatility * 100.0,
                    calm_volatility * 100.0,
                    model.expected_duration(stressed)
                ),
            });
        }
        anomalies
    }
}

/// Flags a pair whose correlation has departed from its long-run relationship.
#[derive(Debug, Clone)]
pub struct CorrelationBreakdownDetector {
    pub recent_window: usize,
    pub threshold: f64,
    /// Long-run correlation below which a pair is not worth watching.
    pub minimum_baseline: f64,
}

impl Default for CorrelationBreakdownDetector {
    fn default() -> Self {
        Self {
            recent_window: 30,
            threshold: 2.5,
            minimum_baseline: 0.4,
        }
    }
}

impl Detector for CorrelationBreakdownDetector {
    fn name(&self) -> &str {
        "correlation-breakdown"
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }

    fn expected_false_positive_rate(&self) -> f64 {
        5.0
    }

    fn detect(&self, context: &DetectionContext) -> Vec<Anomaly> {
        let subjects: Vec<&String> = context.prices.keys().collect();
        let mut anomalies = Vec::new();

        for (index, first) in subjects.iter().enumerate() {
            for second in subjects.iter().skip(index + 1) {
                let a = stats::log_returns(&context.prices[*first]);
                let b = stats::log_returns(&context.prices[*second]);
                let n = a.len().min(b.len());
                if n < self.recent_window * 3 {
                    continue;
                }

                let baseline =
                    stats::correlation(&a[..n - self.recent_window], &b[..n - self.recent_window]);
                if baseline.abs() < self.minimum_baseline {
                    continue;
                }
                let recent =
                    stats::correlation(&a[n - self.recent_window..], &b[n - self.recent_window..]);

                // Fisher's transform makes a correlation difference roughly
                // normal, with a standard error that depends only on the sample
                // sizes. Comparing raw correlations would treat a move from
                // 0.90 to 0.80 the same as one from 0.10 to 0.00.
                let z_baseline = fisher(baseline);
                let z_recent = fisher(recent);
                let standard_error = ((1.0 / (n - self.recent_window - 3).max(1) as f64)
                    + (1.0 / (self.recent_window - 3).max(1) as f64))
                    .sqrt();
                let z = (z_recent - z_baseline) / standard_error;
                if z.abs() < self.threshold {
                    continue;
                }

                anomalies.push(Anomaly {
                    kind: AnomalyKind::CorrelationBreakdown,
                    subject: format!("{first}|{second}"),
                    detector: self.name().to_string(),
                    z_score: z,
                    observed: recent,
                    expected: baseline,
                    sample_size: n,
                    detected_at: context.as_of,
                    description: format!(
                        "correlation between {first} and {second} moved from {baseline:.2} to \
                         {recent:.2}"
                    ),
                });
            }
        }
        anomalies
    }
}

/// Flags spread widening.
#[derive(Debug, Clone)]
pub struct LiquidityDetector {
    pub window: usize,
    pub threshold: f64,
}

impl Default for LiquidityDetector {
    fn default() -> Self {
        Self {
            window: 30,
            threshold: 3.0,
        }
    }
}

impl Detector for LiquidityDetector {
    fn name(&self) -> &str {
        "liquidity-deterioration"
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }

    fn expected_false_positive_rate(&self) -> f64 {
        3.0
    }

    fn detect(&self, context: &DetectionContext) -> Vec<Anomaly> {
        let mut anomalies = Vec::new();
        for (subject, spreads) in &context.spreads {
            if spreads.len() < self.window + 5 {
                continue;
            }
            let history = &spreads[..spreads.len() - 1];
            let latest = spreads[spreads.len() - 1];
            let centre = stats::median(history);
            let scale = stats::median_absolute_deviation(history);
            if scale <= 1e-12 || centre <= 0.0 {
                continue;
            }
            let z = (latest - centre) / scale;
            // Only widening matters; a tighter spread is not a risk.
            if z < self.threshold {
                continue;
            }

            anomalies.push(Anomaly {
                kind: AnomalyKind::LiquidityDeterioration,
                subject: subject.clone(),
                detector: self.name().to_string(),
                z_score: z,
                observed: latest,
                expected: centre,
                sample_size: history.len(),
                detected_at: context.as_of,
                description: format!(
                    "{subject} spread widened to {latest:.1}bp against a typical {centre:.1}bp"
                ),
            });
        }
        anomalies
    }
}

/// Flags a scalar observation far from its recent distribution.
///
/// Used for surprises, sentiment and alternative data — anything arriving as a
/// named series of numbers rather than as prices.
#[derive(Debug, Clone)]
pub struct ObservationDetector {
    pub kind: AnomalyKind,
    pub threshold: f64,
    pub minimum_history: usize,
}

impl ObservationDetector {
    pub fn new(kind: AnomalyKind) -> Self {
        Self {
            kind,
            threshold: 2.5,
            minimum_history: 12,
        }
    }
}

impl Detector for ObservationDetector {
    fn name(&self) -> &str {
        "observation-anomaly"
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }

    fn expected_false_positive_rate(&self) -> f64 {
        8.0
    }

    fn detect(&self, context: &DetectionContext) -> Vec<Anomaly> {
        let mut anomalies = Vec::new();
        for (subject, values) in &context.observations {
            if values.len() < self.minimum_history {
                continue;
            }
            let history = &values[..values.len() - 1];
            let latest = values[values.len() - 1];
            let centre = stats::median(history);
            let scale = stats::median_absolute_deviation(history);
            if scale <= 1e-12 {
                continue;
            }
            let z = (latest - centre) / scale;
            if z.abs() < self.threshold {
                continue;
            }

            anomalies.push(Anomaly {
                kind: self.kind,
                subject: subject.clone(),
                detector: self.name().to_string(),
                z_score: z,
                observed: latest,
                expected: centre,
                sample_size: history.len(),
                detected_at: context.as_of,
                description: format!(
                    "{subject} read {latest:.3} against a typical {centre:.3}, {z:+.1} sigma"
                ),
            });
        }
        anomalies
    }
}

/// The detectors the engine runs.
#[derive(Debug)]
pub struct DetectorRegistry {
    detectors: Vec<Box<dyn Detector>>,
}

impl Default for DetectorRegistry {
    fn default() -> Self {
        Self::standard()
    }
}

impl DetectorRegistry {
    pub fn empty() -> Self {
        Self {
            detectors: Vec::new(),
        }
    }

    /// The full set the platform ships with.
    pub fn standard() -> Self {
        Self {
            detectors: vec![
                Box::new(ReturnAnomalyDetector::default()),
                Box::new(VolatilityShiftDetector::default()),
                Box::new(StructuralBreakDetector::default()),
                Box::new(RegimeDetector::default()),
                Box::new(CorrelationBreakdownDetector::default()),
                Box::new(LiquidityDetector::default()),
                Box::new(ObservationDetector::new(AnomalyKind::FundamentalSurprise)),
            ],
        }
    }

    pub fn add(&mut self, detector: Box<dyn Detector>) {
        self.detectors.push(detector);
    }

    pub fn len(&self) -> usize {
        self.detectors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.detectors.is_empty()
    }

    pub fn names(&self) -> Vec<&str> {
        self.detectors.iter().map(|d| d.name()).collect()
    }

    /// Run every detector.
    pub fn detect_all(&self, context: &DetectionContext) -> Vec<Anomaly> {
        let mut anomalies: Vec<Anomaly> = self
            .detectors
            .iter()
            .flat_map(|detector| detector.detect(context))
            .collect();
        anomalies.sort_by(|a, b| {
            b.z_score
                .abs()
                .partial_cmp(&a.z_score.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.subject.cmp(&b.subject))
                .then_with(|| a.detector.cmp(&b.detector))
        });
        anomalies
    }

    /// Expected false positives per thousand observations, summed.
    ///
    /// The number an operator should look at before believing a busy queue.
    pub fn expected_noise_rate(&self) -> f64 {
        self.detectors
            .iter()
            .map(|d| d.expected_false_positive_rate())
            .sum()
    }
}

/// Fisher's z transform of a correlation.
fn fisher(correlation: f64) -> f64 {
    let r = correlation.clamp(-0.999_999, 0.999_999);
    0.5 * ((1.0 + r) / (1.0 - r)).ln()
}

/// Convert an object id string back into a typed id.
pub fn object_id_of(subject: &str) -> ObjectId {
    ObjectId::from_string(subject.to_string())
}

/// Default horizon for an anomaly kind.
pub fn horizon_for(kind: AnomalyKind) -> Duration {
    match kind {
        AnomalyKind::LiquidityDeterioration => Duration::from_days(2),
        AnomalyKind::PriceMove | AnomalyKind::VolumeSpike | AnomalyKind::SentimentShift => {
            Duration::from_days(5)
        }
        AnomalyKind::VolatilityShift | AnomalyKind::CorrelationBreakdown => Duration::from_days(20),
        AnomalyKind::FundamentalSurprise | AnomalyKind::AlternativeDataDivergence => {
            Duration::from_days(60)
        }
        AnomalyKind::StructuralBreak | AnomalyKind::RegimeChange | AnomalyKind::MacroSurprise => {
            Duration::from_days(90)
        }
    }
}
