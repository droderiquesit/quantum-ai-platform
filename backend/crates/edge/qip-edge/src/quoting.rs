//! The message budget a cell spends at each venue, and what happens when it
//! runs out (§29.2).
//!
//! Quote traffic exceeds order traffic by one to two orders of magnitude, and
//! a venue enforces both a message rate and a message-to-trade ratio. A
//! session that exceeds either is throttled or disconnected by the venue — at
//! a moment nobody chose, with resting orders the cell can then no longer
//! withdraw. That is the failure this module exists to prevent: the cell runs
//! out of budget *deliberately*, on its own arithmetic, and says so, rather
//! than discovering the venue's limit by being cut off at it.
//!
//! Three properties are load-bearing, and each is a decision rather than an
//! implementation detail.
//!
//! * **Withdrawing is not sending.** Placements may spend the bucket down to
//!   [`RateLimits::withdrawal_reserve`] and no further; a withdrawal may spend
//!   what is left. A budget that refused a cancel because the cell had spent
//!   the session quoting would leave exposure on a venue the cell had decided
//!   to leave — the control making the thing worse that it exists to prevent.
//!   The reserve is what makes the mass cancel in `Cell::withdraw_expired`
//!   fundable after a burst of quoting.
//! * **The clock is a parameter.** Nothing here reads a clock. `now` arrives
//!   from the pass, refill is integer arithmetic over elapsed nanoseconds with
//!   the sub-token remainder carried, and a `now` earlier than the last
//!   observation refills nothing rather than refilling backwards. The same
//!   pass timestamps therefore produce the same admissions on a replay.
//! * **A venue this cell was not configured for is refused, not created.**
//!   The bucket set is fixed at assembly from `CellConfig::venues`, so the
//!   series keyed on it is bounded by a deployment-time list and an admission
//!   cannot mint a bucket — which would be both an unbounded label and a
//!   venue with a fresh full budget every time its name changed.
//!
//! The message-to-trade monitor is the venue's *other* limit. It counts
//! messages and trades over a tumbling window of
//! [`RateLimits::monitor_window`] messages; when a window closes with fewer
//! trades than [`RateLimits::messages_per_trade_bound`] allows, quoting
//! narrows — the floor placements face rises from the withdrawal reserve to
//! [`RateLimits::narrowed_reserve`], so the cell keeps less of its rate for
//! quoting until the ratio recovers. It narrows quoting and never withdrawing,
//! for the reason above.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::time::NANOS_PER_SEC;
use qip_core::{Timestamp, time::Duration};
use std::collections::BTreeMap;

/// What a message to a venue does to the cell's exposure.
///
/// The distinction the whole module turns on, as a type rather than as a
/// boolean at each call site: one of these two adds exposure and the other
/// removes it, and they are therefore not interchangeable at a limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    /// A new order, or a requote of one. Bounded by the reserve.
    Placement,
    /// A cancel. May spend the bucket to zero.
    Withdrawal,
}

impl MessageKind {
    /// The label value. A method rather than `Display` so the series identity
    /// is a `&'static str` chosen here and nothing can format a third one.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Placement => "placement",
            Self::Withdrawal => "withdrawal",
        }
    }
}

/// The venue's message limits as this cell has been told to respect them.
///
/// Every field is checked at construction and refused rather than clamped: a
/// budget silently corrected to something the operator did not write is a
/// rate limit nobody can reason about, and the numbers here decide when a
/// safety control fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimits {
    capacity: u32,
    refill_per_second: u32,
    withdrawal_reserve: u32,
    narrowed_reserve: u32,
    messages_per_trade_bound: u32,
    monitor_window: u32,
}

/// The default burst a venue session is assumed to allow.
///
/// See [`RateLimits::default`] for why these are a ceiling rather than a
/// measurement of any particular venue.
pub const DEFAULT_CAPACITY: u32 = 4_096;
/// The default sustained message rate, per second, per venue.
pub const DEFAULT_REFILL_PER_SECOND: u32 = 2_048;
/// The default share of the bucket placements may not touch.
pub const DEFAULT_WITHDRAWAL_RESERVE: u32 = 512;
/// The default floor placements face while the message-to-trade ratio has
/// deteriorated.
pub const DEFAULT_NARROWED_RESERVE: u32 = 2_048;
/// The default messages a venue is assumed to tolerate per trade.
pub const DEFAULT_MESSAGES_PER_TRADE_BOUND: u32 = 64;
/// The default number of messages one monitor window holds.
pub const DEFAULT_MONITOR_WINDOW: u32 = 512;

impl Default for RateLimits {
    /// A budget that is always in force, sized as a ceiling.
    ///
    /// Deliberately not an `Option` on the configuration. A rate limit that
    /// defaults to absent is a control that fires in no deployment nobody
    /// remembered to configure, which is every deployment — the shape this
    /// repository already shipped once, in a limit whose state was always
    /// empty. So the cell always holds a budget and always publishes what is
    /// left of it.
    ///
    /// The numbers are **not** a claim about any venue, because nothing this
    /// cell holds states a venue's limit: they are above every rate this cell
    /// has been measured to produce — the acceptance suite's pass loop runs at
    /// one kilohertz — and far below the rate at which any venue session would
    /// still be up. A deployment that knows its venue's figure sets it with
    /// [`RateLimits::new`]; until it does, the headroom is on a gauge every
    /// pass rather than in somebody's head.
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            refill_per_second: DEFAULT_REFILL_PER_SECOND,
            withdrawal_reserve: DEFAULT_WITHDRAWAL_RESERVE,
            narrowed_reserve: DEFAULT_NARROWED_RESERVE,
            messages_per_trade_bound: DEFAULT_MESSAGES_PER_TRADE_BOUND,
            monitor_window: DEFAULT_MONITOR_WINDOW,
        }
    }
}

impl RateLimits {
    /// The venue's limits, or the refusal naming what to set instead.
    pub fn new(
        capacity: u32,
        refill_per_second: u32,
        withdrawal_reserve: u32,
        narrowed_reserve: u32,
        messages_per_trade_bound: u32,
        monitor_window: u32,
    ) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a quote budget with a capacity of zero refuses every message including the \
                 cancels that would withdraw the cell's exposure; name the burst the venue \
                 session allows",
            ));
        }
        if refill_per_second == 0 {
            return Err(Error::invalid(
                "a quote budget that refills at zero messages per second is spent once and \
                 never again, so the cell stops trading at a moment decided by its first burst; \
                 name the sustained rate the venue allows",
            ));
        }
        if withdrawal_reserve >= capacity {
            return Err(Error::invalid(format!(
                "a withdrawal reserve of {withdrawal_reserve} against a capacity of {capacity} \
                 leaves placements nothing to spend, so the cell would quote nothing at all; \
                 the reserve is the part of the budget cancels keep, and it must be smaller \
                 than the budget"
            )));
        }
        if narrowed_reserve >= capacity {
            return Err(Error::invalid(format!(
                "a narrowed reserve of {narrowed_reserve} against a capacity of {capacity} \
                 stops the cell quoting entirely the first time its message-to-trade ratio \
                 deteriorates; narrowing quoting is not halting it, and a halt is the kill \
                 switch's decision"
            )));
        }
        if narrowed_reserve < withdrawal_reserve {
            return Err(Error::invalid(format!(
                "a narrowed reserve of {narrowed_reserve} is below the withdrawal reserve of \
                 {withdrawal_reserve}, so a deteriorating message-to-trade ratio would let the \
                 cell quote *more* than a healthy one; narrowing may only raise the floor"
            )));
        }
        if messages_per_trade_bound == 0 {
            return Err(Error::invalid(
                "a message-to-trade bound of zero narrows quoting on every window whatever the \
                 cell traded, so the monitor reports the venue's ratio as always bad and the \
                 signal carries nothing; name the messages per trade the venue tolerates",
            ));
        }
        if monitor_window == 0 {
            return Err(Error::invalid(
                "a message-to-trade window of zero messages never closes, so the ratio is never \
                 evaluated and the monitor is a control that cannot fire; name how many \
                 messages one window holds",
            ));
        }
        Ok(Self {
            capacity,
            refill_per_second,
            withdrawal_reserve,
            narrowed_reserve,
            messages_per_trade_bound,
            monitor_window,
        })
    }

    /// The burst, in messages.
    pub const fn capacity(self) -> u32 {
        self.capacity
    }

    /// The sustained rate, in messages per second.
    pub const fn refill_per_second(self) -> u32 {
        self.refill_per_second
    }

    /// What placements may not spend, so that a cancel can always be sent
    /// after a burst of quoting.
    pub const fn withdrawal_reserve(self) -> u32 {
        self.withdrawal_reserve
    }

    /// The floor placements face once the message-to-trade ratio has
    /// deteriorated.
    pub const fn narrowed_reserve(self) -> u32 {
        self.narrowed_reserve
    }

    /// Messages per trade above which quoting narrows.
    pub const fn messages_per_trade_bound(self) -> u32 {
        self.messages_per_trade_bound
    }

    /// How many messages one monitor window holds.
    pub const fn monitor_window(self) -> u32 {
        self.monitor_window
    }

    /// The floor a placement must stay above, given the monitor's verdict.
    const fn placement_floor(self, narrowed: bool) -> u32 {
        if narrowed {
            self.narrowed_reserve
        } else {
            self.withdrawal_reserve
        }
    }
}

/// What the budget said about one message, or one cycle's worth of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// The tokens were spent. `remaining` is what the venue's bucket holds
    /// afterwards — reported so a caller can journal the headroom at the
    /// instant it mattered rather than at the end of the pass.
    Admitted { remaining: u32 },
    /// Nothing was spent and nothing may be sent. The reason names the venue,
    /// the floor and what would clear it, because it is journaled verbatim
    /// and an operator reading "refused" alone learns nothing.
    Refused { reason: String },
}

impl Admission {
    /// Whether the message may be sent.
    pub const fn is_admitted(&self) -> bool {
        matches!(self, Self::Admitted { .. })
    }
}

/// One venue's bucket and monitor.
#[derive(Clone, Debug)]
struct VenueBucket {
    tokens: u32,
    /// Sub-token refill carried between observations, in token-nanoseconds.
    /// Without it a cell passing every millisecond at a rate of 2,048 per
    /// second would gain `2.048` tokens per pass, keep the two and throw the
    /// remainder away, losing 2.3% of its budget to rounding forever.
    carry: i64,
    /// The instant the bucket was last refilled. `None` until the first
    /// admission, so a cell assembled long before its first pass does not
    /// account for that gap.
    last: Option<Timestamp>,
    window_messages: u32,
    window_trades: u32,
    narrowed: bool,
    placements: u64,
    withdrawals: u64,
    trades: u64,
    refusals: u64,
}

impl VenueBucket {
    fn new(capacity: u32) -> Self {
        Self {
            tokens: capacity,
            carry: 0,
            last: None,
            window_messages: 0,
            window_trades: 0,
            narrowed: false,
            placements: 0,
            withdrawals: 0,
            trades: 0,
            refusals: 0,
        }
    }

    /// Accrue what has elapsed since the last observation.
    ///
    /// A `now` at or before the last observation accrues nothing **and leaves
    /// `last` where it was**: the cell's own passes are monotonic, so a
    /// backwards `now` is a caller bug or a clock that stepped, and moving the
    /// mark back would hand the bucket the same interval twice on the way
    /// forward.
    fn refill(&mut self, limits: RateLimits, now: Timestamp) {
        let Some(last) = self.last else {
            self.last = Some(now);
            return;
        };
        let elapsed = now.since(last).as_nanos();
        if elapsed <= 0 {
            return;
        }
        self.last = Some(now);
        // `i128` because the product of a long gap and a high rate overflows
        // `i64` at around a century of elapsed time, and a budget that wrapped
        // would admit everything exactly once.
        let accrued =
            i128::from(elapsed) * i128::from(limits.refill_per_second) + i128::from(self.carry);
        let whole = accrued / i128::from(NANOS_PER_SEC);
        self.carry = i64::try_from(accrued % i128::from(NANOS_PER_SEC)).unwrap_or(0);
        let gained = u32::try_from(whole).unwrap_or(u32::MAX);
        self.tokens = self.tokens.saturating_add(gained).min(limits.capacity);
    }

    /// Count `count` messages against the monitor and close the window if it
    /// is full.
    ///
    /// A tumbling window rather than a sliding one: two counters instead of a
    /// buffer of message instants, which keeps the monitor's memory fixed at
    /// eight bytes per venue however long the cell runs. The verdict persists
    /// between windows, so a cell that has stopped sending stays narrowed
    /// until a window closes healthy rather than recovering by going quiet.
    fn count_messages(&mut self, limits: RateLimits, count: u32) {
        self.window_messages = self.window_messages.saturating_add(count);
        if self.window_messages < limits.monitor_window {
            return;
        }
        let tolerated = u64::from(self.window_trades)
            .saturating_mul(u64::from(limits.messages_per_trade_bound));
        self.narrowed = u64::from(self.window_messages) > tolerated;
        self.window_messages = 0;
        self.window_trades = 0;
    }
}

/// The per-venue message budget of one cell.
///
/// Given to the cell by its configuration, like every other bound it holds.
#[derive(Clone, Debug)]
pub struct QuoteBudget {
    limits: RateLimits,
    /// One bucket per configured venue, and no way to add another. A
    /// `BTreeMap` because the order it is iterated in reaches the pass
    /// summary and the metric registry, and a replay that reorders is not a
    /// replay.
    venues: BTreeMap<String, VenueBucket>,
}

impl QuoteBudget {
    /// A full bucket for each of `venues`, under `limits`.
    pub fn new(limits: RateLimits, venues: &[VenueId]) -> Self {
        let mut buckets = BTreeMap::new();
        for venue in venues {
            buckets.insert(
                venue.as_str().to_string(),
                VenueBucket::new(limits.capacity()),
            );
        }
        Self {
            limits,
            venues: buckets,
        }
    }

    /// The limits in force.
    pub const fn limits(&self) -> RateLimits {
        self.limits
    }

    /// Spend one message's budget at `venue`, or refuse it.
    pub fn admit(&mut self, venue: &VenueId, kind: MessageKind, now: Timestamp) -> Admission {
        match kind {
            MessageKind::Placement => self.admit_all(std::slice::from_ref(venue), now),
            MessageKind::Withdrawal => self.spend_withdrawal(venue, now),
        }
    }

    /// Spend one placement for each entry of `venues`, all of them or none.
    ///
    /// The all-or-nothing form exists for the arbitrage cycle: a cycle short
    /// one leg is a position rather than a smaller cycle, so a budget that
    /// funded three legs of four would convert a rate limit into an open
    /// position. Repeats are counted, so a cycle with two legs at one venue
    /// needs two of that venue's tokens.
    pub fn admit_all(&mut self, venues: &[VenueId], now: Timestamp) -> Admission {
        if venues.is_empty() {
            return Admission::Admitted { remaining: 0 };
        }
        let mut wanted: BTreeMap<&str, u32> = BTreeMap::new();
        for venue in venues {
            *wanted.entry(venue.as_str()).or_insert(0) += 1;
        }
        for (name, count) in &wanted {
            let Some(bucket) = self.venues.get_mut(*name) else {
                return Admission::Refused {
                    reason: unconfigured(name),
                };
            };
            bucket.refill(self.limits, now);
            let floor = self.limits.placement_floor(bucket.narrowed);
            let spendable = bucket.tokens.saturating_sub(floor);
            if spendable < *count {
                bucket.refusals = bucket.refusals.saturating_add(1);
                return Admission::Refused {
                    reason: format!(
                        "the quote budget at {name} holds {} message(s) and keeps {floor} of \
                         them for withdrawals{}, so the {count} this needs are not there; it \
                         refills at {} per second",
                        bucket.tokens,
                        if bucket.narrowed {
                            " while its message-to-trade ratio is narrowed"
                        } else {
                            ""
                        },
                        self.limits.refill_per_second()
                    ),
                };
            }
        }
        let mut remaining = 0;
        for (name, count) in &wanted {
            let Some(bucket) = self.venues.get_mut(*name) else {
                continue;
            };
            bucket.tokens = bucket.tokens.saturating_sub(*count);
            bucket.placements = bucket.placements.saturating_add(u64::from(*count));
            bucket.count_messages(self.limits, *count);
            remaining = bucket.tokens;
        }
        Admission::Admitted { remaining }
    }

    fn spend_withdrawal(&mut self, venue: &VenueId, now: Timestamp) -> Admission {
        let Some(bucket) = self.venues.get_mut(venue.as_str()) else {
            return Admission::Refused {
                reason: unconfigured(venue.as_str()),
            };
        };
        bucket.refill(self.limits, now);
        if bucket.tokens == 0 {
            bucket.refusals = bucket.refusals.saturating_add(1);
            return Admission::Refused {
                reason: format!(
                    "the quote budget at {} is spent to the last message, so this withdrawal \
                     cannot be sent this pass; the order stays open and is withdrawn on a pass \
                     with budget, which is in at most {} millisecond(s)",
                    venue.as_str(),
                    NANOS_PER_SEC / 1_000_000 / i64::from(self.limits.refill_per_second()).max(1)
                ),
            };
        }
        bucket.tokens -= 1;
        bucket.withdrawals = bucket.withdrawals.saturating_add(1);
        bucket.count_messages(self.limits, 1);
        Admission::Admitted {
            remaining: bucket.tokens,
        }
    }

    /// A venue reported a trade. The denominator of the message-to-trade
    /// ratio, taken from a confirmed fill and from nothing else: an order the
    /// cell sent is a message, and only what the venue says filled is a trade.
    pub fn observe_trade(&mut self, venue: &VenueId) {
        if let Some(bucket) = self.venues.get_mut(venue.as_str()) {
            bucket.window_trades = bucket.window_trades.saturating_add(1);
            bucket.trades = bucket.trades.saturating_add(1);
        }
    }

    /// Accrue every bucket to `now` without spending anything.
    ///
    /// Called once per pass so the published headroom is what the cell holds
    /// *now* rather than what it held when it last sent something. A cell
    /// that has been quiet for an hour would otherwise publish the bucket it
    /// drained an hour ago, and an operator would read a spent budget as the
    /// reason for the silence it is not.
    pub fn refill_all(&mut self, now: Timestamp) {
        for bucket in self.venues.values_mut() {
            bucket.refill(self.limits, now);
        }
    }

    /// What every bucket holds, in venue order.
    pub fn summary(&self) -> Vec<VenueBudgetState> {
        self.venues
            .iter()
            .map(|(venue, bucket)| VenueBudgetState {
                venue: venue.clone(),
                tokens: bucket.tokens,
                narrowed: bucket.narrowed,
                placements: bucket.placements,
                withdrawals: bucket.withdrawals,
                trades: bucket.trades,
                refusals: bucket.refusals,
            })
            .collect()
    }
}

fn unconfigured(venue: &str) -> String {
    format!(
        "{venue} is not a venue this cell was configured for, so it has no message budget and \
         nothing may be sent to it; name it in QIP_VENUES or route elsewhere"
    )
}

/// One venue's budget as the pass found it.
///
/// Reported rather than inferred from the series, because the idle reading is
/// the one that matters: a cell that sent nothing publishes a full bucket, an
/// unnarrowed monitor and zero messages, which is a different state from a
/// cell that has no budget at all and a different state again from one that
/// has spent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VenueBudgetState {
    pub venue: String,
    pub tokens: u32,
    pub narrowed: bool,
    pub placements: u64,
    pub withdrawals: u64,
    pub trades: u64,
    pub refusals: u64,
}

/// How long the bucket takes to refill by one message, for a caller stating
/// when a refused message could be retried.
pub fn refill_interval(limits: RateLimits) -> Duration {
    Duration::from_nanos(NANOS_PER_SEC / i64::from(limits.refill_per_second()).max(1))
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn venue() -> VenueId {
        VenueId::new("XPAR")
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(1_760_000_000_000 + millis)
    }

    fn limits() -> Result<RateLimits> {
        // A small bucket so the arithmetic is readable: ten messages of
        // burst, two per second, four kept for cancels, six while narrowed.
        RateLimits::new(10, 2, 4, 6, 4, 8)
    }

    fn budget() -> Result<QuoteBudget> {
        Ok(QuoteBudget::new(limits()?, &[venue()]))
    }

    #[test]
    fn a_placement_may_not_spend_the_reserve_a_cancel_would_need() -> Result<()> {
        // The failure this prevents: sizing the bucket for quoting and
        // discovering at the kill switch that the session has no messages
        // left to withdraw with. Six placements is the whole spendable part
        // of a ten-token bucket with a four-token reserve.
        let mut budget = budget()?;
        for message in 0..6 {
            assert!(
                budget
                    .admit(&venue(), MessageKind::Placement, at(0))
                    .is_admitted(),
                "the premise failed: placement {message} was refused while the bucket still had \
                 spendable tokens"
            );
        }
        assert!(
            !budget
                .admit(&venue(), MessageKind::Placement, at(0))
                .is_admitted(),
            "a seventh placement ate into the reserve the cancels are kept in"
        );
        for message in 0..4 {
            assert!(
                budget
                    .admit(&venue(), MessageKind::Withdrawal, at(0))
                    .is_admitted(),
                "withdrawal {message} was refused although the reserve exists for exactly it"
            );
        }
        assert!(
            !budget
                .admit(&venue(), MessageKind::Withdrawal, at(0))
                .is_admitted(),
            "a fifth withdrawal was admitted from an empty bucket"
        );
        Ok(())
    }

    #[test]
    fn the_bucket_refills_at_the_configured_rate_and_never_past_its_capacity() -> Result<()> {
        let mut budget = budget()?;
        for _ in 0..6 {
            assert!(
                budget
                    .admit(&venue(), MessageKind::Placement, at(0))
                    .is_admitted()
            );
        }
        assert_eq!(
            budget.summary()[0].tokens,
            4,
            "the premise failed: the bucket was not spent to its reserve"
        );
        // Two per second, so one second is two tokens exactly.
        budget.refill_all(at(1_000));
        assert_eq!(
            budget.summary()[0].tokens,
            6,
            "a second of elapsed time did not accrue the configured two messages"
        );
        budget.refill_all(at(1_000_000));
        assert_eq!(
            budget.summary()[0].tokens,
            10,
            "the bucket accrued past the burst the venue allows"
        );
        Ok(())
    }

    #[test]
    fn a_sub_token_interval_accrues_rather_than_rounding_away() -> Result<()> {
        // The failure this prevents: integer division per observation. At two
        // messages per second a 100ms pass earns 0.2 tokens; truncating each
        // pass earns nothing at all, and the budget decays to zero however
        // slowly the cell quotes.
        let mut budget = budget()?;
        for _ in 0..6 {
            assert!(
                budget
                    .admit(&venue(), MessageKind::Placement, at(0))
                    .is_admitted()
            );
        }
        for step in 1..=5 {
            budget.refill_all(at(step * 100));
        }
        assert_eq!(
            budget.summary()[0].tokens,
            5,
            "five 100ms observations at two per second did not accrue one whole message"
        );
        Ok(())
    }

    #[test]
    fn a_clock_that_steps_backwards_accrues_nothing_and_loses_no_interval() -> Result<()> {
        let mut budget = budget()?;
        for _ in 0..6 {
            assert!(
                budget
                    .admit(&venue(), MessageKind::Placement, at(0))
                    .is_admitted()
            );
        }
        budget.refill_all(at(-5_000));
        assert_eq!(
            budget.summary()[0].tokens,
            4,
            "a backwards observation refilled the bucket"
        );
        budget.refill_all(at(1_000));
        assert_eq!(
            budget.summary()[0].tokens,
            6,
            "the backwards observation moved the mark, so the interval to 1s was mismeasured"
        );
        Ok(())
    }

    #[test]
    fn a_window_that_closes_with_too_few_trades_narrows_quoting_and_not_withdrawing() -> Result<()>
    {
        // Eight messages per window, four tolerated per trade: a window with
        // no trade in it narrows. Narrowed, the floor is six of ten, so a
        // bucket holding six admits no placement and still admits a cancel.
        let mut budget = budget()?;
        for _ in 0..6 {
            assert!(
                budget
                    .admit(&venue(), MessageKind::Placement, at(0))
                    .is_admitted()
            );
        }
        assert!(
            !budget.summary()[0].narrowed,
            "the premise failed: the monitor narrowed before its window closed"
        );
        budget.refill_all(at(2_000));
        assert_eq!(budget.summary()[0].tokens, 8, "the premise failed");
        for _ in 0..2 {
            assert!(
                budget
                    .admit(&venue(), MessageKind::Placement, at(2_000))
                    .is_admitted()
            );
        }
        assert!(
            budget.summary()[0].narrowed,
            "eight messages and no trade left the monitor reporting a healthy ratio"
        );
        // The bucket now holds six, which is the narrowed floor exactly.
        assert_eq!(budget.summary()[0].tokens, 6, "the premise failed");
        assert!(
            !budget
                .admit(&venue(), MessageKind::Placement, at(2_000))
                .is_admitted(),
            "a narrowed monitor did not raise the floor placements face"
        );
        assert!(
            budget
                .admit(&venue(), MessageKind::Withdrawal, at(2_000))
                .is_admitted(),
            "narrowing stopped the cell withdrawing, which is the one thing it must never do"
        );
        Ok(())
    }

    #[test]
    fn a_window_with_trades_in_it_leaves_quoting_alone() -> Result<()> {
        // The other half of the property above: without it, a monitor that
        // narrowed unconditionally would pass the test that only checks it
        // narrows.
        let mut budget = budget()?;
        for message in 0..6 {
            assert!(
                budget
                    .admit(&venue(), MessageKind::Placement, at(0))
                    .is_admitted()
            );
            if message % 2 == 0 {
                budget.observe_trade(&venue());
            }
        }
        budget.refill_all(at(2_000));
        for _ in 0..2 {
            assert!(
                budget
                    .admit(&venue(), MessageKind::Placement, at(2_000))
                    .is_admitted()
            );
        }
        assert_eq!(
            budget.summary()[0].trades,
            3,
            "the premise failed: no trade was observed"
        );
        assert!(
            !budget.summary()[0].narrowed,
            "eight messages against three trades is within a bound of four per trade, and \
             quoting was narrowed anyway"
        );
        Ok(())
    }

    #[test]
    fn a_cycle_that_cannot_fund_every_leg_funds_none_of_them() -> Result<()> {
        // The failure this prevents: a rate limit converting a four-leg cycle
        // into a three-leg open position.
        let mut budget = budget()?;
        for _ in 0..5 {
            assert!(
                budget
                    .admit(&venue(), MessageKind::Placement, at(0))
                    .is_admitted()
            );
        }
        assert_eq!(
            budget.summary()[0].tokens,
            5,
            "the premise failed: the bucket does not hold exactly one spendable token"
        );
        let legs = [venue(), venue(), venue()];
        assert!(
            !budget.admit_all(&legs, at(0)).is_admitted(),
            "three legs were admitted against one spendable message"
        );
        assert_eq!(
            budget.summary()[0].tokens,
            5,
            "the refused cycle spent tokens on legs it never sent"
        );
        Ok(())
    }

    #[test]
    fn a_venue_the_cell_was_never_configured_for_has_no_budget_to_spend() -> Result<()> {
        let mut budget = budget()?;
        let elsewhere = VenueId::new("XLON");
        assert!(
            !budget
                .admit(&elsewhere, MessageKind::Placement, at(0))
                .is_admitted(),
            "an unconfigured venue was handed a fresh full bucket"
        );
        assert_eq!(
            budget.summary().len(),
            1,
            "the admission minted a bucket, so the series keyed on venue is unbounded"
        );
        Ok(())
    }

    #[test]
    fn a_budget_that_could_not_hold_its_own_discipline_is_refused_at_configuration() {
        // Each of these would produce a control that reads as protection and
        // is not, so each is refused rather than corrected to something the
        // operator did not write.
        // The message rather than `is_err()`: a capacity of zero also
        // trips the reserve clause below it — a reserve of zero is not
        // smaller than a capacity of zero — so a bare `is_err()` here passed
        // even with the capacity check deleted, which a mutation found.
        let no_capacity = RateLimits::new(0, 2, 0, 0, 4, 8)
            .expect_err("a capacity of zero was admitted")
            .message()
            .to_string();
        assert!(
            no_capacity.contains("capacity of zero"),
            "a capacity of zero was refused for some other reason: {no_capacity}"
        );
        assert!(RateLimits::new(10, 0, 4, 6, 4, 8).is_err(), "zero refill");
        assert!(
            RateLimits::new(10, 2, 10, 10, 4, 8).is_err(),
            "a reserve at capacity leaves no quoting at all"
        );
        assert!(
            RateLimits::new(10, 2, 4, 10, 4, 8).is_err(),
            "a narrowed reserve at capacity halts rather than narrows"
        );
        assert!(
            RateLimits::new(10, 2, 6, 4, 4, 8).is_err(),
            "a narrowed reserve below the withdrawal reserve widens quoting when the ratio \
             deteriorates"
        );
        assert!(
            RateLimits::new(10, 2, 4, 6, 0, 8).is_err(),
            "a bound of zero narrows on every window"
        );
        assert!(
            RateLimits::new(10, 2, 4, 6, 4, 0).is_err(),
            "a window of zero never closes"
        );
    }

    #[test]
    fn the_default_budget_is_in_force_rather_than_absent() {
        // The failure this prevents is named in the rules: a limit that ships
        // in every deployment and can never fire. The default is a ceiling,
        // but it is a ceiling that exists, is spent, and is published.
        let limits = RateLimits::default();
        assert!(
            limits.withdrawal_reserve() < limits.capacity(),
            "the default budget leaves placements nothing to spend"
        );
        let mut budget = QuoteBudget::new(limits, &[venue()]);
        let spendable = limits.capacity() - limits.withdrawal_reserve();
        for message in 0..spendable {
            // A trade per message keeps the message-to-trade monitor healthy,
            // so what is measured here is the bucket rather than the
            // narrowing — the two floors are different numbers and a test
            // that hit the wrong one would say the default budget is smaller
            // than it is.
            budget.observe_trade(&venue());
            assert!(
                budget
                    .admit(&venue(), MessageKind::Placement, at(0))
                    .is_admitted(),
                "the default budget refused placement {message} of its own spendable range"
            );
        }
        assert!(
            !budget
                .admit(&venue(), MessageKind::Placement, at(0))
                .is_admitted(),
            "the default budget admits without bound, so the cell has a rate limit in name only"
        );
    }
}
