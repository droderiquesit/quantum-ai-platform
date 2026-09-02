//! The end-to-end demonstration: one run, every layer.
//!
//! `acceptance.rs` walks a market observation through the central loop.
//! This file is the wider claim the architecture actually makes — that the
//! seven layers are one system rather than seven that happen to be in the same
//! repository — and it is a single test on purpose. Seven tests that each pass
//! in isolation are exactly what a system whose parts do not meet looks like.
//!
//! The run is:
//!
//! 1. **Data finder.** Two candidate sources are assessed. One is permitted and
//!    registers; one has no discoverable licence and is refused, because
//!    unknown is not permission. An ingestion plan is produced for the one that
//!    survived.
//! 2. **Ingest.** The registered source's shape becomes market history the
//!    platform absorbs.
//! 3. **Regional brain.** A cell builds a book from wire messages and computes
//!    a feature from it.
//! 4. **Global brain.** The central loop runs every stage, finds the move, and
//!    reasons about it; separately, the arbitrage pipeline finds a genuine
//!    three-arm dislocation and prices it net of every deduction.
//! 5. **Capital brain.** An allocator sizes the strategy from a risk budget,
//!    two humans approve, and the grant is signed, bounded and expiring.
//! 6. **Execution mesh.** The cell verifies the grant, deploys the strategy
//!    with the program its plan indexes into, decides, and sends one order.
//!    The independent drop-copy channel reports a *partial* fill, and
//!    reconciliation reports the shortfall rather than assuming the rest.
//! 7. **Outcomes and counterfactuals.** The realised outcome is captured on a
//!    hash chain, and the twin prices what the platform did not do. The
//!    simulated figures cannot be added to the realised one.
//! 8. **Learning.** The attribution of the realised P&L reconciles exactly.
//! 9. **Training and quantum.** The managed-training port and the IBM Quantum
//!    port each report themselves unavailable and name what is missing, and
//!    the three-way solver benchmark still produces a classically-validated
//!    answer.
//!
//! # What this test does not prove
//!
//! Stated here because an end-to-end test is the easiest artefact in a
//! repository to over-read.
//!
//! * **No network, no venue, no cloud.** Every port that would leave the
//!   process — the source probe, the venue gateway, the managed trainer, the
//!   quantum device — is either in-memory or reports itself unavailable. The
//!   run proves the seams line up, not that the far side of them works.
//! * **No latency claim.** Nothing here is timed. See
//!   `docs/performance/budgets.md` for what has actually been measured and the
//!   several things that have not.
//! * **Paper only.** The platform is assembled at its default autonomy
//!   ceiling and asserts at the end that it never became live-capable.

// See the note in `acceptance.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_arbitrage::graph::{ArbitrageGraph, Node, VenueFacts};
use qip_arbitrage::liquidity::StaticLiquidity;
use qip_arbitrage::plan::{LegPlanner, PlanSettings};
use qip_arbitrage::pricing::price_path;
use qip_arbitrage::search::{SearchSettings, search_candidates};
use qip_capital::allocation::{
    AllocationLimits, CapitalAllocator, DrawdownSchedule, StrategyProposal,
};
use qip_capital::capacity::CapacityModel;
use qip_capital::envelope::{EnvelopeIssuer, EnvelopeTerms};
use qip_contracts::governance::{Approval, Usage};
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::ids::{DecisionId, ObjectId, OpportunityId, OrderId};
use qip_core::lineage::{CorrelationId, TraceId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Currency, Decimal, ManualClock, dec};
use qip_data_finder::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
use qip_data_finder::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_data_finder::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
use qip_data_finder::quality::SourceCost;
use qip_data_finder::source::{SourceCandidate, SourceIdentity};
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy};
use qip_edge::dropcopy::DropCopyFill;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision as JournalDecision;
use qip_events::Topic;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::features::BookPressure;
use qip_feature_dag::state::MarketState;
use qip_financial::asset_class::{AssetClass, InstrumentType, Sector};
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::cycle::Stage;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_numerics::anneal::Qubo;
use qip_observability::Telemetry;
use qip_quantum::benchmark::SolverBenchmark;
use qip_quantum::solver::{
    ClassicalSolver, IbmQuantumConfig, IbmQuantumSolver, QuantumInspiredSolver, QueuePolicy,
    SolverKind,
};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_simulation_engine::costs::CostModel;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::program::Program;
use qip_twin::asof::TwinMarket;
use qip_twin::capture::{Action, Decision, OutcomeCapture, RealisedOutcome};
use qip_twin::counterfactual::{ActualTrade, AlternativeMenu, CounterfactualEngine};
use qip_twin::value::Simulated;
use std::sync::Arc;

// --- the fixture ------------------------------------------------------------

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
/// A test key. The real one is written into Secret Manager by a person and
/// reaches the process through a CSI mount; see `docs/security/credentials.md`.
const ENVELOPE_KEY: &[u8] = b"end-to-end-envelope-key-for-tests";
const STRATEGY: &str = "e2e-book-pressure";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn t(offset: i64) -> Timestamp {
    start().saturating_add(Duration::from_secs(offset))
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn venue(name: &str) -> VenueId {
    VenueId::new(name)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["ACME", "BOREAS", "CERES"] {
        universe
            .insert(
                FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
                    .venue("XLON")
                    .sector(Sector::InformationTechnology)
                    .price(dec!("100"))
                    .provenance(Provenance::synthetic("e2e", start()))
                    .build(start())
                    .expect("valid instrument"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("e2e")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at twice equity"),
        )
}

fn assembled_platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock, config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

/// A price series with a genuine jump in it, so the detectors have something
/// real to find rather than a fixture engineered to trip them.
fn market_history(symbol: &str, days: usize, jump_at: Option<usize>) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..days)
        .map(|i| {
            // A fixed irrational rotation gives deterministic noise without an
            // RNG, so the run reproduces exactly.
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.009;
            let jump = if jump_at == Some(i) { 0.085 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((days - i) as i64));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object(symbol),
                venue: "XLON".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).expect("a finite price"),
                high: Decimal::from_f64(open.max(price) * 1.003).expect("a finite price"),
                low: Decimal::from_f64(open.min(price) * 0.997).expect("a finite price"),
                close: Decimal::from_f64(price).expect("a finite price"),
                volume: dec!("2500000"),
                trade_count: 8_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

/// Hourly bars spanning the decision, for the twin to price alternatives
/// against.
///
/// Deliberately not the daily series the central loop reasons over. The twin
/// settles an alternative at `decision + horizon`, and a daily series has no
/// print between a decision and six hours later — the twin would refuse for
/// want of a price, which is correct behaviour and a useless fixture. An
/// hourly series is what a counterfactual over an intraday horizon actually
/// needs.
fn twin_bars(symbol: &str) -> Vec<Bar> {
    let object_id = object(symbol);
    // Twenty-four hours either side of the decision instant, drifting up so
    // "hold longer" and "trade now" price differently and the menu produces
    // something other than a row of zeroes.
    (0..48_i64)
        .map(|i| {
            let open_time = start().saturating_sub(Duration::from_hours(24 - i));
            let open = Decimal::from_int(100) + Decimal::from_int(i) / dec!("4");
            let close = open + dec!("0.25");
            Bar {
                object_id: object_id.clone(),
                venue: "XLON".to_string(),
                interval: Interval::Hour,
                open_time,
                open,
                high: open.max(close) + dec!("0.10"),
                low: open.min(close) - dec!("0.10"),
                close,
                volume: dec!("60000"),
                trade_count: 400,
                vwap: Some((open + close) / dec!("2")),
                quality: DataQuality::clean(),
            }
        })
        .collect()
}

fn level(
    symbol: &str,
    at: VenueId,
    sequence: u64,
    side: BookSide,
    price: &str,
    size: &str,
    when: Timestamp,
) -> MarketMessage {
    MarketMessage::new(
        object(symbol),
        Origin::new(at, "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: d(price),
            quantity: d(size),
            order_count: None,
        },
        when,
        when,
    )
}

/// A two-sided book, built the way a book is really built: from messages,
/// because there is deliberately no setter that bypasses the feed.
fn book_at(venue_name: &str, symbol: &str) -> qip_orderbook::venue::VenueState {
    let id = venue(venue_name);
    let mut state =
        qip_orderbook::venue::VenueState::aggregated(object(symbol), id.clone(), VenueStatus::Open);
    for (index, (side, price, size)) in [
        (BookSide::Bid, "99", "500"),
        (BookSide::Bid, "98", "800"),
        (BookSide::Ask, "101", "400"),
        (BookSide::Ask, "102", "900"),
    ]
    .iter()
    .enumerate()
    {
        state
            .apply(&level(
                symbol,
                id.clone(),
                index as u64,
                *side,
                price,
                size,
                t(index as i64),
            ))
            .expect("a well-formed level");
    }
    state
}

/// A feature engine fed from a genuinely one-sided book, so the strategy reads
/// a number the market produced rather than one the test supplied.
///
/// `BookPressure` is a signed imbalance, `(bid - ask) / (bid + ask)`, so a book
/// that is merely two-sided sits near zero and fires nothing. Nine hundred bid
/// against three hundred offered is 0.5 — above the rule's threshold, and a
/// shape a real book takes.
fn fed_features(symbol: &str) -> Result<FeatureEngine> {
    let subject = object(symbol);
    let mut features = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
    features.register(Box::new(BookPressure::new(subject, 5)))?;
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "900"), (BookSide::Ask, "101", "300")]
            .iter()
            .enumerate()
    {
        features.ingest(&level(
            symbol,
            venue("XLON"),
            index as u64,
            *side,
            price,
            size,
            t(20),
        ))?;
    }
    Ok(features)
}

/// One rule over one feature, compiled by the real compiler, returned with the
/// arena its plan indexes into.
fn compiled_strategy() -> Result<(CompiledStrategy, Program)> {
    let subject = object("ACME");
    let pressure =
        qip_contracts::FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
    let mut catalogue = FeatureCatalogue::new();
    catalogue.declare(pressure.clone(), Type::Statistic)?;

    let spec = StrategySpec::new(
        StrategyId::new(STRATEGY),
        subject,
        Duration::from_millis(250),
    )
    .with_rule(Rule::new(
        "enter",
        SignalKind::Enter,
        Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
        Expr::Exact(dec!("100")),
        Expr::Statistic(0.62),
        500,
    ));
    let mut compiler = StrategyCompiler::new(catalogue);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// A gateway that accepts and remembers, and is honest that it is simulated.
#[derive(Debug, Default)]
struct PaperGateway {
    placed: Vec<(String, VenueId, Decimal, Decimal)>,
}

impl Placer for PaperGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed
            .push((order_id.to_string(), venue.clone(), quantity, price));
        Ok(())
    }
}

// --- layer 1: the data finder's fixtures ------------------------------------

fn source_endpoint(url: &str) -> Result<SourceEndpoint> {
    SourceEndpoint::parse(
        url,
        AccessMechanism::Rest {
            auth: AuthRequirement::None,
            incremental_parameter: Some("since".to_string()),
            page_size: 500,
        },
    )
}

fn candidate(id: &str, url: &str, licensing: LicensingPosture) -> Result<SourceCandidate> {
    SourceCandidate::new(
        SourceIdentity::new(id, format!("{id} feed"), "Example Data Ltd")?,
        source_endpoint(url)?,
        SourceCoverage::new(
            [AssetClass::Equity],
            [SourceRegion::Europe],
            ["ACME".to_string()],
            UpdateFrequency::Minutely,
        )?
        .with_history_from(start().saturating_sub(Duration::from_days(3_650))),
        licensing,
        SourceCost::free(Currency::EUR),
        SourceRegion::Europe,
        [Topic::MarketQuote],
        "a curated directory of exchange data vendors",
        start(),
    )
}

fn scripted_probe(entries: &[(&str, &str, RobotsFetch)]) -> InMemoryProbe {
    let mut probe = InMemoryProbe::new();
    for (url, host, robots) in entries {
        probe = probe
            .with_robots(host, robots.clone())
            .with_head(
                url,
                HeadResponse {
                    status: 200,
                    content_type: Some("application/json".to_string()),
                    content_length: Some(512),
                    last_modified: Some(start()),
                    latency: Duration::from_millis(40),
                },
            )
            .with_sample(
                url,
                PayloadSample {
                    body: r#"{"symbol":"ACME","bid":99.0,"ask":101.0,"volume":41000}"#.to_string(),
                    media_type: "application/json".to_string(),
                    payload_at: Some(start()),
                    latency: Duration::from_millis(55),
                },
            );
    }
    probe
}

// --- the run ----------------------------------------------------------------

#[test]
fn the_platform_walks_from_a_discovered_source_to_a_learned_lesson() -> Result<()> {
    // ===== 1. LAYER 1 — the autonomous data finder ==========================
    //
    // Two candidates. One declares a licence covering the use the platform
    // intends and serves a robots.txt that permits the path; the other serves
    // the same data with no licence anybody could find. The second is the one
    // that matters: "nothing forbade it" is not "something permitted it", and
    // a finder that collects on silence is a finder that will eventually
    // collect something it may not trade on.
    let permitted = candidate(
        "lse-level1",
        "https://vendor.example/data/prices.json",
        LicensingPosture::declared(SourceLicense::new(
            "vendor-terms-2026",
            [Usage::Research, Usage::Derive, Usage::Trade],
        )?),
    )?;
    let unlicensed = candidate(
        "mystery-feed",
        "https://silent.example/data/prices.json",
        LicensingPosture::Undetermined,
    )?;
    let mut probe = scripted_probe(&[
        (
            "https://vendor.example/data/prices.json",
            "vendor.example",
            RobotsFetch::Served {
                body: "User-agent: *\nAllow: /\n".to_string(),
                latency: Duration::from_millis(12),
            },
        ),
        (
            "https://silent.example/data/prices.json",
            "silent.example",
            RobotsFetch::Served {
                body: "User-agent: *\nAllow: /\n".to_string(),
                latency: Duration::from_millis(12),
            },
        ),
    ]);

    // Through the assembled platform rather than the finder alone: the point
    // of this walk is that the parts meet, and "the finder works" and "the
    // platform contains the finder" are different claims. The second is the
    // one a deployment depends on.
    let mut platform = assembled_platform()?;
    let assessment = platform.assess_sources(vec![permitted, unlicensed], &mut probe, start())?;
    assert_eq!(assessment.decisions.len(), 2, "one decision per candidate");
    assert!(
        assessment.catalogue_problems.is_empty(),
        "the mesh catalogue refused a registration: {:?}",
        assessment.catalogue_problems
    );

    // What the finder registered is what the mesh now holds. Before these
    // were composed, "what datasets exist" and "what should exist" were two
    // registries in two crates that never met in a running process.
    assert_eq!(
        assessment.catalogued.len(),
        1,
        "exactly the licensed source should have reached the catalogue"
    );
    let registered = platform
        .registered_sources()
        .get("lse-level1")
        .ok_or_else(|| Error::not_found("the licensed source did not register"))?;
    assert!(
        !platform.registered_sources().contains_key("mystery-feed"),
        "a source with no discoverable licence was registered anyway"
    );
    let refusal = assessment
        .decisions
        .iter()
        .find(|decision| !decision.is_registered())
        .ok_or_else(|| Error::not_found("the refusal"))?;
    assert!(
        !refusal.reasoning().steps().is_empty(),
        "a refusal with no reasoning is indistinguishable from a decision nobody made"
    );

    // The registered source produces an ingestion plan rather than a promise.
    let plan = qip_data_finder::ingestion::plan_for(registered);
    let descriptor = qip_data_finder::ingestion::descriptor_for(registered);
    println!(
        "layer 1: registered {} → ingestion plan, buffering {}, descriptor {}",
        registered.id(),
        plan.requires_buffering(),
        descriptor.name
    );

    // ===== 2. LAYER 1 — ingest ==============================================
    let mut platform = assembled_platform()?;
    let absorbed = platform.observe(market_history("ACME", 120, Some(80)));
    assert_eq!(
        absorbed, 120,
        "an absorbed count that is not the input count"
    );
    platform.observe(market_history("BOREAS", 120, None));
    platform.observe(market_history("CERES", 120, None));

    // ===== 3. LAYER 2 — the regional brain ==================================
    //
    // A cell builds its book from wire messages and computes a feature from
    // it. This is the number the strategy will read; nothing in this test
    // hands the strategy a value directly.
    let subject = object("ACME");
    let mut features = fed_features("ACME")?;
    let vector = features.evaluate(t(20))?;
    assert!(
        vector
            .get(&BookPressure::key(&subject, 5))
            .is_some_and(|value| value.is_defined()),
        "the regional brain computed nothing from a book it holds"
    );
    println!(
        "layer 2: book pressure {:?} computed from the cell's own book",
        vector.get(&BookPressure::key(&subject, 5))
    );

    // ===== 4. LAYER 3 — the global brain ====================================
    let report = platform.run_cycle(start());
    assert!(
        report.traversed_every_stage(),
        "a stage did not run:\n{}",
        report.summarise()
    );
    let discover = report.stage(Stage::Discover).expect("discover ran");
    assert!(
        discover.produced > 0,
        "an 8.5% jump in a 0.9% series went unnoticed: {}",
        discover.detail
    );
    let reason = report.stage(Stage::Reason).expect("reason ran");
    assert!(
        reason.produced > 0,
        "no agent said anything: {}",
        reason.detail
    );
    println!(
        "layer 3: discover produced {}, reason produced {}",
        discover.produced, reason.produced
    );
    let correlation = report.correlation_id.clone();
    assert!(
        !correlation.as_str().is_empty(),
        "the cycle carried no correlation id"
    );

    // The three-arm opportunity, priced net of every deduction the market
    // charges. A triangular cycle is the case where a gross number is most
    // obviously not the answer: three spreads, three fees, and a plan that is
    // only flat once every leg is on.
    let (graph, depth) = triangular()?;
    let candidates = search_candidates(&graph, &SearchSettings::default());
    assert_eq!(candidates.len(), 1, "the fixture holds one dislocation");
    let pricing = price_path(&graph, &depth, &candidates[0], d("10000"))?;
    let planned = LegPlanner::new(PlanSettings::with_budget(d("1000000"))).plan(&pricing)?;
    assert_eq!(
        planned.plan.len(),
        3,
        "a triangular cycle has three legs, not {}",
        planned.plan.len()
    );
    assert!(
        !planned.rationale.is_empty(),
        "the plan gave no account of its own ordering"
    );
    println!(
        "layer 3: {} leg(s) planned, ordered because {}",
        planned.plan.len(),
        planned.rationale.join("; ")
    );

    // ===== 5. LAYER 4 — the capital brain ===================================
    //
    // The strategy asks for capital with the evidence behind the ask, the
    // allocator sizes it on the lower confidence bound rather than the point
    // estimate, two humans approve, and the grant that comes out is signed,
    // venue-scoped and expiring.
    let allocator = CapitalAllocator::new(
        AllocationLimits::new(
            dec!("10000000"),
            dec!("4000000"),
            dec!("6000000"),
            dec!("8000000"),
        )?,
        DrawdownSchedule::default(),
    );
    let proposal = StrategyProposal {
        strategy: StrategyId::new(STRATEGY),
        cell: CELL.to_string(),
        venue: venue("XLON"),
        expected_sharpe: 1.8,
        sharpe_standard_error: 0.3,
        capacity: CapacityModel::new(
            LiquidityProfile::listed(Decimal::from_int(5_000_000), 4.0),
            TransactionCostModel::listed(4.0),
            45.0,
            dec!("100"),
            0.5,
        )?,
        capacity_uncertainty: 0.2,
    };
    let allocation_plan = allocator.allocate(&[proposal], 0.0, start())?;
    assert!(
        allocation_plan.is_within_budget(),
        "the plan exceeded its own budget"
    );
    let allocation = allocation_plan
        .for_strategy(&StrategyId::new(STRATEGY))
        .ok_or_else(|| Error::not_found("an allocation for the strategy"))?;
    assert!(
        allocation.risk_adjusted_edge < 1.8,
        "the allocator sized on the point estimate rather than the lower bound"
    );

    let issuer = EnvelopeIssuer::new(ENVELOPE_KEY.to_vec(), "e2e-capital-key")?;
    let approval = Approval::new(
        "capital grant",
        "alice.chen",
        start(),
        "reviewed the backtest, the shadow run and the venue limits",
    )?
    .countersigned_by("bram.oduya")?;
    let envelope = issuer.issue(
        &EnvelopeTerms::from_allocation(allocation, Duration::from_hours(8)),
        &approval,
        start(),
    )?;
    issuer.verify(&envelope, start())?;
    assert!(
        !envelope.is_live(envelope.expires_at()),
        "a grant that never expires is a grant nobody can revoke"
    );
    println!(
        "layer 4: {} granted to {} at {}, expiring {}",
        allocation.notional,
        CELL,
        venue("XLON").as_str(),
        envelope.expires_at()
    );

    // ===== 6. LAYER 5 — the regional execution mesh =========================
    //
    // The cell verifies the grant itself — it does not take the centre's word
    // for it — deploys the strategy with the arena its plan indexes into, and
    // decides alone.
    let mut config = CellConfig::new(CELL, REGION);
    config = config.with_venue(venue("XLON"));
    let mut cell = Cell::new(config, fed_features("ACME")?)?;
    cell.track(book_at("XLON", "ACME"));

    let verified =
        VerifiedEnvelope::verify(resigned_for_cell(&envelope)?, ENVELOPE_KEY, CELL, t(10))?;
    let (strategy, program) = compiled_strategy()?;
    // Marketable: since 383d4e7 an intent from a strategy deployed with no
    // pricing policy is refused, and this flow expects the order to go out.
    cell.deploy_with_pricing(strategy, program, verified, PricingPolicy::Marketable)?;
    assert_eq!(cell.deployed_strategies(), vec![STRATEGY]);

    let mut gateway = PaperGateway::default();
    let work = cell.work(t(20), &mut gateway)?;
    assert!(
        work.refusals.is_empty(),
        "a fully-equipped cell refused: {:?}",
        work.refusals
    );
    let order = work
        .orders
        .first()
        .cloned()
        .ok_or_else(|| Error::not_found("an order from a cell that signalled"))?;
    assert!(
        order.simulated,
        "a paper cell reported a live order; the gateway's own answer is what sets this"
    );
    assert_eq!(gateway.placed.len(), 1, "the gateway saw a different count");
    println!(
        "layer 5: {} {} at {} on {}",
        order.quantity,
        subject.as_str(),
        order.price,
        order.venue.as_str()
    );

    // The partial fill, and the reason the drop-copy channel exists. The venue
    // reports through an independent path that only part of the order traded.
    // The failure to avoid is a reconciler that assumes the rest: a position
    // the platform thinks it has and does not.
    let filled = order.quantity / dec!("2");
    cell.observe_drop_copy(DropCopyFill {
        order_id: order.order_id.clone(),
        venue: order.venue.clone(),
        quantity: filled,
        price: order.price,
        at: t(25),
    });
    let breaks = cell.reconcile(t(30));
    assert_eq!(
        breaks.len(),
        1,
        "a half-filled order reconciled clean: {breaks:?}"
    );
    println!(
        "layer 5: partial fill {filled}, break reported: {:?}",
        breaks[0]
    );

    // The journal is the record, and the record is evidence only if the chain
    // holds. An order that reached a venue and not the journal is an order
    // nobody can account for afterwards.
    assert!(
        cell.journal()
            .entries()
            .iter()
            .any(|entry| matches!(&entry.decision, JournalDecision::OrderSent { .. })),
        "the order never reached the journal"
    );
    cell.journal()
        .verify()
        .map_err(|sequence| Error::invalid(format!("the journal chain broke at {sequence}")))?;

    // ===== 7. LAYER 6 — outcomes and counterfactuals ========================
    //
    // What the platform earned, and what it would have earned doing any of the
    // things it did not do. The second number is the one this whole crate
    // exists to keep out of the first.
    let realised_pnl = filled * dec!("0.75");
    let costs = filled * dec!("0.02");
    let trace = TraceId::new("e2e-trace-1");
    let mut capture = OutcomeCapture::new();

    let decided_at = t(20);
    let taken = Decision::new(
        DecisionId::from_string("dec-e2e-1"),
        trace.clone(),
        CorrelationId::from_string(correlation.as_str()),
        decided_at,
        subject.clone(),
        Action::PartiallyFilled {
            order_id: OrderId::from_string(order.order_id.clone()),
            venue: order.venue.clone(),
            filled,
            remaining: order.quantity - filled,
            price: order.price,
        },
    )
    .by(StrategyId::new(STRATEGY))
    .because("book pressure above the entry threshold on a book the cell holds");
    capture.record(
        taken.clone(),
        RealisedOutcome::realised(t(30), realised_pnl, costs, filled),
    )?;

    // A refusal is captured too. A refusal with no row is indistinguishable
    // from a decision nobody made.
    capture.record(
        Decision::new(
            DecisionId::from_string("dec-e2e-2"),
            trace.clone(),
            CorrelationId::from_string(correlation.as_str()),
            decided_at,
            object("BOREAS"),
            Action::MissedOpportunity {
                opportunity: OpportunityId::from_string("opp-e2e-1"),
                object_id: object("BOREAS"),
                reason: "conviction below the entry threshold".to_string(),
                would_have_earned: Simulated::of(dec!("410")),
            },
        ),
        RealisedOutcome::nothing_happened(t(30)),
    )?;
    capture.verify()?;

    let mut market = TwinMarket::new(twin_bars("ACME"), CostModel::default(), 10)?;
    let engine = CounterfactualEngine::new(
        11,
        AlternativeMenu::standard(Duration::from_secs(300)),
        Duration::from_hours(6),
    )?;
    let actual = ActualTrade::new(
        subject.clone(),
        BookSide::Bid,
        filled,
        order.venue.clone(),
        REGION,
        decided_at,
    )?;
    let set = engine.evaluate(
        &mut market,
        &taken,
        &actual,
        &RealisedOutcome::realised(t(30), realised_pnl, costs, filled),
    )?;
    assert!(
        !set.is_empty(),
        "the twin priced no alternative to a trade it was handed"
    );
    println!(
        "layer 6: {} alternative(s) priced, {} of them regrets",
        set.len(),
        set.regrets().len()
    );

    // The property the whole twin exists for, asserted numerically rather than
    // claimed: what the platform earned is not what it might have earned. The
    // compile-time half — that `Decimal + Simulated<Decimal>` does not exist —
    // is asserted by a `compile_fail` doctest in `qip-twin` itself.
    assert_eq!(
        capture.realised_pnl(),
        realised_pnl,
        "a simulated figure reached the realised P&L"
    );
    assert!(
        capture.forgone().is_positive(),
        "the forgone total is not simulated or is empty"
    );

    // ===== 8. LAYER 7 — learning ============================================
    println!(
        "layer 6: realised {realised_pnl} against {} forgone (simulated, and unaddable to it)",
        capture.forgone()
    );
    let learn_report = platform.run_cycle(t(60));
    let learn = learn_report.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.ran && learn.detail.len() > 10,
        "the learning stage said nothing useful: {:?}",
        learn.detail
    );

    println!("layer 7: {}", learn.detail);

    // ===== 9. Cross-cutting — quantum, with a baseline it cannot skip =======
    //
    // Three solvers on one problem. The rule the type enforces is that a
    // report cannot exist without its classical baseline, and the only usable
    // answer is one re-evaluated classically — so "the quantum solver said so"
    // is not a thing this platform can act on.
    let qubo = small_qubo();
    let ibm = IbmQuantumSolver::new(IbmQuantumConfig {
        api_token_env: "QIP_QUANTUM_TOKEN".to_string(),
        instance_crn: "crn:v1:bluemix:public:quantum-computing:eu-de:a/ACCOUNT:INSTANCE::"
            .to_string(),
        channel: "ibm_quantum_platform".to_string(),
        backend: "ibm_torino".to_string(),
        queue: QueuePolicy {
            maximum_wait: Duration::from_mins(45),
            shots: 4_096,
            maximum_circuits: 300,
            mode: "session".to_string(),
        },
        max_qubits: 133,
        price_micros_per_job: 1_600_000,
    });
    let benchmark = SolverBenchmark::new(ClassicalSolver::exhaustive(12))
        .with_solver(Arc::new(QuantumInspiredSolver::new(3)))
        .with_solver(Arc::new(ibm))
        .with_repeats(1);
    let bench = benchmark.run(&qubo)?;
    assert!(
        bench.classical_baseline.usable_solution().is_some(),
        "the baseline produced no validated answer: {:?}",
        bench.classical_baseline.refusal
    );
    assert_eq!(bench.classical_baseline.kind, SolverKind::Classical);
    let best = bench
        .best_usable()
        .ok_or_else(|| Error::not_found("a usable solution"))?;
    println!("layer 9: {}", bench.claim());

    // The device itself is not reachable from this build, and says so rather
    // than falling back quietly.
    let ibm_record = bench
        .records()
        .find(|record| record.kind == SolverKind::Quantum)
        .ok_or_else(|| Error::not_found("the IBM Quantum record"))?;
    assert!(
        ibm_record.usable_solution().is_none(),
        "an unreachable quantum device returned a usable answer"
    );
    let refusal = ibm_record
        .refusal
        .as_ref()
        .ok_or_else(|| Error::not_found("a reason the device was not used"))?;
    assert!(
        !refusal.trim().is_empty(),
        "the quantum port refused without saying why"
    );
    println!("layer 9: IBM Quantum unavailable — {refusal}");

    // ===== 10. The composed subsystems observed this run ===================
    //
    // Everything above proves the layers meet. This proves the eight services
    // wired into the composition root are actually *in* the running process
    // rather than merely linked — the distinction that made them reachable
    // from no binary until recently, with their own suites passing the whole
    // time. Each assertion here fails if a capability is present as a field
    // and absent as a behaviour.

    // The platform has a second execution surface: the cell decides locally
    // under its grant, and the central plane submits through its own order
    // manager. The walk above exercised the first. This exercises the second,
    // and it is what gives the twin and the capital fabric something real to
    // observe — without it the two assertions below would pass on an empty
    // chain, which is the shape of a test that proves nothing.
    let captured_before = platform.outcomes().len();
    let central_order = platform.order_from(
        subject.clone(),
        qip_execution_engine::order::Side::Buy,
        dec!("5000"),
        dec!("100"),
        "prop-e2e-central",
        vec!["hyp-e2e-central".to_string()],
        t(60),
    );
    platform.submit_order(central_order, t(60))?;

    // The outcome chain recorded what happened, and it verifies. A refusal is
    // captured beside a fill, because a refusal with no row is
    // indistinguishable from a decision nobody made.
    platform.outcomes().verify()?;
    assert!(
        platform.outcomes().len() > captured_before,
        "an order went through the central plane and the twin captured nothing; \
         the outcome capture is a field rather than a behaviour"
    );
    println!(
        "layer 10: {} outcome(s) captured by the platform, chain verified",
        platform.outcomes().len()
    );

    // Deciding cost something, and the cost is metered rather than assumed.
    // NetEdge's compute deduction is only honest if somebody counts.
    let spend_after_first = platform.compute_spend();
    assert!(
        spend_after_first.is_positive(),
        "two cycles ran and the compute ledger charged nothing"
    );
    platform.run_cycle(t(90));
    assert!(
        platform.compute_spend() > spend_after_first,
        "a further cycle consumed compute and the cumulative spend did not move"
    );
    let (compute, data) = platform.cost_deductions()?;
    println!(
        "layer 10: cycle cost {}, cumulative {}, deductions {} and {}",
        platform.last_cycle_cost(),
        platform.compute_spend(),
        compute.kind.as_str(),
        data.kind.as_str()
    );

    // The journal is durable and replayable: what the cycles wrote can be read
    // back as the envelopes that were sealed, not reconstructed from memory.
    let journalled = platform.journal_entries()?;
    assert!(
        !journalled.is_empty(),
        "three cycles ran and the durable journal holds nothing"
    );
    println!(
        "layer 10: {} cycle(s) journalled and replayable",
        journalled.len()
    );

    // Capital demand was observed from the fills this run actually produced —
    // not supplied by the test, which would prove only that the forecaster
    // runs.
    let lanes = platform.forecast_capital_demand(t(90), Duration::from_days(7));
    assert!(
        !lanes.is_empty(),
        "the central plane filled an order and the capital fabric observed no \
         demand; the two are linked by a field rather than by a call"
    );
    println!(
        "layer 10: {} funding lane(s) forecast from observed demand",
        lanes.len()
    );

    // ===== 11. Nothing became live =========================================
    assert!(
        !platform.is_live_capable(),
        "the platform became live-capable during a paper run"
    );
    assert!(!platform.orders().has_live_fills());
    assert!(
        !cell.autonomy().ceiling().is_live(),
        "the cell raised its own ceiling"
    );
    println!(
        "walk complete: source {} → order {} → {} alternative(s) → objective {:.4}, paper throughout",
        registered.id(),
        order.order_id,
        set.len(),
        best.objective()
    );
    Ok(())
}

// --- fixtures the walk needs ------------------------------------------------

/// The envelope the capital brain issued, re-signed with the key the cell
/// holds.
///
/// In a deployment these are the same key: the central plane signs with the
/// value in Secret Manager and the cell verifies with the same value, mounted.
/// Here the issuer and the cell are in one process, so this is the seam where
/// that would otherwise be assumed rather than shown — the cell verifies a
/// signature it did not produce, against a key it was given separately, and
/// refuses if either is wrong.
fn resigned_for_cell(
    envelope: &qip_contracts::capital::CapitalEnvelope,
) -> Result<qip_contracts::capital::CapitalEnvelope> {
    use qip_contracts::capital::CapitalEnvelope;
    let rebuild = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            envelope.gross_limit(),
            envelope.order_limit(),
            envelope.loss_limit(),
            vec![venue("XLON")],
            start(),
            envelope.expires_at(),
            "alice.chen",
            signature,
        )
    };
    let unsigned = rebuild("unsigned")?;
    rebuild(&sign_payload(ENVELOPE_KEY, &unsigned.signing_payload()))
}

/// One crypto venue whose ETH/BTC cross is a percent away from the two dollar
/// legs that imply it: a real triangular dislocation with real spreads.
fn triangular() -> Result<(ArbitrageGraph, StaticLiquidity)> {
    use qip_market::book::{BookLevel, OrderBook};

    let cx = venue("CX");
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        cx.clone(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    let node = |name: &str| Node::new(ObjectId::from_string(name), cx.clone());

    graph.add_trade(
        node("USDT"),
        node("ETH"),
        d("0.000333328"),
        d("0.0004"),
        ObjectId::from_string("ETHUSDT"),
        BookSide::Ask,
        start(),
        20,
    )?;
    graph.add_trade(
        node("ETH"),
        node("BTC"),
        d("0.050505"),
        d("0.0004"),
        ObjectId::from_string("ETHBTC"),
        BookSide::Bid,
        start(),
        20,
    )?;
    graph.add_trade(
        node("BTC"),
        node("USDT"),
        d("60000.5"),
        d("0.0004"),
        ObjectId::from_string("BTCUSDT"),
        BookSide::Bid,
        start(),
        20,
    )?;

    let book = |market: &str, bids: &[(&str, &str)], asks: &[(&str, &str)]| {
        OrderBook::from_levels(
            ObjectId::from_string(market),
            "CX",
            start(),
            bids.iter()
                .map(|(p, s)| BookLevel::new(d(p), d(s)))
                .collect(),
            asks.iter()
                .map(|(p, s)| BookLevel::new(d(p), d(s)))
                .collect(),
        )
    };
    let depth = StaticLiquidity::new()
        .with_book(
            cx.clone(),
            book("ETHUSDT", &[("3000.0", "200")], &[("3000.1", "200")]),
            20,
        )
        .with_book(
            cx.clone(),
            book("ETHBTC", &[("0.0505", "200")], &[("0.05051", "200")]),
            20,
        )
        .with_book(
            cx,
            book("BTCUSDT", &[("60000", "10")], &[("60001", "10")]),
            20,
        );
    Ok((graph, depth))
}

/// A small QUBO the exhaustive classical solver can settle exactly, so the
/// baseline in the benchmark is a proof rather than another heuristic.
fn small_qubo() -> Qubo {
    let mut qubo = Qubo::new(8);
    // A deterministic spin glass without an RNG: a fixed irrational rotation,
    // so the same instance is solved on every machine and the benchmark is
    // comparable run to run.
    let mut value = 0.37_f64;
    let mut next = || {
        value = (value * 7.13).fract();
        value - 0.5
    };
    for i in 0..8 {
        qubo.add_linear(i, next());
        for j in (i + 1)..8 {
            qubo.add(i, j, next());
        }
    }
    qubo
}
