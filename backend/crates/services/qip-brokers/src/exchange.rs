//! A simulated exchange: a venue that actually matches.
//!
//! Everything a venue does to an order happens here for a reason that shows up
//! in a fill: orders queue behind each other at a price, a large order takes
//! several levels and leaves a residual, an amendment that adds size loses its
//! place, a market order that outruns the book has its remainder cancelled
//! rather than quietly filled, and the answer arrives after a latency rather
//! than instantly.
//!
//! Two things are deliberately unpleasant. The venue refuses orders — an
//! unlisted instrument, an off-lot quantity, an off-tick price, an algorithmic
//! order type it does not implement, and a small seeded probability of a
//! refusal for no reason at all — because an execution path that has never met
//! a rejection meets its first one in production. And the heartbeat can be
//! stopped, because a venue that goes quiet without saying so is the failure
//! that a connection state machine exists for.
//!
//! The venue keeps the clearing account's books through
//! [`crate::ledger::AccountLedger`], so `query_cash`, `query_positions` and
//! `query_margin` answer from the same exact accounting the rest of the
//! platform uses rather than from a counter maintained on the side.
//!
//! Nothing reads a clock or an ambient random source. `at` is a parameter and
//! the refusals and latency jitter are drawn from a seeded
//! [`Xoshiro256`], so the same instructions at the same timestamps with the
//! same seed produce byte-identical fills.

use crate::adapter::{
    AdapterClass, CashBalance, Heartbeat, MarginState, MarketData, OrderAck, PositionSnapshot,
    VenueAdapter, VenueOrder, VenueOrderState, stamp_simulated,
};
use crate::connection::{ConnectionState, ReadyTicket};
use crate::credential::{
    RequirementKind, VenueCredential, VenueRequirement, requirements_of_kind, standard_requirements,
};
use crate::ledger::{AccountLedger, MarginPolicy};
use crate::matching::{MatchingEngine, Participant};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::ids::{FillId, ObjectId, OrderId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal};
use qip_execution_engine::broker::{Broker, VenueCapabilities};
use qip_execution_engine::order::{Fill, Order, OrderType, Side};
use qip_financial::asset_class::InstrumentType;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
pub use qip_market::book::BookLevel;
use qip_market::book::OrderBook;
use qip_market::quote::Quote;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One listed instrument's depth, as the simulated venue publishes it.
///
/// The venue's own type, so a consumer that wants the simulator's quotes has
/// to hold the simulator's answer: there is no way to build one of these from
/// a price somebody else made up.
#[derive(Clone, Debug, PartialEq)]
pub struct SimulatedDepth {
    pub object_id: ObjectId,
    /// Best first, aggregated by price.
    pub bids: Vec<BookLevel>,
    /// Best first, aggregated by price.
    pub asks: Vec<BookLevel>,
}

/// A fill with everything an account needs in order to book it.
///
/// A [`Fill`] names an order, not an instrument or a direction, which is enough
/// for the order it belongs to and not enough for a ledger. Carrying the two
/// missing facts alongside beats looking them up and guessing wrong.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookableFill {
    pub object_id: ObjectId,
    pub side: Side,
    pub fill: Fill,
}

/// How the simulated venue behaves.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExchangeSettings {
    /// Commission as a fraction of notional. Exact, because it moves cash.
    pub commission_rate: Decimal,
    /// Base round trip to an acknowledgement.
    pub latency: Duration,
    /// Additional uniform jitter on the round trip, drawn from the seed.
    pub latency_jitter: Duration,
    /// Probability the venue refuses an order for no stated reason.
    ///
    /// Small and non-zero on purpose.
    pub rejection_probability: f64,
    /// How long the session may go without a heartbeat before it is degraded.
    pub heartbeat_interval: Duration,
    /// Cash in the clearing account before anything trades.
    pub opening_cash: Decimal,
    pub currency: Currency,
    pub margin: MarginPolicy,
}

impl Default for ExchangeSettings {
    fn default() -> Self {
        Self {
            // 0.0002 = two basis points, at the nine-digit decimal scale.
            commission_rate: Decimal::from_raw(200_000),
            latency: Duration::from_millis(4),
            latency_jitter: Duration::from_millis(3),
            rejection_probability: 0.005,
            heartbeat_interval: Duration::from_secs(30),
            opening_cash: Decimal::from_int(10_000_000),
            currency: Currency::USD,
            margin: MarginPolicy::default(),
        }
    }
}

impl ExchangeSettings {
    /// No jitter and no unexplained refusals.
    ///
    /// Not frictionless: commission is still charged, the book still runs out,
    /// and an order still queues. This turns off only the venue's coin-flips,
    /// so a test can assert on behaviour that must hold every time rather than
    /// on behaviour that usually holds.
    pub fn orderly() -> Self {
        Self {
            latency_jitter: Duration::ZERO,
            rejection_probability: 0.0,
            ..Self::default()
        }
    }

    /// Refuses everything, for exercising the rejection path.
    pub fn always_refuses() -> Self {
        Self {
            rejection_probability: 1.0,
            ..Self::orderly()
        }
    }
}

/// The venue's own record of a client order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ExchangeOrder {
    order_id: OrderId,
    object_id: ObjectId,
    side: Side,
    /// The size the order was first sent with, kept so an amendment is visible.
    original_quantity: Decimal,
    quantity: Decimal,
    filled: Decimal,
    limit: Option<Decimal>,
    state: VenueOrderState,
    revision: u32,
    submitted_at: Timestamp,
    updated_at: Timestamp,
    priority: u64,
}

impl ExchangeOrder {
    fn remaining(&self) -> Decimal {
        self.quantity - self.filled
    }
}

/// A venue that matches orders against a real book.
#[derive(Debug)]
pub struct SimulatedExchange {
    venue: VenueId,
    settings: ExchangeSettings,
    connection: ConnectionState,
    account: Option<String>,
    rng: Xoshiro256,
    engine: MatchingEngine,
    listings: BTreeMap<String, FinancialObject>,
    orders: BTreeMap<String, ExchangeOrder>,
    ledger: AccountLedger,
    /// Client fills not yet handed to whoever is keeping the client's books.
    undrained: Vec<BookableFill>,
    last_trade: BTreeMap<String, Decimal>,
    /// Whether the venue still answers heartbeats. Turning this off is how a
    /// venue goes quiet without saying so, which is the failure the connection
    /// state machine exists for.
    heartbeat_source: bool,
    fill_sequence: u64,
    heartbeats: u64,
    submitted: u64,
    rejected: u64,
}

impl SimulatedExchange {
    /// A venue with no listings, disconnected, holding the clearing account's
    /// opening cash.
    pub fn new(venue: VenueId, settings: ExchangeSettings, seed: u64, at: Timestamp) -> Self {
        let label = format!("simulated-exchange-{}", venue.as_str());
        let ledger = AccountLedger::new(
            format!("{}-clearing", venue.as_str()),
            settings.currency,
            settings.opening_cash,
            settings.margin,
            at,
        );
        Self {
            connection: ConnectionState::new(venue.clone(), settings.heartbeat_interval, at),
            venue,
            settings,
            account: None,
            rng: Xoshiro256::seeded(seed).fork(&label),
            engine: MatchingEngine::new(),
            listings: BTreeMap::new(),
            orders: BTreeMap::new(),
            ledger,
            undrained: Vec::new(),
            last_trade: BTreeMap::new(),
            heartbeat_source: true,
            fill_sequence: 0,
            heartbeats: 0,
            submitted: 0,
            rejected: 0,
        }
    }

    pub fn settings(&self) -> ExchangeSettings {
        self.settings
    }

    /// List an instrument. The venue will not trade anything else.
    pub fn list(&mut self, object: FinancialObject) {
        self.ledger.register(object.clone());
        self.listings
            .insert(object.object_id.as_str().to_string(), object);
    }

    /// List an instrument from its trading facts alone.
    ///
    /// The full `FinancialObject` vocabulary belongs to the library layer, and
    /// the deployables that hold this venue sit behind ports too narrow to
    /// carry it — the edge cell's `Placer` names a venue, a side, a quantity
    /// and a price and nothing else. So the venue builds the listing itself:
    /// it already speaks the vocabulary, and a listing is the venue's own
    /// record in any case. The provenance is stamped synthetic, because a
    /// listing invented by a simulator must never read as market data.
    pub fn list_synthetic(
        &mut self,
        object_id: ObjectId,
        symbol: &str,
        price: Decimal,
        lot_size: Decimal,
        tick_size: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        if price <= Decimal::ZERO || lot_size <= Decimal::ZERO || tick_size <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "a listing for {symbol} at {} needs a positive price, lot size and tick size; \
                 got price {price}, lot {lot_size}, tick {tick_size}",
                self.venue.as_str()
            )));
        }
        let listing = FinancialObject::builder(object_id, symbol, InstrumentType::CommonStock)
            .venue(self.venue.as_str())
            .price(price)
            .lot_size(lot_size)
            .tick_size(tick_size)
            .provenance(Provenance::synthetic(
                format!("listed by the simulated venue {}", self.venue.as_str()),
                at,
            ))
            .build(at)?;
        self.list(listing);
        Ok(())
    }

    pub fn is_listed(&self, object_id: &ObjectId) -> bool {
        self.listings.contains_key(object_id.as_str())
    }

    pub fn listing(&self, object_id: &ObjectId) -> Option<&FinancialObject> {
        self.listings.get(object_id.as_str())
    }

    /// Add resting interest belonging to other participants.
    pub fn seed_liquidity(
        &mut self,
        object_id: &ObjectId,
        side: Side,
        price: Decimal,
        quantity: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        if !self.is_listed(object_id) {
            return Err(Error::not_found(format!(
                "{} does not list {}, so it has no book to seed",
                self.venue.as_str(),
                object_id.as_str()
            )));
        }
        self.engine.seed(object_id, side, price, quantity, at)
    }

    /// Stop answering heartbeats, without telling anyone.
    pub fn stop_heartbeat(&mut self) {
        self.heartbeat_source = false;
    }

    /// Start answering again.
    pub fn start_heartbeat(&mut self) {
        self.heartbeat_source = true;
    }

    /// Take the client fills produced since the last call.
    ///
    /// Includes fills on orders that were resting and got hit, which is the
    /// half of the flow a broker that only looked at its own acknowledgements
    /// would miss.
    pub fn drain_bookable_fills(&mut self) -> Vec<BookableFill> {
        std::mem::take(&mut self.undrained)
    }

    pub fn submitted_count(&self) -> u64 {
        self.submitted
    }

    pub fn rejected_count(&self) -> u64 {
        self.rejected
    }

    pub fn resting_count(&self) -> usize {
        self.engine.resting_count()
    }

    /// Every listed instrument's resting depth, as the venue itself holds it.
    ///
    /// This is the simulated venue's quote feed: what a cell placing against
    /// this book would see if the venue published its depth. It is read from
    /// the matching engine rather than assembled from anything the caller
    /// said, so a book the cell prices off is the book the venue will match
    /// against — the one property a paper feed has to hold for a fill to
    /// mean anything. Listings are walked in id order so two reads of the
    /// same venue publish in the same order.
    ///
    /// An instrument that is listed and has nothing resting is published with
    /// two empty sides rather than omitted: the cell keeps a book for it and
    /// a book the feed stopped mentioning would keep serving the last level
    /// it ever saw.
    pub fn quotes(&self) -> Vec<SimulatedDepth> {
        self.listings
            .values()
            .map(|listing| SimulatedDepth {
                object_id: listing.object_id.clone(),
                bids: self.engine.depth(&listing.object_id, Side::Buy),
                asks: self.engine.depth(&listing.object_id, Side::Sell),
            })
            .collect()
    }

    pub fn ledger(&self) -> &AccountLedger {
        &self.ledger
    }

    /// The requirements this adapter enforces before it will log anyone on.
    ///
    /// A simulator cannot check an endpoint it never dials or an entitlement
    /// nobody granted, and pretending otherwise would be theatre. It can refuse
    /// to run for an account nobody named, using a secret nobody located — the
    /// two mistakes that carry straight over to production.
    fn enforced_requirements(&self) -> Vec<VenueRequirement> {
        requirements_of_kind(
            &standard_requirements(&self.venue),
            &[RequirementKind::Account, RequirementKind::SessionCredential],
        )
    }

    /// The round trip for this instruction, jitter included.
    fn round_trip(&mut self) -> Duration {
        let jitter = self.settings.latency_jitter.as_nanos();
        if jitter <= 0 {
            return self.settings.latency;
        }
        let drawn = self.rng.below(jitter.unsigned_abs()) as i64;
        self.settings.latency + Duration::from_nanos(drawn)
    }

    /// The limit an order type implies, and whether the venue accepts it.
    fn limit_of(&self, order: &Order) -> Result<Option<Decimal>> {
        match order.order_type {
            OrderType::Market => Ok(None),
            OrderType::Limit { price } => Ok(Some(price)),
            other => Err(Error::denied(format!(
                "{} accepts market and limit orders; {} is an execution algorithm and has to be \
                 worked into child orders before it reaches a venue",
                self.venue.as_str(),
                other.as_str()
            ))),
        }
    }

    /// Everything the venue checks before an order reaches the book.
    fn admit(&mut self, order: &Order, limit: Option<Decimal>) -> Result<()> {
        if self.orders.contains_key(order.order_id.as_str()) {
            return Err(Error::invalid(format!(
                "{} is already working at {}; a client order id is used once",
                order.order_id.as_str(),
                self.venue.as_str()
            )));
        }
        let Some(listing) = self.listings.get(order.object_id.as_str()) else {
            return Err(Error::denied(format!(
                "{} does not trade {}",
                self.venue.as_str(),
                order.object_id.as_str()
            )));
        };
        if order.quantity.floor_to_step(listing.lot_size) != order.quantity {
            return Err(Error::denied(format!(
                "{} of {} is not a multiple of the {} lot size at {}",
                order.quantity,
                listing.symbol,
                listing.lot_size,
                self.venue.as_str()
            )));
        }
        if let Some(price) = limit
            && price.floor_to_step(listing.tick_size) != price
        {
            return Err(Error::denied(format!(
                "a limit of {price} for {} is off the {} tick grid at {}",
                listing.symbol,
                listing.tick_size,
                self.venue.as_str()
            )));
        }
        if self.rng.bernoulli(self.settings.rejection_probability) {
            return Err(Error::denied(format!(
                "{} refused order {} without explanation, which is a thing venues do",
                self.venue.as_str(),
                order.order_id.as_str()
            )));
        }
        Ok(())
    }

    /// Build a fill, charge the commission, and note the trade price.
    fn make_fill(
        &mut self,
        order_id: &OrderId,
        object_id: &ObjectId,
        side: Side,
        price: Decimal,
        quantity: Decimal,
        at: Timestamp,
    ) -> BookableFill {
        self.fill_sequence = self.fill_sequence.saturating_add(1);
        let costs = quantity
            .checked_mul(price)
            .and_then(|notional| notional.checked_mul(self.settings.commission_rate))
            .unwrap_or(Decimal::ZERO);
        self.last_trade
            .insert(object_id.as_str().to_string(), price);
        BookableFill {
            object_id: object_id.clone(),
            side,
            fill: Fill {
                fill_id: FillId::from_string(format!(
                    "{}-fill-{}",
                    self.venue.as_str(),
                    self.fill_sequence
                )),
                order_id: order_id.clone(),
                at,
                quantity,
                price,
                costs,
                venue: self.venue.as_str().to_string(),
                // Overwritten again by `stamp_simulated` on the way out. The
                // flag is the adapter's to set, never the message's.
                simulated: true,
            },
        }
    }

    /// Book a client fill to the clearing account and queue it for the client.
    fn record(&mut self, bookable: BookableFill) -> Result<()> {
        self.ledger
            .apply(&bookable.object_id, &bookable.fill, bookable.side)?;
        self.undrained.push(bookable);
        Ok(())
    }

    /// Advance a resting client order that was hit by somebody else's taker.
    fn credit_maker(&mut self, maker: &OrderId, quantity: Decimal, at: Timestamp) {
        if let Some(record) = self.orders.get_mut(maker.as_str()) {
            record.filled += quantity;
            record.updated_at = at;
            record.state = if record.remaining() <= Decimal::ZERO {
                VenueOrderState::Filled
            } else {
                VenueOrderState::PartiallyFilled {
                    filled: record.filled,
                }
            };
        }
    }

    fn venue_order(&self, record: &ExchangeOrder) -> VenueOrder {
        VenueOrder {
            order_id: record.order_id.clone(),
            object_id: record.object_id.clone(),
            venue: self.venue.as_str().to_string(),
            side: record.side,
            quantity: record.quantity,
            filled: record.filled,
            limit: record.limit,
            state: record.state.clone(),
            revision: record.revision,
            original_quantity: record.original_quantity,
            submitted_at: record.submitted_at,
            updated_at: record.updated_at,
            priority: record.priority,
            simulated: true,
        }
    }
}

impl VenueAdapter for SimulatedExchange {
    fn venue_id(&self) -> &VenueId {
        &self.venue
    }

    fn class(&self) -> AdapterClass {
        AdapterClass::Simulated
    }

    fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    fn connection(&self) -> &ConnectionState {
        &self.connection
    }

    fn requirements(&self) -> Vec<VenueRequirement> {
        standard_requirements(&self.venue)
    }

    fn missing_requirements(&self) -> Vec<VenueRequirement> {
        // The simulator is complete for what it is and incomplete as a venue.
        // Reporting the second is the honest answer to "what would it take to
        // run this for real", and a simulator that presented itself as complete
        // would be the more dangerous of the two.
        standard_requirements(&self.venue)
    }

    fn connect(&mut self, at: Timestamp) -> Result<()> {
        self.connection.connect(at)
    }

    fn authenticate(&mut self, credential: &VenueCredential, at: Timestamp) -> Result<()> {
        credential.require(&self.venue, &self.enforced_requirements())?;
        self.connection.authenticated(at, credential.account())?;
        self.account = Some(credential.account().to_string());
        Ok(())
    }

    fn heartbeat(&mut self, at: Timestamp) -> Result<Heartbeat> {
        self.connection.observe(at);
        self.connection.require_session(at)?;
        if !self.heartbeat_source {
            let gap = self.connection.heartbeat_gap(at);
            return Err(Error::timeout(format!(
                "{} has not answered a heartbeat{}",
                self.venue.as_str(),
                match gap {
                    Some(gap) => format!(" for {gap:?}"),
                    None => " at all".to_string(),
                }
            )));
        }
        let round_trip = self.round_trip();
        let phase = self.connection.observe_heartbeat(at)?;
        self.heartbeats = self.heartbeats.saturating_add(1);
        Ok(Heartbeat {
            venue: self.venue.as_str().to_string(),
            at: at.saturating_add(round_trip),
            round_trip,
            phase,
            session: self.connection.session(),
            sequence: self.heartbeats,
        })
    }

    fn market_data(&self, object_id: &ObjectId, at: Timestamp) -> Result<MarketData> {
        if !self.is_listed(object_id) {
            return Err(Error::not_found(format!(
                "{} does not trade {}",
                self.venue.as_str(),
                object_id.as_str()
            )));
        }
        self.connection.require_session(at)?;
        let bids = self.engine.depth(object_id, Side::Buy);
        let asks = self.engine.depth(object_id, Side::Sell);
        let quote = match (bids.first(), asks.first()) {
            (Some(bid), Some(ask)) => Some(Quote {
                object_id: object_id.clone(),
                venue: self.venue.as_str().to_string(),
                at,
                bid: bid.price,
                ask: ask.price,
                bid_size: bid.size,
                ask_size: ask.size,
                quality: qip_financial::quality::DataQuality::default(),
            }),
            _ => None,
        };
        Ok(MarketData {
            book: OrderBook::from_levels(
                object_id.clone(),
                self.venue.as_str().to_string(),
                at,
                bids,
                asks,
            ),
            object_id: object_id.clone(),
            venue: self.venue.as_str().to_string(),
            at,
            quote,
            last_trade: self.last_trade.get(object_id.as_str()).copied(),
            simulated: true,
        })
    }

    fn submit_order(
        &mut self,
        ticket: &ReadyTicket,
        order: &Order,
        at: Timestamp,
    ) -> Result<OrderAck> {
        self.connection.observe(at);
        self.connection.authorise(ticket, at)?;
        order.validate()?;
        self.submitted = self.submitted.saturating_add(1);

        let limit = self.limit_of(order).inspect_err(|_| {
            self.rejected = self.rejected.saturating_add(1);
        })?;
        if let Err(error) = self.admit(order, limit) {
            self.rejected = self.rejected.saturating_add(1);
            return Err(error);
        }

        let round_trip = self.round_trip();
        let landed = at.saturating_add(round_trip);
        let outcome = self.engine.execute(
            &order.object_id,
            &order.order_id,
            order.side,
            order.quantity,
            limit,
            landed,
            Participant::Client,
        );

        let mut fills = Vec::new();
        // Collected first because booking a fill borrows the venue's books, and
        // a trade can touch two client orders at once.
        let trades = outcome.trades.clone();
        for trade in &trades {
            let bookable = self.make_fill(
                &order.order_id,
                &order.object_id,
                order.side,
                trade.price,
                trade.quantity,
                landed,
            );
            fills.push(bookable.fill.clone());
            self.record(bookable)?;
            if trade.maker_owner.is_client() {
                let maker_side = order.side.opposite();
                let maker_fill = self.make_fill(
                    &trade.maker,
                    &order.object_id,
                    maker_side,
                    trade.price,
                    trade.quantity,
                    landed,
                );
                self.record(maker_fill)?;
                self.credit_maker(&trade.maker, trade.quantity, landed);
            }
        }

        let remaining = order.quantity - outcome.filled;
        let state = if remaining <= Decimal::ZERO {
            VenueOrderState::Filled
        } else if outcome.resting {
            if outcome.filled > Decimal::ZERO {
                VenueOrderState::PartiallyFilled {
                    filled: outcome.filled,
                }
            } else {
                VenueOrderState::Working
            }
        } else {
            VenueOrderState::Cancelled {
                reason: format!(
                    "{remaining} of {} could not be taken immediately, and an unpriced order does \
                     not rest",
                    order.quantity
                ),
            }
        };

        let priority = self
            .engine
            .resting(&order.object_id, &order.order_id)
            .map_or(0, |resting| resting.sequence);
        self.orders.insert(
            order.order_id.as_str().to_string(),
            ExchangeOrder {
                order_id: order.order_id.clone(),
                object_id: order.object_id.clone(),
                side: order.side,
                original_quantity: order.quantity,
                quantity: order.quantity,
                filled: outcome.filled,
                limit,
                state: state.clone(),
                revision: 0,
                submitted_at: landed,
                updated_at: landed,
                priority,
            },
        );

        stamp_simulated(&mut fills, self.is_simulated());
        Ok(OrderAck {
            order_id: order.order_id.clone(),
            venue: self.venue.as_str().to_string(),
            at: landed,
            latency: round_trip,
            detail: format!(
                "{} filled of {}, {} remaining",
                outcome.filled, order.quantity, remaining
            ),
            state,
            fills,
            remaining,
            simulated: self.is_simulated(),
        })
    }

    fn cancel_order(&mut self, order_id: &OrderId, at: Timestamp) -> Result<OrderAck> {
        self.connection.observe(at);
        // No readiness ticket. Cancelling reduces risk, and a degraded session
        // is exactly when it is most needed.
        self.connection.require_session(at)?;
        let Some(record) = self.orders.get(order_id.as_str()) else {
            return Err(Error::not_found(format!(
                "{} has no order {}",
                self.venue.as_str(),
                order_id.as_str()
            )));
        };
        if record.state.is_terminal() {
            return Err(Error::invalid(format!(
                "order {} is {} at {} and cannot be cancelled",
                order_id.as_str(),
                record.state.as_str(),
                self.venue.as_str()
            )));
        }
        let object_id = record.object_id.clone();
        let round_trip = self.round_trip();
        let landed = at.saturating_add(round_trip);
        self.engine.cancel(&object_id, order_id)?;
        let Some(record) = self.orders.get_mut(order_id.as_str()) else {
            return Err(Error::not_found(format!(
                "{} lost order {} between reading it and cancelling it",
                self.venue.as_str(),
                order_id.as_str()
            )));
        };
        record.state = VenueOrderState::Cancelled {
            reason: "cancelled on request".to_string(),
        };
        record.updated_at = landed;
        let remaining = record.remaining();
        Ok(OrderAck {
            order_id: order_id.clone(),
            venue: self.venue.as_str().to_string(),
            at: landed,
            latency: round_trip,
            state: record.state.clone(),
            fills: Vec::new(),
            remaining,
            simulated: true,
            detail: format!("{remaining} withdrawn"),
        })
    }

    fn replace_order(
        &mut self,
        ticket: &ReadyTicket,
        order_id: &OrderId,
        quantity: Decimal,
        limit: Option<Decimal>,
        at: Timestamp,
    ) -> Result<OrderAck> {
        self.connection.observe(at);
        // An amendment can add quantity, which is new risk, so it needs the
        // same proof of readiness a fresh order does.
        self.connection.authorise(ticket, at)?;
        let Some(record) = self.orders.get(order_id.as_str()) else {
            return Err(Error::not_found(format!(
                "{} has no order {}",
                self.venue.as_str(),
                order_id.as_str()
            )));
        };
        if record.state.is_terminal() {
            return Err(Error::invalid(format!(
                "order {} is {} at {} and cannot be replaced",
                order_id.as_str(),
                record.state.as_str(),
                self.venue.as_str()
            )));
        }
        if quantity < record.filled {
            return Err(Error::invalid(format!(
                "order {} has {} filled and cannot be reduced to {quantity}",
                order_id.as_str(),
                record.filled
            )));
        }
        let object_id = record.object_id.clone();
        let working = quantity - record.filled;
        let round_trip = self.round_trip();
        let landed = at.saturating_add(round_trip);
        let amended = self
            .engine
            .replace(&object_id, order_id, working, limit, landed)?;
        let Some(record) = self.orders.get_mut(order_id.as_str()) else {
            return Err(Error::not_found(format!(
                "{} lost order {} between reading it and amending it",
                self.venue.as_str(),
                order_id.as_str()
            )));
        };
        // The identity is the client's and never changes. The revision counts
        // the amendments, and the priority shows what they cost.
        record.quantity = quantity;
        record.limit = Some(amended.price);
        record.revision = record.revision.saturating_add(1);
        record.updated_at = landed;
        record.priority = amended.sequence;
        record.state = if record.filled > Decimal::ZERO {
            VenueOrderState::PartiallyFilled {
                filled: record.filled,
            }
        } else {
            VenueOrderState::Working
        };
        Ok(OrderAck {
            order_id: order_id.clone(),
            venue: self.venue.as_str().to_string(),
            at: landed,
            latency: round_trip,
            state: record.state.clone(),
            fills: Vec::new(),
            remaining: record.remaining(),
            simulated: true,
            detail: format!(
                "amended to {quantity} at {}, revision {}",
                amended.price, record.revision
            ),
        })
    }

    fn query_order(&self, order_id: &OrderId) -> Result<VenueOrder> {
        match self.orders.get(order_id.as_str()) {
            Some(record) => Ok(self.venue_order(record)),
            None => Err(Error::not_found(format!(
                "{} has no order {}",
                self.venue.as_str(),
                order_id.as_str()
            ))),
        }
    }

    fn query_positions(&self) -> Result<Vec<PositionSnapshot>> {
        Ok(self.ledger.positions())
    }

    fn query_cash(&self) -> Result<CashBalance> {
        Ok(self.ledger.cash())
    }

    fn query_margin(&self, at: Timestamp) -> Result<MarginState> {
        Ok(self.ledger.margin(at))
    }

    fn query_fills(&self, since: Option<Timestamp>) -> Result<Vec<Fill>> {
        let mut fills = self.ledger.fills(since);
        stamp_simulated(&mut fills, self.is_simulated());
        Ok(fills)
    }

    fn disconnect(&mut self, reason: &str, at: Timestamp) -> Result<()> {
        self.connection.disconnect(at, reason);
        self.account = None;
        Ok(())
    }
}

impl Broker for SimulatedExchange {
    fn name(&self) -> &str {
        self.venue.as_str()
    }

    fn is_simulated(&self) -> bool {
        true
    }

    fn is_available(&self) -> bool {
        true
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            name: self.venue.as_str().to_string(),
            supported_types: vec!["market".to_string(), "limit".to_string()],
            partial_fills: true,
            // The venue-level figure is the finest lot any listing trades in;
            // each instrument carries its own and that is what is enforced.
            lot_size: self
                .listings
                .values()
                .map(|listing| listing.lot_size)
                .min()
                .unwrap_or(Decimal::ONE),
            // A rate for reporting, so `f64` is the right type here. Every
            // charge against cash uses the exact `Decimal` rate.
            commission_rate: self.settings.commission_rate.to_f64(),
        }
    }

    /// Submit through the narrow port the order management system uses.
    ///
    /// Mints its own readiness ticket, so an OMS that knows nothing about this
    /// crate still cannot reach a venue that is not ready.
    fn submit(&mut self, order: &Order, at: Timestamp) -> Result<Vec<Fill>> {
        let ticket = self.ready(at)?;
        self.submit_order(&ticket, order, at).map(|ack| ack.fills)
    }

    fn cancel(&mut self, order: &Order, at: Timestamp) -> Result<()> {
        self.cancel_order(&order.order_id, at).map(|_| ())
    }

    fn requirement(&self) -> String {
        self.requirement_summary()
    }
}
