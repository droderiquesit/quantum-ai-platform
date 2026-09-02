//! Tests for the market simulator and the conditions injected into it.
//!
//! Two claims carry the module and both are demonstrated rather than asserted
//! in prose:
//!
//! * **The simulator is never more generous than reality.** A fill cannot
//!   exceed the depth the book was showing, under any condition; adding any
//!   condition to a run never improves the execution; and where no honest
//!   price exists — a crossed touch, a book with one side, a fill past the
//!   participation the cost model will quote — the order comes back refused
//!   with its residual exact rather than filled at an invented one.
//! * **Determinism is the product.** The same seed and the same conditions
//!   produce a byte-identical outcome, including when the conditions
//!   themselves were drawn from a seeded generator.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_financial::quality::DataQuality;
use qip_market::bar::{Bar, Interval};
use qip_market::book::Side;
use qip_simulation_engine::conditions::{
    ConditionSchedule, ConditionWindow, FeedFault, MarketCondition,
};
use qip_simulation_engine::execution::{ExecutionPlan, FillStatus, SimOrder};
use qip_simulation_engine::market::{
    InstrumentSpec, MarketSimulator, MarketView, SimStrategy, SyntheticMarket,
};
use qip_simulation_engine::venue::{BookCondition, MarkSource, SimBook};
use qip_simulation_engine::{CostModel, Unfillable};

const VENUE: &str = "XSIM";
const OTHER_VENUE: &str = "XALT";
const SYMBOL: &str = "obj-aaa";

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn step() -> Duration {
    Duration::from_secs(60)
}

fn spec() -> InstrumentSpec {
    InstrumentSpec::liquid(SYMBOL, dec!("100"))
}

fn market() -> SyntheticMarket {
    SyntheticMarket {
        start: start(),
        step: step(),
        steps: 64,
        venues: vec![VENUE.to_string(), OTHER_VENUE.to_string()],
        instruments: vec![spec()],
    }
}

fn simulator(schedule: ConditionSchedule, seed: u64) -> Result<MarketSimulator> {
    MarketSimulator::synthetic(market(), seed)?.with_conditions(schedule)
}

fn calm(seed: u64) -> Result<MarketSimulator> {
    simulator(ConditionSchedule::new(), seed)
}

/// The instant orders are sent at in the single-order tests: far enough into
/// the run that a condition anchored at the start is in the middle of doing
/// whatever it does.
fn trade_time(sim: &MarketSimulator) -> Timestamp {
    sim.steps()
        .get(10)
        .copied()
        .unwrap_or_else(|| start().saturating_add(step() * 10))
}

fn buy(quantity: Decimal) -> SimOrder {
    SimOrder::market(SYMBOL, VENUE, Side::Buy, quantity)
}

/// A strategy that buys a fixed clip on a fixed cadence, so a run's outcome is
/// a function of the market rather than of anything the strategy decided.
#[derive(Debug)]
struct MetronomeBuyer {
    clip: Decimal,
    every: usize,
    seen: usize,
}

impl MetronomeBuyer {
    fn new(clip: Decimal, every: usize) -> Self {
        Self {
            clip,
            every,
            seen: 0,
        }
    }
}

impl SimStrategy for MetronomeBuyer {
    fn name(&self) -> &str {
        "metronome-buyer"
    }

    fn on_step(&mut self, view: &MarketView<'_>) -> Vec<SimOrder> {
        self.seen += 1;
        if self.seen % self.every != 0 {
            return Vec::new();
        }
        // Reads the mark it will trade against, so a delayed or malformed feed
        // reaches the decision as well as the fill.
        let _ = view.mark(SYMBOL, VENUE);
        vec![buy(self.clip)]
    }
}

// ---------------------------------------------------------------- determinism

#[test]
fn the_same_seed_and_the_same_conditions_produce_a_byte_identical_run() -> Result<()> {
    let schedule = ConditionSchedule::new()
        .with(ConditionWindow::always(MarketCondition::SlippageRegime {
            multiplier: 10.0,
        }))
        .with(ConditionWindow::starting(
            MarketCondition::FlashEvent {
                magnitude: 0.08,
                down: Duration::from_secs(600),
                recovery: Duration::from_secs(1_200),
            },
            start().saturating_add(step() * 5),
        ));

    let first =
        simulator(schedule.clone(), 0xC0FFEE)?.run(&mut MetronomeBuyer::new(dec!("400"), 4))?;
    let second = simulator(schedule, 0xC0FFEE)?.run(&mut MetronomeBuyer::new(dec!("400"), 4))?;

    assert_eq!(
        first.digest(),
        second.digest(),
        "the same seed and conditions produced different runs"
    );
    assert_eq!(first.reports, second.reports);
    assert_eq!(first.profit_and_loss, second.profit_and_loss);
    Ok(())
}

#[test]
fn a_different_seed_produces_a_different_run() -> Result<()> {
    let first = calm(1)?.run(&mut MetronomeBuyer::new(dec!("400"), 4))?;
    let second = calm(2)?.run(&mut MetronomeBuyer::new(dec!("400"), 4))?;
    assert_ne!(
        first.digest(),
        second.digest(),
        "two seeds produced the same path, so the seed is not reaching the generator"
    );
    Ok(())
}

/// A schedule assembled by a seeded generator, replayed twice.
fn chaotic_schedule(rng: &mut Xoshiro256, anchor: Timestamp) -> ConditionSchedule {
    let mut schedule = ConditionSchedule::new();
    let count = 1 + rng.below(5) as usize;
    for _ in 0..count {
        let condition = match rng.below(12) {
            0 => MarketCondition::SpreadRegime {
                multiplier: rng.uniform(1.0, 8.0),
            },
            1 => MarketCondition::SlippageRegime {
                multiplier: rng.uniform(1.0, 12.0),
            },
            2 => MarketCondition::Illiquidity {
                depth_fraction: rng.uniform(0.01, 1.0),
            },
            3 => MarketCondition::VolatilitySpike {
                multiplier: rng.uniform(1.0, 10.0),
            },
            4 => MarketCondition::FlashEvent {
                magnitude: rng.uniform(0.02, 0.3),
                down: Duration::from_secs(120 + rng.below(600) as i64),
                recovery: Duration::from_secs(120 + rng.below(1_800) as i64),
            },
            5 => MarketCondition::VenueOutage,
            6 => MarketCondition::CrossedMarket {
                by_bps: rng.uniform(1.0, 60.0),
            },
            7 => MarketCondition::DelayedFeed {
                delay: Duration::from_millis(1 + rng.below(5_000) as i64),
            },
            8 => MarketCondition::BadFeed {
                fault: match rng.below(3) {
                    0 => FeedFault::Malformed,
                    1 => FeedFault::Stale {
                        age: Duration::from_secs(1 + rng.below(60) as i64),
                    },
                    _ => FeedFault::OutOfOrder {
                        by: Duration::from_millis(1 + rng.below(900) as i64),
                    },
                },
            },
            9 => MarketCondition::Latency {
                base: Duration::from_micros(50 + rng.below(2_000) as i64),
                jitter: Duration::from_micros(rng.below(1_000) as i64),
            },
            10 => MarketCondition::LatencySpike {
                probability: rng.uniform(0.0, 1.0),
                spike: Duration::from_millis(1 + rng.below(50) as i64),
            },
            _ => MarketCondition::PartialFillCap {
                fraction: rng.uniform(0.01, 1.0),
            },
        };
        schedule.push(ConditionWindow::starting(condition, anchor));
    }
    schedule
}

#[test]
fn a_randomly_generated_condition_sequence_reproduces_exactly_from_its_seed() -> Result<()> {
    for case in 0..48u64 {
        let seed = 0x5EED_0000 ^ case;
        let mut left = Xoshiro256::seeded(seed);
        let mut right = Xoshiro256::seeded(seed);
        let first = chaotic_schedule(&mut left, start());
        let second = chaotic_schedule(&mut right, start());
        assert_eq!(
            first.digest(),
            second.digest(),
            "case {case}: the same generator seed produced two different schedules"
        );

        let a = simulator(first, seed)?.run(&mut MetronomeBuyer::new(dec!("700"), 3))?;
        let b = simulator(second, seed)?.run(&mut MetronomeBuyer::new(dec!("700"), 3))?;
        assert_eq!(
            a.digest(),
            b.digest(),
            "case {case}: injected chaos was not reproducible\n{}",
            a.summarise()
        );
    }
    Ok(())
}

#[test]
fn asking_about_instants_out_of_order_gives_the_same_answers_as_asking_in_order() -> Result<()> {
    // The regime is a pure function of the instant, so a caller that probes
    // the middle of a run before its start cannot perturb what it sees.
    let schedule = ConditionSchedule::new()
        .with(ConditionWindow::starting(
            MarketCondition::LatencySpike {
                probability: 0.5,
                spike: Duration::from_millis(20),
            },
            start(),
        ))
        .with(ConditionWindow::starting(
            MarketCondition::Latency {
                base: Duration::from_micros(200),
                jitter: Duration::from_micros(800),
            },
            start(),
        ));
    let sim = simulator(schedule, 77)?;

    let forwards: Vec<Duration> = sim
        .steps()
        .iter()
        .map(|at| sim.regime_at(*at, VENUE, SYMBOL, 0).order_latency)
        .collect();
    let backwards: Vec<Duration> = sim
        .steps()
        .iter()
        .rev()
        .map(|at| sim.regime_at(*at, VENUE, SYMBOL, 0).order_latency)
        .collect();

    let reversed: Vec<Duration> = backwards.into_iter().rev().collect();
    assert_eq!(
        forwards, reversed,
        "the regime depended on the order it was asked about"
    );
    Ok(())
}

#[test]
fn the_schedule_digest_changes_when_a_parameter_changes() {
    let mild =
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::SlippageRegime {
            multiplier: 2.0,
        }));
    let harsh =
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::SlippageRegime {
            multiplier: 10.0,
        }));
    assert_ne!(
        mild.digest(),
        harsh.digest(),
        "two schedules with different multipliers fingerprinted the same"
    );
}

// ------------------------------------------------------- depth and the sweep

#[test]
fn the_simulated_sweep_agrees_with_the_platform_order_book() -> Result<()> {
    // The simulator's fills must be priced the way the rest of the platform
    // prices a sweep. Checked against the real type rather than claimed.
    let sim = calm(11)?;
    let book = sim.book_at(SYMBOL, VENUE, trade_time(&sim), 0)?;
    let reference = book.to_order_book();

    for units in [1i64, 250, 999, 1_000, 1_001, 4_500, 20_000, 100_000] {
        let quantity = Decimal::from_int(units);
        for side in [Side::Buy, Side::Sell] {
            let ours = book.sweep(side, quantity);
            match reference.sweep(side, quantity) {
                Some((average, filled)) => {
                    assert_eq!(
                        ours.filled, filled,
                        "sweep of {units} {side:?} filled a different quantity than the platform book"
                    );
                    assert_eq!(
                        ours.average_price(),
                        Some(average),
                        "sweep of {units} {side:?} priced differently from the platform book"
                    );
                }
                None => assert!(
                    !ours.filled.is_positive(),
                    "the platform book could not fill {units} {side:?} but the simulator did"
                ),
            }
        }
    }
    Ok(())
}

#[test]
fn a_sweep_never_supplies_more_than_the_book_is_showing() -> Result<()> {
    let sim = calm(12)?;
    let book = sim.book_at(SYMBOL, VENUE, trade_time(&sim), 0)?;
    let available = book.depth(Side::Sell);
    let greedy = book.sweep(Side::Buy, available * Decimal::from_int(10));
    assert_eq!(
        greedy.filled, available,
        "the sweep invented depth beyond the last published level"
    );
    assert!(
        greedy.residual().is_positive(),
        "an order the book cannot supply reported no residual"
    );
    Ok(())
}

#[test]
fn a_sweep_consumes_resting_orders_in_price_then_time_priority() -> Result<()> {
    let mut book = SimBook::new(ObjectId::from_string(SYMBOL.to_string()), VENUE, start());
    let early = book.rest(Side::Sell, dec!("100.01"), dec!("50"), start())?;
    let late = book.rest(
        Side::Sell,
        dec!("100.01"),
        dec!("50"),
        start().saturating_add(Duration::from_millis(1)),
    )?;
    let deeper = book.rest(
        Side::Sell,
        dec!("100.02"),
        dec!("50"),
        start().saturating_sub(Duration::from_secs(60)),
    )?;

    // The deeper order is the oldest of the three. Price beats time, so it is
    // still consumed last.
    assert_eq!(
        book.queue_ahead(Side::Sell, dec!("100.01"), late),
        Some(dec!("50"))
    );
    assert_eq!(
        book.queue_ahead(Side::Sell, dec!("100.01"), early),
        Some(Decimal::ZERO)
    );

    let outcome = book.take(Side::Buy, dec!("120"), start());
    let order: Vec<u64> = outcome.consumed.iter().map(|c| c.order_id).collect();
    assert_eq!(
        order,
        vec![early, late, deeper],
        "the book did not consume best price first and oldest first within a price"
    );
    assert_eq!(outcome.filled, dec!("120"));
    assert_eq!(book.depth(Side::Sell), dec!("30"));
    Ok(())
}

#[test]
fn a_fill_never_exceeds_the_depth_the_book_was_showing_under_any_condition() -> Result<()> {
    for case in 0..64u64 {
        let seed = 0xDEEB_0000 ^ case;
        let mut rng = Xoshiro256::seeded(seed);
        let schedule = chaotic_schedule(&mut rng, start());
        let sim = simulator(schedule, seed)?;
        let at = trade_time(&sim);
        let order = buy(dec!("50000")).worked_in(4, Duration::from_secs(30));
        let report = sim.execute(&order, at)?;

        for slice in &report.slices {
            assert!(
                slice.filled <= slice.depth_available,
                "case {case}: a slice filled {} against {} of depth",
                slice.filled,
                slice.depth_available
            );
        }
        assert_eq!(
            report.filled + report.residual,
            report.requested,
            "case {case}: fills and residual did not account for the order"
        );
    }
    Ok(())
}

#[test]
fn a_flash_event_cannot_fill_more_than_the_collapsed_depth() -> Result<()> {
    let flash = MarketCondition::FlashEvent {
        magnitude: 0.15,
        down: Duration::from_secs(600),
        recovery: Duration::from_secs(600),
    };
    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::starting(flash, start())),
        21,
    )?;
    // Ten minutes in is the bottom of the fall, where the depth collapse is
    // deepest.
    let at = start().saturating_add(Duration::from_secs(600));
    let book = sim.book_at(SYMBOL, VENUE, at, 0)?;
    let collapsed = book.depth(Side::Sell);
    let calm_book = calm(21)?.book_at(SYMBOL, VENUE, at, 0)?;

    assert!(
        collapsed < calm_book.depth(Side::Sell),
        "a flash event left the book as deep as it was"
    );
    let report = sim.execute(&buy(dec!("100000")), at)?;
    assert!(
        report.filled <= collapsed,
        "a fill of {} exceeded the {collapsed} the collapsed book was showing",
        report.filled
    );
    assert_eq!(report.status, FillStatus::Partial);
    assert!(report.residual.is_positive());
    Ok(())
}

// ------------------------------------------------------------ crossed markets

#[test]
fn a_crossed_market_is_surfaced_rather_than_silently_normalised() -> Result<()> {
    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::CrossedMarket {
            by_bps: 25.0,
        })),
        31,
    )?;
    let at = trade_time(&sim);
    let book = sim.book_at(SYMBOL, VENUE, at, 0)?;

    assert_eq!(book.condition(), BookCondition::Crossed);
    assert!(
        book.mid().is_none(),
        "the simulator served a mid computed from an inverted touch"
    );
    let crossed_by = book
        .crossed_by()
        .expect("a crossed book must say how far through it is");
    assert!(crossed_by.is_positive());

    // A strategy trading on it can see that it was crossed, both at decision
    // time and afterwards on the report.
    let mark = sim.mark_at(SYMBOL, VENUE, at, 0);
    assert!(mark.is_crossed(), "the mark did not carry the cross");
    assert_eq!(mark.crossed_by, Some(crossed_by));

    let report = sim.execute(&buy(dec!("500")), at)?;
    assert!(report.was_crossed(), "the report did not carry the cross");
    assert_eq!(report.crossed_by, Some(crossed_by));
    assert_eq!(report.book_condition, BookCondition::Crossed);
    Ok(())
}

/// A crossed book is a data fault, not a market state, and the simulator
/// refuses to trade against one.
///
/// This test used to assert something weaker and, as it turned out, false:
/// that a taker on a crossed book is *charged the worse of the two touch
/// prices*. That holds only while the cross is wide. The simulated cross
/// inverts the touch symmetrically about the mid, so at a cross narrower than
/// twice the calm half-spread **both** quotes — the buyer's "worse" one
/// included — sit inside the orderly touch, and charging the worse of them
/// hands the buyer a better fill than an orderly market would have. The old
/// test passed only because it happened to check a 40bp cross; the property
/// test below caught it at 5bp.
///
/// The property asserted here is strictly stronger, and holds at every cross
/// width rather than at one: there is no crossed fill price at all. Neither
/// side of a book that contradicts itself is a price the simulator will
/// publish, so no cross width can be found at which a taker is paid for one.
#[test]
fn a_taker_on_a_crossed_book_is_refused_rather_than_filled_at_either_side() -> Result<()> {
    let at = trade_time(&calm(41)?);
    let orderly_buy = calm(41)?.execute(&buy(dec!("500")), at)?;
    let sell = SimOrder::market(SYMBOL, VENUE, Side::Sell, dec!("500"));
    let orderly_sale = calm(41)?.execute(&sell, at)?;
    assert_eq!(orderly_buy.status, FillStatus::Complete);
    assert_eq!(orderly_sale.status, FillStatus::Complete);

    // Deliberately spanning a cross narrower than the calm spread (the case
    // that was paying the taker) as well as one far wider than it.
    for by_bps in [0.5, 1.0, 2.0, 5.0, 25.0, 40.0, 200.0] {
        let crossed = simulator(
            ConditionSchedule::new().with(ConditionWindow::always(
                MarketCondition::CrossedMarket { by_bps },
            )),
            41,
        )?;
        for (order, orderly) in [
            (buy(dec!("500")), &orderly_buy),
            (sell.clone(), &orderly_sale),
        ] {
            let report = crossed.execute(&order, at)?;
            let side = order.side.as_str();

            // Nothing traded, and the reason is on the report rather than
            // buried in a price.
            assert_eq!(
                report.status,
                FillStatus::CrossedBook,
                "a {side} at a book crossed by {by_bps}bp came back {:?}",
                report.status
            );
            assert!(report.was_crossed(), "the report did not carry the cross");
            assert!(report.crossed_by.is_some_and(Decimal::is_positive));

            // No fill, and no price of any kind derived from the inverted
            // touch — neither a fill price nor a reference to measure one
            // against.
            assert_eq!(report.filled, Decimal::ZERO);
            assert_eq!(
                report.average_price(),
                None,
                "the simulator published a {side} price off a book crossed by {by_bps}bp"
            );
            assert_eq!(
                report.reference, None,
                "the simulator published a reference derived from an inverted touch"
            );
            assert_eq!(report.notional, Decimal::ZERO);
            assert_eq!(report.commission, Decimal::ZERO);

            // The residual is exact and known to still be the caller's: the
            // venue was answering, it was the book that could not be trusted.
            assert_eq!(report.residual, report.requested);
            assert!(report.status.residual_is_certain());
            assert!(!report.status.traded());

            // And the execution is worse than the orderly one, by the only
            // scalar that compares them: a request that did not trade at all.
            assert!(
                report.adversity_bps() > orderly.adversity_bps(),
                "a {side} at a book crossed by {by_bps}bp scored {:.6}bp against {:.6}bp in an orderly market",
                report.adversity_bps(),
                orderly.adversity_bps()
            );
        }
    }
    Ok(())
}

/// The narrow cross specifically: the shape of the old defect.
///
/// A cross of a fraction of a basis point leaves a book whose two quotes are
/// all but touching, and whose every price is better than the orderly touch.
/// That is the configuration in which "fill at the worse side" paid the taker,
/// so it gets an assertion of its own rather than only a place in a loop.
#[test]
fn a_cross_narrower_than_the_spread_still_cannot_improve_on_an_orderly_fill() -> Result<()> {
    let at = trade_time(&calm(43)?);
    let orderly = calm(43)?.execute(&buy(dec!("2500")), at)?;
    let orderly_price = orderly
        .average_price()
        .expect("the orderly book filled the order");

    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::CrossedMarket {
            by_bps: 0.25,
        })),
        43,
    )?;
    let book = sim.book_at(SYMBOL, VENUE, at, 0)?;
    let bid = book.best_bid().expect("a crossed book has a bid").price;
    let ask = book.best_ask().expect("a crossed book has an ask").price;
    assert_eq!(book.condition(), BookCondition::Crossed);
    // The premise: on this book even the buyer's worse quote is inside the
    // orderly fill, so any fill off it would be a subsidy.
    assert!(
        bid.max(ask) < orderly_price,
        "the premise no longer holds: the worse touch {} is not inside the orderly fill {orderly_price}",
        bid.max(ask)
    );

    let report = sim.execute(&buy(dec!("2500")), at)?;
    assert_eq!(report.status, FillStatus::CrossedBook);
    assert_eq!(report.average_price(), None);
    assert!(report.adversity_bps() > orderly.adversity_bps());
    Ok(())
}

// ------------------------------------------------------------- venue outages

#[test]
fn a_venue_outage_mid_order_leaves_a_known_residual_and_no_silent_fill() -> Result<()> {
    let at = trade_time(&calm(51)?);
    // The venue answers for the first two slices and then stops.
    let outage_from = at.saturating_add(Duration::from_secs(75));
    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::starting(
            MarketCondition::VenueOutage,
            outage_from,
        )),
        51,
    )?;
    let order = buy(dec!("4000")).worked_in(4, Duration::from_secs(30));
    let report = sim.execute(&order, at)?;

    assert_eq!(report.status, FillStatus::VenueUnreachable);
    assert!(
        report.filled.is_positive(),
        "the slices before the outage should have filled"
    );
    assert!(
        report.residual.is_positive(),
        "an order cut short by an outage reported no residual"
    );
    assert_eq!(
        report.filled + report.residual,
        report.requested,
        "the residual did not account for the unfilled part exactly"
    );
    assert!(
        !report.status.residual_is_certain(),
        "an outage must not claim the residual's whereabouts are known"
    );
    assert!(
        report.slices.iter().any(|slice| !slice.venue_responding),
        "no slice recorded the venue going quiet"
    );

    // A run through the same outage must not silently complete the order.
    let complete = calm(51)?.execute(&order, at)?;
    assert_eq!(complete.status, FillStatus::Complete);
    assert!(complete.filled > report.filled);
    Ok(())
}

#[test]
fn a_venue_outage_on_leg_two_leaves_the_other_legs_reported_and_the_plan_incomplete() -> Result<()>
{
    let sim = simulator(
        ConditionSchedule::new()
            .with(ConditionWindow::always(MarketCondition::VenueOutage).on_leg(1)),
        61,
    )?;
    let at = trade_time(&sim);
    let plan = ExecutionPlan::new()
        .leg(buy(dec!("500")))
        .leg(SimOrder::market(SYMBOL, VENUE, Side::Sell, dec!("500")))
        .leg(buy(dec!("300")));
    let report = sim.execute_plan(&plan, at)?;

    assert_eq!(report.legs.len(), 3);
    assert_eq!(report.unreachable_legs(), vec![1]);
    assert_eq!(report.legs[0].status, FillStatus::Complete);
    assert_eq!(report.legs[1].status, FillStatus::VenueUnreachable);
    assert_eq!(report.legs[1].residual, dec!("500"));
    assert_eq!(
        report.legs[2].status,
        FillStatus::Complete,
        "a later leg was dropped because an earlier one failed, hiding that it would have filled"
    );
    assert!(!report.is_complete());
    Ok(())
}

// ------------------------------------------------------- feeds and staleness

#[test]
fn a_delayed_feed_produces_a_mark_that_is_marked_stale_rather_than_presented_as_current()
-> Result<()> {
    let delay = Duration::from_secs(120);
    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::DelayedFeed {
            delay,
        })),
        71,
    )?;
    let at = trade_time(&sim);

    let fresh = calm(71)?.mark_at(SYMBOL, VENUE, at, 0);
    let delayed = sim.mark_at(SYMBOL, VENUE, at, 0);

    assert!(!fresh.is_stale());
    assert!(delayed.is_stale(), "a delayed feed produced a current mark");
    assert_eq!(delayed.staleness(), delay);
    assert!(
        delayed.current_price().is_none(),
        "a stale mark was offered as the current price"
    );
    assert!(
        delayed.last_known_price().is_some(),
        "a stale mark withheld the last known price as well, which is more than it should"
    );
    assert_ne!(
        delayed.last_known_price(),
        fresh.last_known_price(),
        "the delayed mark carried the current price, so the delay did nothing"
    );
    assert!(delayed.describe().contains("LAST KNOWN"));
    Ok(())
}

#[test]
fn a_malformed_feed_yields_no_observation_and_no_order() -> Result<()> {
    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::BadFeed {
            fault: FeedFault::Malformed,
        })),
        81,
    )?;
    let at = trade_time(&sim);
    let mark = sim.mark_at(SYMBOL, VENUE, at, 0);
    assert!(
        mark.last_known_price().is_none(),
        "a message that would not decode still produced a price"
    );
    assert!(mark.is_stale());

    let report = sim.execute(&buy(dec!("500")), at)?;
    assert_eq!(report.status, FillStatus::FeedUnusable);
    assert_eq!(report.filled, Decimal::ZERO);
    assert_eq!(report.residual, dec!("500"));
    Ok(())
}

#[test]
fn an_out_of_order_message_leaves_the_mark_usable_only_as_a_last_known_value() -> Result<()> {
    let fault = FeedFault::OutOfOrder {
        by: Duration::from_millis(250),
    };
    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::BadFeed { fault })),
        91,
    )?;
    let at = trade_time(&sim);
    let mark = sim.mark_at(SYMBOL, VENUE, at, 0);

    assert!(mark.is_stale());
    assert!(mark.current_price().is_none());
    assert!(mark.last_known_price().is_some());
    assert_eq!(mark.faults, vec![fault]);
    assert!(!fault.is_usable());
    Ok(())
}

// --------------------------------------------------------------- partial fills

#[test]
fn a_partial_fill_leaves_the_exact_residual_with_no_drift() -> Result<()> {
    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::Illiquidity {
            depth_fraction: 0.05,
        })),
        101,
    )?;
    let at = trade_time(&sim);
    let requested = dec!("3333.333333333");
    let report = sim.execute(&SimOrder::market(SYMBOL, VENUE, Side::Buy, requested), at)?;

    assert_eq!(report.status, FillStatus::Partial);
    assert_eq!(
        report.filled + report.residual,
        requested,
        "the residual did not close the order exactly"
    );
    let from_slices: Decimal = report.slices.iter().map(|slice| slice.filled).sum();
    assert_eq!(from_slices, report.filled);
    Ok(())
}

#[test]
fn repeatedly_re_sending_the_residual_closes_the_order_to_the_last_unit() -> Result<()> {
    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::PartialFillCap {
            fraction: 0.37,
        })),
        111,
    )?;
    let at = trade_time(&sim);
    let requested = dec!("1234.567891234");
    let mut remaining = requested;
    let mut filled = Decimal::ZERO;

    for _ in 0..64 {
        if !remaining.is_positive() {
            break;
        }
        let report = sim.execute(&SimOrder::market(SYMBOL, VENUE, Side::Buy, remaining), at)?;
        if !report.filled.is_positive() {
            break;
        }
        filled += report.filled;
        remaining = report.residual;
    }

    assert_eq!(
        filled + remaining,
        requested,
        "a chain of partial fills drifted away from the order it was working"
    );
    assert!(
        remaining < dec!("0.001"),
        "re-sending the residual left {remaining} outstanding, which is more than rounding"
    );
    Ok(())
}

#[test]
fn a_thin_book_partially_fills_an_order_a_normal_one_completes() -> Result<()> {
    let at = trade_time(&calm(121)?);
    let clip = dec!("6000");
    let full = calm(121)?.execute(&buy(clip), at)?;
    assert_eq!(full.status, FillStatus::Complete);

    let thin = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::Illiquidity {
            depth_fraction: 0.2,
        })),
        121,
    )?
    .execute(&buy(clip), at)?;
    assert_eq!(thin.status, FillStatus::Partial);
    assert!(thin.filled < full.filled);
    assert!(thin.residual.is_positive());
    Ok(())
}

// ------------------------------------------------------------------ slippage

#[test]
fn slippage_rises_monotonically_with_order_size() -> Result<()> {
    let sim = calm(131)?;
    let at = trade_time(&sim);
    let mut previous = f64::NEG_INFINITY;
    for units in [100i64, 250, 500, 900, 1_500, 3_000, 6_000, 9_000] {
        let report = sim.execute(&buy(Decimal::from_int(units)), at)?;
        let slippage = report
            .slippage_bps()
            .expect("a filled order has a slippage against its reference");
        assert!(
            slippage > previous,
            "slippage did not rise from {previous:.6}bp to {slippage:.6}bp when size grew to {units}"
        );
        previous = slippage;
    }
    Ok(())
}

#[test]
fn slippage_rises_monotonically_with_volatility() -> Result<()> {
    let at = trade_time(&calm(141)?);
    let mut previous = f64::NEG_INFINITY;
    for multiplier in [1.0, 1.5, 2.0, 4.0, 8.0, 16.0] {
        let sim = simulator(
            ConditionSchedule::new().with(ConditionWindow::always(
                MarketCondition::VolatilitySpike { multiplier },
            )),
            141,
        )?;
        let report = sim.execute(&buy(dec!("2000")), at)?;
        let slippage = report
            .slippage_bps()
            .expect("a filled order has a slippage against its reference");
        assert!(
            slippage > previous,
            "slippage did not rise from {previous:.6}bp to {slippage:.6}bp at {multiplier}x volatility"
        );
        previous = slippage;
    }
    Ok(())
}

#[test]
fn a_slippage_regime_multiplies_what_is_paid_beyond_the_reference() -> Result<()> {
    let at = trade_time(&calm(151)?);
    let base = calm(151)?.execute(&buy(dec!("2000")), at)?;
    let harsh = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::SlippageRegime {
            multiplier: 10.0,
        })),
        151,
    )?
    .execute(&buy(dec!("2000")), at)?;

    let base_bps = base.slippage_bps().unwrap_or_default();
    let harsh_bps = harsh.slippage_bps().unwrap_or_default();
    assert!(
        (harsh_bps - base_bps * 10.0).abs() < 1e-3,
        "a ten-times slippage regime moved {base_bps:.4}bp to {harsh_bps:.4}bp"
    );
    Ok(())
}

// ------------------------------------------------------------ the direction

#[test]
fn injecting_a_condition_never_improves_the_execution() -> Result<()> {
    // Adversity is the scalar in which worse is always larger: the filled part
    // contributes its slippage and the unfilled part contributes a full
    // percentage point per percent unfilled. The comparison is deliberately
    // not on the absolute price — a flash event moves the market, and buying
    // into a crash is cheaper in cash while being no better an execution.
    //
    // The tolerance absorbs the last bit of fixed-point rounding on a book
    // whose level is displaced but whose shape in basis points is not.
    const TOLERANCE_BPS: f64 = 1e-6;

    for case in 0..96u64 {
        let seed = 0xADD1_0000 ^ case;
        let mut rng = Xoshiro256::seeded(seed);
        let schedule = chaotic_schedule(&mut rng, start());
        let names: Vec<String> = schedule
            .windows()
            .iter()
            .map(|window| window.condition.as_str().to_string())
            .collect();

        let baseline_sim = calm(seed)?;
        let at = trade_time(&baseline_sim);
        let order = buy(dec!("2500"));
        let baseline = baseline_sim.execute(&order, at)?;
        let conditioned = simulator(schedule, seed)?.execute(&order, at)?;

        assert!(
            conditioned.adversity_bps() >= baseline.adversity_bps() - TOLERANCE_BPS,
            "case {case}: {names:?} improved the execution from {:.6}bp to {:.6}bp\n  baseline: {}\n  conditioned: {}",
            baseline.adversity_bps(),
            conditioned.adversity_bps(),
            baseline.summarise(),
            conditioned.summarise()
        );
        assert!(
            conditioned.filled <= baseline.filled,
            "case {case}: {names:?} supplied more liquidity than the calm market"
        );
        assert!(
            conditioned.arrived_at >= baseline.arrived_at,
            "case {case}: {names:?} made an order arrive earlier than it would have"
        );
    }
    Ok(())
}

#[test]
fn latency_only_ever_pushes_an_order_later() -> Result<()> {
    let sim = simulator(
        ConditionSchedule::new()
            .with(ConditionWindow::always(MarketCondition::Latency {
                base: Duration::from_millis(3),
                jitter: Duration::from_millis(2),
            }))
            .with(ConditionWindow::always(MarketCondition::LatencySpike {
                probability: 0.25,
                spike: Duration::from_millis(40),
            })),
        161,
    )?;
    for at in sim.steps() {
        let report = sim.execute(&buy(dec!("500")), *at)?;
        assert!(report.latency.as_nanos() >= Duration::from_millis(3).as_nanos());
        assert!(report.arrived_at >= report.submitted_at);
        assert_eq!(report.arrived_at, at.saturating_add(report.latency));
    }
    Ok(())
}

#[test]
fn a_condition_that_would_make_the_market_kinder_is_refused() {
    let cases = [
        MarketCondition::SpreadRegime { multiplier: 0.5 },
        MarketCondition::SlippageRegime { multiplier: 0.9 },
        MarketCondition::VolatilitySpike { multiplier: 0.0 },
        MarketCondition::Illiquidity {
            depth_fraction: 1.5,
        },
        MarketCondition::PartialFillCap { fraction: 2.0 },
        MarketCondition::CrossedMarket { by_bps: -3.0 },
        MarketCondition::DelayedFeed {
            delay: Duration::from_secs(-5),
        },
    ];
    for condition in cases {
        assert!(
            condition.validate().is_err(),
            "{} was accepted even though it improves the market",
            condition.as_str()
        );
    }
}

#[test]
fn conditions_compose_at_least_as_adversely_as_any_one_of_them() -> Result<()> {
    let at = trade_time(&calm(171)?);
    let order = buy(dec!("4000"));
    let spread = MarketCondition::SpreadRegime { multiplier: 4.0 };
    let thin = MarketCondition::Illiquidity {
        depth_fraction: 0.3,
    };

    let one = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(spread)),
        171,
    )?
    .execute(&order, at)?;
    let other = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(thin)),
        171,
    )?
    .execute(&order, at)?;
    let both = simulator(
        ConditionSchedule::new()
            .with(ConditionWindow::always(spread))
            .with(ConditionWindow::always(thin)),
        171,
    )?
    .execute(&order, at)?;

    assert!(both.adversity_bps() >= one.adversity_bps());
    assert!(both.adversity_bps() >= other.adversity_bps());
    Ok(())
}

// ------------------------------------------------------------------- replay

fn recorded_bars(days: usize) -> Vec<Bar> {
    (0..days)
        .map(|day| {
            let open_time = start().saturating_add(Duration::from_days(day as i64));
            let close = 100.0 + (day as f64) * 0.5;
            let open = close - 0.25;
            Bar {
                object_id: ObjectId::from_string(SYMBOL.to_string()),
                venue: VENUE.to_string(),
                interval: Interval::Day,
                open_time,
                open: Decimal::from_f64(open).unwrap_or(Decimal::ONE),
                high: Decimal::from_f64(close + 0.5).unwrap_or(Decimal::ONE),
                low: Decimal::from_f64(open - 0.5).unwrap_or(Decimal::ONE),
                close: Decimal::from_f64(close).unwrap_or(Decimal::ONE),
                volume: Decimal::from_int(1_000_000),
                vwap: None,
                trade_count: 1_000,
                quality: DataQuality::default(),
            }
        })
        .collect()
}

#[test]
fn a_historical_replay_prices_off_the_recorded_closes_and_is_deterministic() -> Result<()> {
    let build = || {
        MarketSimulator::replay(
            recorded_bars(30),
            vec![spec()],
            vec![VENUE.to_string()],
            0xBEEF,
        )
    };
    let sim = build()?;
    assert_eq!(sim.steps().len(), 30);

    // A bar is keyed on its close, so the price at the close of day three is
    // that day's close and not the next day's.
    let third_close = start().saturating_add(Duration::from_days(4));
    assert_eq!(
        sim.reference_price(SYMBOL, third_close),
        Some(dec!("101.5"))
    );
    assert!(
        sim.reference_price(SYMBOL, start()).is_none(),
        "a replay served a price before its first bar had closed"
    );

    let first = build()?.run(&mut MetronomeBuyer::new(dec!("500"), 3))?;
    let second = build()?.run(&mut MetronomeBuyer::new(dec!("500"), 3))?;
    assert_eq!(first.digest(), second.digest());
    assert!(!first.reports.is_empty());
    Ok(())
}

#[test]
fn a_replay_of_an_instrument_with_no_book_shape_is_refused_rather_than_guessed() {
    let other = InstrumentSpec::liquid("obj-zzz", dec!("50"));
    let refused =
        MarketSimulator::replay(recorded_bars(5), vec![other], vec![VENUE.to_string()], 1);
    assert!(
        refused.is_err(),
        "a replay invented a book for an instrument it had no shape for"
    );
}

#[test]
fn a_replay_can_be_put_through_the_same_conditions_as_a_synthetic_market() -> Result<()> {
    let schedule =
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::Illiquidity {
            depth_fraction: 0.1,
        }));
    let sim = MarketSimulator::replay(recorded_bars(20), vec![spec()], vec![VENUE.to_string()], 7)?
        .with_conditions(schedule)?;
    let at = sim.steps()[10];
    let report = sim.execute(&buy(dec!("20000")), at)?;
    assert_eq!(report.status, FillStatus::Partial);
    assert!(report.filled <= report.depth_available);
    Ok(())
}

// ----------------------------------------------------- the composed scenario

#[test]
fn a_strategy_can_be_run_through_a_flash_event_a_ten_times_slippage_regime_and_an_outage_on_leg_two()
-> Result<()> {
    // The scenario the whole module exists to make expressible, and the claim
    // that matters about it: it produces the same answer every time.
    let schedule = ConditionSchedule::new()
        .with(ConditionWindow::starting(
            MarketCondition::FlashEvent {
                magnitude: 0.12,
                down: Duration::from_secs(600),
                recovery: Duration::from_secs(900),
            },
            start().saturating_add(step() * 3),
        ))
        .with(ConditionWindow::always(MarketCondition::SlippageRegime {
            multiplier: 10.0,
        }))
        .with(
            ConditionWindow::always(MarketCondition::VenueOutage)
                .on_leg(1)
                .on_venue(VENUE),
        );

    let sim = simulator(schedule.clone(), 0x5CE7)?;
    let at = start().saturating_add(step() * 8);
    let plan = ExecutionPlan::new()
        .leg(buy(dec!("800")))
        .leg(buy(dec!("800")))
        .leg(buy(dec!("800")));

    let first = sim.execute_plan(&plan, at)?;
    let second = simulator(schedule, 0x5CE7)?.execute_plan(&plan, at)?;
    assert_eq!(first, second, "the composed scenario was not reproducible");

    assert_eq!(first.unreachable_legs(), vec![1]);
    assert_eq!(first.legs[1].residual, dec!("800"));
    assert!(!first.is_complete());
    assert!(
        first.legs[0].adversity_bps()
            > calm(0x5CE7)?
                .execute(&buy(dec!("800")), at)?
                .adversity_bps(),
        "the composed scenario was no worse than the calm market"
    );
    Ok(())
}

#[test]
fn a_run_reports_the_conditions_it_met_rather_than_burying_them() -> Result<()> {
    let schedule = ConditionSchedule::new()
        .with(ConditionWindow::always(MarketCondition::DelayedFeed {
            delay: Duration::from_secs(30),
        }))
        .with(ConditionWindow::always(MarketCondition::CrossedMarket {
            by_bps: 12.0,
        }));
    let run = simulator(schedule, 181)?.run(&mut MetronomeBuyer::new(dec!("600"), 5))?;

    assert!(run.stale_mark_steps > 0, "no step recorded a stale mark");
    assert!(
        run.crossed_market_steps > 0,
        "no step recorded a crossed market"
    );
    assert!(run.conditions.contains(&"delayed_feed".to_string()));
    assert!(run.conditions.contains(&"crossed_market".to_string()));
    assert!(run.summarise().contains("crossed"));
    Ok(())
}

// -------------------------------------- what the simulator declines to price

/// A book with one side is not a market this simulator will trade in.
///
/// Reachable, and it used to be the best execution in the suite. Compose
/// enough spread regimes and the widened half-spread puts the bid at a
/// negative price; those levels are skipped, the book comes back with offers
/// and no bids, and there is no mid. Everything the fill engine charges — the
/// spread crossed, the walk, the impact, the slippage multiplier over all
/// three — is a distance from that mid, so the fill used to be handed back at
/// the raw swept price with no reference against it: adversity 0.0bp, a
/// flawless execution, produced by injecting a spread thirty thousand times
/// the calm one.
#[test]
fn a_book_with_one_side_is_refused_rather_than_filled_against_nothing() -> Result<()> {
    let mut schedule = ConditionSchedule::new();
    for _ in 0..5 {
        schedule.push(ConditionWindow::always(MarketCondition::SpreadRegime {
            multiplier: 8.0,
        }));
    }
    let sim = simulator(schedule, 151)?;
    let at = trade_time(&sim);
    let book = sim.book_at(SYMBOL, VENUE, at, 0)?;
    assert_eq!(
        book.condition(),
        BookCondition::OneSided,
        "the premise no longer holds: this spread no longer removes a side"
    );
    assert_eq!(book.mid(), None);

    let order = buy(dec!("2000"));
    let report = sim.execute(&order, at)?;
    assert_eq!(report.status, FillStatus::Unpriceable);
    assert_eq!(report.filled, Decimal::ZERO);
    assert_eq!(report.residual, report.requested);
    assert_eq!(report.average_price(), None);
    assert_eq!(report.reference, None);

    let calm_report = calm(151)?.execute(&order, at)?;
    assert_eq!(calm_report.status, FillStatus::Complete);
    assert!(
        report.adversity_bps() > calm_report.adversity_bps(),
        "a spread regime of 32768x scored {:.6}bp against {:.6}bp in the calm market",
        report.adversity_bps(),
        calm_report.adversity_bps()
    );
    Ok(())
}

/// A synthetic path that overflows is refused, naming the step and the bound.
///
/// The generator used to reset the walk to the instrument's initial price when
/// a step overflowed and carry on, so a drift large enough to blow the path up
/// produced a series that quietly restarted from the spec's first number in
/// the middle — a price nobody generated, and one every fill priced off it
/// inherited. The comment above the reset said the code did not do this. It
/// now refuses, and this test is what keeps it refusing.
#[test]
fn a_synthetic_path_that_overflows_is_refused_rather_than_restarted_from_the_initial_price()
-> Result<()> {
    let mut shape = spec();
    // A per-step drift whose exponent alone overflows `f64`, so the second
    // point of the path cannot be a price whatever the seed draws.
    shape.step_drift = 800.0;
    shape.step_volatility = 0.0;
    shape.validate()?;

    // Premise: this walk does overflow. Without it, a refusal below could be
    // about something else entirely, and a pass would prove nothing.
    let first_step = shape.initial_price.to_f64() * shape.step_drift.exp();
    assert!(
        !first_step.is_finite(),
        "the premise no longer holds: a drift of {} per step leaves the price finite at {first_step}",
        shape.step_drift
    );

    let market = SyntheticMarket {
        start: start(),
        step: step(),
        steps: 4,
        venues: vec![VENUE.to_string()],
        instruments: vec![shape.clone()],
    };
    let error = match MarketSimulator::synthetic(market, 5) {
        Ok(_) => panic!("a path that overflowed was generated rather than refused"),
        Err(error) => error,
    };
    assert!(
        matches!(error, qip_core::error::Error::Invalid(_)),
        "{error}"
    );
    let message = error.message();
    assert!(
        message.contains(SYMBOL) && message.contains("step 1 of 4"),
        "the refusal must name the instrument and the step that overflowed: {message}"
    );
    assert!(
        message.contains(&format!("(0, {}]", Decimal::MAX)),
        "the refusal must name the bound the price left: {message}"
    );
    assert!(
        message.contains("refused rather than restarted"),
        "the refusal must say what the code used to do instead: {message}"
    );

    // And the same shape with a drift the range can hold generates, so the
    // refusal is about the overflow rather than about the parameters at all.
    shape.step_drift = 0.0;
    let calm = MarketSimulator::synthetic(
        SyntheticMarket {
            start: start(),
            step: step(),
            steps: 4,
            venues: vec![VENUE.to_string()],
            instruments: vec![shape],
        },
        5,
    )?;
    assert_eq!(calm.steps().len(), 4);
    Ok(())
}

/// The participation limit the cost model documents is the one the fill engine
/// enforces.
///
/// [`CostModel::cost_of`] refuses to quote an order past
/// `maximum_participation` — "the impact model is not calibrated that far and
/// would return a number rather than an answer". The fill engine reimplements
/// the same square-root law inline and inherited none of that: an order for
/// eighty per cent of a day's volume came back a complete fill at a
/// comfortable forty basis points. The two now agree, and this test is what
/// keeps them agreeing.
#[test]
fn a_fill_past_the_participation_the_cost_model_will_quote_is_refused_in_the_same_terms()
-> Result<()> {
    let mut shape = spec();
    // A book far deeper than the instrument's daily volume, which is the only
    // way a single fill reaches a large share of it.
    shape.daily_volume = 10_000.0;
    let market = SyntheticMarket {
        start: start(),
        step: step(),
        steps: 64,
        venues: vec![VENUE.to_string()],
        instruments: vec![shape.clone()],
    };
    let sim = MarketSimulator::synthetic(market, 5)?;
    let at = trade_time(&sim);
    let price = sim
        .reference_price(SYMBOL, at)
        .expect("the path has a price at the trade instant");
    let costs = CostModel::default();

    // Modest participation: both price it.
    let modest = dec!("500");
    assert!(
        costs
            .cost_of(modest, price, shape.daily_volume, shape.step_volatility)
            .is_ok()
    );
    let filled = sim.execute(&buy(modest), at)?;
    assert_eq!(filled.status, FillStatus::Complete);

    // Past the limit: the cost model refuses, and so does the fill.
    let excessive = dec!("8000");
    assert!(
        matches!(
            costs.cost_of(excessive, price, shape.daily_volume, shape.step_volatility),
            Err(Unfillable::ExceedsParticipation { .. })
        ),
        "the premise no longer holds: the cost model now quotes this order"
    );
    let refused = sim.execute(&buy(excessive), at)?;
    assert_eq!(
        refused.status,
        FillStatus::Unpriceable,
        "the fill engine priced an order the cost model refuses to quote"
    );
    assert_eq!(refused.filled, Decimal::ZERO);
    assert_eq!(refused.residual, excessive);
    assert!(refused.depth_available.is_positive(), "the depth was there");
    assert_eq!(refused.slippage_bps(), None);
    Ok(())
}

/// A mark taken on a crossed book carries the cross and no price.
///
/// It used to carry the bid, tagged `OneSidedBook`. Nothing about a cross
/// makes a mark stale, so `current_price` served that bid as a *current*
/// price: a strategy reading the market got a number off a book that had just
/// finished saying it did not know its own price.
#[test]
fn a_mark_on_a_crossed_book_carries_the_cross_and_no_price() -> Result<()> {
    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::CrossedMarket {
            by_bps: 18.0,
        })),
        61,
    )?;
    let at = trade_time(&sim);
    let mark = sim.mark_at(SYMBOL, VENUE, at, 0);

    assert!(mark.is_crossed(), "the mark did not carry the cross");
    assert!(mark.crossed_by.is_some_and(Decimal::is_positive));
    assert_eq!(mark.condition, BookCondition::Crossed);
    assert_eq!(
        mark.price, None,
        "a price was published off an inverted touch"
    );
    assert_eq!(mark.current_price(), None);
    assert_eq!(
        mark.last_known_price(),
        None,
        "the inverted touch was still reachable as a last known value"
    );
    assert_eq!(mark.source, MarkSource::Unavailable);

    // A calm mark at the same instant still has one, so the absence is the
    // cross rather than the harness.
    assert!(calm(61)?.mark_at(SYMBOL, VENUE, at, 0).price.is_some());
    Ok(())
}

/// A cross that appears part way through a worked order stops it there.
#[test]
fn a_cross_appearing_mid_order_leaves_the_earlier_slices_filled_and_an_exact_residual() -> Result<()>
{
    let interval = Duration::from_secs(60);
    let sim = simulator(
        ConditionSchedule::new().with(ConditionWindow::starting(
            MarketCondition::CrossedMarket { by_bps: 30.0 },
            start().saturating_add(step() * 12),
        )),
        67,
    )?;
    let at = trade_time(&sim);
    let order = buy(dec!("2000")).worked_in(4, interval);
    let report = sim.execute(&order, at)?;

    assert_eq!(report.status, FillStatus::Partial);
    assert!(
        report.filled.is_positive(),
        "no slice filled before the cross"
    );
    assert_eq!(report.residual, order.quantity - report.filled);
    assert_eq!(report.slices.len(), 4);
    let traded: Vec<bool> = report
        .slices
        .iter()
        .map(|slice| slice.filled.is_positive())
        .collect();
    assert_eq!(
        traded,
        vec![true, true, false, false],
        "the cross did not stop the slices that met it"
    );
    Ok(())
}

/// A collapsed level rests exactly the depth it was left with.
///
/// [`displayed_size`] rounds a depth collapse *down* on purpose — rounding it
/// up hands back liquidity the condition removed. Splitting the level into two
/// resting orders then rounded it back up: a level too small to halve rested
/// two orders of a raw unit each, doubling a book the condition had all but
/// emptied.
#[test]
fn a_depth_collapse_is_never_rounded_back_up_by_the_queue_it_rests_in() -> Result<()> {
    let calm_book = calm(71)?.book_at(SYMBOL, VENUE, trade_time(&calm(71)?), 0)?;
    let calm_depth = calm_book.depth(Side::Sell);
    assert!(calm_depth.is_positive());

    for depth_fraction in [0.5, 0.1, 0.01, 1e-9, 1e-12] {
        let sim = simulator(
            ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::Illiquidity {
                depth_fraction,
            })),
            71,
        )?;
        let at = trade_time(&sim);
        let book = sim.book_at(SYMBOL, VENUE, at, 0)?;
        for side in [Side::Buy, Side::Sell] {
            let collapsed = book.depth(side);
            let ceiling = Decimal::from_f64(calm_depth.to_f64() * depth_fraction)
                .expect("the collapsed depth is representable");
            assert!(
                collapsed <= ceiling,
                "a {depth_fraction} depth collapse left {collapsed} showing against a ceiling of {ceiling}"
            );
        }
    }
    Ok(())
}

// ------------------------------------------------- what a run is marked at

/// A flash event still in progress cannot improve a run's profit and loss.
///
/// The run used to mark its closing position at the *undisturbed* path price
/// while the fills happened at the crashed one, so a strategy that bought
/// into a crash booked the whole displacement as profit: the same run that
/// scored a calm P&L of about fifteen thousand scored ninety-six thousand
/// once a twenty-five per cent flash event was injected under it. The mark now
/// comes off the book the conditions actually left.
#[test]
fn a_flash_event_still_in_progress_cannot_improve_a_runs_profit_and_loss() -> Result<()> {
    let flash = MarketCondition::FlashEvent {
        magnitude: 0.25,
        // Long enough that the run ends with the price still in the hole.
        down: Duration::from_secs(60 * 60),
        recovery: Duration::from_secs(60 * 60 * 24),
    };
    let schedule = ConditionSchedule::new().with(ConditionWindow::starting(
        flash,
        start().saturating_add(step() * 2),
    ));

    let calm_run = calm(9)?.run(&mut MetronomeBuyer::new(dec!("400"), 4))?;
    let flash_run = simulator(schedule, 9)?.run(&mut MetronomeBuyer::new(dec!("400"), 4))?;

    assert_eq!(
        calm_run.positions, flash_run.positions,
        "the two runs did not end up holding the same thing, so their P&Ls are not comparable"
    );
    assert!(
        flash_run.cash > calm_run.cash,
        "the premise no longer holds: buying through the crash was not cheaper in cash"
    );
    let calm_mark = calm_run.final_marks[SYMBOL];
    let flash_mark = flash_run.final_marks[SYMBOL];
    assert!(
        flash_mark < calm_mark,
        "the closing mark ignored the flash event: {flash_mark} against {calm_mark}"
    );
    assert!(
        flash_run.profit_and_loss < calm_run.profit_and_loss,
        "a flash event improved the run's P&L from {} to {}",
        calm_run.profit_and_loss,
        flash_run.profit_and_loss
    );
    assert!(flash_run.unmarked_positions.is_empty());
    Ok(())
}

/// A position nobody can price is reported, not folded into the P&L.
#[test]
fn a_position_no_venue_can_mark_is_named_rather_than_valued() -> Result<()> {
    let schedule =
        ConditionSchedule::new().with(ConditionWindow::always(MarketCondition::CrossedMarket {
            by_bps: 20.0,
        }));
    // Every book crossed at every venue for the whole run: the buyer fills
    // nothing, so there is no position, and there is nothing to report.
    let run = simulator(schedule, 83)?.run(&mut MetronomeBuyer::new(dec!("400"), 4))?;
    assert!(run.positions.values().all(|quantity| quantity.is_zero()));
    assert!(run.unmarked_positions.is_empty());
    assert!(run.final_marks.is_empty(), "a crossed book supplied a mark");
    assert!(
        !run.reports.is_empty()
            && run
                .reports
                .iter()
                .all(|report| report.status == FillStatus::CrossedBook)
    );
    assert_eq!(run.profit_and_loss, run.cash);
    Ok(())
}

// ------------------------------------------- the direction, for worked orders

/// A worked order is measured against the market each slice met, not against a
/// reference frozen at arrival.
///
/// The order above executes in one slice, so its arrival mid and its fill's
/// mid are the same instant and the distinction never shows. Work it over
/// several minutes and they come apart: a flash event drops the price between
/// the slices, the later ones print far below the arrival mid, and the
/// shortfall against that mid goes deeply negative. Adversity was computed
/// from it, so the worst crash in the schedule scored as the best execution in
/// the run — several hundred basis points *better* than the calm market.
///
/// The fill is worse in every way that is actually about the fill: wider
/// spread, thinner book, more paid over the mid it traded into. That is what
/// adversity now measures, and `slippage_bps` keeps the shortfall — an honest
/// number about the trade, which is not the same question.
#[test]
fn a_flash_event_cannot_flatter_a_worked_order() -> Result<()> {
    let schedule = ConditionSchedule::new().with(ConditionWindow::starting(
        MarketCondition::FlashEvent {
            magnitude: 0.25,
            down: Duration::from_secs(60 * 30),
            recovery: Duration::from_secs(60 * 60 * 6),
        },
        start().saturating_add(step() * 4),
    ));
    let order = buy(dec!("1200")).worked_in(6, Duration::from_secs(60));
    let at = trade_time(&calm(97)?);

    let baseline = calm(97)?.execute(&order, at)?;
    let crashed = simulator(schedule, 97)?.execute(&order, at)?;

    assert!(baseline.filled.is_positive() && crashed.filled.is_positive());
    // The premise: the flash event really did make the cash cheaper, and the
    // shortfall against the arrival mid really did go negative on it.
    assert!(
        crashed.average_price() < baseline.average_price(),
        "the premise no longer holds: buying through the crash was not cheaper"
    );
    assert!(
        crashed.slippage_bps().unwrap_or_default() < 0.0,
        "the premise no longer holds: the shortfall against arrival did not go negative"
    );

    // And the claim: none of that is an improvement in the execution.
    assert!(
        crashed.adversity_bps() >= baseline.adversity_bps(),
        "a flash event improved a worked order from {:.6}bp to {:.6}bp\n  baseline: {}\n  crashed:  {}",
        baseline.adversity_bps(),
        crashed.adversity_bps(),
        baseline.summarise(),
        crashed.summarise()
    );
    assert!(
        crashed.execution_cost_bps().unwrap_or_default()
            > baseline.execution_cost_bps().unwrap_or_default(),
        "the crash cost no more against the books it actually traded into"
    );
    Ok(())
}

/// The same direction property as above, over generated schedules, but for the
/// orders the single-slice case cannot reach: worked orders, sells, and orders
/// carrying a limit.
///
/// Worth its own test rather than a widening of the one above, because these
/// are the orders whose fills happen at instants the arrival snapshot knows
/// nothing about — which is exactly where a condition found room to flatter.
#[test]
fn injecting_a_condition_never_improves_a_worked_or_a_sold_execution() -> Result<()> {
    const TOLERANCE_BPS: f64 = 1e-6;

    for case in 0..96u64 {
        let seed = 0x_C0DE_0000u64 ^ case;
        let mut rng = Xoshiro256::seeded(seed);
        let schedule = chaotic_schedule(&mut rng, start());
        let names: Vec<String> = schedule
            .windows()
            .iter()
            .map(|window| window.condition.as_str().to_string())
            .collect();

        let side = if case % 2 == 0 { Side::Buy } else { Side::Sell };
        let mut order = SimOrder::market(SYMBOL, VENUE, side, dec!("1500"))
            .worked_in(1 + (case % 4) as usize, Duration::from_secs(60));
        if case % 5 == 0 {
            // Marketable in the calm market by a wide margin, so the limit is
            // not itself what stops the fill.
            order = order.with_limit(match side {
                Side::Buy => dec!("160"),
                Side::Sell => dec!("40"),
            });
        }

        let baseline_sim = calm(seed)?;
        let at = trade_time(&baseline_sim);
        let baseline = baseline_sim.execute(&order, at)?;
        let conditioned = simulator(schedule, seed)?.execute(&order, at)?;

        assert!(
            conditioned.adversity_bps() >= baseline.adversity_bps() - TOLERANCE_BPS,
            "case {case}: {names:?} improved a {} of {} slice(s) from {:.6}bp to {:.6}bp\n  baseline:    {}\n  conditioned: {}",
            side.as_str(),
            order.slices,
            baseline.adversity_bps(),
            conditioned.adversity_bps(),
            baseline.summarise(),
            conditioned.summarise()
        );
        assert!(
            conditioned.filled <= baseline.filled,
            "case {case}: {names:?} supplied more liquidity than the calm market"
        );

        // And the invariant the measure rests on: anything that filled can be
        // costed. A fill the simulator cannot measure is one it should not
        // have made.
        for report in [&baseline, &conditioned] {
            assert_eq!(
                report.filled.is_positive(),
                report.execution_cost_bps().is_some(),
                "case {case}: a fill with no cost anyone can state: {}",
                report.summarise()
            );
        }
    }
    Ok(())
}
