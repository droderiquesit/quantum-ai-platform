//! The two joins the constituent crates deliberately left to their composer.
//!
//! `qip-strategy` does not depend on `qip-feature-dag`: the compiler needs the
//! feature *vocabulary*, not the DAG machinery, and an edge between them would
//! have made every strategy recompilation wait on the graph. The cost is that
//! each side has its own value-kind tag, and something has to map them. That
//! something is here, and [`value_type`] is exhaustive on both sides so adding
//! a variant to either breaks this file rather than silently defaulting.
//!
//! `qip-arbitrage` defined [`LiquiditySource`] rather than depending on
//! `qip-orderbook`, so the path pricer could be written and tested before the
//! book existed. [`CellLiquidity`] is where the real book is finally supplied.

use qip_arbitrage::liquidity::LiquiditySource;
use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_feature_dag::definition::ValueKind;
use qip_orderbook::venue::VenueState;
use qip_orderbook::view::BookView;
use qip_strategy::ir::Type;
use std::collections::BTreeMap;

/// Translate the DAG's value kind into the compiler's type.
///
/// Total in both directions and deliberately not a `From` impl on either
/// crate's type — neither crate can see the other, and putting the mapping in
/// a third place keeps the reason for the split visible.
pub const fn value_type(kind: ValueKind) -> Type {
    match kind {
        ValueKind::Exact => Type::Exact,
        ValueKind::Statistic => Type::Statistic,
        ValueKind::Count => Type::Count,
        ValueKind::Flag => Type::Flag,
    }
}

/// The inverse, for a caller declaring a feature from a compiled requirement.
pub const fn value_kind(value_type: Type) -> ValueKind {
    match value_type {
        Type::Exact => ValueKind::Exact,
        Type::Statistic => ValueKind::Statistic,
        Type::Count => ValueKind::Count,
        Type::Flag => ValueKind::Flag,
    }
}

/// The cell's venue state, presented as depth the arbitrage engine can price
/// a path against.
///
/// Two rules make this safe to hand to a path pricer:
///
/// * **A stale book supplies nothing.** After a detected gap the book is
///   marked stale, and a stale book answers `None` to every question rather
///   than serving the depth it held before the gap. `None` means "this source
///   is ignorant", which the pricer already distinguishes from a thin book.
/// * **A venue that cannot be priced supplies nothing.** Halted, closed and
///   unreachable venues answer `None` for the same reason.
#[derive(Debug, Default)]
pub struct CellLiquidity {
    /// Keyed by venue then instrument, so one venue going stale cannot take
    /// another's depth with it.
    books: BTreeMap<(String, String), VenueState>,
}

impl CellLiquidity {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, state: VenueState) {
        let key = (
            state.venue().as_str().to_string(),
            state.object_id().as_str().to_string(),
        );
        self.books.insert(key, state);
    }

    pub fn get(&self, venue: &VenueId, object: &ObjectId) -> Option<&VenueState> {
        self.books
            .get(&(venue.as_str().to_string(), object.as_str().to_string()))
    }

    pub fn get_mut(&mut self, venue: &VenueId, object: &ObjectId) -> Option<&mut VenueState> {
        self.books
            .get_mut(&(venue.as_str().to_string(), object.as_str().to_string()))
    }

    pub fn iter(&self) -> impl Iterator<Item = &VenueState> {
        self.books.values()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut VenueState> {
        self.books.values_mut()
    }

    pub fn len(&self) -> usize {
        self.books.len()
    }

    pub fn is_empty(&self) -> bool {
        self.books.is_empty()
    }

    /// The state, but only where it may be priced from.
    ///
    /// The single predicate every method below goes through, so a new accessor
    /// cannot forget the staleness rule.
    fn usable(&self, venue: &VenueId, object: &ObjectId) -> Option<&VenueState> {
        let state = self.get(venue, object)?;
        (!state.is_stale() && state.prices_are_usable()).then_some(state)
    }
}

impl LiquiditySource for CellLiquidity {
    fn sweep_cost(
        &self,
        venue: &VenueId,
        object: &ObjectId,
        side: BookSide,
        quantity: Decimal,
    ) -> Option<(Decimal, Decimal)> {
        let state = self.usable(venue, object)?;
        let sweep = state.book().sweep_cost(side, quantity);
        // A sweep that fills nothing is not a price. Reporting one would let a
        // path claim it could trade at the touch of an empty book.
        let average = sweep.average_price()?;
        Some((average, sweep.filled))
    }

    fn touch(
        &self,
        venue: &VenueId,
        object: &ObjectId,
        side: BookSide,
    ) -> Option<(Decimal, Decimal)> {
        let state = self.usable(venue, object)?;
        let level = match side {
            BookSide::Bid => state.best_bid()?,
            BookSide::Ask => state.best_ask()?,
        };
        Some((level.price, level.size))
    }

    fn as_of(&self, venue: &VenueId, object: &ObjectId) -> Option<Timestamp> {
        self.usable(venue, object)?.last_update()
    }

    fn observations(&self, venue: &VenueId, object: &ObjectId) -> u32 {
        self.usable(venue, object).map_or(0, |state| {
            u32::try_from(state.applied()).unwrap_or(u32::MAX)
        })
    }
}
