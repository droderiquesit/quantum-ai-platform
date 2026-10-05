//! Which tradable conversions the graph can see, and which it cannot.
//!
//! The graph is built from what somebody chose to put in it. Nothing about
//! that tells a reader what was left out, and a conversion with no edge is
//! invisible in exactly the way that matters: the search cannot propose a
//! cycle through it, the scan reports no rejection for it, and a desk that
//! is blind to half a market reads the same as a desk watching a quiet one.
//!
//! So the question is asked the other way round. A [`TradableRegistry`] says
//! what *can* be traded, from a source that is not the graph — a cell's own
//! books, in the deployed node. [`ArbitrageGraph::coverage`] then sorts every
//! entry into one of two lists: it has a directed edge, or it is a gap. There
//! is no third outcome and nothing is skipped, which is the whole property
//! (MESH-023).
//!
//! Directed, because the two directions of one market are different
//! conversions with different costs: consuming the bids sells the base and
//! consuming the offers buys it. An edge one way says nothing about the
//! other, and a registry entry is one direction for that reason.
//!
//! A gap is reported and never repaired here. Whether a conversion *may* be
//! traded is the centre's decision, shipped as a signed whitelist; adding an
//! edge because a book exists would trade something nobody permitted.

use crate::graph::{ArbitrageGraph, EdgeKind};
use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::ObjectId;
use qip_core::error::{Error, Result};
use std::collections::BTreeSet;

/// One conversion a registry marks tradable, in one direction.
///
/// There is no synthetic variant. A synthetic is assembled from components
/// that are themselves trades, so a registry of what can be traded names the
/// components; which baskets are worth assembling is a modelling choice and
/// not a fact a registry holds.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tradable {
    /// One side of one market's book at one venue. Consuming `Bid` sells the
    /// market's base; consuming `Ask` buys it.
    Trade {
        venue: VenueId,
        market: ObjectId,
        side: BookSide,
    },
    /// One instrument moved from one venue to another, that way round.
    Transfer {
        object: ObjectId,
        from: VenueId,
        to: VenueId,
    },
}

impl Tradable {
    /// A stable label for logs and gap reports.
    pub fn label(&self) -> String {
        match self {
            Self::Trade {
                venue,
                market,
                side,
            } => format!("{}@{}/{}", market.as_str(), venue.as_str(), side.as_str()),
            Self::Transfer { object, from, to } => {
                format!("{}:{}>{}", object.as_str(), from.as_str(), to.as_str())
            }
        }
    }
}

/// What can be traded, according to something other than the graph.
///
/// A set, ordered, so two registries built from the same facts in a
/// different order are the same registry and report the same gaps in the
/// same order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TradableRegistry {
    conversions: BTreeSet<Tradable>,
}

impl TradableRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A market whose book can be traded on both sides: two conversions.
    #[must_use]
    pub fn with_market(self, venue: VenueId, market: ObjectId) -> Self {
        self.with_side(venue.clone(), market.clone(), BookSide::Bid)
            .with_side(venue, market, BookSide::Ask)
    }

    /// One side of one market: one conversion.
    #[must_use]
    pub fn with_side(mut self, venue: VenueId, market: ObjectId, side: BookSide) -> Self {
        self.conversions.insert(Tradable::Trade {
            venue,
            market,
            side,
        });
        self
    }

    /// One instrument movable from one venue to another, in that direction
    /// only. Refused when the two venues are the same: that is not a
    /// conversion, and a registry entry no edge could ever satisfy would be
    /// reported as a gap for ever.
    pub fn with_transfer(mut self, object: ObjectId, from: VenueId, to: VenueId) -> Result<Self> {
        if from == to {
            return Err(Error::invalid(format!(
                "a transfer of {} from {} to itself is not a conversion; name two venues",
                object.as_str(),
                from.as_str()
            )));
        }
        self.conversions
            .insert(Tradable::Transfer { object, from, to });
        Ok(self)
    }

    pub fn len(&self) -> usize {
        self.conversions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.conversions.is_empty()
    }

    /// Every conversion, in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = &Tradable> {
        self.conversions.iter()
    }
}

/// A registry sorted against a graph: every entry is in exactly one list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Tradable conversions the graph holds a directed edge for.
    pub covered: Vec<Tradable>,
    /// Tradable conversions with no edge. No cycle through one can be found.
    pub gaps: Vec<Tradable>,
}

impl Coverage {
    /// Whether every tradable conversion has its edge.
    pub fn is_complete(&self) -> bool {
        self.gaps.is_empty()
    }

    /// The gaps as labels, in the registry's order.
    pub fn gap_labels(&self) -> Vec<String> {
        self.gaps.iter().map(Tradable::label).collect()
    }
}

impl ArbitrageGraph {
    /// Sort every conversion `registry` marks tradable into covered or gap.
    ///
    /// A trade is covered by a trade edge on the same market, at the same
    /// venue, consuming the same side — not by the edge for the other side.
    /// A transfer is covered by a transfer edge of the same instrument
    /// between the same venues the same way round. Whether an edge's venue
    /// is currently open is not asked: a halted venue's conversion is
    /// represented and temporarily unusable, which the search handles, and
    /// reporting it as missing would send an operator to the whitelist for a
    /// problem that is at the venue.
    pub fn coverage(&self, registry: &TradableRegistry) -> Coverage {
        let mut coverage = Coverage::default();
        for tradable in registry.iter() {
            let represented = self
                .edges()
                .iter()
                .any(|edge| match (tradable, &edge.kind) {
                    (
                        Tradable::Trade {
                            venue,
                            market,
                            side,
                        },
                        EdgeKind::Trade {
                            market: edge_market,
                            side: edge_side,
                        },
                    ) => edge.from.venue == *venue && edge_market == market && edge_side == side,
                    (Tradable::Transfer { object, from, to }, EdgeKind::Transfer) => {
                        edge.from.object == *object
                            && edge.from.venue == *from
                            && edge.to.venue == *to
                    }
                    _ => false,
                });
            if represented {
                coverage.covered.push(tradable.clone());
            } else {
                coverage.gaps.push(tradable.clone());
            }
        }
        coverage
    }
}
