//! The node's venue seam: where the cell's orders meet a matching engine, and
//! where a deployment chooses which one.
//!
//! Until this module existed the node held a [`Cell`] and no gateway at all —
//! the venue adapter framework lived in `qip-brokers` and shipped in nothing.
//! [`SimulatedGateway`] was the composition that fixed that: it implements the
//! cell's [`Placer`] on top of [`SimulatedExchange`], so an order the cell
//! places clears the same price-time matching, the same lot and tick
//! admission, the same commission arithmetic and the same rejection draw the
//! broker test suite proves.
//!
//! [`RestGateway`] is the second composition, and the one that changes what
//! this binary is. It implements the same [`Placer`] on top of
//! [`RestOrderEntryAdapter`], which opens a socket. [`NodeGateway`] is the
//! choice between them, and [`crate::venue`] is where that choice is made,
//! refused and announced.
//!
//! # What carries over from the pieces, and what does not
//!
//! * **Fills come back on the independent channel.** Both gateways drain the
//!   venue's fills as [`DropCopyFill`]s for `Cell::observe_drop_copy`, so
//!   reconciliation compares what the cell believes against what the venue
//!   says — the same two-channel shape a real deployment has, exercised in the
//!   deployable rather than only in a test harness.
//! * **`is_simulated` is read from the adapter, never from configuration.**
//!   The flag that decides whether a fill is paper comes from the thing that
//!   produced the fill. What that flag *means* differs between the two, and the
//!   difference is the most important sentence in this module: for
//!   [`SimulatedGateway`] it is a fact about a book held in this process, and
//!   for [`RestGateway`] it is a claim about an endpoint that nothing here can
//!   check. See [`RestGateway`].
//! * **A listing invented here says so** — and only the simulated venue invents
//!   one. An instrument the simulator has not seen is listed on demand with
//!   provenance stamped synthetic, because a listing a simulator made up must
//!   never read as market data. A real venue has its own reference data and
//!   there is nothing here that would add to it.
//!
//! [`Cell`]: qip_edge::cell::Cell

use crate::venue::{LiveVenueChoice, VenueChoice};
use qip_brokers::adapter::VenueAdapter;
use qip_brokers::connection::ConnectionPhase;
use qip_brokers::credential::{
    RequirementKind, VenueCredential, requirements_of_kind, standard_requirements,
};
use qip_brokers::exchange::{ExchangeSettings, SimulatedDepth, SimulatedExchange};
use qip_brokers::rest::{RestOrderEntryAdapter, RestOrderStats};
use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::Timestamp;
use qip_edge::cell::Placer;
use qip_edge::dropcopy::DropCopyFill;
use qip_execution_engine::broker::Broker;
use qip_execution_engine::order::{Order, OrderType, Side};

/// The finest step the synthetic listing trades in.
///
/// One billionth — the smallest representable `Decimal` step — so any positive
/// price and quantity the cell computes is admissible. A real venue's lot and
/// tick come from its reference data; a synthetic listing has none, and
/// inventing a coarser step would refuse orders for a reason the venue never
/// stated.
const SYNTHETIC_STEP: Decimal = Decimal::from_raw(1);

/// The cell's paper gateway: one simulated venue, deterministically seeded.
#[derive(Debug)]
pub struct SimulatedGateway {
    exchange: SimulatedExchange,
    /// Fills awaiting collection by the drop-copy channel.
    pending: Vec<DropCopyFill>,
}

impl SimulatedGateway {
    /// Bring up a session against a freshly assembled venue.
    ///
    /// The credential carries *references* to the standard requirements —
    /// which environment variables would hold the session values — and no
    /// secret material, which is all a simulated venue authenticates against
    /// and all this binary is permitted to hold.
    pub fn new(venue: VenueId, seed: u64, at: Timestamp) -> Result<Self> {
        Self::with_settings(venue, ExchangeSettings::orderly(), seed, at)
    }

    /// The same, with a venue that refuses a share of orders for no stated
    /// reason — which real venues do, and which a paper session that never
    /// experiences it is not a rehearsal for.
    pub fn with_rejection_probability(
        venue: VenueId,
        seed: u64,
        rejection_probability: f64,
        at: Timestamp,
    ) -> Result<Self> {
        let settings = ExchangeSettings {
            rejection_probability,
            ..ExchangeSettings::orderly()
        };
        Self::with_settings(venue, settings, seed, at)
    }

    fn with_settings(
        venue: VenueId,
        settings: ExchangeSettings,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        let mut exchange = SimulatedExchange::new(venue.clone(), settings, seed, at);
        let enforced = requirements_of_kind(
            &standard_requirements(&venue),
            &[RequirementKind::Account, RequirementKind::SessionCredential],
        );
        let credential = VenueCredential::satisfying(
            venue.as_str(),
            format!("{}-paper", venue.as_str()),
            &enforced,
        )?;
        exchange.bring_up(&credential, at)?;
        Ok(Self {
            exchange,
            pending: Vec::new(),
        })
    }

    /// The venue this gateway reaches.
    pub fn venue(&self) -> &str {
        self.exchange.name()
    }

    /// The adapter class, read from the venue itself.
    pub fn class(&self) -> &'static str {
        if self.exchange.is_simulated() {
            "simulated"
        } else {
            "sandbox"
        }
    }

    pub fn submitted_count(&self) -> u64 {
        self.exchange.submitted_count()
    }

    pub fn rejected_count(&self) -> u64 {
        self.exchange.rejected_count()
    }

    /// Rest contra liquidity at a touch, listing the instrument if needed.
    ///
    /// This is how a test or a replay gives the venue something to trade
    /// against; the gateway itself never invents depth on the order path,
    /// because a venue that quotes whatever the taker asks for fills
    /// everything and proves nothing.
    pub fn seed_touch(
        &mut self,
        object_id: &ObjectId,
        side: Side,
        price: Decimal,
        quantity: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        self.ensure_listed(object_id, price, at)?;
        self.exchange
            .seed_liquidity(object_id, side, price, quantity, at)
    }

    /// Everything the venue has filled since the last drain, as the
    /// independent channel reports it.
    pub fn drain_drop_copies(&mut self) -> Vec<DropCopyFill> {
        std::mem::take(&mut self.pending)
    }

    /// Orders the venue holds open, from the venue's own book.
    ///
    /// The one observation that tells a buy from a sell at the matching
    /// engine without reading the order back: a buy rests against bids and
    /// fills against offers, and a test that only counts fills against the
    /// liquidity it happened to seed cannot tell the two apart.
    pub fn resting_count(&self) -> usize {
        self.exchange.resting_count()
    }

    /// The venue's resting depth for every instrument it lists, from the
    /// venue itself.
    ///
    /// What `crate::feed` publishes to the cell each pass. It exists on the
    /// simulated gateway alone: a real venue publishes its own feed, and a
    /// method here that returned depth for one would be this process
    /// inventing a market.
    pub fn quotes(&self) -> Vec<SimulatedDepth> {
        self.exchange.quotes()
    }

    fn ensure_listed(
        &mut self,
        object_id: &ObjectId,
        reference: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        if self.exchange.is_listed(object_id) {
            return Ok(());
        }
        self.exchange.list_synthetic(
            object_id.clone(),
            object_id.as_str(),
            reference,
            SYNTHETIC_STEP,
            SYNTHETIC_STEP,
            at,
        )
    }
}

impl Placer for SimulatedGateway {
    fn is_simulated(&self) -> bool {
        // From the venue's own Broker implementation, never from configuration:
        // the flag that decides whether a fill is paper comes from the thing
        // that produced the fill.
        self.exchange.is_simulated()
    }

    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        // The cell names the side of the book it takes: hitting the ask is a
        // buy, hitting the bid is a sell.
        let taking = match side {
            BookSide::Ask => Side::Buy,
            BookSide::Bid => Side::Sell,
        };
        self.ensure_listed(object_id, price, at)?;
        let order = Order::new(
            OrderId::from_string(order_id),
            object_id.clone(),
            taking,
            quantity,
            OrderType::Limit { price },
            price,
            format!("cell-order-{order_id}"),
            vec![format!("placed by the edge cell as {order_id}")],
            venue.as_str(),
            at,
        );
        let fills = self.exchange.submit(&order, at)?;
        for fill in fills {
            debug_assert!(fill.simulated, "a simulated venue produced a live fill");
            self.pending.push(DropCopyFill {
                order_id: fill.order_id.as_str().to_string(),
                venue: venue.clone(),
                quantity: fill.quantity,
                price: fill.price,
                at: fill.at,
            });
        }
        Ok(())
    }

    fn required_configuration(&self) -> Vec<String> {
        // The simulated venue is complete as it stands. What production still
        // needs — a real feed, a real order-entry session, a real drop-copy —
        // is reported by the node at startup, not hidden here.
        Vec::new()
    }
}

/// The cell's live gateway: one REST order-entry session against a real venue.
///
/// The composition mirrors [`SimulatedGateway`] exactly — same [`Placer`], same
/// drop-copy drain, same synthetic-listing rule does *not* apply because a real
/// venue has its own reference data — and differs in the one way that matters:
/// the orders leave the process.
///
/// # What it inherits, and does not re-argue
///
/// Everything about *how* an order is sent belongs to
/// [`RestOrderEntryAdapter`] and is proven by that crate's suite: the
/// idempotency key over the order's material terms, the refusal to retry an
/// ambiguous submit unless the venue documents that it deduplicates, the rule
/// that a fill is never inferred, and the unknown-order list that is the number
/// to alert on. This type adds no policy of its own to any of that. What it
/// adds is the plumbing: an [`Order`] built from the cell's intent, a session
/// re-proven before each send, and the venue's fills handed to the independent
/// drop-copy channel.
///
/// # `is_simulated` answers `true`, and that is not a claim about the money
///
/// [`Placer::is_simulated`] reads the adapter's own [`Broker::is_simulated`],
/// which is derived from `AdapterClass::is_paper` — and there is no
/// `AdapterClass::Live`, so it is `true` for every adapter in `qip-brokers`
/// including this one. Point the endpoint at a production host and the orders
/// are real while this method still answers `true`.
///
/// That is not papered over here and it is not a bug this type could fix: the
/// class is a claim about the endpoint the deployment supplied, and nothing in
/// this process can check it. It is surfaced instead — [`crate::venue`] refuses
/// to construct this gateway without an operator naming the endpoint, and the
/// node's start-up banner states the consequence in as many words.
///
/// # It does not seed liquidity, and there is nothing to seed
///
/// [`SimulatedGateway::seed_touch`] exists because a simulated venue has an
/// empty book until a test fills it. A real venue has a real book, and a method
/// here that appeared to rest contra liquidity would be inventing depth on a
/// market this process does not own.
pub struct RestGateway {
    adapter: RestOrderEntryAdapter,
    venue: VenueId,
    /// Fills awaiting collection by the drop-copy channel.
    pending: Vec<DropCopyFill>,
    /// Orders this gateway handed to the adapter. Counted here rather than read
    /// from the adapter's own `submits_sent` so the health surface reports the
    /// same quantity for both gateways.
    submitted: u64,
    rejected: u64,
}

/// Written by hand because the adapter holds a session secret. `Secret`
/// redacts and `RestOrderEntryAdapter`'s own `Debug` reports only whether one
/// is present; this keeps that property rather than re-deriving past it.
impl std::fmt::Debug for RestGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestGateway")
            .field("venue", &self.venue.as_str())
            .field("adapter", &self.adapter)
            .field("pending_drop_copies", &self.pending.len())
            .field("submitted", &self.submitted)
            .field("rejected", &self.rejected)
            .finish()
    }
}

impl RestGateway {
    /// Bring a session up: connect, authenticate, and prove it heartbeats.
    ///
    /// All three, in that order, before the node serves anything. A REST venue
    /// has no session to hold open, so "authenticated" here means the venue
    /// accepted one real request carrying the credential — which is the only
    /// way to learn that a secret is wrong at start-up rather than at the first
    /// order. The heartbeat is what promotes the session to ready; an adapter
    /// that has authenticated and never been heard from is not ready, it is
    /// merely new, and `ConnectionState` will refuse to mint a ticket for it.
    ///
    /// Every failure here is fatal to start-up on purpose. A node that came up
    /// with a dead venue session would serve a healthy liveness probe while
    /// refusing every order the cell produced.
    pub fn connect(choice: &LiveVenueChoice, at: Timestamp) -> Result<Self> {
        let venue = choice.venue.clone();
        let mut adapter = RestOrderEntryAdapter::new(venue.clone(), choice.adapter_config(), at)?;

        // The credential carries the *resolved* secret, not a reference to the
        // variable it came from: this adapter has to write the value into a
        // header, and a reference is what a simulator takes.
        let enforced = requirements_of_kind(
            &standard_requirements(&venue),
            &[RequirementKind::Account, RequirementKind::SessionCredential],
        );
        let mut credential = VenueCredential::new(venue.as_str(), &choice.account)?;
        for requirement in &enforced {
            credential = if requirement.kind == RequirementKind::SessionCredential {
                credential.with_secret(
                    &requirement.name,
                    &requirement.env_var,
                    choice.credential.clone(),
                )
            } else {
                credential.with_reference(&requirement.name, &requirement.env_var)
            };
        }

        adapter.connect(at)?;
        adapter.authenticate(&credential, at)?;
        adapter.heartbeat(at)?;
        Ok(Self {
            adapter,
            venue,
            pending: Vec::new(),
            submitted: 0,
            rejected: 0,
        })
    }

    /// The venue this gateway reaches.
    pub fn venue(&self) -> &str {
        self.venue.as_str()
    }

    /// The adapter class, read from the adapter rather than from this node's
    /// configuration. See [`RestGateway`]'s documentation for what it does and
    /// does not settle.
    pub fn class(&self) -> &'static str {
        self.adapter.class().as_str()
    }

    pub fn submitted_count(&self) -> u64 {
        self.submitted
    }

    pub fn rejected_count(&self) -> u64 {
        self.rejected
    }

    /// Orders whose state nobody knows, and the number to alert on.
    ///
    /// Each one may be working at the venue, may have filled, or may never have
    /// arrived. Published rather than kept internal because an unknown order is
    /// a position the platform cannot account for, and the only thing worse
    /// than having one is not knowing how many there are.
    pub fn unknown_orders(&self) -> usize {
        self.adapter.unknown_orders().len()
    }

    /// The adapter's own counters, for the health surface.
    pub fn stats(&self) -> RestOrderStats {
        self.adapter.stats()
    }

    /// Everything the venue has filled since the last drain.
    pub fn drain_drop_copies(&mut self) -> Vec<DropCopyFill> {
        std::mem::take(&mut self.pending)
    }

    /// Re-prove the session if it has gone stale, before an order is sent.
    ///
    /// This node has no scheduler — the liveness probe is its only periodic
    /// event — so nothing else asks the venue whether it is still there. That
    /// leaves two choices at the moment an order is placed: send into a session
    /// last confirmed some unknown time ago, or spend a round trip proving it
    /// first. This spends the round trip, and that is a real cost on the
    /// latency-sensitive path, named here rather than hidden: a production cell
    /// heartbeats on a timer and this call is then almost always a no-op.
    fn ensure_ready(&mut self, at: Timestamp) -> Result<()> {
        if self.adapter.connection().effective_phase(at) == ConnectionPhase::Ready {
            return Ok(());
        }
        self.adapter.heartbeat(at).map(|_| ())
    }
}

impl Placer for RestGateway {
    fn is_simulated(&self) -> bool {
        // From the adapter's own `Broker` implementation, never from this
        // node's configuration. It answers `true` for every adapter class this
        // crate has; see the type's documentation for why that is a claim about
        // the endpoint rather than a guarantee about the money.
        self.adapter.is_simulated()
    }

    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        // A cell that reached a venue it was not configured for would be
        // sending an order for an account nobody chose. Refused before the
        // session is touched.
        if venue != &self.venue {
            return Err(Error::denied(format!(
                "the cell placed order {order_id} on {} and this gateway holds a session with {}. \
                 An order is not re-routed to whatever venue happens to be connected",
                venue.as_str(),
                self.venue.as_str()
            )));
        }
        let taking = match side {
            BookSide::Ask => Side::Buy,
            BookSide::Bid => Side::Sell,
        };
        self.ensure_ready(at)?;
        let order = Order::new(
            OrderId::from_string(order_id),
            object_id.clone(),
            taking,
            quantity,
            OrderType::Limit { price },
            price,
            format!("cell-order-{order_id}"),
            vec![format!("placed by the edge cell as {order_id}")],
            venue.as_str(),
            at,
        );

        // `Broker::submit` mints its own readiness ticket, so this path cannot
        // reach a venue whose session is not ready even if `ensure_ready`
        // above were wrong.
        self.submitted = self.submitted.saturating_add(1);
        let fills = match self.adapter.submit(&order, at) {
            Ok(fills) => fills,
            Err(error) => {
                self.rejected = self.rejected.saturating_add(1);
                return Err(error);
            }
        };
        for fill in fills {
            self.pending.push(DropCopyFill {
                order_id: fill.order_id.as_str().to_string(),
                venue: venue.clone(),
                quantity: fill.quantity,
                price: fill.price,
                at: fill.at,
            });
        }
        Ok(())
    }

    fn required_configuration(&self) -> Vec<String> {
        // The adapter's own standing requirements, verbatim and in full. The
        // first of them is that nothing in the code can tell a sandbox host
        // from a production one, which is the reason this node has a banner and
        // an acknowledgement variable at all — so it is reported through the
        // same channel every other unmet production requirement is, rather than
        // being considered discharged by the start-up text.
        let mut requirements = self.adapter.missing_configuration();
        requirements.extend(
            RestOrderEntryAdapter::REQUIREMENTS
                .iter()
                .map(|requirement| (*requirement).to_string()),
        );
        requirements
    }
}

/// The venue seam the node actually holds, whichever was selected.
///
/// An enum rather than a `Box<dyn Placer>` for one reason: the health surface
/// and the banner ask questions — which class, which venue, how many unknown
/// orders — that are not on the [`Placer`] trait and should not be, because
/// they are the node's reporting rather than the cell's contract. Keeping the
/// two variants nameable is also what lets a test assert *which* was chosen,
/// which is the property task one exists to establish.
#[derive(Debug)]
pub enum NodeGateway {
    /// The in-process matching engine.
    Simulated(SimulatedGateway),
    /// A real venue over HTTP.
    Live(RestGateway),
}

impl NodeGateway {
    /// The venue this gateway reaches, by name.
    pub fn venue(&self) -> &str {
        match self {
            Self::Simulated(gateway) => gateway.venue(),
            Self::Live(gateway) => gateway.venue(),
        }
    }

    /// The adapter class, always read from the adapter that would produce the
    /// fills.
    pub fn class(&self) -> &'static str {
        match self {
            Self::Simulated(gateway) => gateway.class(),
            Self::Live(gateway) => gateway.class(),
        }
    }

    /// Whether an order placed through this gateway leaves the process.
    ///
    /// The question `class` cannot answer: both classes report themselves
    /// paper, and only one of them opens a socket.
    pub const fn reaches_a_socket(&self) -> bool {
        matches!(self, Self::Live(_))
    }

    pub fn submitted_count(&self) -> u64 {
        match self {
            Self::Simulated(gateway) => gateway.submitted_count(),
            Self::Live(gateway) => gateway.submitted_count(),
        }
    }

    pub fn rejected_count(&self) -> u64 {
        match self {
            Self::Simulated(gateway) => gateway.rejected_count(),
            Self::Live(gateway) => gateway.rejected_count(),
        }
    }

    /// Orders whose state nobody knows. Always zero for the simulated venue,
    /// which cannot lose an order because it never sends one.
    pub fn unknown_orders(&self) -> usize {
        match self {
            Self::Simulated(_) => 0,
            Self::Live(gateway) => gateway.unknown_orders(),
        }
    }

    pub fn drain_drop_copies(&mut self) -> Vec<DropCopyFill> {
        match self {
            Self::Simulated(gateway) => gateway.drain_drop_copies(),
            Self::Live(gateway) => gateway.drain_drop_copies(),
        }
    }

    /// The simulated gateway, when that is what was chosen.
    ///
    /// The pass loop takes the simulated gateway by its own type, so this is
    /// the one place the choice is narrowed — and `None` for the live
    /// gateway is what keeps the simulated feed from ever pricing a real
    /// order.
    pub fn simulated_mut(&mut self) -> Option<&mut SimulatedGateway> {
        match self {
            Self::Simulated(gateway) => Some(gateway),
            Self::Live(_) => None,
        }
    }

    /// Build the gateway the choice names.
    ///
    /// The simulated venue is assembled in process and cannot fail for a reason
    /// outside it. The live one opens a socket, authenticates and heartbeats,
    /// and any of the three failing stops the node — see
    /// [`RestGateway::connect`].
    pub fn open(choice: &VenueChoice, venue: VenueId, at: Timestamp) -> Result<Self> {
        match choice {
            VenueChoice::Simulated { seed } => {
                SimulatedGateway::new(venue, *seed, at).map(Self::Simulated)
            }
            VenueChoice::Live(live) => RestGateway::connect(live, at).map(Self::Live),
        }
    }
}

impl Placer for NodeGateway {
    fn is_simulated(&self) -> bool {
        match self {
            Self::Simulated(gateway) => gateway.is_simulated(),
            Self::Live(gateway) => gateway.is_simulated(),
        }
    }

    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        match self {
            Self::Simulated(gateway) => {
                gateway.place(order_id, object_id, venue, side, quantity, price, at)
            }
            Self::Live(gateway) => {
                gateway.place(order_id, object_id, venue, side, quantity, price, at)
            }
        }
    }

    fn required_configuration(&self) -> Vec<String> {
        match self {
            Self::Simulated(gateway) => gateway.required_configuration(),
            Self::Live(gateway) => gateway.required_configuration(),
        }
    }
}
