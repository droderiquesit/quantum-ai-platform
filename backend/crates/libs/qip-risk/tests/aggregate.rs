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

// --- the strategy-level gate, checked before netting ------------------------
//
// | Rule | Pass fixture | Veto fixture |
// |---|---|---|
// | strategy budget (`admit_contribution`) | `a_contribution_inside_the_strategy_budget_is_admitted` | `a_contribution_beyond_the_strategy_budget_is_dropped_whole` |
// | a budget of nothing admits nothing | `a_contribution_inside_the_strategy_budget_is_admitted` | `a_budget_that_admits_nothing_refuses_even_a_flat_strategy` |
// | a contribution is non-zero | `a_contribution_inside_the_strategy_budget_is_admitted` | `a_contribution_of_nothing_is_not_a_contribution` |
// | a fill names its strategy, instrument and a non-zero notional | `a_well_formed_fill_is_charged` | `a_fill_the_aggregate_cannot_charge_is_refused` |
// | a mark carries a comparable drawdown | `a_mark_inside_the_unit_interval_is_recorded` | `a_mark_with_a_drawdown_the_halt_cannot_compare_is_refused` |
// | equity is non-negative at open | `a_well_formed_fill_is_charged` | `an_aggregate_cannot_open_over_negative_equity` |

#[test]
fn a_contribution_inside_the_strategy_budget_is_admitted() {
    let book = book(2);
    // Premise: the strategy holds something, so the budget has a figure to
    // compare, and the same rule binds when the budget is tightened — which
    // is the proof it read that figure.
    assert_eq!(book.strategy_gross("strategy-0000"), dec!("40000"));
    assert!(
        book.admit_contribution("strategy-0000", dec!("10000"), dec!("45000"))
            .is_err()
    );

    book.admit_contribution("strategy-0000", dec!("10000"), dec!("60000"))
        .expect("a contribution inside the budget");
}

#[test]
fn a_contribution_beyond_the_strategy_budget_is_dropped_whole() {
    let book = book(2);
    // Premise: the same rule admits the same contribution under a wider
    // budget, so it is not refusing everything.
    book.admit_contribution("strategy-0000", dec!("10000"), dec!("60000"))
        .expect("premise: a wider budget admits it");

    let refused = book
        .admit_contribution("strategy-0000", dec!("10000"), dec!("45000"))
        .expect_err("a contribution past the budget was admitted");
    assert_eq!(refused.code(), "denied");
    assert!(
        refused.message().contains("strategy-0000") && refused.message().contains("dropped"),
        "{refused}"
    );
    // A sale is gross too: a strategy cannot get under its budget by
    // contributing the other way.
    assert!(
        book.admit_contribution("strategy-0000", dec!("-10000"), dec!("45000"))
            .is_err()
    );
}

#[test]
fn a_budget_that_admits_nothing_refuses_even_a_flat_strategy() {
    let book = book(1);
    // Premise: the strategy is flat, so only the ceiling can refuse it.
    assert_eq!(book.strategy_gross("never-traded"), Decimal::ZERO);
    book.admit_contribution("never-traded", dec!("1"), dec!("1"))
        .expect("premise: a positive budget admits a flat strategy");
    for budget in [Decimal::ZERO, dec!("-1")] {
        let refused = book
            .admit_contribution("never-traded", dec!("1"), budget)
            .expect_err("a budget of nothing admitted a contribution");
        assert_eq!(refused.code(), "denied");
    }
}

#[test]
fn a_contribution_of_nothing_is_not_a_contribution() {
    let book = book(1);
    // Premise: a real contribution under the same budget is admitted, so
    // the refusal below is the zero rule and not the ceiling.
    book.admit_contribution("strategy-0000", dec!("1"), dec!("100000"))
        .expect("premise");
    let refused = book
        .admit_contribution("strategy-0000", Decimal::ZERO, dec!("100000"))
        .expect_err("a contribution of nothing passed the gate");
    assert_eq!(refused.code(), "invalid");
}

#[test]
fn a_well_formed_fill_is_charged() {
    let mut book = RiskAggregates::new(dec!("100000"), dec!("100000")).expect("open");
    book.apply_fill("alpha", "AAA", &axes("AAA"), dec!("5000"))
        .expect("a well-formed fill");
    assert_eq!(book.fills(), 1);
    assert_eq!(book.strategy_gross("alpha"), dec!("5000"));
}

#[test]
fn a_fill_the_aggregate_cannot_charge_is_refused() {
    let mut book = RiskAggregates::new(dec!("100000"), dec!("100000")).expect("open");
    // Premise: the same fill with every field present is accepted.
    book.apply_fill("alpha", "AAA", &axes("AAA"), dec!("5000"))
        .expect("premise");
    let before = book.clone();

    for (strategy, instrument, notional) in [
        ("", "AAA", dec!("5000")),
        ("alpha", " ", dec!("5000")),
        ("alpha", "AAA", Decimal::ZERO),
    ] {
        let refused = book
            .apply_fill(strategy, instrument, &axes("AAA"), notional)
            .expect_err("a fill with a missing field was charged");
        assert_eq!(refused.code(), "invalid");
    }
    assert_eq!(book, before, "a refused fill changed a counter");
}

#[test]
fn a_mark_inside_the_unit_interval_is_recorded() {
    let mut book = RiskAggregates::new(dec!("100000"), dec!("100000")).expect("open");
    book.mark(dec!("90000"), 0.1)
        .expect("a comparable drawdown");
    assert_eq!(book.equity(), dec!("90000"));
    assert!((book.drawdown() - 0.1).abs() < 1e-12);
}

#[test]
fn a_cash_mark_replaces_the_cash_the_fills_implied_and_may_go_negative() {
    let mut book = RiskAggregates::new(dec!("100000"), dec!("100000")).expect("open");
    book.apply_fill("alpha", "AAA", &axes("AAA"), dec!("5000"))
        .expect("a well-formed fill");
    // Premise: the fill moved cash by its notional, so the mark below is
    // overriding a figure and not filling an empty one.
    assert_eq!(book.cash(), dec!("95000"));

    // The ledger paid a fee the fill never told the aggregate about.
    book.mark_cash(dec!("94990"));
    assert_eq!(book.cash(), dec!("94990"));

    // A margined book is a negative figure the buffer limit exists to see,
    // so the mark records it rather than refusing it.
    book.mark_cash(dec!("-250"));
    assert_eq!(book.cash(), dec!("-250"));
}

#[test]
fn a_mark_with_a_drawdown_the_halt_cannot_compare_is_refused() {
    let mut book = RiskAggregates::new(dec!("100000"), dec!("100000")).expect("open");
    book.mark(dec!("90000"), 0.1)
        .expect("premise: a comparable mark");
    for drawdown in [f64::NAN, f64::INFINITY, -0.1, 1.5] {
        assert!(
            book.mark(dec!("90000"), drawdown).is_err(),
            "a drawdown of {drawdown} was recorded"
        );
    }
    assert!(book.mark(dec!("-1"), 0.1).is_err());
    assert!(
        (book.drawdown() - 0.1).abs() < 1e-12,
        "a refused mark moved the figure"
    );
}

#[test]
fn an_aggregate_cannot_open_over_negative_equity() {
    assert!(RiskAggregates::new(Decimal::ZERO, Decimal::ZERO).is_ok());
    assert!(RiskAggregates::new(dec!("-1"), Decimal::ZERO).is_err());
}
