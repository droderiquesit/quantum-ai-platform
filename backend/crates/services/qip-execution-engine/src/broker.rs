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

use crate::creation::{CreationAction, CreationAdmission};
use crate::observation::{DeclaredVenueProfile, ObservedVenueFacts, VenueObservation};
use crate::order::{Fill, Order, OrderType, Side};
use qip_core::Decimal;
use qip_core::decimal::SCALE_DIGITS;
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

    /// What this venue declares about itself and what it has been seen doing.
    ///
    /// The seam blueprint §34.4's observed rung waits on. Before it existed
    /// the port carried [`Self::capabilities`] — a venue's own description of
    /// itself — and nothing at all about how long the venue took to answer or
    /// which of those capabilities it honoured when one was exercised, so the
    /// promotion ladder had a declaration to check and nothing to check it
    /// against.
    ///
    /// **The default is [`None`], and that is the load-bearing part of this
    /// signature.** An adapter that keeps no tally reports that it keeps
    /// none. A default returning a zeroed [`VenueObservation`] would present
    /// every adapter that never overrode it as a venue answering instantly,
    /// charging nothing and accepting everything — three claims nobody made,
    /// each of which a gate would read as evidence in the venue's favour. An
    /// unmeasured venue is not a fast venue.
    fn observation(&self) -> Option<VenueObservation> {
        None
    }

    /// The venue's listing surface, where a market can be listed, seeded and
    /// withdrawn (the Market Factory, R12-EXEC-5080).
    ///
    /// **[`None`] by default, and [`SimulatedBroker`] is the only
    /// implementor that answers.** A venue that cannot have a market brought
    /// into existence on it — every real venue, under ADR 0003 and ADR
    /// 0021 — says so by not overriding this, and the factory, which reaches
    /// a venue only through this method, then has nothing to list on. The
    /// factory also refuses a surface whose [`ListingVenue::is_simulated`]
    /// is false, so an adapter that overrode this without being a simulator
    /// would still be refused, by name, rather than listed on.
    fn listing_venue(&mut self) -> Option<&mut dyn ListingVenue> {
        None
    }
}

/// Where the Market Factory lists, seeds and withdraws a market.
///
/// A port rather than the simulator's concrete type, so the kernel that
/// composes the factory names no venue (EXEC-010) and the factory's refusal
/// of a non-simulated venue is a check on this surface rather than a type
/// nobody else could ever satisfy by accident.
pub trait ListingVenue: fmt::Debug {
    /// The venue's name, as [`Broker::name`] reports it.
    fn venue_name(&self) -> &str;
    /// Whether the venue is simulated. The factory refuses one that is not.
    fn is_simulated(&self) -> bool;
    /// The book resting for an instrument, if one is listed.
    fn listed_book(&self, object_id: &str) -> Option<&OrderBook>;
    /// Rest `book` for its instrument, listing it or replacing what rests,
    /// under an admission to list or seed exactly that instrument at exactly
    /// this venue.
    ///
    /// The admission is on the primitive and not only on the factory above
    /// it (R12-COVERAGE-5107). Until 2026-10-24 this took a bare book, and
    /// [`Broker::listing_venue`] hands this surface to any holder of a
    /// broker, so a new caller could have rested a book for any instrument
    /// with no permission recorded anywhere and no test would have noticed.
    /// An implementor refuses, before anything is written, an admission
    /// whose action is not [`CreationAction::List`] or
    /// [`CreationAction::Seed`], whose venue is not [`Self::venue_name`], or
    /// whose product is not the book's instrument.
    fn list_book(&mut self, book: OrderBook, admission: &CreationAdmission) -> Result<()>;
    /// Withdraw an instrument's book, returning what rested.
    fn withdraw_book(&mut self, object_id: &str) -> Option<OrderBook>;
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
    /// Current books by instrument, written only by
    /// [`ListingVenue::list_book`] under a creation admission.
    ///
    /// There used to be a public `set_book` beside it taking a bare book, so
    /// the admission the factory demanded could be stepped around by any
    /// holder of the broker. It is gone: a book rests here because the
    /// register admitted listing or seeding that instrument at this venue,
    /// or it does not rest here at all.
    books: std::collections::BTreeMap<String, OrderBook>,
    /// Recent daily volume by instrument, for the impact model.
    daily_volumes: std::collections::BTreeMap<String, f64>,
    /// Daily volatility by instrument, for the impact model.
    daily_volatility: std::collections::BTreeMap<String, f64>,
    sequence: u64,
    submitted: usize,
    rejected: usize,
    /// Order types this venue has answered by filling or by resting.
    ///
    /// The measured half of §34.4's order-type check. Bounded by
    /// [`OrderType::as_str`], which is a closed set of five literals, so this
    /// set cannot grow with traffic.
    accepted_order_types: std::collections::BTreeSet<String>,
    /// Commission actually charged, and the notional it was charged on.
    ///
    /// Two figures rather than a rate, because the rate is a quotient and the
    /// denominator is the half that can be zero. A venue that has filled
    /// nothing reports no rate rather than a free one.
    /// `None` once an accumulation has overflowed: a venue that can no
    /// longer itemise says so rather than reporting a figure it knows is
    /// short. Overflow does not refuse the fill, because a diagnostic tally
    /// must never decide whether an order trades.
    fees_charged: Option<Decimal>,
    notional_filled: Option<Decimal>,
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
            accepted_order_types: std::collections::BTreeSet::new(),
            fees_charged: Some(Decimal::ZERO),
            notional_filled: Some(Decimal::ZERO),
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

    /// The fewest fractional digits a fill price may keep, and so the most a
    /// fill quantity may carry (`SCALE_DIGITS` less this).
    const MIN_PRICE_DIGITS: u32 = 4;

    /// Round a fill price so that `quantity x price` is exactly representable.
    ///
    /// The ledger the fill is booked into holds money at nine fractional
    /// digits and refuses a notional that needs more, rather than rounding it.
    /// This venue produced prices at nine digits for a quantity that already
    /// carried one (60% of a whole-share order), so a notional of ten digits
    /// came out of the default settings and the fill, which the venue had made,
    /// could not be booked. A venue that emits what the books cannot hold is
    /// the venue's defect, so the price is made to fit here: it keeps the
    /// digits the notional can carry after the quantity's own.
    ///
    /// Rounded against the taker: up for a buy, down for a sell, so the
    /// simulator never flatters the strategy by rounding. A quantity with so
    /// many digits that the price would keep fewer than four is refused, not
    /// clamped; a price rounded to the nearest dollar is not a price.
    fn ledger_exact_price(order: &Order, quantity: Decimal, price: Decimal) -> Result<Decimal> {
        let digits = (0..=SCALE_DIGITS)
            .find(|d| quantity.raw() % 10i128.pow(SCALE_DIGITS - d) == 0)
            .unwrap_or(SCALE_DIGITS);
        let keep = SCALE_DIGITS - digits;
        if keep < Self::MIN_PRICE_DIGITS {
            return Err(Error::numeric(format!(
                "a fill of {quantity} on order {} carries {digits} fractional digits, which \
                 would leave the price fewer than {} to stay exact in the ledger; size the order \
                 to a lot grid",
                order.order_id.as_str(),
                Self::MIN_PRICE_DIGITS
            )));
        }
        let step = Decimal::from_raw(10i128.pow(SCALE_DIGITS - keep));
        let nearest = price.round_dp(keep);
        let rounded = match order.side {
            Side::Buy if nearest < price => nearest.checked_add(step),
            Side::Sell if nearest > price => nearest.checked_sub(step),
            _ => Some(nearest),
        };
        match rounded {
            Some(rounded) if rounded > Decimal::ZERO => Ok(rounded),
            _ => Err(Error::numeric(format!(
                "the fill price {price} on order {} cannot be held at {keep} fractional digits \
                 without becoming non-positive",
                order.order_id.as_str()
            ))),
        }
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

/// Refuse `admission` unless it is to list or seed exactly `book`'s
/// instrument at exactly `venue`.
///
/// The three exact comparisons `qip_brokers::exchange::SimulatedExchange`
/// makes before it lists, with no normalisation, and with one difference that
/// is the reason this surface exists: the factory seeds by replacing the
/// listed empty book with the rule's ladder, so an admission to seed rests a
/// book here as well as one to list. An admission to create does not — create
/// records a market and its rule and rests nothing, so a create admission
/// that reached a venue would be a market listed on the strength of a
/// permission nobody gave for listing it.
fn admits_book(venue: &str, book: &OrderBook, admission: &CreationAdmission) -> Result<()> {
    let request = admission.request();
    let rests_a_book = matches!(request.action, CreationAction::List | CreationAction::Seed);
    if !rests_a_book || request.venue != venue || request.product != book.object_id.as_str() {
        return Err(Error::denied(format!(
            "no book for {} was rested at {venue}: the admission offered is to {} {} at {}, and \
             a book rests only under an admission to list or seed exactly this instrument at \
             exactly this venue; admit that request through CreationPermissions::admit and \
             offer it instead",
            book.object_id.as_str(),
            request.action.as_str(),
            request.product,
            request.venue
        )));
    }
    Ok(())
}

/// Refuse a book its admission's action does not cover (R12-EXEC-5079).
///
/// `admits_book` takes List or Seed alike, and until 2026-10-24 that was the
/// whole check, so either admission did the other's work: a List admission
/// rested a populated book — seeding with no seeding permission recorded —
/// and a Seed admission listed an instrument nobody had listed. Listing
/// rests an empty book once; seeding replaces the book of an instrument
/// already listed, which is the factory's sequence and the only one.
fn refuse_unlike_its_action(
    book: &OrderBook,
    admission: &CreationAdmission,
    listed: bool,
) -> Result<()> {
    let product = book.object_id.as_str();
    match admission.request().action {
        CreationAction::List if listed => Err(Error::denied(format!(
            "{product} is already listed here, and a listing is made once rather than replaced; \
             seed the listed book under a seeding admission instead"
        ))),
        CreationAction::List if !book.is_empty() => Err(Error::denied(format!(
            "a listing of {product} offered a book with resting levels; listing rests an \
                 empty book, and resting liquidity is seeding, which needs its own admission"
        ))),
        CreationAction::Seed if !listed => Err(Error::denied(format!(
            "{product} is not listed here, so there is no book to seed; list it first under a \
             listing admission"
        ))),
        _ => Ok(()),
    }
}

impl ListingVenue for SimulatedBroker {
    fn venue_name(&self) -> &str {
        Broker::name(self)
    }

    fn is_simulated(&self) -> bool {
        Broker::is_simulated(self)
    }

    fn listed_book(&self, object_id: &str) -> Option<&OrderBook> {
        self.books.get(object_id)
    }

    fn list_book(&mut self, book: OrderBook, admission: &CreationAdmission) -> Result<()> {
        admits_book(Broker::name(self), &book, admission)?;
        let listed = self.books.contains_key(book.object_id.as_str());
        refuse_unlike_its_action(&book, admission, listed)?;
        self.books.insert(book.object_id.as_str().to_string(), book);
        Ok(())
    }

    fn withdraw_book(&mut self, object_id: &str) -> Option<OrderBook> {
        self.books.remove(object_id)
    }
}

impl Broker for SimulatedBroker {
    fn listing_venue(&mut self) -> Option<&mut dyn ListingVenue> {
        Some(self)
    }

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
            // Deliberately *not* recorded against the order's type. This
            // refusal is the seeded coin flip and carries no reason, so it
            // says nothing about whether the venue supports `market`; filing
            // it under the type would let one unlucky order teach the
            // promotion ladder that the venue refuses a type it accepts.
            return Err(Error::unavailable(format!(
                "the venue rejected order {}",
                order.order_id.as_str()
            )));
        }
        // The venue has taken the instruction. Recorded here rather than
        // after the fill, because a limit order the market never reaches was
        // still accepted, and acceptance is what §34.4's order-type check is
        // about.
        self.accepted_order_types
            .insert(order.order_type.as_str().to_string());

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

        let price = Self::ledger_exact_price(order, quantity, self.fill_price(order, quantity)?)?;
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

        // The measured half of §34.4's fee check: what was actually billed,
        // and the notional it was billed on. Accumulated rather than derived
        // from the settings, so the figure the ladder reads comes from the
        // fills that happened. On overflow the tally becomes `None` — the
        // venue stops claiming to itemise — rather than saturating, because a
        // fee total short by an unknown amount reads as a cheap venue. It
        // never refuses the fill: a diagnostic must not decide whether an
        // order trades.
        let notional = quantity.checked_mul(price);
        self.fees_charged = self.fees_charged.and_then(|total| total.checked_add(costs));
        self.notional_filled = self
            .notional_filled
            .and_then(|total| notional.and_then(|n| total.checked_add(n)));

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

    /// What this venue declares, and the three facts it can honestly claim to
    /// have observed.
    ///
    /// **`acknowledgement_latency` is [`None`], and that is the whole point of
    /// this implementation.** This venue does not time its acknowledgements;
    /// it stamps `at + settings.latency` on every fill, so a latency read back
    /// out of it is the declaration with extra steps. §34.4's observed rung
    /// exists to catch a venue that misdescribes itself, and a number checking
    /// itself catches nothing — it would pass the gate every time and mean
    /// nothing when it did. Reporting [`None`] costs this venue the rung; the
    /// alternative costs the rung its meaning.
    ///
    /// `rejected_order_types` is empty for a related reason. This venue
    /// refuses a small seeded fraction of orders *for no stated reason*, and
    /// that refusal is not attributable to the order's type. An unattributed
    /// refusal filed under a type is a finding nobody made.
    fn observation(&self) -> Option<VenueObservation> {
        let capabilities = self.capabilities();
        // The declared rate is `Decimal` at source — `SimulationSettings`
        // keeps commission as exact fixed point precisely because it
        // multiplies a notional — so the crossing into basis points stays in
        // `Decimal` and never travels through the `f64` copy on
        // `VenueCapabilities`, which exists only because other adapters
        // publish one.
        let fee_bps = self
            .settings
            .commission_rate
            .checked_mul(Decimal::from_int(10_000))?;
        Some(VenueObservation {
            venue: qip_contracts::venue::VenueId::new(self.name()),
            declared: DeclaredVenueProfile {
                // A book-sweeping venue with a central limit order book,
                // which is what `fill_price` does when a book is supplied.
                // Declared rather than inferred: the venue says what it is
                // and the ladder checks what it does.
                class: qip_contracts::venue::VenueClass::Exchange,
                fee_bps,
                acknowledgement_latency: self.settings.latency,
                order_types: capabilities.supported_types.into_iter().collect(),
            },
            observed: ObservedVenueFacts {
                acknowledgements: self.submitted,
                acknowledgement_latency: None,
                fees_charged: self.fees_charged,
                notional_filled: self.notional_filled,
                accepted_order_types: self.accepted_order_types.clone(),
                rejected_order_types: std::collections::BTreeSet::new(),
            },
        })
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

#[cfg(test)]
mod ledger_exact_tests {
    use super::*;
    use crate::order::OrderType;
    use qip_core::ids::{ObjectId, OrderId};
    use qip_core::{Timestamp, dec};

    fn order(side: Side) -> Order {
        Order::new(
            OrderId::from_string("ord-exact"),
            ObjectId::from_string("obj-EXACT"),
            side,
            dec!("1714"),
            OrderType::Market,
            dec!("87.538238117"),
            "prop-exact",
            vec!["hyp-exact".to_string()],
            "exact",
            Timestamp::from_secs(1_760_000_000),
        )
    }

    /// The ledger's own rule: the product of two nine-digit decimals is exact
    /// only when it needs no more than nine fractional digits.
    fn notional_is_exact(quantity: Decimal, price: Decimal) -> bool {
        (quantity.raw() * price.raw()) % qip_core::decimal::SCALE == 0
    }

    #[test]
    fn a_buy_price_is_rounded_up_to_the_digits_the_notional_can_carry() {
        // 1028.4 carries one fractional digit, so the price keeps eight. The
        // ninth digit is 3, so the nearest eight-digit price is BELOW this one:
        // only the adverse step makes the buyer pay at least the price.
        let quantity = dec!("1028.4");
        let price = dec!("87.538238113");
        assert!(
            !notional_is_exact(quantity, price),
            "premise: the unrounded notional is the one the ledger refused"
        );
        let fitted = SimulatedBroker::ledger_exact_price(&order(Side::Buy), quantity, price)
            .expect("a price with eight digits to keep fits");
        assert!(notional_is_exact(quantity, fitted), "{fitted}");
        assert!(fitted >= price, "a buy was rounded in its own favour");
        assert!(fitted - price < dec!("0.00000001") + dec!("0.00000000001"));
    }

    #[test]
    fn a_sell_price_is_rounded_down_so_the_simulator_never_flatters() {
        let quantity = dec!("1028.4");
        let price = dec!("87.538238117");
        let fitted =
            SimulatedBroker::ledger_exact_price(&order(Side::Sell), quantity, price).expect("fits");
        assert!(notional_is_exact(quantity, fitted), "{fitted}");
        assert!(fitted <= price, "a sell was rounded in its own favour");
    }

    #[test]
    fn a_price_already_exact_is_left_alone() {
        let fitted =
            SimulatedBroker::ledger_exact_price(&order(Side::Buy), dec!("1000"), dec!("100.25"))
                .expect("fits");
        assert_eq!(fitted, dec!("100.25"));
    }

    #[test]
    fn a_quantity_too_fine_to_leave_the_price_four_digits_is_refused_not_clamped() {
        // Six fractional digits leave three for the price: below the floor.
        let error = SimulatedBroker::ledger_exact_price(
            &order(Side::Buy),
            dec!("1.123456"),
            dec!("87.538238117"),
        )
        .expect_err("a price of three digits is not a price");
        assert!(
            error.message().contains("fractional digits"),
            "{}",
            error.message()
        );
        // Five digits is the most a fill may carry and still leave four.
        assert!(
            SimulatedBroker::ledger_exact_price(
                &order(Side::Buy),
                dec!("1.12345"),
                dec!("87.538238117")
            )
            .is_ok()
        );
    }

    #[test]
    fn a_price_that_would_round_to_nothing_is_refused() {
        let error = SimulatedBroker::ledger_exact_price(
            &order(Side::Sell),
            dec!("1.12345"),
            dec!("0.00000001"),
        )
        .expect_err("a sell price rounded down to zero");
        assert!(
            error.message().contains("non-positive"),
            "{}",
            error.message()
        );
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod listing_tests {
    //! The admission on the listing primitive itself (R12-COVERAGE-5107).
    //!
    //! Every admission offered below is genuine — minted by a register
    //! holding all three authorities' permissions — so a refusal is the
    //! venue's own comparison and never the register's. Each test then
    //! reaches the venue the way any new production holder would, through
    //! `Broker::listing_venue`, rather than through the factory, whose own
    //! checks would otherwise answer first and hide a hole in this one.

    use super::*;
    use crate::creation::{CreationAuthority, CreationPermissions, CreationRequest};
    use qip_core::ids::ObjectId;

    const VENUE: &str = "simulated-venue";
    const PRODUCT: &str = "TEST/USD";
    const REFUSAL: &str = "a book rests only under an admission to list or seed exactly this \
                           instrument at exactly this venue";

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn admitted(action: CreationAction, venue: &str, product: &str) -> Result<CreationAdmission> {
        let request = CreationRequest::new(action, venue, product, "paper");
        let mut permissions = CreationPermissions::new();
        for authority in CreationAuthority::ALL {
            permissions.record(authority, &request, "listing test fixture")?;
        }
        permissions.admit(&request)
    }

    fn empty_book(product: &str) -> OrderBook {
        OrderBook::new(ObjectId::from_string(product), VENUE, at())
    }

    /// Offer `admission` with a book for `product` through the port, and
    /// return what the venue answered and whether anything now rests.
    fn offer(admission: &CreationAdmission, product: &str) -> Result<(Result<()>, bool)> {
        let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 3);
        let venue = broker
            .listing_venue()
            .ok_or_else(|| Error::invalid("the simulated broker offers no listing venue"))?;
        // Premise: the surface under test is the one the factory is handed.
        assert_eq!(venue.venue_name(), VENUE);
        assert!(venue.listed_book(product).is_none());
        let answer = venue.list_book(empty_book(product), admission);
        let rests = venue.listed_book(product).is_some();
        Ok((answer, rests))
    }

    fn assert_refused(answer: Result<()>, rests: bool, case: &str) {
        let refusal = answer.expect_err(case);
        assert_eq!(refusal.code(), "denied", "{case}: {}", refusal.message());
        assert!(
            refusal.message().contains(REFUSAL),
            "{case} was refused for another reason: {}",
            refusal.message()
        );
        assert!(!rests, "{case} was refused and a book rests anyway");
    }

    /// The contrast the refusals below are read against: an admission to
    /// list rests the empty book it is for, and one to seed then rests the
    /// populated book over it. Without it a venue that refused everything
    /// would pass every refusal. Since 2026-10-24 the seed is offered over a
    /// listed book, because a seeding admission no longer lists one
    /// (R12-EXEC-5079).
    #[test]
    fn an_admission_to_list_or_to_seed_this_instrument_here_rests_its_book() -> Result<()> {
        let (answer, rests) = offer(&admitted(CreationAction::List, VENUE, PRODUCT)?, PRODUCT)?;
        answer?;
        assert!(rests, "an admission to list rested nothing");

        let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 3);
        let venue = broker
            .listing_venue()
            .ok_or_else(|| Error::invalid("the simulated broker offers no listing venue"))?;
        venue.list_book(
            empty_book(PRODUCT),
            &admitted(CreationAction::List, VENUE, PRODUCT)?,
        )?;
        venue.list_book(
            seeded_book(PRODUCT),
            &admitted(CreationAction::Seed, VENUE, PRODUCT)?,
        )?;
        assert!(
            venue
                .listed_book(PRODUCT)
                .is_some_and(|book| !book.is_empty()),
            "an admission to seed rested nothing over the listed book"
        );
        Ok(())
    }

    fn seeded_book(product: &str) -> OrderBook {
        let mut book = empty_book(product);
        book.asks.push(qip_market::book::BookLevel {
            price: Decimal::from_int(100),
            size: Decimal::from_int(10),
            order_count: 1,
        });
        book
    }

    /// R12-EXEC-5079: either admission used to do the other's work, because
    /// `admits_book` takes List or Seed alike. A listing admission rested a
    /// populated book — seeding with no seeding permission — and a seeding
    /// admission listed an instrument nobody had listed; a second listing
    /// replaced a live one. Each is refused, and nothing rests.
    ///
    /// Mutation: make `refuse_unlike_its_action` return `Ok(())` for every
    /// action — all three cases rest a book.
    #[test]
    fn a_listing_rests_an_empty_book_once_and_a_seed_rests_only_over_a_listed_one() -> Result<()> {
        let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 3);
        let venue = broker
            .listing_venue()
            .ok_or_else(|| Error::invalid("the simulated broker offers no listing venue"))?;

        let seeded_under_list = venue
            .list_book(
                seeded_book(PRODUCT),
                &admitted(CreationAction::List, VENUE, PRODUCT)?,
            )
            .expect_err("a listing admission rested a populated book");
        assert!(
            seeded_under_list
                .message()
                .contains("resting liquidity is seeding"),
            "{}",
            seeded_under_list.message()
        );
        assert!(
            venue.listed_book(PRODUCT).is_none(),
            "the populated book rests anyway"
        );

        let seed_unlisted = venue
            .list_book(
                seeded_book(PRODUCT),
                &admitted(CreationAction::Seed, VENUE, PRODUCT)?,
            )
            .expect_err("a seeding admission listed an instrument nobody listed");
        assert!(
            seed_unlisted.message().contains("is not listed here"),
            "{}",
            seed_unlisted.message()
        );
        assert!(
            venue.listed_book(PRODUCT).is_none(),
            "the unlisted seed rests anyway"
        );

        venue.list_book(
            empty_book(PRODUCT),
            &admitted(CreationAction::List, VENUE, PRODUCT)?,
        )?;
        let relisted = venue
            .list_book(
                empty_book(PRODUCT),
                &admitted(CreationAction::List, VENUE, PRODUCT)?,
            )
            .expect_err("a second listing replaced the first");
        assert!(
            relisted.message().contains("already listed"),
            "{}",
            relisted.message()
        );
        Ok(())
    }

    /// Create records a market and its rule and rests nothing; an admission
    /// to create is not one to list. Mutation: drop `!rests_a_book ||` from
    /// `admits_book` and the create admission rests a book.
    #[test]
    fn an_admission_to_create_rests_no_book() -> Result<()> {
        let (answer, rests) = offer(&admitted(CreationAction::Create, VENUE, PRODUCT)?, PRODUCT)?;
        assert_refused(answer, rests, "an admission to create rested a book");
        Ok(())
    }

    /// Permission to list at one venue is not permission to list at this
    /// one. Mutation: drop `request.venue != venue ||` from `admits_book` and
    /// the admission for XOTHER rests a book at the simulator.
    #[test]
    fn an_admission_for_another_venue_rests_no_book() -> Result<()> {
        let (answer, rests) = offer(&admitted(CreationAction::List, "XOTHER", PRODUCT)?, PRODUCT)?;
        assert_refused(answer, rests, "an admission for XOTHER rested a book here");
        Ok(())
    }

    /// Permission for one instrument is not permission for another. The
    /// failure this prevents is the one the finding named: a holder of
    /// `listing_venue` resting a book for any object id it liked under
    /// whatever admission it happened to hold. Mutation: drop
    /// `request.product != book.object_id.as_str()` from `admits_book` and
    /// the book for OTHER/USD rests.
    #[test]
    fn an_admission_for_another_instrument_rests_no_book() -> Result<()> {
        let (answer, rests) = offer(
            &admitted(CreationAction::List, VENUE, PRODUCT)?,
            "OTHER/USD",
        )?;
        assert_refused(answer, rests, "an admission for TEST/USD rested OTHER/USD");
        Ok(())
    }
}
