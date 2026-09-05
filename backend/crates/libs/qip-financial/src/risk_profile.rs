//! Risk characteristics carried on every financial object.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Option sensitivities. Present on derivatives; absent elsewhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Greeks {
    /// Sensitivity to a unit move in the underlying.
    pub delta: f64,
    /// Rate of change of delta.
    pub gamma: f64,
    /// Sensitivity to a one-point move in implied volatility.
    pub vega: f64,
    /// Time decay per day.
    pub theta: f64,
    /// Sensitivity to a one-percent move in rates.
    pub rho: f64,
    /// Sensitivity of vega to volatility, which matters for volatility books.
    pub vomma: f64,
    /// Cross-sensitivity of delta to volatility.
    pub vanna: f64,
}

impl Greeks {
    pub fn is_finite(&self) -> bool {
        [
            self.delta, self.gamma, self.vega, self.theta, self.rho, self.vomma, self.vanna,
        ]
        .iter()
        .all(|v| v.is_finite())
    }

    /// Scale by a position size, producing position-level Greeks.
    pub fn scaled(&self, quantity: f64) -> Self {
        Self {
            delta: self.delta * quantity,
            gamma: self.gamma * quantity,
            vega: self.vega * quantity,
            theta: self.theta * quantity,
            rho: self.rho * quantity,
            vomma: self.vomma * quantity,
            vanna: self.vanna * quantity,
        }
    }

    /// Sum two Greek vectors — Greeks aggregate linearly across positions.
    pub fn combined(&self, other: &Self) -> Self {
        Self {
            delta: self.delta + other.delta,
            gamma: self.gamma + other.gamma,
            vega: self.vega + other.vega,
            theta: self.theta + other.theta,
            rho: self.rho + other.rho,
            vomma: self.vomma + other.vomma,
            vanna: self.vanna + other.vanna,
        }
    }
}

/// Loadings on the risk factors the platform tracks.
///
/// Kept as a named map rather than a fixed struct so a new factor model can be
/// introduced without a schema migration; the factor names themselves are
/// registered in the world model.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FactorExposures {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub loadings: BTreeMap<String, f64>,
}

impl FactorExposures {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, factor: impl Into<String>, loading: f64) -> Self {
        self.loadings.insert(factor.into(), loading);
        self
    }

    pub fn get(&self, factor: &str) -> f64 {
        self.loadings.get(factor).copied().unwrap_or(0.0)
    }

    pub fn set(&mut self, factor: impl Into<String>, loading: f64) {
        self.loadings.insert(factor.into(), loading);
    }

    pub fn factors(&self) -> impl Iterator<Item = &str> {
        self.loadings.keys().map(String::as_str)
    }

    /// Weighted blend of two exposure vectors, for aggregating to a portfolio.
    pub fn blend(&self, weight: f64, other: &Self, other_weight: f64) -> Self {
        let mut out = BTreeMap::new();
        for (factor, loading) in &self.loadings {
            *out.entry(factor.clone()).or_insert(0.0) += loading * weight;
        }
        for (factor, loading) in &other.loadings {
            *out.entry(factor.clone()).or_insert(0.0) += loading * other_weight;
        }
        Self { loadings: out }
    }

    /// The factor with the largest absolute loading.
    pub fn dominant(&self) -> Option<(&str, f64)> {
        self.loadings
            .iter()
            .max_by(|a, b| {
                a.1.abs()
                    .partial_cmp(&b.1.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(k, v)| (k.as_str(), *v))
    }
}

/// The standard factor names used by the built-in models.
pub mod factors {
    pub const MARKET: &str = "market";
    pub const SIZE: &str = "size";
    pub const VALUE: &str = "value";
    pub const MOMENTUM: &str = "momentum";
    pub const QUALITY: &str = "quality";
    pub const LOW_VOLATILITY: &str = "low_volatility";
    pub const DURATION: &str = "duration";
    pub const CREDIT_SPREAD: &str = "credit_spread";
    pub const INFLATION: &str = "inflation";
    pub const REAL_RATES: &str = "real_rates";
    pub const DOLLAR: &str = "dollar";
    pub const OIL: &str = "oil";
    pub const LIQUIDITY: &str = "liquidity";
    pub const CARRY: &str = "carry";

    pub const ALL: [&str; 14] = [
        MARKET,
        SIZE,
        VALUE,
        MOMENTUM,
        QUALITY,
        LOW_VOLATILITY,
        DURATION,
        CREDIT_SPREAD,
        INFLATION,
        REAL_RATES,
        DOLLAR,
        OIL,
        LIQUIDITY,
        CARRY,
    ];
}

/// Aggregate risk description attached to a financial object.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RiskCharacteristics {
    /// Annualised volatility of returns.
    pub annualised_volatility: f64,
    /// Beta to the object's primary benchmark.
    pub beta: f64,
    /// Notional exposure per unit of capital committed.
    pub leverage: f64,
    /// Historical worst peak-to-trough decline, as a positive fraction.
    pub max_historical_drawdown: f64,
    /// Excess kurtosis of returns; a fat-tail warning for sizing.
    pub excess_kurtosis: f64,
    /// Skewness of returns.
    pub skewness: f64,
    /// Probability of default over one year, where meaningful.
    pub default_probability: f64,
    /// Fraction of notional recovered in default.
    pub recovery_rate: f64,
    pub factor_exposures: FactorExposures,
}

impl RiskCharacteristics {
    /// Whether the object's own statistics are internally coherent. Used as an
    /// ingestion guard: a negative volatility is a data error, not a signal.
    pub fn is_coherent(&self) -> bool {
        self.annualised_volatility >= 0.0
            && self.annualised_volatility.is_finite()
            && self.leverage >= 0.0
            && (0.0..=1.0).contains(&self.max_historical_drawdown)
            && (0.0..=1.0).contains(&self.default_probability)
            && (0.0..=1.0).contains(&self.recovery_rate)
            && self.beta.is_finite()
    }

    /// Expected loss given default, per unit of notional.
    pub fn expected_credit_loss(&self) -> f64 {
        self.default_probability * (1.0 - self.recovery_rate)
    }

    /// Decomposes the credit-spread identity `spread ≈ default_probability ×
    /// (1 − recovery_rate)` into its named components, in exact `Decimal`
    /// arithmetic.
    ///
    /// This identity assumes risk-neutral default and recovery probabilities,
    /// a single period to the credit horizon, and no liquidity premium. A
    /// market-observed spread also compensates for liquidity and term
    /// premia this identity does not separate out — the result is the
    /// credit-risk component of a spread, not a market spread itself.
    ///
    /// Refuses rather than clamps: a `default_probability` or `recovery_rate`
    /// outside `[0, 1]` is a data error upstream, and silently clamping it
    /// would hide that error inside a plausible-looking number.
    pub fn spread_decomposition(&self) -> Result<SpreadDecomposition> {
        if !(0.0..=1.0).contains(&self.default_probability) {
            return Err(Error::invalid(format!(
                "default_probability must lie within [0, 1], got {}",
                self.default_probability
            )));
        }
        if !(0.0..=1.0).contains(&self.recovery_rate) {
            return Err(Error::invalid(format!(
                "recovery_rate must lie within [0, 1], got {}",
                self.recovery_rate
            )));
        }

        // The range checks above already exclude NaN and infinities, so these
        // conversions cannot fail on the values that reach them; the `Err`
        // arms exist because `from_f64`/`checked_*` return `Option`, not
        // because failure is expected here.
        let default_probability = Decimal::from_f64(self.default_probability)
            .ok_or_else(|| Error::invalid("default_probability is not a finite number"))?;
        let recovery_rate = Decimal::from_f64(self.recovery_rate)
            .ok_or_else(|| Error::invalid("recovery_rate is not a finite number"))?;
        let loss_given_default = Decimal::ONE
            .checked_sub(recovery_rate)
            .ok_or_else(|| Error::numeric("loss given default overflowed"))?;
        let spread = default_probability
            .checked_mul(loss_given_default)
            .ok_or_else(|| Error::numeric("spread computation overflowed"))?;

        Ok(SpreadDecomposition {
            default_probability,
            loss_given_default,
            spread,
        })
    }
}

/// The named components of the credit-spread identity. See
/// [`RiskCharacteristics::spread_decomposition`] for what this assumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadDecomposition {
    /// Probability of default over the horizon, as supplied.
    pub default_probability: Decimal,
    /// `1 − recovery_rate`: the fraction of notional lost in default.
    pub loss_given_default: Decimal,
    /// `default_probability × loss_given_default`, the credit-risk
    /// component of a spread.
    pub spread: Decimal,
}
