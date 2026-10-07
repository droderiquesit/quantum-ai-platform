pub mod causal;
pub mod forecast_lattice;
/// World Model & Data Implementation
///
/// This library provides two core components for the world model:
///
/// 1. NOW Brain (WORLD-067): Latent state estimation with uncertainty quantification
/// 2. Temporal Forecast Lattice (WORLD-069): Multi-scale probabilistic forecasting
///
/// Both components work together to provide a complete probabilistic world model.
pub mod now_brain;

pub use forecast_lattice::{
    CalibrationMetrics, CalibrationValidator, ExponentialForecaster, ForecastDistribution,
    ProbabilityForecaster, TemporalForecastLattice, TimeScale,
};
pub use now_brain::{Freshness, KalmanFilter, LatentStateVector, NOWBrain, Observation};
