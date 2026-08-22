//! Transaction costs.
//!
//! A backtest without costs is a backtest of a strategy nobody can run. The
//! model here has four components, each stated separately so a result can be
//! decomposed into what the strategy earned and what the market took:
//!
//! * **Commission** — a per-share or per-notional fee, and a minimum.
//! * **Spread** — half the quoted spread, paid on entry and on exit.
//! * **Impact** — the price move the order itself causes, growing with the
//!   square root of participation.
//! * **Borrow** — the financing cost of a short, accrued over the holding
//!   period.
//!
//! Impact uses the square-root law because that is the shape the empirical
//! literature converges on across markets and decades. It is a calibrated
//! assumption rather than a measurement of any particular instrument, and
//! [`CostModel::describe`] says so, because a cost model presented as fact is
//! how a strategy that only works at zero impact reaches production.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// Cost parameters. Every field is a stated assumption.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostModel {
    /// Commission as a fraction of notional.
    pub commission_rate: f64,
    /// Minimum commission per order, in currency units.
    pub minimum_commission: f64,
    /// Half-spread paid per side, in basis points.
    pub half_spread_bps: f64,
    /// Coefficient in `impact_bps = k · σ_daily · sqrt(participation) · 10⁴`.
    pub impact_coefficient: f64,
    /// Annualised borrow cost for a short position.
    pub borrow_rate: f64,
    /// Participation above which the model refuses to quote a cost.
    ///
    /// The square-root law is calibrated on modest participation. Extrapolating
    /// it to a third of a day's volume produces a number that looks like an
    /// answer and is not one, so beyond this the simulation reports the order
    /// as unfillable rather than pricing it.
    pub maximum_participation: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        // Institutional US equities, mid-cap: a basis point of commission,
        // three basis points of half-spread, unit impact coefficient.
        Self {
            commission_rate: 0.0001,
            minimum_commission: 1.0,
            half_spread_bps: 3.0,
            impact_coefficient: 1.0,
            borrow_rate: 0.005,
            maximum_participation: 0.20,
        }
    }
}

impl CostModel {
    /// Costs an institution would face in a liquid market.
    pub fn liquid_equity() -> Self {
        Self::default()
    }

    /// A less liquid market: wider spreads, more impact, harder borrow.
    pub fn small_cap() -> Self {
        Self {
            commission_rate: 0.0002,
            minimum_commission: 5.0,
            half_spread_bps: 25.0,
            impact_coefficient: 1.8,
            borrow_rate: 0.04,
            maximum_participation: 0.10,
        }
    }

    /// Zero costs. Available for isolating a signal's raw predictive content,
    /// and never for a result presented as achievable.
    pub fn frictionless() -> Self {
        Self {
            commission_rate: 0.0,
            minimum_commission: 0.0,
            half_spread_bps: 0.0,
            impact_coefficient: 0.0,
            borrow_rate: 0.0,
            maximum_participation: 1.0,
        }
    }

    pub fn is_frictionless(&self) -> bool {
        self.commission_rate == 0.0 && self.half_spread_bps == 0.0 && self.impact_coefficient == 0.0
    }

    pub fn validate(&self) -> Result<()> {
        if self.commission_rate < 0.0
            || self.half_spread_bps < 0.0
            || self.impact_coefficient < 0.0
            || self.borrow_rate < 0.0
        {
            return Err(Error::invalid("cost parameters cannot be negative"));
        }
        if !(0.0..=1.0).contains(&self.maximum_participation) {
            return Err(Error::invalid(
                "maximum participation must be a fraction of daily volume",
            ));
        }
        Ok(())
    }

    pub fn describe(&self) -> String {
        if self.is_frictionless() {
            return "frictionless: this measures a signal's raw content, not an achievable return"
                .to_string();
        }
        format!(
            "{:.1}bp commission, {:.1}bp half-spread, impact k={:.2} on the square-root law (a calibrated assumption, not a measurement), {:.2}% borrow, refusing orders above {:.0}% of daily volume",
            self.commission_rate * 10_000.0,
            self.half_spread_bps,
            self.impact_coefficient,
            self.borrow_rate * 100.0,
            self.maximum_participation * 100.0
        )
    }
}

/// The cost of one order, decomposed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TradeCost {
    pub commission: f64,
    pub spread: f64,
    pub impact: f64,
    /// Notional the costs were computed on.
    pub notional: f64,
}

impl TradeCost {
    pub fn total(&self) -> f64 {
        self.commission + self.spread + self.impact
    }

    /// Total cost in basis points of notional.
    pub fn total_bps(&self) -> f64 {
        if self.notional.abs() < 1e-12 {
            return 0.0;
        }
        self.total() / self.notional.abs() * 10_000.0
    }
}

/// Why an order could not be filled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Unfillable {
    /// The order is larger than the model will price.
    ExceedsParticipation { requested: f64, limit: f64 },
    /// There was no volume to trade against.
    NoVolume,
    /// The price required to compute a cost was missing.
    NoPrice,
}

impl Unfillable {
    pub fn describe(&self) -> String {
        match self {
            Self::ExceedsParticipation { requested, limit } => format!(
                "order is {:.1}% of daily volume against a {:.1}% limit; the impact model is not calibrated that far and would return a number rather than an answer",
                requested * 100.0,
                limit * 100.0
            ),
            Self::NoVolume => "no volume traded in the fill window".to_string(),
            Self::NoPrice => "no price available to compute a cost against".to_string(),
        }
    }
}

impl CostModel {
    /// Cost of trading `quantity` units at `price`.
    ///
    /// `daily_volume` and `daily_volatility` set the impact. Returning an
    /// error rather than a large number when participation is excessive is
    /// deliberate: a backtest that quietly prices a 40%-of-volume order has
    /// told you nothing about whether the strategy is implementable.
    pub fn cost_of(
        &self,
        quantity: Decimal,
        price: Decimal,
        daily_volume: f64,
        daily_volatility: f64,
    ) -> std::result::Result<TradeCost, Unfillable> {
        let quantity_f = quantity.to_f64().abs();
        let price_f = price.to_f64();
        if price_f <= 0.0 {
            return Err(Unfillable::NoPrice);
        }
        if quantity_f < 1e-12 {
            return Ok(TradeCost::default());
        }
        if daily_volume <= 0.0 {
            return Err(Unfillable::NoVolume);
        }

        let participation = quantity_f / daily_volume;
        if participation > self.maximum_participation {
            return Err(Unfillable::ExceedsParticipation {
                requested: participation,
                limit: self.maximum_participation,
            });
        }

        let notional = quantity_f * price_f;
        let commission = (notional * self.commission_rate).max(self.minimum_commission);
        let spread = notional * self.half_spread_bps / 10_000.0;
        let impact_bps = if daily_volatility.is_finite() && daily_volatility > 0.0 {
            self.impact_coefficient * daily_volatility * participation.sqrt() * 10_000.0
        } else {
            0.0
        };
        let impact = notional * impact_bps / 10_000.0;

        Ok(TradeCost {
            commission,
            spread,
            impact,
            notional,
        })
    }

    /// Financing cost of holding a short for `days`.
    pub fn borrow_cost(&self, notional: f64, days: f64) -> f64 {
        if notional >= 0.0 || days <= 0.0 {
            return 0.0;
        }
        notional.abs() * self.borrow_rate * days / 365.0
    }
}
