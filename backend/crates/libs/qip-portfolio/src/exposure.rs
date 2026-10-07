//! Exposure aggregation.
//!
//! The same portfolio is looked at along several axes — asset class, sector,
//! country, currency, factor, counterparty — because a limit is breached along
//! one of them, not in aggregate. Gross and net are both reported: a
//! market-neutral book can be flat on net and carry very large gross.

use qip_core::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Exposure along one axis.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Exposure {
    /// Signed exposure per bucket.
    pub net: BTreeMap<String, Decimal>,
    /// Unsigned exposure per bucket.
    pub gross: BTreeMap<String, Decimal>,
}

impl Exposure {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, bucket: impl Into<String>, signed_value: Decimal) {
        let bucket = bucket.into();
        *self.net.entry(bucket.clone()).or_insert(Decimal::ZERO) += signed_value;
        *self.gross.entry(bucket).or_insert(Decimal::ZERO) += signed_value.abs();
    }

    pub fn net_of(&self, bucket: &str) -> Decimal {
        self.net.get(bucket).copied().unwrap_or(Decimal::ZERO)
    }

    pub fn gross_of(&self, bucket: &str) -> Decimal {
        self.gross.get(bucket).copied().unwrap_or(Decimal::ZERO)
    }

    pub fn total_net(&self) -> Decimal {
        self.net.values().copied().sum()
    }

    pub fn total_gross(&self) -> Decimal {
        self.gross.values().copied().sum()
    }

    /// Each bucket's share of total gross exposure.
    pub fn shares(&self) -> BTreeMap<String, f64> {
        let total = self.total_gross();
        if !total.is_positive() {
            return BTreeMap::new();
        }
        self.gross
            .iter()
            .map(|(bucket, value)| (bucket.clone(), value.to_f64() / total.to_f64()))
            .collect()
    }

    /// The largest share of gross exposure in any one bucket.
    pub fn concentration(&self) -> f64 {
        self.shares().values().copied().fold(0.0, f64::max)
    }

    /// Herfindahl index of the gross exposure: 1.0 is everything in one bucket,
    /// 1/n is perfectly even.
    ///
    /// A better concentration measure than the largest share alone, which
    /// cannot distinguish two large positions from one large and many tiny.
    pub fn herfindahl(&self) -> f64 {
        self.shares().values().map(|s| s * s).sum()
    }

    /// Buckets ordered by gross exposure, largest first.
    pub fn ranked(&self) -> Vec<(String, Decimal)> {
        let mut out: Vec<(String, Decimal)> =
            self.gross.iter().map(|(k, v)| (k.clone(), *v)).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }
}

/// Exposure along every axis the risk engine checks.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExposureBreakdown {
    pub by_asset_class: Exposure,
    pub by_sector: Exposure,
    pub by_country: Exposure,
    pub by_currency: Exposure,
    pub by_issuer: Exposure,
    pub by_venue: Exposure,
    /// Factor exposures, in signed notional terms.
    pub by_factor: Exposure,
    /// Exposure by strategy, for cross-strategy portfolio totals.
    pub by_strategy: Exposure,
    /// Exposure by region, for cross-regional capital allocation.
    pub by_region: Exposure,
    /// Exposure by horizon, for horizon-based risk management.
    pub by_horizon: Exposure,
}

impl ExposureBreakdown {
    /// Total gross across positions, taken from the asset-class axis since
    /// every position belongs to exactly one.
    pub fn gross_exposure(&self) -> Decimal {
        self.by_asset_class.total_gross()
    }

    pub fn net_exposure(&self) -> Decimal {
        self.by_asset_class.total_net()
    }

    /// Gross divided by equity: how much notional each unit of capital carries.
    pub fn leverage(&self, equity: Decimal) -> f64 {
        if !equity.is_positive() {
            return f64::INFINITY;
        }
        self.gross_exposure().to_f64() / equity.to_f64()
    }

    /// Verify that all dimension breakdowns sum to the same total gross exposure.
    /// Used for validation that exposure is consistently tracked across all axes.
    pub fn validate_sums(&self) -> Result<(), String> {
        let total = self.gross_exposure();
        let by_strategy_total = self.by_strategy.total_gross();
        let by_region_total = self.by_region.total_gross();
        let by_horizon_total = self.by_horizon.total_gross();

        if by_strategy_total != total {
            return Err(format!(
                "by_strategy total {} != asset_class total {}",
                by_strategy_total, total
            ));
        }
        if by_region_total != total {
            return Err(format!(
                "by_region total {} != asset_class total {}",
                by_region_total, total
            ));
        }
        if by_horizon_total != total {
            return Err(format!(
                "by_horizon total {} != asset_class total {}",
                by_horizon_total, total
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_breakdown_with_four_dimensions_sums_consistently() {
        let mut breakdown = ExposureBreakdown::default();

        // Add exposures across multiple strategies, classes, regions and horizons
        breakdown
            .by_asset_class
            .add("Equity", Decimal::from_int(100));
        breakdown
            .by_asset_class
            .add("Fixed Income", Decimal::from_int(50));
        breakdown
            .by_asset_class
            .add("Commodity", Decimal::from_int(25));

        breakdown.by_strategy.add("Value", Decimal::from_int(75));
        breakdown.by_strategy.add("Growth", Decimal::from_int(100));

        breakdown.by_region.add("US", Decimal::from_int(80));
        breakdown.by_region.add("EU", Decimal::from_int(50));
        breakdown.by_region.add("APAC", Decimal::from_int(45));

        breakdown.by_horizon.add("1Y", Decimal::from_int(60));
        breakdown.by_horizon.add("3Y", Decimal::from_int(80));
        breakdown.by_horizon.add("5Y+", Decimal::from_int(35));

        breakdown.by_sector.add("Tech", Decimal::from_int(50));
        breakdown.by_sector.add("Finance", Decimal::from_int(75));
        breakdown.by_sector.add("Energy", Decimal::from_int(50));

        // Verify all dimensions sum to the same total
        let total = breakdown.gross_exposure();
        assert_eq!(
            total,
            Decimal::from_int(175),
            "total should be sum of asset classes"
        );

        // Validate that cross-dimensional sums are equal
        assert!(
            breakdown.validate_sums().is_ok(),
            "all dimensions must sum to same total"
        );

        // Check that each dimension's total equals the aggregate
        assert_eq!(breakdown.by_strategy.total_gross(), total);
        assert_eq!(breakdown.by_region.total_gross(), total);
        assert_eq!(breakdown.by_horizon.total_gross(), total);
    }

    #[test]
    fn four_dimension_breakdown_with_negative_values_maintains_consistency() {
        let mut breakdown = ExposureBreakdown::default();

        // Each position contributes to all four dimensions simultaneously.
        // Position 1: Equity, Value, US, 1Y: +100
        breakdown
            .by_asset_class
            .add("Equity", Decimal::from_int(100));
        breakdown.by_strategy.add("Value", Decimal::from_int(100));
        breakdown.by_region.add("US", Decimal::from_int(100));
        breakdown.by_horizon.add("1Y", Decimal::from_int(100));

        // Position 2: Equity, Hedge, US, Short: -50 (short position)
        breakdown
            .by_asset_class
            .add("Equity", Decimal::from_int(-50));
        breakdown.by_strategy.add("Hedge", Decimal::from_int(-50));
        breakdown.by_region.add("US", Decimal::from_int(-50));
        breakdown.by_horizon.add("Short", Decimal::from_int(-50));

        // Position 3: Bonds, Long, EU, 5Y: +75
        breakdown.by_asset_class.add("Bonds", Decimal::from_int(75));
        breakdown.by_strategy.add("Long", Decimal::from_int(75));
        breakdown.by_region.add("EU", Decimal::from_int(75));
        breakdown.by_horizon.add("5Y", Decimal::from_int(75));

        // All dimensions must sum to the same gross total: 100 + 50 + 75 = 225
        let total = breakdown.gross_exposure();
        assert_eq!(total, Decimal::from_int(225), "total gross should be 225");

        assert!(
            breakdown.validate_sums().is_ok(),
            "four-dimension breakdown must sum consistently even with shorts"
        );
    }

    #[test]
    fn each_bucket_in_breakdown_is_independent_per_dimension() {
        let mut breakdown = ExposureBreakdown::default();

        breakdown.by_strategy.add("A", Decimal::from_int(100));
        breakdown.by_strategy.add("B", Decimal::from_int(200));
        breakdown.by_region.add("North", Decimal::from_int(100));
        breakdown.by_region.add("South", Decimal::from_int(200));

        // Each dimension's buckets must be independent - the same holding appears once per axis
        assert_eq!(breakdown.by_strategy.net_of("A"), Decimal::from_int(100));
        assert_eq!(breakdown.by_strategy.net_of("B"), Decimal::from_int(200));
        assert_eq!(breakdown.by_region.net_of("North"), Decimal::from_int(100));
        assert_eq!(breakdown.by_region.net_of("South"), Decimal::from_int(200));

        // Totals must still be consistent
        assert_eq!(breakdown.by_strategy.total_gross(), Decimal::from_int(300));
        assert_eq!(breakdown.by_region.total_gross(), Decimal::from_int(300));
    }
}
