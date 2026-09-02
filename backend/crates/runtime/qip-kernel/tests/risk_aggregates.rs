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

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::aggregate::{AggregateFigures, RiskAggregates};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use std::cell::RefCell;
use std::collections::BTreeMap;

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
                FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
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

/// The conservative default less its two share-of-gross concentration caps.
///
/// The seam tests below need a first order into an empty book admitted, and
/// a share-of-gross cap cannot grant that: the first position in any book is
/// the whole of gross, so `sector-concentration` and `country-concentration`
/// read 100% and refuse it. That was invisible while the kernel fed the
/// checks no axis — the buckets were empty and the caps could not fire —
/// which the bucket tests below now close. Every other default limit stays.
fn limits() -> LimitSet {
    let mut limits = LimitSet::conservative_default();
    limits
        .limits
        .retain(|limit| !matches!(limit.kind, LimitKind::MaxConcentration { .. }));
    limits
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
