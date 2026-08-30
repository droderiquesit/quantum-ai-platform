//! The current market picture for a set of instruments.
//!
//! The Fast Brain keeps one of these in memory and updates it in place as
//! events arrive; everything downstream reads a consistent view rather than
//! chasing individual events.

use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::bar::{Bar, BarSeries};
use crate::book::OrderBook;
use crate::quote::{Quote, Trade};

/// Latest known state for one instrument.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InstrumentState {
    pub quote: Option<Quote>,
    pub book: Option<OrderBook>,
    pub last_trade: Option<Trade>,
    pub bars: BarSeries,
    /// Cumulative volume observed today.
    pub session_volume: Decimal,
    pub updated_at: Option<Timestamp>,
}

impl InstrumentState {
    /// Best available price: last trade, else mid, else nothing.
    ///
    /// Ordering matters — a trade is a transacted price, a mid is an
    /// indication. Never fabricate one from the other.
    pub fn reference_price(&self) -> Option<Decimal> {
        self.last_trade
            .as_ref()
            .map(|t| t.price)
            .or_else(|| self.quote.as_ref().and_then(Quote::mid))
    }

    /// How stale the state is as of `now`.
    pub fn staleness(&self, now: Timestamp) -> Option<Duration> {
        self.updated_at.map(|at| now.since(at))
    }
}

/// Market state across instruments.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub as_of: Timestamp,
    states: BTreeMap<String, InstrumentState>,
}

impl MarketSnapshot {
    pub fn new(as_of: Timestamp) -> Self {
        Self {
            as_of,
            states: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    pub fn get(&self, object_id: &ObjectId) -> Option<&InstrumentState> {
        self.states.get(object_id.as_str())
    }

    pub fn instruments(&self) -> impl Iterator<Item = (&str, &InstrumentState)> {
        self.states.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn apply_quote(&mut self, quote: Quote) {
        let at = quote.at;
        let state = self.state_for(&quote.object_id);
        state.updated_at = Some(at);
        state.quote = Some(quote);
        self.advance_to(at);
    }

    pub fn apply_book(&mut self, book: OrderBook) {
        let at = book.at;
        let quote = book.to_quote();
        let state = self.state_for(&book.object_id);
        state.updated_at = Some(at);
        if let Some(quote) = quote {
            state.quote = Some(quote);
        }
        state.book = Some(book);
        self.advance_to(at);
    }

    pub fn apply_trade(&mut self, trade: Trade) {
        let at = trade.at;
        let state = self.state_for(&trade.object_id);
        state.updated_at = Some(at);
        state.session_volume += trade.size;
        state.last_trade = Some(trade);
        self.advance_to(at);
    }

    pub fn apply_bar(&mut self, bar: Bar) {
        let at = bar.close_time();
        let state = self.state_for(&bar.object_id);
        state.updated_at = Some(at);
        state.bars.push(bar);
        self.advance_to(at);
    }

    /// Move the snapshot's clock forward.
    pub fn advance_to(&mut self, at: Timestamp) {
        if at > self.as_of {
            self.as_of = at;
        }
    }

    /// Reset per-session counters at the start of a new trading day.
    pub fn roll_session(&mut self) {
        for state in self.states.values_mut() {
            state.session_volume = Decimal::ZERO;
        }
    }

    /// Instruments whose state has not updated within `tolerance`.
    ///
    /// Stale data is the failure mode that silently poisons everything
    /// downstream, so it is surfaced explicitly rather than left to be noticed.
    pub fn stale_instruments(&self, tolerance: Duration) -> Vec<(&str, Duration)> {
        self.states
            .iter()
            .filter_map(|(id, state)| {
                let age = state.staleness(self.as_of)?;
                (age > tolerance).then_some((id.as_str(), age))
            })
            .collect()
    }

    /// Reference prices for every instrument that has one.
    pub fn prices(&self) -> BTreeMap<String, Decimal> {
        self.states
            .iter()
            .filter_map(|(id, state)| state.reference_price().map(|p| (id.clone(), p)))
            .collect()
    }

    fn state_for(&mut self, object_id: &ObjectId) -> &mut InstrumentState {
        self.states
            .entry(object_id.as_str().to_string())
            .or_default()
    }
}
