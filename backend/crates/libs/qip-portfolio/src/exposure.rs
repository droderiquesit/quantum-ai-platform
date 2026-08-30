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

    /// The worst concentration across every axis, with the axis named.
    pub fn worst_concentration(&self) -> (&'static str, f64) {
        [
            ("asset_class", self.by_asset_class.concentration()),
            ("sector", self.by_sector.concentration()),
            ("country", self.by_country.concentration()),
            ("currency", self.by_currency.concentration()),
            ("issuer", self.by_issuer.concentration()),
        ]
        .into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or(("asset_class", 0.0))
    }
}
