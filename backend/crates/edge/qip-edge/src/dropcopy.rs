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
use std::collections::BTreeMap;

/// A fill as the venue reported it, on the independent channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DropCopyFill {
    pub order_id: String,
    pub venue: VenueId,
    pub quantity: Decimal,
    pub price: Decimal,
    pub at: Timestamp,
}

/// What the cell believes about one order.
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

/// Compares the cell's fills against the venue's own account of them.
#[derive(Debug, Default)]
pub struct DropCopyReconciler {
    venue_fills: BTreeMap<String, DropCopyFill>,
    checked: u64,
}

impl DropCopyReconciler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Absorb a fill from the independent channel.
    ///
    /// Repeated deliveries of the same fill are the norm on a drop copy; the
    /// later one replaces the earlier rather than accumulating, because two
    /// copies of one fill would look exactly like a doubled position.
    pub fn observe(&mut self, fill: DropCopyFill) {
        self.venue_fills.insert(fill.order_id.clone(), fill);
    }

    pub fn observed(&self) -> usize {
        self.venue_fills.len()
    }

    pub const fn checked(&self) -> u64 {
        self.checked
    }

    /// Compare the cell's record against everything the venue has reported.
    ///
    /// Both directions. A cell that only checks its own fills against the
    /// venue's misses the case that matters most — a fill the cell never knew
    /// about, which is an unhedged position nobody is watching.
    pub fn reconcile(&mut self, cell_fills: &[CellFill]) -> Vec<Discrepancy> {
        self.checked += 1;
        let mut found = Vec::new();
        let mut seen: BTreeMap<&str, &CellFill> = BTreeMap::new();
        for fill in cell_fills {
            seen.insert(fill.order_id.as_str(), fill);
        }

        for (order_id, venue_fill) in &self.venue_fills {
            match seen.get(order_id.as_str()) {
                Some(cell_fill) => {
                    if cell_fill.quantity != venue_fill.quantity {
                        found.push(Discrepancy::QuantityDiffers {
                            order_id: order_id.clone(),
                            cell: cell_fill.quantity,
                            venue: venue_fill.quantity,
                        });
                    }
                    if cell_fill.price != venue_fill.price {
                        found.push(Discrepancy::PriceDiffers {
                            order_id: order_id.clone(),
                            cell: cell_fill.price,
                            venue: venue_fill.price,
                        });
                    }
                }
                None => found.push(Discrepancy::UnknownToCell {
                    order_id: order_id.clone(),
                    venue: venue_fill.venue.clone(),
                    quantity: venue_fill.quantity,
                }),
            }
        }

        for fill in cell_fills {
            if !self.venue_fills.contains_key(&fill.order_id) {
                found.push(Discrepancy::UnknownToVenue {
                    order_id: fill.order_id.clone(),
                    venue: fill.venue.clone(),
                    quantity: fill.quantity,
                });
            }
        }

        found
    }
}
