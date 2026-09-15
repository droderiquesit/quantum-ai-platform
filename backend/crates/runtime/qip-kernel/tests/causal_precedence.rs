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
use qip_world_model::causal::Mechanism;

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
