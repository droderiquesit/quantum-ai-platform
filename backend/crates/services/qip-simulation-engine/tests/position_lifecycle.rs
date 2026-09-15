//! Blueprint §35.1's position lifecycle, as a run actually moves it.
//!
//! The backtester is the one production seam in this workspace that holds a
//! [`qip_portfolio::portfolio::Portfolio`] of positions *and* a strategy's
//! live statement of what it intends to hold, so it is where "the thesis was
//! withdrawn" becomes a fact about a position rather than a number in a
//! weight map. Reached from the deep brain's evolution engine — locate the
//! call with `grep -n 'Backtester::new' backend/crates/apps/qip-deepbrain/src/evolution.rs`.
//!
//! The failure these prevent is the one §35 opens with: "a position could be
//! orphaned when its strategy retired, unwound arbitrarily when a user
//! reduced an allocation, or held indefinitely past the thesis that opened
//! it." Until this wiring existed the run's own record could not tell a
//! position the strategy still wanted from one it had stopped wanting, and
//! `Flagged` and `Unwinding` had no writer outside a test.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_market::bar::{Bar, Interval};
use qip_portfolio::lifecycle::PositionLifecycle;
use qip_simulation_engine::backtest::{
    BacktestConfig, BacktestResult, BacktestStrategy, Backtester,
};
use qip_simulation_engine::clock::{ExecutionAssumptions, PointInTimeView, SimulationClock};
use std::collections::BTreeMap;

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-AAA")
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    let instrument = FinancialObject::builder(
        object(),
        "AAA",
        InstrumentType::CommonStock,
        LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .provenance(Provenance::synthetic("test", start()))
    .build(start())
    .expect("the fixture instrument is well formed");
    universe
        .insert(instrument)
        .expect("the universe accepts it");
    universe
}

fn bar(day: i64, close: f64, volume: f64) -> Bar {
    let open_time = start().saturating_add(Duration::from_days(day));
    let open = close * 0.999;
    Bar {
        object_id: object(),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time,
        open: Decimal::from_f64(open).expect("the open is representable"),
        high: Decimal::from_f64(close.max(open) * 1.003).expect("the high is representable"),
        low: Decimal::from_f64(close.min(open) * 0.997).expect("the low is representable"),
        close: Decimal::from_f64(close).expect("the close is representable"),
        volume: Decimal::from_f64(volume).expect("the volume is representable"),
        trade_count: 1_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }
}

/// Holds one name at `weight` for its first `decisions_held` decisions and
/// then states a weight of exactly zero for it.
///
/// Zero rather than dropping the key, because
/// `BacktestStrategy::target_weights` documents those as different
/// instructions: an empty map is "hold what you have" and a zero weight is
/// "go to cash". Only the second withdraws the thesis, and only the second
/// may move the lifecycle.
struct ExitsAfter {
    weight: f64,
    decisions_held: usize,
    decisions: usize,
}

impl BacktestStrategy for ExitsAfter {
    fn name(&self) -> &str {
        "exits-after"
    }

    fn target_weights(&mut self, _view: &PointInTimeView<'_>) -> BTreeMap<String, f64> {
        self.decisions += 1;
        let weight = if self.decisions <= self.decisions_held {
            self.weight
        } else {
            0.0
        };
        BTreeMap::from([(object().as_str().to_string(), weight)])
    }
}

fn run(config: BacktestConfig, bars: Vec<Bar>, decisions_held: usize) -> Result<BacktestResult> {
    let mut clock = SimulationClock::new(bars, ExecutionAssumptions::next_bar())?;
    let mut strategy = ExitsAfter {
        weight: 0.5,
        decisions_held,
        decisions: 0,
    };
    Backtester::new(config)?.run(&mut strategy, &mut clock, &universe())
}

/// What the run says became of the one name it traded.
fn lifecycle_of(result: &BacktestResult) -> PositionLifecycle {
    *result
        .position_lifecycle
        .get(object().as_str())
        .expect("the run reports the lifecycle of every position it touched")
}

#[test]
fn a_strategy_that_takes_a_name_to_cash_leaves_the_position_recorded_as_closed() -> Result<()> {
    // Ten decisions at half weight, then cash, over a flat, liquid market
    // where the flattening order can actually be sent.
    let bars: Vec<Bar> = (0..20).map(|i| bar(i, 100.0, 5_000_000.0)).collect();
    let result = run(BacktestConfig::default(), bars, 10)?;

    // Premise: the run opened the position and then sold it. A test that only
    // asserted the final state would pass on a run that never traded at all.
    assert!(
        result.fills.iter().any(|fill| fill.quantity.is_positive()),
        "the premise is that the strategy bought the name: {:?}",
        result.fills
    );
    assert!(
        result.fills.iter().any(|fill| fill.quantity.is_negative()),
        "the premise is that the exit order was sent: {:?}",
        result.fills
    );
    // The deliberate close went through `Flagged` and `Unwinding` on the way.
    // The table has no `Held -> Unwinding` edge, so had the concern not been
    // raised first the unwind would have been refused, the refusal would be
    // sitting in `rejected` and the sell above would never have been sent.
    // (The one rejection this run does produce is the last decision's
    // execution lag running past the end of the data, which is unrelated.)
    assert!(
        !result
            .rejected
            .iter()
            .any(|order| order.reason.contains("position lifecycle")),
        "no order may be refused by the lifecycle table on this path: {:?}",
        result.rejected
    );
    assert_eq!(lifecycle_of(&result), PositionLifecycle::Closed);
    Ok(())
}

#[test]
fn a_position_whose_exit_order_is_too_small_to_send_is_left_flagged_rather_than_quietly_held()
-> Result<()> {
    // Blueprint §35.2's third question: "What catches a position held past its
    // thesis?" A holding worth less than one unit of currency is below the
    // backtester's own order threshold, so the run declines to trade it — and
    // before this wiring existed it then looked exactly like a position the
    // strategy still wanted. The price collapses on the same bar the strategy
    // goes to cash, so the flatten order is worth well under a currency unit.
    let mut bars: Vec<Bar> = (0..11).map(|i| bar(i, 100.0, 5_000_000.0)).collect();
    bars.extend((11..20).map(|i| bar(i, 0.1, 5_000_000.0)));
    let config = BacktestConfig {
        initial_capital: dec!("1000"),
        ..BacktestConfig::default()
    };
    let result = run(config, bars, 10)?;

    // Premise one: the strategy actually took the position.
    assert!(
        result.fills.iter().any(|fill| fill.quantity.is_positive()),
        "the premise is that the strategy bought the name: {:?}",
        result.fills
    );
    // Premise two: no exit order was ever sent, so what the assertion below
    // measures is a position the run left behind rather than one it closed.
    assert!(
        !result.fills.iter().any(|fill| fill.quantity.is_negative()),
        "the premise is that the exit was never sent: {:?}",
        result.fills
    );

    assert_eq!(
        lifecycle_of(&result),
        PositionLifecycle::Flagged,
        "a name the strategy put at zero weight and the run could not leave is \
         held past its thesis, and the record has to say so"
    );
    Ok(())
}

#[test]
fn a_flatten_order_the_market_cannot_absorb_leaves_the_position_recorded_as_unwinding() -> Result<()>
{
    // The exit is decided, sized and priced, and the cost model refuses it —
    // the market has gone to one share a day against the book. The position is
    // then neither held nor closed: the desk is trying to leave it. Recording
    // that is the difference between a position nobody has looked at and one
    // the platform has been unable to exit for nine sessions.
    let mut bars: Vec<Bar> = (0..11).map(|i| bar(i, 100.0, 5_000_000.0)).collect();
    bars.extend((11..20).map(|i| bar(i, 100.0, 1.0)));
    let config = BacktestConfig {
        impact_window: 1,
        ..BacktestConfig::default()
    };
    let result = run(config, bars, 10)?;

    // Premise one: the position was opened while the market was deep.
    assert!(
        result.fills.iter().any(|fill| fill.quantity.is_positive()),
        "the premise is that the strategy bought the name: {:?}",
        result.fills
    );
    // Premise two: the exit was sized and then refused, rather than never
    // being attempted.
    assert!(
        result
            .rejected
            .iter()
            .any(|order| order.quantity.is_negative()),
        "the premise is that a sell order was refused: {:?}",
        result.rejected
    );
    // Premise three: it never filled.
    assert!(
        !result.fills.iter().any(|fill| fill.quantity.is_negative()),
        "the premise is that no exit filled: {:?}",
        result.fills
    );

    assert_eq!(
        lifecycle_of(&result),
        PositionLifecycle::Unwinding,
        "a close the desk has decided on and cannot execute is unwinding, not held"
    );
    Ok(())
}
