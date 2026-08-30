//! Margin against collateral, and how long the book takes to leave.
//!
//! Two numbers that are usually reported as one, and are not the same:
//!
//! **Margin** is what has to be posted right now against what is held. It is
//! arithmetic against declared rates, and the only interesting part is the
//! concentration add-on: a broker charges more against a book that is one
//! position than against the same notional spread over fifty, because the
//! first one cannot be liquidated without moving the price.
//!
//! **Liquidity** is how long it would take to get out. A hundred million in a
//! mega-cap and a hundred million in a small-cap are the same notional, the
//! same margin and completely different animals — one is a morning's work and
//! the other is three weeks of being the only seller, during which the price
//! is whatever the market decides it is. A risk system that reports only
//! notional cannot tell them apart, and the horizon is the number that decides
//! whether a stress scenario is survivable or merely unpleasant.
//!
//! The days-to-exit arithmetic is
//! [`qip_financial::costs::LiquidityProfile::days_to_exit`], reused rather
//! than restated. The participation rate is supplied by the caller because it
//! is a decision — being 10% of a day's volume for a week is a different
//! choice from being 30% for two days, and both are defensible.

use crate::exposure::{AggregateExposure, CellPosition};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_financial::costs::LiquidityProfile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Rates applied to gross exposure to obtain a margin requirement.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarginModel {
    /// Initial margin as a fraction of gross exposure.
    pub initial_rate: Decimal,
    /// Maintenance margin as a fraction of gross exposure.
    pub maintenance_rate: Decimal,
    /// Extra initial margin charged on the share of the book above
    /// `concentration_threshold` in any one instrument.
    pub concentration_add_on: Decimal,
    /// Share of gross in one instrument above which the add-on applies.
    pub concentration_threshold: f64,
}

impl Default for MarginModel {
    fn default() -> Self {
        Self {
            // Reg-T-like: half the notional initially, a quarter maintained.
            initial_rate: Decimal::from_raw(500_000_000),
            maintenance_rate: Decimal::from_raw(250_000_000),
            concentration_add_on: Decimal::from_raw(250_000_000),
            concentration_threshold: 0.10,
        }
    }
}

/// What must be posted, against what is posted.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarginRequirement {
    pub gross_exposure: Decimal,
    pub initial: Decimal,
    pub maintenance: Decimal,
    /// The part of `initial` that is the concentration add-on.
    pub concentration_add_on: Decimal,
    pub posted_collateral: Decimal,
}

impl MarginRequirement {
    /// Collateral above the maintenance requirement. Negative is a call.
    pub fn excess(&self) -> Decimal {
        self.posted_collateral - self.maintenance
    }

    /// Whether the book is under-collateralised right now.
    pub fn is_call(&self) -> bool {
        self.excess().is_negative()
    }

    /// Whether new risk can be taken without posting more.
    pub fn can_open(&self) -> bool {
        self.posted_collateral >= self.initial
    }

    /// Fraction of posted collateral consumed by the initial requirement.
    pub fn utilisation(&self) -> f64 {
        if !self.posted_collateral.is_positive() {
            return f64::INFINITY;
        }
        self.initial.to_f64() / self.posted_collateral.to_f64()
    }

    pub fn describe(&self) -> String {
        format!(
            "{} initial ({} of it concentration) and {} maintenance against {} posted; {}",
            self.initial,
            self.concentration_add_on,
            self.maintenance,
            self.posted_collateral,
            if self.is_call() {
                "a margin call"
            } else if self.can_open() {
                "room to open"
            } else {
                "held, but no room to open"
            }
        )
    }
}

impl MarginModel {
    /// Margin required against an aggregate book.
    ///
    /// The add-on is charged on the excess above the threshold rather than on
    /// the whole position, so a book that is one basis point over its
    /// concentration threshold does not pay as though it were entirely
    /// concentrated. A cliff there would make the number jump on a rounding
    /// step in a price.
    pub fn require(
        &self,
        exposure: &AggregateExposure,
        posted_collateral: Decimal,
    ) -> Result<MarginRequirement> {
        let gross = exposure.gross();
        let initial_base = gross
            .checked_mul(self.initial_rate)
            .ok_or_else(|| Error::numeric("the initial margin overflowed"))?;
        let maintenance = gross
            .checked_mul(self.maintenance_rate)
            .ok_or_else(|| Error::numeric("the maintenance margin overflowed"))?;

        let mut add_on = Decimal::ZERO;
        for (bucket, share) in exposure.by_instrument.shares() {
            if share <= self.concentration_threshold {
                continue;
            }
            let excess_share = share - self.concentration_threshold;
            let excess = Decimal::from_f64(excess_share)
                .and_then(|fraction| gross.checked_mul(fraction))
                .ok_or_else(|| {
                    Error::numeric(format!("the concentration excess for {bucket} overflowed"))
                })?;
            add_on += excess
                .checked_mul(self.concentration_add_on)
                .ok_or_else(|| Error::numeric("the concentration add-on overflowed"))?;
        }

        Ok(MarginRequirement {
            gross_exposure: gross,
            initial: initial_base + add_on,
            maintenance,
            concentration_add_on: add_on,
            posted_collateral,
        })
    }
}

/// How long one instrument takes to exit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiquidationHorizon {
    pub instrument: String,
    /// Gross notional held across every cell.
    pub gross: Decimal,
    /// Sessions to exit at the stated participation rate. `None` where the
    /// instrument only trades by negotiation and no volume-based estimate is
    /// meaningful — which is information, not a missing value.
    pub days: Option<f64>,
}

/// How long the whole book takes to exit, and which part is the slow part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiquidityAssessment {
    /// The participation rate the estimate assumes.
    pub participation_rate: f64,
    pub horizons: Vec<LiquidationHorizon>,
    /// Sessions to exit the slowest position that has an estimate.
    pub worst_days: f64,
    /// Gross-weighted mean sessions to exit.
    pub weighted_days: f64,
    /// Gross with no volume-based estimate at all.
    pub unquantifiable_gross: Decimal,
}

impl LiquidityAssessment {
    /// Gross that takes longer than `days` sessions to exit.
    ///
    /// Positions with no estimate count as slow. Treating "we cannot say" as
    /// "fast" is the assumption that makes an illiquid book look liquid right
    /// up until it has to be sold.
    pub fn gross_slower_than(&self, days: f64) -> Decimal {
        self.horizons
            .iter()
            .filter(|h| h.days.is_none_or(|d| d > days))
            .map(|h| h.gross)
            .sum()
    }

    /// Share of the book that takes longer than `days` sessions to exit.
    pub fn share_slower_than(&self, days: f64) -> f64 {
        let total: Decimal = self.horizons.iter().map(|h| h.gross).sum();
        if !total.is_positive() {
            return 0.0;
        }
        self.gross_slower_than(days).to_f64() / total.to_f64()
    }

    pub fn describe(&self) -> String {
        format!(
            "at {:.0}% participation the book exits in {:.1} session(s) on a gross-weighted \
             basis, {:.1} for the slowest position; {} has no volume-based estimate",
            self.participation_rate * 100.0,
            self.weighted_days,
            self.worst_days,
            self.unquantifiable_gross
        )
    }
}

/// How long the book takes to exit at a stated participation rate.
///
/// Positions are netted per instrument first: two cells long the same name
/// leave through the same door, and costing them separately would report a
/// book that exits twice as fast as it does.
pub fn assess_liquidity(
    positions: &[CellPosition],
    liquidity: &BTreeMap<String, LiquidityProfile>,
    participation_rate: f64,
) -> Result<LiquidityAssessment> {
    if !(0.0..=1.0).contains(&participation_rate) || participation_rate <= 0.0 {
        return Err(Error::invalid(
            "a participation rate must be a positive fraction of daily volume",
        ));
    }

    let mut net_quantity: BTreeMap<String, Decimal> = BTreeMap::new();
    let mut gross_notional: BTreeMap<String, Decimal> = BTreeMap::new();
    for position in positions {
        *net_quantity
            .entry(position.instrument.clone())
            .or_insert(Decimal::ZERO) += position.quantity;
        *gross_notional
            .entry(position.instrument.clone())
            .or_insert(Decimal::ZERO) += position.signed_notional().abs();
    }

    let mut horizons = Vec::with_capacity(net_quantity.len());
    let mut unquantifiable_gross = Decimal::ZERO;
    let mut worst_days: f64 = 0.0;
    let mut weighted_numerator = 0.0;
    let mut weighted_denominator = 0.0;

    for (instrument, quantity) in &net_quantity {
        let gross = gross_notional
            .get(instrument)
            .copied()
            .unwrap_or(Decimal::ZERO);
        // Reuse the profile's own arithmetic with the participation rate the
        // caller chose, rather than the profile's default policy rate.
        let days = liquidity.get(instrument).and_then(|profile| {
            let mut at_rate = profile.clone();
            at_rate.max_participation_rate = participation_rate;
            at_rate.days_to_exit(*quantity)
        });
        match days {
            Some(days) => {
                worst_days = worst_days.max(days);
                weighted_numerator += days * gross.to_f64();
                weighted_denominator += gross.to_f64();
            }
            None => unquantifiable_gross += gross,
        }
        horizons.push(LiquidationHorizon {
            instrument: instrument.clone(),
            gross,
            days,
        });
    }

    horizons.sort_by(|a, b| {
        b.days
            .unwrap_or(f64::INFINITY)
            .total_cmp(&a.days.unwrap_or(f64::INFINITY))
            .then_with(|| a.instrument.cmp(&b.instrument))
    });

    Ok(LiquidityAssessment {
        participation_rate,
        horizons,
        worst_days,
        weighted_days: if weighted_denominator > 0.0 {
            weighted_numerator / weighted_denominator
        } else {
            0.0
        },
        unquantifiable_gross,
    })
}
