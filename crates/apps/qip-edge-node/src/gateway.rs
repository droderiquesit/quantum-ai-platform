//! The node's venue seam: the cell's orders, through a real matching engine.
//!
//! Until this module existed the node held a [`Cell`] and no gateway at all —
//! the venue adapter framework lived in `qip-brokers` and shipped in nothing.
//! This is the composition that makes the deployable actually contain the
//! seam: [`SimulatedGateway`] implements the cell's [`Placer`] on top of
//! [`SimulatedExchange`], so an order the cell places clears the same
//! price-time matching, the same lot and tick admission, the same commission
//! arithmetic and the same rejection draw the broker test suite proves.
//!
//! Three properties carry over from the pieces rather than being re-argued:
//!
//! * **It cannot be live.** `AdapterClass` has no `Live` variant, and
//!   [`Placer::is_simulated`] answers from the venue's own `Broker`
//!   implementation, not from configuration. A paper fill is labelled at the
//!   source that produced it.
//! * **Fills come back on the independent channel.** The venue's fills are
//!   drained as [`DropCopyFill`]s for `Cell::observe_drop_copy`, so
//!   reconciliation compares what the cell believes against what the venue
//!   says — the same two-channel shape a real deployment has, exercised in
//!   the deployable rather than only in a test harness.
//! * **A listing invented here says so.** An instrument the venue has not
//!   seen is listed on demand with provenance stamped synthetic, because a
//!   listing a simulator made up must never read as market data.
//!
//! [`Cell`]: qip_edge::cell::Cell

use qip_brokers::adapter::VenueAdapter;
use qip_brokers::credential::{
    RequirementKind, VenueCredential, requirements_of_kind, standard_requirements,
};
use qip_brokers::exchange::{ExchangeSettings, SimulatedExchange};
use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::error::Result;
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
