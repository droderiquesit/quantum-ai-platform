//! A return forecast for tests that build an `ApprovedThesis` by hand
//! (R12-WORLD-10187). Compiled only under `cfg(test)`: production theses get
//! their forecast from `Platform::return_forecast`, never from here.

use qip_contracts::forecast::{
    Calibration, Forecast, ForecastFields, OutcomeDistribution, QuantilePoint, Stated, TailMass,
    TailSide, UncertaintyDecomposition,
};
use qip_contracts::validated::Contract;
use qip_core::{Decimal, Duration, Timestamp};

/// A forecast of `object`'s return with its median at `median`, a spread of
/// five points either side, issued at the epoch and expiring far beyond any
/// fixture's clock.
pub(crate) fn return_forecast(object: &str, median: f64) -> Forecast {
    let at = |v: f64| Decimal::from_f64(v).expect("a fixture return is representable");
    Forecast::new(ForecastFields {
        schema_version: ForecastFields::SCHEMA_VERSION,
        forecast_id: format!("fixture/{object}/return"),
        model: "fixture".to_string(),
        target: format!(
            "{object}{}",
            qip_portfolio_engine::construction::RETURN_TARGET_SUFFIX
        ),
        issued_at: Timestamp::EPOCH,
        horizon: Duration::from_days(5),
        distribution: OutcomeDistribution::Quantiles {
            points: vec![
                QuantilePoint {
                    quantile_bp: 1_000,
                    value: at(median - 0.05),
                },
                QuantilePoint {
                    quantile_bp: qip_portfolio_engine::construction::MEDIAN_BP,
                    value: at(median),
                },
                QuantilePoint {
                    quantile_bp: 9_000,
                    value: at(median + 0.05),
                },
            ],
        },
        uncertainty: UncertaintyDecomposition {
            epistemic: Decimal::ZERO,
            aleatoric: at(0.0025),
        },
        tail: TailMass {
            side: TailSide::Below,
            threshold: Decimal::ZERO,
            probability_bp: 1_000,
        },
        models_consulted: 1,
        model_disagreement: Decimal::ZERO,
        regime_confidence_bp: Stated::Unmeasurable {
            unmeasurable: "fixture".to_string(),
        },
        expected_impact: Stated::Unmeasurable {
            unmeasurable: "fixture".to_string(),
        },
        calibration: Calibration::Uncalibrated,
        evidence_lineage: vec!["fixture".to_string()],
        expires_at: Timestamp::from_secs(4_000_000_000),
    })
    .expect("the fixture forecast is valid")
}
