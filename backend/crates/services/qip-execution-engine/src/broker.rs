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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulationSettings {
    /// Commission as a fraction of notional.
    pub commission_rate: f64,
    /// Half-spread paid crossing the book, in basis points, used when no book
    /// is available.
    pub half_spread_bps: f64,
    /// Coefficient in the square-root impact law.
    pub impact_coefficient: f64,
    /// Fraction of the order that fills immediately; the rest is worked.
    ///
    /// Below one on purpose. A simulator that always fills in full teaches a
    /// strategy that liquidity is free, and the lesson is expensive to unlearn.
    pub immediate_fill_fraction: f64,
    /// Probability an order is rejected by the venue.
    ///
    /// Small but non-zero: an execution path never tested against a rejection
    /// is one that will meet its first rejection in production.
    pub rejection_probability: f64,
    /// Latency between submission and the first fill.
    pub latency: qip_core::Duration,
}

impl Default for SimulationSettings {
    fn default() -> Self {
        Self {
            commission_rate: 0.0001,
            half_spread_bps: 3.0,
            impact_coefficient: 1.0,
            immediate_fill_fraction: 0.6,
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
            commission_rate: 0.0,
            half_spread_bps: 0.0,
            impact_coefficient: 0.0,
            immediate_fill_fraction: 1.0,
            rejection_probability: 0.0,
            latency: qip_core::Duration::ZERO,
        }
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
        self.settings.commission_rate == 0.0
            && self.settings.half_spread_bps == 0.0
            && self.settings.impact_coefficient == 0.0
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
    fn fill_price(&self, order: &Order, quantity: Decimal) -> Decimal {
        let arrival = order.arrival_price;
        if let Some(book) = self.books.get(order.object_id.as_str()) {
            let side = match order.side {
                Side::Buy => qip_market::book::Side::Buy,
                Side::Sell => qip_market::book::Side::Sell,
            };
            if let Some((_, average)) = book.sweep(side, quantity) {
                return average;
            }
        }

        let notional = quantity.to_f64() * arrival.to_f64();
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
        let impact_bps = if volume > 0.0 && volatility > 0.0 {
            let participation = quantity.to_f64() / volume;
            self.settings.impact_coefficient * volatility * participation.sqrt() * 10_000.0
        } else {
            0.0
        };
        let cost_bps = self.settings.half_spread_bps + impact_bps;
        let _ = notional;
        // Costs move the price against the order, whichever way it goes.
        let adjustment = arrival.to_f64() * cost_bps / 10_000.0 * f64::from(order.side.sign());
        Decimal::from_f64(arrival.to_f64() + adjustment).unwrap_or(arrival)
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
            commission_rate: self.settings.commission_rate,
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
            let market = self.fill_price(order, remaining);
            let would_fill = match order.side {
                Side::Buy => market <= price,
                Side::Sell => market >= price,
            };
            if !would_fill {
                return Ok(Vec::new());
            }
        }

        let fraction = self.settings.immediate_fill_fraction.clamp(0.0, 1.0);
        let quantity = if fraction >= 1.0 {
            remaining
        } else {
            Decimal::from_f64(remaining.to_f64() * fraction).unwrap_or(remaining)
        };
        if quantity <= Decimal::ZERO {
            return Ok(Vec::new());
        }

        let price = self.fill_price(order, quantity);
        let costs =
            Decimal::from_f64(quantity.to_f64() * price.to_f64() * self.settings.commission_rate)
                .unwrap_or(Decimal::ZERO);

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
