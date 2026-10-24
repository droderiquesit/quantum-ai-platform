//! The venue-facing surface, and a simulator that stands where a venue would.
//!
//! [`Gateway::is_simulated`] is not advisory. It is the bit that decides
//! whether a fill is real, and it lives on the implementation rather than in
//! configuration, because configuration is exactly what gets confused between a
//! test and a deployment.
//!
//! [`NativeGateway`] is the shape of a real adapter and reports itself
//! unavailable in this build: there is no venue credential, no FIX or WebSocket
//! session, and no egress. It says precisely that, and
//! [`Gateway::missing_credentials`] enumerates what a production deployment
//! would have to supply, so a misconfigured deployment fails at start-up with a
//! legible message rather than at the first order with a confusing one.
//!
//! [`SimulatedGateway`] names the same list. It is standing in for something,
//! and a simulator that presented itself as complete would be the more
//! dangerous of the two.
//!
//! The transmit path and the venue's answer are deliberately different
//! channels. `send` returning `Ok` means the order left; what the venue did
//! with it arrives through [`Gateway::drain`]. A rejection is an event, not an
//! error, because it is something the venue said rather than something that
//! went wrong on the way there.

use crate::children::ChildOrder;
use crate::ordertype::{OrderTypeKind, RoutedOrderType};
use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, ObjectId};
use qip_market::book::OrderBook;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// One thing a production deployment has to supply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayCredential {
    /// What it is, in words an operator would recognise.
    pub name: String,
    /// Environment variable it is read from. Never the value: a secret in a
    /// struct is a secret in a log.
    pub env_var: String,
    /// What breaks without it.
    pub purpose: String,
}

impl GatewayCredential {
    pub fn new(
        name: impl Into<String>,
        env_var: impl Into<String>,
        purpose: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            env_var: env_var.into(),
            purpose: purpose.into(),
        }
    }
}

/// What the venue said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum GatewayEvent {
    Accepted {
        client_id: String,
        at: Timestamp,
    },
    Rejected {
        client_id: String,
        at: Timestamp,
        reason: String,
    },
    Filled {
        client_id: String,
        at: Timestamp,
        quantity: Decimal,
        price: Decimal,
    },
    Cancelled {
        client_id: String,
        at: Timestamp,
        reason: String,
    },
    Replaced {
        client_id: String,
        at: Timestamp,
        quantity: Decimal,
        price: Option<Decimal>,
    },
}

impl GatewayEvent {
    pub fn client_id(&self) -> &str {
        match self {
            Self::Accepted { client_id, .. }
            | Self::Rejected { client_id, .. }
            | Self::Filled { client_id, .. }
            | Self::Cancelled { client_id, .. }
            | Self::Replaced { client_id, .. } => client_id,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Accepted { .. } => "accepted",
            Self::Rejected { .. } => "rejected",
            Self::Filled { .. } => "filled",
            Self::Cancelled { .. } => "cancelled",
            Self::Replaced { .. } => "replaced",
        }
    }
}

/// Confirmation that an instruction left for the venue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GatewayAck {
    pub client_id: String,
    pub venue: VenueId,
    pub at: Timestamp,
    /// Copied from [`Gateway::is_simulated`] at the moment of sending, so a
    /// paper fill cannot be reclassified later by reconfiguring something.
    pub simulated: bool,
    pub latency: Duration,
}

/// Send, cancel, replace: everything a venue is asked to do.
pub trait Gateway: fmt::Debug + Send + Sync {
    fn venue(&self) -> &VenueId;

    /// Whether this is a simulator.
    ///
    /// The single most consequential bit in the routing path.
    fn is_simulated(&self) -> bool;

    /// Whether instructions can be transmitted at all.
    fn is_available(&self) -> bool;

    /// What a production deployment must supply to make this a real venue
    /// connection. Empty only for an adapter that is genuinely complete.
    fn missing_credentials(&self) -> Vec<GatewayCredential>;

    fn send(&mut self, child: &ChildOrder, at: Timestamp) -> Result<GatewayAck>;

    fn cancel(&mut self, client_id: &str, at: Timestamp) -> Result<GatewayAck>;

    /// Amend a working order. `price` of `None` leaves the price alone.
    fn replace(
        &mut self,
        client_id: &str,
        quantity: Decimal,
        price: Option<Decimal>,
        at: Timestamp,
    ) -> Result<GatewayAck>;

    /// Take everything the venue has said since the last call.
    fn drain(&mut self) -> Vec<GatewayEvent>;

    /// A sentence an operator can act on.
    fn requirement(&self) -> String {
        let missing = self.missing_credentials();
        if missing.is_empty() {
            return format!("{} needs nothing further", self.venue().as_str());
        }
        let parts: Vec<String> = missing
            .iter()
            .map(|credential| {
                format!(
                    "{} (from {}), without which {}",
                    credential.name, credential.env_var, credential.purpose
                )
            })
            .collect();
        format!(
            "a production deployment of {} must supply: {}",
            self.venue().as_str(),
            parts.join("; ")
        )
    }
}

/// What a production adapter for any venue has to be given.
///
/// Listed once and returned by both implementations, so the simulator and the
/// unfinished real adapter cannot drift into disagreeing about what is missing.
fn production_requirements(venue: &VenueId) -> Vec<GatewayCredential> {
    let prefix = venue.as_str().to_ascii_uppercase().replace('-', "_");
    vec![
        GatewayCredential::new(
            "a venue API key or FIX session credential",
            format!("QIP_{prefix}_CREDENTIAL"),
            "no order can be authenticated and every send is refused",
        ),
        GatewayCredential::new(
            "a session endpoint and TLS trust store",
            format!("QIP_{prefix}_ENDPOINT"),
            "there is no transport, and this build ships none",
        ),
        GatewayCredential::new(
            "the account or sub-account orders are sent for",
            format!("QIP_{prefix}_ACCOUNT"),
            "fills cannot be attributed to a book",
        ),
        GatewayCredential::new(
            "an explicit operator enablement for live trading",
            format!("QIP_{prefix}_ENABLED"),
            "holding a credential is not the same as having decided to trade",
        ),
        GatewayCredential::new(
            "a clock discipline source for the venue's session",
            format!("QIP_{prefix}_TIME_SOURCE"),
            "acknowledgement latency cannot be measured, so venue health is blind",
        ),
    ]
}

/// How the simulator behaves.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GatewaySettings {
    /// Probability the venue refuses an order outright.
    ///
    /// Small and non-zero on purpose. A routing path never tested against a
    /// rejection is one that meets its first rejection in production.
    pub rejection_probability_f64: f64,
    /// Share of a marketable order that fills on the first pass.
    ///
    /// Below one on purpose. A simulator that always fills in full teaches a
    /// router that liquidity is free.
    pub immediate_fill_fraction_f64: f64,
    /// Round trip to an acknowledgement.
    pub latency: Duration,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            rejection_probability_f64: 0.005,
            immediate_fill_fraction_f64: 0.7,
            latency: Duration::from_millis(5),
        }
    }
}

impl GatewaySettings {
    /// Fills everything, refuses nothing, takes no time.
    ///
    /// For isolating the accounting from the market. Never for a result
    /// presented as achievable — [`SimulatedGateway::is_frictionless`] says so
    /// out loud.
    pub fn frictionless() -> Self {
        Self {
            rejection_probability_f64: 0.0,
            immediate_fill_fraction_f64: 1.0,
            latency: Duration::ZERO,
        }
    }
}

/// An order a venue is still holding.
///
/// Exposed because reconciliation is not optional: an operator comparing the
/// platform's view against the venue's needs the venue's view, and a simulator
/// that kept its resting orders private would be untestable in exactly the way
/// that matters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkingOrder {
    pub object_id: ObjectId,
    pub side: BookSide,
    pub quantity: Decimal,
    pub filled: Decimal,
    pub order_type: RoutedOrderType,
}

impl WorkingOrder {
    pub fn remaining(&self) -> Decimal {
        self.quantity - self.filled
    }
}

/// A venue that fills orders without touching a market.
///
/// **It has no public way to rest a book, and so outside this module's own
/// tests it trades nothing: every child is answered `does not trade`.**
/// Resting a book at a venue is listing an instrument there, and listing is
/// one of the three governed actions (R12-COVERAGE-5107): it needs a
/// `CreationAdmission` from `qip_execution_engine::creation`, whose one mint
/// refuses unless the venue, the product rules and the jurisdiction have each
/// recorded a permission. This crate cannot name that type — it expresses
/// strategies, and `qip-acceptance`'s `architecture.rs`
/// (`no_crate_that_expresses_a_strategy_can_reach_a_facility_that_sends_an_order`)
/// refuses any edge from it to `qip-execution-engine` — so it cannot check
/// one either. Until 2026-10-24 it had `set_book` and `with_book`, which took
/// a bare book from any caller: an instrument nobody had permitted could be
/// made tradeable here by whoever held the gateway, with no record of
/// anybody's permission. They had no production caller, and they are gone
/// rather than left as an ungated door for the next one. The simulated venue
/// that does list instruments is `qip_brokers::exchange::SimulatedExchange`,
/// and it demands the admission.
#[derive(Debug)]
pub struct SimulatedGateway {
    venue: VenueId,
    settings: GatewaySettings,
    rng: Xoshiro256,
    /// Written only by this module's tests, for the reason the type's own
    /// documentation gives.
    books: BTreeMap<ObjectId, OrderBook>,
    live: BTreeMap<String, WorkingOrder>,
    events: Vec<GatewayEvent>,
    sent: u64,
    rejected: u64,
}

impl SimulatedGateway {
    pub fn new(venue: VenueId, settings: GatewaySettings, seed: u64) -> Self {
        let label = format!("simulated-gateway-{}", venue.as_str());
        Self {
            venue,
            settings,
            rng: Xoshiro256::seeded(seed).fork(&label),
            books: BTreeMap::new(),
            live: BTreeMap::new(),
            events: Vec::new(),
            sent: 0,
            rejected: 0,
        }
    }

    pub fn sent_count(&self) -> u64 {
        self.sent
    }

    pub fn rejected_count(&self) -> u64 {
        self.rejected
    }

    /// Everything still resting at this venue, by client id.
    pub fn working(&self) -> &BTreeMap<String, WorkingOrder> {
        &self.live
    }

    pub fn working_count(&self) -> usize {
        self.live.len()
    }

    /// Whether this simulator has been configured to make execution free.
    pub fn is_frictionless(&self) -> bool {
        self.settings.rejection_probability_f64 <= 0.0
            && self.settings.immediate_fill_fraction_f64 >= 1.0
    }

    /// Quantity and volume-weighted price available at a limit, or better.
    ///
    /// `None` for the limit means marketable at any price.
    fn available(
        book: &OrderBook,
        side: BookSide,
        limit: Option<Decimal>,
    ) -> Option<(Decimal, Decimal)> {
        // Buying consumes offers, selling consumes bids. Both sides are held in
        // book order, so walking stops at the first unacceptable price.
        let levels = match side {
            BookSide::Ask => &book.asks,
            BookSide::Bid => &book.bids,
        };
        let mut quantity = Decimal::ZERO;
        let mut consideration = Decimal::ZERO;
        for level in levels {
            let acceptable = limit.is_none_or(|limit| match side {
                BookSide::Ask => level.price <= limit,
                BookSide::Bid => level.price >= limit,
            });
            if !acceptable {
                break;
            }
            quantity += level.size;
            consideration += level.size.checked_mul(level.price)?;
        }
        if quantity <= Decimal::ZERO {
            return None;
        }
        Some((quantity, consideration.checked_div(quantity)?))
    }

    fn immediate_share(&self, quantity: Decimal) -> Decimal {
        let fraction = self.settings.immediate_fill_fraction_f64.clamp(0.0, 1.0);
        if fraction >= 1.0 {
            return quantity;
        }
        Decimal::from_f64(fraction)
            .and_then(|fraction| quantity.checked_mul(fraction))
            .unwrap_or(quantity)
    }

    fn ack(&self, client_id: &str, at: Timestamp) -> GatewayAck {
        GatewayAck {
            client_id: client_id.to_string(),
            venue: self.venue.clone(),
            at: at.saturating_add(self.settings.latency),
            simulated: true,
            latency: self.settings.latency,
        }
    }
}

impl Gateway for SimulatedGateway {
    fn venue(&self) -> &VenueId {
        &self.venue
    }

    fn is_simulated(&self) -> bool {
        true
    }

    fn is_available(&self) -> bool {
        true
    }

    fn missing_credentials(&self) -> Vec<GatewayCredential> {
        // The simulator is complete for what it is and incomplete as a venue.
        // Reporting the second is the honest answer to "what would it take to
        // run this for real".
        production_requirements(&self.venue)
    }

    fn send(&mut self, child: &ChildOrder, at: Timestamp) -> Result<GatewayAck> {
        if child.venue != self.venue {
            return Err(Error::invalid(format!(
                "child {} is addressed to {}, not {}",
                child.client_id,
                child.venue.as_str(),
                self.venue.as_str()
            )));
        }
        if self.live.contains_key(&child.client_id) {
            return Err(Error::invalid(format!(
                "client id {} is already working",
                child.client_id
            )));
        }
        self.sent = self.sent.saturating_add(1);
        let landed = at.saturating_add(self.settings.latency);

        if self.rng.bernoulli(self.settings.rejection_probability_f64) {
            self.rejected = self.rejected.saturating_add(1);
            self.events.push(GatewayEvent::Rejected {
                client_id: child.client_id.clone(),
                at: landed,
                reason: "the venue refused the order".to_string(),
            });
            return Ok(self.ack(&child.client_id, at));
        }

        let Some(book) = self.books.get(&child.object_id) else {
            self.rejected = self.rejected.saturating_add(1);
            self.events.push(GatewayEvent::Rejected {
                client_id: child.client_id.clone(),
                at: landed,
                reason: format!(
                    "{} does not trade {}",
                    self.venue.as_str(),
                    child.object_id.as_str()
                ),
            });
            return Ok(self.ack(&child.client_id, at));
        };

        self.events.push(GatewayEvent::Accepted {
            client_id: child.client_id.clone(),
            at: landed,
        });

        let limit = child.order_type.limit_price();
        let marketable = Self::available(book, child.side, limit);
        let kind = child.order_type.kind();

        match kind {
            OrderTypeKind::FillOrKill => match marketable {
                Some((available, price)) if available >= child.quantity => {
                    self.events.push(GatewayEvent::Filled {
                        client_id: child.client_id.clone(),
                        at: landed,
                        quantity: child.quantity,
                        price,
                    });
                }
                _ => {
                    self.events.push(GatewayEvent::Cancelled {
                        client_id: child.client_id.clone(),
                        at: landed,
                        reason: "all or none, and the book could not do all of it".to_string(),
                    });
                }
            },
            OrderTypeKind::Market | OrderTypeKind::ImmediateOrCancel => {
                let share = self.immediate_share(child.quantity);
                match marketable {
                    Some((available, price)) if available > Decimal::ZERO => {
                        let filled = available.min(share);
                        if filled > Decimal::ZERO {
                            self.events.push(GatewayEvent::Filled {
                                client_id: child.client_id.clone(),
                                at: landed,
                                quantity: filled,
                                price,
                            });
                        }
                        if filled < child.quantity {
                            self.events.push(GatewayEvent::Cancelled {
                                client_id: child.client_id.clone(),
                                at: landed,
                                reason: format!(
                                    "{} of {} could not be taken immediately",
                                    child.quantity - filled,
                                    child.quantity
                                ),
                            });
                        }
                    }
                    _ => {
                        self.events.push(GatewayEvent::Cancelled {
                            client_id: child.client_id.clone(),
                            at: landed,
                            reason: "nothing was marketable at that limit".to_string(),
                        });
                    }
                }
            }
            OrderTypeKind::Limit | OrderTypeKind::Peg => {
                // A resting order rests. It fills when the market comes to it,
                // which the simulator models by leaving it working rather than
                // by pretending the market already did.
                let filled = match marketable {
                    Some((available, price)) if available > Decimal::ZERO => {
                        let share = self.immediate_share(child.quantity).min(available);
                        if share > Decimal::ZERO {
                            self.events.push(GatewayEvent::Filled {
                                client_id: child.client_id.clone(),
                                at: landed,
                                quantity: share,
                                price,
                            });
                        }
                        share
                    }
                    _ => Decimal::ZERO,
                };
                if filled < child.quantity {
                    self.live.insert(
                        child.client_id.clone(),
                        WorkingOrder {
                            object_id: child.object_id.clone(),
                            side: child.side,
                            quantity: child.quantity,
                            filled,
                            order_type: child.order_type,
                        },
                    );
                }
            }
        }

        Ok(self.ack(&child.client_id, at))
    }

    fn cancel(&mut self, client_id: &str, at: Timestamp) -> Result<GatewayAck> {
        if self.live.remove(client_id).is_none() {
            return Err(Error::not_found(format!(
                "{} has nothing working under {client_id}",
                self.venue.as_str()
            )));
        }
        self.events.push(GatewayEvent::Cancelled {
            client_id: client_id.to_string(),
            at: at.saturating_add(self.settings.latency),
            reason: "cancelled on request".to_string(),
        });
        Ok(self.ack(client_id, at))
    }

    fn replace(
        &mut self,
        client_id: &str,
        quantity: Decimal,
        price: Option<Decimal>,
        at: Timestamp,
    ) -> Result<GatewayAck> {
        let Some(order) = self.live.get_mut(client_id) else {
            return Err(Error::not_found(format!(
                "{} has nothing working under {client_id}",
                self.venue.as_str()
            )));
        };
        if quantity < order.filled {
            return Err(Error::invalid(format!(
                "{client_id} has {} filled and cannot be reduced to {quantity}",
                order.filled
            )));
        }
        order.quantity = quantity;
        if let Some(price) = price {
            order.order_type = RoutedOrderType::Limit { price };
        }
        self.events.push(GatewayEvent::Replaced {
            client_id: client_id.to_string(),
            at: at.saturating_add(self.settings.latency),
            quantity,
            price,
        });
        Ok(self.ack(client_id, at))
    }

    fn drain(&mut self) -> Vec<GatewayEvent> {
        std::mem::take(&mut self.events)
    }
}

/// How a real venue connection would be configured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeGatewayConfig {
    pub venue: VenueId,
    /// Where the session is established. Not a secret, and useful in a log.
    pub endpoint: String,
    /// The account orders would be sent for.
    pub account: String,
    /// The protocol the venue speaks, for the adapter that will implement it.
    pub protocol: String,
}

impl NativeGatewayConfig {
    pub fn new(
        venue: VenueId,
        endpoint: impl Into<String>,
        account: impl Into<String>,
        protocol: impl Into<String>,
    ) -> Self {
        Self {
            venue,
            endpoint: endpoint.into(),
            account: account.into(),
            protocol: protocol.into(),
        }
    }
}

/// An adapter to a real venue.
///
/// The interface is complete; the connection is not. This build has no
/// credential, no session and no egress path, so every instruction is refused
/// with the reason rather than attempted.
#[derive(Debug)]
pub struct NativeGateway {
    config: NativeGatewayConfig,
    credential_present: bool,
    transport_present: bool,
    enabled_by_operator: bool,
}

impl NativeGateway {
    pub fn new(config: NativeGatewayConfig) -> Self {
        Self {
            config,
            credential_present: false,
            transport_present: false,
            enabled_by_operator: false,
        }
    }

    /// Construct with the availability inputs set, for testing the logic that
    /// depends on them without inventing a transport.
    pub fn configured(
        config: NativeGatewayConfig,
        credential_present: bool,
        enabled_by_operator: bool,
    ) -> Self {
        Self {
            config,
            credential_present,
            // Never true in this build. There is no FIX or WebSocket session
            // here, and a flag that could be set to say otherwise would be a
            // way to claim there is.
            transport_present: false,
            enabled_by_operator,
        }
    }

    pub fn config(&self) -> &NativeGatewayConfig {
        &self.config
    }
}

impl Gateway for NativeGateway {
    fn venue(&self) -> &VenueId {
        &self.config.venue
    }

    fn is_simulated(&self) -> bool {
        false
    }

    fn is_available(&self) -> bool {
        self.credential_present && self.transport_present && self.enabled_by_operator
    }

    fn missing_credentials(&self) -> Vec<GatewayCredential> {
        let mut missing = production_requirements(&self.config.venue);
        if self.credential_present {
            missing.retain(|credential| !credential.name.contains("API key"));
        }
        if self.enabled_by_operator {
            missing.retain(|credential| !credential.name.contains("operator enablement"));
        }
        missing
    }

    fn send(&mut self, _child: &ChildOrder, _at: Timestamp) -> Result<GatewayAck> {
        Err(Error::unavailable(self.requirement()))
    }

    fn cancel(&mut self, _client_id: &str, _at: Timestamp) -> Result<GatewayAck> {
        Err(Error::unavailable(self.requirement()))
    }

    fn replace(
        &mut self,
        _client_id: &str,
        _quantity: Decimal,
        _price: Option<Decimal>,
        _at: Timestamp,
    ) -> Result<GatewayAck> {
        Err(Error::unavailable(self.requirement()))
    }

    fn drain(&mut self) -> Vec<GatewayEvent> {
        Vec::new()
    }

    fn requirement(&self) -> String {
        let missing = self.missing_credentials();
        let parts: Vec<String> = missing
            .iter()
            .map(|credential| format!("{} (from {})", credential.name, credential.env_var))
            .collect();
        format!(
            "{} over {} at {} for account {} is not usable: missing {}. Orders are routed to the simulated gateway, which is this build's only working venue.",
            self.config.venue.as_str(),
            self.config.protocol,
            self.config.endpoint,
            self.config.account,
            parts.join("; and ")
        )
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    //! The simulator's matching behaviour against a book, which needs a book
    //! to rest. These three lived in `tests/routing.rs` and moved here,
    //! unchanged in what they assert, when the public `with_book` that gave
    //! them one was removed (R12-COVERAGE-5107): a book written into the
    //! private map from inside the module is the only kind there now is, and
    //! no caller outside it can write one.

    use super::*;
    use qip_core::ids::OrderId;
    use qip_market::book::BookLevel;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn d(value: &str) -> Decimal {
        Decimal::parse(value).unwrap_or(Decimal::ZERO)
    }

    fn object() -> ObjectId {
        ObjectId::from_string("TKN")
    }

    fn venue(name: &str) -> VenueId {
        VenueId::new(name)
    }

    fn parent_id() -> OrderId {
        OrderId::from_string("ord-parent")
    }

    fn book(venue_name: &str, bids: &[(&str, &str)], asks: &[(&str, &str)]) -> OrderBook {
        OrderBook::from_levels(
            object(),
            venue_name,
            at(),
            bids.iter()
                .map(|(price, size)| BookLevel::new(d(price), d(size)))
                .collect(),
            asks.iter()
                .map(|(price, size)| BookLevel::new(d(price), d(size)))
                .collect(),
        )
    }

    /// `gateway`, holding `book`. Test-only, and inside the module, for the
    /// reason the type's documentation gives.
    fn holding(mut gateway: SimulatedGateway, book: OrderBook) -> SimulatedGateway {
        gateway.books.insert(book.object_id.clone(), book);
        gateway
    }

    #[test]
    fn the_simulated_gateway_marks_every_acknowledgement_as_simulated() -> Result<()> {
        let mut gateway = holding(
            SimulatedGateway::new(venue("SIM"), GatewaySettings::frictionless(), 7),
            book("SIM", &[("99.90", "1000")], &[("100.00", "1000")]),
        );
        let child = ChildOrder::new(
            "c1",
            parent_id(),
            venue("SIM"),
            object(),
            BookSide::Ask,
            d("10"),
            RoutedOrderType::Market,
        )?;
        let ack = gateway.send(&child, at())?;
        assert!(ack.simulated);
        assert_eq!(ack.venue, venue("SIM"));

        let events = gateway.drain();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, GatewayEvent::Accepted { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, GatewayEvent::Filled { .. }))
        );
        assert!(gateway.drain().is_empty(), "events are taken, not copied");
        Ok(())
    }

    #[test]
    fn a_fill_or_kill_the_book_cannot_do_in_full_is_killed() -> Result<()> {
        let mut gateway = holding(
            SimulatedGateway::new(venue("SIM"), GatewaySettings::frictionless(), 11),
            book("SIM", &[("99.90", "1000")], &[("100.00", "5")]),
        );
        let child = ChildOrder::new(
            "c1",
            parent_id(),
            venue("SIM"),
            object(),
            BookSide::Ask,
            d("50"),
            RoutedOrderType::FillOrKill { limit: d("100.00") },
        )?;
        gateway.send(&child, at())?;

        let events = gateway.drain();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, GatewayEvent::Filled { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, GatewayEvent::Cancelled { .. })),
            "all or none means none"
        );
        Ok(())
    }

    #[test]
    fn a_resting_order_stays_working_rather_than_being_assumed_filled() -> Result<()> {
        let mut gateway = holding(
            SimulatedGateway::new(venue("SIM"), GatewaySettings::frictionless(), 13),
            book("SIM", &[("99.90", "1000")], &[("100.00", "1000")]),
        );
        let child = ChildOrder::new(
            "c1",
            parent_id(),
            venue("SIM"),
            object(),
            BookSide::Ask,
            d("50"),
            RoutedOrderType::Limit { price: d("99.00") },
        )?;
        gateway.send(&child, at())?;

        assert_eq!(gateway.working_count(), 1);
        let working = gateway
            .working()
            .get("c1")
            .ok_or_else(|| Error::not_found("it is resting"))?;
        assert_eq!(working.remaining(), d("50"));

        gateway.replace("c1", d("30"), Some(d("99.50")), at())?;
        assert_eq!(
            gateway.working().get("c1").map(|order| order.quantity),
            Some(d("30"))
        );
        gateway.cancel("c1", at())?;
        assert_eq!(gateway.working_count(), 0);
        assert!(gateway.cancel("c1", at()).is_err(), "and only once");
        Ok(())
    }

    /// The other half of the removal: with no way in for a book, a child
    /// for any instrument is answered as one this venue does not trade,
    /// rather than filled against depth somebody supplied without a
    /// permission. Asserts the premise that the send itself was accepted
    /// onto the wire, so the rejection is the venue's answer.
    #[test]
    fn with_no_admitted_listing_the_simulated_gateway_trades_nothing() -> Result<()> {
        let mut gateway = SimulatedGateway::new(venue("SIM"), GatewaySettings::frictionless(), 17);
        let child = ChildOrder::new(
            "c1",
            parent_id(),
            venue("SIM"),
            object(),
            BookSide::Ask,
            d("10"),
            RoutedOrderType::Market,
        )?;
        let ack = gateway.send(&child, at())?;
        assert!(ack.simulated, "premise: the instruction left for the venue");
        let events = gateway.drain();
        assert!(
            events.iter().any(|e| matches!(
                e,
                GatewayEvent::Rejected { reason, .. } if reason == "SIM does not trade TKN"
            )),
            "an instrument nobody listed was not refused: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, GatewayEvent::Filled { .. })),
            "an instrument nobody listed filled: {events:?}"
        );
        Ok(())
    }
}
