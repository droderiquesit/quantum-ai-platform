//! How much a strategy can absorb before its own trading eats its edge.
//!
//! Capacity is the size at which the market impact of getting into a position
//! costs as much as the position is expected to earn. Past that point extra
//! capital does not earn less, it earns a negative amount, and an allocator
//! that sizes linearly off a Sharpe ratio will walk straight through it.
//!
//! The impact law is not reimplemented here.
//! [`qip_financial::costs::TransactionCostModel`] already carries the
//! square-root law and [`TransactionCostModel::breakeven_participation`]
//! already solves it for the participation at which impact exhausts a stated
//! alpha; [`qip_financial::costs::LiquidityProfile`] already carries the
//! volume and the participation policy. This module converts between
//! participation and notional and names which of the two bounds binds.
//!
//! Two things bound capacity and they are different in kind. The **edge** runs
//! out at the participation where impact eats alpha — an economic fact about
//! the strategy. The **policy** runs out at
//! [`LiquidityProfile::max_participation_rate`] — a decision about how much of
//! a day's volume the platform is willing to be, which holds even for a
//! strategy whose alpha could pay for more. Reporting which one bound matters,
//! because only one of them can be relaxed by finding a better strategy.

use qip_core::error::{Error, Result};
use qip_core::Decimal;
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use serde::{Deserialize, Serialize};

/// Which constraint set the capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapacityBound {
    /// Impact would eat the alpha beyond this size.
    EdgeExhausted,
    /// The platform's own participation policy binds first.
    ParticipationPolicy,
    /// The alpha does not cover the spread and fees at any size.
    NoEdgeAtAnySize,
    /// No volume to trade against, so no volume-based estimate exists.
    NoVolume,
}

impl CapacityBound {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::EdgeExhausted => "edge_exhausted",
            Self::ParticipationPolicy => "participation_policy",
            Self::NoEdgeAtAnySize => "no_edge_at_any_size",
            Self::NoVolume => "no_volume",
        }
    }
}

/// The most a strategy can hold, and why that is the number.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capacity {
    /// Book notional at capacity.
    pub notional: Decimal,
    /// Daily participation the book implies at that size.
    pub participation: f64,
    pub binding: CapacityBound,
}

impl Capacity {
    /// Nothing can be allocated, with the reason.
    pub const fn none(binding: CapacityBound) -> Self {
        Self {
            notional: Decimal::ZERO,
            participation: 0.0,
            binding,
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "{} at {:.2}% of daily volume, bound by {}",
            self.notional,
            self.participation * 100.0,
            self.binding.as_str()
        )
    }
}

/// What a strategy trades, how expensive that is, and how much it expects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapacityModel {
    pub liquidity: LiquidityProfile,
    pub costs: TransactionCostModel,
    /// Gross expected alpha per unit of turnover, in basis points.
    ///
    /// Gross, so the cost model can deduct from it. Netting the costs off
    /// before they reach the impact calculation deducts them twice.
    pub expected_alpha_bps: f64,
    /// Price used to convert instrument units into notional.
    pub reference_price: Decimal,
    /// Fraction of the book turned over per day.
    ///
    /// A strategy that turns its book over twice a day is four times the
    /// participation of one that holds for two days at the same notional, and
    /// therefore has a quarter of the capacity. Leaving this out is how a fast
    /// strategy gets sized like a slow one.
    pub daily_turnover: f64,
}

impl CapacityModel {
    pub fn new(
        liquidity: LiquidityProfile,
        costs: TransactionCostModel,
        expected_alpha_bps: f64,
        reference_price: Decimal,
        daily_turnover: f64,
    ) -> Result<Self> {
        if !reference_price.is_positive() {
            return Err(Error::invalid("a reference price must be positive"));
        }
        if !daily_turnover.is_finite() || daily_turnover <= 0.0 {
            return Err(Error::invalid(
                "a strategy that never turns its book over has no participation and no capacity",
            ));
        }
        if !expected_alpha_bps.is_finite() {
            return Err(Error::invalid("expected alpha must be a finite number"));
        }
        Ok(Self {
            liquidity,
            costs,
            expected_alpha_bps,
            reference_price,
            daily_turnover,
        })
    }

    /// Daily participation implied by holding `notional`.
    pub fn participation_at(&self, notional: Decimal) -> f64 {
        let adv = self.liquidity.average_daily_volume.to_f64();
        if adv <= 0.0 {
            return f64::INFINITY;
        }
        let units = notional.abs().to_f64() / self.reference_price.to_f64();
        units * self.daily_turnover / adv
    }

    /// Book notional that would imply `participation` of daily volume.
    fn notional_at(&self, participation: f64) -> Option<Decimal> {
        let adv = self.liquidity.average_daily_volume.to_f64();
        let units = participation * adv / self.daily_turnover;
        Decimal::from_f64(units * self.reference_price.to_f64())
    }

    /// Net expected edge, in basis points, at a given book size.
    ///
    /// Goes negative beyond capacity. That sign change is the whole point: an
    /// allocator reading this rather than the gross alpha cannot be persuaded
    /// that another ten million of a full strategy is a smaller good thing.
    pub fn net_edge_bps(&self, notional: Decimal) -> f64 {
        let participation = self.participation_at(notional);
        if !participation.is_finite() {
            return f64::NEG_INFINITY;
        }
        self.expected_alpha_bps - self.costs.total_bps(participation)
    }

    /// Whether this size is past the point where the strategy pays for itself.
    pub fn is_beyond_capacity(&self, notional: Decimal) -> bool {
        self.net_edge_bps(notional) <= 0.0
    }

    /// The largest book this strategy supports, and which bound set it.
    pub fn capacity(&self) -> Capacity {
        if self.liquidity.average_daily_volume.to_f64() <= 0.0 || self.liquidity.is_negotiated {
            return Capacity::none(CapacityBound::NoVolume);
        }

        let breakeven = self.costs.breakeven_participation(self.expected_alpha_bps);
        if breakeven <= 0.0 {
            return Capacity::none(CapacityBound::NoEdgeAtAnySize);
        }

        let policy = self.liquidity.max_participation_rate;
        let (participation, binding) = if policy > 0.0 && policy < breakeven {
            (policy, CapacityBound::ParticipationPolicy)
        } else {
            (breakeven, CapacityBound::EdgeExhausted)
        };

        self.notional_at(participation).map_or(
            Capacity::none(CapacityBound::NoVolume),
            |notional| Capacity {
                notional,
                participation,
                binding,
            },
        )
    }

    /// Capacity shrunk for how uncertain the estimate is.
    ///
    /// `uncertainty` is the relative standard error of the capacity estimate.
    /// It is subtracted rather than used to widen a band, because the two
    /// directions are not symmetric in consequence: being wrong about capacity
    /// on the low side leaves money on the table, and being wrong on the high
    /// side means the impact of getting in is larger than the reason for
    /// getting in.
    pub fn conservative_capacity(&self, uncertainty: f64) -> Capacity {
        let capacity = self.capacity();
        let haircut = 1.0 - uncertainty.clamp(0.0, 1.0);
        let notional = Decimal::from_f64(haircut)
            .and_then(|factor| capacity.notional.checked_mul(factor))
            .unwrap_or(Decimal::ZERO);
        Capacity {
            notional,
            participation: capacity.participation * haircut,
            binding: capacity.binding,
        }
    }

    /// A sentence an operator can read next to an allocation.
    pub fn describe(&self) -> String {
        let capacity = self.capacity();
        format!(
            "{:.1}bp gross alpha against {:.1}bp of cost at capacity; {}",
            self.expected_alpha_bps,
            self.costs.total_bps(capacity.participation),
            capacity.describe()
        )
    }
}
