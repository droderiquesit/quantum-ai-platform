//! `qip-portfolio` — positions, portfolios and exact accounting.
//!
//! Every quantity and every amount is a [`qip_core::Decimal`]. The accounting
//! identity that matters is
//!
//! ```text
//! equity = cash + sum(position market value)
//! ```
//!
//! and it holds exactly, to the last unit, after any sequence of fills. That is
//! not a stylistic preference: a portfolio whose books drift by a fraction of a
//! cent per trade cannot be reconciled against a broker statement, and once
//! reconciliation is approximate, a real break becomes indistinguishable from
//! accumulated rounding.
//!
//! Realised profit is computed from lots, not from an average price, so the tax
//! lot a sale closes is an explicit choice rather than an artefact.

pub mod exposure;
pub mod lifecycle;
pub mod lot;
pub mod portfolio;
pub mod position;

pub use exposure::{Exposure, ExposureBreakdown};
pub use lifecycle::PositionLifecycle;
pub use lot::{Lot, LotMethod, RealisedTrade};
pub use portfolio::{Portfolio, PortfolioSnapshot, Valuation};
pub use position::{Position, PositionSide};
