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
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::feasibility::{GATE_LOT, GATE_TICK};
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_observability::metrics::{Snapshot, labels, names};
use qip_risk::limits::LimitSet;

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
            FinancialObject::builder(object("AAA"), "AAA", InstrumentType::CommonStock)
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
            FinancialObject::builder(object("BBB"), "BBB", InstrumentType::CommonStock)
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
