//! Blueprint §29.1's composition point: what the platform would quote, priced
//! against its own book, and sent nowhere.
//!
//! `qip_execution_engine::quoting` holds the arithmetic and knows nothing
//! about this platform. This module is the only place that reads the market
//! view, the order manager and the equity together and turns them into a
//! [`QuoteInputs`] — the same reason [`crate::valuation`] exists, and the same
//! reason `rule_review`, `venue_review`, `family_review` and `sizing_review`
//! are each a module here rather than a method on a service.
//!
//! # The boundary, restated where a reader will be standing
//!
//! **This platform is paper trading only and never submits a live order.** A
//! quote loop is the most dangerous thing in this workspace to compose,
//! because in every real venue quoting *is* order submission. So what this
//! module produces is a [`QuoteLoopReview`] — a sentence for the cycle report
//! and a list of priced pairs — and there is no path from it to an order, a
//! broker or a venue. The three layers of the boundary are untouched by every
//! line of it: nothing here reads or writes an autonomy ceiling, constructs a
//! `qip_edge::Cell`, or names a model rung.
//!
//! # What each input is, and what it is not
//!
//! Being precise about provenance matters more here than usual, because the
//! blueprint's §29.1 table is written in the vocabulary of tick-level
//! microstructure and this platform's own record is bar-resolution. Each
//! derivation below says what it measures. None of them is called something it
//! is not.
//!
//! * **Reference mid** — the observed book's mid, else the top-of-book quote's
//!   mid. An instrument with neither is not quoted, and is reported as such
//!   rather than quoted off a fabricated price.
//! * **Volatility** — the standard deviation of one-bar simple returns over
//!   the observed series, in basis points. Per bar, deliberately not
//!   annualised: the half spread is charged for the exposure of one quote, not
//!   of one year.
//! * **Adverse selection** — the mean absolute one-bar return, in basis
//!   points. This is a *bar-resolution proxy* for "how far the reference moves
//!   against a resting quote over the horizon it is exposed for". It is not
//!   the realised-minus-effective spread decomposition
//!   [`qip_market::microstructure::MicrostructureMetrics`] computes, because
//!   that needs a window of quotes and trades and the platform's snapshot
//!   holds one of each.
//! * **Directional persistence** — up-bars minus down-bars over up-bars plus
//!   down-bars, in `[-1, 1]`. A measure of how one-directional the series has
//!   been, which is what the toxic-flow gate pairs with the adverse-selection
//!   reading.
//! * **Signal imbalance** — the observed book's depth imbalance over
//!   [`IMBALANCE_LEVELS`], or the top-of-book quote's, or zero where neither
//!   is published. Zero is the honest reading for "no depth observed": it
//!   moves the fair value nowhere.
//! * **Belief** — [`Platform::sizing_confidence`], the platform's own
//!   confidence arithmetic, unchanged. An instrument it refuses to size is one
//!   this loop does not quote.
//! * **Inventory** — signed filled quantity across every order the manager
//!   holds for the object. Filled, not ordered: an order that was placed and
//!   never filled is not a position.
//! * **Queue position** — [`QueuePosition::Measured`] where the platform holds
//!   a working *limit* order on the instrument and an observed book to locate
//!   it in, [`QueuePosition::Unknown`] otherwise. **The kernel's own
//!   DECIDE→ACT path never produces a limit order** —
//!   [`qip_execution_engine::oms::order_type_for`] returns market,
//!   time-weighted, volume-weighted or participation and never
//!   [`OrderType::Limit`] — so every pass of a today's cycle takes the
//!   `Unknown` arm, which is the pessimistic one. The measured arm is reached
//!   through [`Platform::submit_order`] with a limit order, which is a public
//!   production door, and it exists because the edge cell does rest limit
//!   orders and this is the seam their queue position would arrive through.
//!   That is stated rather than implied, because a branch nothing reaches is
//!   the shape of defect this workspace has a register of.
//!
//! # Money and statistics
//!
//! Equity, inventory, budgets, limits and prices are money or quantities and
//! are [`Decimal`]. Volatility, persistence, imbalance and belief are
//! statistics and are `f64`. **The crossings are at three marked sites**:
//! [`bar_statistics`], which reads closes out of `Decimal` into the return
//! series; [`belief_of`], which reads the platform's confidence out of
//! `Decimal`; and nothing converts back — every price and size this module
//! reports came out of the quoting arithmetic as [`Decimal`] and stays that
//! way.

use std::collections::BTreeMap;

use qip_core::Decimal;
use qip_core::error::Error;
use qip_core::time::Timestamp;
use qip_execution_engine::order::{OrderType, Side};
use qip_execution_engine::quoting::{
    QueuePosition, QuoteDecision, QuoteInputs, QuotePair, QuotePolicy, QuoteReference,
};
use qip_market::book::OrderBook;
use qip_market::snapshot::InstrumentState;

use crate::cycle::StageOutcome;
use crate::platform::Platform;

/// One whole, in basis points. A return is a fraction and a spread is a basis
/// point, and this is the one place the two meet.
const TEN_THOUSAND_BPS: f64 = 10_000.0;

/// Depth levels read for the touch imbalance.
///
/// Five, because the touch alone is the level most easily spoofed and the
/// whole book is dominated by resting size nobody intends to trade.
const IMBALANCE_LEVELS: usize = 5;

/// Bars required before the volatility, adverse-selection and persistence
/// readings are trusted.
///
/// Twenty. Below it the readings are supplied as zero — which is not a guess
/// at a quiet market but the arithmetic's identity: a zero volatility term and
/// a zero adverse-selection term leave the half spread at its base, and a zero
/// persistence cannot trip the toxic-flow gate. An instrument the platform has
/// barely observed is therefore quoted at the base spread rather than at a
/// width invented from four observations.
pub const QUOTE_MIN_BARS: usize = 20;

/// Instruments priced in one pass.
///
/// A bound on the working set, like every other bound in this workspace. The
/// pass is arithmetic on data already in memory, but a loop whose cost grows
/// with the universe is a loop that eventually decides how long a cycle takes.
pub const QUOTE_PASS_LIMIT: usize = 64;

/// The share of equity one instrument's inventory may drift from its target
/// before quoting halts.
///
/// Two per cent. The number the [`Withheld::InventoryAtLimit`] halt is
/// measured against, and it is deliberately tighter than any position limit in
/// the risk set: a market maker that has drifted two per cent of the book into
/// one name has stopped making a market in it and started holding it.
///
/// [`Withheld::InventoryAtLimit`]: qip_execution_engine::quoting::Withheld::InventoryAtLimit
pub const QUOTE_INVENTORY_FRACTION: Decimal = Decimal::from_raw(20_000_000);

/// The share of equity shown on one side of one instrument's quote.
///
/// Half a per cent, before belief, volatility and queue value narrow it
/// further. What it bounds is the size of a *priced intent*; nothing sends it.
pub const QUOTE_BUDGET_FRACTION: Decimal = Decimal::from_raw(5_000_000);

/// The policy every pass is priced under.
///
/// Stated here rather than in [`crate::config::PlatformConfig`] on purpose,
/// and the purpose is narrow: these numbers have never priced anything a venue
/// saw, and a configurable is a promise that a deployment may tune it. When
/// the loop has a consumer beyond the cycle report, this becomes a config
/// block and the default stays exactly these numbers.
///
/// The widths are chosen against a liquid listed name quoted at three basis
/// points, which is what this workspace's own fixtures describe: a ten
/// basis-point half spread is a maker who wants the flow, a hundred is the
/// point past which the quote is a message with no trade in it.
pub fn default_policy() -> QuotePolicy {
    QuotePolicy {
        base_half_spread_bps: 10.0,
        volatility_coefficient: 0.5,
        adverse_selection_coefficient: 1.0,
        imbalance_coefficient_bps: 5.0,
        skew_bps_at_limit: 20.0,
        max_half_spread_bps: 100.0,
        requote_threshold_bps: 5.0,
        minimum_confidence: 0.30,
        toxic_persistence: 0.80,
        toxic_adverse_selection_bps: 25.0,
    }
}

/// What one pass of the loop found across the platform's instruments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuoteLoopReview {
    /// Instruments the pass looked at.
    pub considered: usize,
    /// The pairs it priced, in instrument order.
    pub quoted: Vec<QuotePair>,
    /// Object id to the token of the reason it was not quoted. `BTreeMap`
    /// because this reaches a cycle report and a replay that reorders is not a
    /// replay.
    pub withheld: BTreeMap<String, String>,
    /// Object id to the reason no [`QuoteInputs`] could be built at all — no
    /// observable price, no equity to bound an inventory against, or a belief
    /// the platform refuses to state. A normal state, reported and not charged
    /// as a problem.
    pub unpriceable: BTreeMap<String, String>,
    /// Object id to a refusal the quoting arithmetic returned. **These are
    /// problems**: every one is a reading this module derived and the loop
    /// found malformed, which is a defect here rather than a market state.
    pub refused: BTreeMap<String, String>,
}

impl QuoteLoopReview {
    /// Price every instrument the platform holds a view on.
    pub fn of(platform: &Platform, now: Timestamp) -> Self {
        let policy = default_policy();
        let equity = platform.equity();
        let inventories = inventories_of(platform);
        let mut review = Self::default();

        // The guard borrows the shared market slot, so it is held for this
        // block and not for the cycle — `Platform::market_view`'s own note.
        let market = platform.market_view();
        for (object_id, state) in market.snapshot.instruments().take(QUOTE_PASS_LIMIT) {
            review.considered += 1;
            let inventory = inventories.get(object_id).copied().unwrap_or(Decimal::ZERO);
            let inputs = match build_inputs(platform, object_id, state, inventory, equity, now) {
                Ok(inputs) => inputs,
                Err(reason) => {
                    review.unpriceable.insert(object_id.to_string(), reason);
                    continue;
                }
            };
            match qip_execution_engine::quoting::quote(&policy, &inputs) {
                Ok(QuoteDecision::Quoted(pair)) => review.quoted.push(*pair),
                Ok(QuoteDecision::Withheld(reason)) => {
                    review
                        .withheld
                        .insert(object_id.to_string(), reason.describe());
                }
                Err(error) => {
                    review
                        .refused
                        .insert(object_id.to_string(), error.message().to_string());
                }
            }
        }
        review
    }

    /// The sentence the ACT stage carries.
    ///
    /// `None` when the pass looked at nothing: a platform with no market view
    /// has not withheld any quotes, and a report saying "0 quoted, 0 withheld"
    /// every cycle is noise that hides the cycle where it means something.
    pub fn describe(&self) -> Option<String> {
        if self.considered == 0 {
            return None;
        }
        let mut detail = format!(
            "quote loop priced {} of {} instrument(s)",
            self.quoted.len(),
            self.considered
        );
        if !self.withheld.is_empty() {
            // Named, not counted. "Three withheld" tells an operator that
            // something stopped and not what, and the reason is the only part
            // of it they can act on.
            let named: Vec<String> = self
                .withheld
                .iter()
                .map(|(object, reason)| format!("{object} {reason}"))
                .collect();
            detail.push_str(&format!("; withheld: {}", named.join("; ")));
        }
        if !self.unpriceable.is_empty() {
            detail.push_str(&format!(
                "; {} instrument(s) had nothing to quote against",
                self.unpriceable.len()
            ));
        }
        Some(detail)
    }

    /// What went wrong, as stage problems.
    ///
    /// Only [`Self::refused`]. A withheld quote is the loop working and an
    /// unpriceable instrument is an ordinary state; a refused reading is this
    /// module having derived something the arithmetic would not accept, which
    /// nothing else in the cycle would surface.
    pub fn problems(&self) -> Vec<String> {
        self.refused
            .iter()
            .map(|(object, refusal)| {
                format!(
                    "the quote loop derived a reading {object} could not be priced from: {refusal}"
                )
            })
            .collect()
    }
}

/// The one entry point a stage calls.
///
/// Takes the stage's outcome and returns it carrying what the pass found, so
/// the call site is one line and the value cannot be computed and dropped.
/// The same shape as [`crate::family_review::record_standings`]'s seam and for
/// the same reason: a review whose result the caller may forget to use is a
/// review that will eventually not be used.
pub fn review(platform: &Platform, now: Timestamp, outcome: StageOutcome) -> StageOutcome {
    let review = QuoteLoopReview::of(platform, now);
    let mut outcome = match review.describe() {
        Some(detail) => StageOutcome {
            detail: format!("{}; {detail}", outcome.detail),
            ..outcome
        },
        None => outcome,
    };
    for problem in review.problems() {
        outcome = outcome.with_problem(problem);
    }
    outcome
}

/// Net signed filled quantity per object, across every order the manager
/// holds.
///
/// Filled and not ordered: an order that was placed and never filled is not a
/// position, and quoting a skew against one would skew away from inventory
/// that does not exist. `BTreeMap` because the caller iterates it into a
/// report.
fn inventories_of(platform: &Platform) -> BTreeMap<String, Decimal> {
    let mut inventories: BTreeMap<String, Decimal> = BTreeMap::new();
    for order in platform.orders().orders() {
        let filled = order.filled_quantity();
        if !filled.is_positive() {
            continue;
        }
        let signed = match order.side {
            Side::Buy => filled,
            Side::Sell => -filled,
        };
        *inventories
            .entry(order.object_id.as_str().to_string())
            .or_insert(Decimal::ZERO) += signed;
    }
    inventories
}

/// Build one instrument's reading, or say why there is none.
///
/// `Err(String)` is a sentence for the report rather than an
/// [`qip_core::error::Error`]: every branch here is an ordinary state of a
/// platform that has not observed everything, and dressing it as an error
/// would put an instrument with no book in the same bucket as a reading the
/// arithmetic refused.
fn build_inputs(
    platform: &Platform,
    object_id: &str,
    state: &InstrumentState,
    inventory: Decimal,
    equity: Decimal,
    now: Timestamp,
) -> Result<QuoteInputs, String> {
    let mid = reference_mid(state)
        .ok_or_else(|| "no book or two-sided quote to price against".to_string())?;
    if !mid.is_positive() {
        return Err(format!("the observed mid is {mid}"));
    }
    if !equity.is_positive() {
        return Err(format!(
            "equity is {equity}, so there is no budget to quote against and no book to bound an \
             inventory by"
        ));
    }
    let budget = equity * QUOTE_BUDGET_FRACTION;
    let inventory_limit = (equity * QUOTE_INVENTORY_FRACTION)
        .checked_div(mid)
        .ok_or_else(|| format!("an inventory limit could not be derived from a mid of {mid}"))?;
    if !inventory_limit.is_positive() {
        return Err(format!(
            "an inventory limit of {inventory_limit} at a mid of {mid} would halt quoting on \
             every pass"
        ));
    }

    let belief = belief_of(platform, object_id, now)?;
    let stats = bar_statistics(state);

    Ok(QuoteInputs {
        object_id: object_id.to_string(),
        reference: QuoteReference::ObservedMid { mid },
        inventory,
        // Flat. A market maker's target inventory is flat unless a desk says
        // otherwise, and stating it here makes that a premise a reader can see
        // rather than an assumption baked into the arithmetic.
        inventory_target: Decimal::ZERO,
        inventory_limit,
        budget,
        volatility_bps: stats.volatility_bps,
        adverse_selection_bps: stats.adverse_selection_bps,
        signal_imbalance: imbalance_of(state),
        directional_persistence: stats.persistence,
        belief_confidence: belief,
        queue: queue_position(platform, object_id, state.book.as_ref()),
    })
}

/// The book's mid, else the top-of-book quote's.
///
/// In that order and never the last trade: a mid is what a two-sided quote is
/// centred on, and a trade print is where somebody crossed. Centring a quote
/// on a print would quote around the price the last taker paid rather than the
/// price the market is showing.
fn reference_mid(state: &InstrumentState) -> Option<Decimal> {
    state
        .book
        .as_ref()
        .and_then(OrderBook::mid)
        .or_else(|| state.quote.as_ref().and_then(qip_market::quote::Quote::mid))
}

/// The book's depth imbalance, else the quote's, else nothing.
///
/// Zero for an instrument with neither, which is the arithmetic's identity
/// rather than a guess: a zero signal moves the fair value nowhere.
fn imbalance_of(state: &InstrumentState) -> f64 {
    // A book with levels on either side is preferred over the quote, and the
    // test is whether it has levels rather than whether its imbalance is
    // non-zero: a genuinely balanced book reads zero, and falling through to
    // the quote on that reading would prefer one level to five.
    match state.book.as_ref() {
        Some(book) if !book.bids.is_empty() || !book.asks.is_empty() => {
            book.imbalance(IMBALANCE_LEVELS)
        }
        _ => state
            .quote
            .as_ref()
            .map_or(0.0, qip_market::quote::Quote::imbalance),
    }
}

/// What the observed bars say about volatility, adverse selection and
/// direction.
///
/// **The `Decimal` → `f64` crossing for the market series happens here**, in
/// [`qip_market::bar::BarSeries::returns`], and nothing crosses back: every
/// figure this struct carries is a statistic and reaches the quoting
/// arithmetic as one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BarStatistics {
    volatility_bps: f64,
    adverse_selection_bps: f64,
    persistence: f64,
}

fn bar_statistics(state: &InstrumentState) -> BarStatistics {
    let returns = state.bars.returns();
    if returns.len() < QUOTE_MIN_BARS {
        return BarStatistics::default();
    }
    if !returns.iter().all(|value| value.is_finite()) {
        // A non-finite return means a zero or missing close somewhere in the
        // series. Reporting zeros is the same arithmetic identity the short
        // series takes — the base spread, and no toxicity finding — rather
        // than a width derived from a division nobody can defend.
        return BarStatistics::default();
    }
    let volatility_bps = qip_numerics::stats::stddev(&returns) * TEN_THOUSAND_BPS;
    let magnitudes: Vec<f64> = returns.iter().map(|value| value.abs()).collect();
    let adverse_selection_bps = qip_numerics::stats::mean(&magnitudes) * TEN_THOUSAND_BPS;
    let up = returns.iter().filter(|value| **value > 0.0).count();
    let down = returns.iter().filter(|value| **value < 0.0).count();
    let moves = up + down;
    let persistence = if moves == 0 {
        0.0
    } else {
        // usize → f64: a ratio of counts, in the statistics lane.
        (up as f64 - down as f64) / moves as f64
    };
    BarStatistics {
        volatility_bps,
        adverse_selection_bps,
        persistence,
    }
}

/// The platform's own confidence in this instrument, as a statistic.
///
/// **A `Decimal` → `f64` crossing**, and the one place this module reads a
/// number the rest of the platform sizes with. A confidence outside `[0, 1]`
/// is refused rather than clamped: the quoting arithmetic would refuse it
/// anyway, and refusing here says which instrument rather than which field.
fn belief_of(platform: &Platform, object_id: &str, now: Timestamp) -> Result<f64, String> {
    let confidence = platform
        .sizing_confidence(object_id, now)
        .map_err(|error: Error| {
            format!(
                "the platform will not state a sizing confidence for it: {}",
                error.message()
            )
        })?;
    let confidence = confidence.to_f64();
    if !(0.0..=1.0).contains(&confidence) {
        return Err(format!(
            "its sizing confidence is {confidence}, which is not a fraction"
        ));
    }
    Ok(confidence)
}

/// Where the platform's own working order sits in the queue, if it has one.
///
/// See this module's header for why the `Measured` arm is unreached by
/// today's cycle and reachable through [`Platform::submit_order`].
///
/// Size at prices that trade *at or before* ours counts as ahead: a resting
/// order at our own price arrived either before or after ours and the platform
/// cannot tell which, so counting it as ahead is the conservative reading. The
/// optimistic one would size as if the platform were at the front of a queue
/// it may be at the back of.
fn queue_position(platform: &Platform, object_id: &str, book: Option<&OrderBook>) -> QueuePosition {
    let Some(book) = book else {
        return QueuePosition::Unknown;
    };
    let mut ahead = Decimal::ZERO;
    let mut own = Decimal::ZERO;
    for order in platform.orders().open_orders() {
        if order.object_id.as_str() != object_id {
            continue;
        }
        let OrderType::Limit { price } = order.order_type else {
            continue;
        };
        let remaining = order.remaining_quantity();
        if !remaining.is_positive() {
            continue;
        }
        let levels = match order.side {
            Side::Buy => &book.bids,
            Side::Sell => &book.asks,
        };
        let resting: Decimal = levels
            .iter()
            .filter(|level| match order.side {
                Side::Buy => level.price >= price,
                Side::Sell => level.price <= price,
            })
            .map(|level| level.size)
            .sum();
        ahead += resting;
        own += remaining;
    }
    if own.is_positive() {
        QueuePosition::Measured { ahead, own }
    } else {
        QueuePosition::Unknown
    }
}

#[cfg(test)]
mod tests {
    // The one `f64` compared exactly below is a skew of zero on a flat book —
    // the identity of the arithmetic, not a rounded result. A tolerance there
    // would admit a skew that was merely small on a book with nothing in it,
    // which is the premise the test beside it rests on.
    #![allow(clippy::float_cmp)]

    use super::*;
    use crate::config::PlatformConfig;
    use crate::cycle::Stage;
    use qip_core::ids::ObjectId;
    use qip_core::{Context, dec};
    use qip_execution_engine::order::Order;
    use qip_financial::asset_class::{InstrumentType, Sector};
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::{DataQuality, Provenance};
    use qip_financial::universe::Universe;
    use qip_market::book::BookLevel;
    use qip_market::quote::Quote;
    use qip_market_ingestion::adapter::SensedRecord;
    use qip_observability::Telemetry;
    use qip_risk::limits::{Limit, LimitKind, LimitSet};

    const SYMBOL: &str = "AAA";

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn object() -> ObjectId {
        ObjectId::from_string(format!("obj-{SYMBOL}"))
    }

    /// The liquidity the fixture states for itself, because
    /// `LiquidityProfile` deliberately has no `Default` — a fixture may state
    /// its own premise and may not inherit one nobody wrote down.
    fn universe() -> Universe {
        let mut universe = Universe::new();
        universe
            .insert(
                FinancialObject::builder(
                    object(),
                    SYMBOL,
                    InstrumentType::CommonStock,
                    qip_financial::costs::LiquidityProfile::listed(dec!("5000000"), 3.0),
                )
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .expect("a valid object"),
            )
            .expect("insertable");
        universe
    }

    fn limits() -> LimitSet {
        LimitSet::new("quote-loop-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        )
    }

    fn fixture() -> Platform {
        let config = PlatformConfig::default();
        let (context, _clock) = Context::deterministic(start(), config.seed);
        Platform::new(config, context, Telemetry::silent(), universe(), limits())
            .expect("a platform")
    }

    /// A two-sided book at 99.95 / 100.05, balanced at the touch.
    fn book(bid_size: &str, ask_size: &str) -> SensedRecord {
        SensedRecord::Book(Box::new(OrderBook::from_levels(
            object(),
            "XNYS",
            start(),
            vec![BookLevel::new(
                dec!("99.95"),
                Decimal::parse(bid_size).expect("a size"),
            )],
            vec![BookLevel::new(
                dec!("100.05"),
                Decimal::parse(ask_size).expect("a size"),
            )],
        )))
    }

    fn quote_record() -> SensedRecord {
        SensedRecord::Quote(Quote {
            object_id: object(),
            venue: "XNYS".to_string(),
            at: start(),
            bid: dec!("99.95"),
            ask: dec!("100.05"),
            bid_size: dec!("100"),
            ask_size: dec!("100"),
            quality: DataQuality::default(),
        })
    }

    fn reviewed(platform: &Platform) -> QuoteLoopReview {
        QuoteLoopReview::of(platform, start())
    }

    #[test]
    fn a_platform_with_an_observed_book_prices_a_two_sided_quote_and_sends_nothing() {
        // The admitting case. A loop that found nothing to quote would satisfy
        // every other test in this file, so this one asserts a pair was
        // actually priced — and then asserts the thing this whole lane is
        // about: the pass created no order. Quoting is order submission in
        // every real venue, and the property that it is not here has to be
        // asserted rather than described.
        let mut platform = fixture();
        assert_eq!(platform.observe(vec![book("100", "100")]), 1);

        let review = reviewed(&platform);
        assert_eq!(review.considered, 1, "the premise: one instrument was seen");
        assert_eq!(
            review.quoted.len(),
            1,
            "nothing was priced: {:?} / {:?}",
            review.withheld,
            review.unpriceable
        );
        let pair = &review.quoted[0];
        assert_eq!(pair.object_id, "obj-AAA");
        assert_eq!(
            pair.fair_value,
            dec!("100"),
            "a balanced book moved the mid"
        );
        assert!(pair.bid < pair.fair_value && pair.fair_value < pair.ask);
        assert!(pair.size.is_positive());
        assert!(review.refused.is_empty(), "{:?}", review.refused);

        assert_eq!(
            platform.orders().orders().count(),
            0,
            "the quote loop created an order; a quote in this platform is an intent that is \
             priced and never sent"
        );
        assert!(
            platform.orders().fills().is_empty(),
            "the quote loop produced a fill"
        );
    }

    #[test]
    fn an_instrument_with_no_price_to_quote_against_is_reported_rather_than_priced_off_a_guess() {
        // The last trade is deliberately not a fallback: a print is where
        // somebody crossed, not what the market is showing, and centring a
        // two-sided quote on one quotes around the price the last taker paid.
        // So an instrument with a trade and no book is unpriceable — and the
        // premise is asserted by observing the book afterwards and watching it
        // become quotable.
        let mut platform = fixture();
        let trade = SensedRecord::Trade(qip_market::quote::Trade {
            object_id: object(),
            venue: "XNYS".to_string(),
            at: start(),
            price: dec!("100"),
            size: dec!("10"),
            aggressor: None,
            condition: qip_market::quote::TradeCondition::Regular,
            trade_id: None,
            quality: DataQuality::default(),
        });
        assert_eq!(platform.observe(vec![trade]), 1);

        let review = reviewed(&platform);
        assert_eq!(review.considered, 1);
        assert!(review.quoted.is_empty(), "a print was quoted around");
        assert_eq!(review.unpriceable.len(), 1);
        assert!(
            review.unpriceable["obj-AAA"].contains("no book or two-sided quote"),
            "{:?}",
            review.unpriceable
        );
        // And the report says so without calling it a problem: an instrument
        // the platform has not seen a two-sided market in is an ordinary
        // state, not a defect.
        assert!(review.problems().is_empty());

        assert_eq!(platform.observe(vec![quote_record()]), 1);
        assert_eq!(
            reviewed(&platform).quoted.len(),
            1,
            "the premise: the same instrument is quotable once a two-sided quote arrives"
        );
    }

    #[test]
    fn a_position_the_platform_actually_holds_skews_the_quote_it_would_show() {
        // Blueprint §29.1's inventory skew, reached through the platform's own
        // order manager rather than a hand-built inventory map: the skew has
        // to move when the *book* moves, or it is arithmetic nothing feeds.
        // The premise is the flat pair, asserted first, because two identical
        // pairs would pass a test that only looked at the second.
        let mut platform = fixture();
        assert_eq!(platform.observe(vec![book("100", "100")]), 1);
        let flat = reviewed(&platform).quoted[0].clone();
        assert_eq!(flat.skew_bps, 0.0, "the premise: a flat book is not skewed");

        let order = Order::new(
            qip_core::ids::OrderId::from_string("ord-quote-loop-1"),
            object(),
            Side::Buy,
            dec!("100"),
            OrderType::Market,
            dec!("100"),
            "prop-quote-loop",
            vec!["hyp-quote-loop".to_string()],
            "platform",
            start(),
        );
        platform
            .submit_order(order, start())
            .expect("a paper order clears the controls");
        let held: Decimal = platform
            .orders()
            .orders()
            .map(qip_execution_engine::order::Order::filled_quantity)
            .sum();
        assert!(
            held.is_positive(),
            "the premise: the simulated broker filled something to be long of"
        );

        let long = reviewed(&platform).quoted[0].clone();
        assert!(
            long.skew_bps > 0.0,
            "a long position carried no skew: {}",
            long.describe()
        );
        assert!(
            long.bid < flat.bid && long.ask < flat.ask,
            "a long position did not move both quotes down: {} against {}",
            long.describe(),
            flat.describe()
        );
    }

    #[test]
    fn a_working_limit_order_is_located_in_the_book_and_sizes_the_quote_down_behind_the_queue() {
        // The `Measured` arm, which the kernel's own DECIDE→ACT path never
        // reaches — `order_type_for` returns market, time-weighted,
        // volume-weighted or participation and never `Limit` — and which
        // `Platform::submit_order` does. Asserted here because a branch whose
        // reachability is only described is the shape of defect this workspace
        // keeps a register of, and because the edge cell rests limit orders
        // and this is the seam their queue position would arrive through.
        let mut platform = fixture();
        // A thousand resting at the touch on the bid, against a limit order of
        // ten: nearly all of the queue is ahead.
        assert_eq!(platform.observe(vec![book("1000", "100")]), 1);
        let unknown = reviewed(&platform).quoted[0].clone();

        let order = Order::new(
            qip_core::ids::OrderId::from_string("ord-quote-loop-2"),
            object(),
            Side::Buy,
            dec!("10"),
            OrderType::Limit {
                price: dec!("99.95"),
            },
            dec!("100"),
            "prop-quote-loop",
            vec!["hyp-quote-loop".to_string()],
            "platform",
            start(),
        );
        platform
            .submit_order(order, start())
            .expect("a paper limit order clears the controls");

        let working: Vec<_> = platform.orders().open_orders();
        assert!(
            !working.is_empty(),
            "the premise: a limit order is working for the book to locate"
        );
        match queue_position(
            &platform,
            "obj-AAA",
            platform
                .market_view()
                .snapshot
                .get(&object())
                .and_then(|state| state.book.as_ref()),
        ) {
            QueuePosition::Measured { ahead, own } => {
                assert!(
                    ahead.is_positive(),
                    "the thousand resting at the touch was not counted as ahead"
                );
                assert!(own.is_positive());
            }
            QueuePosition::Unknown => {
                panic!("a working limit order against an observed book read as an unknown queue")
            }
        }

        // A position that is nearly all queue-ahead sizes no larger than the
        // unknown floor, and the unknown floor is what a platform with no
        // working order takes.
        let measured = reviewed(&platform).quoted[0].clone();
        assert!(
            measured.size >= unknown.size,
            "a measured position sized below the unknown floor, which is the worst case"
        );
    }

    #[test]
    fn the_stage_outcome_carries_the_pass_so_the_review_cannot_be_computed_and_dropped() {
        // The seam the production caller uses. One line, and the line returns
        // the outcome — a `review` whose result a stage could discard would be
        // a routing decision that is calculated and ignored, which reads as a
        // control and is not one.
        let mut platform = fixture();
        assert_eq!(platform.observe(vec![book("100", "100")]), 1);

        let before = StageOutcome::ran(Stage::Act, 0, "0 order(s) released");
        let after = review(&platform, start(), before.clone());
        assert_ne!(
            after.detail, before.detail,
            "the pass left no trace on the stage outcome"
        );
        assert!(
            after
                .detail
                .contains("quote loop priced 1 of 1 instrument(s)"),
            "{}",
            after.detail
        );
        assert!(after.problems.is_empty());

        // And a platform that has observed nothing adds nothing: "0 quoted, 0
        // withheld" on every quiet cycle is noise that hides the cycle where
        // the sentence means something.
        let quiet = fixture();
        let untouched = review(&quiet, start(), before.clone());
        assert_eq!(untouched.detail, before.detail);
    }
}
