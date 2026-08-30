//! `qip-market` — market structure.
//!
//! The data the Fast Brain reacts to: quotes, trades, order books, bars,
//! corporate actions and term structures. Each type is both a domain object and
//! an event contract, so what a handler receives is exactly what was recorded.
//!
//! Prices and quantities are [`qip_core::Decimal`]; derived statistics are
//! `f64`. The boundary matters: a mid-price is a price and must be exact, while
//! an order-flow imbalance is a statistic and does not need to be.

pub mod bar;
pub mod book;
pub mod corporate_action;
pub mod curve;
pub mod microstructure;
pub mod quote;
pub mod snapshot;

pub use bar::{Bar, BarSeries, Interval};
pub use book::{BookLevel, OrderBook, Side};
pub use corporate_action::{CorporateAction, CorporateActionKind};
pub use curve::{CurvePoint, TermStructure};
pub use microstructure::MicrostructureMetrics;
pub use quote::{Quote, Tick, Trade, TradeCondition};
pub use snapshot::MarketSnapshot;
