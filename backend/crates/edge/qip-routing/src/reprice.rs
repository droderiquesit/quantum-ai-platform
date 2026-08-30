//! Dynamic repricing: deciding a resting order is stale, and replacing it
//! without ever having two orders in the market for one intention.
//!
//! # Where this sits, and why here
//!
//! The orders worth repricing are the resting limit children this crate
//! already accounts for — [`crate::children::ChildOrder`] under a
//! [`crate::children::ParentOrder`] — priced against the [`Touch`] this crate
//! already reads off a venue book. The OMS in `qip-execution-engine` deals in
//! parent intentions and hands them to a broker whole; the shares that
//! actually rest at a venue, and the conservation arithmetic that makes
//! withdrawing one safe, live here. So does the only "requote" the workspace
//! had before this module: the *cost assumption* in [`crate::health`], which
//! prices what a forced requote costs. This module is the mechanism that
//! assumption was waiting for.
//!
//! # The seam: what calls this, with what, at what cadence
//!
//! Nothing here sends anything, and nothing is wired yet — deliberately. The
//! intended composition:
//!
//! * **Caller**: the edge cell's gateway loop (`qip-edge-node`), once per book
//!   update, *after* draining [`crate::gateway::GatewayEvent`]s into the
//!   parent — fills and cancels must be booked before staleness is judged,
//!   or the repricer prices a quantity that no longer exists.
//! * **Inputs**: each working child, the current [`Touch`] read off that
//!   venue's book, and the caller's clock. Policy values come from
//!   configuration, like every other declared number in this workspace.
//! * **Outputs**: [`RepriceDecision::CancelAndReplace`] is an instruction the
//!   caller carries to [`crate::gateway::Gateway::cancel`]. Only after the
//!   venue's cancel acknowledgement has been applied to the parent does
//!   [`Repricer::on_cancel_acknowledged`] mint the replacement child, which
//!   the caller sends like any new order.
//!
//! # Cancel-plus-new, never amend — and why
//!
//! The [`crate::gateway::Gateway`] trait offers `replace`, and this module
//! deliberately never asks for it. The live venue adapter
//! (`qip-brokers`' REST order entry) refuses amendment outright, and its
//! reasoning is the policy here: a cancel is acknowledged or it is not, and
//! either way the order's size is knowable; an amend that times out leaves a
//! live order whose price and quantity the client *cannot compute* — the one
//! state worse than losing queue priority. Queue priority is the cheaper
//! thing to lose, so it is what this module spends.
//!
//! The replacement is a **new order**: a fresh client id from
//! [`crate::children::ParentOrder::next_client_id`], which downstream becomes
//! a fresh idempotency key — `qip-brokers` derives its key from the order id
//! and terms, so reusing the old id would make an honouring venue dedupe the
//! replacement away as a retry of the order that was just cancelled.
//!
//! And the replacement is **not minted until the cancel is acknowledged**. A
//! replace racing its own cancel is two live orders for one intention, and if
//! the market trades through both prices, both fill — a doubled position
//! measured only at reconciliation. Two mechanisms enforce the ordering: this
//! type refuses to construct a replacement for a child that is not terminal,
//! and [`crate::children::ParentOrder::attach`] refuses a child whose
//! quantity is still working at a venue. Belt, and braces.
//!
//! # Partial fills
//!
//! The replacement carries the child's *remainder* at acknowledgement time
//! and nothing else. The booked fill is never re-sent — the same discipline
//! as `qip-brokers`' fill deduplication, applied to quantity instead of fill
//! ids: re-sending a filled share is inventing a position by arithmetic.
//! A child that filled completely while its cancel was in flight yields no
//! replacement at all.
//!
//! # Budgets, and the failure they name
//!
//! A repricer with no budget chases a fast market: every tick the touch moves,
//! it pays a cancel round trip and rejoins the back of a queue that is moving
//! away — self-inflicted throttling that looks like diligence in a log.
//! Requotes are therefore budgeted per order and per instrument per window
//! (policy values), and exhaustion is a named decision
//! ([`RepriceDecision::Throttled`]) rather than a silent skip, because "the
//! repricer stopped chasing" is a sentence somebody has to be able to finish.
//! An instruction spends its budget when it is issued, whether or not the
//! cancel later succeeds: chasing is measured in instructions sent.
//!
//! # What this module does not promise
//!
//! * It does not send, cancel, or transmit anything; the caller owns the
//!   gateway and the event loop.
//! * It only reprices resting **limit** children. A peg follows the book at
//!   the venue, a market/IOC/FOK order does not rest; none are its business.
//! * It never crosses. The replacement rejoins the touch passively; turning a
//!   passive order into an aggression is an urgency decision that belongs to
//!   whoever set the order's type.
//! * It holds state per process, unpersisted. A restart forgets in-flight
//!   cancels and spent budgets; the venue's own records, not this type's, are
//!   the reconciliation of record.
//!
//! Everything is deterministic: the same children, touches and timestamps
//! produce the same decisions, to the digit — `at` is a parameter, budgets
//! use fixed windows floored from it, and nothing reads a clock or a random
//! source.

use crate::children::{ChildOrder, ParentOrder};
use crate::ordertype::{RoutedOrderType, Touch};
use qip_contracts::message::BookSide;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::OrderId;
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where the lines are drawn. Every field is a declared policy value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepricePolicy {
    /// The instrument's price increment. Declared here because
    /// [`crate::venue::VenueProfile`] carries only a quantity lot; staleness
    /// in "ticks" is meaningless without it.
    pub tick: Decimal,
    /// Ticks behind the touch at or past which a resting order is stale.
    /// At least one: a zero threshold requotes on any breath, which is the
    /// chasing this policy exists to prevent.
    pub max_drift_ticks: u32,
    /// Basis points behind the touch at or past which it is stale, whichever
    /// of the two thresholds binds first.
    pub max_drift_bps_f64: f64,
    /// Most requote instructions one parent order may spend over its life.
    pub max_requotes_per_order: u32,
    /// Most requote instructions one instrument may spend per window.
    pub max_requotes_per_instrument: u32,
    /// The window the per-instrument budget refills on. Fixed windows floored
    /// from the caller's timestamp, so a replay draws the same boundaries.
    pub per_instrument_window: Duration,
}

impl RepricePolicy {
    /// A policy with the given tick and both drift thresholds; budgets start
    /// at conservative values a deployment tightens or relaxes explicitly.
    pub fn new(tick: Decimal, max_drift_ticks: u32, max_drift_bps_f64: f64) -> Self {
        Self {
            tick,
            max_drift_ticks,
            max_drift_bps_f64,
            max_requotes_per_order: 3,
            max_requotes_per_instrument: 30,
            per_instrument_window: Duration::from_secs(60),
        }
    }

    pub fn with_order_budget(mut self, max_requotes_per_order: u32) -> Self {
        self.max_requotes_per_order = max_requotes_per_order;
        self
    }

    pub fn with_instrument_budget(
        mut self,
        max_requotes_per_instrument: u32,
        per_window: Duration,
    ) -> Self {
        self.max_requotes_per_instrument = max_requotes_per_instrument;
        self.per_instrument_window = per_window;
        self
    }

    pub fn validate(&self) -> Result<()> {
        if self.tick <= Decimal::ZERO {
            return Err(Error::invalid(
                "a reprice policy needs a positive tick; ticks are its unit of staleness",
            ));
        }
        if self.max_drift_ticks == 0 {
            return Err(Error::invalid(
                "a drift threshold of zero ticks would requote on any movement at all, which is \
                 the chasing the budgets exist to prevent; declare at least one tick",
            ));
        }
        if !self.max_drift_bps_f64.is_finite() || self.max_drift_bps_f64 <= 0.0 {
            return Err(Error::invalid(
                "the basis-point drift threshold must be a positive finite number",
            ));
        }
        if self.max_requotes_per_order == 0 || self.max_requotes_per_instrument == 0 {
            return Err(Error::invalid(
                "a requote budget of zero disables repricing silently; if that is wanted, do not \
                 construct a repricer",
            ));
        }
        if self.per_instrument_window.as_nanos() <= 0 {
            return Err(Error::invalid(
                "the per-instrument budget needs a positive window to refill on",
            ));
        }
        Ok(())
    }
}

/// How far behind the touch a resting order sits.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drift {
    /// Price distance behind the touch. Negative means at or ahead of it.
    pub behind_by: Decimal,
    /// The same distance in ticks. A statistic.
    pub ticks_f64: f64,
    /// The same distance in basis points of the touch. A statistic.
    pub bps_f64: f64,
}

/// Which budget ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThrottleScope {
    Order,
    Instrument,
}

impl ThrottleScope {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Order => "order",
            Self::Instrument => "instrument",
        }
    }
}

/// Why an order was left alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldReason {
    /// Not a resting limit order: a peg follows the book at the venue, and a
    /// market, IOC or FOK order does not rest.
    NotRepriceable,
    /// The child is terminal; there is nothing at the venue to withdraw.
    Terminal,
    /// A cancel for this child is already in flight. A second instruction
    /// while the first is unresolved is how two replacements happen.
    CancelInFlight,
    /// Within the declared drift thresholds.
    Fresh,
}

impl HoldReason {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NotRepriceable => "not_repriceable",
            Self::Terminal => "terminal",
            Self::CancelInFlight => "cancel_in_flight",
            Self::Fresh => "fresh",
        }
    }
}

/// What the repricer decided about one resting child.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum RepriceDecision {
    /// Leave it resting, and why.
    Hold { reason: HoldReason, detail: String },
    /// Stale, but the budget is spent. The named failure: a repricer chasing
    /// a fast market inflicts its own throttling, so it stops by policy and
    /// says so rather than skipping silently.
    Throttled {
        scope: ThrottleScope,
        used: u32,
        budget: u32,
        detail: String,
    },
    /// Withdraw it. The caller sends a cancel to the venue; the replacement
    /// exists only after [`Repricer::on_cancel_acknowledged`].
    CancelAndReplace {
        client_id: String,
        drift: Drift,
        detail: String,
    },
}

/// A cancel this repricer has requested and not yet seen resolved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingReplace {
    pub client_id: String,
    pub parent: OrderId,
    pub instrument: String,
    pub requested_at: Timestamp,
    pub reason: String,
}

/// The repricer. Holds the policy, the budgets it has spent, and the cancels
/// in flight; decides, and constructs replacements — sends nothing.
#[derive(Clone, Debug)]
pub struct Repricer {
    policy: RepricePolicy,
    /// Requote instructions spent per parent order, for the order budget.
    spent_by_order: BTreeMap<String, u32>,
    /// Requote instructions spent per instrument in the current window.
    spent_by_instrument: BTreeMap<String, (Timestamp, u32)>,
    /// Cancels requested and unresolved, keyed by child client id.
    pending: BTreeMap<String, PendingReplace>,
}

impl Repricer {
    pub fn new(policy: RepricePolicy) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            policy,
            spent_by_order: BTreeMap::new(),
            spent_by_instrument: BTreeMap::new(),
            pending: BTreeMap::new(),
        })
    }

    pub fn policy(&self) -> &RepricePolicy {
        &self.policy
    }

    /// Cancels in flight, oldest client id first.
    pub fn pending(&self) -> Vec<&PendingReplace> {
        self.pending.values().collect()
    }

    /// Requote instructions a parent order has spent.
    pub fn spent_by(&self, parent: &OrderId) -> u32 {
        self.spent_by_order
            .get(parent.as_str())
            .copied()
            .unwrap_or(0)
    }

    /// How far behind the touch a resting limit child sits. Pure.
    ///
    /// `None` for a child that does not rest at a price of its own. Drift is
    /// measured *behind* the touch only: an order at or through the touch is
    /// ahead of its queue, and pulling it would spend priority to buy nothing.
    pub fn drift_of(&self, child: &ChildOrder, touch: Touch) -> Option<Drift> {
        let RoutedOrderType::Limit { price } = child.order_type else {
            return None;
        };
        // `child.side` names the side of the book the order consumes, so a
        // buy (consuming the ask) rests on the bid and falls behind when the
        // bid rises above it; a sell rests on the ask and falls behind when
        // the ask drops below it.
        let reference = touch.resting(child.side);
        let behind_by = match child.side {
            BookSide::Ask => reference - price,
            BookSide::Bid => price - reference,
        };
        let ticks_f64 = behind_by
            .checked_div(self.policy.tick)
            .map_or(0.0, Decimal::to_f64);
        let bps_f64 = if reference.is_positive() {
            behind_by.to_f64() / reference.to_f64() * 10_000.0
        } else {
            0.0
        };
        Some(Drift {
            behind_by,
            ticks_f64,
            bps_f64,
        })
    }

    /// Decide what to do about one resting child against the current touch.
    ///
    /// Deterministic: the same child, touch and timestamp against the same
    /// prior instruction history produce the same decision. A
    /// `CancelAndReplace` records the cancel as in flight and spends both
    /// budgets; everything else changes nothing.
    pub fn consider(&mut self, child: &ChildOrder, touch: Touch, at: Timestamp) -> RepriceDecision {
        if child.state.is_terminal() {
            return RepriceDecision::Hold {
                reason: HoldReason::Terminal,
                detail: format!(
                    "child {} is {}; there is nothing at the venue to withdraw",
                    child.client_id,
                    child.state.as_str()
                ),
            };
        }
        if self.pending.contains_key(&child.client_id) {
            return RepriceDecision::Hold {
                reason: HoldReason::CancelInFlight,
                detail: format!(
                    "a cancel for {} is already in flight; a second instruction while the first \
                     is unresolved is how two replacements happen",
                    child.client_id
                ),
            };
        }
        let Some(drift) = self.drift_of(child, touch) else {
            return RepriceDecision::Hold {
                reason: HoldReason::NotRepriceable,
                detail: format!(
                    "child {} is a {} order, which does not rest at a price this repricer owns",
                    child.client_id,
                    child.order_type.kind().as_str()
                ),
            };
        };

        let tick_threshold = Decimal::from_int(i64::from(self.policy.max_drift_ticks));
        let stale_by_ticks = drift.ticks_f64 >= tick_threshold.to_f64();
        let stale_by_bps = drift.bps_f64 >= self.policy.max_drift_bps_f64;
        if !stale_by_ticks && !stale_by_bps {
            return RepriceDecision::Hold {
                reason: HoldReason::Fresh,
                detail: format!(
                    "child {} rests {:.2} tick(s) ({:.2} bps) behind the touch, inside the \
                     declared thresholds of {} tick(s) and {} bps",
                    child.client_id,
                    drift.ticks_f64,
                    drift.bps_f64,
                    self.policy.max_drift_ticks,
                    self.policy.max_drift_bps_f64
                ),
            };
        }

        // Stale. Budgets are checked before anything is spent, order first —
        // a deterministic order of refusal, so the same history throttles the
        // same way.
        let order_key = child.parent.as_str().to_string();
        let order_spent = self.spent_by_order.get(&order_key).copied().unwrap_or(0);
        if order_spent >= self.policy.max_requotes_per_order {
            return RepriceDecision::Throttled {
                scope: ThrottleScope::Order,
                used: order_spent,
                budget: self.policy.max_requotes_per_order,
                detail: format!(
                    "order {} has spent its requote budget ({order_spent} of {}); repricing it \
                     again would be chasing the market — every requote pays a cancel round trip \
                     to rejoin the back of a queue that is moving away, self-inflicted \
                     throttling wearing diligence's name. The order rests where it is",
                    order_key, self.policy.max_requotes_per_order
                ),
            };
        }

        let instrument_key = child.object_id.as_str().to_string();
        let window_start = at.floor_to(self.policy.per_instrument_window);
        let instrument_spent = match self.spent_by_instrument.get(&instrument_key) {
            Some((window, count)) if *window == window_start => *count,
            _ => 0,
        };
        if instrument_spent >= self.policy.max_requotes_per_instrument {
            return RepriceDecision::Throttled {
                scope: ThrottleScope::Instrument,
                used: instrument_spent,
                budget: self.policy.max_requotes_per_instrument,
                detail: format!(
                    "instrument {instrument_key} has spent its requote budget for this window \
                     ({instrument_spent} of {}); a market moving faster than the budget refills \
                     is one this repricer must not chase",
                    self.policy.max_requotes_per_instrument
                ),
            };
        }

        // Spend, record the in-flight cancel, and instruct.
        self.spent_by_order
            .insert(order_key, order_spent.saturating_add(1));
        self.spent_by_instrument.insert(
            instrument_key.clone(),
            (window_start, instrument_spent.saturating_add(1)),
        );
        let reason = format!(
            "resting {:.2} tick(s) ({:.2} bps) behind the touch, past the declared threshold",
            drift.ticks_f64, drift.bps_f64
        );
        self.pending.insert(
            child.client_id.clone(),
            PendingReplace {
                client_id: child.client_id.clone(),
                parent: child.parent.clone(),
                instrument: instrument_key,
                requested_at: at,
                reason: reason.clone(),
            },
        );
        RepriceDecision::CancelAndReplace {
            client_id: child.client_id.clone(),
            drift,
            detail: format!(
                "cancel {} and replace the remainder at the touch once — and only once — the \
                 cancel is acknowledged: {reason}",
                child.client_id
            ),
        }
    }

    /// The venue acknowledged the cancel: mint the replacement.
    ///
    /// Call this only after the cancel (and any fills the venue reported with
    /// it) has been applied to the parent. The replacement:
    ///
    /// * carries the child's **remainder** and nothing more — the booked fill
    ///   is never re-sent;
    /// * takes a **fresh client id**, so downstream it carries a fresh
    ///   idempotency key instead of being deduped away as a retry of the
    ///   order that was just cancelled;
    /// * rests at the current touch, never through it;
    /// * is already attached to the parent, whose conservation arithmetic
    ///   has accepted it.
    ///
    /// Refuses a child whose cancel was never requested here, and — the guard
    /// this module exists for — a child the venue has not finished with: a
    /// replacement sent while the original may still fill is two live orders
    /// for one intention, and if the market trades through both prices, both
    /// fill.
    ///
    /// `Ok(None)` means the child filled completely while the cancel was in
    /// flight: there is no remainder, so there is no replacement.
    pub fn on_cancel_acknowledged(
        &mut self,
        parent: &mut ParentOrder,
        client_id: &str,
        touch: Touch,
    ) -> Result<Option<ChildOrder>> {
        if !self.pending.contains_key(client_id) {
            return Err(Error::invalid(format!(
                "no cancel was requested for child {client_id}; a replacement without its cancel \
                 is the race this repricer exists to prevent"
            )));
        }
        let Some(child) = parent.child(client_id) else {
            return Err(Error::not_found(format!(
                "child {client_id} is not attached to parent {}",
                parent.order_id.as_str()
            )));
        };
        if !child.state.is_terminal() {
            return Err(Error::guard(format!(
                "the venue has not acknowledged the cancel of {client_id} (it is {}); a replace \
                 racing its own cancel is two live orders for one intention, and if the market \
                 trades through both prices, both fill. The replacement waits",
                child.state.as_str()
            )));
        }

        // The venue has said its final word; the pending entry is resolved
        // whatever happens next.
        self.pending.remove(client_id);

        let remainder = child.remaining();
        if remainder <= Decimal::ZERO {
            // Filled (or over-accounted, which the child itself refuses)
            // while the cancel was in flight: the intention is complete and
            // the booked fill is never re-sent.
            return Ok(None);
        }

        let side = child.side;
        let venue = child.venue.clone();
        let object_id = child.object_id.clone();

        let price = touch.resting(side);
        if price <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "the touch offers no usable resting price for {client_id}'s replacement; an \
                 order at a non-positive price is not a reprice, it is a mistake"
            )));
        }
        let replacement_id = parent.next_client_id();
        let replacement = ChildOrder::new(
            replacement_id,
            parent.order_id.clone(),
            venue,
            object_id,
            side,
            remainder,
            RoutedOrderType::Limit { price },
        )?;
        // The parent's own conservation arithmetic has the last word: it
        // refuses a child whose quantity is not free to assign, which is what
        // makes a double-send structurally impossible rather than merely
        // avoided.
        parent.attach(replacement.clone())?;
        Ok(Some(replacement))
    }

    /// The cancel failed or was refused: resolve the in-flight entry.
    ///
    /// The budget stays spent — chasing is measured in instructions sent, not
    /// in instructions that worked. Returns the entry, or `None` when no
    /// cancel was pending for the id.
    pub fn abandon(&mut self, client_id: &str) -> Option<PendingReplace> {
        self.pending.remove(client_id)
    }
}
