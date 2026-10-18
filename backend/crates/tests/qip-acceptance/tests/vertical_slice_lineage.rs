//! The vertical slice, lineage half: the correlation id ingestion mints for a
//! tick reaches the platform's own records, through the production bridge.
//!
//! ```text
//! tick -> IngestionService (gate, mints the correlation id) -> EventBus + EventLog
//!      -> PlatformBridge (bounded queue) -> Platform::observe_with_lineage
//!      -> run_cycle -> order -> fill -> outcome chain
//! ```
//!
//! Every hop is the production type. Before this file the bridge was a
//! test-owned subscriber and `Platform::observe` took no lineage, so the id
//! stopped at the bus (finding G1) and no production code subscribed the
//! platform to a bus (G2). `vertical_slice.rs` keeps its own glue for the
//! cases that predate this; nothing here borrows it.
//!
//! The join under test is by id, not by time and instrument: tick correlation
//! id -> the platform's event-log record -> the journal entry of the cycle that
//! reasoned over it -> that cycle's correlation id -> the outcome rows.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::lineage::CorrelationId;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ManualClock, ObjectId, dec};
use qip_events::{AnyEvent, EventBus, EventLog, Topic};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, LicensingClass, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::bus_bridge::{BridgeState, PlatformBridge, SensePath};
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market::quote::Tick;
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::{IngestionService, ReplayAdapter};
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_risk::limits::{Limit, LimitKind, LimitSet};
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

fn platform() -> Result<Platform> {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                acme(),
                "ACME",
                InstrumentType::CommonStock,
                qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("lineage-slice", start()))
            .build(start())
            .expect("valid instrument"),
        )
        .expect("insertable");
    let limits = LimitSet::new("lineage-slice")
        .with(Limit::new(
            "max-position-weight",
            LimitKind::MaxPositionWeight { limit: 0.10 },
        ))
        .with(Limit::new(
            "max-leverage",
            LimitKind::MaxLeverage { limit: 2.0 },
        ));
    let config = PlatformConfig::default();
    let context = Context::new(Arc::new(ManualClock::new(start())), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits)
}

fn bar(at: Timestamp, close: f64) -> Bar {
    Bar {
        object_id: acme(),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(close).expect("a price"),
        high: Decimal::from_f64(close * 1.002).expect("a price"),
        low: Decimal::from_f64(close * 0.998).expect("a price"),
        close: Decimal::from_f64(close).expect("a price"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64(close),
        quality: DataQuality::default(),
    }
}

/// Distinct daily bars ending the day before `start()`. Distinct because the
/// bus de-duplicates on payload, and identical bars would be suppressed.
fn history(days: i64) -> Vec<SensedRecord> {
    (1..=days)
        .rev()
        .map(|back| {
            let at = start().saturating_sub(Duration::from_days(back));
            SensedRecord::Bar(Box::new(bar(at, 100.0 + back as f64 * 0.01)))
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

/// The production wiring, assembled once: gate -> bus (with a log) -> bridge.
struct Pipe {
    context: Context,
    bus: EventBus,
    log: Rc<RefCell<EventLog>>,
    bridge: PlatformBridge,
    telemetry: Telemetry,
}

fn pipe(capacity: usize) -> Result<Pipe> {
    let (context, clock) = Context::deterministic(start(), 7);
    clock.set(start().saturating_add(Duration::from_secs(31)));
    let log = Rc::new(RefCell::new(EventLog::in_memory()));
    let mut bus = EventBus::new().with_log(Rc::clone(&log));
    let telemetry = Telemetry::silent();
    let bridge = PlatformBridge::attach(&mut bus, capacity, Arc::clone(&telemetry.metrics))?;
    Ok(Pipe {
        context,
        bus,
        log,
        bridge,
        telemetry,
    })
}

impl Pipe {
    /// Publish `records` through a real ingestion service, without draining.
    fn publish(&mut self, records: Vec<SensedRecord>) -> Result<u64> {
        let mut service = IngestionService::new(Telemetry::silent());
        service.register(Box::new(
            ReplayAdapter::from_records("simulated-tape", records)
                .with_licensing(LicensingClass::Synthetic),
        ));
        service.start(start())?;
        let until = start().saturating_add(Duration::from_secs(31));
        Ok(service
            .poll_and_publish_reporting(&self.context, &mut self.bus, until)?
            .published)
    }

    fn counter(&self, outcome: &str) -> u64 {
        self.telemetry
            .metrics
            .snapshot()
            .counter(names::SENSE_BRIDGE_RECORDS, &labels([("outcome", outcome)]))
    }

    fn tick_event(&self) -> AnyEvent {
        self.log
            .borrow()
            .by_topic(Topic::MarketTick)
            .first()
            .map(|event| (*event).clone())
            .expect("premise: the tick is on the bus log")
    }
}

fn received() -> Timestamp {
    start().saturating_add(Duration::from_secs(31))
}

// --- the join ----------------------------------------------------------------

#[test]
fn the_ingestion_correlation_id_is_findable_in_the_platform_log_and_joins_to_the_outcome_row()
-> Result<()> {
    let mut pipe = pipe(1_024)?;
    let mut platform = platform()?;
    let printed = start().saturating_add(Duration::from_secs(30));
    let captured = printed.saturating_add(Duration::from_millis(40));
    let mut tape = history(60);
    tape.push(SensedRecord::Tick(tick(dec!("100.25"), printed, captured)));
    let expected = tape.len();

    assert_eq!(pipe.publish(tape)? as usize, expected);
    let report = pipe
        .bridge
        .flow(&mut pipe.bus, &pipe.context, &mut platform, received())?;
    assert_eq!(report.delivered, expected);
    assert_eq!(report.absorbed, expected);

    // The id ingestion minted, read off the bus log it published to.
    let ingestion_tick = pipe.tick_event();
    let tick_correlation: CorrelationId = ingestion_tick.lineage.correlation_id.clone();
    assert_eq!(ingestion_tick.lineage.producer, "market-ingestion");

    // The platform's own log holds the absorbed tick under that same id,
    // caused by the ingestion event and produced by the bridge.
    let found = platform.event_log().by_correlation(&tick_correlation);
    assert_eq!(found.len(), 1, "the platform logged the tick exactly once");
    assert_eq!(found[0].topic, Topic::MarketTick);
    assert_eq!(found[0].lineage.producer, "platform-bridge");
    assert_eq!(
        found[0].lineage.causation_id.as_ref().map(|c| c.as_str()),
        Some(ingestion_tick.event_id.as_str()),
        "the platform's record names the ingestion event as its cause"
    );
    assert_eq!(found[0].decode::<Tick>()?.body.price, dec!("100.25"));
    let logged_event = found[0].event_id.as_str().to_string();

    // A cycle reasons over it; an order fills against it.
    let now = start().saturating_add(Duration::from_mins(2));
    let cycle = platform.run_cycle(now);
    assert!(cycle.traversed_every_stage(), "{}", cycle.summarise());
    assert_ne!(
        cycle.correlation_id, tick_correlation,
        "premise: the cycle has its own id, so the join below is not an identity"
    );
    let order = platform.order_from(
        acme(),
        Side::Buy,
        dec!("700"),
        dec!("100.25"),
        "prop-lineage",
        vec!["hyp-lineage".to_string()],
        now,
    );
    let order_id = order.order_id.clone();
    platform.submit_order(order, now)?;
    assert_eq!(platform.orders().fills().len(), 1, "premise: a fill exists");

    // The journal entry of that cycle names the tick's correlation id.
    let journal = platform.journal_entries()?;
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].correlation_id, cycle.correlation_id.as_str());
    assert_eq!(journal[0].sensed.len(), expected);
    let reference = journal[0]
        .sensed
        .iter()
        .find(|sensed| sensed.correlation_id == tick_correlation.as_str())
        .expect("the cycle's journal entry does not name the tick's correlation id");
    assert_eq!(reference.topic, "market.tick");
    assert_eq!(reference.event_id, logged_event);
    assert_eq!(journal[0].sensed_unlisted, 0);

    // And the outcome rows carry the journal entry's cycle id: tick -> cycle ->
    // outcome, by id.
    platform.outcomes().verify()?;
    let rows = platform.outcomes().entries();
    assert!(!rows.is_empty(), "premise: the outcome chain has rows");
    assert!(
        platform.outcomes().taken().iter().any(|entry| matches!(
            &entry.decision.action,
            Action::Filled { order_id: id, .. } if *id == order_id
        )),
        "no Filled row for the order"
    );
    let joined_cycle = journal
        .iter()
        .find(|entry| {
            entry
                .sensed
                .iter()
                .any(|s| s.correlation_id == tick_correlation.as_str())
        })
        .map(|entry| entry.correlation_id.clone())
        .expect("a journal entry names the tick");
    for row in rows {
        assert_eq!(
            row.decision.correlation.as_str(),
            joined_cycle,
            "an outcome row is not joined to the tick through its cycle"
        );
    }
    Ok(())
}

#[test]
fn the_fast_brain_path_carries_a_correlation_id_per_record_into_the_log_and_the_journal()
-> Result<()> {
    // `SensePath` is what `qip-fastbrain`'s step holds. The feed hands it
    // records it has already validated; it mints the lineage itself.
    let mut platform = platform()?;
    let mut path = SensePath::new(&platform)?;
    let records = history(5);
    let absorbed = path.feed(&mut platform, "synthetic", records, received())?;
    assert_eq!(absorbed, 5);

    let logged = platform.event_log().by_topic(Topic::MarketBar);
    assert_eq!(logged.len(), 5);
    let correlations: std::collections::BTreeSet<String> = logged
        .iter()
        .map(|event| event.lineage.correlation_id.as_str().to_string())
        .collect();
    assert_eq!(
        correlations.len(),
        5,
        "each record starts its own lineage chain"
    );
    assert_eq!(path.ingestion().stats().accepted, 5);

    platform.run_cycle(start().saturating_add(Duration::from_mins(2)));
    let journal = platform.journal_entries()?;
    let named: std::collections::BTreeSet<String> = journal[0]
        .sensed
        .iter()
        .map(|sensed| sensed.correlation_id.clone())
        .collect();
    assert_eq!(named, correlations, "the entry names exactly what was fed");
    Ok(())
}

// --- idempotency ---------------------------------------------------------------

#[test]
fn a_redelivered_event_is_absorbed_once() -> Result<()> {
    let mut pipe = pipe(1_024)?;
    let mut platform = platform()?;
    let printed = start().saturating_add(Duration::from_secs(30));
    pipe.publish(vec![SensedRecord::Tick(tick(
        dec!("100.5"),
        printed,
        printed.saturating_add(Duration::from_millis(40)),
    ))])?;
    pipe.bus.drain(&pipe.context)?;
    let original = pipe.tick_event();
    // Premise: the first delivery reached the queue, so what follows is a
    // redelivery of something the bridge has really seen.
    assert_eq!(pipe.bridge.queued(), 1);

    // The same event again, as an at-least-once transport would redeliver it.
    pipe.bus.publish_raw(original.clone())?;
    pipe.bus.publish_raw(original)?;
    pipe.bus.drain(&pipe.context)?;
    assert_eq!(pipe.bus.duplicates_suppressed(), 2);
    assert_eq!(pipe.bridge.queued(), 1, "a redelivery reached the queue");

    let report = pipe.bridge.pump(&mut platform, received());
    assert_eq!(report.absorbed, 1);
    assert_eq!(platform.event_log().by_topic(Topic::MarketTick).len(), 1);
    assert_eq!(pipe.bridge.counts().queued, 1);
    assert_eq!(pipe.counter("queued"), 1);
    Ok(())
}

// --- backpressure ---------------------------------------------------------------

#[test]
fn a_full_queue_refuses_the_newest_keeps_the_oldest_and_counts_each_refusal() -> Result<()> {
    let mut pipe = pipe(3)?;
    let mut platform = platform()?;
    let tape = history(5);
    // The five bars, oldest first, are the instants the queue must keep three of.
    let instants: Vec<Timestamp> = tape.iter().map(SensedRecord::occurred_at).collect();
    assert!(instants.windows(2).all(|pair| pair[0] < pair[1]), "premise");

    assert_eq!(pipe.publish(tape)?, 5);
    pipe.bus.drain(&pipe.context)?;

    assert_eq!(pipe.bridge.queued(), 3, "the queue is at its bound");
    let counts = pipe.bridge.counts();
    assert_eq!(counts.queued, 3);
    assert_eq!(counts.refused_full, 2);
    assert_eq!(pipe.counter("refused_full"), 2, "the metric agrees");
    assert_eq!(
        pipe.bus.failure_count(),
        2,
        "each refusal is also a failure the bus recorded, where an operator reads it"
    );

    let report = pipe.bridge.pump(&mut platform, received());
    assert_eq!(report.absorbed, 3);
    let kept: Vec<Timestamp> = platform
        .event_log()
        .by_topic(Topic::MarketBar)
        .iter()
        .map(|event| event.occurred_at)
        .collect();
    assert_eq!(
        kept,
        instants[..3].to_vec(),
        "the oldest three were kept; dropping the oldest would leave a gap before the newest"
    );

    // Pumping freed the queue: the bridge admits again.
    pipe.publish(vec![SensedRecord::Bar(Box::new(bar(
        start().saturating_sub(Duration::from_days(30)),
        101.0,
    )))])?;
    pipe.bus.drain(&pipe.context)?;
    assert_eq!(pipe.bridge.queued(), 1);
    Ok(())
}

#[test]
fn a_bridge_with_no_capacity_is_refused_rather_than_read_as_one() -> Result<()> {
    let mut bus = EventBus::new();
    let error = PlatformBridge::attach(&mut bus, 0, Arc::clone(&Telemetry::silent().metrics))
        .expect_err("a zero-capacity bridge was accepted");
    assert!(error.message().contains("zero capacity"));
    assert_eq!(bus.subscriber_count(), 0, "a refused attach subscribed");
    Ok(())
}

// --- refusal ----------------------------------------------------------------------

#[test]
fn malformed_and_ungated_input_is_refused_counted_and_never_reaches_the_platform() -> Result<()> {
    let mut pipe = pipe(1_024)?;
    let mut platform = platform()?;
    let printed = start().saturating_add(Duration::from_secs(30));
    let good = SensedRecord::Tick(tick(
        dec!("100"),
        printed,
        printed.saturating_add(Duration::from_millis(40)),
    ));

    // A payload that is not a tick, on the tick topic.
    pipe.publish(vec![good])?;
    pipe.bus.drain(&pipe.context)?;
    let mut hollow = pipe.tick_event();
    hollow.payload = serde_json::json!({"not": "a tick"});
    hollow.payload_hash = "distinct-from-the-real-tick".to_string();
    // The tick's natural key would make the bus read this as a redelivery of
    // the good one, which is the other test's subject.
    hollow.idempotency_key = None;
    // A new event: the bus log refuses a second record under an existing id.
    hollow.event_id = pipe.context.ids().generate(printed);
    pipe.bus.publish_raw(hollow)?;

    // A tick that decodes but would have failed the ingestion gate: a publisher
    // that went around it.
    pipe.bus.publish(
        &pipe.context,
        qip_core::Lineage::root(pipe.context.ids().generate(printed), "rogue-publisher"),
        printed,
        tick(
            Decimal::ZERO,
            printed.saturating_add(Duration::from_secs(1)),
            printed.saturating_add(Duration::from_secs(2)),
        ),
    )?;
    pipe.bus.drain(&pipe.context)?;

    let counts = pipe.bridge.counts();
    assert_eq!(counts.queued, 1, "premise: the good tick was admitted");
    assert_eq!(counts.malformed, 1);
    assert_eq!(counts.invalid, 1);
    assert_eq!(pipe.counter("malformed"), 1);
    assert_eq!(pipe.counter("invalid"), 1);
    assert_eq!(counts.refused(), 2);
    assert_eq!(
        pipe.bus.failure_count(),
        2,
        "both are visible on the bus too"
    );

    pipe.bridge.pump(&mut platform, received());
    assert_eq!(
        platform.event_log().by_topic(Topic::MarketTick).len(),
        1,
        "only the good tick reached the platform's log"
    );
    Ok(())
}

#[test]
fn a_licence_refused_source_is_reported_counted_and_publishes_nothing() -> Result<()> {
    let mut pipe = pipe(1_024)?;
    let telemetry = Telemetry::silent();
    let mut service = IngestionService::new(telemetry.clone()).enforcing_production_licensing(true);
    service.register(Box::new(
        ReplayAdapter::from_records("simulated-tape", history(3))
            .with_licensing(LicensingClass::Synthetic),
    ));
    let until = start().saturating_add(Duration::from_secs(31));

    let report = service.poll_and_publish_reporting(&pipe.context, &mut pipe.bus, until)?;
    assert_eq!(report.published, 0);
    assert_eq!(report.refused_sources, vec!["simulated-tape".to_string()]);
    assert_eq!(pipe.bus.queued(), 0);
    assert_eq!(
        service.stats().refusals_by_source.get("simulated-tape"),
        Some(&1)
    );
    assert_eq!(
        telemetry.metrics.snapshot().counter(
            names::SOURCES_REFUSED,
            &labels([("source", "simulated-tape")])
        ),
        1
    );
    assert_eq!(pipe.bridge.queued(), 0);
    Ok(())
}

// --- shutdown ---------------------------------------------------------------------

#[test]
fn shutdown_drains_what_the_bus_holds_and_then_refuses_and_counts_what_follows() -> Result<()> {
    let mut pipe = pipe(1_024)?;
    let mut platform = platform()?;

    // Published and never drained: the bus holds them, the bridge has seen none.
    pipe.publish(history(2))?;
    assert_eq!(pipe.bus.queued(), 2, "premise: undelivered");
    assert_eq!(pipe.bridge.queued(), 0, "premise: not yet admitted");
    assert_eq!(pipe.bridge.state(), BridgeState::Running);

    let report = pipe
        .bridge
        .shutdown(&mut pipe.bus, &pipe.context, &mut platform, received())?;
    assert_eq!(report.delivered, 2, "shutdown delivered what was in flight");
    assert_eq!(report.absorbed, 2);
    assert_eq!(platform.event_log().by_topic(Topic::MarketBar).len(), 2);
    assert_eq!(pipe.bridge.state(), BridgeState::Stopped);
    assert_eq!(pipe.bridge.queued(), 0);

    // After shutdown an arriving event is refused and counted, not lost.
    pipe.publish(vec![SensedRecord::Bar(Box::new(bar(
        start().saturating_sub(Duration::from_days(30)),
        102.0,
    )))])?;
    pipe.bus.drain(&pipe.context)?;
    assert_eq!(pipe.bridge.counts().refused_stopped, 1);
    assert_eq!(pipe.counter("refused_stopped"), 1);
    assert_eq!(pipe.bridge.queued(), 0);
    let again = pipe.bridge.pump(&mut platform, received());
    assert_eq!(again.delivered, 0);
    assert_eq!(platform.event_log().by_topic(Topic::MarketBar).len(), 2);
    Ok(())
}

// --- a root with no bus ---------------------------------------------------------

#[test]
fn a_root_with_no_bus_mints_a_root_chain_per_record_and_the_journal_names_each() -> Result<()> {
    let mut platform = platform()?;
    let absorbed = platform.observe_fresh(history(4), "qip-api/sense", received());
    assert_eq!(absorbed, 4);

    let logged = platform.event_log().by_topic(Topic::MarketBar);
    assert_eq!(logged.len(), 4);
    let ids: std::collections::BTreeSet<String> = logged
        .iter()
        .map(|event| event.lineage.correlation_id.as_str().to_string())
        .collect();
    assert_eq!(ids.len(), 4, "one chain per record, not one for the batch");
    assert!(logged.iter().all(|event| {
        event.lineage.producer == "qip-api/sense" && event.lineage.causation_id.is_none()
    }));

    platform.run_cycle(start().saturating_add(Duration::from_mins(2)));
    let journal = platform.journal_entries()?;
    let named: std::collections::BTreeSet<String> = journal[0]
        .sensed
        .iter()
        .map(|sensed| sensed.correlation_id.clone())
        .collect();
    assert_eq!(named, ids);

    // A second cycle names only what it itself was fed: an empty list, not the
    // first cycle's.
    platform.run_cycle(start().saturating_add(Duration::from_mins(3)));
    let journal = platform.journal_entries()?;
    assert_eq!(journal.len(), 2);
    assert!(
        journal[1].sensed.is_empty(),
        "a cycle that sensed nothing new credited itself with the last cycle's ticks"
    );
    Ok(())
}

#[test]
fn the_journal_names_a_bounded_number_of_records_and_counts_the_rest() -> Result<()> {
    use qip_kernel::platform::MAX_SENSED_REFS_PER_CYCLE;
    let over = 4;
    let total = MAX_SENSED_REFS_PER_CYCLE + over;
    let mut platform = platform()?;
    let records: Vec<SensedRecord> = (0..total)
        .map(|index| {
            let at = start().saturating_sub(Duration::from_mins(index as i64 + 1));
            SensedRecord::Bar(Box::new(bar(at, 100.0 + index as f64 * 0.0001)))
        })
        .collect();
    platform.observe_fresh(records, "qip-api/sense", received());
    assert_eq!(
        platform.event_log().by_topic(Topic::MarketBar).len(),
        total,
        "every record is logged under its own id whatever the journal lists"
    );

    platform.run_cycle(start().saturating_add(Duration::from_mins(2)));
    let journal = platform.journal_entries()?;
    assert_eq!(journal[0].sensed.len(), MAX_SENSED_REFS_PER_CYCLE);
    assert_eq!(
        journal[0].sensed_unlisted, over as u64,
        "the join is incomplete and the entry says by how much"
    );
    Ok(())
}
