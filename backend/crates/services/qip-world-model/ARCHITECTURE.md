# World Model & Data - Architecture Design

## Overview

This document describes the architecture for two P0 blocker components:
- **WORLD-067: NOW Brain** - Latent state estimation with uncertainty
- **WORLD-069: Temporal Forecast Lattice** - Multi-scale probabilistic forecasting

## WORLD-067: NOW Brain

### Purpose
Estimate the latent global/market/system state with uncertainty quantification. The NOW Brain maintains a continuously-updated belief about the current state of relevant dimensions, accounting for observation lag and providing confidence bounds.

### Core Components

#### 1. LatentStateVector
Represents the estimated latent state at a point in time.

```
LatentStateVector {
  timestamp: Instant,
  state_dims: Vec<StateEstimate>,
  uncertainty: UncertaintyMatrix,
  freshness: Freshness,
}

StateEstimate {
  name: String,
  value: f64,
  variance: f64,  // Uncertainty for this dimension
}

UncertaintyMatrix {
  // Covariance matrix capturing cross-dimensional uncertainty
  covariance: Matrix,
  confidence_level: f64,  // e.g., 0.95 for 95% CI
}

Freshness {
  last_observation: Instant,
  lag_estimate: Duration,
  stale_factor: f64,  // How much to increase uncertainty over time
}
```

#### 2. ObservationProcessor
Integrates new observations and updates latent state estimate.

```
impl ObservationProcessor {
  fn process_observation(&mut self, obs: Observation) -> StateUpdate,
  fn kalman_filter_step(&mut self, obs: Observation) -> StateEstimate,
  fn uncertainty_decay(&mut self),  // Increases uncertainty over time
}
```

#### 3. StateEstimator (Kalman Filter based)
Uses Kalman filtering to estimate latent state from noisy observations.

```
impl StateEstimator {
  fn predict(&self, dt: Duration) -> (StateVector, UncertaintyMatrix),
  fn update(&mut self, observation: Observation) -> StateVector,
  fn get_confidence_interval(&self, confidence: f64) -> Vec<(f64, f64)>,
}
```

### Integration Points
- Input: Time-series observations from various data sources
- Output: Latent state estimates with uncertainty bounds
- Connects to: qip-world-model, data ingestion pipeline

### Test Strategy
1. **Unit tests**: Kalman filter convergence with synthetic data
2. **Integration tests**: Multiple observation types, uncertainty propagation
3. **Validation tests**: Distributions improve with more observations

---

## WORLD-069: Temporal Forecast Lattice

### Purpose
Generate calibrated probability distributions across multiple time scales (microseconds to years). The Forecast Lattice ensures forecasts at different time horizons are internally consistent and properly calibrated.

### Core Components

#### 1. TimeScale Hierarchy
Defines the multi-scale structure for forecasting.

```
TimeScale {
  name: String,
  unit: Duration,  // e.g., 1 second, 1 minute, 1 hour, 1 day, 1 year
  aggregation_method: AggregationMethod,
}

enum AggregationMethod {
  Mean,
  Sum,
  Last,
  Custom(Fn(Vec<f64>) -> f64),
}

TimeScaleLattice {
  scales: Vec<TimeScale>,
  relationships: Vec<ScaleRelationship>,  // How scales relate to each other
}
```

#### 2. ForecastDistribution
Represents a calibrated probability distribution for a forecast.

```
ForecastDistribution {
  time_horizon: Duration,
  mean: f64,
  variance: f64,
  skewness: f64,  // Capture non-normality
  kurtosis: f64,
  confidence_intervals: Vec<(f64, (f64, f64))>,  // (confidence_level, (lower, upper))
  distribution_type: DistributionType,  // Normal, LogNormal, StudentT, etc.
}

enum DistributionType {
  Normal,
  LogNormal,
  StudentT { df: f64 },
  Empirical { samples: Vec<f64> },
}
```

#### 3. ForecastLattice
Produces forecasts across time scales with consistency constraints.

```
impl ForecastLattice {
  fn forecast_at_scales(
    &self, 
    current_state: &StateVector,
    scales: Vec<TimeScale>
  ) -> Vec<ForecastDistribution>,
  
  fn ensure_consistency(&self, forecasts: Vec<ForecastDistribution>) -> Vec<ForecastDistribution>,
  
  fn aggregate_forecast(&self, 
    fine_forecasts: Vec<f64>,
    aggregation: AggregationMethod
  ) -> ForecastDistribution,
}
```

#### 4. CalibrationValidator
Validates that forecasts are properly calibrated.

```
impl CalibrationValidator {
  fn validate_calibration(
    &self,
    forecasts: Vec<ForecastDistribution>,
    outcomes: Vec<Outcome>,
  ) -> CalibrationMetrics,
  
  fn compute_coverage(&self, 
    forecasts: Vec<ForecastDistribution>,
    outcomes: Vec<Outcome>,
  ) -> HashMap<f64, f64>,  // confidence_level -> actual_coverage
  
  fn compute_crps(&self, 
    forecast_dist: &ForecastDistribution,
    outcome: f64,
  ) -> f64,
}

struct CalibrationMetrics {
  mean_coverage_error: f64,
  crps_mean: f64,
  calibration_slope: f64,
}
```

#### 5. ProbabilityForecaster (Base component)
Generates base forecasts that are then aggregated across scales.

```
impl ProbabilityForecaster {
  fn forecast(&self, 
    state: &StateVector,
    horizon: Duration,
  ) -> (f64, f64),  // (mean, variance)
  
  fn get_higher_moments(&self, 
    horizon: Duration,
  ) -> (f64, f64),  // (skewness, kurtosis)
}
```

### Integration Points
- Input: Current latent state from NOW Brain, historical forecasts
- Output: Calibrated distributions at multiple time scales
- Connects to: qip-world-model, decision systems, backtesting

### Test Strategy
1. **Unit tests**: Aggregation consistency, distribution properties
2. **Calibration tests**: Synthetic forecasts with known outcomes
3. **Validation tests**: Coverage matches confidence levels
4. **Integration tests**: Consistency across time scales

---

## Data Flow

```
Observations → NOW Brain → Latent State → Temporal Forecast Lattice → Calibrated Distributions
              (WORLD-067)   (State Vector)      (WORLD-069)           (Multi-scale forecasts)
                ↓                                                              ↓
         Uncertainty Bounds                                         Validation & Metrics
```

## Integration Strategy

1. **Phase 1**: Implement NOW Brain with basic Kalman filtering
2. **Phase 2**: Implement Temporal Forecast Lattice with synthetic data
3. **Phase 3**: Connect to existing qip-world-model infrastructure
4. **Phase 4**: Add data ingestion and calibration feedback loops

## Success Criteria

### WORLD-067
- [ ] NOW Brain produces state estimates with proper uncertainty quantification
- [ ] Uncertainty decreases with more observations (convergence)
- [ ] Freshness tracking prevents stale state propagation
- [ ] Integration with existing world model data sources

### WORLD-069
- [ ] Forecasts are properly calibrated (coverage matches confidence)
- [ ] Consistency maintained across time scales
- [ ] Multi-scale aggregation is mathematically sound
- [ ] CRPS and calibration metrics < target thresholds

