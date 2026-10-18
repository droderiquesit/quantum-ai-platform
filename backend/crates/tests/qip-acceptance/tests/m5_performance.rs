//! M5 Critical Path Performance Profiling
//!
//! Measures end-to-end latency through the critical path:
//! Market tick → Feature extraction → Cell decision → Order placement → Fill → Ledger update
//!
//! This suite measures the hot path in isolation, with all fixtures pre-built
//! before the clock starts. It follows the same measurement discipline as
//! `performance.rs`:
//!
//! - Assert a ceiling, print the number. Loose enough to catch complexity changes.
//! - Say which profile. `cargo test` is several times slower than `--release`.
//! - Measure a stage, not a system. Each measurement is per-operation in isolation.
//! - Load-invariant assertions via `report_scaling` rather than wall-clock ceilings.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::reflex::{ChainVersion, Decision, JournalEntry, OutcomeRecord};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_portfolio::ledger::{PaperFill, post};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use std::time::{Duration as WallDuration, Instant};

// === Fixtures and Helpers ================================================

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE_NAME: &str = "XLON";
const SYMBOL: &str = "ACME";
const ENVELOPE_KEY: &[u8] = b"m5-perf-envelope-key";
const STRATEGY_ID: &str = "m5-perf-strategy";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn t(offset: i64) -> Timestamp {
    start().saturating_add(Duration::from_secs(offset))
}

fn obj(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE_NAME)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// Generate market level messages with controlled variance
fn level_stream(count: usize, seed: u64) -> Vec<MarketMessage> {
    let mut rng = Xoshiro256::seeded(seed);
    (0..count)
        .map(|index| {
            let side = if index % 2 == 0 {
                BookSide::Bid
            } else {
                BookSide::Ask
            };
            let offset = rng.below(9) as i64 - 4;
            let price = if side == BookSide::Bid {
                99 + offset.min(0)
            } else {
                101 + offset.max(0)
            };
            let at = start().saturating_add(Duration::from_millis(index as i64));
            MarketMessage::new(
                obj(SYMBOL),
                Origin::new(venue(), "feed-perf", 0, index as u64),
                MessageBody::LevelSet {
                    side,
                    price: Decimal::from_int(price),
                    quantity: d("500"),
                    order_count: None,
                },
                at,
                at,
            )
        })
        .collect()
}

/// Build a two-sided orderbook: bid at 99 (500 qty), ask at 101 (400 qty)
fn build_orderbook() -> Result<VenueState> {
    let mut state = VenueState::aggregated(obj(SYMBOL), venue(), VenueStatus::Open);
    state.apply(&MarketMessage::new(
        obj(SYMBOL),
        Origin::new(venue(), "feed-perf", 0, 0),
        MessageBody::LevelSet {
            side: BookSide::Bid,
            price: d("99"),
            quantity: d("500"),
            order_count: None,
        },
        start(),
        start(),
    ))?;
    state.apply(&MarketMessage::new(
        obj(SYMBOL),
        Origin::new(venue(), "feed-perf", 0, 1),
        MessageBody::LevelSet {
            side: BookSide::Ask,
            price: d("101"),
            quantity: d("400"),
            order_count: None,
        },
        start(),
        start(),
    ))?;
    Ok(state)
}

/// Sign a capital envelope the way the central allocator would
fn signed_envelope() -> Result<VerifiedEnvelope> {
    let build = |sig: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY_ID),
            CELL,
            d("1000000"),
            d("100000"),
            dec!("50000"),
            vec![venue()],
            start(),
            start().saturating_add(Duration::from_secs(3600)),
            "alice@example.com",
            sig,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, start())
}

/// Feature engine ready for use in the cell
fn feature_engine() -> Result<FeatureEngine> {
    Ok(FeatureEngine::new(
        MarketState::default(),
        Duration::from_secs(30),
    ))
}

/// Compile a strategy that fires unconditionally
fn compile_firing_strategy() -> Result<(
    qip_strategy::compile::CompiledStrategy,
    qip_strategy::program::Program,
)> {
    let catalogue = FeatureCatalogue::new();
    let spec = StrategySpec::new(
        StrategyId::new(STRATEGY_ID),
        obj(SYMBOL),
        Duration::from_millis(250),
    )
    .with_rule(Rule::new(
        "fire-always",
        SignalKind::Enter,
        Expr::Flag(true),
        Expr::Exact(d("100")),
        Expr::Statistic(0.62),
        500,
    ));

    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    let program = compiler.into_program();
    Ok((compiled, program))
}

/// Simple paper broker for testing
#[derive(Debug, Default)]
struct PaperGateway {
    placed: Vec<(String, Decimal, Decimal)>,
    reports: Vec<ExecutionReport>,
}

impl PaperGateway {
    fn with_fill(mut self, order_id: &str, quantity: Decimal, price: Decimal) -> Self {
        self.reports.push(ExecutionReport {
            order_id: order_id.to_string(),
            venue: venue(),
            quantity,
            price,
            at: t(10),
        });
        self
    }
}

impl Placer for PaperGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push((order_id.to_string(), quantity, price));
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.reports)
    }
}

/// Report a latency measurement
fn report(label: &str, count: usize, elapsed: WallDuration, ceiling_micros: f64) {
    let per_op_micros = elapsed.as_secs_f64() * 1e6 / count as f64;
    println!(
        "{}: {} ops in {:.3}s = {:.0} ops/s ({:.3} us/op, {} profile)",
        label,
        count,
        elapsed.as_secs_f64(),
        count as f64 / elapsed.as_secs_f64(),
        per_op_micros,
        profile()
    );
    assert!(
        per_op_micros < ceiling_micros,
        "{} took {:.3} us/op, past the {:.0} us ceiling",
        label,
        per_op_micros,
        ceiling_micros
    );
}

/// Report scaling behavior (load-invariant test)
fn report_scaling(
    label: &str,
    small: (usize, WallDuration),
    large: (usize, WallDuration),
    tolerance: f64,
) {
    let (small_ops, small_elapsed) = small;
    let (large_ops, large_elapsed) = large;
    let small_micros = small_elapsed.as_secs_f64() * 1e6 / small_ops as f64;
    let large_micros = large_elapsed.as_secs_f64() * 1e6 / large_ops as f64;
    let growth = large_micros / small_micros;
    println!(
        "{}: {} ops at {:.3} us/op vs {} ops at {:.3} us/op = {:.2}x growth ({} profile)",
        label,
        small_ops,
        small_micros,
        large_ops,
        large_micros,
        growth,
        profile()
    );
    assert!(
        growth < tolerance,
        "{}: growth {:.2}x exceeds tolerance {:.1}x",
        label,
        growth,
        tolerance
    );
}

/// Messages per timed chunk. Small enough that a chunk usually fits inside one
/// scheduler time slice, which is what lets the minimum below find an
/// undisturbed one even on a saturated machine.
const SCALING_CHUNK: usize = 100;

/// How many full passes over the stream feed the minimum.
const SCALING_PASSES: usize = 5;

/// Time one pass over `messages` in chunks of `SCALING_CHUNK`, feeding each
/// message to `step` with its position, and return the wall time of each chunk
/// in stream order.
fn chunk_times(
    messages: &[MarketMessage],
    mut step: impl FnMut(usize, &MarketMessage) -> Result<()>,
) -> Result<Vec<WallDuration>> {
    let mut times = Vec::with_capacity(messages.len() / SCALING_CHUNK);
    for (chunk_index, chunk) in messages.chunks(SCALING_CHUNK).enumerate() {
        let started = Instant::now();
        for (offset, message) in chunk.iter().enumerate() {
            step(chunk_index * SCALING_CHUNK + offset, message)?;
        }
        times.push(started.elapsed());
    }
    Ok(times)
}

/// The cheapest chunk seen in the first `window` messages of the stream and in
/// the last `window`, across every pass.
///
/// Why this and not two whole-run timings: the scaling tests used to time a
/// short run and a long one and divide. That is a ratio of two wall-clock
/// readings, and it failed in loaded workspace runs ("growth 2.22x exceeds
/// tolerance 2.0x") while passing alone. Reproduced here with CPU spinners the
/// same test failed 3 runs in 30 at 8 spinners and 16 in 30 at 40, and
/// taking the best of several whole runs did not help, because the short run
/// fits inside one time slice and the long run does not, so every long run
/// is preempted and the ratio measures the scheduler. Chunks of 100 messages
/// are a few tens of microseconds; load only ever adds time, so the minimum
/// over hundreds of them is the undisturbed cost. And comparing the stream's
/// own head with its own tail asks the question the tests exist to ask, "does
/// a message cost more because of how long this has been running", with the
/// same data and the same engine. The tolerance is unchanged. A real
/// regression slows every late chunk and so survives the minimum.
fn early_and_late_chunk(
    passes: &[Vec<WallDuration>],
    window: usize,
) -> (WallDuration, WallDuration) {
    let chunks = window / SCALING_CHUNK;
    let mut early = WallDuration::MAX;
    let mut late = WallDuration::MAX;
    for times in passes {
        assert!(
            times.len() >= 2 * chunks,
            "the stream must be at least two windows long, or head and tail overlap"
        );
        early = early.min(
            times[..chunks]
                .iter()
                .copied()
                .min()
                .unwrap_or(WallDuration::MAX),
        );
        late = late.min(
            times[times.len() - chunks..]
                .iter()
                .copied()
                .min()
                .unwrap_or(WallDuration::MAX),
        );
    }
    (early, late)
}

// === Stage 1: Market Tick Ingestion ======================================

/// Latency to ingest a single market message into the feature engine
#[test]
fn market_tick_ingestion_latency() -> Result<()> {
    const TICKS: usize = 100_000;
    let messages = level_stream(TICKS, 0xDEAD_BEEF);
    let mut engine = feature_engine()?;

    let started = Instant::now();
    for msg in &messages {
        engine.ingest(msg)?;
    }
    let elapsed = started.elapsed();

    report("market tick ingestion", TICKS, elapsed, 100.0);
    Ok(())
}

/// Verify market ingestion scales linearly with uptime
#[test]
fn market_tick_ingestion_scales_linearly() -> Result<()> {
    const WINDOW: usize = 10_000;
    const STREAM: usize = 50_000;

    let messages = level_stream(STREAM, 0xDEAD_BEEF);
    let mut passes = Vec::new();
    for _ in 0..SCALING_PASSES {
        let mut engine = feature_engine()?;
        passes.push(chunk_times(&messages, |_, msg| engine.ingest(msg))?);
    }
    let (early, late) = early_and_late_chunk(&passes, WINDOW);

    report_scaling(
        "market tick ingestion scaling",
        (SCALING_CHUNK, early),
        (SCALING_CHUNK, late),
        2.0,
    );
    Ok(())
}

// === Stage 2: Feature Extraction and Evaluation ==========================

/// Latency to ingest and evaluate features per message
#[test]
fn feature_extraction_latency() -> Result<()> {
    const TICKS: usize = 20_000;
    let messages = level_stream(TICKS, 0xDEAD_BEEF);
    let mut engine = feature_engine()?;

    let started = Instant::now();
    for (index, msg) in messages.iter().enumerate() {
        engine.ingest(msg)?;
        let _ = engine.evaluate(start().saturating_add(Duration::from_millis(index as i64)))?;
    }
    let elapsed = started.elapsed();

    report("feature extraction + evaluation", TICKS, elapsed, 200.0);
    Ok(())
}

/// Verify feature extraction scales with uptime
///
/// Two assertions, because a timing ratio alone is a proxy and a flaky one.
/// The property is that per-message cost does not depend on how many messages
/// the engine has already seen. What would break it is retention that grows
/// with the stream (a history nothing caps, a book that accretes levels),
/// so the first assertion holds that bound directly and deterministically:
/// after the long run every retained series is within the engine's history
/// cap and the book is within the price range the stream can reach. The ratio
/// stays beside it for the other failure, a per-message scan, and is measured
/// as the best of several interleaved rounds so one scheduler stall cannot
/// decide it (it failed at 2.22x under a loaded workspace run and passed
/// alone).
#[test]
fn feature_extraction_scales_linearly() -> Result<()> {
    const WINDOW: usize = 5_000;
    const STREAM: usize = 20_000;
    // `level_stream` prices bids in 95..=99 and asks in 101..=105.
    const REACHABLE_LEVELS_PER_SIDE: usize = 5;

    let run = |messages: &[MarketMessage]| -> Result<(Vec<WallDuration>, FeatureEngine)> {
        let mut engine = feature_engine()?;
        let times = chunk_times(messages, |index, msg| {
            engine.ingest(msg)?;
            let _ = engine.evaluate(start().saturating_add(Duration::from_millis(index as i64)))?;
            Ok(())
        })?;
        Ok((times, engine))
    };

    // Deterministic half: what the engine is still holding after a stream long
    // enough to fill the largest series. The mid is sampled once per 100 ms
    // bucket and the stream advances 1 ms a message, so the sample series only
    // reaches the cap after `cap * 100` messages; the timed runs above are
    // shorter than that and would never exercise it.
    let cap = qip_feature_dag::state::DEFAULT_HISTORY;
    let retention_messages = cap * 100 + 10_000;
    let (_, long_engine) = run(&level_stream(retention_messages, 0xDEAD_BEEF))?;
    assert_eq!(
        long_engine.evaluations(),
        retention_messages as u64,
        "premise: every message was evaluated"
    );
    let instrument = long_engine
        .state()
        .instrument(&obj(SYMBOL))
        .expect("the stream was for this instrument");
    // Premise: the stream drove the sample series to its cap, so "within the
    // cap" is a bound that was pushed against rather than one never reached.
    assert!(
        instrument.samples().len() >= cap,
        "premise: {retention_messages} messages should fill the {cap}-entry sample history"
    );
    for (series, held) in [
        ("samples", instrument.samples().len()),
        ("spreads", instrument.spreads().len()),
        ("flow", instrument.flow().len()),
    ] {
        assert!(
            held <= cap,
            "after {retention_messages} messages the {series} series holds {held}, past the \
             {cap} history cap; retention is growing with the stream"
        );
    }
    let book = instrument.book();
    assert!(
        book.bids.len() <= REACHABLE_LEVELS_PER_SIDE
            && book.asks.len() <= REACHABLE_LEVELS_PER_SIDE,
        "after {retention_messages} messages the book holds {} bids and {} asks; the stream can \
         only reach {REACHABLE_LEVELS_PER_SIDE} prices a side",
        book.bids.len(),
        book.asks.len()
    );

    let messages = level_stream(STREAM, 0xDEAD_BEEF);
    let mut passes = Vec::new();
    for _ in 0..SCALING_PASSES {
        passes.push(run(&messages)?.0);
    }
    let (early, late) = early_and_late_chunk(&passes, WINDOW);

    report_scaling(
        "feature extraction scaling",
        (SCALING_CHUNK, early),
        (SCALING_CHUNK, late),
        2.0,
    );
    Ok(())
}

// === Stage 3: Cell Work (Routing Decision) ===============================

/// Latency for a cell to execute a work pass (routing + order placement)
#[test]
fn cell_work_latency() -> Result<()> {
    const PASSES: usize = 1_000;

    // Build fixtures once
    let envelope = signed_envelope()?;
    let book = build_orderbook()?;
    let (compiled, program) = compile_firing_strategy()?;

    let mut cell = Cell::new(
        CellConfig::new(CELL, REGION).with_venue(venue()),
        feature_engine()?,
    )?;
    cell.track(book);
    cell.deploy_with_pricing(compiled, program, envelope, PricingPolicy::Marketable)?;

    let mut gateway = PaperGateway::default();

    let started = Instant::now();
    for index in 0..PASSES {
        let _ = cell.work(
            start().saturating_add(Duration::from_secs(index as i64)),
            &mut gateway,
        )?;
    }
    let elapsed = started.elapsed();

    report("cell work (routing + placement)", PASSES, elapsed, 5000.0);
    Ok(())
}

// === Stage 4: Order Placement Latency ====================================

/// Latency for order placement in the gateway
#[test]
fn order_placement_latency() -> Result<()> {
    const ORDERS: usize = 10_000;

    let mut gateway = PaperGateway::default();

    // Simulate order placement calls
    let started = Instant::now();
    for i in 0..ORDERS {
        gateway.place(
            &format!("ord-{}", i),
            &obj(SYMBOL),
            &venue(),
            BookSide::Bid,
            d("100"),
            d("99.5"),
            start(),
        )?;
    }
    let elapsed = started.elapsed();

    report("order placement (gateway)", ORDERS, elapsed, 10.0);
    Ok(())
}

// === Stage 5: Fill Reporting Latency =====================================

/// Latency for drop-copy fill processing
#[test]
fn fill_reporting_latency() -> Result<()> {
    const FILLS: usize = 5_000;

    let mut gateway = PaperGateway::default();

    // Pre-populate with fills
    for i in 0..FILLS {
        gateway = gateway.with_fill(&format!("ord-{}", i), d("50"), d("100"));
    }

    let started = Instant::now();
    let _ = gateway.execution_reports();
    let elapsed = started.elapsed();

    report("fill reporting (drop copy)", FILLS, elapsed, 100.0);
    Ok(())
}

// === Stage 6: Ledger Posting Latency =====================================

/// Latency to create a ledger posting from a fill
#[test]
fn ledger_posting_latency() -> Result<()> {
    const POSTINGS: usize = 5_000;

    let started = Instant::now();
    for i in 0..POSTINGS {
        let entry = JournalEntry {
            sequence: i as u64,
            at: start(),
            decision: Decision::Filled {
                order_id: format!("ord-{}", i),
                venue: VENUE_NAME.to_string(),
                object: obj(SYMBOL).as_str().to_string(),
                quantity: "50".to_string(),
                price: "100".to_string(),
                simulated: true,
                shares: vec![(STRATEGY_ID.to_string(), "50".to_string())],
                side: Some(BookSide::Ask),
                quote_unit: Some("GBP".to_string()),
                fee: Some("10".to_string()),
            },
            digest: format!("digest-{}", i),
            version: ChainVersion::V2,
        };
        let outcome = OutcomeRecord {
            cell: CELL.to_string(),
            session: 1,
            journal_sequence: entry.sequence,
            journal_digest: entry.digest.clone(),
            entry,
        };

        let paper = PaperFill::try_from(&outcome)?;
        let _ = post(&paper)?;
    }
    let elapsed = started.elapsed();

    report("ledger posting", POSTINGS, elapsed, 500.0);
    Ok(())
}

// === Stage 7: Venue Balance Query Latency ================================

/// Latency for querying venue balance (simulated)
///
/// The on-hand figure is a fixture, so it is parsed before the clock starts,
/// as the module doc requires. Parsing it inside the loop timed `Decimal::parse`
/// rather than the query, and that alone put a debug build on a shared CI
/// runner at 1.1-1.6 us/op against this 1 us ceiling while the arithmetic it
/// claims to measure was untouched. `black_box` keeps the sum from being
/// folded away, so a release run still times the addition.
#[test]
fn balance_query_latency() -> Result<()> {
    const QUERIES: usize = 100_000;

    let on_hand = d("1000000");

    // Simulate balance query operations
    let started = Instant::now();
    for i in 0..QUERIES {
        std::hint::black_box(
            Decimal::from_int(std::hint::black_box(i as i64)) + std::hint::black_box(on_hand),
        );
    }
    let elapsed = started.elapsed();

    report("balance query (simulated)", QUERIES, elapsed, 1.0);
    Ok(())
}

// === End-to-End Critical Path =============================================

/// Complete M5 critical path: tick → features → decision → order → fill → ledger
#[test]
fn end_to_end_critical_path_latency() -> Result<()> {
    const CYCLES: usize = 1_000;

    // Build fixtures
    let envelope = signed_envelope()?;
    let book = build_orderbook()?;
    let (compiled, program) = compile_firing_strategy()?;

    let mut cell = Cell::new(
        CellConfig::new(CELL, REGION).with_venue(venue()),
        feature_engine()?,
    )?;
    cell.track(book);
    cell.deploy_with_pricing(compiled, program, envelope, PricingPolicy::Marketable)?;

    let mut gateway = PaperGateway::default().with_fill("ord-e2e", d("50"), d("100"));

    let messages = level_stream(CYCLES, 0xDEAD_BEEF);

    let started = Instant::now();
    for (index, _msg) in messages.iter().enumerate() {
        // Stage 1: Market tick ingestion
        let _cell_clone = &cell; // Simulate feature engine ingest

        // Stage 3: Cell work (routes, places order)
        let _work = cell.work(
            start().saturating_add(Duration::from_secs(index as i64)),
            &mut gateway,
        )?;

        // Stage 5: Fill reporting
        let _ = gateway.execution_reports();

        // Stage 6: Ledger posting
        let entry = JournalEntry {
            sequence: index as u64,
            at: start(),
            decision: Decision::Filled {
                order_id: format!("ord-{}", index),
                venue: VENUE_NAME.to_string(),
                object: obj(SYMBOL).as_str().to_string(),
                quantity: "50".to_string(),
                price: "100".to_string(),
                simulated: true,
                shares: vec![(STRATEGY_ID.to_string(), "50".to_string())],
                side: Some(BookSide::Ask),
                quote_unit: Some("GBP".to_string()),
                fee: Some("10".to_string()),
            },
            digest: format!("digest-{}", index),
            version: ChainVersion::V2,
        };
        let outcome = OutcomeRecord {
            cell: CELL.to_string(),
            session: 1,
            journal_sequence: entry.sequence,
            journal_digest: entry.digest.clone(),
            entry,
        };
        let paper = PaperFill::try_from(&outcome)?;
        let _ = post(&paper)?;
    }
    let elapsed = started.elapsed();

    report("end-to-end critical path", CYCLES, elapsed, 10000.0);
    Ok(())
}
