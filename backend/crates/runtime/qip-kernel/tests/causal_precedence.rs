//! The second, real writer of a `CausalEdge` (§9.2): the UNDERSTAND stage's
//! temporal-precedence pass, driven by `Platform::observe`'s own bar history
//! rather than `qip_world_model::world::seed_demo_world`'s synthetic seed.
//!
//! The refusal test — two instruments whose returns share no lag structure —
//! is the one that matters most: it proves the production path does not
//! write a causal edge on noise, which is exactly the false-positive shape
//! the establishment method's own significance bar exists to keep out of the
//! graph a decision might later read.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_world_model::causal::{ConditionStanding, Mechanism};

fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> qip_core::ObjectId {
    qip_core::ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe(symbols: &[&str]) -> Universe {
    let mut universe = Universe::new();
    for symbol in symbols {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    *symbol,
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

fn limits() -> LimitSet {
    LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn platform(symbols: &[&str]) -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(symbols),
        limits(),
    )
}

fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).expect("a price"),
        high: Decimal::from_f64(open.max(close) * 1.002).expect("a price"),
        low: Decimal::from_f64(open.min(close) * 0.998).expect("a price"),
        close: Decimal::from_f64(close).expect("a price"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: DataQuality::default(),
    }))
}

/// Bars for `symbol` whose log returns follow `returns` exactly, starting
/// from a price of 100 and stepping one day per observation, oldest first.
fn bars_from_returns(symbol: &str, returns: &[f64], count: usize) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..count)
        .map(|i| {
            let open = price;
            price *= returns[i].exp();
            let at = start().saturating_sub(Duration::from_days((count - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// A deterministic, bounded pseudo-random sequence in roughly `[-scale,
/// scale]` — no crate randomness needed for a fixture this small, and
/// deterministic means a failure reproduces byte-for-byte.
fn noise(seed: u64, count: usize, scale: f64) -> Vec<f64> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            // A small xorshift, not a real generator — good enough for a
            // fixture's noise floor, not used anywhere near a decision.
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let unit = (state % 1_000_000) as f64 / 1_000_000.0;
            (unit - 0.5) * 2.0 * scale
        })
        .collect()
}

#[test]
fn a_real_lagged_pair_of_instruments_produces_a_temporal_precedence_edge() -> Result<()> {
    let count = 120;
    let cause_returns = noise(11, count, 0.02);
    let mut effect_returns = vec![0.0; count];
    let effect_noise = noise(22, count, 0.004);
    for t in 1..count {
        effect_returns[t] = 0.8 * cause_returns[t - 1] + effect_noise[t];
    }

    let mut platform = platform(&["AAA", "BBB"])?;
    platform.observe(bars_from_returns("AAA", &cause_returns, count));
    platform.observe(bars_from_returns("BBB", &effect_returns, count));

    // Premise: no causal claim exists before the cycle runs. Without this a
    // pass that discovered nothing would look identical to one that worked.
    assert_eq!(
        platform.world().causal().len(),
        0,
        "premise: an empty graph"
    );

    let report = platform.run_cycle(start());
    let understood = report
        .stage(Stage::Understand)
        .expect("UNDERSTAND always runs");
    assert!(
        understood.detail.contains("temporal-precedence pass"),
        "the stage detail does not name the pass it ran: {}",
        understood.detail
    );

    let edges = platform.world().causal().edges().to_vec();
    assert!(
        !edges.is_empty(),
        "a real, strongly lagged relationship over {count} points produced no edge: {}",
        understood.detail
    );
    let edge = edges
        .iter()
        .find(|e| e.cause == "obj-AAA" && e.effect == "obj-BBB")
        .unwrap_or_else(|| panic!("no edge from obj-AAA to obj-BBB among {edges:?}"));
    assert_eq!(edge.mechanism, Mechanism::TemporalPrecedence);
    assert!(edge.is_evidenced());
    assert!(
        edge.confidence > 0.0 && edge.confidence <= 0.5,
        "confidence {} is not the capped, test-statistic-derived value the writer promises",
        edge.confidence
    );
    Ok(())
}

#[test]
fn two_independent_instruments_produce_no_causal_edge() -> Result<()> {
    // The refusal case. Two return series built from independent noise share
    // no lag structure, and a production pass that wrote an edge here would
    // be exactly the false positive this whole method exists to keep out —
    // an empty graph is the honest state until something clears a real bar.
    let count = 120;
    let cause_returns = noise(33, count, 0.02);
    let effect_returns = noise(44, count, 0.02);

    let mut platform = platform(&["CCC", "DDD"])?;
    platform.observe(bars_from_returns("CCC", &cause_returns, count));
    platform.observe(bars_from_returns("DDD", &effect_returns, count));

    assert_eq!(
        platform.world().causal().len(),
        0,
        "premise: an empty graph"
    );
    let report = platform.run_cycle(start());
    assert!(
        report.stage(Stage::Understand).is_some(),
        "UNDERSTAND always runs"
    );

    assert_eq!(
        platform.world().causal().len(),
        0,
        "an independent pair of instruments produced a causal edge: {:?}",
        platform.world().causal().edges()
    );
    Ok(())
}

/// The same bars, with the object id the feed carried replaced by a blank one.
///
/// `Platform::observe` keys `price_history` on `bar.object_id.as_str()` with no
/// validation, and `ObjectId::from_string` accepts anything, so this is the
/// shape a real adapter produces from one malformed vendor row — not a
/// contrivance that could only be built in a test.
fn blank_object_id(records: Vec<SensedRecord>) -> Vec<SensedRecord> {
    records
        .into_iter()
        .map(|record| match record {
            SensedRecord::Bar(mut bar) => {
                bar.object_id = qip_core::ObjectId::from_string("");
                SensedRecord::Bar(bar)
            }
            other => other,
        })
        .collect()
}

#[test]
fn a_bar_carrying_no_object_id_has_its_edge_refused_rather_than_stopping_every_order() -> Result<()>
{
    // A security review found this end to end, so it is not hypothetical. The
    // pass writes an edge named by the ids `price_history` is keyed on, and
    // those ids arrive from a feed over a wire that validates nothing. One
    // blank id used to produce one edge with a blank cause; from the next
    // `risk_state()` onwards `qip_risk::SharedCauseExposure::attribute`
    // refused the blank driver, `qip_kernel::shared_cause` answered by
    // refusing all three shared-cause levels, and `PreTradeChecker::check`
    // turned that into a rejection of **every** order — no rate limit, no way
    // to clear it but repairing the world model, and the cause three stages
    // from the symptom. Fail-closed in direction and catastrophic in reach.
    //
    // The admitting half of this pair is
    // `a_real_lagged_pair_of_instruments_produces_a_temporal_precedence_edge`
    // above: it drives the same fixture with named ids and asserts the edge is
    // written, so this test measures the blank id and not a pass that has
    // stopped writing anything.
    let count = 120;
    let cause_returns = noise(11, count, 0.02);
    let mut effect_returns = vec![0.0; count];
    let effect_noise = noise(22, count, 0.004);
    for t in 1..count {
        effect_returns[t] = 0.8 * cause_returns[t - 1] + effect_noise[t];
    }

    let mut platform = platform(&["AAA", "BBB"])?;
    platform.observe(blank_object_id(bars_from_returns(
        "AAA",
        &cause_returns,
        count,
    )));
    platform.observe(bars_from_returns("BBB", &effect_returns, count));

    assert_eq!(
        platform.world().causal().len(),
        0,
        "premise: an empty graph"
    );

    let report = platform.run_cycle(start());
    let understood = report
        .stage(Stage::Understand)
        .expect("UNDERSTAND always runs");

    // The premise and the finding in one line: the refusal is only reported
    // when the pass actually offered a malformed edge, so this says the
    // blank id reached the writer *and* was turned away there.
    assert!(
        understood
            .detail
            .contains("malformed edge(s) naming no cause or no effect"),
        "the pass reported no refusal, so either the blank id never reached the writer or it was \
         admitted: {}",
        understood.detail
    );

    // And nothing blank reached the graph, which is what the risk producer
    // three stages later would have refused every order over.
    let edges = platform.world().causal().edges().to_vec();
    let blank: Vec<_> = edges
        .iter()
        .filter(|edge| edge.cause.trim().is_empty() || edge.effect.trim().is_empty())
        .collect();
    assert!(
        blank.is_empty(),
        "an edge with a blank end reached the graph: {blank:?}"
    );
    Ok(())
}
// --- §9.1's conditions layer, through the production pass -------------------

#[test]
fn an_edge_the_production_pass_writes_names_the_regime_it_cleared_its_bar_in() -> Result<()> {
    // Blueprint §9.1's conditions layer — "the regime under which an edge
    // holds" — written where the edge is written. Before this the pass wrote
    // a strength, a lag, a confidence and evidence, and said nothing about
    // the conditions the relationship was measured under, so a graph built
    // across a regime turn carried edges from two different worlds with
    // nothing distinguishing them.
    let count = 120;
    let cause_returns = noise(11, count, 0.02);
    let mut effect_returns = vec![0.0; count];
    let effect_noise = noise(22, count, 0.004);
    for t in 1..count {
        effect_returns[t] = 0.8 * cause_returns[t - 1] + effect_noise[t];
    }

    let mut platform = platform(&["AAA", "BBB"])?;
    platform.observe(bars_from_returns("AAA", &cause_returns, count));
    platform.observe(bars_from_returns("BBB", &effect_returns, count));
    assert_eq!(
        platform.world().causal().len(),
        0,
        "premise: an empty graph"
    );

    platform.run_cycle(start());

    let edges = platform.world().causal().edges().to_vec();
    let edge = edges
        .iter()
        .find(|e| e.cause == "obj-AAA" && e.effect == "obj-BBB")
        .unwrap_or_else(|| panic!("premise: the pass wrote the edge at all, among {edges:?}"));

    // The regime the platform itself reports for the effect's tape, not a
    // literal: a test asserting a hard-coded label would pass on an edge
    // tagged with the wrong instrument's regime.
    let in_force = platform.regime_context("obj-BBB");
    assert!(
        edge.holds_in.contains(&in_force),
        "the edge names the regimes {:?} and the tape it was measured on is in {in_force:?}; an \
         edge with no condition on it is §9.1's layer left empty by the one pass that could fill \
         it",
        edge.holds_in
    );
    assert_eq!(
        edge.in_regime(&in_force),
        ConditionStanding::Holds,
        "the condition was written but does not read back"
    );
    Ok(())
}

#[test]
fn a_pair_tested_on_enough_history_that_does_not_clear_the_bar_is_reported_as_a_condition_tested()
-> Result<()> {
    // The negative arm of the same test, which the pass computed every cycle
    // and threw away. Two independent series are tested on ample history and
    // refused — that refusal *is* §9.1's "the conditions under which it is
    // known to fail", and until this it existed only as a dropped `Ok(None)`.
    //
    // Nothing is marked here because neither ordered pair was ever claimed as
    // an edge, and the stage detail says both numbers rather than one: a pass
    // that tested nothing and a pass that tested many pairs and marked none
    // must not read alike.
    let count = 120;
    let cause_returns = noise(33, count, 0.02);
    let effect_returns = noise(44, count, 0.02);

    let mut platform = platform(&["CCC", "DDD"])?;
    platform.observe(bars_from_returns("CCC", &cause_returns, count));
    platform.observe(bars_from_returns("DDD", &effect_returns, count));
    assert_eq!(
        platform.world().causal().len(),
        0,
        "premise: an empty graph"
    );

    let report = platform.run_cycle(start());
    let understood = report
        .stage(Stage::Understand)
        .expect("UNDERSTAND always runs");
    assert!(
        understood
            .detail
            .contains("temporal-precedence pass tested"),
        "premise: the pass ran at all: {}",
        understood.detail
    );
    assert!(
        understood
            .detail
            .contains("pair(s) tested on enough history and did not clear it"),
        "a test that ran on ample history and refused was not recorded as a condition at all: {}",
        understood.detail
    );
    Ok(())
}

#[test]
fn a_relationship_that_stops_holding_marks_the_edge_it_already_wrote_as_failing() -> Result<()> {
    // The whole point of §9.1's conditions layer, driven end to end through
    // the production pass: an edge established while a lead-lag relationship
    // held, and the same pair re-tested once the tape no longer carries it.
    //
    // The regime break is real rather than simulated by hand — `price_history`
    // is a bounded window, so feeding a full window of unrelated returns
    // leaves the pass testing a pair whose relationship has genuinely gone
    // while the edge it wrote earlier is still in the graph. That is the
    // state §9 was written about: an edge that was true and is not, with
    // nothing in the record saying so.
    let count = 120;
    let cause_returns = noise(11, count, 0.02);
    let mut effect_returns = vec![0.0; count];
    let effect_noise = noise(22, count, 0.004);
    for t in 1..count {
        effect_returns[t] = 0.8 * cause_returns[t - 1] + effect_noise[t];
    }

    let mut platform = platform(&["AAA", "BBB"])?;
    platform.observe(bars_from_returns("AAA", &cause_returns, count));
    platform.observe(bars_from_returns("BBB", &effect_returns, count));
    platform.run_cycle(start());

    let edges = platform.world().causal().edges().to_vec();
    let established = edges
        .iter()
        .find(|e| e.cause == "obj-AAA" && e.effect == "obj-BBB")
        .unwrap_or_else(|| panic!("premise: the relationship was established, among {edges:?}"));
    assert!(
        established.fails_in.is_empty(),
        "premise: the edge is not already marked as failing anywhere"
    );

    // A full window of unrelated returns for both names. `SERIES_HISTORY`
    // bars displace the lagged history entirely, so the next pass tests the
    // same pair on a tape in which the relationship does not exist.
    let broken = qip_kernel::platform::SERIES_HISTORY;
    platform.observe(bars_from_returns("AAA", &noise(55, broken, 0.02), broken));
    platform.observe(bars_from_returns("BBB", &noise(66, broken, 0.02), broken));

    let report = platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let understood = report
        .stage(Stage::Understand)
        .expect("UNDERSTAND always runs");

    let edges = platform.world().causal().edges().to_vec();
    let marked: Vec<_> = edges
        .iter()
        .filter(|e| e.cause == "obj-AAA" && e.effect == "obj-BBB" && !e.fails_in.is_empty())
        .collect();
    assert!(
        !marked.is_empty(),
        "the relationship stopped holding and the edge carries no record of it: {} / {edges:?}",
        understood.detail
    );
    assert!(
        understood.detail.contains("marking"),
        "the pass marked an edge and the stage said nothing about it: {}",
        understood.detail
    );
    Ok(())
}

// --- §9.3's hidden concentration, through the production read ---------------

#[test]
fn a_book_whose_positions_share_an_unheld_causal_driver_has_it_surfaced_by_the_understand_stage()
-> Result<()> {
    // Blueprint §9.3: "Positions that appear diversified but share a causal
    // driver are surfaced as concentration." The traversal has existed in
    // `qip_world_model::exposure` and every caller was a test, which is the
    // state this register calls UNREACHED — a risk query nobody asks answers
    // nothing.
    //
    // Everything here is the production path: the edges are written by the
    // UNDERSTAND stage's own pass from observed bars, the book is the one the
    // platform filled through `submit_order`, and the finding is read back in
    // the same stage.
    let count = 120;
    let driver = noise(77, count, 0.02);
    let mut first = vec![0.0; count];
    let mut second = vec![0.0; count];
    let first_noise = noise(88, count, 0.004);
    let second_noise = noise(99, count, 0.004);
    for t in 1..count {
        first[t] = 0.8 * driver[t - 1] + first_noise[t];
        second[t] = 0.8 * driver[t - 1] + second_noise[t];
    }

    let mut platform = platform(&["DRV", "AAA", "BBB"])?;
    platform.observe(bars_from_returns("DRV", &driver, count));
    platform.observe(bars_from_returns("AAA", &first, count));
    platform.observe(bars_from_returns("BBB", &second, count));

    // Two positions, and deliberately not the driver: §8.2's question is about
    // the exposure the desk has *not* counted, and a driver the book holds is
    // concentration it can already see.
    for symbol in ["AAA", "BBB"] {
        let order = platform.order_from(
            object(symbol),
            qip_execution_engine::order::Side::Buy,
            dec!("10"),
            dec!("100"),
            "prop-1",
            vec!["hyp-1".to_string()],
            start(),
        );
        platform.submit_order(order, start())?;
    }

    let report = platform.run_cycle(start());
    let understood = report
        .stage(Stage::Understand)
        .expect("UNDERSTAND always runs");

    // Premise first, and it is two premises: the graph really does hold the
    // shared-driver edges, and the book really does hold both names. Without
    // them an empty finding would read as a clean book.
    let edges = platform.world().causal().edges().to_vec();
    let from_driver: Vec<_> = edges.iter().filter(|e| e.cause == "obj-DRV").collect();
    assert!(
        from_driver.len() >= 2,
        "premise: the pass wrote the shared-driver edges, among {edges:?}"
    );
    assert!(
        understood.detail.contains("held position(s)"),
        "premise: the concentration query ran against a book at all: {}",
        understood.detail
    );

    assert!(
        understood.detail.contains("hidden concentration(s)"),
        "two positions reached by one unheld driver were not surfaced as concentration: {}",
        understood.detail
    );
    assert!(
        understood.detail.contains("obj-DRV reaches"),
        "the concentration was reported without naming the driver it rests on: {}",
        understood.detail
    );
    Ok(())
}
#[test]
fn a_pair_with_too_little_history_is_not_recorded_as_a_condition_it_failed() -> Result<()> {
    // The premise of the whole conditions layer, and the easiest thing to get
    // wrong: the establishment method answers `Ok(None)` both for "too little
    // history" and for "ran and did not clear the bar", and filing the first
    // as a failure would write a refutation nobody obtained. Early in any
    // deployment's life every pair is in that state, so the graph would be
    // born with every edge already condemned under the regime it was born in.
    let count = 30;
    let cause_returns = noise(33, count, 0.02);
    let effect_returns = noise(44, count, 0.02);

    let mut platform = platform(&["CCC", "DDD"])?;
    platform.observe(bars_from_returns("CCC", &cause_returns, count));
    platform.observe(bars_from_returns("DDD", &effect_returns, count));

    let report = platform.run_cycle(start());
    let understood = report
        .stage(Stage::Understand)
        .expect("UNDERSTAND always runs");
    assert!(
        understood
            .detail
            .contains("temporal-precedence pass tested"),
        "premise: the pass ran and tested the pair at all: {}",
        understood.detail
    );
    assert!(
        !understood
            .detail
            .contains("tested on enough history and did not clear it"),
        "a pair nobody had the history to test was recorded as having failed a test: {}",
        understood.detail
    );
    Ok(())
}
