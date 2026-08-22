//! `qip-orderbook` — in-memory venue state.
//!
//! One instrument at one venue, rebuilt message by message from
//! [`qip_contracts::MarketMessage`]. Every strategy evaluation and every
//! arbitrage scan reads this, so the shape of what it exposes decides what the
//! rest of the edge can ask about the market.
//!
//! Four things drove the design:
//!
//! * **The book never guesses.** [`view::Sweep`] reports the quantity it could
//!   actually find alongside the price, because a caller handed only a price
//!   cannot tell a full fill from a partial one and will size as though it
//!   could. Nothing in this crate extrapolates past the last published level.
//! * **A broken book says so.** A crossed market is either a data error or an
//!   arbitrage, and both are decisions for the consumer. [`view::BookCondition`]
//!   reports it; the derived prices that a strategy would trade on
//!   ([`view::BookView::mid`], [`view::BookView::microprice`]) refuse to serve a
//!   number computed from an inverted touch.
//! * **"Empty" and "wrong" are different states.** A [`qip_contracts::MessageBody::Reset`]
//!   leaves [`venue::VenueState`] awaiting a snapshot, and until it is
//!   resynchronised it serves no prices at all. Trading off a book known to be
//!   stale is far worse than trading off no book.
//! * **Nothing reaches for a clock.** Times arrive on the messages; where a
//!   caller must supply one it is a parameter. Replaying a message stream twice
//!   produces byte-identical state, which is what makes the whole platform
//!   replayable.
//!
//! Two book flavours share one read surface. [`l3::L3Book`] tracks every
//! resting order by reference and can therefore answer the question an
//! aggregated feed cannot — [`l3::L3Book::queue_position`], how much size is
//! ahead of an order at its level. [`l2::L2Book`] holds published levels only.
//! [`book::Book`] wraps whichever the venue supplies, and
//! [`view::BookView`] is implemented by all three so a consumer never branches
//! on the feed's depth.

pub mod auction;
pub mod book;
pub mod l2;
pub mod l3;
mod ladder;
pub mod snapshot;
pub mod venue;
pub mod view;

pub use auction::AuctionState;
pub use book::Book;
pub use l2::L2Book;
pub use l3::{L3Book, QueuePosition, RestingOrder};
pub use ladder::LevelWalk;
pub use snapshot::{BookKind, BookSnapshot, VenueSnapshot};
pub use venue::{LastTrade, VenueState};
pub use view::{BookCondition, BookView, Level, Sweep};
