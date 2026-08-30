//! The market conditions a simulation can be put through.
//!
//! A backtest that only ever sees an orderly market is a backtest of a market
//! nobody trades in. What breaks a strategy in production is not the average
//! day; it is the venue that stopped answering halfway through a two-leg
//! trade, the feed that kept publishing a price that stopped being true, and
//! the ten seconds in which the depth a size was chosen against was not there.
//!
//! Each of those is a [`MarketCondition`]. They are values, not code paths, so
//! a scenario is assembled rather than written: a [`ConditionWindow`] scopes
//! one condition to a time range, a venue, an instrument and a leg, and a
//! [`ConditionSchedule`] holds however many of them a test wants to compose.
//! "A flash event with a ten-times slippage regime and a venue outage on leg
//! two" is three windows.
//!
//! Two rules hold across the whole module and everything downstream depends on
//! them.
//!
//! * **Every condition is monotonically adverse.** A multiplier is validated to
//!   be at least one, a fraction to be at most one, a delay to be
//!   non-negative. Composition is multiplicative for multipliers, minimum for
//!   fractions and maximum for delays. There is no way to assemble a schedule
//!   that makes execution better than the same run without it, which is what
//!   lets [`crate::execution::ExecutionReport::adversity_bps`] be asserted
//!   monotone in a test.
//! * **The regime is a pure function of the instant.** Jitter and latency
//!   spikes are drawn from a stream seeded on the run seed, the instant, the
//!   venue, the instrument and the leg — never from a stream that advances as
//!   the simulation calls it. Evaluation order therefore cannot change the
//!   answer, and "same seed, same conditions, byte-identical outcome" survives
//!   a caller asking about instants out of order.

use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// A way a feed can be wrong that is not simply "late".
///
/// Kept apart from delay because the handling differs: a delayed feed is
/// usable if you know how old it is, and a malformed one is not usable at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FeedFault {
    /// A message the decoder cannot parse. Nothing can be recovered from it,
    /// so the instant it covers has no observation rather than a wrong one.
    Malformed,
    /// A message repeating a price that is already this old.
    Stale { age: Duration },
    /// A message arriving after one that supersedes it.
    ///
    /// The dangerous case, because it parses. Applying it would move the mark
    /// backwards, so it is recorded and discarded rather than applied.
    OutOfOrder { by: Duration },
}

impl FeedFault {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::Stale { .. } => "stale",
            Self::OutOfOrder { .. } => "out_of_order",
        }
    }

    /// Whether an observation carrying this fault may be priced off at all.
    ///
    /// False for everything: a malformed message yields nothing, a stale one
    /// describes a market that has moved, and an out-of-order one describes a
    /// market that has already been superseded.
    pub const fn is_usable(&self) -> bool {
        false
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Malformed => "the message could not be decoded, so this instant has no observation rather than a wrong one".to_string(),
            Self::Stale { age } => format!(
                "the feed repeated a price already {age:?} old; it is a last-known value, not a current one"
            ),
            Self::OutOfOrder { by } => format!(
                "the message arrived {by:?} behind one that supersedes it; applying it would move the mark backwards"
            ),
        }
    }
}

/// One thing that can be wrong with the market.
///
/// Every variant is adverse by construction — see the module documentation.
/// The parameters are validated rather than trusted, so a scenario cannot
/// accidentally be assembled as a *favourable* one and then be reported as a
/// stress test.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "condition", rename_all = "snake_case")]
pub enum MarketCondition {
    /// The quoted spread widens by a multiple of its calm width.
    SpreadRegime { multiplier: f64 },
    /// Everything a taker pays beyond the reference mid is multiplied.
    ///
    /// The "ten times slippage" a scenario asks for, expressed once rather
    /// than smeared across a spread assumption and an impact coefficient. Read
    /// it as the obvious thing: at `multiplier: 10.0` a fill that cost 3bp
    /// against the arrival mid costs 30bp, and
    /// [`crate::execution::ExecutionReport::slippage_bps`] is ten times what
    /// the same order paid in the calm market.
    ///
    /// *All three* components of that cost scale, because all three are things
    /// the taker pays beyond the mid it is measured against: the half-spread
    /// crossed to reach the touch, the walk into the levels behind it, and the
    /// impact term. A multiplier that scaled only some of them would be a
    /// "ten-times slippage regime" that multiplied slippage by something else,
    /// and the number in the scenario would stop meaning what it says.
    SlippageRegime { multiplier: f64 },
    /// Displayed depth collapses to a fraction of what it was.
    ///
    /// The condition that turns a sized order into a partial fill, which is
    /// the whole reason a simulator must model depth rather than a touch
    /// price.
    Illiquidity { depth_fraction: f64 },
    /// Realised volatility is multiplied, widening the spread with it.
    VolatilitySpike { multiplier: f64 },
    /// A violent move and a recovery.
    ///
    /// The price falls by `magnitude` over `down`, then returns over
    /// `recovery`. Depth collapses and the spread widens in proportion to how
    /// far through the move the market is, because that is the part that hurts:
    /// the price coming back does not give back what was paid to trade in the
    /// hole.
    FlashEvent {
        magnitude: f64,
        down: Duration,
        recovery: Duration,
    },
    /// The venue stops responding.
    ///
    /// Not a rejection — a silence. An order in flight when it starts is
    /// neither filled nor acknowledged, and the residual is what the caller
    /// still owns.
    VenueOutage,
    /// The bid prints above the ask.
    ///
    /// A data fault rather than a market state: the two quotes cannot both be
    /// true, and the book is saying it does not know its own price. Surfaced
    /// rather than normalised — the condition is reported and no mid is
    /// derived from it — and an order that meets it is *refused* rather than
    /// filled at either side, which is the only answer that does not require
    /// deciding which of the two quotes is the error.
    CrossedMarket { by_bps: f64 },
    /// The feed runs behind the market by a fixed delay.
    DelayedFeed { delay: Duration },
    /// The feed publishes something that cannot be trusted.
    BadFeed { fault: FeedFault },
    /// A fixed round-trip latency, with a deterministic jitter band.
    Latency { base: Duration, jitter: Duration },
    /// Latency spikes on a fraction of orders.
    LatencySpike { probability: f64, spike: Duration },
    /// The venue fills at most this fraction of what was asked for.
    ///
    /// Models the queue ahead of an order that an aggregated book cannot see:
    /// the size is displayed, it is simply not this order's to take.
    PartialFillCap { fraction: f64 },
}

impl MarketCondition {
    /// A short stable name, used in reports and in the schedule digest.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SpreadRegime { .. } => "spread_regime",
            Self::SlippageRegime { .. } => "slippage_regime",
            Self::Illiquidity { .. } => "illiquidity",
            Self::VolatilitySpike { .. } => "volatility_spike",
            Self::FlashEvent { .. } => "flash_event",
            Self::VenueOutage => "venue_outage",
            Self::CrossedMarket { .. } => "crossed_market",
            Self::DelayedFeed { .. } => "delayed_feed",
            Self::BadFeed { .. } => "bad_feed",
            Self::Latency { .. } => "latency",
            Self::LatencySpike { .. } => "latency_spike",
            Self::PartialFillCap { .. } => "partial_fill_cap",
        }
    }

    /// Reject a condition that would make the market *better*.
    ///
    /// A multiplier below one or a fraction above one is almost always a typo,
    /// and the cost of accepting it is a stress test that reports a strategy
    /// as robust because the stress was a subsidy.
    pub fn validate(&self) -> Result<()> {
        let finite = |value: f64, label: &str| -> Result<()> {
            if value.is_finite() {
                Ok(())
            } else {
                Err(Error::invalid(format!("{label} must be a finite number")))
            }
        };
        match *self {
            Self::SpreadRegime { multiplier }
            | Self::SlippageRegime { multiplier }
            | Self::VolatilitySpike { multiplier } => {
                finite(multiplier, self.as_str())?;
                if multiplier < 1.0 {
                    return Err(Error::invalid(format!(
                        "{} multiplier {multiplier} is below one, which would make the market kinder than the calm case",
                        self.as_str()
                    )));
                }
            }
            Self::Illiquidity { depth_fraction } => {
                finite(depth_fraction, "depth fraction")?;
                if !(0.0..=1.0).contains(&depth_fraction) {
                    return Err(Error::invalid(
                        "an illiquidity depth fraction must lie in [0, 1]; above one is added liquidity, not less",
                    ));
                }
            }
            Self::PartialFillCap { fraction } => {
                finite(fraction, "fill cap")?;
                if !(0.0..=1.0).contains(&fraction) {
                    return Err(Error::invalid(
                        "a partial-fill cap must lie in [0, 1]; above one would fill more than was asked for",
                    ));
                }
            }
            Self::FlashEvent {
                magnitude,
                down,
                recovery,
            } => {
                finite(magnitude, "flash magnitude")?;
                if !(0.0..1.0).contains(&magnitude) {
                    return Err(Error::invalid(
                        "a flash event's magnitude is a downward fraction in [0, 1)",
                    ));
                }
                if down.as_nanos() <= 0 {
                    return Err(Error::invalid(
                        "a flash event must take some time to fall; an instantaneous move has no interval to trade in",
                    ));
                }
                if recovery.as_nanos() < 0 {
                    return Err(Error::invalid("a flash recovery cannot run backwards"));
                }
            }
            Self::CrossedMarket { by_bps } => {
                finite(by_bps, "cross width")?;
                if by_bps <= 0.0 {
                    return Err(Error::invalid(
                        "a crossed market has the bid strictly above the ask, so the cross must be positive",
                    ));
                }
            }
            Self::DelayedFeed { delay } => {
                if delay.as_nanos() < 0 {
                    return Err(Error::invalid("a feed cannot be delayed into the future"));
                }
            }
            Self::Latency { base, jitter } => {
                if base.as_nanos() < 0 || jitter.as_nanos() < 0 {
                    return Err(Error::invalid("latency cannot be negative"));
                }
            }
            Self::LatencySpike { probability, spike } => {
                finite(probability, "spike probability")?;
                if !(0.0..=1.0).contains(&probability) {
                    return Err(Error::invalid("a spike probability must lie in [0, 1]"));
                }
                if spike.as_nanos() < 0 {
                    return Err(Error::invalid("a latency spike cannot be negative"));
                }
            }
            Self::VenueOutage | Self::BadFeed { .. } => {}
        }
        Ok(())
    }

    /// What this condition does, in the terms a report reader needs.
    pub fn describe(&self) -> String {
        match *self {
            Self::SpreadRegime { multiplier } => {
                format!("the quoted spread is {multiplier:.2}x its calm width")
            }
            Self::SlippageRegime { multiplier } => format!(
                "everything paid beyond the reference mid — spread, walk and impact alike — is multiplied by {multiplier:.2}, so a size chosen at calm slippage is the wrong size"
            ),
            Self::Illiquidity { depth_fraction } => format!(
                "displayed depth collapses to {:.1}% of normal, so an order sized against normal depth partially fills",
                depth_fraction * 100.0
            ),
            Self::VolatilitySpike { multiplier } => {
                format!("realised volatility is {multiplier:.2}x normal and the spread widens with it")
            }
            Self::FlashEvent {
                magnitude,
                down,
                recovery,
            } => format!(
                "a {:.1}% fall over {down:?} recovering over {recovery:?}; the price comes back and the slippage paid in the hole does not",
                magnitude * 100.0
            ),
            Self::VenueOutage => {
                "the venue stops responding: an order in flight is neither filled nor rejected, and the residual is the caller's".to_string()
            }
            Self::CrossedMarket { by_bps } => format!(
                "the bid prints {by_bps:.1}bp above the ask; reported rather than normalised, and an order meeting it is refused rather than filled off a book that contradicts itself"
            ),
            Self::DelayedFeed { delay } => format!(
                "the feed runs {delay:?} behind the market, so every mark is a last-known value rather than a current one"
            ),
            Self::BadFeed { fault } => fault.describe(),
            Self::Latency { base, jitter } => {
                format!("orders reach the venue after {base:?} plus up to {jitter:?} of jitter")
            }
            Self::LatencySpike { probability, spike } => format!(
                "{:.1}% of orders take an extra {spike:?} to arrive",
                probability * 100.0
            ),
            Self::PartialFillCap { fraction } => format!(
                "the venue fills at most {:.1}% of what was asked for, modelling the queue an aggregated book does not show",
                fraction * 100.0
            ),
        }
    }
}

/// One condition, scoped.
///
/// The scope is what makes conditions composable rather than global. A venue
/// outage on leg two is a `VenueOutage` window with `leg = Some(1)`; the same
/// condition with no leg set applies to every order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConditionWindow {
    pub condition: MarketCondition,
    /// First instant the condition applies, inclusive.
    pub from: Timestamp,
    /// Last instant it applies, exclusive. `None` runs to the end of the run.
    pub until: Option<Timestamp>,
    /// Venue the condition is confined to. `None` is every venue.
    pub venue: Option<String>,
    /// Instrument the condition is confined to. `None` is every instrument.
    pub object_id: Option<String>,
    /// Leg index the condition is confined to. `None` is every leg.
    pub leg: Option<usize>,
}

impl ConditionWindow {
    /// A condition that applies from the epoch onward, everywhere.
    pub fn always(condition: MarketCondition) -> Self {
        Self {
            condition,
            from: Timestamp::EPOCH,
            until: None,
            venue: None,
            object_id: None,
            leg: None,
        }
    }

    /// A condition that starts at `from` and never ends.
    pub fn starting(condition: MarketCondition, from: Timestamp) -> Self {
        Self {
            from,
            ..Self::always(condition)
        }
    }

    /// A condition confined to `[from, until)`.
    pub fn between(condition: MarketCondition, from: Timestamp, until: Timestamp) -> Self {
        Self {
            from,
            until: Some(until),
            ..Self::always(condition)
        }
    }

    /// Confine the condition to one venue.
    pub fn on_venue(mut self, venue: impl Into<String>) -> Self {
        self.venue = Some(venue.into());
        self
    }

    /// Confine the condition to one instrument.
    pub fn on_object(mut self, object_id: impl Into<String>) -> Self {
        self.object_id = Some(object_id.into());
        self
    }

    /// Confine the condition to one leg of a multi-leg plan.
    ///
    /// Legs are zero-indexed, so "leg two" is `on_leg(1)`.
    pub fn on_leg(mut self, leg: usize) -> Self {
        self.leg = Some(leg);
        self
    }

    pub fn validate(&self) -> Result<()> {
        self.condition.validate()?;
        if let Some(until) = self.until
            && until < self.from
        {
            return Err(Error::invalid(format!(
                "condition {} ends before it starts",
                self.condition.as_str()
            )));
        }
        Ok(())
    }

    /// Whether this window covers the given instant and scope.
    pub fn applies(&self, at: Timestamp, venue: &str, object_id: &str, leg: usize) -> bool {
        if at < self.from {
            return false;
        }
        if let Some(until) = self.until
            && at >= until
        {
            return false;
        }
        if self.venue.as_deref().is_some_and(|scoped| scoped != venue) {
            return false;
        }
        if self
            .object_id
            .as_deref()
            .is_some_and(|scoped| scoped != object_id)
        {
            return false;
        }
        if self.leg.is_some_and(|scoped| scoped != leg) {
            return false;
        }
        true
    }

    /// How far into the window `at` sits. Zero before it opens.
    pub fn elapsed(&self, at: Timestamp) -> Duration {
        if at <= self.from {
            Duration::ZERO
        } else {
            at.since(self.from)
        }
    }
}

/// The collapsed effect of every condition active at one instant.
///
/// Produced by [`ConditionSchedule::regime`] and consumed by the book builder
/// and the fill engine. Holding it as data rather than as branching means the
/// composition rules live in exactly one place, and every one of them moves in
/// the adverse direction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Regime {
    /// Multiplier on the calm half-spread. Never below one.
    pub spread_multiplier: f64,
    /// Multiplier on everything paid beyond the reference mid — the spread
    /// crossed, the walk and the impact alike. Never below one.
    pub slippage_multiplier: f64,
    /// Surviving fraction of displayed depth. Never above one.
    pub depth_fraction: f64,
    /// Multiplier on realised volatility. Never below one.
    pub volatility_multiplier: f64,
    /// Multiplicative displacement of the reference price, from a flash event.
    /// One when nothing is displacing it.
    pub price_multiplier: f64,
    /// The venue is not answering.
    pub venue_down: bool,
    /// Basis points by which the bid sits above the ask. Zero when uncrossed.
    pub crossed_by_bps: f64,
    /// How far behind the market the feed is running.
    pub feed_delay: Duration,
    /// Faults on the feed at this instant, deduplicated and ordered.
    pub feed_faults: Vec<FeedFault>,
    /// Total latency an order submitted now would take to arrive.
    pub order_latency: Duration,
    /// Ceiling on the fraction of available depth that will fill.
    pub fill_fraction_cap: f64,
    /// Names of the conditions that produced this regime, in schedule order.
    pub applied: Vec<String>,
}

impl Default for Regime {
    fn default() -> Self {
        Self::calm()
    }
}

impl Regime {
    /// The market behaving itself: no widening, no collapse, no delay.
    pub fn calm() -> Self {
        Self {
            spread_multiplier: 1.0,
            slippage_multiplier: 1.0,
            depth_fraction: 1.0,
            volatility_multiplier: 1.0,
            price_multiplier: 1.0,
            venue_down: false,
            crossed_by_bps: 0.0,
            feed_delay: Duration::ZERO,
            feed_faults: Vec::new(),
            order_latency: Duration::ZERO,
            fill_fraction_cap: 1.0,
            applied: Vec::new(),
        }
    }

    /// Whether nothing is being injected at this instant.
    pub fn is_calm(&self) -> bool {
        self.applied.is_empty()
    }

    /// Whether the feed may be priced off as a current observation.
    pub fn feed_is_current(&self) -> bool {
        self.feed_delay.is_zero() && self.feed_faults.is_empty()
    }

    pub fn describe(&self) -> String {
        if self.is_calm() {
            return "calm: no conditions injected".to_string();
        }
        format!(
            "{}: spread {:.2}x, slippage {:.2}x, depth {:.0}%, volatility {:.2}x, price {:.4}x{}{}",
            self.applied.join(" + "),
            self.spread_multiplier,
            self.slippage_multiplier,
            self.depth_fraction * 100.0,
            self.volatility_multiplier,
            self.price_multiplier,
            if self.venue_down { ", venue down" } else { "" },
            if self.crossed_by_bps > 0.0 {
                ", crossed"
            } else {
                ""
            }
        )
    }
}

/// A set of scoped conditions, injected into a run as one value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConditionSchedule {
    windows: Vec<ConditionWindow>,
}

impl ConditionSchedule {
    /// An empty schedule: the calm market.
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
        }
    }

    /// Add a window, for chained construction.
    pub fn with(mut self, window: ConditionWindow) -> Self {
        self.windows.push(window);
        self
    }

    pub fn push(&mut self, window: ConditionWindow) {
        self.windows.push(window);
    }

    pub fn windows(&self) -> &[ConditionWindow] {
        &self.windows
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn validate(&self) -> Result<()> {
        for window in &self.windows {
            window.validate()?;
        }
        Ok(())
    }

    /// The windows covering one instant and scope, in schedule order.
    pub fn active(
        &self,
        at: Timestamp,
        venue: &str,
        object_id: &str,
        leg: usize,
    ) -> Vec<&ConditionWindow> {
        self.windows
            .iter()
            .filter(|window| window.applies(at, venue, object_id, leg))
            .collect()
    }

    /// Collapse every active condition into one [`Regime`].
    ///
    /// `seed` is the run's seed. Jitter and spikes are drawn from a stream
    /// derived from it together with the instant and the scope, so this
    /// function is pure: calling it twice returns the same regime, and calling
    /// it for instants out of order returns the same answers as calling it in
    /// order. A mutable RNG threaded through the simulation would make the
    /// result depend on how many times anything else happened to draw first.
    pub fn regime(
        &self,
        at: Timestamp,
        venue: &str,
        object_id: &str,
        leg: usize,
        seed: u64,
    ) -> Regime {
        let mut regime = Regime::calm();
        for window in self.active(at, venue, object_id, leg) {
            regime.applied.push(window.condition.as_str().to_string());
            apply_condition(&mut regime, window, at, venue, object_id, leg, seed);
        }
        regime.feed_faults.sort_unstable();
        regime.feed_faults.dedup();
        regime
    }

    /// A stable fingerprint of the schedule.
    ///
    /// Two runs that claim to be the same run can be checked on this rather
    /// than on a prose description of what was injected.
    pub fn digest(&self) -> String {
        let mut bytes = Vec::with_capacity(self.windows.len() * 64);
        for window in &self.windows {
            bytes.extend_from_slice(window.condition.as_str().as_bytes());
            bytes.extend_from_slice(&window.from.as_nanos().to_le_bytes());
            bytes.extend_from_slice(
                &window
                    .until
                    .map_or(i64::MIN, |until| until.as_nanos())
                    .to_le_bytes(),
            );
            bytes.extend_from_slice(window.venue.as_deref().unwrap_or("*").as_bytes());
            bytes.extend_from_slice(window.object_id.as_deref().unwrap_or("*").as_bytes());
            bytes.extend_from_slice(&(window.leg.map_or(u64::MAX, |leg| leg as u64)).to_le_bytes());
            // The parameters travel through the serialised form so a change to
            // a multiplier changes the digest without this needing a match arm
            // per variant that a new condition could be forgotten from.
            if let Ok(encoded) = serde_json::to_vec(&window.condition) {
                bytes.extend_from_slice(&encoded);
            }
        }
        qip_core::sha256_hex(&bytes)
    }
}

/// Fold one window's condition into the regime being built.
#[allow(clippy::too_many_arguments)]
fn apply_condition(
    regime: &mut Regime,
    window: &ConditionWindow,
    at: Timestamp,
    venue: &str,
    object_id: &str,
    leg: usize,
    seed: u64,
) {
    match window.condition {
        MarketCondition::SpreadRegime { multiplier } => {
            regime.spread_multiplier *= multiplier.max(1.0);
        }
        MarketCondition::SlippageRegime { multiplier } => {
            regime.slippage_multiplier *= multiplier.max(1.0);
        }
        MarketCondition::Illiquidity { depth_fraction } => {
            regime.depth_fraction = regime.depth_fraction.min(depth_fraction.clamp(0.0, 1.0));
        }
        MarketCondition::VolatilitySpike { multiplier } => {
            let multiplier = multiplier.max(1.0);
            regime.volatility_multiplier *= multiplier;
            // A volatility spike that left the spread alone would be a
            // free option: market makers widen exactly when they are least
            // sure where the price is.
            regime.spread_multiplier *= 1.0 + (multiplier - 1.0) * 0.5;
        }
        MarketCondition::FlashEvent {
            magnitude,
            down,
            recovery,
        } => {
            let displacement = flash_displacement(window.elapsed(at), magnitude, down, recovery);
            regime.price_multiplier *= 1.0 - displacement;
            // Intensity is how far through the move the market is, so the
            // widening and the depth collapse peak at the bottom and unwind
            // with the price.
            let intensity = if magnitude > 0.0 {
                (displacement / magnitude).clamp(0.0, 1.0)
            } else {
                0.0
            };
            regime.spread_multiplier *= 1.0 + 9.0 * intensity;
            regime.volatility_multiplier *= 1.0 + 9.0 * intensity;
            regime.depth_fraction = regime.depth_fraction.min(1.0 - 0.9 * intensity);
        }
        MarketCondition::VenueOutage => regime.venue_down = true,
        MarketCondition::CrossedMarket { by_bps } => {
            regime.crossed_by_bps = regime.crossed_by_bps.max(by_bps.max(0.0));
        }
        MarketCondition::DelayedFeed { delay } => {
            if delay > regime.feed_delay {
                regime.feed_delay = delay;
            }
        }
        MarketCondition::BadFeed { fault } => regime.feed_faults.push(fault),
        MarketCondition::Latency { base, jitter } => {
            let mut stream = scope_stream(seed, at, venue, object_id, leg, "latency");
            let drawn = if jitter.as_nanos() > 0 {
                Duration::from_nanos(stream.below(jitter.as_nanos().unsigned_abs() + 1) as i64)
            } else {
                Duration::ZERO
            };
            regime.order_latency = regime.order_latency + base + drawn;
        }
        MarketCondition::LatencySpike { probability, spike } => {
            let mut stream = scope_stream(seed, at, venue, object_id, leg, "latency_spike");
            if stream.bernoulli(probability.clamp(0.0, 1.0)) {
                regime.order_latency = regime.order_latency + spike;
            }
        }
        MarketCondition::PartialFillCap { fraction } => {
            regime.fill_fraction_cap = regime.fill_fraction_cap.min(fraction.clamp(0.0, 1.0));
        }
    }
}

/// How far below its undisturbed level a flash event has pushed the price.
///
/// Zero before the event and after the recovery, `magnitude` at the bottom,
/// linear in between. Linear rather than shaped because the shape is not what
/// is being tested — the interval in which depth is gone is.
fn flash_displacement(
    elapsed: Duration,
    magnitude: f64,
    down: Duration,
    recovery: Duration,
) -> f64 {
    let elapsed_ns = elapsed.as_nanos().max(0) as f64;
    let down_ns = down.as_nanos().max(1) as f64;
    if elapsed_ns < down_ns {
        return magnitude * (elapsed_ns / down_ns);
    }
    let recovery_ns = recovery.as_nanos().max(0) as f64;
    if recovery_ns <= 0.0 {
        // No recovery leg: the move is permanent from the bottom onward.
        return magnitude;
    }
    let into_recovery = elapsed_ns - down_ns;
    if into_recovery >= recovery_ns {
        return 0.0;
    }
    magnitude * (1.0 - into_recovery / recovery_ns)
}

/// A stream keyed on the run seed and the exact scope being asked about.
///
/// The whole point is that it does not advance: two calls with the same
/// arguments produce the same draws, so the regime is a function of the
/// instant rather than of how many draws preceded it.
fn scope_stream(
    seed: u64,
    at: Timestamp,
    venue: &str,
    object_id: &str,
    leg: usize,
    label: &str,
) -> Xoshiro256 {
    let mut mix = seed ^ 0x9E37_79B9_7F4A_7C15;
    mix = mix.rotate_left(17) ^ (at.as_nanos() as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93);
    mix = mix.rotate_left(23) ^ (leg as u64).wrapping_mul(0xA076_1D64_78BD_642F);
    for text in [venue, object_id, label] {
        for byte in text.as_bytes() {
            mix = mix.rotate_left(7) ^ u64::from(*byte).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
        mix = mix.rotate_left(11) ^ 0x2545_F491_4F6C_DD1D;
    }
    Xoshiro256::seeded(mix)
}
