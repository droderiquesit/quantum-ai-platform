use std::time::Duration;

/// Distribution types supported by the Forecast Lattice
#[derive(Debug, Clone)]
pub enum DistributionType {
    Normal,
    LogNormal,
    StudentT { df: f64 },
    Empirical { samples: Vec<f64> },
}

/// A calibrated probability distribution for a forecast
#[derive(Debug, Clone)]
pub struct ForecastDistribution {
    pub time_horizon: Duration,
    pub mean: f64,
    pub variance: f64,
    pub skewness: f64,
    pub kurtosis: f64,
    pub confidence_intervals: Vec<(f64, (f64, f64))>, // (confidence_level, (lower, upper))
    pub distribution_type: DistributionType,
}

impl ForecastDistribution {
    pub fn new(time_horizon: Duration, mean: f64, variance: f64) -> Self {
        Self {
            time_horizon,
            mean,
            variance,
            skewness: 0.0,
            kurtosis: 0.0,
            confidence_intervals: Vec::new(),
            distribution_type: DistributionType::Normal,
        }
    }

    pub fn with_skewness(mut self, skewness: f64) -> Self {
        self.skewness = skewness;
        self
    }

    pub fn with_kurtosis(mut self, kurtosis: f64) -> Self {
        self.kurtosis = kurtosis;
        self
    }

    pub fn std_dev(&self) -> f64 {
        self.variance.sqrt()
    }

    /// Add a confidence interval
    pub fn add_confidence_interval(&mut self, confidence_level: f64, interval: (f64, f64)) {
        // Remove existing entry if present
        self.confidence_intervals
            .retain(|(cl, _)| (cl - confidence_level).abs() > 1e-10);
        self.confidence_intervals.push((confidence_level, interval));
    }

    /// Compute confidence interval from normal distribution
    pub fn compute_normal_ci(&mut self, confidence_level: f64) {
        let z = self.z_score(confidence_level);
        let margin = z * self.std_dev();
        self.add_confidence_interval(confidence_level, (self.mean - margin, self.mean + margin));
    }

    /// Get z-score for normal distribution
    fn z_score(&self, confidence_level: f64) -> f64 {
        // Approximate z-scores for common confidence levels
        match confidence_level {
            x if (x - 0.68).abs() < 0.01 => 1.0,
            x if (x - 0.90).abs() < 0.01 => 1.645,
            x if (x - 0.95).abs() < 0.01 => 1.96,
            x if (x - 0.99).abs() < 0.01 => 2.576,
            _ => {
                // Approximate using inverse normal
                let p = (1.0 + confidence_level) / 2.0;
                approximated_z_score(p)
            }
        }
    }
}

/// Approximation of z-score using Taylor series
fn approximated_z_score(p: f64) -> f64 {
    if p < 0.5 {
        return -approximated_z_score(1.0 - p);
    }
    if p > 0.9999 {
        return 4.0; // Cap at reasonable value
    }

    let c = [2.515517, 0.802853, 0.010328, 1.432788, 0.189269, 0.001308];

    let t = (-(2.0 * (1.0 - p)).ln()).sqrt();
    let numerator = c[0] + c[1] * t + c[2] * t * t;
    let denominator = 1.0 + c[3] * t + c[4] * t * t + c[5] * t * t * t;

    t - numerator / denominator
}

/// Time scale definition
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TimeScale {
    pub name: String,
    pub duration: Duration,
}

impl TimeScale {
    pub fn microseconds(count: u64) -> Self {
        Self {
            name: format!("{}µs", count),
            duration: Duration::from_micros(count),
        }
    }

    pub fn milliseconds(count: u64) -> Self {
        Self {
            name: format!("{}ms", count),
            duration: Duration::from_millis(count),
        }
    }

    pub fn seconds(count: u64) -> Self {
        Self {
            name: format!("{}s", count),
            duration: Duration::from_secs(count),
        }
    }

    pub fn minutes(count: u64) -> Self {
        Self {
            name: format!("{}m", count),
            duration: Duration::from_secs(count * 60),
        }
    }

    pub fn hours(count: u64) -> Self {
        Self {
            name: format!("{}h", count),
            duration: Duration::from_secs(count * 3600),
        }
    }

    pub fn days(count: u64) -> Self {
        Self {
            name: format!("{}d", count),
            duration: Duration::from_secs(count * 86400),
        }
    }

    pub fn years(count: u64) -> Self {
        Self {
            name: format!("{}y", count),
            duration: Duration::from_secs(count * 31536000), // 365 days
        }
    }
}

/// Aggregation method for combining forecasts across time scales
pub enum AggregationMethod {
    Mean,
    Sum,
    Last,
}

/// Base probability forecaster
pub trait ProbabilityForecaster {
    fn forecast(&self, state_value: f64, horizon: Duration) -> (f64, f64); // (mean, variance)
    fn get_higher_moments(&self, horizon: Duration) -> (f64, f64); // (skewness, kurtosis)
}

/// Simple exponential growth forecaster
pub struct ExponentialForecaster {
    growth_rate: f64, // Annual growth rate
    volatility: f64,  // Annual volatility
}

impl ExponentialForecaster {
    pub fn new(growth_rate: f64, volatility: f64) -> Self {
        Self {
            growth_rate,
            volatility,
        }
    }
}

impl ProbabilityForecaster for ExponentialForecaster {
    fn forecast(&self, state_value: f64, horizon: Duration) -> (f64, f64) {
        let t = horizon.as_secs_f64() / (365.25 * 24.0 * 3600.0); // Convert to years

        // Geometric Brownian Motion: E[S_t] = S_0 * exp(mu * t)
        let mean = state_value * (self.growth_rate * t).exp();

        // Var[S_t] = S_0^2 * exp(2*mu*t) * (exp(sigma^2*t) - 1)
        let variance = state_value
            * state_value
            * (2.0 * self.growth_rate * t).exp()
            * ((self.volatility * self.volatility * t).exp() - 1.0);

        (mean, variance.max(1e-10))
    }

    fn get_higher_moments(&self, _horizon: Duration) -> (f64, f64) {
        // For log-normal distribution
        // Skewness and kurtosis depend on volatility
        let s = self.volatility;
        let skewness = (s * s + 2.0).sqrt() * (s * s + 3.0);
        let kurtosis = s.powi(8) + 6.0 * s.powi(6) + 15.0 * s.powi(4) + 16.0 * s.powi(2) + 3.0;
        (skewness, kurtosis)
    }
}

/// The Temporal Forecast Lattice
pub struct TemporalForecastLattice {
    forecaster: Box<dyn ProbabilityForecaster>,
    time_scales: Vec<TimeScale>,
}

impl TemporalForecastLattice {
    pub fn new(forecaster: Box<dyn ProbabilityForecaster>, time_scales: Vec<TimeScale>) -> Self {
        Self {
            forecaster,
            time_scales,
        }
    }

    /// Generate forecasts at all time scales
    pub fn forecast_at_scales(&self, current_state: f64) -> Vec<ForecastDistribution> {
        self.time_scales
            .iter()
            .map(|scale| {
                let (mean, variance) = self.forecaster.forecast(current_state, scale.duration);
                let (skewness, kurtosis) = self.forecaster.get_higher_moments(scale.duration);

                let mut dist = ForecastDistribution::new(scale.duration, mean, variance)
                    .with_skewness(skewness)
                    .with_kurtosis(kurtosis);

                // Add standard confidence intervals
                dist.compute_normal_ci(0.68);
                dist.compute_normal_ci(0.90);
                dist.compute_normal_ci(0.95);
                dist.compute_normal_ci(0.99);

                dist
            })
            .collect()
    }

    /// Ensure consistency across time scales
    pub fn ensure_consistency(
        &self,
        mut forecasts: Vec<ForecastDistribution>,
    ) -> Vec<ForecastDistribution> {
        // For geometric Brownian motion, consistency is maintained through the model
        // This is a validation step that can check relationships

        // Sort by time horizon
        forecasts.sort_by_key(|f| f.time_horizon.as_secs());

        // Check that variance increases monotonically
        for i in 1..forecasts.len() {
            if forecasts[i].variance < forecasts[i - 1].variance {
                // Ensure monotonic increase in uncertainty
                forecasts[i].variance = forecasts[i - 1].variance * 1.1;
            }
        }

        forecasts
    }

    /// Aggregate fine-scale forecasts to coarser scale
    pub fn aggregate_forecast(
        &self,
        fine_forecasts: Vec<f64>,
        method: AggregationMethod,
    ) -> ForecastDistribution {
        let aggregated = match method {
            AggregationMethod::Mean => {
                let sum: f64 = fine_forecasts.iter().sum();
                sum / fine_forecasts.len() as f64
            }
            AggregationMethod::Sum => fine_forecasts.iter().sum(),
            AggregationMethod::Last => fine_forecasts.last().copied().unwrap_or(0.0),
        };

        ForecastDistribution::new(Duration::from_secs(0), aggregated, 0.1)
    }
}

/// Calibration metrics for forecast validation
#[derive(Debug, Clone)]
pub struct CalibrationMetrics {
    pub mean_coverage_error: f64,
    pub crps_mean: f64,
    pub calibration_slope: f64,
    pub sample_count: usize,
}

impl CalibrationMetrics {
    pub fn new() -> Self {
        Self {
            mean_coverage_error: 0.0,
            crps_mean: 0.0,
            calibration_slope: 0.0,
            sample_count: 0,
        }
    }
}

/// Calibration validator
pub struct CalibrationValidator;

impl CalibrationValidator {
    /// Validate that forecasts are properly calibrated
    pub fn validate_calibration(
        forecasts: &[ForecastDistribution],
        outcomes: &[f64],
    ) -> CalibrationMetrics {
        let mut metrics = CalibrationMetrics::new();
        metrics.sample_count = outcomes.len();

        if outcomes.is_empty() {
            return metrics;
        }

        // Compute coverage for each confidence level
        let mut coverage_errors = Vec::new();
        if !forecasts.is_empty() {
            for &(conf_level, _) in &forecasts[0].confidence_intervals {
                let mut hits = 0;
                for (forecast, outcome) in forecasts.iter().zip(outcomes.iter()) {
                    if let Some((lower, upper)) = forecast
                        .confidence_intervals
                        .iter()
                        .find(|(cl, _)| (cl - conf_level).abs() < 1e-10)
                        .map(|(_, interval)| interval)
                    {
                        if outcome >= lower && outcome <= upper {
                            hits += 1;
                        }
                    }
                }
                let actual_coverage = hits as f64 / outcomes.len() as f64;
                let coverage_error = (actual_coverage - conf_level).abs();
                coverage_errors.push(coverage_error);
            }
        }

        if !coverage_errors.is_empty() {
            metrics.mean_coverage_error =
                coverage_errors.iter().sum::<f64>() / coverage_errors.len() as f64;
        }

        // Compute CRPS (Continuous Ranked Probability Score)
        let mut crps_sum = 0.0;
        for (forecast, outcome) in forecasts.iter().zip(outcomes.iter()) {
            crps_sum += Self::compute_crps(forecast, *outcome);
        }
        metrics.crps_mean = crps_sum / outcomes.len() as f64;

        metrics
    }

    /// Compute CRPS for a single forecast-outcome pair
    fn compute_crps(forecast: &ForecastDistribution, outcome: f64) -> f64 {
        // For normal distribution, CRPS ≈ σ * (z/√π + 2*Φ(z) - 1)
        // where z = (x - μ) / σ and Φ is CDF
        let z = (outcome - forecast.mean) / forecast.std_dev();
        let std_dev = forecast.std_dev();

        let cdf_z = normal_cdf(z);
        std_dev
            * (z * (2.0 * cdf_z - 1.0) + 2.0 * normal_pdf(z) - 1.0 / (std::f64::consts::PI).sqrt())
    }

    /// Compute calibration slope via regression
    pub fn calibration_slope(forecasts: &[ForecastDistribution], outcomes: &[f64]) -> f64 {
        if outcomes.len() < 2 {
            return 1.0;
        }

        // Simple linear regression: outcome vs forecast mean
        let n = outcomes.len() as f64;
        let mean_forecast: f64 = forecasts.iter().map(|f| f.mean).sum::<f64>() / n;
        let mean_outcome: f64 = outcomes.iter().sum::<f64>() / n;

        let numerator: f64 = forecasts
            .iter()
            .zip(outcomes.iter())
            .map(|(f, o)| (f.mean - mean_forecast) * (o - mean_outcome))
            .sum();

        let denominator: f64 = forecasts
            .iter()
            .map(|f| (f.mean - mean_forecast).powi(2))
            .sum();

        if denominator.abs() < 1e-10 {
            1.0
        } else {
            numerator / denominator
        }
    }
}

/// Normal distribution CDF approximation (Abramowitz and Stegun)
fn normal_cdf(z: f64) -> f64 {
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let sign = if z < 0.0 { -1.0 } else { 1.0 };
    let z = z.abs();

    let t = 1.0 / (1.0 + p * z);
    let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-z * z).exp();

    0.5 * (1.0 + sign * y)
}

/// Normal distribution PDF
fn normal_pdf(z: f64) -> f64 {
    (-z * z / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_scale_creation() {
        let scales = vec![
            TimeScale::milliseconds(100),
            TimeScale::seconds(1),
            TimeScale::minutes(5),
            TimeScale::hours(1),
            TimeScale::days(1),
            TimeScale::years(1),
        ];

        assert_eq!(scales.len(), 6);
        assert!(scales[0].duration < scales[1].duration);
    }

    #[test]
    fn test_forecast_distribution() {
        let mut dist = ForecastDistribution::new(Duration::from_secs(1), 100.0, 10.0);
        dist.compute_normal_ci(0.95);

        let (lower, upper) = dist
            .confidence_intervals
            .iter()
            .find(|(cl, _)| (cl - 0.95).abs() < 1e-10)
            .map(|(_, interval)| interval)
            .copied()
            .unwrap();
        assert!(lower < 100.0);
        assert!(upper > 100.0);
    }

    #[test]
    fn test_exponential_forecaster() {
        let forecaster = ExponentialForecaster::new(0.05, 0.2); // 5% growth, 20% volatility

        let (mean, variance) = forecaster.forecast(100.0, Duration::from_secs(86400)); // 1 day
        assert!(mean > 100.0); // Positive drift
        assert!(variance > 0.0);
    }

    #[test]
    fn test_temporal_forecast_lattice() {
        let forecaster = Box::new(ExponentialForecaster::new(0.05, 0.2));
        let scales = vec![
            TimeScale::seconds(1),
            TimeScale::minutes(1),
            TimeScale::hours(1),
        ];

        let lattice = TemporalForecastLattice::new(forecaster, scales);
        let forecasts = lattice.forecast_at_scales(100.0);

        assert_eq!(forecasts.len(), 3);

        // Longer horizons should have higher variance
        for i in 1..forecasts.len() {
            assert!(forecasts[i].variance >= forecasts[i - 1].variance * 0.9);
        }
    }

    #[test]
    fn test_calibration_metrics() {
        let forecasts = vec![
            ForecastDistribution::new(Duration::from_secs(1), 100.0, 10.0),
            ForecastDistribution::new(Duration::from_secs(1), 105.0, 12.0),
            ForecastDistribution::new(Duration::from_secs(1), 98.0, 11.0),
        ];

        let outcomes = vec![101.0, 104.0, 99.0];
        let metrics = CalibrationValidator::validate_calibration(&forecasts, &outcomes);

        assert!(metrics.mean_coverage_error >= 0.0);
        assert!(metrics.crps_mean > 0.0);
        assert_eq!(metrics.sample_count, 3);
    }

    #[test]
    fn test_consistency_across_scales() {
        let forecaster = Box::new(ExponentialForecaster::new(0.05, 0.2));
        let scales = vec![
            TimeScale::seconds(1),
            TimeScale::seconds(10),
            TimeScale::minutes(1),
        ];

        let lattice = TemporalForecastLattice::new(forecaster, scales);
        let mut forecasts = lattice.forecast_at_scales(100.0);
        forecasts = lattice.ensure_consistency(forecasts);

        // Verify variance is monotonically increasing
        for i in 1..forecasts.len() {
            assert!(forecasts[i].variance >= forecasts[i - 1].variance * 0.99);
        }
    }
}
