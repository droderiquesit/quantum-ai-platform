//! The central feasibility gate, installed: an order the venue could not
//! express is refused on the central path before any control downstream
//! treats its size or price as real (blueprint §18.1, rule 23).
//!
//! `qip_execution_engine::feasibility` has carried the refusal since it was
//! written, and until this suite's fixture existed the kernel constructed its
//! order manager bare — so the gate was a module nothing reached, and every
//! off-lot or off-tick order rode the kill switch, the autonomy gate and
//! pre-trade risk to the venue. These tests drive orders through the same
//! `OrderManager` the platform holds, so what they prove is the wiring and
//! not the module.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::ids::OrderId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::feasibility::{GATE_LOT, GATE_TICK};
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_observability::metrics::{Snapshot, labels, names};
use qip_risk::limits::LimitSet;

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

/// Two listings with different grids, so a test can tell the catalogue's
/// own lot from a constant: `AAA` states nothing and so carries
/// `qip_financial`'s builder default of one lot and a hundredth of a tick;
/// `BBB` states a board lot of a hundred and a five-cent tick.
fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object("AAA"),
                "AAA",
                InstrumentType::CommonStock,
                fixture_liquidity(),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    universe
        .insert(
            FinancialObject::builder(
                object("BBB"),
                "BBB",
                InstrumentType::CommonStock,
                fixture_liquidity(),
            )
            .venue("XTKS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .lot_size(dec!("100"))
            .tick_size(dec!("0.05"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    universe
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(),
        LimitSet::conservative_default(),
    )
}

fn recorded(platform: &Platform) -> Snapshot {
    platform.telemetry().metrics.snapshot()
}

/// A traceable market order, through the platform's own constructor.
fn market(platform: &mut Platform, symbol: &str, quantity: Decimal) -> Order {
    platform.order_from(
        object(symbol),
        Side::Buy,
        quantity,
        dec!("100"),
        "prop-feasibility",
        vec!["hyp-feasibility".to_string()],
        start(),
    )
}

/// A traceable limit order at `price`, which is the only order type that
/// states a price for the tick rule to judge.
fn limit(symbol: &str, quantity: Decimal, price: Decimal, id: &str) -> Order {
    Order::new(
        OrderId::from_string(id),
        object(symbol),
        Side::Buy,
        quantity,
        OrderType::Limit { price },
        dec!("100"),
        "prop-feasibility",
        vec!["hyp-feasibility".to_string()],
        "platform",
        start(),
    )
}

fn refused_under(snapshot: &Snapshot, gate: &str) -> u64 {
    snapshot.counter(names::ORDERS_REFUSED, &labels([("control", gate)]))
}

/// A flat daily bar at `level`, for the twin to price a refusal against.
fn bar(symbol: &str, at: Timestamp, level: f64) -> SensedRecord {
    let price = Decimal::from_f64(level).expect("a price");
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: price,
        high: Decimal::from_f64(level * 1.002).expect("a price"),
        low: Decimal::from_f64(level * 0.998).expect("a price"),
        close: price,
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Some(price),
        quality: DataQuality::default(),
    }))
}

/// `before` flat days ending at `start()`, then `after` flat days past it —
/// enough history for the twin's liquidity view and a close past its horizon.
fn flat_tape(symbol: &str, before: usize, after: i64) -> Vec<SensedRecord> {
    (0..before)
        .map(|i| {
            bar(
                symbol,
                start().saturating_sub(Duration::from_days((before - i) as i64)),
                100.0,
            )
        })
        .chain((1..=after).map(|day| {
            bar(
                symbol,
                start().saturating_add(Duration::from_days(day)),
                100.0,
            )
        }))
        .collect()
}

#[test]
fn a_desk_feasibility_refusal_is_attributed_to_the_desk_venue_and_counted_by_constraint()
-> Result<()> {
    // The failure this guards: blueprint §12.3's fourth row — "feasibility
    // rejections cluster on one venue" — had no key. A feasibility veto was
    // counted under its gate (`qip_orders_refused_total{control}`) and
    // carried to the twin without a venue, so a window of them could not
    // say *where* they clustered. The venue is the desk's one broker, read
    // from the same `Broker::name` the accepted arm's `result.venue` is
    // filled from, so a refusal and a fill on the same order can never be
    // charged to different venues.
    let mut platform = platform()?;
    platform.observe(flat_tape("AAA", 90, 5));
    let off_grid = market(&mut platform, "AAA", dec!("10.5"));
    platform
        .submit_order(off_grid, start())
        .expect_err("ten and a half shares of a one-lot listing reached the venue");

    // Premise: the refusal is queued for the twin, once, and the window
    // holds it with its venue and gate.
    assert_eq!(platform.declined_awaiting_score(), 1);
    let window = platform.feasibility_refusals();
    assert_eq!(window.len(), 1, "the refusal did not reach the window");
    assert_eq!(window[0].venue, "simulated-venue");
    assert_eq!(window[0].constraint, GATE_LOT);
    let snapshot = recorded(&platform);
    assert_eq!(
        snapshot.counter(
            names::FEASIBILITY_REFUSALS,
            &labels([("venue", "simulated-venue"), ("constraint", GATE_LOT)])
        ),
        1,
        "the refusal is not counted by venue and constraint: {:?}",
        snapshot
            .series
            .iter()
            .filter(|s| s.name == names::FEASIBILITY_REFUSALS)
            .map(|s| s.labels.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        snapshot.counter_total(names::FEASIBILITY_REFUSALS),
        1,
        "the refusal was counted under a second label set"
    );

    // And the venue survives scoring, so the twin's record of the refusal
    // says where it was refused and not only by what.
    platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let scores = platform.declined_scores();
    assert_eq!(scores.len(), 1, "the refusal was not priced");
    assert_eq!(scores[0].gate, GATE_LOT);
    assert_eq!(
        scores[0].venue.as_deref(),
        Some("simulated-venue"),
        "the priced refusal lost its venue"
    );

    // A posture refusal names no venue: an untraceable order is about the
    // order, not about where it was going, and attributing it to the desk's
    // broker would put a venue in the window that no feasibility gate
    // refused.
    let untraceable = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("10"),
        dec!("100"),
        "prop-untraceable",
        Vec::new(),
        start(),
    );
    platform
        .submit_order(untraceable, start())
        .expect_err("an untraceable order was accepted");
    assert_eq!(
        platform.feasibility_refusals().len(),
        1,
        "a posture refusal was admitted to the feasibility window"
    );
    Ok(())
}

#[test]
fn an_order_off_the_lot_grid_is_refused_by_the_lot_gate_and_one_on_it_is_admitted() -> Result<()> {
    let mut platform = platform()?;

    // The premise: an on-grid order in the same name is admitted and fills,
    // so the refusal below is the grid and not some other control that
    // would have refused any order at all.
    let on_grid = market(&mut platform, "AAA", dec!("1000"));
    platform.submit_order(on_grid, start())?;
    assert!(
        !platform.orders().fills().is_empty(),
        "the on-grid order did not fill, so nothing below is about the grid"
    );

    let off_grid = market(&mut platform, "AAA", dec!("10.5"));
    let error = platform
        .submit_order(off_grid, start())
        .expect_err("ten and a half shares of a one-lot listing reached the venue");
    assert!(
        error
            .message()
            .contains(&format!("infeasible ({GATE_LOT}):")),
        "refused for another reason than the lot grid: {}",
        error.message()
    );
    assert!(
        error.message().contains("refused rather than rounded"),
        "the refusal does not say the size was not silently corrected: {}",
        error.message()
    );

    // Counted under the gate literal the edge plane charts its own vetoes
    // under, and not under the bar an untraceable order lands on: an
    // operator reading `order-validation` climbing cannot tell a sizer that
    // lost the grid from a strategy that lost its hypothesis.
    let snapshot = recorded(&platform);
    assert_eq!(refused_under(&snapshot, GATE_LOT), 1);
    assert_eq!(
        refused_under(&snapshot, "order-validation"),
        0,
        "the feasibility veto was charged to the generic validation bar"
    );
    assert_eq!(
        snapshot.counter_total(names::ORDERS_SUBMITTED),
        1,
        "only the on-grid order may have reached the venue"
    );
    Ok(())
}

#[test]
fn the_lot_the_gate_judges_against_is_the_catalogue_record_and_not_a_constant() -> Result<()> {
    // `BBB` states a board lot of a hundred. Two hundred and fifty shares is
    // a whole number of the default lot of one — so a gate built from a
    // constant would admit it — and is not a whole number of hundreds.
    let mut platform = platform()?;

    let off_board_lot = market(&mut platform, "BBB", dec!("250"));
    let error = platform
        .submit_order(off_board_lot, start())
        .expect_err("two and a half board lots reached the venue");
    assert!(
        error
            .message()
            .contains(&format!("infeasible ({GATE_LOT}):"))
            && error.message().contains("lots of 100"),
        "the gate did not judge against the record's board lot: {}",
        error.message()
    );

    let three_board_lots = market(&mut platform, "BBB", dec!("300"));
    platform.submit_order(three_board_lots, start())?;
    assert_eq!(
        recorded(&platform).counter_total(names::ORDERS_SUBMITTED),
        1,
        "the on-grid order was not the one that reached the venue"
    );
    Ok(())
}

#[test]
fn a_limit_price_off_the_catalogue_tick_is_refused_by_the_tick_gate() -> Result<()> {
    let mut platform = platform()?;

    // 100.003 is off a hundredth; 100.01 is on it. Both are on `AAA`'s lot
    // grid and both are traceable, so the tick rule is the one deciding.
    let off_tick = limit("AAA", dec!("10"), dec!("100.003"), "ord-off-tick");
    let error = platform
        .submit_order(off_tick, start())
        .expect_err("a price between two ticks reached the venue");
    assert!(
        error
            .message()
            .contains(&format!("infeasible ({GATE_TICK}):")),
        "refused for another reason than the tick grid: {}",
        error.message()
    );
    assert_eq!(refused_under(&recorded(&platform), GATE_TICK), 1);

    let on_tick = limit("AAA", dec!("10"), dec!("100.01"), "ord-on-tick");
    platform.submit_order(on_tick, start())?;
    Ok(())
}
