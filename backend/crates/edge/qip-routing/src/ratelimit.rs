//! What a venue will accept per unit time, and what has already been spent.
//!
//! Blueprint §34.1 lists rate limits among the nine things a venue adapter
//! must provide, for a reason that is easy to underrate: a venue does not
//! answer "too many" politely. It drops the connection, or it bans the account
//! for a fixed period, or — worst — it accepts the order and rejects the
//! *cancel* that follows, leaving a position nobody can withdraw. A platform
//! that discovers its rate limit by exceeding it discovers it at the moment it
//! most needs the venue.
//!
//! Two facts, kept apart on purpose:
//!
//! * [`RateLimits`] is what the venue *publishes* — a descriptor, part of the
//!   venue profile, alongside the fee schedule and the lot size.
//! * [`RateLedger`] is what has *actually been sent*, counted by whoever is
//!   doing the sending.
//!
//! They are separate types because they are separate claims, and because two
//! different parties keep the second one. The router keeps a ledger of what it
//! decided to send; the simulated venue in `qip-brokers` keeps a ledger of what
//! arrived. Those are not the same number when a message is lost, and a design
//! in which one of them is derived from the other cannot ever show the gap.
//!
//! # The window is sliding, and the memory is still bounded
//!
//! A fixed window is O(1) and admits up to twice the limit across a boundary,
//! which is exactly the burst a venue bans for. A sliding window needs the
//! instant of every send, which sounds unbounded and is not: once the window
//! holds `orders_per_window` entries the next order is *refused*, so nothing is
//! ever appended beyond the limit. The working set per venue is bounded by the
//! limits themselves, which are validated on construction — a limit of zero is
//! refused, and so is one large enough to be a denial of service against this
//! process.

use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::VecDeque;

/// The largest per-window allowance this ledger will track for one venue.
///
/// The bound is on *memory*, not on the venue: a venue may publish whatever it
/// likes, and a profile claiming more than this is refused at construction
/// rather than silently trimmed. Trimming would be a clamp, and a clamped rate
/// limit is a limit nobody can reason about — the thing this module exists to
/// avoid. A hundred thousand messages in one window is already two orders of
/// magnitude above any published venue schedule.
pub const MAX_TRACKED_PER_WINDOW: u32 = 100_000;

/// What a venue says it will accept, per window.
///
/// Both counters, because they are different limits at every real venue: an
/// exchange that accepts fifty orders a second typically accepts several
/// hundred messages, since a cancel, a replace and a query are all messages and
/// none of them is an order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimits {
    orders_per_window: u32,
    messages_per_window: u32,
    window: Duration,
}

impl RateLimits {
    /// The allowance assumed for a venue that has not published one.
    ///
    /// Finite, and deliberately not generous. A venue profile always carries a
    /// schedule — there is no `Option` here — because an absent limit is the
    /// shape of control that cannot fire: nothing would ever be refused, the
    /// series would read zero forever, and the first evidence of a limit would
    /// be a ban. Fifty orders and five hundred messages a second is below every
    /// published venue schedule the desk has seen, so a deployment that sets
    /// nothing is conservative rather than unlimited, and a venue that really
    /// allows more says so in its own profile.
    pub const ASSUMED: Self = Self {
        orders_per_window: 50,
        messages_per_window: 500,
        window: Duration::from_secs(1),
    };

    /// Build a schedule, refusing one that cannot be obeyed.
    ///
    /// Four refusals, and none of them clamps:
    ///
    /// * A zero allowance is a venue that accepts nothing. If that is meant,
    ///   the venue is closed and belongs out of rotation, not in a profile with
    ///   a budget of nothing.
    /// * A non-positive window has no rate in it at all.
    /// * More orders than messages is a misread schedule. Every order is at
    ///   least one message, so the order allowance can never exceed the message
    ///   allowance, and a profile saying otherwise has the two columns swapped
    ///   — which would silently hand the router a budget it does not have.
    /// * An allowance above [`MAX_TRACKED_PER_WINDOW`] is refused rather than
    ///   trimmed, because the ledger's working set is bounded by this number.
    pub fn new(orders_per_window: u32, messages_per_window: u32, window: Duration) -> Result<Self> {
        if orders_per_window == 0 || messages_per_window == 0 {
            return Err(Error::invalid(
                "a rate limit of zero admits no orders at all; mark the venue closed through its \
                 VenueStatus rather than giving it a budget of nothing",
            ));
        }
        if window.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a rate limit needs a positive window; state the period the venue publishes, such \
                 as one second",
            ));
        }
        if orders_per_window > messages_per_window {
            return Err(Error::invalid(format!(
                "{orders_per_window} orders against {messages_per_window} messages per window is \
                 not a schedule a venue can publish: every order is at least one message, so the \
                 two columns are swapped; state the message allowance as the larger of the two"
            )));
        }
        if messages_per_window > MAX_TRACKED_PER_WINDOW {
            return Err(Error::invalid(format!(
                "{messages_per_window} messages per window is above the {MAX_TRACKED_PER_WINDOW} \
                 this ledger will hold; shorten the window rather than raising the count, because \
                 the working set is bounded by this number"
            )));
        }
        Ok(Self {
            orders_per_window,
            messages_per_window,
            window,
        })
    }

    /// A per-second schedule, the form every venue publishes.
    pub fn per_second(orders: u32, messages: u32) -> Result<Self> {
        Self::new(orders, messages, Duration::from_secs(1))
    }

    pub const fn orders_per_window(&self) -> u32 {
        self.orders_per_window
    }

    pub const fn messages_per_window(&self) -> u32 {
        self.messages_per_window
    }

    pub const fn window(&self) -> Duration {
        self.window
    }
}

/// What one venue has been sent inside the window.
///
/// Two deques of instants rather than two counters, so the window slides. Each
/// is bounded by its own allowance: nothing is appended once the allowance is
/// reached, because the send that would have appended it is refused.
#[derive(Clone, Debug, Default, PartialEq)]
struct VenueSpend {
    orders: VecDeque<Timestamp>,
    messages: VecDeque<Timestamp>,
}

impl VenueSpend {
    /// Drop everything that fell out of the window ending at `at`.
    fn prune(&mut self, window: Duration, at: Timestamp) {
        let cutoff = at.saturating_sub(window);
        while self.orders.front().is_some_and(|sent| *sent <= cutoff) {
            self.orders.pop_front();
        }
        while self.messages.front().is_some_and(|sent| *sent <= cutoff) {
            self.messages.pop_front();
        }
    }

    fn inside(entries: &VecDeque<Timestamp>, window: Duration, at: Timestamp) -> u32 {
        let cutoff = at.saturating_sub(window);
        // `u32` because the deque is bounded by an allowance that is a `u32`;
        // the saturating cast is the honest form of that bound rather than a
        // cast that could wrap.
        u32::try_from(entries.iter().filter(|sent| **sent > cutoff).count()).unwrap_or(u32::MAX)
    }
}

/// What has actually been sent to each venue, and whether more may be.
///
/// Deliberately not a field of [`crate::router::Router`]: the router is
/// stateless and comparable, and a ledger inside it would make two routers
/// built from the same settings unequal for a reason that has nothing to do
/// with settings. It is passed in, exactly as [`crate::health::HealthTracker`]
/// is, and for the same reason — the caller owns the history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RateLedger {
    /// Keyed by venue in a `BTreeMap`, because the refusal messages this
    /// produces reach a journal and a replay that reorders is not a replay.
    spend: BTreeMap<String, VenueSpend>,
}

impl RateLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// How much of the allowance is left at `at`, without recording anything.
    ///
    /// Read-only on purpose: the router asks this before it has decided
    /// anything, and a question that changed the answer would make the
    /// decision depend on how many times it was asked.
    pub fn remaining(&self, venue: &str, limits: &RateLimits, at: Timestamp) -> (u32, u32) {
        let Some(spend) = self.spend.get(venue) else {
            return (limits.orders_per_window, limits.messages_per_window);
        };
        let orders = VenueSpend::inside(&spend.orders, limits.window, at);
        let messages = VenueSpend::inside(&spend.messages, limits.window, at);
        (
            limits.orders_per_window.saturating_sub(orders),
            limits.messages_per_window.saturating_sub(messages),
        )
    }

    /// Whether one more order would be inside both allowances at `at`.
    pub fn admits_order(&self, venue: &str, limits: &RateLimits, at: Timestamp) -> bool {
        let (orders, messages) = self.remaining(venue, limits, at);
        orders > 0 && messages > 0
    }

    /// Spend one order — and the message that carries it — or refuse.
    ///
    /// Checking and recording are one operation because they were once two,
    /// and two is how a budget gets spent twice: every caller that asked
    /// `admits_order` and then sent had a window between the two in which
    /// another caller could ask the same question and get the same answer.
    /// There is no way to record a send that was not admitted, and no way to
    /// be admitted without the send being recorded.
    pub fn spend_order(&mut self, venue: &str, limits: &RateLimits, at: Timestamp) -> Result<()> {
        let spend = self.spend.entry(venue.to_string()).or_default();
        spend.prune(limits.window, at);
        let orders = VenueSpend::inside(&spend.orders, limits.window, at);
        let messages = VenueSpend::inside(&spend.messages, limits.window, at);
        if orders >= limits.orders_per_window {
            return Err(Error::denied(format!(
                "{venue} allows {} orders per {}; {orders} have already been sent inside the \
                 window ending at {at}, so this one would be the one the venue answers by \
                 dropping the session. Wait for the window to roll, or route to another venue",
                limits.orders_per_window,
                describe_window(limits.window),
            )));
        }
        if messages >= limits.messages_per_window {
            return Err(Error::denied(format!(
                "{venue} allows {} messages per {}; {messages} have already been sent inside the \
                 window ending at {at}. An order is a message too, so cancel less or route to \
                 another venue rather than retrying this one",
                limits.messages_per_window,
                describe_window(limits.window),
            )));
        }
        spend.orders.push_back(at);
        spend.messages.push_back(at);
        Ok(())
    }

    /// Record one message that is not an order — a cancel, a replace, a query.
    ///
    /// Counted because the venue counts it, and **never refused**, which is the
    /// one asymmetry in this module. A cancel reduces risk, and a rate limit
    /// that blocked one would turn a busy window into a window in which the
    /// platform cannot withdraw what it has already placed — the exact failure
    /// an exhausted allowance causes at the venue, reproduced here by the
    /// control meant to avoid it. So a message is recorded and the pressure
    /// lands on [`Self::spend_order`], which refuses the *next order* instead.
    ///
    /// The deque is capped at the allowance so the working set stays bounded
    /// under a caller that cancels without limit. Dropping the oldest cannot
    /// ease anything: the count inside the window saturates at the allowance,
    /// and [`Self::spend_order`] tests `>=`, so it stays refusing.
    pub fn record_message(&mut self, venue: &str, limits: &RateLimits, at: Timestamp) {
        let spend = self.spend.entry(venue.to_string()).or_default();
        spend.prune(limits.window, at);
        spend.messages.push_back(at);
        while spend.messages.len() > limits.messages_per_window as usize {
            spend.messages.pop_front();
        }
    }

    /// Venues this ledger has seen anything sent to.
    pub fn venues(&self) -> impl Iterator<Item = &str> {
        self.spend.keys().map(String::as_str)
    }
}

/// A window as a refusal message should say it.
fn describe_window(window: Duration) -> String {
    let nanos = window.as_nanos();
    if nanos % qip_core::time::NANOS_PER_SEC == 0 {
        let seconds = nanos / qip_core::time::NANOS_PER_SEC;
        if seconds == 1 {
            return "second".to_string();
        }
        return format!("{seconds} seconds");
    }
    format!("{}ms", window.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    #[test]
    fn a_schedule_with_more_orders_than_messages_is_refused_as_two_swapped_columns() {
        let refusal = RateLimits::per_second(100, 10).expect_err("the columns are swapped");
        assert!(
            refusal.message().contains("swapped"),
            "the refusal must name the mistake: {}",
            refusal.message()
        );
    }

    #[test]
    fn an_order_beyond_the_allowance_is_refused_and_the_next_window_admits_it() {
        let limits = RateLimits::per_second(2, 10).expect("a schedule");
        let mut ledger = RateLedger::new();
        // The premise: the budget starts full.
        assert_eq!(ledger.remaining("XLON", &limits, at(0)), (2, 10));
        ledger.spend_order("XLON", &limits, at(0)).expect("first");
        ledger.spend_order("XLON", &limits, at(0)).expect("second");
        let refusal = ledger
            .spend_order("XLON", &limits, at(0))
            .expect_err("the third order is beyond the allowance");
        assert!(refusal.message().contains("2 orders per second"));
        // And the window really slides rather than latching.
        ledger
            .spend_order("XLON", &limits, at(2))
            .expect("a later window is a fresh allowance");
    }

    #[test]
    fn a_cancel_spends_the_message_allowance_without_spending_the_order_allowance() {
        let limits = RateLimits::per_second(2, 3).expect("a schedule");
        let mut ledger = RateLedger::new();
        ledger.record_message("XLON", &limits, at(0));
        assert_eq!(
            ledger.remaining("XLON", &limits, at(0)),
            (2, 2),
            "a cancel is a message and not an order"
        );
    }

    #[test]
    fn cancelling_without_limit_starves_order_entry_and_never_the_cancel_itself() {
        // A cancel is never refused, but it is counted, and enough of them stop
        // the *next order* rather than the next withdrawal. The working set
        // stays bounded while that is true, which is the half a fixed counter
        // would have got right and a naive deque would not.
        let limits = RateLimits::per_second(2, 3).expect("a schedule");
        let mut ledger = RateLedger::new();
        assert!(
            ledger.admits_order("XLON", &limits, at(0)),
            "the premise: order entry is open before any cancel is sent"
        );
        for _ in 0..50 {
            ledger.record_message("XLON", &limits, at(0));
        }
        assert_eq!(
            ledger.remaining("XLON", &limits, at(0)),
            (2, 0),
            "the message allowance saturates"
        );
        // And the *memory* is bounded, which `remaining` cannot show because it
        // saturates whether fifty entries are held or three. This assertion
        // reaches the private deque on purpose: a mutation that removed the cap
        // left `remaining` reading (2, 0) either way, so the test above guarded
        // the refusal and nothing at all about the bound it claims.
        assert_eq!(
            ledger
                .spend
                .get("XLON")
                .map(|spend| spend.messages.len())
                .expect("the venue has been recorded against"),
            limits.messages_per_window() as usize,
            "fifty cancels are held as three entries, not fifty"
        );
        let refusal = ledger
            .spend_order("XLON", &limits, at(0))
            .expect_err("order entry is starved by the message budget");
        assert!(refusal.message().contains("3 messages per second"));
    }
}
