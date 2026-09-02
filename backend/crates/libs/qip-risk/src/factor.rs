//! Factor risk decomposition.
//!
//! Total portfolio risk is split into what comes from common factor exposure
//! and what is specific to individual holdings. The split matters because the
//! two are managed differently: factor risk is hedged or accepted deliberately,
//! specific risk is diversified away. A portfolio that looks well diversified
//! by position count but carries all its risk in one factor is not diversified.

use qip_core::error::{Error, Result};
use qip_numerics::matrix::Matrix;
use qip_numerics::stats;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A factor risk model: exposures, factor covariance and specific variances.
#[derive(Clone, Debug)]
pub struct FactorRisk {
    /// Factor names, in the order the matrices use.
    pub factors: Vec<String>,
    /// Exposure of each asset to each factor. Rows are assets.
    pub exposures: Matrix,
    /// Covariance between factors.
    pub factor_covariance: Matrix,
    /// Variance not explained by the factors, per asset.
    pub specific_variance: Vec<f64>,
    /// Asset identifiers, aligned with the exposure rows.
    pub assets: Vec<String>,
}

impl FactorRisk {
    /// Build a model, checking the pieces are mutually consistent.
    pub fn new(
        assets: Vec<String>,
        factors: Vec<String>,
        exposures: Matrix,
        factor_covariance: Matrix,
        specific_variance: Vec<f64>,
    ) -> Result<Self> {
        if exposures.rows() != assets.len() {
            return Err(Error::invalid("exposure rows must match the asset count"));
        }
        if exposures.cols() != factors.len() {
            return Err(Error::invalid(
                "exposure columns must match the factor count",
            ));
        }
        if factor_covariance.rows() != factors.len() || !factor_covariance.is_square() {
            return Err(Error::invalid(
                "factor covariance must be square over the factors",
            ));
        }
        if specific_variance.len() != assets.len() {
            return Err(Error::invalid("specific variance must be given per asset"));
        }
        if specific_variance.iter().any(|v| *v < 0.0) {
            return Err(Error::invalid("specific variance cannot be negative"));
        }
        Ok(Self {
            factors,
            exposures,
            factor_covariance,
            specific_variance,
            assets,
        })
    }

    /// Estimate a model from asset and factor return histories.
    ///
    /// Exposures come from a time-series regression per asset; specific
    /// variance is the residual variance of that regression.
    pub fn estimate(
        assets: Vec<String>,
        asset_returns: &Matrix,
        factors: Vec<String>,
        factor_returns: &Matrix,
    ) -> Result<Self> {
        if asset_returns.rows() != factor_returns.rows() {
            return Err(Error::invalid("asset and factor histories must align"));
        }
        if asset_returns.cols() != assets.len() {
            return Err(Error::invalid(
                "asset return columns must match the asset count",
            ));
        }

        let mut exposures = Matrix::zeros(assets.len(), factors.len());
        let mut specific = Vec::with_capacity(assets.len());
        for (index, _) in assets.iter().enumerate() {
            let y = asset_returns.column(index);
            let fit = stats::ols(factor_returns, &y)?;
            // Coefficient 0 is the intercept; the loadings follow.
            for (factor_index, loading) in fit.coefficients.iter().skip(1).enumerate() {
                exposures.set(index, factor_index, *loading);
            }
            specific.push(fit.residual_stddev * fit.residual_stddev);
        }

        let factor_covariance = stats::covariance_matrix(factor_returns)?;
        Self::new(assets, factors, exposures, factor_covariance, specific)
    }

    /// Decompose the risk of a portfolio with the given weights.
    pub fn decompose(&self, weights: &[f64]) -> Result<RiskDecomposition> {
        if weights.len() != self.assets.len() {
            return Err(Error::invalid("weights must be given per asset"));
        }

        // Portfolio factor exposure: X' w.
        let portfolio_exposure = self.exposures.transpose().mul_vec(weights)?;
        let factor_variance = self.factor_covariance.quadratic_form(&portfolio_exposure)?;
        let specific_var: f64 = weights
            .iter()
            .zip(&self.specific_variance)
            .map(|(w, v)| w * w * v)
            .sum();
        let total_variance = (factor_variance + specific_var).max(0.0);
        let total_volatility = total_variance.sqrt();

        // Marginal contribution of each factor: how total risk moves with a
        // unit more exposure to it.
        let covariance_times_exposure = self.factor_covariance.mul_vec(&portfolio_exposure)?;
        let mut factor_contributions = BTreeMap::new();
        for (index, name) in self.factors.iter().enumerate() {
            let contribution = portfolio_exposure[index] * covariance_times_exposure[index];
            factor_contributions.insert(name.clone(), contribution);
        }

        // Contribution of each asset to total variance, which sums to it.
        let mut asset_contributions = BTreeMap::new();
        for (index, asset) in self.assets.iter().enumerate() {
            let row: Vec<f64> = (0..self.factors.len())
                .map(|f| self.exposures.get(index, f))
                .collect();
            let factor_part = row
                .iter()
                .zip(&covariance_times_exposure)
                .map(|(x, c)| x * c)
                .sum::<f64>()
                * weights[index];
            let specific_part = weights[index] * weights[index] * self.specific_variance[index];
            asset_contributions.insert(asset.clone(), factor_part + specific_part);
        }

        Ok(RiskDecomposition {
            total_volatility,
            factor_volatility: factor_variance.max(0.0).sqrt(),
            specific_volatility: specific_var.max(0.0).sqrt(),
            factor_share: if total_variance > 1e-15 {
                factor_variance / total_variance
            } else {
                0.0
            },
            portfolio_exposures: self
                .factors
                .iter()
                .cloned()
                .zip(portfolio_exposure)
                .collect(),
            factor_contributions,
            asset_contributions,
        })
    }

    /// The implied asset covariance matrix: `X F X' + diag(specific)`.
    pub fn asset_covariance(&self) -> Result<Matrix> {
        let systematic = self
            .exposures
            .matmul(&self.factor_covariance)?
            .matmul(&self.exposures.transpose())?;
        let mut covariance = systematic;
        for (index, variance) in self.specific_variance.iter().enumerate() {
            covariance.add_to(index, index, *variance);
        }
        Ok(covariance.symmetrised())
    }
}

/// How a portfolio's risk breaks down.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskDecomposition {
    /// Total portfolio volatility, in the units of the input returns.
    pub total_volatility: f64,
    pub factor_volatility: f64,
    pub specific_volatility: f64,
    /// Fraction of variance explained by common factors.
    pub factor_share: f64,
    /// Net exposure to each factor.
    pub portfolio_exposures: BTreeMap<String, f64>,
    /// Each factor's contribution to variance.
    pub factor_contributions: BTreeMap<String, f64>,
    /// Each asset's contribution to variance. Sums to total variance.
    pub asset_contributions: BTreeMap<String, f64>,
}

impl RiskDecomposition {
    /// The factor carrying the most risk.
    pub fn dominant_factor(&self) -> Option<(&str, f64)> {
        self.factor_contributions
            .iter()
            .max_by(|a, b| {
                a.1.abs()
                    .partial_cmp(&b.1.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(name, value)| (name.as_str(), *value))
    }

    /// Whether risk is concentrated in one factor despite many holdings.
    ///
    /// The failure this catches: a portfolio of thirty names that is really one
    /// bet on a single factor. Position count says diversified; this does not.
    pub fn is_factor_concentrated(&self, threshold: f64) -> bool {
        let total: f64 = self.factor_contributions.values().map(|v| v.abs()).sum();
        if total <= 1e-15 {
            return false;
        }
        self.dominant_factor()
            .is_some_and(|(_, value)| value.abs() / total > threshold)
    }

    /// Effective number of independent bets, from the asset contributions.
    ///
    /// The inverse Herfindahl of the risk contributions. Thirty equal-weighted
    /// positions that all move together have an effective count near one.
    pub fn effective_bets(&self) -> f64 {
        let total: f64 = self.asset_contributions.values().map(|v| v.abs()).sum();
        if total <= 1e-15 {
            return 0.0;
        }
        let herfindahl: f64 = self
            .asset_contributions
            .values()
            .map(|v| (v.abs() / total).powi(2))
            .sum();
        if herfindahl <= 1e-15 {
            0.0
        } else {
            1.0 / herfindahl
        }
    }
}
