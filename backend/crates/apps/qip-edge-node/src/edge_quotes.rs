//! What this cell offers its peers: the touch of each book it holds
//! (MESH-5060), and the legs its own gate authorises it to trade in a
//! peer-coordinated cycle (MESH-5046), as plain data for the peer link's
//! port (ADR 0116).
//!
//! Built on the decision thread, because only that thread holds the cell,
//! and handed over with `ReflexPort::try_publish`, which never waits. It is
//! plain data on purpose: this module, like every node module but
//! `main.rs`, may name the peer link only through `qip_peer_link::boundary`,
//! and may not name a protocol type at all — `mesh_lane.rs` (MESH-013)
//! refuses it — so the decision loop holds nothing it could wait on a peer
//! through. The session thread validates and frames on its own side.
//!
//! # Why the loop authorises legs rather than finding cycles
//!
//! A cross-region opportunity is this cell's touch against a peer's. The
//! peer's touch reaches only the session thread, and the loop cannot receive
//! it: the port has no method that could, and must not (ADR 0116 §3). So the
//! loop decides the half it alone can decide — whether this cell may trade a
//! leg at each book, how much, at what limit — and the session pairs that
//! decision with what peers publish. A [`LegAuthority`] is the cell's gate,
//! run here, every turn, and published with a short life: a session that
//! stops hearing from the loop stops being authorised within
//! [`AUTHORITY_TTL`].

use qip_core::{Decimal, Duration, Timestamp};
use qip_edge::cell::Cell;
use qip_peer_link::boundary::{EdgeQuote, LegAuthority, MAX_QUOTES, Offer, Side};

/// How long one turn's authorities stand. Two pass intervals, so a single
/// dropped offer does not lapse them, and short enough that a cell that has
/// halted, or whose loop has stopped, authorises nothing a moment later.
pub const AUTHORITY_TTL: Duration = Duration::from_secs(2);

/// The value a quote is chosen by when the cell holds more books than an
/// edge state may carry: the smaller side's notional at the touch, which is
/// what a peer could actually trade against it.
fn value(quote: &EdgeQuote) -> Decimal {
    let bid = quote
        .bid
        .checked_mul(quote.bid_size)
        .unwrap_or(Decimal::MAX);
    let ask = quote
        .ask
        .checked_mul(quote.ask_size)
        .unwrap_or(Decimal::MAX);
    bid.min(ask)
}

/// The touch of every book the cell holds a two-sided price for, at most
/// [`MAX_QUOTES`] of them, and how many were left out.
///
/// Past the bound the most valuable books are kept, largest first, ties
/// broken by venue then instrument, so the same books always make the cut
/// and a peer's view does not change with the order the cell happens to
/// iterate in; the rest are counted and the count reaches the health body
/// and the port's status (MESH-5060). This took the first sixty-four in
/// iteration order and said nothing, so a cell with more books published a
/// partial view nobody could see was partial. Never depth: a peer pricing a
/// cycle needs what it could trade against.
pub fn executable_quotes_counted(cell: &Cell) -> (Vec<EdgeQuote>, u64) {
    let mut quotes: Vec<EdgeQuote> = cell
        .liquidity()
        .iter()
        .filter_map(|state| {
            let bid = state.best_bid()?;
            let ask = state.best_ask()?;
            Some(EdgeQuote {
                venue: state.venue().as_str().to_string(),
                instrument: state.object_id().as_str().to_string(),
                bid: bid.price,
                bid_size: bid.size,
                ask: ask.price,
                ask_size: ask.size,
                as_of_ns: state.last_update().map_or(0, Timestamp::as_nanos),
            })
        })
        .collect();
    if quotes.len() <= MAX_QUOTES {
        return (quotes, 0);
    }
    quotes.sort_by(|a, b| {
        value(b)
            .cmp(&value(a))
            .then_with(|| a.venue.cmp(&b.venue))
            .then_with(|| a.instrument.cmp(&b.instrument))
    });
    let withheld = (quotes.len() - MAX_QUOTES) as u64;
    quotes.truncate(MAX_QUOTES);
    (quotes, withheld)
}

/// [`executable_quotes_counted`] without the count.
pub fn executable_quotes(cell: &Cell) -> Vec<EdgeQuote> {
    executable_quotes_counted(cell).0
}

/// The legs this cell's gate authorises at each quoted book, now.
///
/// Nothing at all when the cell is halted by any of its halts, when it holds
/// no region allocation or has none free, or when its ceiling is anything
/// but paper — which `Cell`'s constructor already makes impossible, and
/// which is checked here anyway because an authority is the one thing this
/// loop hands Lane 1 that lets it trade.
///
/// Each book gets a buy at its ask and a sell at its bid. The size is the
/// free allocation split evenly across every authority this turn issues, at
/// that authority's price, so that every authority filled at once still fits
/// inside what the region has free: an authority sized to the whole free
/// balance would let the session reserve the same capital once per book.
pub fn leg_authorities(cell: &Cell, quotes: &[EdgeQuote], now: Timestamp) -> Vec<LegAuthority> {
    if cell.is_halted() || cell.autonomy().ceiling().is_live() || quotes.is_empty() {
        return Vec::new();
    }
    let Some(free) = cell.region_allocation_free().filter(|f| f.is_positive()) else {
        return Vec::new();
    };
    let Some(share) = divide_down(free, Decimal::from_int(2 * quotes.len() as i64)) else {
        return Vec::new();
    };
    let valid_until_ns = now.saturating_add(AUTHORITY_TTL).as_nanos();
    let mut authorities = Vec::with_capacity(2 * quotes.len());
    for quote in quotes {
        let spread = quote.ask - quote.bid;
        for (side, limit, touch) in [
            (Side::Buy, quote.ask, quote.ask_size),
            (Side::Sell, quote.bid, quote.bid_size),
        ] {
            let Some(max_size) = divide_down(share, limit).filter(|s| s.is_positive()) else {
                continue;
            };
            authorities.push(LegAuthority {
                venue: quote.venue.clone(),
                instrument: quote.instrument.clone(),
                side,
                limit_price: limit,
                max_size,
                touch_size: touch,
                unwind_per_unit: spread,
                valid_until_ns,
            });
        }
    }
    authorities
}

/// `amount / by`, rounded toward zero so that `result × by` never exceeds
/// `amount`. `Decimal` division rounds to nearest, and an authority rounded
/// up by one unit in the last place is capital the region does not have —
/// small, and exactly the kind of over-commitment a limit exists to refuse.
fn divide_down(amount: Decimal, by: Decimal) -> Option<Decimal> {
    let mut quotient = amount.checked_div(by)?;
    while quotient.checked_mul(by)? > amount {
        quotient = quotient.checked_sub(Decimal::from_raw(1))?;
    }
    Some(quotient)
}

/// Everything this turn offers the peer link.
pub fn cell_offer(cell: &Cell, now: Timestamp) -> Offer {
    let (quotes, withheld) = executable_quotes_counted(cell);
    let legs = leg_authorities(cell, &quotes, now);
    Offer {
        quotes,
        legs,
        withheld,
    }
}
