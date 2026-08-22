//! What a venue charges, accepts, and can be relied on for.
//!
//! A router that compares venues on their quoted price is comparing the one
//! number that is not the cost. The fee schedule is the rest of it, and it is
//! not a constant: maker and taker are different prices for the same trade, a
//! maker fee is frequently negative, and both move with trailing volume. All
//! three of those turn a venue that looks cheap into one that is not.

use qip_contracts::venue::{VenueClass, VenueId};
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use qip_core::Decimal;
use serde::{Deserialize, Serialize};

/// Which side of the trade provided the liquidity.
///
/// The distinction the whole fee schedule turns on: the same fill is charged
/// two different prices depending on which order was resting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Liquidity {
    /// The order was resting and was traded against.
    Maker,
    /// The order crossed and took what was resting.
    Taker,
}

impl Liquidity {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Maker => "maker",
            Self::Taker => "taker",
        }
    }
}

/// One rung of a volume-tiered fee schedule.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeeTier {
    /// Trailing volume at or above which this rung applies.
    pub from_volume: Decimal,
    /// Maker fee in basis points of notional. Negative is a rebate, and a
    /// rebate is the reason this is signed rather than a magnitude.
    pub maker_bps_f64: f64,
    /// Taker fee in basis points of notional.
    pub taker_bps_f64: f64,
}

impl FeeTier {
    pub fn new(from_volume: Decimal, maker_bps_f64: f64, taker_bps_f64: f64) -> Self {
        Self {
            from_volume,
            maker_bps_f64,
            taker_bps_f64,
        }
    }
}

/// A venue's fee schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeeSchedule {
    tiers: Vec<FeeTier>,
}

impl FeeSchedule {
    /// One rate whatever the volume.
    pub fn flat(maker_bps_f64: f64, taker_bps_f64: f64) -> Self {
        Self {
            tiers: vec![FeeTier::new(Decimal::ZERO, maker_bps_f64, taker_bps_f64)],
        }
    }

    /// A tiered schedule, sorted and checked.
    ///
    /// Refuses a schedule that does not start at zero volume: without a bottom
    /// rung there is no rate for a new account, and the natural fallback — the
    /// cheapest tier — is the one nobody qualifies for.
    pub fn tiered(mut tiers: Vec<FeeTier>) -> Result<Self> {
        if tiers.is_empty() {
            return Err(Error::invalid("a fee schedule needs at least one tier"));
        }
        tiers.sort_by(|a, b| a.from_volume.cmp(&b.from_volume));
        if tiers[0].from_volume > Decimal::ZERO {
            return Err(Error::invalid(
                "a fee schedule must have a tier starting at zero volume",
            ));
        }
        if tiers
            .iter()
            .any(|tier| !tier.maker_bps_f64.is_finite() || !tier.taker_bps_f64.is_finite())
        {
            return Err(Error::invalid("a fee schedule tier has a non-finite rate"));
        }
        Ok(Self { tiers })
    }

    pub fn tiers(&self) -> &[FeeTier] {
        &self.tiers
    }

    /// The rate that applies at a trailing volume, in basis points.
    pub fn rate_bps_f64(&self, liquidity: Liquidity, trailing_volume: Decimal) -> f64 {
        let tier = self
            .tiers
            .iter()
            .rev()
            .find(|tier| trailing_volume >= tier.from_volume)
            .or_else(|| self.tiers.first());
        tier.map_or(0.0, |tier| match liquidity {
            Liquidity::Maker => tier.maker_bps_f64,
            Liquidity::Taker => tier.taker_bps_f64,
        })
    }

    /// What a notional would be charged. Negative is a rebate paid to you.
    pub fn fee(
        &self,
        notional: Decimal,
        liquidity: Liquidity,
        trailing_volume: Decimal,
    ) -> Decimal {
        notional
            .abs()
            .apply_bps(self.rate_bps_f64(liquidity, trailing_volume))
    }
}

/// Everything the router needs to know about a venue that is not its book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueProfile {
    pub venue: VenueId,
    pub class: VenueClass,
    pub fees: FeeSchedule,
    /// Order types the venue accepts. A router that sends an unsupported type
    /// gets a reject, which costs a round trip and a re-route.
    pub supported: Vec<crate::ordertype::OrderTypeKind>,
    /// Smallest order the venue will accept.
    pub min_size: Decimal,
    /// Increment every order size must be a multiple of.
    pub lot_size: Decimal,
    /// Round-trip time to an acknowledgement, when the venue is behaving.
    pub typical_latency: Duration,
    /// Share of orders expected to be accepted rather than rejected. A prior,
    /// and a statistic: what actually happens is tracked in
    /// [`crate::health`].
    pub reliability_f64: f64,
    /// Trailing volume already traded here, which fixes the fee tier.
    pub trailing_volume: Decimal,
}

impl VenueProfile {
    /// A profile with the shape of a listed exchange.
    pub fn listed(venue: VenueId, maker_bps_f64: f64, taker_bps_f64: f64) -> Self {
        Self {
            venue,
            class: VenueClass::Exchange,
            fees: FeeSchedule::flat(maker_bps_f64, taker_bps_f64),
            supported: vec![
                crate::ordertype::OrderTypeKind::Market,
                crate::ordertype::OrderTypeKind::Limit,
                crate::ordertype::OrderTypeKind::ImmediateOrCancel,
                crate::ordertype::OrderTypeKind::FillOrKill,
                crate::ordertype::OrderTypeKind::Peg,
            ],
            min_size: Decimal::ONE,
            lot_size: Decimal::ONE,
            typical_latency: Duration::from_millis(5),
            reliability_f64: 0.999,
            trailing_volume: Decimal::ZERO,
        }
    }

    pub fn with_sizes(mut self, min_size: Decimal, lot_size: Decimal) -> Self {
        self.min_size = min_size;
        self.lot_size = lot_size;
        self
    }

    pub fn with_supported(mut self, supported: Vec<crate::ordertype::OrderTypeKind>) -> Self {
        self.supported = supported;
        self
    }

    pub fn with_latency(mut self, typical_latency: Duration) -> Self {
        self.typical_latency = typical_latency;
        self
    }

    pub fn with_trailing_volume(mut self, trailing_volume: Decimal) -> Self {
        self.trailing_volume = trailing_volume;
        self
    }

    pub fn supports(&self, kind: crate::ordertype::OrderTypeKind) -> bool {
        self.supported.contains(&kind)
    }

    /// Round a quantity down to a whole number of lots.
    ///
    /// Down, never up: rounding an order up sends more than was decided, and a
    /// share nobody asked for is worse than a share left behind — the one left
    /// behind is reported as unrouted and can be dealt with.
    pub fn round_to_lot(&self, quantity: Decimal) -> Decimal {
        quantity.floor_to_step(self.lot_size)
    }

    /// Whether a quantity is one this venue would accept.
    pub fn accepts_size(&self, quantity: Decimal) -> bool {
        quantity >= self.min_size && quantity > Decimal::ZERO
    }

    pub fn validate(&self) -> Result<()> {
        if self.lot_size <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "{} has a non-positive lot size",
                self.venue.as_str()
            )));
        }
        if self.min_size < Decimal::ZERO {
            return Err(Error::invalid(format!(
                "{} has a negative minimum size",
                self.venue.as_str()
            )));
        }
        if !(0.0..=1.0).contains(&self.reliability_f64) {
            return Err(Error::invalid(format!(
                "{} has a reliability of {}, which is not a probability",
                self.venue.as_str(),
                self.reliability_f64
            )));
        }
        Ok(())
    }
}
