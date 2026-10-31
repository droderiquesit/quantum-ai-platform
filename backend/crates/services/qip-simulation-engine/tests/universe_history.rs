//! R12-TICK-2090: no hidden survivorship in a historical universe. A
//! backtest over a window that ends before an instrument was delisted trades
//! it as the member it then was; one that runs past the delisting refuses it
//! from that day; and a venue outage overlapping the run is reported on the
//! result rather than read as a quiet market.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::expect_used)]

use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::history::{UniverseHistory, VenueOutage};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_market::bar::{Bar, Interval};
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

/// Holds one name at half weight on every decision.
struct AlwaysHolds;

impl BacktestStrategy for AlwaysHolds {
    fn name(&self) -> &str {
        "always-holds"
    }

    fn target_weights(&mut self, _view: &PointInTimeView<'_>) -> BTreeMap<String, f64> {
        BTreeMap::from([(object().as_str().to_string(), 0.5)])
    }
}

fn run_over(days: i64, history: &UniverseHistory) -> Result<BacktestResult> {
    let bars = (0..days)
        .map(|day| bar(day, 100.0 + day as f64, 1_000_000.0))
        .collect();
    let mut clock = SimulationClock::new(bars, ExecutionAssumptions::next_bar())?;
    Backtester::new(BacktestConfig::default())?.run_with_history(
        &mut AlwaysHolds,
        &mut clock,
        &universe(),
        history,
    )
}

/// Listed at the start, delisted on day 20, its venue dark on days 5-7.
fn history() -> Result<UniverseHistory> {
    let mut history = UniverseHistory::new();
    history.list(object(), "AAA", start())?;
    history.delist(&object(), start().saturating_add(Duration::from_days(20)))?;
    history.record_outage(VenueOutage {
        venue: "XNYS".to_string(),
        from: start().saturating_add(Duration::from_days(5)),
        until: start().saturating_add(Duration::from_days(8)),
        reason: "exchange-wide halt".to_string(),
    })?;
    Ok(history)
}

/// Mutations, each fails here: drop the membership refusal in
/// `Backtester::rebalance` (the run past the delisting fills after day 20);
/// report no outages from `run_inner` (the outage is missing); make
/// `UniverseHistory::delist` remove the listing instead of dating it (the
/// pre-delisting window is not a member and refuses everything).
#[test]
fn a_backtest_over_a_past_date_includes_an_instrument_delisted_later_and_reports_the_outage()
-> Result<()> {
    let history = history()?;
    // The history's own answer first: a member before the delisting.
    let early = start().saturating_add(Duration::from_days(10));
    assert!(
        history
            .members_as_of(early)
            .iter()
            .any(|listing| listing.object_id == object()),
        "premise: members_as_of a date before the delisting includes the instrument"
    );

    // A window wholly before the delisting trades the instrument.
    let before = run_over(15, &history)?;
    assert!(
        !before.fills.is_empty(),
        "a member before its delisting is traded"
    );
    assert!(
        before
            .rejected
            .iter()
            .all(|order| !order.reason.contains("not listed")),
        "nothing is refused for membership before the delisting: {:?}",
        before.rejected
    );
    assert_eq!(
        before.outages.len(),
        1,
        "the outage on days 5-7 overlaps the run and is reported"
    );
    assert_eq!(before.outages[0].reason, "exchange-wide halt");

    // A window running past the delisting refuses it from that day on.
    let delisted = start().saturating_add(Duration::from_days(20));
    let across = run_over(30, &history)?;
    assert!(
        across.fills.iter().all(|fill| fill.at < delisted),
        "a fill after the delisting traded an instrument that no longer existed"
    );
    assert!(
        across
            .rejected
            .iter()
            .any(|order| order.at >= delisted && order.reason.contains("not listed")),
        "the run says it refused the delisted instrument: {:?}",
        across.rejected
    );
    Ok(())
}
