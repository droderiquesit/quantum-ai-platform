//! The O(1)-in-strategy-count property of the aggregate risk check.
//!
//! Blueprint rule 11: risk reads aggregates, never strategy lists. The test
//! that holds it counts reads rather than timing them — a timing test passes
//! on a fast machine with a slow algorithm, and fails on a slow machine with
//! a fast one — by wrapping the aggregate in a probe that records every
//! figure the check consults.

use qip_core::{Decimal, dec};
use qip_risk::aggregate::{AggregateFigures, RiskAggregates};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use std::cell::RefCell;
use std::collections::BTreeMap;

/// Records every figure the check reads, by accessor name.
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

fn axes(instrument: &str) -> BTreeMap<String, String> {
    let sector = if instrument < "CCC" {
        "technology"
    } else {
        "energy"
    };
    BTreeMap::from([("sector".to_string(), sector.to_string())])
}

/// A book of `strategies` strategies over the same four instruments. Equity
/// scales with the strategy count so leverage is identical at every size and
/// the check's *outcome* cannot be what distinguishes the two runs.
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
                &axes(instrument),
                per_fill,
            )
            .expect("a well-formed fill");
        }
    }
    book
}

fn limits() -> LimitSet {
    LimitSet::conservative_default()
        .with(Limit::new(
            "net-exposure",
            LimitKind::MaxNetExposure { limit: 1.0 },
        ))
        .with(Limit::new(
            "position-notional",
            LimitKind::MaxPositionNotional {
                limit: dec!("100000000"),
            },
        ))
}

#[test]
fn the_aggregate_check_reads_the_same_fixed_figures_at_eight_strategies_and_at_five_hundred_and_twelve()
 {
    let small = book(8);
    let large = book(512);

    // Premise: the two books really differ in strategy count and both carry
    // exposure, so a check that iterated strategies would have something to
    // iterate and the equality below is not two empty maps agreeing.
    assert_eq!(small.strategies().len(), 8);
    assert_eq!(large.strategies().len(), 512);
    assert!(small.gross_exposure().is_positive());
    assert_eq!(large.gross_exposure(), small.gross_exposure() * dec!("64"));

    let probe_small = CountingProbe::over(&small);
    let probe_large = CountingProbe::over(&large);
    let returns = [0.001, -0.002, 0.003, -0.001];
    let check_small = limits().check_aggregates(&probe_small, &returns);
    let check_large = limits().check_aggregates(&probe_large, &returns);

    // Premise: the check evaluated limits and consulted the aggregate at all.
    assert!(check_small.evaluated > 0);
    let reads_small = probe_small.reads();
    let reads_large = probe_large.reads();
    assert!(
        !reads_small.is_empty(),
        "the check consulted nothing, so nothing can be said about how much"
    );
    assert_eq!(
        check_small.is_blocked(),
        check_large.is_blocked(),
        "the books are scaled to the same leverage, so the outcome must agree"
    );

    // The property. Sixty-four times the strategies, the same reads.
    assert_eq!(
        reads_small, reads_large,
        "the aggregate check consulted a different set of figures at 512 strategies than at 8"
    );
    for accessor in ["strategies", "strategy_gross"] {
        assert!(
            !reads_large.contains_key(accessor),
            "the aggregate check read `{accessor}`, which is a walk over the strategy set: \
             {reads_large:?}"
        );
    }
    // And each book-level figure is read once, not once per limit.
    for (figure, count) in &reads_large {
        assert_eq!(*count, 1, "`{figure}` was read {count} times");
    }
}

#[test]
fn the_incremental_counters_agree_with_a_full_recount() {
    // The premise the O(1) test rests on: the counters the check reads are
    // the right numbers. An incremental aggregate that drifted from the truth
    // would be fast and wrong, which is worse than slow and right.
    let mut book = RiskAggregates::new(dec!("1000000"), dec!("1000000")).expect("open");
    let fills: Vec<(&str, &str, Decimal)> = vec![
        ("alpha", "AAA", dec!("50000")),
        ("beta", "AAA", dec!("-20000")),
        ("alpha", "BBB", dec!("-30000")),
        ("beta", "CCC", dec!("10000")),
        ("alpha", "AAA", dec!("-60000")),
        ("gamma", "DDD", dec!("15000")),
    ];
    for (strategy, instrument, notional) in &fills {
        book.apply_fill(strategy, instrument, &axes(instrument), *notional)
            .expect("fill");
    }
    assert_eq!(book.fills(), 6, "premise: every fill was applied");

    // Recount from the fills themselves.
    let mut positions: BTreeMap<&str, Decimal> = BTreeMap::new();
    let mut per_strategy: BTreeMap<&str, BTreeMap<&str, Decimal>> = BTreeMap::new();
    for (strategy, instrument, notional) in &fills {
        *positions.entry(instrument).or_insert(Decimal::ZERO) += *notional;
        *per_strategy
            .entry(strategy)
            .or_default()
            .entry(instrument)
            .or_insert(Decimal::ZERO) += *notional;
    }
    let gross: Decimal = positions.values().map(|p| p.abs()).sum();
    let net: Decimal = positions.values().copied().sum();
    assert_eq!(book.gross_exposure(), gross);
    assert_eq!(book.net_exposure(), net);
    assert_eq!(book.cash(), dec!("1000000") - net);
    for (instrument, position) in &positions {
        assert_eq!(book.position_notionals()[*instrument], *position);
    }
    for (strategy, contributions) in &per_strategy {
        let expected: Decimal = contributions.values().map(|c| c.abs()).sum();
        assert_eq!(book.strategy_gross(strategy), expected, "{strategy}");
    }
    let technology: Decimal = positions
        .iter()
        .filter(|(i, _)| **i < "CCC")
        .map(|(_, p)| p.abs())
        .sum();
    assert_eq!(book.axis_exposures()["sector"]["technology"], technology);
}
