//! The broker port, and the simulated venue behind it.
//!
//! [`Broker::is_simulated`] is not advisory. Every fill carries
//! [`crate::order::Fill::simulated`] copied from it, the OMS refuses to send a
//! live order below the required autonomy level, and a reconciliation can tell
//! paper from real without consulting configuration. The distinction is on the
//! data rather than in the environment, because environment is exactly what
//! gets confused between a test and a deployment.
//!
//! [`LiveBroker`] is the shape of a real adapter and reports itself unavailable
//! in this build: no venue credential, no FIX or REST transport, no egress. It
//! says precisely that rather than pretending, so a deployment configured for
//! live trading fails at start-up with a legible message rather than at the
//! first order with a confusing one.

use crate::order::{Fill, Order, OrderType, Side};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::FillId;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::Timestamp;
use qip_market::book::OrderBook;
use serde::{Deserialize, Serialize};
use std::fmt;

/// What a venue can do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueCapabilities {
    pub name: String,
    /// Order types the venue accepts.
    pub supported_types: Vec<String>,
    /// Whether partial fills are possible.
    pub partial_fills: bool,
    /// Smallest tradable increment.
    pub lot_size: Decimal,
    /// Commission as a fraction of notional.
    pub commission_rate: f64,
}

/// A venue orders can be sent to.
pub trait Broker: Send + Sync + fmt::Debug {
    fn name(&self) -> &str;

    /// Whether this is a simulated venue.
    ///
    /// The single most consequential bit in the execution path. Everything
    /// that could confuse a paper fill with a real one keys off it.
    fn is_simulated(&self) -> bool;

    /// Whether the venue can be reached.
    fn is_available(&self) -> bool;

    fn capabilities(&self) -> VenueCapabilities;

    /// Submit an order. Returns the fills the venue produced.
    fn submit(&mut self, order: &Order, at: Timestamp) -> Result<Vec<Fill>>;

    /// Cancel a working order.
    fn cancel(&mut self, order: &Order, at: Timestamp) -> Result<()>;

    /// What a deployment would need to make this usable. Empty when available.
    fn requirement(&self) -> String {
        String::new()
    }
}

/// How the simulated venue fills orders.
///
/// The fields are private, and that is the point. They used to be public and
/// the fill fraction was `clamp`ed at the point of use, so an operator who
/// configured `1.6` got `1.0` and an operator who configured `-1.0` got no
/// fills, in both cases without a word. A value silently corrected is a caller
/// bug that survives into every backtest run afterwards. The only ways to build
/// these settings now are [`SimulationSettings::default`],
/// [`SimulationSettings::frictionless`], and the `with_*` methods, each of
/// which refuses a value it cannot honour and names the range it wanted.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SimulationSettings {
    /// Commission as a fraction of notional.
    ///
    /// `Decimal` rather than `f64` because it multiplies a notional to produce
    /// booked money, and money is never floating point here.
    commission_rate: Decimal,
    /// Half-spread paid crossing the book, in basis points, used when no book
    /// is available. A model parameter, so `f64`.
    half_spread_bps: f64,
    /// Coefficient in the square-root impact law. A model parameter, so `f64`.
    impact_coefficient: f64,
    /// Fraction of the order that fills immediately; the rest is worked.
    ///
    /// Below one on purpose. A simulator that always fills in full teaches a
    /// strategy that liquidity is free, and the lesson is expensive to unlearn.
    ///
    /// `Decimal` because it multiplies an order quantity.
    immediate_fill_fraction: Decimal,
    /// Probability an order is rejected by the venue.
    ///
    /// Small but non-zero: an execution path never tested against a rejection
    /// is one that will meet its first rejection in production.
    rejection_probability: f64,
    /// Latency between submission and the first fill.
    latency: qip_core::Duration,
}

impl Default for SimulationSettings {
    fn default() -> Self {
        Self {
            // Decimal::from_raw takes units of 10^-9: 0.0001 and 0.6 exactly.
            commission_rate: Decimal::from_raw(100_000),
            half_spread_bps: 3.0,
            impact_coefficient: 1.0,
            immediate_fill_fraction: Decimal::from_raw(600_000_000),
            rejection_probability: 0.005,
            latency: qip_core::Duration::from_millis(50),
        }
    }
}

impl SimulationSettings {
    /// Settings that fill everything instantly at the arrival price.
    ///
    /// For isolating a strategy's raw signal. Never for a result presented as
    /// achievable, and [`SimulatedBroker::is_frictionless`] says so.
    pub fn frictionless() -> Self {
        Self {
            commission_rate: Decimal::ZERO,
            half_spread_bps: 0.0,
            impact_coefficient: 0.0,
            immediate_fill_fraction: Decimal::ONE,
            rejection_probability: 0.0,
            latency: qip_core::Duration::ZERO,
        }
    }

    pub fn commission_rate(&self) -> Decimal {
        self.commission_rate
    }

    pub fn half_spread_bps(&self) -> f64 {
        self.half_spread_bps
    }

    pub fn impact_coefficient(&self) -> f64 {
        self.impact_coefficient
    }

    pub fn immediate_fill_fraction(&self) -> Decimal {
        self.immediate_fill_fraction
    }

    pub fn rejection_probability(&self) -> f64 {
        self.rejection_probability
    }

    pub fn latency(&self) -> qip_core::Duration {
        self.latency
    }

    /// Commission as a fraction of notional, in `[0, 1]`.
    pub fn with_commission_rate(mut self, rate: Decimal) -> Result<Self> {
        if rate < Decimal::ZERO || rate > Decimal::ONE {
            return Err(Error::invalid(format!(
                "a commission rate of {rate} is not a fraction of notional; supply a rate between 0 and 1 inclusive, where 0.0001 is one basis point"
            )));
        }
        self.commission_rate = rate;
        Ok(self)
    }

    /// Half-spread in basis points; finite and not negative.
    pub fn with_half_spread_bps(mut self, bps: f64) -> Result<Self> {
        if !bps.is_finite() || bps < 0.0 {
            return Err(Error::invalid(format!(
                "a half-spread of {bps} basis points would pay the order to cross; supply a finite value of zero or more"
            )));
        }
        self.half_spread_bps = bps;
        Ok(self)
    }

    /// Coefficient in the square-root impact law; finite and not negative.
    pub fn with_impact_coefficient(mut self, coefficient: f64) -> Result<Self> {
        if !coefficient.is_finite() || coefficient < 0.0 {
            return Err(Error::invalid(format!(
                "an impact coefficient of {coefficient} would make size improve the fill price; supply a finite value of zero or more"
            )));
        }
        self.impact_coefficient = coefficient;
        Ok(self)
    }

    /// Fraction of an order filled immediately, in `(0, 1]`.
    ///
    /// Zero is refused along with the out-of-range values: a simulator that
    /// fills nothing is not a conservative simulator, it is a broken one, and
    /// it would report every strategy as never having traded.
    pub fn with_immediate_fill_fraction(mut self, fraction: Decimal) -> Result<Self> {
        if fraction <= Decimal::ZERO || fraction > Decimal::ONE {
            return Err(Error::invalid(format!(
                "an immediate fill fraction of {fraction} is not a fraction of an order; supply a value greater than 0 and at most 1, where 1 fills the whole order at once"
            )));
        }
        self.immediate_fill_fraction = fraction;
        Ok(self)
    }

    /// Probability of a venue rejection, in `[0, 1]`.
    pub fn with_rejection_probability(mut self, probability: f64) -> Result<Self> {
        if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
            return Err(Error::invalid(format!(
                "a rejection probability of {probability} is not a probability; supply a finite value between 0 and 1 inclusive"
            )));
        }
        self.rejection_probability = probability;
        Ok(self)
    }

    pub fn with_latency(mut self, latency: qip_core::Duration) -> Self {
        self.latency = latency;
        self
    }
}

/// Deserialization goes through the same refusals as the builders.
///
/// A settings blob read from a file is exactly the case the clamp used to
/// swallow, so the validation has to sit here too or the boundary has a hole
/// in the shape of `serde`.
impl<'de> Deserialize<'de> for SimulationSettings {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as DeError;

        #[derive(Deserialize)]
        struct Wire {
            commission_rate: Decimal,
            half_spread_bps: f64,
            impact_coefficient: f64,
            immediate_fill_fraction: Decimal,
            rejection_probability: f64,
            latency: qip_core::Duration,
        }

        let wire = Wire::deserialize(d)?;
        SimulationSettings::default()
            .with_commission_rate(wire.commission_rate)
            .and_then(|s| s.with_half_spread_bps(wire.half_spread_bps))
            .and_then(|s| s.with_impact_coefficient(wire.impact_coefficient))
            .and_then(|s| s.with_immediate_fill_fraction(wire.immediate_fill_fraction))
            .and_then(|s| s.with_rejection_probability(wire.rejection_probability))
            .map(|s| s.with_latency(wire.latency))
            .map_err(|e| D::Error::custom(e.message().to_string()))
    }
}

/// A venue that fills orders without touching a market.
#[derive(Debug)]
pub struct SimulatedBroker {
    settings: SimulationSettings,
    rng: Xoshiro256,
    /// Current books by instrument, when the caller supplies them.
    books: std::collections::BTreeMap<String, OrderBook>,
    /// Recent daily volume by instrument, for the impact model.
    daily_volumes: std::collections::BTreeMap<String, f64>,
    /// Daily volatility by instrument, for the impact model.
    daily_volatility: std::collections::BTreeMap<String, f64>,
    sequence: u64,
    submitted: usize,
    rejected: usize,
}

impl SimulatedBroker {
    pub fn new(settings: SimulationSettings, seed: u64) -> Self {
        Self {
            settings,
            rng: Xoshiro256::seeded(seed).fork("simulated-broker"),
            books: std::collections::BTreeMap::new(),
            daily_volumes: std::collections::BTreeMap::new(),
            daily_volatility: std::collections::BTreeMap::new(),
            sequence: 0,
            submitted: 0,
            rejected: 0,
        }
    }

    pub fn is_frictionless(&self) -> bool {
        self.settings.commission_rate.is_zero()
            && self.settings.half_spread_bps == 0.0
            && self.settings.impact_coefficient == 0.0
    }

    pub fn settings(&self) -> &SimulationSettings {
        &self.settings
    }

    /// Supply the book for an instrument, so fills price against real depth.
    pub fn set_book(&mut self, book: OrderBook) {
        self.books.insert(book.object_id.as_str().to_string(), book);
    }

    /// Supply liquidity statistics for the impact model.
    pub fn set_liquidity(&mut self, object_id: &str, daily_volume: f64, daily_volatility: f64) {
        self.daily_volumes
            .insert(object_id.to_string(), daily_volume);
        self.daily_volatility
            .insert(object_id.to_string(), daily_volatility);
    }

    pub fn submitted_count(&self) -> usize {
        self.submitted
    }

    pub fn rejected_count(&self) -> usize {
        self.rejected
    }

    /// The price this order would fill at.
    ///
    /// Uses the book where one is available — sweeping real depth is the
    /// honest answer — and falls back to arrival plus a modelled cost where it
    /// is not.
    ///
    /// Fallible on purpose. This used to end in
    /// `Decimal::from_f64(...).unwrap_or(arrival)`, so a cost the conversion
    /// could not represent became a fill at the arrival price — zero slippage,
    /// the single most flattering answer available, delivered silently. Every
    /// backtest run through this venue inherited the bias. A price that cannot
    /// be computed is now a refusal naming the setting to look at.
    fn fill_price(&self, order: &Order, quantity: Decimal) -> Result<Decimal> {
        let arrival = order.arrival_price;
        if let Some(book) = self.books.get(order.object_id.as_str()) {
            let side = match order.side {
                Side::Buy => qip_market::book::Side::Buy,
                Side::Sell => qip_market::book::Side::Sell,
            };
            if let Some((_, average)) = book.sweep(side, quantity) {
                return Ok(average);
            }
        }

        let volume = self
            .daily_volumes
            .get(order.object_id.as_str())
            .copied()
            .unwrap_or(0.0);
        let volatility = self
            .daily_volatility
            .get(order.object_id.as_str())
            .copied()
            .unwrap_or(0.0);
        // The crossing from money to statistics happens here and only here.
        // The square-root impact law has no exact fixed-point form, and
        // participation, volume and volatility are estimates rather than
        // amounts, so the *rate* is computed in `f64`. It is converted back to
        // `Decimal` before it touches a price, and everything from that point
        // on is exact.
        let impact_bps = if volume > 0.0 && volatility > 0.0 {
            let participation = quantity.to_f64() / volume;
            self.settings.impact_coefficient * volatility * participation.sqrt() * 10_000.0
        } else {
            0.0
        };
        let cost_bps = self.settings.half_spread_bps + impact_bps;
        let Some(cost_fraction) = Decimal::from_f64(cost_bps / 10_000.0) else {
            return Err(Error::numeric(format!(
                "the modelled execution cost for order {} came to {cost_bps} basis points, which is not a representable price adjustment; reduce half_spread_bps or impact_coefficient, or supply a book for {}",
                order.order_id.as_str(),
                order.object_id.as_str()
            )));
        };
        // Costs move the price against the order, whichever way it goes.
        let signed = match order.side.sign() {
            s if s < 0 => -cost_fraction,
            _ => cost_fraction,
        };
        let Some(price) = arrival
            .checked_mul(signed)
            .and_then(|adjustment| arrival.checked_add(adjustment))
        else {
            return Err(Error::numeric(format!(
                "applying {cost_bps} basis points of cost to an arrival price of {arrival} overflowed for order {}; the arrival price or the cost settings are wrong",
                order.order_id.as_str()
            )));
        };
        Ok(price)
    }
}

impl Broker for SimulatedBroker {
    fn name(&self) -> &str {
        "simulated-venue"
    }

    fn is_simulated(&self) -> bool {
        true
    }

    fn is_available(&self) -> bool {
        true
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            name: "simulated-venue".to_string(),
            supported_types: vec![
                "market".to_string(),
                "limit".to_string(),
                "twap".to_string(),
                "vwap".to_string(),
                "participation".to_string(),
            ],
            partial_fills: true,
            lot_size: Decimal::from_int(1),
            // `VenueCapabilities` is a description of a venue rather than a
            // ledger entry, and its field is `f64` because other adapters
            // report one. The authoritative rate stays `Decimal` in the
            // settings; nothing books money from this copy.
            commission_rate: self.settings.commission_rate.to_f64(),
        }
    }

    fn submit(&mut self, order: &Order, at: Timestamp) -> Result<Vec<Fill>> {
        order.validate()?;
        self.submitted += 1;

        if self.rng.next_f64() < self.settings.rejection_probability {
            self.rejected += 1;
            return Err(Error::unavailable(format!(
                "the venue rejected order {}",
                order.order_id.as_str()
            )));
        }

        let remaining = order.remaining_quantity();
        if remaining <= Decimal::ZERO {
            return Ok(Vec::new());
        }

        // A limit order only fills if the market reaches it.
        if let OrderType::Limit { price } = order.order_type {
            let market = self.fill_price(order, remaining)?;
            let would_fill = match order.side {
                Side::Buy => market <= price,
                Side::Sell => market >= price,
            };
            if !would_fill {
                return Ok(Vec::new());
            }
        }

        // No clamp: the fraction was validated when the settings were built,
        // and the multiplication is exact fixed point rather than a round trip
        // through `f64`.
        let fraction = self.settings.immediate_fill_fraction;
        let quantity = if fraction >= Decimal::ONE {
            remaining
        } else {
            let Some(quantity) = remaining.checked_mul(fraction) else {
                return Err(Error::numeric(format!(
                    "filling {fraction} of the {remaining} remaining on order {} overflowed; the order quantity is beyond what this venue can model",
                    order.order_id.as_str()
                )));
            };
            quantity
        };
        if quantity <= Decimal::ZERO {
            return Ok(Vec::new());
        }

        let price = self.fill_price(order, quantity)?;
        // Commission used to be `Decimal::from_f64(...).unwrap_or(Decimal::ZERO)`,
        // so a computation that failed booked a free trade. Free is the most
        // favourable answer there is, and a simulator that fails favourably
        // flatters every strategy measured against it.
        let Some(costs) = quantity
            .checked_mul(price)
            .and_then(|notional| notional.checked_mul(self.settings.commission_rate))
        else {
            return Err(Error::numeric(format!(
                "commission on {quantity} at {price} overflowed for order {}; the fill is too large to price and must not be booked without its cost",
                order.order_id.as_str()
            )));
        };

        self.sequence += 1;
        Ok(vec![Fill {
            fill_id: FillId::from_string(format!("fill-{}", self.sequence)),
            order_id: order.order_id.clone(),
            at: at.saturating_add(self.settings.latency),
            quantity,
            price,
            costs,
            venue: self.name().to_string(),
            // The bit that keeps a paper fill from ever being mistaken for a
            // real one.
            simulated: true,
        }])
    }

    fn cancel(&mut self, _order: &Order, _at: Timestamp) -> Result<()> {
        Ok(())
    }
}

/// How a live venue is configured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveVenueConfig {
    pub venue: String,
    /// Environment variable holding the credential. Never the credential
    /// itself: a secret in a configuration struct is a secret in a log.
    pub credential_env: String,
    pub endpoint: String,
    /// The account orders would be sent for.
    pub account: String,
    /// Autonomy level required before this venue accepts anything.
    pub required_autonomy: String,
}

/// An adapter to a real venue.
///
/// The interface is complete; the transport is not. This build has no venue
/// credential, no FIX or REST client and no egress path, so the adapter reports
/// itself unavailable and says exactly what is missing.
#[derive(Debug)]
pub struct LiveBroker {
    config: LiveVenueConfig,
    credential_present: bool,
    transport_present: bool,
    /// Whether an operator has explicitly enabled this venue.
    ///
    /// Separate from the credential: having a token is not the same as having
    /// decided to trade.
    enabled_by_operator: bool,
}

impl LiveBroker {
    pub fn new(config: LiveVenueConfig) -> Self {
        Self {
            config,
            credential_present: false,
            transport_present: false,
            enabled_by_operator: false,
        }
    }

    /// Construct with the availability inputs set, for testing the logic.
    pub fn configured(
        config: LiveVenueConfig,
        credential_present: bool,
        enabled_by_operator: bool,
    ) -> Self {
        Self {
            config,
            credential_present,
            transport_present: false,
            enabled_by_operator,
        }
    }

    pub fn config(&self) -> &LiveVenueConfig {
        &self.config
    }
}

impl Broker for LiveBroker {
    fn name(&self) -> &str {
        &self.config.venue
    }

    fn is_simulated(&self) -> bool {
        false
    }

    fn is_available(&self) -> bool {
        self.credential_present && self.transport_present && self.enabled_by_operator
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            name: self.config.venue.clone(),
            supported_types: vec!["market".to_string(), "limit".to_string()],
            partial_fills: true,
            lot_size: Decimal::from_int(1),
            commission_rate: 0.0005,
        }
    }

    fn submit(&mut self, _order: &Order, _at: Timestamp) -> Result<Vec<Fill>> {
        Err(Error::unavailable(self.requirement()))
    }

    fn cancel(&mut self, _order: &Order, _at: Timestamp) -> Result<()> {
        Err(Error::unavailable(self.requirement()))
    }

    fn requirement(&self) -> String {
        let mut missing = Vec::new();
        if !self.credential_present {
            missing.push(format!(
                "a venue credential in the environment variable {}",
                self.config.credential_env
            ));
        }
        if !self.transport_present {
            missing.push(
                "a FIX or REST transport with TLS, which is not present in this build".to_string(),
            );
        }
        if !self.enabled_by_operator {
            missing.push(
                "an explicit operator enablement for this venue; holding a credential is not the same as having decided to trade"
                    .to_string(),
            );
        }
        format!(
            "{} at {} is not usable for account {}: missing {}. Orders are routed to the simulated venue, which is the configured default.",
            self.config.venue,
            self.config.endpoint,
            self.config.account,
            missing.join("; and ")
        )
    }
}
