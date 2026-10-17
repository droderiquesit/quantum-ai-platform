//! The vertical slice, desk half: one simulated tick to a learned fill.
//!
//! ```text
//! tick -> IngestionService (gate) -> EventBus + EventLog -> [test glue] -> Platform::observe
//!      -> run_cycle (SENSE..LEARN) -> submit_order (OMS: validate, kill switch, autonomy,
//!         pre-trade risk against tracked equity) -> simulated broker -> fill
//!      -> capital tracking -> OutcomeCapture (hash chain) -> LEARN (attribution, twin score)
//! ```
//!
//! Every component is the production type. The one hop that is NOT production
//! wiring is `bus -> Platform::observe`: no production code subscribes
//! `Platform` to an `EventBus` (`grep -rn 'bus.on::<\|on_all(' runtime apps
//! services` outside tests is empty on 2026-10-17), so this file's subscriber
//! is the glue a composition root would own, and it is named as such where it
//! stands. The seam is a finding, not something this file papers over.
//!
//! The second half of the slice (cell fill -> spool -> double-entry ledger) is
//! `vertical_slice_cell_ledger.rs`. The two halves do not join in process:
//! a desk `Fill` never becomes an `OutcomeRecord`, so the desk never reaches
//! the ledger; the cell's fill never reaches `Platform::capital`. See the hop
//! tables in the report that accompanies these files.
//!
//! Negative tests assert the same invariant after every failure: no fill, no
//! position change, no cash change, and a refusal row on the outcome chain
//! rather than silence.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::lineage::CorrelationId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, ObjectId, dec};
use qip_events::{EventBus, EventLog, HandlerOutcome, Topic};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, LicensingClass, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::cycle::Stage;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market::quote::Tick;
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::{IngestionService, ReplayAdapter};
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::{AutonomyLevel, Identification, OperatorIdentity};
use qip_twin::capture::Action;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

// --- fixtures ---------------------------------------------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn acme() -> ObjectId {
    ObjectId::from_string("obj-ACME")
}

fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0)
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                acme(),
                "ACME",
                InstrumentType::CommonStock,
                fixture_liquidity(),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("vertical-slice", start()))
            .build(start())
            .expect("valid instrument"),
        )
        .expect("insertable");
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("vertical-slice")
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

fn platform_with(config: PlatformConfig) -> Result<Platform> {
    let clock = Arc::new(ManualClock::new(start()));
    let context = Context::new(clock, config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

fn platform() -> Result<Platform> {
    platform_with(PlatformConfig::default())
}

/// A flat daily tape at 100, ending the day before `start()`, so the twin has
/// history to estimate liquidity from and the first fill has a reference.
fn history(days: i64) -> Vec<SensedRecord> {
    (1..=days)
        .rev()
        .map(|back| {
            let at = start().saturating_sub(Duration::from_days(back));
            SensedRecord::Bar(Box::new(bar(at, 100.0, 100.0)))
        })
        .collect()
}

fn bar(at: Timestamp, open: f64, close: f64) -> Bar {
    Bar {
        object_id: acme(),
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
    }
}

/// A flat tape at 100 after `from`, one bar a day.
fn flat_after(from: Timestamp, days: i64) -> Vec<SensedRecord> {
    (1..=days)
        .map(|day| {
            let at = from.saturating_add(Duration::from_days(day));
            SensedRecord::Bar(Box::new(bar(at, 100.0, 100.0)))
        })
        .collect()
}

fn tick(price: Decimal, at: Timestamp, captured: Timestamp) -> Tick {
    Tick {
        object_id: acme(),
        venue: "XNYS".to_string(),
        at,
        capture_time: captured,
        sequence: Some(1),
        price,
        volume: dec!("100"),
        quality: DataQuality::default(),
    }
}

/// What the ingestion service and the bus did with a batch.
struct Delivered {
    /// What reached the (test-owned) platform subscriber, in bus order.
    records: Vec<SensedRecord>,
    log: Rc<RefCell<EventLog>>,
    published: u64,
    rejected: u64,
    duplicates_suppressed: u64,
    failures_on_bus: usize,
}

/// Replay `records` through the real ingestion gate onto a real `EventBus`
/// backed by a real `EventLog`, and collect what a subscriber on the market
/// topics receives.
///
/// The subscriber is the glue named in the module doc: it stands where a
/// composition root's `bus -> Platform::observe` bridge would stand, and it is
/// the only part of this function that is not production wiring.
fn through_ingestion_and_bus(records: Vec<SensedRecord>, until: Timestamp) -> Result<Delivered> {
    let (context, clock) = Context::deterministic(start(), 7);
    clock.set(until);
    let log = Rc::new(RefCell::new(EventLog::in_memory()));
    let mut bus = EventBus::new().with_log(Rc::clone(&log));
    let received: Rc<RefCell<Vec<SensedRecord>>> = Rc::new(RefCell::new(Vec::new()));

    let sink = Rc::clone(&received);
    bus.on::<Tick, _>("platform-bridge-tick", move |envelope, _any, _publisher| {
        sink.borrow_mut()
            .push(SensedRecord::Tick(envelope.body.clone()));
        Ok(HandlerOutcome::Handled)
    });
    let sink = Rc::clone(&received);
    bus.on::<Bar, _>("platform-bridge-bar", move |envelope, _any, _publisher| {
        sink.borrow_mut()
            .push(SensedRecord::Bar(Box::new(envelope.body.clone())));
        Ok(HandlerOutcome::Handled)
    });

    let mut service = IngestionService::new(Telemetry::silent());
    service.register(Box::new(
        ReplayAdapter::from_records("simulated-tape", records)
            .with_licensing(LicensingClass::Synthetic),
    ));
    service.start(start())?;
    let published = service.poll_and_publish(&context, &mut bus, until)?;
    bus.drain(&context)?;

    let delivered = Delivered {
        records: received.borrow().clone(),
        log,
        published,
        rejected: service.stats().rejected,
        duplicates_suppressed: bus.duplicates_suppressed(),
        failures_on_bus: bus.failure_count(),
    };
    Ok(delivered)
}

// --- the happy path, hop by hop ---------------------------------------------

#[test]
fn a_simulated_tick_travels_from_ingestion_to_a_learned_paper_fill() -> Result<()> {
    let mut platform = platform()?;
    let initial_equity = platform.equity();

    // HOP 1-3: ingestion gate, normalisation onto the bus, delivery.
    // The tick prints 30s after the last daily bar closes and reaches this
    // platform 40ms later; the two instants are the bitemporal pair.
    let printed = start().saturating_add(Duration::from_secs(30));
    let captured = printed.saturating_add(Duration::from_millis(40));
    let mut tape = history(60);
    tape.push(SensedRecord::Tick(tick(dec!("100.25"), printed, captured)));
    let expected = tape.len();
    let until = start().saturating_add(Duration::from_secs(31));
    let delivered = through_ingestion_and_bus(tape, until)?;
    assert_eq!(
        delivered.published as usize, expected,
        "ingestion published every record"
    );
    assert_eq!(delivered.rejected, 0);
    assert_eq!(delivered.failures_on_bus, 0, "no subscriber failed");
    assert_eq!(
        delivered.records.len(),
        expected,
        "the bus delivered every record to the bridge"
    );

    // The tick's envelope on the log: one correlation id, root lineage, the
    // producer that minted it, and the body the adapter produced.
    let (tick_correlation, tick_price) = {
        let log = delivered.log.borrow();
        let ticks = log.by_topic(Topic::MarketTick);
        assert_eq!(ticks.len(), 1, "premise: exactly one tick was published");
        let event = ticks[0];
        assert_eq!(event.lineage.producer, "market-ingestion");
        assert!(
            event.lineage.causation_id.is_none(),
            "a tick starts its chain"
        );
        let correlation = event.lineage.correlation_id.clone();
        assert!(!correlation.as_str().is_empty());
        assert_eq!(
            log.by_correlation(&correlation).len(),
            1,
            "the correlation id selects exactly the tick's own chain"
        );
        let decoded = event.decode::<Tick>()?;
        (correlation, decoded.body.price)
    };
    assert_eq!(
        tick_price,
        dec!("100.25"),
        "normalisation preserved the price exactly"
    );
    assert!(
        delivered.log.borrow().verify_chain().is_ok(),
        "the bus log is hash-chained"
    );

    // HOP 4: the platform absorbs the delivered records.
    let absorbed = platform.observe(delivered.records);
    assert_eq!(absorbed, expected);
    // Bitemporal: the tick is readable once it is knowable, and not before.
    let id = acme();
    let world = platform.world();
    let before_capture = world
        .features()
        .history("close", id.as_str(), printed)
        .len();
    let after_capture = world
        .features()
        .history("close", id.as_str(), captured)
        .len();
    assert_eq!(
        after_capture,
        before_capture + 1,
        "the tick became readable at its capture instant and not at its print instant"
    );
    assert_eq!(
        world
            .features()
            .current("close", id.as_str(), captured)
            .map(|v| v.value),
        Some(100.25_f64),
    );
    drop(world);

    // HOP 5-6: the cycle runs every stage; DECIDE produced a decision record.
    let now = start().saturating_add(Duration::from_mins(2));
    let report = platform.run_cycle(now);
    assert!(report.traversed_every_stage(), "{}", report.summarise());
    assert_eq!(
        platform.last_correlation(),
        Some(report.correlation_id.clone())
    );
    assert_eq!(report.stages.len(), Stage::all().len());
    assert!(
        !platform.proposals().is_empty(),
        "DECIDE created a proposal record"
    );
    let proposal = platform.proposals().last().expect("a proposal");
    assert_eq!(
        proposal.equity.amount, initial_equity,
        "sized against the tracked book"
    );
    // The cycle's own correlation is distinct from the tick's: observe() takes
    // no lineage (see the seam test below).
    assert_ne!(report.correlation_id, tick_correlation);

    // HOP 7-9: a released order goes through the OMS: risk, authorisation, venue.
    let order = platform.order_from(
        acme(),
        Side::Buy,
        dec!("700"),
        dec!("100.25"),
        "prop-vertical-slice",
        vec!["hyp-vertical-slice".to_string()],
        now,
    );
    let order_id = order.order_id.clone();
    platform.submit_order(order, now)?;
    let fills = platform.orders().fills();
    assert_eq!(fills.len(), 1, "one order, one fill");
    assert!(fills[0].simulated, "the fill is a paper fill");
    assert_eq!(fills[0].venue, "simulated-venue");
    assert_eq!(fills[0].order_id, order_id);
    assert!(!platform.orders().has_live_fills());
    assert!(!platform.is_live_capable());
    let (quantity, price, costs) = (fills[0].quantity, fills[0].price, fills[0].costs);
    assert!(quantity.is_positive());

    // HOP 10: position and cash agree with the fill, to the unit.
    let positions = platform.positions_at_cost();
    let (held, average) = positions.get("obj-ACME").copied().expect("a position");
    assert_eq!(held, quantity);
    assert_eq!(average, price);
    assert_eq!(platform.cash(), initial_equity - quantity * price - costs);
    assert_eq!(platform.trading_costs(), costs);
    assert_eq!(
        platform.equity(),
        initial_equity - costs,
        "opening a position realises only its costs"
    );
    assert_eq!(platform.realised_pnl(), Decimal::ZERO);

    // HOP 11: the outcome chain holds the order and the fill under the cycle's
    // correlation id, and the chain verifies.
    let outcomes = platform.outcomes();
    outcomes.verify()?;
    let taken = outcomes.taken();
    assert!(
        taken.iter().any(|entry| matches!(&entry.decision.action,
            Action::Filled { order_id: id, .. } if *id == order_id)),
        "no Filled row for the order on the outcome chain"
    );
    for entry in outcomes.entries() {
        assert_eq!(
            entry.decision.correlation, report.correlation_id,
            "an outcome row carries a correlation id other than the cycle's"
        );
    }

    // HOP 12: LEARN attributes the fill to its hypothesis and, once the horizon
    // has passed, the twin prices it.
    assert_eq!(
        platform.filled_awaiting_score(),
        1,
        "premise: the fill awaits its horizon"
    );
    platform.observe(flat_after(start(), 6));
    let later = platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    let learn = later.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("across 1 hypothesis(es)"),
        "attribution did not reach the hypothesis the order named: {}",
        learn.detail
    );
    assert!(
        learn.detail.contains("filled path(s) priced for size"),
        "the twin did not price the fill: {}",
        learn.detail
    );
    assert_eq!(platform.filled_awaiting_score(), 0);
    let scores = platform.fill_scores();
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0].order_id, order_id);
    let journal = platform.journal_entries()?;
    assert_eq!(journal.len(), 2, "two cycles, two sealed journal entries");
    assert_eq!(journal[0].correlation_id, report.correlation_id.as_str());
    assert_eq!(journal[1].correlation_id, later.correlation_id.as_str());
    assert_ne!(report.correlation_id, later.correlation_id);
    // The paper boundary after the whole run.
    assert!(!platform.is_live_capable());
    assert_eq!(platform.autonomy().level(), AutonomyLevel::PaperTrading);
    let _: CorrelationId = later.correlation_id;
    Ok(())
}

// --- the invariant every failure must leave behind ---------------------------

/// Everything a failed order must not have moved.
#[derive(Debug, PartialEq)]
struct Book {
    cash: Decimal,
    equity: Decimal,
    costs: Decimal,
    realised: Decimal,
    positions: std::collections::BTreeMap<String, (Decimal, Decimal)>,
    fills: usize,
    reserved: Decimal,
}

fn book(platform: &Platform) -> Book {
    Book {
        cash: platform.cash(),
        equity: platform.equity(),
        costs: platform.trading_costs(),
        realised: platform.realised_pnl(),
        positions: platform.positions_at_cost(),
        fills: platform.orders().fills().len(),
        reserved: platform.reservations().reserved_total(),
    }
}

/// A platform with history and one tick absorbed, already cycled once, so the
/// failures below happen in a platform that has done real work.
fn warm_platform() -> Result<Platform> {
    let mut platform = platform()?;
    let printed = start().saturating_add(Duration::from_secs(30));
    let captured = printed.saturating_add(Duration::from_millis(40));
    let mut tape = history(60);
    tape.push(SensedRecord::Tick(tick(dec!("100"), printed, captured)));
    let delivered =
        through_ingestion_and_bus(tape, start().saturating_add(Duration::from_secs(31)))?;
    platform.observe(delivered.records);
    platform.run_cycle(start().saturating_add(Duration::from_mins(2)));
    Ok(platform)
}

fn order(
    platform: &mut Platform,
    quantity: Decimal,
    hypotheses: Vec<String>,
) -> qip_execution_engine::order::Order {
    platform.order_from(
        acme(),
        Side::Buy,
        quantity,
        dec!("100"),
        "prop-negative",
        hypotheses,
        start().saturating_add(Duration::from_mins(3)),
    )
}

fn later() -> Timestamp {
    start().saturating_add(Duration::from_mins(3))
}

/// The refusal row a failed order must leave: a `Rejected` outcome naming the
/// gate, so a refusal is distinguishable from a decision nobody made.
fn assert_refused_under(platform: &Platform, order_id: &qip_core::OrderId, gate: &str) {
    platform
        .outcomes()
        .verify()
        .expect("the outcome chain still verifies");
    let rows = platform.outcomes().refusals();
    assert!(
        rows.iter().any(|row| matches!(&row.decision.action,
            Action::Rejected { order_id: id, gate: g, .. } if id == order_id && g == gate)),
        "no Rejected row under gate `{gate}` for {}: {:?}",
        order_id.as_str(),
        platform.outcomes().tally()
    );
    let refusal = platform
        .orders()
        .refusals()
        .iter()
        .find(|result| &result.order_id == order_id)
        .expect("the order manager recorded the refusal");
    assert!(!refusal.accepted);
    assert!(refusal.fills.is_empty(), "a refused order carries fills");
    assert_eq!(
        refusal.venue, None,
        "a refused order names a venue: the broker was reached"
    );
}

// --- negative: risk ----------------------------------------------------------

#[test]
fn a_risk_rejected_decision_reaches_no_broker_and_leaves_every_ledger_untouched() -> Result<()> {
    let mut platform = warm_platform()?;
    // Premise: the platform can fill a sane order, so the refusal below is the
    // risk limit and not a platform that cannot trade.
    let sane = order(&mut platform, dec!("100"), vec!["hyp-sane".to_string()]);
    platform.submit_order(sane, later())?;
    assert_eq!(platform.orders().fills().len(), 1);
    let before = book(&platform);
    let rows_before = platform.outcomes().len();

    // 30 million of a 10 million book against a 10% position-weight limit.
    let oversize = order(
        &mut platform,
        dec!("300000"),
        vec!["hyp-oversize".to_string()],
    );
    let id = oversize.order_id.clone();
    let error = platform.submit_order(oversize, later()).unwrap_err();
    assert!(
        error.message().contains("max-position-weight") || error.message().contains("max-leverage"),
        "the refusal does not name the limit that fired: {}",
        error.message()
    );
    assert_eq!(book(&platform), before, "a refused order moved the book");
    assert_eq!(
        platform.outcomes().len(),
        rows_before + 1,
        "exactly one refusal row"
    );
    assert_refused_under(&platform, &id, "pre-trade-risk");
    let state = platform
        .orders()
        .order(&id)
        .expect("recorded")
        .state
        .as_str();
    assert_eq!(state, "rejected");
    Ok(())
}

#[test]
fn insufficient_capital_stops_the_order_and_a_reservation_beyond_free_capital_is_denied()
-> Result<()> {
    // A 5,000 book cannot carry 700 shares at 100: the order is stopped by the
    // pre-trade limits sized against the tracked book, before any venue call.
    let config = PlatformConfig::default().with_initial_equity(dec!("5000"));
    let mut platform = platform_with(config)?;
    platform.observe(history(60));
    platform.run_cycle(start().saturating_add(Duration::from_mins(2)));
    assert_eq!(
        platform.equity(),
        dec!("5000"),
        "premise: the book is the small one"
    );
    let before = book(&platform);

    let order = order(
        &mut platform,
        dec!("700"),
        vec!["hyp-underfunded".to_string()],
    );
    let id = order.order_id.clone();
    assert!(platform.submit_order(order, later()).is_err());
    assert_eq!(book(&platform), before);
    assert_refused_under(&platform, &id, "pre-trade-risk");

    // The reservation ledger the DECIDE stage sizes against refuses to hold
    // more than is free, and a refused reservation holds nothing.
    let mut ledger = platform.reservations().clone();
    let free = ledger.free(later());
    assert_eq!(
        free,
        platform.equity(),
        "free capital is the tracked equity"
    );
    let denied = ledger
        .reserve(
            "prop-too-big",
            free + dec!("1"),
            later(),
            Duration::from_mins(5),
        )
        .unwrap_err();
    assert!(denied.message().contains("free"), "{}", denied.message());
    assert_eq!(ledger.reserved_total(), Decimal::ZERO);
    assert_eq!(
        ledger.free(later()),
        free,
        "a denied reservation moved the free balance"
    );
    Ok(())
}

// --- negative: authorisation -------------------------------------------------

#[test]
fn an_order_below_the_executing_autonomy_level_is_refused_and_nothing_moves() -> Result<()> {
    let mut platform = warm_platform()?;
    let operator = OperatorIdentity::verified("alice@example.com", "hardware-token", later());
    platform.autonomy_mut().request_change(
        AutonomyLevel::Advisory,
        &operator,
        "stepping down to advisory for the review",
        Some(Identification::Identified),
        later(),
    )?;
    assert_eq!(
        platform.autonomy().level(),
        AutonomyLevel::Advisory,
        "premise"
    );
    let before = book(&platform);

    let order = order(&mut platform, dec!("100"), vec!["hyp-advisory".to_string()]);
    let id = order.order_id.clone();
    let error = platform.submit_order(order, later()).unwrap_err();
    assert!(error.message().contains("autonomy"), "{}", error.message());
    assert_eq!(book(&platform), before);
    assert_refused_under(&platform, &id, "autonomy");
    Ok(())
}

#[test]
fn a_tripped_kill_switch_stops_an_order_that_would_otherwise_fill() -> Result<()> {
    let mut platform = warm_platform()?;
    let fine = order(&mut platform, dec!("100"), vec!["hyp-fine".to_string()]);
    platform.submit_order(fine, later())?;
    assert_eq!(
        platform.orders().fills().len(),
        1,
        "premise: it trades until halted"
    );
    platform.autonomy_mut().kill_switch_mut().trip_global(
        later(),
        "vertical-slice",
        "an operator halted the platform",
    );
    let before = book(&platform);

    let order = order(&mut platform, dec!("100"), vec!["hyp-halted".to_string()]);
    let id = order.order_id.clone();
    assert!(platform.submit_order(order, later()).is_err());
    assert_eq!(book(&platform), before);
    assert_refused_under(&platform, &id, "kill-switch");
    Ok(())
}

#[test]
fn nothing_in_the_slice_can_be_talked_into_a_live_path() -> Result<()> {
    let mut platform = warm_platform()?;
    let two = OperatorIdentity::verified("alice@example.com", "hardware-token", later())
        .with_second_approver("bob@example.com");
    let error = platform
        .autonomy_mut()
        .request_change(
            AutonomyLevel::AutonomousLive,
            &two,
            "attempting to enable fully autonomous live trading",
            Some(Identification::Identified),
            later(),
        )
        .unwrap_err();
    assert!(error.message().contains("ceiling"), "{}", error.message());
    assert!(!platform.is_live_capable());
    assert_eq!(platform.autonomy().level(), AutonomyLevel::PaperTrading);
    let fine = order(&mut platform, dec!("100"), vec!["hyp-paper".to_string()]);
    platform.submit_order(fine, later())?;
    for fill in platform.orders().fills() {
        assert!(fill.simulated, "a live-flagged fill exists");
        assert_eq!(fill.venue, "simulated-venue");
    }
    assert!(!platform.orders().has_live_fills());
    Ok(())
}

// --- negative: malformed -----------------------------------------------------

#[test]
fn a_malformed_order_is_refused_as_such_and_leaves_the_book_alone() -> Result<()> {
    let mut platform = warm_platform()?;
    let before = book(&platform);

    // Traces to no hypothesis: "an untraceable order is one nobody can explain".
    let untraceable = order(&mut platform, dec!("100"), Vec::new());
    let id = untraceable.order_id.clone();
    assert!(platform.submit_order(untraceable, later()).is_err());
    assert_refused_under(&platform, &id, "order-validation");

    // A zero quantity is refused, not rounded up to something tradable.
    let empty = order(&mut platform, Decimal::ZERO, vec!["hyp-zero".to_string()]);
    let id = empty.order_id.clone();
    assert!(platform.submit_order(empty, later()).is_err());
    assert_refused_under(&platform, &id, "order-validation");

    assert_eq!(book(&platform), before);
    Ok(())
}

#[test]
fn malformed_market_data_is_rejected_at_the_gate_and_never_reaches_the_platform() -> Result<()> {
    let printed = start().saturating_add(Duration::from_secs(30));
    // A zero price, and a record that claims to have arrived before it happened.
    let zero_price = tick(
        Decimal::ZERO,
        printed,
        printed.saturating_add(Duration::from_millis(1)),
    );
    let time_travel = tick(
        dec!("100"),
        printed,
        printed.saturating_sub(Duration::from_secs(5)),
    );
    let mut tape = history(5);
    tape.push(SensedRecord::Tick(zero_price));
    tape.push(SensedRecord::Tick(time_travel));
    let delivered =
        through_ingestion_and_bus(tape, start().saturating_add(Duration::from_secs(31)))?;

    assert_eq!(delivered.rejected, 2, "both malformed ticks were rejected");
    assert_eq!(
        delivered.published, 5,
        "only the five good bars were published"
    );
    assert_eq!(
        delivered.records.len(),
        5,
        "no malformed tick reached a subscriber"
    );
    let log = delivered.log.borrow();
    assert!(
        log.by_topic(Topic::MarketTick).is_empty(),
        "a malformed tick is on the market topic"
    );
    let failures = log.by_topic(Topic::DataQualityFailed);
    assert_eq!(
        failures.len(),
        2,
        "each rejection is a DataQualityFailed event"
    );
    assert!(
        failures
            .iter()
            .all(|event| event.lineage.producer == "market-ingestion")
    );
    drop(log);

    // The platform fed only what survived is intact and still legible.
    let mut platform = platform()?;
    assert_eq!(platform.observe(delivered.records), 5);
    let report = platform.run_cycle(start().saturating_add(Duration::from_mins(2)));
    assert!(report.traversed_every_stage());
    assert!(platform.orders().fills().is_empty());
    Ok(())
}

// --- negative: duplicate -----------------------------------------------------

#[test]
fn a_duplicate_tick_is_suppressed_by_the_bus_and_the_platform_absorbs_it_once() -> Result<()> {
    let printed = start().saturating_add(Duration::from_secs(30));
    let captured = printed.saturating_add(Duration::from_millis(40));
    let one = tick(dec!("100.5"), printed, captured);
    let tape = vec![SensedRecord::Tick(one.clone()), SensedRecord::Tick(one)];
    let delivered =
        through_ingestion_and_bus(tape, start().saturating_add(Duration::from_secs(31)))?;

    assert_eq!(
        delivered.published, 2,
        "premise: ingestion published the replay twice"
    );
    assert_eq!(
        delivered.duplicates_suppressed, 1,
        "the bus suppressed the repeat"
    );
    assert_eq!(delivered.records.len(), 1, "the subscriber saw it once");
    assert_eq!(delivered.log.borrow().by_topic(Topic::MarketTick).len(), 1);

    let mut platform = platform()?;
    assert_eq!(platform.observe(delivered.records), 1);
    let world = platform.world();
    assert_eq!(
        world
            .features()
            .history("close", "obj-ACME", captured)
            .len(),
        1
    );
    Ok(())
}

// --- negative: missing dependency --------------------------------------------

#[test]
fn a_source_the_licence_gate_refuses_is_reported_and_publishes_nothing() -> Result<()> {
    let (context, _clock) = Context::deterministic(start(), 7);
    let mut bus = EventBus::new();
    let mut service =
        IngestionService::new(Telemetry::silent()).enforcing_production_licensing(true);
    service.register(Box::new(
        ReplayAdapter::from_records("simulated-tape", history(3))
            .with_licensing(LicensingClass::Synthetic),
    ));
    let until = start().saturating_add(Duration::from_secs(31));

    let batch = service.poll_batch(until, until)?;
    assert_eq!(batch.refused_sources, vec!["simulated-tape".to_string()]);
    assert!(batch.accepted.is_empty());

    let published = service.poll_and_publish(&context, &mut bus, until)?;
    assert_eq!(published, 0);
    assert_eq!(bus.queued(), 0, "nothing was queued from a refused source");
    Ok(())
}

#[test]
fn a_platform_with_no_market_data_runs_legibly_and_trades_nothing() -> Result<()> {
    let mut platform = platform()?;
    let report = platform.run_cycle(start());
    assert!(report.traversed_every_stage());
    let sense = report.stage(Stage::Sense).expect("sense ran");
    assert!(sense.detail.contains("running blind"), "{}", sense.detail);
    assert!(platform.orders().fills().is_empty());
    assert!(platform.positions_at_cost().is_empty());
    assert_eq!(platform.equity(), PlatformConfig::default().initial_equity);
    Ok(())
}

/// An order for an instrument outside the universe is refused and leaves no
/// trace (report finding D2, fixed).
///
/// `Platform::submit_order` used to ask nothing about the instrument: an order
/// for `obj-GHOST` was risk-checked against a book with no figures for it,
/// filled by the simulated venue at the order's own arrival price, and booked
/// as a position. The order manager now holds the universe the platform was
/// assembled from and refuses any other id before a gate, a venue or the book
/// sees it. The refusal is a recorded one, so it is distinguishable from a
/// decision nobody made.
#[test]
fn an_order_outside_the_universe_is_refused_and_leaves_no_trace() -> Result<()> {
    let mut platform = warm_platform()?;
    let before = book(&platform);
    let refusals_before = platform.orders().refusals().len();
    // Premise: a valid instrument is accepted by this same platform, so the
    // refusal below is about the instrument and not about the fixture.
    let valid = order(&mut platform, dec!("100"), vec!["hyp-valid".to_string()]);
    platform.submit_order(valid, later())?;
    let after_valid = book(&platform);
    assert_eq!(after_valid.fills, before.fills + 1, "premise: a fill");

    let ghost = platform.order_from(
        ObjectId::from_string("obj-GHOST"),
        Side::Buy,
        dec!("100"),
        dec!("100"),
        "prop-ghost",
        vec!["hyp-ghost".to_string()],
        later(),
    );
    let ghost_id = ghost.order_id.clone();
    let error = platform
        .submit_order(ghost, later())
        .expect_err("an order for an instrument outside the universe was accepted");
    assert!(
        error.message().contains("not in the universe"),
        "{}",
        error.message()
    );
    assert_eq!(
        book(&platform),
        after_valid,
        "the refused order moved cash, a position, costs or a fill"
    );
    assert!(!platform.positions_at_cost().contains_key("obj-GHOST"));
    assert!(
        platform
            .orders()
            .refusals()
            .iter()
            .skip(refusals_before)
            .any(|refusal| refusal.order_id == ghost_id && !refusal.accepted),
        "the refusal was not recorded"
    );
    assert!(
        platform
            .orders()
            .fills()
            .iter()
            .all(|fill| fill.order_id != ghost_id),
        "a fill exists for the refused order"
    );

    // A replay of the refused id is still a replay, not a second chance.
    let replay = platform.order_from(
        ObjectId::from_string("obj-GHOST"),
        Side::Buy,
        dec!("100"),
        dec!("100"),
        "prop-ghost",
        vec!["hyp-ghost".to_string()],
        later(),
    );
    assert!(platform.submit_order(replay, later()).is_err());
    assert_eq!(book(&platform), after_valid);
    Ok(())
}

// --- the duplicate that matters: the same order twice ------------------------

#[test]
fn submitting_the_same_order_twice_has_one_financial_effect() -> Result<()> {
    let mut platform = warm_platform()?;
    let original = order(&mut platform, dec!("100"), vec!["hyp-once".to_string()]);
    let replay = original.clone();
    let id = original.order_id.clone();
    platform.submit_order(original, later())?;
    let after_first = book(&platform);
    assert_eq!(after_first.fills, 1, "premise: the first submission filled");
    let held_before = platform.orders().order(&id).expect("held").clone();

    let second = platform.submit_order(replay, later());
    let error = second.expect_err("a replayed order id was accepted a second time");
    assert!(
        error.message().contains("already submitted"),
        "{}",
        error.message()
    );
    assert_eq!(
        platform.orders().order(&id),
        Some(&held_before),
        "the replay overwrote the original order's record"
    );
    assert_eq!(
        book(&platform),
        after_first,
        "the replayed order moved cash, position or fills a second time"
    );
    Ok(())
}

// --- an honest seam, asserted so that closing it is noticed -------------------

/// FINDING G1. `Platform::observe` takes `Vec<SensedRecord>` and no lineage, so
/// the correlation id the ingestion service minted for a tick stops at the bus:
/// the platform's cycle mints its own, and nothing it records — event log,
/// journal, outcome chain — carries the tick's. Reconstructing "this fill came
/// from that tick" therefore needs a join on time and instrument, not on an id.
///
/// This test asserts the gap as it is today. When `observe` grows a lineage
/// parameter it fails, and the failure is the signal to turn it into the
/// positive assertion (every outcome row's correlation chain reaches the tick).
#[test]
fn finding_g1_the_ingestion_correlation_id_does_not_reach_the_platform() -> Result<()> {
    let printed = start().saturating_add(Duration::from_secs(30));
    let captured = printed.saturating_add(Duration::from_millis(40));
    let delivered = through_ingestion_and_bus(
        vec![SensedRecord::Tick(tick(dec!("100"), printed, captured))],
        start().saturating_add(Duration::from_secs(31)),
    )?;
    let tick_correlation = {
        let log = delivered.log.borrow();
        let events = log.by_topic(Topic::MarketTick);
        assert_eq!(events.len(), 1, "premise: the tick is on the bus log");
        events[0].lineage.correlation_id.clone()
    };

    let mut platform = platform()?;
    platform.observe(delivered.records);
    let now = start().saturating_add(Duration::from_mins(2));
    let report = platform.run_cycle(now);
    let fine = order(&mut platform, dec!("100"), vec!["hyp-g1".to_string()]);
    platform.submit_order(fine, later())?;
    assert!(
        !platform.orders().fills().is_empty(),
        "premise: a fill exists"
    );

    assert!(
        platform
            .event_log()
            .by_correlation(&tick_correlation)
            .is_empty(),
        "the platform's event log now carries the ingestion correlation id: G1 is closed"
    );
    assert!(
        platform
            .outcomes()
            .entries()
            .iter()
            .all(|row| row.decision.correlation != tick_correlation),
        "an outcome row now carries the ingestion correlation id: G1 is closed"
    );
    assert_ne!(report.correlation_id, tick_correlation);
    Ok(())
}
