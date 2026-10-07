use ndarray::{Array1, Array2};
use std::time::{Duration, Instant};

/// Represents a single state dimension estimate
#[derive(Debug, Clone)]
pub struct StateEstimate {
    pub name: String,
    pub value: f64,
    pub variance: f64,
}

/// Uncertainty matrix (covariance) with confidence level
#[derive(Debug, Clone)]
pub struct UncertaintyMatrix {
    pub covariance: Array2<f64>,
    pub confidence_level: f64,
}

impl UncertaintyMatrix {
    pub fn new(dim: usize, confidence_level: f64) -> Self {
        Self {
            covariance: Array2::<f64>::eye(dim),
            confidence_level,
        }
    }

    pub fn get_confidence_interval(&self, idx: usize, z_score: f64) -> (f64, f64) {
        let variance = self.covariance[[idx, idx]];
        let std_dev = variance.sqrt();
        (-z_score * std_dev, z_score * std_dev)
    }
}

/// Freshness tracking for latent state
#[derive(Debug, Clone)]
pub struct Freshness {
    pub last_observation: Instant,
    pub lag_estimate: Duration,
    pub stale_factor: f64,
}

impl Freshness {
    pub fn new() -> Self {
        Self {
            last_observation: Instant::now(),
            lag_estimate: Duration::from_secs(0),
            stale_factor: 0.01, // 1% uncertainty increase per second
        }
    }

    pub fn update_observation(&mut self) {
        self.last_observation = Instant::now();
    }

    pub fn age(&self) -> Duration {
        Instant::now().duration_since(self.last_observation)
    }

    pub fn uncertainty_multiplier(&self) -> f64 {
        let age_secs = self.age().as_secs_f64();
        1.0 + (self.stale_factor * age_secs)
    }
}

/// Latent state vector with uncertainty quantification
#[derive(Debug, Clone)]
pub struct LatentStateVector {
    pub timestamp: Instant,
    pub state_dims: Vec<StateEstimate>,
    pub uncertainty: UncertaintyMatrix,
    pub freshness: Freshness,
}

impl LatentStateVector {
    pub fn new(dim: usize) -> Self {
        Self {
            timestamp: Instant::now(),
            state_dims: vec![],
            uncertainty: UncertaintyMatrix::new(dim, 0.95),
            freshness: Freshness::new(),
        }
    }

    pub fn get_confidence_intervals(&self, z_score: f64) -> Vec<(f64, f64)> {
        (0..self.state_dims.len())
            .map(|i| self.uncertainty.get_confidence_interval(i, z_score))
            .collect()
    }
}

/// Observation data for state estimation
#[derive(Debug, Clone)]
pub struct Observation {
    pub timestamp: Instant,
    pub values: Vec<f64>,
    pub measurement_variance: Vec<f64>, // Measurement noise for each dimension
}

/// Simple Kalman Filter implementation
pub struct KalmanFilter {
    // State transition matrix (A) - identity for simplicity
    state_dim: usize,
    // Process noise covariance (Q)
    process_noise: Array2<f64>,
    // Measurement model (H) - identity for full observation
    measurement_model: Array2<f64>,
    // Current state estimate
    state: Array1<f64>,
    // Current covariance estimate
    covariance: Array2<f64>,
}

impl KalmanFilter {
    pub fn new(state_dim: usize, process_noise_std: f64) -> Self {
        let process_noise = Array2::<f64>::eye(state_dim) * (process_noise_std * process_noise_std);
        let measurement_model = Array2::<f64>::eye(state_dim);

        Self {
            state_dim,
            process_noise,
            measurement_model,
            state: Array1::<f64>::zeros(state_dim),
            covariance: Array2::<f64>::eye(state_dim),
        }
    }

    /// Prediction step
    pub fn predict(&mut self, dt: Duration) {
        let dt_secs = dt.as_secs_f64();

        // State transition: x = x (constant model)
        // Covariance prediction: P = P + Q
        self.covariance = self.covariance.clone() + &self.process_noise * dt_secs;
    }

    /// Update step with observation
    pub fn update(&mut self, observation: &Observation) {
        let z = Array1::from_vec(observation.values.clone());
        let measurement_noise =
            Array2::from_diag(&Array1::from_vec(observation.measurement_variance.clone()));

        // Innovation: y = z - H*x
        let innovation = z - self.measurement_model.dot(&self.state);

        // Innovation covariance: S = H*P*H^T + R
        let s = self
            .measurement_model
            .dot(&self.covariance)
            .dot(&self.measurement_model.t())
            + &measurement_noise;

        // Kalman gain: K = P*H^T*S^-1
        let p_ht = self.covariance.dot(&self.measurement_model.t());
        let s_inv = self.invert_2d(&s);
        let kalman_gain = p_ht.dot(&s_inv);

        // State update: x = x + K*y
        self.state = self.state.clone() + kalman_gain.dot(&innovation);

        // Covariance update: P = (I - K*H)*P
        let identity = Array2::<f64>::eye(self.state_dim);
        let kh = kalman_gain.dot(&self.measurement_model);
        self.covariance = (&identity - &kh).dot(&self.covariance);
    }

    /// Simple 2D matrix inversion using Gaussian elimination
    fn invert_2d(&self, mat: &Array2<f64>) -> Array2<f64> {
        let n = mat.nrows();
        let mut aug = mat.clone();
        let mut inv = Array2::<f64>::eye(n);

        // Forward elimination
        for i in 0..n {
            let pivot = aug[[i, i]];
            if pivot.abs() < 1e-10 {
                continue;
            }

            for j in 0..n {
                aug[[i, j]] /= pivot;
                inv[[i, j]] /= pivot;
            }

            for k in 0..n {
                if k != i {
                    let factor = aug[[k, i]];
                    for j in 0..n {
                        aug[[k, j]] -= factor * aug[[i, j]];
                        inv[[k, j]] -= factor * inv[[i, j]];
                    }
                }
            }
        }

        inv
    }

    pub fn get_state(&self) -> Array1<f64> {
        self.state.clone()
    }

    pub fn get_covariance(&self) -> Array2<f64> {
        self.covariance.clone()
    }
}

/// NOW Brain - Latent state estimator with uncertainty
pub struct NOWBrain {
    kalman_filter: KalmanFilter,
    current_state: LatentStateVector,
    dimension_names: Vec<String>,
}

impl NOWBrain {
    pub fn new(dimension_names: Vec<String>, process_noise_std: f64) -> Self {
        let dim = dimension_names.len();
        let mut state = LatentStateVector::new(dim);

        for name in &dimension_names {
            state.state_dims.push(StateEstimate {
                name: name.clone(),
                value: 0.0,
                variance: 1.0,
            });
        }

        Self {
            kalman_filter: KalmanFilter::new(dim, process_noise_std),
            current_state: state,
            dimension_names,
        }
    }

    pub fn process_observation(&mut self, observation: Observation) {
        // Predict step
        if self.current_state.timestamp != Instant::now() {
            let dt = Instant::now().duration_since(self.current_state.timestamp);
            self.kalman_filter.predict(dt);
        }

        // Update step
        self.kalman_filter.update(&observation);

        // Update state from filter
        let state_vec = self.kalman_filter.get_state();
        let cov = self.kalman_filter.get_covariance();

        for (i, estimate) in self.current_state.state_dims.iter_mut().enumerate() {
            estimate.value = state_vec[i];
            estimate.variance = cov[[i, i]];
        }

        self.current_state.timestamp = Instant::now();
        self.current_state.uncertainty.covariance = cov;
        self.current_state.freshness.update_observation();
    }

    pub fn get_current_state(&self) -> &LatentStateVector {
        &self.current_state
    }

    pub fn get_state_with_freshness_uncertainty(&self) -> LatentStateVector {
        let mut state = self.current_state.clone();

        // Apply freshness-based uncertainty increase
        let multiplier = state.freshness.uncertainty_multiplier();
        for estimate in &mut state.state_dims {
            estimate.variance *= multiplier;
        }

        state.uncertainty.covariance = state.uncertainty.covariance.clone() * multiplier;
        state
    }

    pub fn predict_state(&mut self, horizon: Duration) {
        self.kalman_filter.predict(horizon);
    }

    pub fn get_dimensions(&self) -> &[String] {
        &self.dimension_names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kalman_filter_convergence() {
        let mut kf = KalmanFilter::new(2, 0.1);
        kf.state = Array1::from_vec(vec![1.0, 2.0]);

        for _ in 0..10 {
            let obs = Observation {
                timestamp: Instant::now(),
                values: vec![1.0, 2.0],
                measurement_variance: vec![0.01, 0.01],
            };
            kf.update(&obs);
        }

        let state = kf.get_state();
        assert!((state[0] - 1.0).abs() < 0.1);
        assert!((state[1] - 2.0).abs() < 0.1);
    }

    #[test]
    fn test_now_brain_state_estimation() {
        let mut brain = NOWBrain::new(
            vec!["market_level".to_string(), "volatility".to_string()],
            0.1,
        );

        let obs = Observation {
            timestamp: Instant::now(),
            values: vec![100.0, 0.2],
            measurement_variance: vec![1.0, 0.01],
        };

        brain.process_observation(obs);
        let state = brain.get_current_state();

        assert_eq!(state.state_dims.len(), 2);
        assert!(state.state_dims[0].variance > 0.0);
    }

    #[test]
    fn test_freshness_uncertainty_increases_over_time() {
        let mut fresh = Freshness::new();
        fresh.stale_factor = 0.1;

        let mult1 = fresh.uncertainty_multiplier();
        std::thread::sleep(Duration::from_millis(100));
        let mult2 = fresh.uncertainty_multiplier();

        assert!(mult2 > mult1);
    }

    #[test]
    fn test_confidence_intervals() {
        let mut brain = NOWBrain::new(vec!["dim1".to_string()], 0.1);
        let obs = Observation {
            timestamp: Instant::now(),
            values: vec![5.0],
            measurement_variance: vec![0.1],
        };
        brain.process_observation(obs);

        let state = brain.get_current_state();
        let intervals = state.get_confidence_intervals(1.96); // 95% CI

        assert_eq!(intervals.len(), 1);
        assert!(intervals[0].0 < 0.0 && intervals[0].1 > 0.0);
    }
}
