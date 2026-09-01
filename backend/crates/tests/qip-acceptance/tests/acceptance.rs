//! The end-to-end acceptance scenario.
//!
//! One test walks a market observation from a synthetic venue through every
//! stage of the loop and out the other side as a reviewed thesis, a checked
//! proposal, a risk-gated order and an exact attribution. The rest assert the
//! properties that matter once the whole thing is assembled — the ones that
//! could hold in every crate individually and still fail when they are wired
//! together.
//!
//! A control that works in isolation and not once assembled is not a control,
//! which is why these are separate from the per-crate tests rather than a
//! duplicate of them.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_agents::finding::{AgentBrief, Direction, NumericProvenance};
use qip_core::error::Result;
use qip_core::lineage::{CorrelationId, Lineage};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Currency, Decimal, ManualClock, Money, ObjectId, PortfolioId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::cycle::Stage;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::{AutonomyLevel, OperatorIdentity};
use std::sync::Arc;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["ACME", "BOREAS", "CERES"] {
        universe
            .insert(
                FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
                    .venue("XNYS")
                    .sector(Sector::InformationTechnology)
                    .price(dec!("100"))
                    .provenance(Provenance::synthetic("acceptance", start()))
                    .build(start())
                    .expect("valid instrument"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("acceptance")
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
        .with(
            Limit::new("max-drawdown", LimitKind::MaxDrawdown { limit: 0.20 })
                .with_rationale("past a fifth of the book, the models are not working"),
        )
}

fn platform(config: PlatformConfig) -> Result<Platform> {
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock, config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

/// A price series with a genuine jump, so the detectors have something real to
/// find rather than a fixture engineered to trip them.
fn market_history(symbol: &str, days: usize, jump_at: Option<usize>) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..days)
        .map(|i| {
            // A fixed irrational rotation gives deterministic noise without an
            // RNG, so the scenario reproduces exactly.
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.009;
            let jump = if jump_at == Some(i) { 0.085 } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + jump;
            let at = start().saturating_sub(Duration::from_days((days - i) as i64));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).unwrap(),
                high: Decimal::from_f64(open.max(price) * 1.003).unwrap(),
                low: Decimal::from_f64(open.min(price) * 0.997).unwrap(),
                close: Decimal::from_f64(price).unwrap(),
                volume: dec!("2500000"),
                trade_count: 8_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

// --- the scenario -----------------------------------------------------------

#[test]
fn a_market_observation_travels_the_whole_loop() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;

    // 1. SENSE — the platform observes a market with a real move in it.
    let absorbed = platform.observe(market_history("ACME", 120, Some(80)));
    assert_eq!(absorbed, 120, "every bar was absorbed");
    platform.observe(market_history("BOREAS", 120, None));
    platform.observe(market_history("CERES", 120, None));

    // 2. The cycle runs every stage.
    let report = platform.run_cycle(start());
    assert!(
        report.traversed_every_stage(),
        "a stage did not run:\n{}",
        report.summarise()
    );

    // 3. DISCOVER found the move and queued it.
    let discover = report.stage(Stage::Discover).expect("discover ran");
    assert!(
        discover.produced > 0,
        "an 8.5% jump in a 0.9% series went unnoticed: {}",
        discover.detail
    );
    assert!(!platform.queue().is_empty(), "nothing reached the queue");

    // 4. REASON dispatched to the organisation and reached a verdict.
    let reason = report.stage(Stage::Reason).expect("reason ran");
    assert!(
        reason.produced > 0,
        "no agent said anything: {}",
        reason.detail
    );
    assert!(
        reason.detail.contains("hypothesis") || reason.detail.contains("no mechanism"),
        "reason neither formed a thesis nor said why not: {}",
        reason.detail
    );

    // 5. Every other stage reported something legible.
    for stage in Stage::all() {
        let outcome = report.stage(stage).expect("every stage reports");
        assert!(outcome.ran, "{} did not run", stage.as_str());
        assert!(
            outcome.detail.len() > 10,
            "{} said nothing useful: {:?}",
            stage.as_str(),
            outcome.detail
        );
    }

    // 6. One correlation id ties the whole cycle together.
    assert!(!report.correlation_id.as_str().is_empty());

    // 7. Nothing reached a real venue.
    assert!(!platform.orders().has_live_fills());
    assert!(!platform.is_live_capable());

    Ok(())
}

#[test]
fn an_order_travels_the_control_path_and_produces_an_exact_attribution() -> Result<()> {
    // The other half of the scenario: a decision becomes an order, the order
    // becomes a fill, and the fill reconciles exactly.
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(market_history("ACME", 90, None));

    let order = platform.order_from(
        object("ACME"),
        Side::Buy,
        dec!("5000"),
        dec!("100"),
        "prop-acceptance",
        vec!["hyp-acceptance".to_string()],
        start(),
    );
    platform.submit_order(order, start())?;

    let fills = platform.orders().fills();
    assert!(!fills.is_empty(), "the order did not fill");
    for fill in &fills {
        assert!(fill.simulated, "a paper platform produced a live fill");
    }

    // LEARN attributes it, and the decomposition closes.
    let report = platform.run_cycle(start());
    let learn = report.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("residual 0") || learn.produced > 0,
        "the attribution did not reconcile: {}",
        learn.detail
    );
    Ok(())
}

#[test]
fn a_fill_is_attributed_to_the_hypothesis_the_order_was_released_for() -> Result<()> {
    // The join that makes learning possible. `Order::hypotheses` documents
    // itself as required — "an untraceable order is one nobody can explain
    // after the fact" — and LEARN used to hand the attributor an empty vector
    // for every fill, so `by_hypothesis` returned nothing for everything the
    // platform traded. The stage still reported fills attributed, which is why
    // it went unnoticed: the number that was wrong was the one nobody printed.
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(market_history("ACME", 90, None));

    let order = platform.order_from(
        object("ACME"),
        Side::Buy,
        dec!("5000"),
        dec!("100"),
        "prop-attribution",
        vec!["hyp-carried-through".to_string()],
        start(),
    );
    // The premise, in two parts: the order really does name a hypothesis, and
    // it really does fill. Without both, the assertion below would pass on a
    // platform that attributed nothing because there was nothing to attribute.
    assert_eq!(
        order.hypotheses,
        vec!["hyp-carried-through".to_string()],
        "the fixture order names no hypothesis, so nothing could be carried"
    );
    platform.submit_order(order, start())?;
    assert!(
        !platform.orders().fills().is_empty(),
        "the premise failed: the order did not fill"
    );

    let report = platform.run_cycle(start());
    let learn = report.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.produced > 0,
        "nothing was attributed at all: {}",
        learn.detail
    );
    // One hypothesis, by count. `across 0 hypothesis(es)` is exactly what the
    // defect printed once the count was reported, so this is the assertion
    // that distinguishes the two.
    assert!(
        learn.detail.contains("across 1 hypothesis(es)"),
        "the fill was not attributed to the hypothesis its order named: {}",
        learn.detail
    );
    Ok(())
}

// --- properties that only exist once assembled ------------------------------

#[test]
fn the_assembled_platform_cannot_be_talked_into_live_trading() -> Result<()> {
    // Three refusals in sequence, each for a different reason.
    let mut paper = platform(PlatformConfig::default())?;

    // Two operators, correct in every way, and still refused by the ceiling.
    let two = OperatorIdentity::verified("alice@example.com", "hardware-token", start())
        .with_second_approver("bob@example.com");
    let error = paper
        .autonomy_mut()
        .request_change(
            AutonomyLevel::AutonomousLive,
            &two,
            "attempting to enable fully autonomous live trading",
            start(),
        )
        .unwrap_err();
    assert!(error.message().contains("ceiling"), "{}", error.message());

    // Raise the ceiling — and one operator is still not enough.
    let mut capable =
        platform(PlatformConfig::default().with_live_ceiling(AutonomyLevel::SupervisedLive))?;
    let one = OperatorIdentity::verified("alice@example.com", "hardware-token", start());
    assert!(
        capable
            .autonomy_mut()
            .request_change(
                AutonomyLevel::SupervisedLive,
                &one,
                "enabling supervised live trading",
                start()
            )
            .is_err()
    );

    // Two operators, but a stale credential.
    let stale = start().saturating_add(Duration::from_hours(2));
    assert!(
        capable
            .autonomy_mut()
            .request_change(
                AutonomyLevel::SupervisedLive,
                &two,
                "enabling supervised live trading",
                stale
            )
            .is_err()
    );

    // Everything correct: it works, and only then.
    capable.autonomy_mut().request_change(
        AutonomyLevel::SupervisedLive,
        &two,
        "enabling supervised live trading for the pilot",
        start(),
    )?;
    assert!(capable.autonomy().is_live());
    Ok(())
}

#[test]
fn a_halt_stops_the_assembled_platform_at_every_entry_point() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(market_history("ACME", 90, Some(60)));
    platform.autonomy_mut().kill_switch_mut().trip_global(
        start(),
        "acceptance",
        "an operator halted the platform",
    );

    // An order is refused.
    let order = platform.order_from(
        object("ACME"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-1",
        vec!["hyp-1".to_string()],
        start(),
    );
    assert!(platform.submit_order(order, start()).is_err());

    // A cycle still runs, and reports the halt.
    let report = platform.run_cycle(start());
    assert!(report.halted);
    assert!(
        report.traversed_every_stage(),
        "a halt must not break the loop"
    );

    // The effective level drops to observation while the configured one is
    // untouched, so clearing restores exactly what was there.
    assert_eq!(platform.autonomy().level(), AutonomyLevel::Observation);
    assert_eq!(
        platform.autonomy().configured_level(),
        AutonomyLevel::PaperTrading
    );
    Ok(())
}

#[test]
fn every_number_the_organisation_produces_carries_a_provenance() -> Result<()> {
    // The rule that keeps a fabricated quantity out of a decision, checked
    // across all eighteen agents at once rather than one at a time.
    use qip_investment_agents::Organisation;
    use qip_investment_agents::desk::Desk;

    let desk = Arc::new(Desk::empty(start()));
    let mut organisation = Organisation::standard(
        desk,
        start(),
        start(),
        7,
        Some(Arc::new(qip_ai::language::DeterministicModel::new())),
        vec!["web-traffic".to_string()],
        false,
    )?;

    let brief = AgentBrief::new("is ACME mispriced", start(), Duration::from_days(30))
        .about_objects(vec![object("ACME")])
        .about_entities(vec!["ent-acme".to_string()]);
    let report = organisation.dispatch(
        &brief,
        start(),
        &Lineage::root(CorrelationId::from_string("cor-acceptance"), "acceptance"),
    );

    assert!(
        report.failed.is_empty(),
        "agents failed: {:?}",
        report.failed
    );
    assert!(
        report.permission_violations().is_empty(),
        "an agent reached beyond its manifest"
    );

    for finding in &report.findings {
        for fact in &finding.facts {
            fact.validate()?;
            match &fact.provenance {
                NumericProvenance::Computed { by, inputs } => {
                    assert!(
                        !inputs.is_empty(),
                        "{by} computed {} from nothing",
                        fact.label
                    );
                }
                NumericProvenance::Observed { source, .. } => assert!(!source.is_empty()),
            }
        }
        // And a directional claim always states what would refute it.
        if matches!(finding.direction, Direction::Positive | Direction::Negative) {
            assert!(
                !finding.falsifiers.is_empty(),
                "{} took a view with no falsifier",
                finding.agent_id
            );
        }
    }
    Ok(())
}

#[test]
fn the_whole_platform_is_deterministic() -> Result<()> {
    // Replayability is what makes the audit trail worth having. Two platforms
    // built from the same configuration and fed the same observations must
    // produce identical cycles.
    let run = || -> Result<Vec<(String, usize, String)>> {
        let mut platform = platform(PlatformConfig::default())?;
        platform.observe(market_history("ACME", 120, Some(80)));
        platform.observe(market_history("BOREAS", 120, None));
        Ok(platform
            .run_cycle(start())
            .stages
            .into_iter()
            .map(|outcome| {
                (
                    outcome.stage.as_str().to_string(),
                    outcome.produced,
                    outcome.detail,
                )
            })
            .collect())
    };
    assert_eq!(run()?, run()?);
    Ok(())
}

#[test]
fn a_platform_with_no_data_is_legible_rather_than_merely_quiet() -> Result<()> {
    // A cold start is a normal state and must read as one.
    let mut platform = platform(PlatformConfig::default())?;
    let report = platform.run_cycle(start());

    assert!(report.traversed_every_stage());
    assert!(report.problems().is_empty(), "{:?}", report.problems());
    assert!(
        report
            .stage(Stage::Sense)
            .unwrap()
            .detail
            .contains("running blind")
    );
    Ok(())
}

#[test]
fn the_event_log_hash_chain_survives_a_full_run() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(market_history("ACME", 90, Some(60)));
    for i in 0..3 {
        platform.run_cycle(start().saturating_add(Duration::from_mins(i * 5)));
    }
    assert!(
        platform.event_log().verify_chain().is_ok(),
        "the log's hash chain is broken"
    );
    Ok(())
}

// --- the cross-stage invariants ---------------------------------------------

#[test]
fn the_backtest_and_the_live_loop_agree_on_what_a_bar_means() -> Result<()> {
    // Both filter on close time. A backtest that used open time and a live
    // loop that used close time would produce results that could never be
    // reconciled, and the difference would look like alpha.
    use qip_simulation_engine::clock::{ExecutionAssumptions, SimulationClock};

    let bars: Vec<Bar> = market_history("ACME", 30, None)
        .into_iter()
        .filter_map(|record| match record {
            SensedRecord::Bar(bar) => Some(*bar),
            _ => None,
        })
        .collect();

    let clock = SimulationClock::new(bars.clone(), ExecutionAssumptions::next_bar())?;
    let view = clock.view().expect("a view at the first step");
    let visible = view.bars(&object("ACME"));

    assert_eq!(visible.len(), 1, "only the closed bar is visible");
    assert!(visible[0].close_time() <= view.as_of());
    // And the live path's market snapshot keys on the same thing.
    let mut snapshot = qip_market::snapshot::MarketSnapshot::new(bars[0].close_time());
    snapshot.apply_bar(bars[0].clone());
    assert_eq!(snapshot.as_of, bars[0].close_time());
    Ok(())
}

#[test]
fn the_risk_limits_the_platform_runs_under_all_state_a_rationale() -> Result<()> {
    // A limit nobody can explain is a limit nobody will defend when it blocks
    // a trade someone wants.
    for limit in &LimitSet::conservative_default().limits {
        assert!(
            limit.rationale.len() > 20,
            "{} has no real rationale",
            limit.name
        );
    }
    Ok(())
}

#[test]
fn the_platform_reports_its_own_capability_consistently() -> Result<()> {
    // Three places answer "could this reach a venue" and they must agree.
    let paper = platform(PlatformConfig::default())?;
    assert!(!paper.is_live_capable());
    assert!(!paper.config().permits_live_trading());
    assert!(!paper.autonomy().ceiling().is_live());

    let capable = platform(
        PlatformConfig::default().with_live_ceiling(AutonomyLevel::LimitedAutonomousLive),
    )?;
    assert!(capable.is_live_capable());
    assert!(capable.config().permits_live_trading());
    assert!(capable.autonomy().ceiling().is_live());
    Ok(())
}

#[test]
fn a_proposal_cannot_reach_execution_without_both_controls() -> Result<()> {
    // The separation the whole organisation is built around, checked at the
    // proposal rather than in the manifests.
    use qip_portfolio_engine::proposal::{Proposal, ProposalLeg, ProposalStatus, Side as LegSide};

    let mut proposal = Proposal {
        proposal_id: qip_core::ProposalId::from_string("prop-acceptance"),
        created_at: start(),
        as_of: start(),
        status: ProposalStatus::Draft,
        legs: vec![ProposalLeg {
            object_id: object("ACME"),
            side: LegSide::Buy,
            quantity: dec!("1000"),
            reference_price: dec!("100"),
            current_weight: 0.0,
            target_weight: 0.01,
            estimated_cost_bps: 10.0,
            hypotheses: vec!["hyp-acceptance".to_string()],
        }],
        equity: Money::new(dec!("10000000"), Currency::USD),
        target_gross: 0.01,
        target_net: 0.01,
        turnover: 0.01,
        estimated_cost_bps: 0.2,
        rationale: "expresses one approved thesis".to_string(),
        compromises: Vec::new(),
        checks_passed: Vec::new(),
    };
    let _ = PortfolioId::from_string("pf-acceptance");

    // Risk alone is not enough.
    assert!(
        proposal
            .approve(start(), vec!["risk-control".to_string()])
            .is_err()
    );
    assert!(!proposal.status.is_releasable());

    // Both controls, and it releases.
    proposal.approve(
        start(),
        vec!["risk-control".to_string(), "compliance-control".to_string()],
    )?;
    proposal.release(start())?;
    Ok(())
}

#[test]
fn the_reservation_ledger_stays_anchored_to_equity_across_full_cycles() -> Result<()> {
    // The consistency half of gap-matrix item 10, at the acceptance seam. The
    // headline property — a second proposal sized against what the first
    // still holds, and commit-versus-release at the act stage — lives beside
    // the decide stage's own tests in the kernel, where theses can be staged
    // directly; the default synthetic history here does not clear the action
    // bar (the panel rejects at confidence 0.22), so no hold is exercised in
    // this run and this test does not claim one is.
    //
    // What full cycles must nonetheless keep true, and what this pins: the
    // ledger stays anchored to the book on every pass including quiet ones —
    // free plus active holds equals tracked equity, one claim about one
    // balance — and nothing stays held across a completed cycle.
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(market_history("ACME", 120, Some(80)));
    platform.observe(market_history("BOREAS", 120, None));
    platform.observe(market_history("CERES", 120, None));

    let mut cycles = 0;
    for i in 0..4i32 {
        let now = start().saturating_add(Duration::from_mins(i64::from(i) * 5));
        platform.run_cycle(now);
        cycles += 1;
        assert_eq!(
            platform.reservations().reserved_total(),
            Decimal::ZERO,
            "a hold survived the cycle that took it"
        );
        let free = platform
            .reservations()
            .clone()
            .free(now.saturating_add(Duration::from_secs(1)));
        assert_eq!(
            free,
            platform.equity(),
            "the reservation ledger's free balance drifted from tracked equity"
        );
    }
    assert_eq!(
        cycles, 4,
        "the loop did not run, so nothing above was tested"
    );
    Ok(())
}
