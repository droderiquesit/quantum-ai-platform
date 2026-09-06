//! The kernel's risk check reads running counters, not the strategy set.
//!
//! `qip_risk::aggregate` holds a limit check O(1) in strategy count, and its
//! own test proves that for the lib's `check_aggregates`. That proof said
//! nothing about the kernel until this file: the platform's `risk_state`
//! used to rebuild the state from a walk over its lots, and the aggregate
//! the lib provided was consulted by nothing in production. These tests pin
//! the two halves of the seam — the read side consults the same fixed
//! figures at eight strategies and at five hundred and twelve, and the fill
//! side carries every desk fill into the counters the read side reads.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::intent::Contributor;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_contracts::wire::{FillRecord, FillShare};
use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::central::CellReport;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_mesh::delta::DeltaOrder;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_risk::aggregate::{AggregateFigures, RiskAggregates};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use std::cell::RefCell;
use std::collections::BTreeMap;

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

/// Wraps an aggregate and counts every figure the read side consults.
///
/// The two strategy-level accessors are on the trait, so a read side that
/// iterated strategies would have to go through here and be counted.
struct CountingProbe<'a> {
    inner: &'a RiskAggregates,
    reads: RefCell<BTreeMap<&'static str, usize>>,
}

impl<'a> CountingProbe<'a> {
    fn over(inner: &'a RiskAggregates) -> Self {
        Self {
            inner,
            reads: RefCell::new(BTreeMap::new()),
        }
    }

    fn note(&self, figure: &'static str) {
        *self.reads.borrow_mut().entry(figure).or_insert(0) += 1;
    }

    fn reads(&self) -> BTreeMap<&'static str, usize> {
        self.reads.borrow().clone()
    }
}

impl AggregateFigures for CountingProbe<'_> {
    fn equity(&self) -> Decimal {
        self.note("equity");
        self.inner.equity()
    }
    fn cash(&self) -> Decimal {
        self.note("cash");
        self.inner.cash()
    }
    fn gross_exposure(&self) -> Decimal {
        self.note("gross_exposure");
        self.inner.gross_exposure()
    }
    fn net_exposure(&self) -> Decimal {
        self.note("net_exposure");
        self.inner.net_exposure()
    }
    fn drawdown(&self) -> f64 {
        self.note("drawdown");
        self.inner.drawdown()
    }
    fn position_notionals(&self) -> &BTreeMap<String, Decimal> {
        self.note("position_notionals");
        self.inner.position_notionals()
    }
    fn axis_exposures(&self) -> &BTreeMap<String, BTreeMap<String, Decimal>> {
        self.note("axis_exposures");
        self.inner.axis_exposures()
    }
    fn strategies(&self) -> Vec<&str> {
        self.note("strategies");
        self.inner.strategies()
    }
    fn strategy_gross(&self, strategy: &str) -> Decimal {
        self.note("strategy_gross");
        self.inner.strategy_gross(strategy)
    }
}

const INSTRUMENTS: [&str; 4] = ["AAA", "BBB", "CCC", "DDD"];

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in INSTRUMENTS {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
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
    }
    universe
}

/// The conservative default, exactly as it ships.
///
/// This fixture used to strip `MaxConcentration`, because the seam tests
/// below need the first order into an empty book admitted and a
/// share-of-gross cap cannot grant that: the first position in any book is
/// the whole of gross, so the cap read 100% and refused it at every size.
///
/// ADR 0027 settled that by making the default caps a share of *equity*. The
/// `retain` is removed rather than kept as a no-op — `conservative_default`
/// holds no `MaxConcentration` today, so it stripped nothing while still
/// reading as though these tests ran against a reduced set. That is the
/// worse of the two failures: a real exemption is visible, a vestigial one
/// quietly becomes real again the day the limit returns.
fn limits() -> LimitSet {
    LimitSet::conservative_default()
}

/// A platform over `limits` with the given initial equity.
///
/// Equity is a parameter because the conservative default caps a single
/// name at ten percent of it, and the seam test below needs that ceiling
/// low enough to reach with orders the order-notional limit still admits.
fn platform_under(initial_equity: Decimal, limits: LimitSet) -> Result<Platform> {
    let config = PlatformConfig::default().with_initial_equity(initial_equity);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits)
}

fn platform(initial_equity: Decimal) -> Result<Platform> {
    platform_under(initial_equity, limits())
}

/// A desk order, submitted through the full control path.
fn buy(platform: &mut Platform, symbol: &str, shares: Decimal, tag: &str) -> Result<()> {
    let order = platform.order_from(
        object(symbol),
        Side::Buy,
        shares,
        dec!("100"),
        &format!("prop-{tag}"),
        vec![format!("hyp-{tag}")],
        start(),
    );
    platform.submit_order(order, start())
}

/// The sector bucket every fixture instrument belongs to, as the aggregate
/// holds it.
fn sector_bucket(platform: &Platform) -> Decimal {
    platform
        .risk_figures()
        .axis_exposures()
        .get("sector")
        .and_then(|buckets| buckets.get("information_technology"))
        .copied()
        .unwrap_or(Decimal::ZERO)
}

/// `limits()` plus a cap on the fixture's one sector at a tenth of equity.
///
/// Named `sector-bucket`, so the refusal below can be told from the
/// position-weight cap — which is also a tenth of equity, and which the
/// second order is kept under by being in a different name.
fn limits_with_sector_bucket_cap() -> LimitSet {
    limits().with(
        Limit::new(
            "sector-bucket",
            LimitKind::MaxBucketExposure {
                axis: "sector".into(),
                bucket: "information_technology".into(),
                limit: 0.10,
            },
        )
        .with_rationale("the fixture's one sector may not exceed a tenth of equity"),
    )
}

/// A book of `strategies` strategies over the same four instruments, with
/// equity scaled so leverage is identical at every size.
fn book(strategies: usize) -> RiskAggregates {
    let per_fill = dec!("10000");
    let fills = Decimal::from_int(strategies as i64 * INSTRUMENTS.len() as i64);
    let equity = per_fill * fills * dec!("2");
    let mut book = RiskAggregates::new(equity, equity).expect("non-negative equity");
    for strategy in 0..strategies {
        for instrument in INSTRUMENTS {
            book.apply_fill(
                &format!("strategy-{strategy:04}"),
                instrument,
                &BTreeMap::new(),
                per_fill,
            )
            .expect("a well-formed fill");
        }
    }
    book
}

#[test]
fn the_platforms_risk_state_consults_the_same_fixed_figures_at_eight_strategies_and_at_five_hundred_and_twelve()
-> Result<()> {
    let platform = platform(dec!("10000000"))?;
    let small = book(8);
    let large = book(512);

    // Premise: the two books really differ in strategy count and both carry
    // exposure, so a read side that iterated strategies would have something
    // to iterate and the equality below is not two empty maps agreeing.
    assert_eq!(small.strategies().len(), 8);
    assert_eq!(large.strategies().len(), 512);
    assert!(small.gross_exposure().is_positive());

    let probe_small = CountingProbe::over(&small);
    let probe_large = CountingProbe::over(&large);
    let state_small = platform.risk_state_from(&probe_small);
    let state_large = platform.risk_state_from(&probe_large);

    // Premise: the state was built from the figures, not from somewhere else
    // the probe could not see.
    assert_eq!(state_small.gross_exposure, small.gross_exposure());
    assert_eq!(state_large.gross_exposure, large.gross_exposure());
    let reads_small = probe_small.reads();
    assert!(
        !reads_small.is_empty(),
        "the read side consulted nothing, so nothing can be said about how much"
    );

    // The property: sixty-four times the strategies, the same reads, and
    // neither strategy-level accessor touched at all.
    assert_eq!(
        reads_small,
        probe_large.reads(),
        "the kernel's read side consulted a different set of figures at 512 strategies than at 8"
    );
    for accessor in ["strategies", "strategy_gross"] {
        assert!(
            !reads_small.contains_key(accessor),
            "the read side called {accessor}, which walks the strategy set"
        );
    }
    Ok(())
}

#[test]
fn a_desk_fill_is_carried_into_the_counters_the_risk_check_reads() -> Result<()> {
    // One million of equity puts the ten-percent position-weight ceiling at
    // a hundred thousand, inside the order-notional limit, so the ceiling
    // can be reached by two orders the platform admits one at a time.
    let mut platform = platform(dec!("1000000"))?;

    // Premise: nothing has been aggregated before the first fill.
    assert_eq!(platform.risk_figures().fills(), 0);
    assert!(platform.risk_figures().gross_exposure().is_zero());
    let opening_cash = platform.risk_figures().cash();

    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("900"),
        dec!("100"),
        "prop-aggregate",
        vec!["hyp-aggregate".to_string()],
        start(),
    );
    platform.submit_order(order, start())?;

    // Premise: the venue filled it, so there is a fill to have carried.
    let fills = platform.orders().fills();
    assert!(!fills.is_empty(), "the simulated venue filled nothing");
    let at_cost: Decimal = fills
        .iter()
        .map(|fill| fill.quantity * fill.price)
        .fold(Decimal::ZERO, |sum, notional| sum + notional);
    assert!(at_cost.is_positive());

    // The seam: the same fill the order manager records is the fill the
    // aggregate holds, to the cent, and the desk is the strategy it was
    // charged to. Cash is the ledger's — it paid the venue's costs — so it
    // fell by more than the notional alone.
    let figures = platform.risk_figures();
    assert_eq!(figures.fills(), fills.len() as u64);
    assert_eq!(figures.gross_exposure(), at_cost);
    assert_eq!(figures.net_exposure(), at_cost);
    assert_eq!(figures.strategy_gross("central-desk"), at_cost);
    assert!(
        figures.cash() < opening_cash - at_cost,
        "cash {} did not pay the fill's costs on top of its {at_cost} notional",
        figures.cash()
    );

    // And the limit check reads what was carried. A follow-on order in the
    // same name is sized from the fill the venue actually made — it filled
    // part of the order, at its own price — so that alone it sits under the
    // position-weight ceiling and breaches only when projected onto what
    // the first fill already holds. Premise first: the first fill really
    // sits under the ceiling, and the follow-on really does on its own, so
    // the refusal below is the sum and neither order alone.
    let ceiling = dec!("100000");
    assert!(
        at_cost < ceiling,
        "the first fill {at_cost} breached by itself"
    );
    let shares = (ceiling - at_cost)
        .checked_div(dec!("100"))
        .expect("a hundred is not zero")
        .truncate_dp(0)
        + Decimal::from_int(10);
    let follow_on = shares * dec!("100");
    assert!(
        follow_on < ceiling,
        "the follow-on {follow_on} breaches alone"
    );
    assert!(at_cost + follow_on > ceiling, "at cost {at_cost}");
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        shares,
        dec!("100"),
        "prop-aggregate-follow-on",
        vec!["hyp-aggregate".to_string()],
        start(),
    );
    let refused = platform.submit_order(order, start()).expect_err(
        "the second order was admitted, so the pre-trade check never saw the first fill",
    );
    assert!(
        refused.message().contains("position-weight:"),
        "the second order was refused for another reason: {}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_fill_is_charged_to_its_sector_bucket_and_an_order_that_would_overfill_the_bucket_is_refused()
-> Result<()> {
    // A tenth of a million is a hundred thousand: the bucket cap. The first
    // order fills most of it in one name; the second, in a different name of
    // the same sector, sits under every per-name limit on its own and
    // breaches only when projected onto the bucket the first fill already
    // holds — which is exactly the case an empty bucket could never refuse.
    let mut platform = platform_under(dec!("1000000"), limits_with_sector_bucket_cap())?;

    // Premise: nothing has been charged to any bucket before the first fill.
    assert!(platform.risk_figures().axis_exposures().is_empty());
    buy(&mut platform, "AAA", dec!("900"), "bucket-open")?;

    // Premise: the venue filled, and the fill reached the sector bucket, so
    // the refusal below is a bucket the aggregate carries and not an empty
    // map agreeing with a limit that reads zero.
    let fills = platform.orders().fills();
    assert!(!fills.is_empty(), "the simulated venue filled nothing");
    let at_cost: Decimal = fills
        .iter()
        .map(|fill| fill.quantity * fill.price)
        .fold(Decimal::ZERO, |sum, notional| sum + notional);
    let bucket = sector_bucket(&platform);
    assert!(
        bucket.is_positive(),
        "the fill was aggregated to no sector bucket"
    );
    assert_eq!(
        bucket, at_cost,
        "the bucket holds something other than the fill"
    );
    let ceiling = dec!("100000");
    assert!(
        bucket < ceiling,
        "the first fill {bucket} overfilled the bucket by itself"
    );

    // An order in another name that takes the bucket over, and only the
    // bucket: a hundred shares more than the room left, well under the
    // ten-percent per-name weight and the single-order notional cap.
    let room = (ceiling - bucket)
        .checked_div(dec!("100"))
        .expect("a hundred is not zero")
        .truncate_dp(0);
    let shares = room + Decimal::from_int(100);
    assert!(
        shares * dec!("100") < ceiling,
        "the follow-on breaches per-name limits alone"
    );
    let refused = buy(&mut platform, "BBB", shares, "bucket-over")
        .expect_err("the order was admitted, so the pre-trade check never saw the bucket");
    assert!(
        refused.message().contains("sector-bucket:"),
        "refused for another reason: {}",
        refused.message()
    );
    assert!(
        !refused.message().contains("position-weight:"),
        "the per-name cap fired too, so this run does not isolate the bucket: {}",
        refused.message()
    );
    // Nothing was charged for a refused order.
    assert_eq!(sector_bucket(&platform), at_cost);
    Ok(())
}

#[test]
fn an_order_that_keeps_its_sector_bucket_under_the_cap_is_admitted() -> Result<()> {
    let mut platform = platform_under(dec!("1000000"), limits_with_sector_bucket_cap())?;
    buy(&mut platform, "AAA", dec!("900"), "bucket-open")?;
    let opened = sector_bucket(&platform);
    // Premise: the bucket is live, so the admission below is a limit that
    // read a real figure and found it inside, not one that read nothing.
    assert!(
        opened.is_positive(),
        "the fill was aggregated to no sector bucket"
    );
    let ceiling = dec!("100000");
    assert!(opened < ceiling);

    // Fifty shares more in another name of the same sector: five thousand
    // against the room left, so the bucket ends under its cap.
    let shares = dec!("50");
    assert!(
        opened + shares * dec!("100") < ceiling,
        "the fixture would overfill the bucket"
    );
    buy(&mut platform, "BBB", shares, "bucket-under")?;
    let after = sector_bucket(&platform);
    assert!(
        after > opened,
        "the admitted order's fill was not charged to the bucket ({opened} before, {after} after)"
    );
    assert!(after < ceiling);
    Ok(())
}

const CELL: &str = "cell-lon-1";

/// One buy a cell shipped, netted from a single contributor on the buy side.
fn cell_buy(symbol: &str, shares: Decimal) -> DeltaOrder {
    let strategy = StrategyId::new("foundry-alpha");
    DeltaOrder {
        order_id: format!("cell-ord-{symbol}"),
        strategy: strategy.clone(),
        object_id: object(symbol),
        venue: VenueId::new("XNYS"),
        // A buy lifts the offer; the plane reads `Ask` as a buy.
        side: BookSide::Ask,
        quantity: shares,
        price: dec!("100"),
        simulated: true,
        contributors: vec![Contributor {
            strategy,
            signed_size: Decimal::ONE,
            inputs: vec![("alpha-feature".to_string(), 1)],
        }],
    }
}

/// The venue's report on [`cell_buy`]'s order, for `shares` of it.
fn cell_fill(symbol: &str, shares: Decimal) -> FillRecord {
    cell_fill_at(symbol, shares, "XNYS")
}

/// The name the desk's own simulated broker reports itself under, and so the
/// counterparty bucket every desk fill is charged to.
///
/// Taken from `qip_execution_engine::broker::SimulatedBroker`'s `name()`. It
/// is a literal here rather than a lookup because the point of the test below
/// is that the *cell's* venue and the *desk's* broker land in one bucket; a
/// fixture that derived the cell's venue from the platform could not tell that
/// case apart from the kernel charging every cell fill to the desk's broker,
/// which is the wrong fix for the same gap.
const DESK_VENUE: &str = "simulated-venue";

/// [`cell_fill`] against a named venue.
fn cell_fill_at(symbol: &str, shares: Decimal, venue: &str) -> FillRecord {
    FillRecord {
        order_id: format!("cell-ord-{symbol}"),
        object_id: object(symbol),
        venue: VenueId::new(venue),
        side: BookSide::Ask,
        quantity: shares,
        price: dec!("100"),
        simulated: true,
        at: start(),
        shares: vec![FillShare {
            strategy: StrategyId::new("foundry-alpha"),
            quantity: shares,
        }],
    }
}

#[test]
fn a_sent_order_the_venue_has_not_filled_charges_nothing_to_the_aggregate() -> Result<()> {
    // The defect: a report carrying a sent order and no fill was billed as
    // a fill of the order's whole size, so a resting order — or one that
    // expired unfilled — was charged into gross, moved a strategy book and
    // sat in the aggregate as a position nobody held. Premise first: the
    // report genuinely carries the order and no fill, and the cell is
    // charged nothing before it.
    let mut platform = platform(dec!("1000000"))?;
    buy(&mut platform, "AAA", dec!("100"), "desk-before")?;
    let desk_gross = platform.risk_figures().gross_exposure();
    let bucket_before = sector_bucket(&platform);
    assert!(platform.risk_figures().strategy_gross(CELL).is_zero());

    let report = CellReport::new(CELL, start()).with_orders(vec![cell_buy("BBB", dec!("16000"))]);
    assert_eq!(report.orders.len(), 1, "the premise is a sent order");
    assert!(
        report.fills.is_empty(),
        "the premise is that nothing filled"
    );
    let ingestion = platform.ingest_cell_report(report, start())?;

    assert_eq!(
        ingestion.settlement.orders_sent, 1,
        "the order was not registered as sent"
    );
    assert_eq!(ingestion.settlement.fills_settled, 0);
    assert!(
        ingestion.settlement.absorbed.is_empty(),
        "a sent order was absorbed as a fill"
    );
    assert!(
        ingestion.settlement.attribution.is_none(),
        "a sent order was attributed"
    );
    assert!(ingestion.halted.is_none(), "an open order is not a break");
    let figures = platform.risk_figures();
    assert!(
        figures.strategy_gross(CELL).is_zero(),
        "a resting order was charged to the cell's gross: {}",
        figures.strategy_gross(CELL)
    );
    assert_eq!(figures.gross_exposure(), desk_gross);
    assert_eq!(sector_bucket(&platform), bucket_before);
    assert_eq!(
        platform
            .telemetry()
            .metrics
            .snapshot()
            .counter_total(names::CENTRAL_ORDERS_SENT),
        1,
        "the sent order left no series behind it"
    );
    Ok(())
}

#[test]
fn the_same_order_filled_in_the_next_report_charges_exactly_the_fill() -> Result<()> {
    // Sixteen thousand sent in one report, six thousand of it filled in
    // the next. What the aggregate is charged is six hundred thousand — the
    // fill — and not the 1.6 million the order was sent for. Premise: the
    // first report charged nothing, so what moves below is the fill alone.
    let mut platform = platform(dec!("1000000"))?;
    let sent = dec!("16000");
    let filled = dec!("6000");
    assert!(filled < sent, "the fixture is a partial fill");
    let first = CellReport::new(CELL, start()).with_orders(vec![cell_buy("BBB", sent)]);
    platform.ingest_cell_report(first, start())?;
    assert!(platform.risk_figures().strategy_gross(CELL).is_zero());

    let second = CellReport::new(CELL, start()).with_fills(vec![cell_fill("BBB", filled)]);
    assert!(
        second.orders.is_empty(),
        "the order was sent in the earlier report"
    );
    let ingestion = platform.ingest_cell_report(second, start())?;
    assert!(
        ingestion.halted.is_none(),
        "{:?}",
        ingestion.settlement.breaks
    );
    assert_eq!(ingestion.settlement.fills_settled, 1);
    assert_eq!(ingestion.settlement.absorbed.len(), 1);
    assert_eq!(
        platform.risk_figures().strategy_gross(CELL),
        filled * dec!("100"),
        "the aggregate was charged something other than the fill"
    );
    Ok(())
}

#[test]
fn a_cells_fills_are_charged_into_the_aggregate_and_the_next_desk_order_is_refused_on_leverage()
-> Result<()> {
    // A million of equity under the default leverage cap of 1.5x. The desk
    // opens a small position first, so the premise "a desk order is admitted
    // against this book" is shown rather than assumed; then one cell reports
    // sixteen thousand shares at a hundred — 1.6 million, over the cap on its
    // own — and the same small desk order is refused.
    let mut platform = platform(dec!("1000000"))?;
    buy(&mut platform, "AAA", dec!("100"), "desk-before")?;
    let desk_gross = platform.risk_figures().gross_exposure();
    let desk_cash = platform.risk_figures().cash();
    let bucket_before = sector_bucket(&platform);
    // Premise: the desk fill is in the counters, and nothing is yet charged
    // to the cell — so what moves below is the cell's report and only that.
    assert!(
        desk_gross.is_positive(),
        "the desk's opening order did not fill"
    );
    assert!(platform.risk_figures().strategy_gross(CELL).is_zero());

    let shares = dec!("16000");
    let cell_notional = shares * dec!("100");
    // The order and the venue's confirmation of it, in one report: the fill
    // is what is charged, and it names the order beside it.
    let report = CellReport::new(CELL, start())
        .with_orders(vec![cell_buy("BBB", shares)])
        .with_fills(vec![cell_fill("BBB", shares)]);
    let ingestion = platform.ingest_cell_report(report, start())?;
    // Premise: the plane settled the fill, so there was something to charge.
    assert_eq!(ingestion.settlement.fills_settled, 1);
    assert_eq!(ingestion.settlement.absorbed.len(), 1);

    // The seam: the cell's fill is in the same counters the desk's is, under
    // the cell's id, in the instrument's sector bucket, and the desk's cash
    // is untouched by capital the desk never held.
    let figures = platform.risk_figures();
    assert_eq!(figures.strategy_gross(CELL), cell_notional);
    assert_eq!(figures.gross_exposure(), desk_gross + cell_notional);
    assert_eq!(sector_bucket(&platform), bucket_before + cell_notional);
    assert_eq!(figures.cash(), desk_cash);

    let refused = buy(&mut platform, "AAA", dec!("100"), "desk-after")
        .expect_err("the desk order was admitted, so the cell's fill never reached the check");
    assert!(
        refused.message().contains("leverage:"),
        "refused for another reason: {}",
        refused.message()
    );
    Ok(())
}

/// The counterparty bucket the cap reads, as the aggregate holds it, together
/// with the name it is filed under.
///
/// Returned as a pair rather than looked up by a literal so the assertions
/// below can check the *name* against the venue the fills actually came back
/// from. A test that hardcoded "simulated-venue" would keep passing if the
/// kernel started charging every fill to a counterparty nobody traded with.
fn counterparty_bucket(platform: &Platform) -> Option<(String, Decimal)> {
    platform
        .risk_figures()
        .axis_exposures()
        .get(qip_risk::limits::COUNTERPARTY_AXIS)
        .and_then(|buckets| buckets.iter().next())
        .map(|(name, value)| (name.clone(), *value))
}

/// `limits()` plus a cap on exposure to any one counterparty at a tenth of
/// equity.
///
/// Named `counterparty` so a refusal can be told from the `position-weight`
/// cap, which is also a tenth of equity and which the second order below is
/// kept under by being in a different name.
fn limits_with_counterparty_cap() -> LimitSet {
    limits().with(
        Limit::new(
            "counterparty",
            LimitKind::MaxCounterpartyExposure { limit: 0.10 },
        )
        .with_rationale("no single counterparty may hold more than a tenth of the book"),
    )
}

#[test]
fn a_fill_is_charged_to_the_venue_that_executed_it_and_an_order_that_would_overfill_that_counterparty_is_refused()
-> Result<()> {
    // `LimitKind::MaxCounterpartyExposure` read a `RiskState::counterparty_exposures`
    // map that no production code in this workspace ever wrote: the platform
    // passed `None` for the counterparty at the only kernel call site of
    // `OrderManager::submit`, and even had it named one, the sole writer —
    // `PreTradeChecker::project` — added one instrument's delta to a balance
    // that always started empty. So a deployment that configured a
    // counterparty cap got no `LimitBreach`, ever, while the cap counted in
    // `LimitCheck::evaluated` and read as a control that had run and passed.
    //
    // A tenth of a million is a hundred thousand: the cap. The first order
    // fills most of it; the second, in a different name, sits under every
    // per-name limit on its own and breaches only when projected onto the
    // counterparty balance the first fill already holds.
    let mut platform = platform_under(dec!("1000000"), limits_with_counterparty_cap())?;

    // Premise: nothing has been charged to any counterparty before the first
    // fill, so the refusal below cannot come from a bucket that was already
    // there.
    assert!(
        counterparty_bucket(&platform).is_none(),
        "a counterparty balance existed before any order was sent"
    );
    buy(&mut platform, "AAA", dec!("900"), "cp-open")?;

    // Premise: the venue filled, and the fill reached a counterparty bucket
    // named after the venue that reported it. Both halves matter — an empty
    // map and a map filed under the wrong name are the two ways this cap goes
    // back to never firing.
    let fills = platform.orders().fills();
    assert!(!fills.is_empty(), "the simulated venue filled nothing");
    let venue = fills[0].venue.clone();
    let at_cost: Decimal = fills
        .iter()
        .map(|fill| fill.quantity * fill.price)
        .fold(Decimal::ZERO, |sum, notional| sum + notional);
    let (name, balance) =
        counterparty_bucket(&platform).expect("the fill was aggregated to no counterparty");
    assert_eq!(
        name, venue,
        "the counterparty balance is filed under a name the venue never reported"
    );
    assert_eq!(
        balance, at_cost,
        "the counterparty balance holds something other than the fill"
    );
    let ceiling = dec!("100000");
    assert!(
        balance < ceiling,
        "the first fill {balance} overfilled the counterparty by itself"
    );

    // An order that takes the counterparty over, and only the counterparty: a
    // hundred shares more than the room left, well under the ten-percent
    // per-name weight and the single-order notional cap.
    let room = (ceiling - balance)
        .checked_div(dec!("100"))
        .expect("a hundred is not zero")
        .truncate_dp(0);
    let shares = room + Decimal::from_int(100);
    assert!(
        shares * dec!("100") < ceiling,
        "the follow-on breaches per-name limits alone"
    );
    let refused = buy(&mut platform, "BBB", shares, "cp-over")
        .expect_err("the order was admitted, so the pre-trade check never saw the counterparty");
    // Matched with the delimiter the refusal formats after a limit name, so
    // this cannot be satisfied by some other limit whose name merely contains
    // the word.
    assert!(
        refused.message().contains("counterparty:"),
        "refused for another reason: {}",
        refused.message()
    );
    assert!(
        !refused.message().contains("position-weight:"),
        "the per-name cap fired too, so this run does not isolate the counterparty: {}",
        refused.message()
    );
    // Nothing was charged for a refused order.
    assert_eq!(
        counterparty_bucket(&platform).map(|(_, value)| value),
        Some(at_cost)
    );
    Ok(())
}

#[test]
fn an_order_that_keeps_its_counterparty_under_the_cap_is_admitted() -> Result<()> {
    // The half that makes the refusal above mean something. A cap that
    // refuses everything is an outage, not a control, and a test that only
    // ever asserts a breach cannot tell the two apart — which matters more
    // here than usual, because a single-broker paper deployment routes the
    // whole book through one counterparty and a cap set too low would stop it
    // trading at all.
    let mut platform = platform_under(dec!("1000000"), limits_with_counterparty_cap())?;
    buy(&mut platform, "AAA", dec!("900"), "cp-open")?;
    let (_, opened) =
        counterparty_bucket(&platform).expect("the fill was aggregated to no counterparty");
    // Premise: the balance is live, so the admission below is a limit that
    // read a real figure and found it inside, not one that read nothing.
    assert!(opened.is_positive());
    let ceiling = dec!("100000");
    assert!(opened < ceiling);

    // Fifty shares more in another name: five thousand against the room left,
    // so the counterparty ends under its cap.
    let shares = dec!("50");
    assert!(
        opened + shares * dec!("100") < ceiling,
        "the follow-on would breach the cap, so an admission proves nothing"
    );
    buy(&mut platform, "BBB", shares, "cp-under")?;
    let (_, after) =
        counterparty_bucket(&platform).expect("the second fill was aggregated to no counterparty");
    assert!(
        after > opened,
        "the second fill did not reach the counterparty balance: {opened} then {after}"
    );
    assert!(
        after < ceiling,
        "the premise failed: the book ended over its own cap"
    );
    Ok(())
}

#[test]
fn a_cell_fill_is_charged_to_the_venue_that_executed_it_and_a_desk_order_over_the_cap_is_refused()
-> Result<()> {
    // The gap this closes, which was written into `Platform::aggregate_fill`
    // as a known one: the counterparty axis was fed only by the desk's own
    // executions, because `AbsorbedFill` named no venue. A book that also
    // trades through regional cells therefore held counterparty exposure that
    // the running balance did not carry, so `MaxCounterpartyExposure` read
    // low on it and admitted the next desk order against a number smaller
    // than the book. A limit that reads low is not a conservative limit; it
    // is a control that admits the order it exists to refuse.
    //
    // The cell here trades through the *same* counterparty the desk does,
    // which is the case the under-charge was invisible in: the bucket already
    // existed and simply held too little, so nothing on a dashboard and
    // nothing in a limit check looked wrong.
    let mut platform = platform_under(dec!("1000000"), limits_with_counterparty_cap())?;

    // Premise: nothing is charged to any counterparty before the cell
    // reports, so the balance below is the cell's fill and only that.
    assert!(
        counterparty_bucket(&platform).is_none(),
        "a counterparty balance existed before anything traded"
    );

    let shares = dec!("900");
    let cell_notional = shares * dec!("100");
    // Driven through `ingest_cell_report`, the seam a deployed centre uses,
    // with the order and the venue's confirmation of it in one report. The
    // limit evaluator is never called directly here: the point is that the
    // production path charges the bucket, not that the evaluator can read one.
    let report = CellReport::new(CELL, start())
        .with_orders(vec![cell_buy("BBB", shares)])
        .with_fills(vec![cell_fill_at("BBB", shares, DESK_VENUE)]);
    let ingestion = platform.ingest_cell_report(report, start())?;

    // Premise: the plane settled the fill, so there was something to charge,
    // and what it absorbed names the venue the cell reported it from.
    assert!(
        ingestion.halted.is_none(),
        "{:?}",
        ingestion.settlement.breaks
    );
    assert_eq!(ingestion.settlement.fills_settled, 1);
    assert_eq!(ingestion.settlement.absorbed.len(), 1);
    assert_eq!(
        ingestion.settlement.absorbed[0].venue, DESK_VENUE,
        "the settlement absorbed a fill naming a venue the report never carried"
    );

    // The bucket moved, filed under the venue the cell reported. Both halves
    // matter: an unmoved bucket is the old under-charge, and a bucket under
    // another name would be exposure booked against a counterparty that never
    // saw the trade.
    let (name, balance) =
        counterparty_bucket(&platform).expect("the cell's fill reached no counterparty bucket");
    assert_eq!(
        name, DESK_VENUE,
        "the cell's fill is filed under a name its report never mentioned"
    );
    assert_eq!(
        balance, cell_notional,
        "the counterparty balance holds something other than the cell's fill"
    );
    let ceiling = dec!("100000");
    assert!(
        balance < ceiling,
        "the cell's fill {balance} overfilled the counterparty by itself, so the refusal below \
         would not need the desk order at all"
    );

    // A desk order that takes the counterparty over, and only the
    // counterparty: a hundred shares more than the room the cell left, in a
    // name nothing has traded, so it sits under the ten-percent per-name
    // weight and under the single-order notional cap on its own.
    let room = (ceiling - balance)
        .checked_div(dec!("100"))
        .expect("a hundred is not zero")
        .truncate_dp(0);
    let desk_shares = room + Decimal::from_int(100);
    assert!(
        desk_shares * dec!("100") < ceiling,
        "the desk order breaches a per-name limit alone, so a refusal proves nothing"
    );
    let refused = buy(&mut platform, "AAA", desk_shares, "cell-cp-over").expect_err(
        "the desk order was admitted, so the cell's fill never reached the counterparty balance",
    );
    // Matched with the delimiter the refusal formats after a limit name, so
    // this cannot be satisfied by another limit whose name merely contains
    // the word.
    assert!(
        refused.message().contains("counterparty:"),
        "refused for another reason: {}",
        refused.message()
    );
    assert!(
        !refused.message().contains("position-weight:"),
        "the per-name cap fired too, so this run does not isolate the counterparty: {}",
        refused.message()
    );
    Ok(())
}

#[test]
fn a_desk_order_that_keeps_the_cells_counterparty_under_the_cap_is_still_admitted() -> Result<()> {
    // The other half, and it is not ceremony: charging cell fills to the
    // counterparty axis moves a number that every desk order is now measured
    // against, and a change that only ever tightens is indistinguishable from
    // one that refuses everything. A single-broker paper deployment routes
    // both the desk and its cells through one counterparty, so if this
    // charging were wrong by an order of magnitude the desk would simply stop
    // trading, and the test above would still pass.
    let mut platform = platform_under(dec!("1000000"), limits_with_counterparty_cap())?;

    let shares = dec!("900");
    let report = CellReport::new(CELL, start())
        .with_orders(vec![cell_buy("BBB", shares)])
        .with_fills(vec![cell_fill_at("BBB", shares, DESK_VENUE)]);
    platform.ingest_cell_report(report, start())?;
    let (_, charged) =
        counterparty_bucket(&platform).expect("the cell's fill reached no counterparty bucket");

    // Premise: the balance is live and inside its cap, so the admission below
    // is a limit that read a real figure and found room, not one that read
    // nothing.
    assert_eq!(charged, shares * dec!("100"));
    let ceiling = dec!("100000");
    assert!(charged.is_positive() && charged < ceiling);

    // Fifty shares in another name: five thousand against the ten thousand
    // the cell left, so the counterparty ends under its cap.
    let desk_shares = dec!("50");
    assert!(
        charged + desk_shares * dec!("100") < ceiling,
        "the desk order would breach the cap, so an admission proves nothing"
    );
    buy(&mut platform, "AAA", desk_shares, "cell-cp-under")?;

    let (_, after) =
        counterparty_bucket(&platform).expect("the desk's own fill reached no counterparty bucket");
    assert!(
        after > charged,
        "the desk's fill did not join the cell's in one bucket: {charged} then {after}"
    );
    assert!(
        after < ceiling,
        "the premise failed: the book ended over its own cap"
    );
    Ok(())
}

#[test]
fn a_cycle_publishes_the_books_value_and_leverage_from_the_state_the_monitor_ruled_on() -> Result<()>
{
    // `qip_portfolio_value` and `qip_portfolio_leverage` were declared in
    // `qip_observability::metrics::names` and recorded by nothing at all —
    // kept alive as fixtures for the exposition encoders while the kernel
    // marked both figures every cycle and published neither. A name in that
    // module reads as a series the platform publishes, so the two most basic
    // questions asked of a trading platform, how big the book is and how
    // levered it is, had no answer on any chart.
    let mut platform = platform(dec!("1000000"))?;

    // Premise: neither gauge exists before a cycle, so what is read below was
    // recorded by this pass rather than at assembly.
    let before = platform.telemetry().metrics.snapshot();
    assert!(
        before.gauge(names::PORTFOLIO_VALUE, &labels([])).is_none(),
        "the book's value was already on a gauge before any cycle ran"
    );
    assert!(
        before
            .gauge(names::PORTFOLIO_LEVERAGE, &labels([]))
            .is_none()
    );

    // A real position first. A book with no exposure is levered zero however
    // the quotient is computed, so a leverage gauge asserted on an empty book
    // would survive a recording site that published a constant.
    buy(&mut platform, "AAA", dec!("900"), "gauge")?;
    let equity = platform.risk_figures().equity();
    let gross = platform.risk_figures().gross_exposure();
    assert!(
        gross.is_positive(),
        "the simulated venue filled nothing, so leverage is zero either way"
    );
    assert_ne!(
        gross, equity,
        "gross and equity are equal on this fixture, so a gauge fed from the wrong one would \
         read correct"
    );

    platform.run_cycle(start());

    // Premise: the cycle itself traded nothing, so the state ACT ruled on is
    // the state still standing here and the comparison is against one book.
    assert_eq!(platform.risk_figures().equity(), equity);
    assert_eq!(platform.risk_figures().gross_exposure(), gross);

    let after = platform.telemetry().metrics.snapshot();
    assert_eq!(
        after.gauge(names::PORTFOLIO_VALUE, &labels([])),
        Some(equity.to_f64()),
        "the book's value on the gauge is not the equity the limits were evaluated against"
    );
    // Crossed and then divided, in that order, because that is what
    // `qip_risk::limits::RiskState::ratio` does for `LimitKind::MaxLeverage`:
    // a gauge that divided as `Decimal` and crossed after would disagree with
    // the control in the last place.
    let expected = gross.to_f64() / equity.to_f64();
    assert!(
        expected > 0.0 && expected < 1.0,
        "the fixture's leverage is {expected}, which is 0 or 1 and so indistinguishable from a \
         constant"
    );
    assert_eq!(
        after.gauge(names::PORTFOLIO_LEVERAGE, &labels([])),
        Some(expected),
        "the leverage gauge is not gross over equity"
    );
    Ok(())
}
