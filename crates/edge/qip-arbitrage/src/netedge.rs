//! Turning a priced path into a [`NetEdge`] with all seven deductions.
//!
//! The gross figure is the easy part and the least interesting: it is the
//! quantity that comes back at the end of the cycle, minus the quantity that
//! went in, before anything is charged for getting it. What decides whether the
//! trade happens is everything taken off it,
//! and [`NetEdge::require_complete`] is the reason each one has to be named
//! rather than assumed away. A deduction that was never considered does not
//! show up as zero here; it shows up as a refusal.
//!
//! The seven, and what each is actually modelling:
//!
//! * **Spread** — crossing from the mid to the touch on every leg. Charged to
//!   everyone, whatever the size.
//! * **Slippage** — walking past the touch to fill *this* size, measured from
//!   the sweep rather than assumed at some round number of basis points. A
//!   fixed slippage guess is indistinguishable from having no book at all.
//! * **Fees** — the proportional cost each conversion charges regardless of how
//!   the book looked.
//! * **Latency** — the position is unhedged between the first leg and the last,
//!   and the market moves during that window. Priced as the expected *absolute*
//!   move over the round trip: the direction is unknown, and an arbitrage that
//!   needs both legs is hurt whichever way it goes, so halving it for the
//!   favourable case would be wishful.
//! * **Funding** — borrow, carry and gas over the time capital is committed.
//! * **Collateral** — margin the position ties up and cannot earn elsewhere. A
//!   path with a leg that can revert ties up the whole notional, whatever
//!   margin the venue asks for, because the inventory has to be sitting there.
//! * **Uncertainty** — not a cost the market charges. A discount the platform
//!   takes on its own estimate, growing as the inputs get stale and shrinking
//!   as more observations back them.
//!
//! Every deduction is denominated in units of the instrument the cycle started
//! from. A cost quoted as a fraction of one leg's notional converts by
//! multiplying by the starting size, because each conversion carries the whole
//! value of the path — a first-order treatment that is exact at the sizes an
//! arbitrage is done in and is stated here rather than buried.

use crate::arith::{div, from_statistic, mul};
use crate::pricing::PathPricing;
use qip_contracts::edge::{Deduction, DeductionKind, NetEdge};
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// Expectation of the absolute value of a standard normal, `sqrt(2/pi)`.
///
/// The factor that turns a volatility into an expected move. Without it the
/// latency deduction is a one-standard-deviation figure, which overstates the
/// typical case by about a quarter and makes every path look worse than it is.
const EXPECTED_ABSOLUTE_NORMAL: f64 = 0.797_884_560_802_865_4;

/// What the calculator is entitled to assume about the world.
///
/// Held in one struct rather than passed piecemeal so that a change of
/// assumption is one diff and one review, and so that the assumptions a net
/// edge was computed under can be recorded next to the number.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeAssumptions {
    /// Decision to last leg landing. What the latency deduction is priced over.
    pub round_trip: Duration,
    /// Annualised volatility of the path's payoff, as a fraction. A statistic.
    pub volatility_annual_f64: f64,
    /// How long capital stays committed. What funding and collateral are priced
    /// over. For a plan that has to prefund inventory, this is the prefunding
    /// horizon and not the round trip.
    pub holding_period: Duration,
    /// Annualised borrow, funding or amortised gas rate. A statistic.
    pub funding_rate_annual_f64: f64,
    /// Fraction of notional posted as margin.
    pub collateral_fraction_f64: f64,
    /// What posted collateral would have earned elsewhere, annualised.
    pub collateral_rate_annual_f64: f64,
    /// Age at which an input's contribution to confidence has decayed by `1/e`.
    pub confidence_half_life: Duration,
    /// Age past which the inputs are refused outright.
    ///
    /// A haircut is the right answer for an estimate that is getting old. It is
    /// the wrong answer for one that is simply no longer about this market, and
    /// no discount makes a stale book fillable.
    pub max_staleness: Duration,
    /// Observations the sample term behaves as if it had already seen.
    ///
    /// Keeps the haircut on a single observation severe without being total: a
    /// prior of one halves the confidence of a one-observation estimate.
    pub prior_observations: u32,
}

impl Default for EdgeAssumptions {
    fn default() -> Self {
        Self {
            round_trip: Duration::from_millis(250),
            volatility_annual_f64: 0.40,
            holding_period: Duration::from_mins(5),
            funding_rate_annual_f64: 0.05,
            collateral_fraction_f64: 0.20,
            collateral_rate_annual_f64: 0.04,
            confidence_half_life: Duration::from_secs(30),
            max_staleness: Duration::from_secs(300),
            prior_observations: 1,
        }
    }
}

/// Builds a complete [`NetEdge`] for a priced path.
#[derive(Clone, Debug, PartialEq)]
pub struct NetEdgeCalculator {
    assumptions: EdgeAssumptions,
}

impl NetEdgeCalculator {
    pub fn new(assumptions: EdgeAssumptions) -> Self {
        Self { assumptions }
    }

    pub fn assumptions(&self) -> &EdgeAssumptions {
        &self.assumptions
    }

    /// Price every deduction and refuse anything incomplete.
    ///
    /// `now` is a parameter and not a clock read. The uncertainty haircut is a
    /// function of how old the inputs are, and a component that fetched the
    /// time itself would give a replay a different answer from the live run it
    /// is supposed to reproduce.
    pub fn calculate(&self, pricing: &PathPricing, now: Timestamp) -> Result<NetEdge> {
        let size = pricing.start_quantity;
        if size <= Decimal::ZERO {
            return Err(Error::invalid("a net edge needs a positive size"));
        }

        let age = elapsed(pricing.oldest_input_at, now);
        if age.as_nanos() > self.assumptions.max_staleness.as_nanos() {
            return Err(Error::guard(format!(
                "the oldest input is {age:?} old, past the {:?} limit; no haircut makes a stale book fillable",
                self.assumptions.max_staleness
            )));
        }

        let (spread_fraction, slippage_fraction) = book_cost_fractions(pricing)?;
        let fee_fraction = pricing
            .conversions
            .iter()
            .fold(Decimal::ZERO, |sum, c| sum + c.cost_fraction);

        let spread = mul(size, spread_fraction, "spread deduction")?;
        let slippage = mul(size, slippage_fraction, "slippage deduction")?;
        let fees = mul(size, fee_fraction, "fee deduction")?;
        let latency = self.latency_deduction(size)?;
        let funding = self.funding_deduction(size)?;
        let collateral = self.collateral_deduction(size, pricing.all_atomic)?;

        let gross = pricing.gross_edge();
        let before_uncertainty =
            gross - (spread + slippage + fees + latency + funding + collateral);
        let (uncertainty, confidence_f64) =
            self.uncertainty_deduction(before_uncertainty, age, pricing.fewest_observations)?;

        let edge = NetEdge::gross(gross, size)?
            .deduct(Deduction::new(
                DeductionKind::Spread,
                spread,
                format!("crossing to the touch on {} legs, {spread_fraction} of notional", pricing.legs().count()),
            )?)
            .deduct(Deduction::new(
                DeductionKind::Fees,
                fees,
                format!(
                    "conversion costs across {} conversions, {fee_fraction} of notional",
                    pricing.conversions.len()
                ),
            )?)
            .deduct(Deduction::new(
                DeductionKind::Latency,
                latency,
                format!(
                    "expected absolute move at {} annualised volatility over a {:?} round trip",
                    self.assumptions.volatility_annual_f64, self.assumptions.round_trip
                ),
            )?)
            .deduct(Deduction::new(
                DeductionKind::Slippage,
                slippage,
                format!(
                    "walking past the touch at the size traded, {slippage_fraction} of notional, measured from the sweep"
                ),
            )?)
            .deduct(Deduction::new(
                DeductionKind::Funding,
                funding,
                format!(
                    "{} annualised over a {:?} commitment",
                    self.assumptions.funding_rate_annual_f64, self.assumptions.holding_period
                ),
            )?)
            .deduct(Deduction::new(
                DeductionKind::Collateral,
                collateral,
                if pricing.all_atomic {
                    format!(
                        "{} of notional posted at {} annualised",
                        self.assumptions.collateral_fraction_f64,
                        self.assumptions.collateral_rate_annual_f64
                    )
                } else {
                    "a leg can revert, so the full notional is held as inventory for the duration"
                        .to_string()
                },
            )?)
            .deduct(Deduction::new(
                DeductionKind::Uncertainty,
                uncertainty,
                format!(
                    "confidence {confidence_f64} from inputs {age:?} old backed by {} observations",
                    pricing.fewest_observations
                ),
            )?);

        // The check that makes the other six load-bearing. It cannot catch a
        // deduction that is wrong, only one that is absent, and absent is the
        // way this goes wrong in practice.
        edge.require_complete()?;
        Ok(edge)
    }

    fn latency_deduction(&self, size: Decimal) -> Result<Decimal> {
        let years_f64 = self.assumptions.round_trip.as_years_f64().max(0.0);
        let move_f64 = self.assumptions.volatility_annual_f64.max(0.0)
            * years_f64.sqrt()
            * EXPECTED_ABSOLUTE_NORMAL;
        let fraction = from_statistic(move_f64, "expected adverse move")?;
        mul(size, fraction.max(Decimal::ZERO), "latency deduction")
    }

    fn funding_deduction(&self, size: Decimal) -> Result<Decimal> {
        let years_f64 = self.assumptions.holding_period.as_years_f64().max(0.0);
        // A negative funding rate is money earned, not a negative cost. It
        // belongs in the gross figure, and a deduction refuses it outright.
        let cost_f64 = self.assumptions.funding_rate_annual_f64.max(0.0) * years_f64;
        let fraction = from_statistic(cost_f64, "funding cost")?;
        mul(size, fraction.max(Decimal::ZERO), "funding deduction")
    }

    fn collateral_deduction(&self, size: Decimal, all_atomic: bool) -> Result<Decimal> {
        let posted_f64 = if all_atomic {
            self.assumptions.collateral_fraction_f64.max(0.0)
        } else {
            // Nothing about a venue's margin rule helps when the leg it is
            // margining can revert: the inventory has to be there in full.
            self.assumptions.collateral_fraction_f64.max(1.0)
        };
        let years_f64 = self.assumptions.holding_period.as_years_f64().max(0.0);
        let cost_f64 =
            posted_f64 * self.assumptions.collateral_rate_annual_f64.max(0.0) * years_f64;
        let fraction = from_statistic(cost_f64, "collateral cost")?;
        mul(size, fraction.max(Decimal::ZERO), "collateral deduction")
    }

    /// The haircut, and the confidence it came from.
    ///
    /// Confidence is a freshness term times a sample term, both in `(0, 1]`, so
    /// the haircut can approach a total write-off without ever quite reaching
    /// one — by which point the staleness limit has already refused the path.
    /// It is applied to what is left after the real costs: the platform banks
    /// only the fraction of its own estimate it is prepared to stand behind.
    fn uncertainty_deduction(
        &self,
        before_uncertainty: Decimal,
        age: Duration,
        observations: u32,
    ) -> Result<(Decimal, f64)> {
        let half_life_f64 = self.assumptions.confidence_half_life.as_years_f64();
        let freshness_f64 = if half_life_f64 <= 0.0 {
            1.0
        } else {
            (-(age.as_years_f64() / half_life_f64)).exp()
        };
        let observed_f64 = f64::from(observations);
        let prior_f64 = f64::from(self.assumptions.prior_observations.max(1));
        let sample_f64 = observed_f64 / (observed_f64 + prior_f64);
        let confidence_f64 = (freshness_f64 * sample_f64).clamp(0.0, 1.0);

        let haircut = from_statistic(1.0 - confidence_f64, "uncertainty haircut")?;
        // Nothing to discount when the estimate is already a loss; the path is
        // refused on its own numbers rather than on its confidence.
        let exposed = before_uncertainty.max(Decimal::ZERO);
        Ok((
            mul(exposed, haircut, "uncertainty deduction")?,
            confidence_f64,
        ))
    }
}

/// Elapsed time, floored at zero.
///
/// A book stamped after the moment being reasoned about is a clock problem, not
/// a negative age, and a negative age would turn the freshness term into a
/// bonus.
fn elapsed(from: Timestamp, to: Timestamp) -> Duration {
    Duration::from_nanos(to.since(from).as_nanos().max(0))
}

/// Spread and slippage across the path, as fractions of the starting size.
///
/// Each leg's cost is weighted by its share of its own conversion's notional,
/// so a synthetic with five components is charged one conversion's worth of
/// spread rather than five.
fn book_cost_fractions(pricing: &PathPricing) -> Result<(Decimal, Decimal)> {
    let mut spread = Decimal::ZERO;
    let mut slippage = Decimal::ZERO;
    for conversion in &pricing.conversions {
        let notional = conversion.notional()?;
        if notional <= Decimal::ZERO {
            continue;
        }
        for leg in &conversion.legs {
            let weight = div(leg.notional()?, notional, "leg weight")?;
            spread += mul(weight, leg.spread_fraction()?, "weighted spread")?;
            slippage += mul(weight, leg.slippage_fraction()?, "weighted slippage")?;
        }
    }
    Ok((spread, slippage))
}
