//! Parent orders, the children they are split into, and the arithmetic that
//! keeps the two in step.
//!
//! A split is only safe if it can be undone. Every share of the parent is in
//! exactly one of four places at any moment — filled, working at a venue,
//! handed back by a child that will not fill it, or never assigned — and the
//! four add up to the parent. That identity is the whole point of this module:
//! a child that gets rejected releases its quantity back into
//! [`ParentOrder::outstanding`] rather than taking it with it, so a failure
//! shrinks the parent's progress instead of orphaning it.
//!
//! Everything is exact. A parent split eight ways and reassembled has to come
//! back to the quantity it started with, to the share, because the alternative
//! is a position that appears from nowhere on one order in ten thousand.

use crate::ordertype::RoutedOrderType;
use crate::router::RoutingDecision;
use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::ids::OrderId;
use qip_core::{Decimal, ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a child is in its life.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChildState {
    /// Created, not yet sent.
    Pending,
    /// At the venue, nothing filled.
    Working,
    /// At the venue, some filled.
    PartiallyFilled,
    /// Done.
    Filled,
    /// Withdrawn. Whatever was unfilled goes back to the parent.
    Cancelled { reason: String },
    /// Refused by the venue. Same, and the venue's health hears about it.
    Rejected { reason: String },
}

impl ChildState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Working => "working",
            Self::PartiallyFilled => "partially_filled",
            Self::Filled => "filled",
            Self::Cancelled { .. } => "cancelled",
            Self::Rejected { .. } => "rejected",
        }
    }

    /// Whether nothing more will happen to this child.
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Filled | Self::Cancelled { .. } | Self::Rejected { .. }
        )
    }

    /// Whether the child failed rather than finished.
    pub const fn is_failure(&self) -> bool {
        matches!(self, Self::Cancelled { .. } | Self::Rejected { .. })
    }
}

/// One venue's slice of a parent order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChildOrder {
    pub client_id: String,
    pub parent: OrderId,
    pub venue: VenueId,
    pub object_id: ObjectId,
    pub side: BookSide,
    pub quantity: Decimal,
    pub order_type: RoutedOrderType,
    pub state: ChildState,
    filled: Decimal,
    /// Traded value, kept so the average price is exact rather than a running
    /// mean that drifts with every fill.
    consideration: Decimal,
}

impl ChildOrder {
    pub fn new(
        client_id: impl Into<String>,
        parent: OrderId,
        venue: VenueId,
        object_id: ObjectId,
        side: BookSide,
        quantity: Decimal,
        order_type: RoutedOrderType,
    ) -> Result<Self> {
        if quantity <= Decimal::ZERO {
            return Err(Error::invalid("a child order needs a positive quantity"));
        }
        Ok(Self {
            client_id: client_id.into(),
            parent,
            venue,
            object_id,
            side,
            quantity,
            order_type,
            state: ChildState::Pending,
            filled: Decimal::ZERO,
            consideration: Decimal::ZERO,
        })
    }

    pub fn filled(&self) -> Decimal {
        self.filled
    }

    pub fn remaining(&self) -> Decimal {
        self.quantity - self.filled
    }

    pub fn average_price(&self) -> Option<Decimal> {
        if self.filled <= Decimal::ZERO {
            return None;
        }
        self.consideration.checked_div(self.filled)
    }

    /// Quantity this child has given back to the parent.
    ///
    /// Non-zero only once it is terminal: a working child has not released
    /// anything, it simply has not finished.
    pub fn released(&self) -> Decimal {
        if self.state.is_terminal() {
            self.remaining()
        } else {
            Decimal::ZERO
        }
    }

    pub fn mark_working(&mut self) -> Result<()> {
        if self.state.is_terminal() {
            return Err(Error::invalid(format!(
                "child {} is {} and cannot start working",
                self.client_id,
                self.state.as_str()
            )));
        }
        if matches!(self.state, ChildState::Pending) {
            self.state = ChildState::Working;
        }
        Ok(())
    }

    /// Apply a fill, refusing one the child cannot account for.
    ///
    /// An over-fill is refused rather than absorbed. A venue that reports more
    /// than it was asked for has put the platform's position out of step with
    /// reality, and quietly widening the child to fit hides the divergence at
    /// the exact moment it is cheapest to catch.
    pub fn apply_fill(&mut self, quantity: Decimal, price: Decimal) -> Result<()> {
        if quantity <= Decimal::ZERO {
            return Err(Error::invalid("a fill needs a positive quantity"));
        }
        if self.state.is_terminal() {
            return Err(Error::invalid(format!(
                "child {} is {} and cannot be filled",
                self.client_id,
                self.state.as_str()
            )));
        }
        if self.filled + quantity > self.quantity {
            return Err(Error::invalid(format!(
                "child {} was asked for {} and has {} filled; a further {quantity} would over-fill it",
                self.client_id, self.quantity, self.filled
            )));
        }
        self.filled += quantity;
        self.consideration += quantity
            .checked_mul(price)
            .ok_or_else(|| Error::numeric("a fill's consideration overflowed"))?;
        self.state = if self.filled >= self.quantity {
            ChildState::Filled
        } else {
            ChildState::PartiallyFilled
        };
        Ok(())
    }

    pub fn reject(&mut self, reason: impl Into<String>) -> Result<()> {
        self.finish(ChildState::Rejected {
            reason: reason.into(),
        })
    }

    pub fn cancel(&mut self, reason: impl Into<String>) -> Result<()> {
        self.finish(ChildState::Cancelled {
            reason: reason.into(),
        })
    }

    fn finish(&mut self, state: ChildState) -> Result<()> {
        if self.state.is_terminal() {
            return Err(Error::invalid(format!(
                "child {} is already {}",
                self.client_id,
                self.state.as_str()
            )));
        }
        self.state = state;
        Ok(())
    }
}

/// An order that was split, and the accounting that puts it back together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParentOrder {
    pub order_id: OrderId,
    pub object_id: ObjectId,
    pub side: BookSide,
    pub quantity: Decimal,
    children: BTreeMap<String, ChildOrder>,
    sequence: u64,
}

impl ParentOrder {
    pub fn new(
        order_id: OrderId,
        object_id: ObjectId,
        side: BookSide,
        quantity: Decimal,
    ) -> Result<Self> {
        if quantity <= Decimal::ZERO {
            return Err(Error::invalid("a parent order needs a positive quantity"));
        }
        Ok(Self {
            order_id,
            object_id,
            side,
            quantity,
            children: BTreeMap::new(),
            sequence: 0,
        })
    }

    pub fn children(&self) -> impl Iterator<Item = &ChildOrder> {
        self.children.values()
    }

    pub fn child(&self, client_id: &str) -> Option<&ChildOrder> {
        self.children.get(client_id)
    }

    pub fn child_mut(&mut self, client_id: &str) -> Option<&mut ChildOrder> {
        self.children.get_mut(client_id)
    }

    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// A client id nobody else will mint. Deterministic, so a replay names the
    /// same child the same thing.
    pub fn next_client_id(&mut self) -> String {
        self.sequence += 1;
        format!("{}-c{}", self.order_id.as_str(), self.sequence)
    }

    /// Attach a child, refusing one that would oversubscribe the parent.
    ///
    /// The quantity checked against is what is free to assign, not the
    /// parent's total and not everything still outstanding: a share already
    /// working at a venue is spoken for. Quantity released by a *failed* child
    /// becomes free again, which is what makes a re-route possible without
    /// inflating the order.
    pub fn attach(&mut self, child: ChildOrder) -> Result<()> {
        if child.parent != self.order_id {
            return Err(Error::invalid(format!(
                "child {} belongs to {}, not {}",
                child.client_id,
                child.parent.as_str(),
                self.order_id.as_str()
            )));
        }
        if self.children.contains_key(&child.client_id) {
            return Err(Error::invalid(format!(
                "child {} is already attached",
                child.client_id
            )));
        }
        if child.quantity > self.available_to_assign() {
            return Err(Error::invalid(format!(
                "child {} asks for {} but only {} of the parent is free to assign",
                child.client_id,
                child.quantity,
                self.available_to_assign()
            )));
        }
        self.children.insert(child.client_id.clone(), child);
        Ok(())
    }

    /// Turn a routing decision into children.
    ///
    /// Returns the client ids in slice order. Refuses a decision that does not
    /// balance rather than splitting from it, because a split built on a broken
    /// total is a position discrepancy waiting to be found by someone else.
    pub fn split(&mut self, decision: &RoutingDecision) -> Result<Vec<String>> {
        decision.validate()?;
        if decision.parent != self.order_id {
            return Err(Error::invalid(format!(
                "the decision is for {}, not {}",
                decision.parent.as_str(),
                self.order_id.as_str()
            )));
        }
        let mut ids = Vec::with_capacity(decision.slices.len());
        for slice in &decision.slices {
            let client_id = self.next_client_id();
            let child = ChildOrder::new(
                client_id.clone(),
                self.order_id.clone(),
                slice.venue.clone(),
                self.object_id.clone(),
                self.side,
                slice.quantity,
                slice.order_type,
            )?;
            self.attach(child)?;
            ids.push(client_id);
        }
        Ok(ids)
    }

    /// Quantity handed to a child, whatever became of it.
    pub fn assigned(&self) -> Decimal {
        self.children
            .values()
            .fold(Decimal::ZERO, |sum, child| sum + child.quantity)
    }

    pub fn filled(&self) -> Decimal {
        self.children
            .values()
            .fold(Decimal::ZERO, |sum, child| sum + child.filled())
    }

    /// Quantity still live at a venue.
    pub fn working(&self) -> Decimal {
        self.children
            .values()
            .filter(|child| !child.state.is_terminal())
            .fold(Decimal::ZERO, |sum, child| sum + child.remaining())
    }

    /// Quantity a child gave back by failing, which nothing has picked up.
    ///
    /// The number that answers "what did that reject actually cost us". A
    /// non-zero value here with no working children is an order that has
    /// stopped without finishing, and it is visible rather than inferred.
    pub fn orphaned(&self) -> Decimal {
        self.children
            .values()
            .filter(|child| child.state.is_failure())
            .fold(Decimal::ZERO, |sum, child| sum + child.remaining())
    }

    /// Quantity never handed to any child.
    pub fn unassigned(&self) -> Decimal {
        self.quantity - self.assigned()
    }

    /// Everything not yet filled: unassigned, orphaned, or still working.
    pub fn outstanding(&self) -> Decimal {
        self.unassigned() + self.orphaned() + self.working()
    }

    /// Quantity a new child may be given.
    ///
    /// Outstanding less what is already live at a venue. The distinction is the
    /// whole of the double-send bug: a share working somewhere is outstanding,
    /// and sending it again would put two orders in the market for one
    /// intention.
    pub fn available_to_assign(&self) -> Decimal {
        self.unassigned() + self.orphaned()
    }

    pub fn average_price(&self) -> Option<Decimal> {
        let filled = self.filled();
        if filled <= Decimal::ZERO {
            return None;
        }
        let consideration = self
            .children
            .values()
            .fold(Decimal::ZERO, |sum, child| sum + child.consideration);
        consideration.checked_div(filled)
    }

    pub fn is_complete(&self) -> bool {
        self.filled() >= self.quantity
    }

    /// Whether every share of the parent is somewhere.
    pub fn accounts_for_every_share(&self) -> bool {
        self.filled() + self.outstanding() == self.quantity
    }

    /// The same identity as an error, for a caller that must stop without it.
    pub fn reconcile(&self) -> Result<()> {
        if !self.accounts_for_every_share() {
            return Err(Error::invalid(format!(
                "parent {} has {} filled and {} outstanding against a quantity of {}",
                self.order_id.as_str(),
                self.filled(),
                self.outstanding(),
                self.quantity
            )));
        }
        Ok(())
    }

    /// Venues that failed this parent, so a re-route does not go back to them.
    pub fn failed_venues(&self) -> Vec<&VenueId> {
        let mut venues: Vec<&VenueId> = Vec::new();
        for child in self.children.values() {
            if child.state.is_failure() && !venues.contains(&&child.venue) {
                venues.push(&child.venue);
            }
        }
        venues
    }
}
