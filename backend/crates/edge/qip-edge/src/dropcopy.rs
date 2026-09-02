//! Independent trade truth.
//!
//! A cell's own record of what it traded comes from the same code path that
//! did the trading, so it agrees with itself by construction and proves
//! nothing. A drop copy is the venue's account of the same fills, delivered on
//! a separate channel — and the only thing that can catch the cell's record
//! being wrong.
//!
//! A mismatch is not a warning. Positions the platform believes it holds and
//! positions it actually holds having diverged means every number downstream
//! is computed against a fiction, so a break halts the cell and stays halted
//! until a person has looked.

use qip_contracts::venue::VenueId;
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// A fill as the venue reported it, on the independent channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DropCopyFill {
    pub order_id: String,
    pub venue: VenueId,
    pub quantity: Decimal,
    pub price: Decimal,
    pub at: Timestamp,
}

/// One fill the cell has confirmed from its own order-entry channel.
///
/// Several may name one order when the venue filled it in parts; the
/// reconciler sums them. Never an order merely sent — see
/// [`DropCopyReconciler::reconcile`] for why that distinction is the whole
/// point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellFill {
    pub order_id: String,
    pub venue: VenueId,
    pub quantity: Decimal,
    pub price: Decimal,
}

/// A disagreement between the two records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Discrepancy {
    /// The venue says it filled something the cell has no record of.
    UnknownToCell {
        order_id: String,
        venue: VenueId,
        quantity: Decimal,
    },
    /// The cell believes it filled something the venue did not report.
    UnknownToVenue {
        order_id: String,
        venue: VenueId,
        quantity: Decimal,
    },
    /// Both know the order and disagree about size.
    QuantityDiffers {
        order_id: String,
        cell: Decimal,
        venue: Decimal,
    },
    /// Both know the order and disagree about price.
    PriceDiffers {
        order_id: String,
        cell: Decimal,
        venue: Decimal,
    },
}

impl Discrepancy {
    pub fn describe(&self) -> String {
        match self {
            Self::UnknownToCell {
                order_id,
                venue,
                quantity,
            } => format!(
                "venue {} reports a fill of {quantity} on order {order_id} the cell has no record of",
                venue.as_str()
            ),
            Self::UnknownToVenue {
                order_id,
                venue,
                quantity,
            } => format!(
                "the cell believes it filled {quantity} on order {order_id} at {} and the venue did not report it",
                venue.as_str()
            ),
            Self::QuantityDiffers {
                order_id,
                cell,
                venue,
            } => format!("order {order_id}: the cell holds {cell}, the venue reports {venue}"),
            Self::PriceDiffers {
                order_id,
                cell,
                venue,
            } => format!("order {order_id}: the cell priced {cell}, the venue reports {venue}"),
        }
    }
}

/// How many fills one order may carry on the venue's channel.
///
/// Past this the reconciler stops storing fills for the order and the venue's
/// total understates what it reported, which the next comparison reads as a
/// quantity disagreement and the cell halts on. A venue that splits one order
/// into more executions than this is a venue whose account the cell cannot
/// hold in full, and halting is the only answer that does not invent one.
pub const MAX_FILLS_PER_ORDER: usize = 256;

/// How many settled orders the reconciler remembers, oldest evicted.
///
/// A settled order's fills are kept so a redelivery of one of them — the
/// norm on a drop copy — is recognised rather than read as a fresh fill the
/// cell knows nothing about. Past the bound the oldest is forgotten, and a
/// redelivery for a forgotten order is a fill unknown to the cell: a break,
/// and a halt. That is a false stop, and a false stop is the cheap error.
pub const MAX_SETTLED_ORDERS: usize = 4_096;

/// What both channels say about one order, summed.
struct Aggregate {
    venue: VenueId,
    quantity: Decimal,
    notional: Decimal,
}

impl Aggregate {
    fn new(venue: VenueId) -> Self {
        Self {
            venue,
            quantity: Decimal::ZERO,
            notional: Decimal::ZERO,
        }
    }

    fn add(&mut self, quantity: Decimal, price: Decimal) {
        self.quantity += quantity;
        self.notional += quantity * price;
    }

    /// The volume-weighted price, for a message; the comparison itself is on
    /// the notional, which is exact.
    fn average(&self) -> Decimal {
        self.notional
            .checked_div(self.quantity)
            .unwrap_or(self.notional)
    }
}

/// Compares the cell's fills against the venue's own account of them.
///
/// Both records are held per order as the fills each channel reported, and
/// compared as sums: a venue that fills one order in three executions and a
/// cell that confirmed them as two are describing the same trade, and a
/// reconciler that compared fill against fill would halt on the venue's
/// choice of how to slice it. What must agree is what traded and what it
/// cost, so the quantity is compared exactly and then the notional.
#[derive(Debug, Default)]
pub struct DropCopyReconciler {
    /// Fills the venue has reported, by order, distinct deliveries only.
    venue_fills: BTreeMap<String, Vec<DropCopyFill>>,
    /// Orders both sides have agreed on and the cell has finished with, kept
    /// so a redelivery is recognised. Bounded by [`MAX_SETTLED_ORDERS`].
    settled: BTreeMap<String, Vec<DropCopyFill>>,
    settled_order: VecDeque<String>,
    late_duplicates: u64,
    overflowed: u64,
    checked: u64,
}

impl DropCopyReconciler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Absorb a fill from the independent channel.
    ///
    /// Repeated deliveries of the same fill are the norm on a drop copy; a
    /// delivery identical to one already held is dropped rather than added,
    /// because two copies of one fill would look exactly like a doubled
    /// position. A delivery that differs — a second partial on the same
    /// order — is a second fill and accumulates, because a venue that fills
    /// an order in parts reports each part, and replacing the first with the
    /// second would make a filled order look half filled.
    ///
    /// The one thing this cannot tell apart is two genuine executions on one
    /// order at the same instant, price and size, which arrive as identical
    /// records. They are held as one, the venue's total then understates the
    /// cell's, and the comparison halts the cell rather than doubling
    /// anything. The channel carries no execution id to do better with.
    pub fn observe(&mut self, fill: DropCopyFill) {
        if let Some(fills) = self.settled.get(&fill.order_id)
            && fills.contains(&fill)
        {
            self.late_duplicates = self.late_duplicates.saturating_add(1);
            return;
        }
        // A settled order the venue now says more about is not settled: the
        // fill is stored like any other and, with nothing on the cell's side
        // to match it, reads as unknown to the cell on the next comparison.
        let fills = self.venue_fills.entry(fill.order_id.clone()).or_default();
        if fills.contains(&fill) {
            return;
        }
        if fills.len() >= MAX_FILLS_PER_ORDER {
            self.overflowed = self.overflowed.saturating_add(1);
            return;
        }
        fills.push(fill);
    }

    /// Orders the venue has reported fills for and the cell has not settled.
    pub fn observed(&self) -> usize {
        self.venue_fills.len()
    }

    pub const fn checked(&self) -> u64 {
        self.checked
    }

    /// Redeliveries received for orders already settled.
    pub const fn late_duplicates(&self) -> u64 {
        self.late_duplicates
    }

    /// Fills not stored because their order was past [`MAX_FILLS_PER_ORDER`].
    pub const fn overflowed(&self) -> u64 {
        self.overflowed
    }

    /// What the venue has reported traded on one order, summed.
    pub fn venue_quantity(&self, order_id: &str) -> Option<Decimal> {
        self.venue_fills
            .get(order_id)
            .map(|fills| fills.iter().map(|fill| fill.quantity).sum())
    }

    /// Forget an order both sides agree on and the cell has finished with.
    ///
    /// Called by the cell after a clean comparison, for orders it has closed.
    /// The fills move to the settled memory rather than vanishing, so the
    /// channel's habit of redelivering is met with recognition rather than
    /// a break. See [`MAX_SETTLED_ORDERS`] for what happens past the bound.
    pub fn retire(&mut self, order_id: &str) {
        let Some(fills) = self.venue_fills.remove(order_id) else {
            return;
        };
        while self.settled.len() >= MAX_SETTLED_ORDERS {
            match self.settled_order.pop_front() {
                Some(oldest) => {
                    self.settled.remove(&oldest);
                }
                None => break,
            }
        }
        self.settled.insert(order_id.to_string(), fills);
        self.settled_order.push_back(order_id.to_string());
    }

    /// Compare the cell's record against everything the venue has reported.
    ///
    /// Both directions. A cell that only checks its own fills against the
    /// venue's misses the case that matters most — a fill the cell never knew
    /// about, which is an unhedged position nobody is watching.
    ///
    /// `cell_fills` are the fills the cell has *confirmed* from its own
    /// order-entry channel, one entry per fill and several per order when the
    /// venue filled it in parts. An order the cell sent and the venue has not
    /// filled appears in neither list and is not a disagreement: a resting
    /// order is not a position, and until this reconciler compared confirmed
    /// fills rather than sent orders, every order that rested for one pass
    /// halted the cell.
    pub fn reconcile(&mut self, cell_fills: &[CellFill]) -> Vec<Discrepancy> {
        self.checked += 1;
        let mut found = Vec::new();

        let mut cell: BTreeMap<&str, Aggregate> = BTreeMap::new();
        for fill in cell_fills {
            cell.entry(fill.order_id.as_str())
                .or_insert_with(|| Aggregate::new(fill.venue.clone()))
                .add(fill.quantity, fill.price);
        }

        for (order_id, venue_fills) in &self.venue_fills {
            let Some(first) = venue_fills.first() else {
                continue;
            };
            let mut venue = Aggregate::new(first.venue.clone());
            for fill in venue_fills {
                venue.add(fill.quantity, fill.price);
            }
            match cell.get(order_id.as_str()) {
                Some(cell_side) => {
                    if cell_side.quantity != venue.quantity {
                        found.push(Discrepancy::QuantityDiffers {
                            order_id: order_id.clone(),
                            cell: cell_side.quantity,
                            venue: venue.quantity,
                        });
                    } else if cell_side.notional != venue.notional {
                        found.push(Discrepancy::PriceDiffers {
                            order_id: order_id.clone(),
                            cell: cell_side.average(),
                            venue: venue.average(),
                        });
                    }
                }
                None => found.push(Discrepancy::UnknownToCell {
                    order_id: order_id.clone(),
                    venue: venue.venue,
                    quantity: venue.quantity,
                }),
            }
        }

        for (order_id, cell_side) in &cell {
            if !self.venue_fills.contains_key(*order_id) {
                found.push(Discrepancy::UnknownToVenue {
                    order_id: (*order_id).to_string(),
                    venue: cell_side.venue.clone(),
                    quantity: cell_side.quantity,
                });
            }
        }

        found
    }
}
