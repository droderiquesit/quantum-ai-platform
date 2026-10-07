use qip_world_model::{
    CalibrationValidator, ExponentialForecaster, NOWBrain, Observation, TemporalForecastLattice,
    TimeScale,
};
use std::time::Duration;
use std::time::Instant;

#[test]
fn test_now_brain_processes_multiple_observations() {
    let mut brain = NOWBrain::new(
        vec!["market_level".to_string(), "volatility".to_string()],
        0.1,
    );

    let base_time = Instant::now();

    // Process sequence of observations
    for i in 0..5 {
        let obs = Observation {
            timestamp: base_time,
            values: vec![100.0 + i as f64, 0.2],
            measurement_variance: vec![1.0, 0.01],
        };
        brain.process_observation(obs);
    }

    let state = brain.get_current_state();

    // State should be updating (not stuck at zero)
    assert!(state.state_dims[0].value != 0.0);
    // Variance should decrease with observations
    assert!(state.state_dims[0].variance < 1.0);
}

#[test]
fn test_now_brain_freshness_tracking() {
    let mut brain = NOWBrain::new(vec!["price".to_string()], 0.1);

    let obs = Observation {
        timestamp: Instant::now(),
        values: vec![100.0],
        measurement_variance: vec![1.0],
    };

    brain.process_observation(obs);

    let fresh_state = brain.get_current_state();
    let initial_variance = fresh_state.state_dims[0].variance;

    // Simulate time passing
    std::thread::sleep(Duration::from_millis(10));

    let stale_state = brain.get_state_with_freshness_uncertainty();
    let stale_variance = stale_state.state_dims[0].variance;

    // Variance should increase due to freshness staleness
    assert!(stale_variance > initial_variance);
}

#[test]
fn test_forecast_lattice_multi_scale() {
    let forecaster = Box::new(ExponentialForecaster::new(0.05, 0.2));
    let scales = vec![
        TimeScale::milliseconds(100),
        TimeScale::seconds(1),
        TimeScale::seconds(10),
        TimeScale::minutes(1),
        TimeScale::hours(1),
        TimeScale::days(1),
        TimeScale::years(1),
    ];

    let lattice = TemporalForecastLattice::new(forecaster, scales);
    let forecasts = lattice.forecast_at_scales(100.0);

    // Should have forecasts at all scales
    assert_eq!(forecasts.len(), 7);

    // Variance should increase with horizon
    for i in 1..forecasts.len() {
        assert!(forecasts[i].variance >= forecasts[i - 1].variance * 0.9);
    }

    // All should be positive mean for positive growth rate
    for forecast in &forecasts {
        assert!(forecast.mean > 0.0);
    }
}

#[test]
fn test_confidence_intervals_valid() {
    let forecaster = Box::new(ExponentialForecaster::new(0.05, 0.2));
    let scales = vec![TimeScale::hours(1)];

    let lattice = TemporalForecastLattice::new(forecaster, scales);
    let forecasts = lattice.forecast_at_scales(100.0);

    let forecast = &forecasts[0];

    // Check that confidence intervals are properly ordered
    let ci_95 = forecast
        .confidence_intervals
        .iter()
        .find(|(cl, _)| (cl - 0.95).abs() < 1e-10)
        .map(|(_, interval)| interval)
        .unwrap();
    let ci_99 = forecast
        .confidence_intervals
        .iter()
        .find(|(cl, _)| (cl - 0.99).abs() < 1e-10)
        .map(|(_, interval)| interval)
        .unwrap();

    // Wider confidence should have wider interval
    assert!(ci_99.1 - ci_99.0 > ci_95.1 - ci_95.0);

    // Both should contain the mean
    assert!(ci_95.0 < forecast.mean && forecast.mean < ci_95.1);
    assert!(ci_99.0 < forecast.mean && forecast.mean < ci_99.1);
}

#[test]
fn test_calibration_validation() {
    // Create forecasts
    let forecasts = vec![
        {
            let mut f =
                qip_world_model::ForecastDistribution::new(Duration::from_secs(1), 100.0, 25.0);
            f.compute_normal_ci(0.95);
            f
        },
        {
            let mut f =
                qip_world_model::ForecastDistribution::new(Duration::from_secs(1), 105.0, 30.0);
            f.compute_normal_ci(0.95);
            f
        },
        {
            let mut f =
                qip_world_model::ForecastDistribution::new(Duration::from_secs(1), 98.0, 24.0);
            f.compute_normal_ci(0.95);
            f
        },
        {
            let mut f =
                qip_world_model::ForecastDistribution::new(Duration::from_secs(1), 102.0, 26.0);
            f.compute_normal_ci(0.95);
            f
        },
    ];

    // Simulate outcomes
    let outcomes = vec![101.0, 103.0, 99.0, 104.0];

    let metrics = CalibrationValidator::validate_calibration(&forecasts, &outcomes);

    assert!(metrics.mean_coverage_error >= 0.0);
    assert!(metrics.mean_coverage_error <= 1.0);
    assert!(metrics.crps_mean > 0.0);
    assert_eq!(metrics.sample_count, 4);
}

#[test]
fn test_integration_now_brain_to_forecast_lattice() {
    // Step 1: Use NOW Brain to estimate current state
    let mut brain = NOWBrain::new(
        vec!["market_price".to_string(), "volatility".to_string()],
        0.1,
    );

    // Feed observations
    for i in 0..10 {
        let obs = Observation {
            timestamp: Instant::now(),
            values: vec![100.0 + i as f64 * 0.5, 0.15 + i as f64 * 0.01],
            measurement_variance: vec![1.0, 0.001],
        };
        brain.process_observation(obs);
    }

    // Get current state estimate
    let current_state = brain.get_current_state();
    let current_price = current_state.state_dims[0].value;
    let current_volatility = current_state.state_dims[1].value;

    // Step 2: Use Forecast Lattice to generate multi-scale forecasts
    let forecaster = Box::new(ExponentialForecaster::new(0.05, current_volatility));
    let scales = vec![
        TimeScale::seconds(10),
        TimeScale::minutes(1),
        TimeScale::hours(1),
        TimeScale::days(1),
    ];

    let lattice = TemporalForecastLattice::new(forecaster, scales);
    let forecasts = lattice.forecast_at_scales(current_price);

    // Verify forecasts
    assert_eq!(forecasts.len(), 4);
    for forecast in &forecasts {
        assert!(forecast.mean > 0.0);
        assert!(forecast.variance > 0.0);
        assert!(!forecast.confidence_intervals.is_empty());
    }

    // Verify monotonic uncertainty increase
    for i in 1..forecasts.len() {
        assert!(forecasts[i].variance >= forecasts[i - 1].variance * 0.9);
    }
}

#[test]
fn test_consistency_across_aggregation_levels() {
    // Create fine-grained scales
    let fine_scales = vec![
        TimeScale::seconds(1),
        TimeScale::seconds(2),
        TimeScale::seconds(3),
        TimeScale::seconds(4),
        TimeScale::seconds(5),
    ];

    let fine_lattice =
        TemporalForecastLattice::new(Box::new(ExponentialForecaster::new(0.05, 0.2)), fine_scales);

    let fine_forecasts = fine_lattice.forecast_at_scales(100.0);

    // Create coarse scales
    let coarse_scales = vec![
        TimeScale::seconds(10),
        TimeScale::seconds(20),
        TimeScale::minutes(1),
    ];

    let coarse_lattice = TemporalForecastLattice::new(
        Box::new(ExponentialForecaster::new(0.05, 0.2)),
        coarse_scales,
    );

    let coarse_forecasts = coarse_lattice.forecast_at_scales(100.0);

    // Verify that coarse forecasts have larger horizons and higher variance
    for coarse in &coarse_forecasts {
        for fine in &fine_forecasts {
            if coarse.time_horizon > fine.time_horizon {
                assert!(coarse.variance >= fine.variance * 0.9);
            }
        }
    }
}

#[test]
fn test_world_model_end_to_end() {
    // Simulate a complete world model update cycle

    // 1. Create NOW Brain
    let mut brain = NOWBrain::new(
        vec![
            "price".to_string(),
            "volume".to_string(),
            "volatility".to_string(),
        ],
        0.1,
    );

    // 2. Feed market data
    let market_observations = vec![
        (100.0, 1000.0, 0.15),
        (101.5, 1100.0, 0.16),
        (100.8, 950.0, 0.14),
        (102.1, 1050.0, 0.17),
    ];

    for (price, volume, vol) in market_observations {
        let obs = Observation {
            timestamp: Instant::now(),
            values: vec![price, volume, vol],
            measurement_variance: vec![0.5, 100.0, 0.01],
        };
        brain.process_observation(obs);
    }

    // 3. Get current state
    let current = brain.get_current_state();
    let price = current.state_dims[0].value;
    let volume = current.state_dims[1].value;
    let volatility = current.state_dims[2].value;

    // 4. Generate forecasts
    let forecaster = Box::new(ExponentialForecaster::new(0.02, volatility));
    let scales = vec![
        TimeScale::minutes(1),
        TimeScale::hours(1),
        TimeScale::days(1),
    ];

    let lattice = TemporalForecastLattice::new(forecaster, scales);
    let forecasts = lattice.forecast_at_scales(price);

    // 5. Validate
    assert!(price > 0.0);
    assert!(volume > 0.0);
    assert!(volatility > 0.0);
    assert_eq!(forecasts.len(), 3);

    for forecast in &forecasts {
        assert!(forecast.mean > 0.0);
        assert!(forecast.variance > 0.0);
    }
}
