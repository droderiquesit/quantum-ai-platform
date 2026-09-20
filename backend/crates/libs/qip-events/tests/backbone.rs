//! Topics, envelopes, the deterministic bus and the hash-chained log.

use qip_core::error::Result;
use qip_core::{Context, CorrelationId, Duration, Lineage, Timestamp};
use qip_events::bus::{DispatchFailure, HandlerOutcome, Publisher};
use qip_events::envelope::canonical_json;
use qip_events::topic::TopicGroup;
use qip_events::{
    AnyEvent, Envelope, EventBody, EventBus, EventFilter, EventLog, SchemaRegistry, Topic,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;

// --- test event bodies ------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Tick {
    symbol: String,
    price: f64,
}

impl EventBody for Tick {
    const TOPIC: Topic = Topic::MarketTick;
    const SCHEMA_VERSION: u32 = 1;
    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.symbol, self.price))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Anomaly {
    symbol: String,
    z_score: f64,
}

impl EventBody for Anomaly {
    const TOPIC: Topic = Topic::AnomalyDetected;
    const SCHEMA_VERSION: u32 = 1;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Opportunity {
    symbol: String,
}

impl EventBody for Opportunity {
    const TOPIC: Topic = Topic::OpportunityDetected;
    const SCHEMA_VERSION: u32 = 1;
}

fn context() -> (Context, Timestamp) {
    let now = Timestamp::from_civil(2026, 8, 22);
    let (ctx, _clock) = Context::deterministic(now, 7);
    (ctx, now)
}

fn root_lineage(producer: &str) -> Lineage {
    Lineage::root(
        CorrelationId::from_string("COR00000000000000000000001"),
        producer,
    )
}

// --- topics -----------------------------------------------------------------

#[test]
fn every_topic_has_a_unique_stable_name() {
    let mut names: Vec<&str> = Topic::ALL.iter().map(|t| t.name()).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "duplicate topic names");
}

#[test]
fn topic_names_round_trip() {
    for topic in Topic::ALL {
        assert_eq!(Topic::from_name(topic.name()), Some(topic), "{topic}");
    }
    assert_eq!(Topic::from_name("not.a.topic"), None);
}

#[test]
fn every_topic_belongs_to_a_group_and_the_groups_are_sane() {
    assert_eq!(Topic::MarketTick.group(), TopicGroup::Sense);
    assert_eq!(Topic::OrderFilled.group(), TopicGroup::Act);
    assert_eq!(Topic::HypothesisCreated.group(), TopicGroup::Reason);
    assert!(TopicGroup::Sense.is_latency_critical());
    assert!(!TopicGroup::Simulate.is_latency_critical());
}

#[test]
fn decision_relevant_topics_are_retained_permanently_and_never_dropped() {
    for topic in [
        Topic::HypothesisApproved,
        Topic::RiskApproved,
        Topic::OrderFilled,
        Topic::AttributionCompleted,
        Topic::KillSwitchEngaged,
    ] {
        assert!(
            topic.requires_permanent_retention(),
            "{topic} must be retained"
        );
        assert!(!topic.is_lossy_tolerable(), "{topic} must never be evicted");
    }
    // High-volume market data is the only thing allowed to be dropped.
    assert!(Topic::MarketTick.is_lossy_tolerable());
    assert!(!Topic::MarketTick.requires_permanent_retention());
}

// --- envelopes --------------------------------------------------------------

#[test]
fn envelopes_round_trip_through_type_erasure() {
    let (ctx, now) = context();
    let envelope = Envelope::new(
        ctx.ids().generate(now),
        now,
        now,
        root_lineage("test"),
        Tick {
            symbol: "AAPL".into(),
            price: 195.5,
        },
    );
    let erased = envelope.erase().unwrap();
    assert_eq!(erased.topic, Topic::MarketTick);

    let decoded = erased.decode::<Tick>().unwrap();
    assert_eq!(decoded.body, envelope.body);
    assert_eq!(decoded.event_id, envelope.event_id);
}

#[test]
fn decoding_the_wrong_type_is_rejected() {
    let (ctx, now) = context();
    let erased = Envelope::new(
        ctx.ids().generate(now),
        now,
        now,
        root_lineage("test"),
        Tick {
            symbol: "AAPL".into(),
            price: 1.0,
        },
    )
    .erase()
    .unwrap();
    let err = erased.decode::<Anomaly>().unwrap_err();
    assert!(err.to_string().contains("cannot decode"), "{err}");
}

#[test]
fn a_payload_from_a_newer_schema_is_refused_rather_than_partially_read() {
    let (ctx, now) = context();
    let mut erased = Envelope::new(
        ctx.ids().generate(now),
        now,
        now,
        root_lineage("test"),
        Tick {
            symbol: "AAPL".into(),
            price: 1.0,
        },
    )
    .erase()
    .unwrap();
    erased.schema_version = 99;
    let err = erased.decode::<Tick>().unwrap_err();
    assert!(
        err.to_string().contains("newer than the supported"),
        "{err}"
    );
}

#[test]
fn canonical_json_is_key_order_independent() {
    let a: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":1,"c":{"z":1,"y":2}}"#).unwrap();
    let b: serde_json::Value = serde_json::from_str(r#"{"c":{"y":2,"z":1},"a":1,"b":2}"#).unwrap();
    assert_eq!(canonical_json(&a), canonical_json(&b));
    // And it is still order-sensitive for arrays, where order is meaningful.
    let x: serde_json::Value = serde_json::from_str("[1,2]").unwrap();
    let y: serde_json::Value = serde_json::from_str("[2,1]").unwrap();
    assert_ne!(canonical_json(&x), canonical_json(&y));
}

#[test]
fn identical_payloads_hash_identically() {
    let (ctx, now) = context();
    let make = || {
        Envelope::new(
            ctx.ids().generate(now),
            now,
            now,
            root_lineage("test"),
            Tick {
                symbol: "AAPL".into(),
                price: 195.5,
            },
        )
        .erase()
        .unwrap()
    };
    let a = make();
    let b = make();
    assert_eq!(a.payload_hash, b.payload_hash);
    assert_ne!(a.event_id, b.event_id, "ids must still differ");
}

// --- the bus ----------------------------------------------------------------

#[test]
fn dispatch_order_is_deterministic_and_breadth_first() {
    // A handler that publishes in response must not preempt events already
    // queued: that is what makes replay reproduce the original order.
    let (ctx, now) = context();
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));

    let mut bus = EventBus::new();
    let record = seen.clone();
    bus.on::<Tick, _>("expander", move |tick, any, publisher| {
        record.borrow().len();
        publisher.publish(
            any,
            "expander",
            any.occurred_at,
            Anomaly {
                symbol: tick.body.symbol.clone(),
                z_score: 4.0,
            },
        )?;
        Ok(HandlerOutcome::Handled)
    });
    let record = seen.clone();
    bus.on_all("observer", move |any, _| {
        record.borrow_mut().push(any.topic.name().to_string());
        Ok(HandlerOutcome::Handled)
    });

    for symbol in ["A", "B", "C"] {
        bus.publish(
            &ctx,
            root_lineage("feed"),
            now,
            Tick {
                symbol: symbol.into(),
                price: 1.0,
            },
        )
        .unwrap();
    }
    let dispatched = bus.drain(&ctx).unwrap();

    assert_eq!(dispatched, 6, "three ticks and three derived anomalies");
    // All three ticks are delivered before any derived anomaly.
    assert_eq!(
        *seen.borrow(),
        vec![
            "market.tick",
            "market.tick",
            "market.tick",
            "anomaly.detected",
            "anomaly.detected",
            "anomaly.detected"
        ]
    );
}

#[test]
fn the_same_inputs_always_produce_the_same_dispatch_sequence() {
    let run = || {
        let (ctx, now) = context();
        let seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let mut bus = EventBus::new();
        let record = seen.clone();
        bus.on_all("observer", move |any, _| {
            record.borrow_mut().push(any.summary());
            Ok(HandlerOutcome::Handled)
        });
        for i in 0..20 {
            bus.publish(
                &ctx,
                root_lineage("feed"),
                now.saturating_add(Duration::from_secs(i)),
                Tick {
                    symbol: format!("S{i}"),
                    price: i as f64,
                },
            )
            .unwrap();
        }
        bus.drain(&ctx).unwrap();
        seen.borrow().clone()
    };
    assert_eq!(run(), run(), "dispatch must be reproducible");
}

#[test]
fn duplicate_events_are_suppressed_by_idempotency_key() {
    let (ctx, now) = context();
    let count = Rc::new(RefCell::new(0usize));
    let mut bus = EventBus::new();
    let counter = count.clone();
    bus.on::<Tick, _>("counter", move |_, _, _| {
        *counter.borrow_mut() += 1;
        Ok(HandlerOutcome::Handled)
    });

    // Same symbol and price three times: one logical event.
    for _ in 0..3 {
        bus.publish(
            &ctx,
            root_lineage("feed"),
            now,
            Tick {
                symbol: "AAPL".into(),
                price: 195.5,
            },
        )
        .unwrap();
    }
    bus.drain(&ctx).unwrap();

    assert_eq!(*count.borrow(), 1, "duplicates must be suppressed");
    assert_eq!(bus.duplicates_suppressed(), 2);
}

#[test]
fn a_failing_handler_does_not_stop_delivery_to_others() {
    let (ctx, now) = context();
    let delivered = Rc::new(RefCell::new(0usize));
    let mut bus = EventBus::new();
    bus.on::<Tick, _>("broken", |_, _, _| {
        Err(qip_core::Error::io("downstream unavailable"))
    });
    let counter = delivered.clone();
    bus.on::<Tick, _>("risk", move |_, _, _| {
        *counter.borrow_mut() += 1;
        Ok(HandlerOutcome::Handled)
    });

    bus.publish(
        &ctx,
        root_lineage("feed"),
        now,
        Tick {
            symbol: "A".into(),
            price: 1.0,
        },
    )
    .unwrap();
    bus.drain(&ctx).unwrap();

    assert_eq!(
        *delivered.borrow(),
        1,
        "the healthy handler must still receive it"
    );
    assert_eq!(bus.failures().len(), 1);
    let recorded: Vec<&DispatchFailure> = bus.failures().collect();
    assert_eq!(recorded[0].handler, "broken");
}

#[test]
fn a_handler_that_publishes_in_a_loop_is_stopped() {
    let (ctx, now) = context();
    let mut bus = EventBus::new().max_events_per_drain(500);
    bus.on::<Tick, _>("loop", |tick, any, publisher| {
        publisher.publish(any, "loop", any.occurred_at, tick.body.clone())?;
        Ok(HandlerOutcome::Handled)
    });
    bus.publish(
        &ctx,
        root_lineage("feed"),
        now,
        Tick {
            symbol: "A".into(),
            price: 1.0,
        },
    )
    .unwrap();

    // The idempotency key stops the exact repeat, so the loop is broken either
    // by deduplication or by the ceiling; neither may hang.
    let result = bus.drain(&ctx);
    assert!(
        result.is_ok()
            || result
                .unwrap_err()
                .to_string()
                .contains("publishing in a loop")
    );
}

#[test]
fn an_unbounded_publishing_loop_trips_the_guard() {
    let (ctx, now) = context();
    let mut bus = EventBus::new().max_events_per_drain(200);
    // Distinct payload each time, so deduplication cannot break the cycle.
    bus.on::<Anomaly, _>("amplifier", |anomaly, any, publisher| {
        publisher.publish(
            any,
            "amplifier",
            any.occurred_at,
            Anomaly {
                symbol: anomaly.body.symbol.clone(),
                z_score: anomaly.body.z_score + 1.0,
            },
        )?;
        Ok(HandlerOutcome::Handled)
    });
    bus.publish(
        &ctx,
        root_lineage("feed"),
        now,
        Anomaly {
            symbol: "A".into(),
            z_score: 1.0,
        },
    )
    .unwrap();

    let err = bus.drain(&ctx).unwrap_err();
    assert!(err.to_string().contains("publishing in a loop"), "{err}");
}

#[test]
fn unsubscribing_stops_delivery() {
    let (ctx, now) = context();
    let count = Rc::new(RefCell::new(0usize));
    let mut bus = EventBus::new();
    let counter = count.clone();
    let subscription = bus.on::<Tick, _>("temp", move |_, _, _| {
        *counter.borrow_mut() += 1;
        Ok(HandlerOutcome::Handled)
    });
    assert_eq!(bus.subscriber_count(), 1);

    bus.unsubscribe(&subscription);
    assert_eq!(bus.subscriber_count(), 0);

    bus.publish(
        &ctx,
        root_lineage("feed"),
        now,
        Tick {
            symbol: "A".into(),
            price: 1.0,
        },
    )
    .unwrap();
    bus.drain(&ctx).unwrap();
    assert_eq!(*count.borrow(), 0);
}

// --- the log ----------------------------------------------------------------

fn log_with_chain(ctx: &Context, now: Timestamp) -> (Rc<RefCell<EventLog>>, CorrelationId) {
    let log = Rc::new(RefCell::new(EventLog::in_memory()));
    let correlation = CorrelationId::from_string("COR00000000000000000000001");
    let mut bus = EventBus::new().with_log(log.clone());

    // A three-stage chain: tick -> anomaly -> opportunity.
    bus.on::<Tick, _>("detector", |tick, any, publisher| {
        publisher.publish(
            any,
            "opportunity-engine",
            any.occurred_at,
            Anomaly {
                symbol: tick.body.symbol.clone(),
                z_score: 4.2,
            },
        )?;
        Ok(HandlerOutcome::Handled)
    });
    bus.on::<Anomaly, _>("ranker", |anomaly, any, publisher| {
        publisher.publish(
            any,
            "opportunity-engine",
            any.occurred_at,
            Opportunity {
                symbol: anomaly.body.symbol.clone(),
            },
        )?;
        Ok(HandlerOutcome::Handled)
    });

    bus.publish(
        ctx,
        Lineage::root(correlation.clone(), "market-ingestion"),
        now,
        Tick {
            symbol: "AAPL".into(),
            price: 195.5,
        },
    )
    .unwrap();
    bus.drain(ctx).unwrap();
    (log, correlation)
}

#[test]
fn the_log_assigns_monotonic_sequence_numbers() {
    let (ctx, now) = context();
    let (log, _) = log_with_chain(&ctx, now);
    let log = log.borrow();
    assert_eq!(log.len(), 3);
    let sequences: Vec<u64> = log.records().iter().map(|r| r.sequence).collect();
    assert_eq!(sequences, vec![1, 2, 3]);
}

#[test]
fn the_hash_chain_verifies_and_links_each_record_to_its_predecessor() {
    let (ctx, now) = context();
    let (log, _) = log_with_chain(&ctx, now);
    let log = log.borrow();
    assert!(log.verify_chain().is_ok());

    let records = log.records();
    assert_eq!(records[0].previous_hash, qip_events::log::GENESIS_HASH);
    for pair in records.windows(2) {
        assert_eq!(
            pair[1].previous_hash, pair[0].record_hash,
            "record {} must commit to record {}",
            pair[1].sequence, pair[0].sequence
        );
    }
}

#[test]
fn editing_stored_history_is_detected_by_the_chain() {
    // The realistic attack is editing the file on disk, not the in-memory log.
    let dir = std::env::temp_dir().join(format!("qip-tamper-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("events.jsonl");

    let (ctx, now) = context();
    {
        let mut log = EventLog::open(&path).unwrap();
        for i in 0..4 {
            let event = Envelope::new(
                ctx.ids().generate(now),
                now.saturating_add(Duration::from_secs(i)),
                now,
                root_lineage("feed"),
                Tick {
                    symbol: format!("S{i}"),
                    price: i as f64,
                },
            )
            .erase()
            .unwrap();
            log.append(&event).unwrap();
        }
        assert!(log.verify_chain().is_ok());
    }

    // Rewrite the third record's payload, leaving its stored hashes alone.
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let mut record: serde_json::Value = serde_json::from_str(&lines[2]).unwrap();
    record["event"]["payload"]["price"] = serde_json::json!(999_999.0);
    lines[2] = serde_json::to_string(&record).unwrap();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    let reopened = EventLog::open(&path).unwrap();
    let broken_at = reopened
        .verify_chain()
        .expect_err("tampering must be detected");
    assert_eq!(broken_at, 3, "the edited record is the one reported");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_decision_can_be_reconstructed_from_its_correlation_id() {
    let (ctx, now) = context();
    let (log, correlation) = log_with_chain(&ctx, now);
    let log = log.borrow();

    let chain = log.by_correlation(&correlation);
    assert_eq!(chain.len(), 3, "the whole chain shares one correlation id");
    let topics: Vec<&str> = chain.iter().map(|e| e.topic.name()).collect();
    assert_eq!(
        topics,
        vec!["market.tick", "anomaly.detected", "opportunity.detected"]
    );

    // And the causation edges reconstruct the exact tree.
    let root = chain[0];
    assert!(
        root.lineage.causation_id.is_none(),
        "the observation is the root"
    );
    let children = log.children_of(&root.event_id);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].topic, Topic::AnomalyDetected);
    let grandchildren = log.children_of(&children[0].event_id);
    assert_eq!(grandchildren[0].topic, Topic::OpportunityDetected);
}

#[test]
fn producers_are_recorded_on_every_event() {
    let (ctx, now) = context();
    let (log, _) = log_with_chain(&ctx, now);
    let log = log.borrow();
    let producers: Vec<&str> = log.events().map(|e| e.lineage.producer.as_str()).collect();
    assert_eq!(
        producers,
        vec![
            "market-ingestion",
            "opportunity-engine",
            "opportunity-engine"
        ]
    );
}

#[test]
fn filters_select_by_topic_group_producer_and_time() {
    let (ctx, now) = context();
    let (log, correlation) = log_with_chain(&ctx, now);
    let log = log.borrow();

    assert_eq!(
        log.query(&EventFilter::new().topic(Topic::MarketTick))
            .len(),
        1
    );
    assert_eq!(
        log.query(&EventFilter::new().group(TopicGroup::Discover))
            .len(),
        2
    );
    assert_eq!(
        log.query(&EventFilter::new().producer("opportunity-engine"))
            .len(),
        2
    );
    assert_eq!(
        log.query(&EventFilter::new().correlation(correlation))
            .len(),
        3
    );
    assert_eq!(
        log.query(&EventFilter::new().as_of(now)).len(),
        0,
        "as_of is exclusive"
    );
    assert_eq!(
        log.query(&EventFilter::new().as_of(now.saturating_add(Duration::from_secs(1))))
            .len(),
        3
    );
}

#[test]
fn replay_visits_every_event_in_order() {
    let (ctx, now) = context();
    let (log, _) = log_with_chain(&ctx, now);
    let log = log.borrow();

    let mut order = Vec::new();
    let count = log
        .replay(|event| {
            order.push(event.sequence);
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 3);
    assert_eq!(order, vec![1, 2, 3]);
}

#[test]
fn a_log_fed_back_into_the_bus_that_produced_it_dispatches_nothing_twice() {
    // The bus once carried `reset_deduplication` "for replay to call". Nothing
    // called it, and the design is the opposite of what it offered: a replay
    // runs on a fresh bus (the test below), while the bus that produced the
    // log has to treat its own events, fed back, as the duplicates they are.
    // Forgetting the window would run every handler side effect a second
    // time and append every event to the log again.
    let (ctx, now) = context();
    let log = Rc::new(RefCell::new(EventLog::in_memory()));
    let mut bus = EventBus::new().with_log(log.clone());
    let delivered = Rc::new(RefCell::new(0usize));
    let counter = delivered.clone();
    bus.on_all("observer", move |_, _| {
        *counter.borrow_mut() += 1;
        Ok(HandlerOutcome::Handled)
    });
    for price in [100.0, 101.0, 102.0] {
        bus.publish(
            &ctx,
            root_lineage("sensor"),
            now,
            Tick {
                symbol: "ACME".into(),
                price,
            },
        )
        .unwrap();
    }
    bus.drain(&ctx).unwrap();

    // Premise: the run produced events, the log holds them and the observer
    // saw each exactly once.
    let recorded: Vec<AnyEvent> = log.borrow().events().cloned().collect();
    assert_eq!(
        recorded.len(),
        3,
        "the run must have produced events to feed back"
    );
    assert_eq!(*delivered.borrow(), 3);
    let suppressed_before = bus.duplicates_suppressed();

    for event in &recorded {
        bus.publish_raw(event.clone()).unwrap();
    }
    bus.drain(&ctx).unwrap();

    assert_eq!(
        *delivered.borrow(),
        3,
        "feeding the log back into its own bus re-ran the handlers"
    );
    assert_eq!(
        bus.duplicates_suppressed(),
        suppressed_before + 3,
        "each replayed event must be counted as the duplicate it is"
    );
    assert_eq!(
        log.borrow().events().count(),
        3,
        "a replayed event must not be appended to the log a second time"
    );
}

#[test]
fn a_replayed_log_reproduces_the_original_run() {
    let (ctx, now) = context();
    let (original, _) = log_with_chain(&ctx, now);

    // Feed the recorded events back through a fresh bus with the same handlers
    // and confirm the observer sees an identical sequence.
    let observed_live: Vec<String> = original
        .borrow()
        .events()
        .map(|e| e.topic.name().to_string())
        .collect();

    let mut replay_bus = EventBus::new();
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let record = seen.clone();
    replay_bus.on_all("observer", move |any, _| {
        record.borrow_mut().push(any.topic.name().to_string());
        Ok(HandlerOutcome::Handled)
    });
    for event in original.borrow().events() {
        replay_bus.publish_raw(event.clone()).unwrap();
    }
    replay_bus.drain(&ctx).unwrap();

    assert_eq!(*seen.borrow(), observed_live);
}

#[test]
fn a_file_backed_log_survives_a_restart() {
    let dir = std::env::temp_dir().join(format!("qip-log-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("events.jsonl");

    let (ctx, now) = context();
    {
        let mut log = EventLog::open(&path).unwrap();
        let event = Envelope::new(
            ctx.ids().generate(now),
            now,
            now,
            root_lineage("feed"),
            Tick {
                symbol: "AAPL".into(),
                price: 195.5,
            },
        )
        .erase()
        .unwrap();
        log.append(&event).unwrap();
        assert_eq!(log.len(), 1);
    }

    let reopened = EventLog::open(&path).unwrap();
    assert_eq!(reopened.len(), 1, "records must be reloaded from disk");
    assert!(
        reopened.verify_chain().is_ok(),
        "the chain must survive a round trip"
    );
    assert_eq!(reopened.records()[0].event.topic, Topic::MarketTick);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capacity_eviction_never_drops_an_auditable_event() {
    let (ctx, now) = context();
    let mut log = EventLog::in_memory().with_capacity(5).unwrap();

    // Interleave high-volume ticks with an order-relevant event.
    for i in 0..20 {
        let event = Envelope::new(
            ctx.ids().generate(now),
            now.saturating_add(Duration::from_secs(i)),
            now,
            root_lineage("feed"),
            Tick {
                symbol: format!("S{i}"),
                price: i as f64,
            },
        )
        .erase()
        .unwrap();
        log.append(&event).unwrap();
    }
    let opportunity = Envelope::new(
        ctx.ids().generate(now),
        now,
        now,
        root_lineage("engine"),
        Opportunity {
            symbol: "KEEP".into(),
        },
    )
    .erase()
    .unwrap();
    log.append(&opportunity).unwrap();
    for i in 20..40 {
        let event = Envelope::new(
            ctx.ids().generate(now),
            now.saturating_add(Duration::from_secs(i)),
            now,
            root_lineage("feed"),
            Tick {
                symbol: format!("S{i}"),
                price: i as f64,
            },
        )
        .erase()
        .unwrap();
        log.append(&event).unwrap();
    }

    assert!(
        log.by_topic(Topic::OpportunityDetected).len() == 1,
        "an auditable event must never be evicted"
    );
    assert!(
        log.len() <= 6,
        "ticks should have been evicted, got {}",
        log.len()
    );
}

#[test]
fn log_stats_summarise_the_history() {
    let (ctx, now) = context();
    let (log, _) = log_with_chain(&ctx, now);
    let stats = log.borrow().stats();
    assert_eq!(stats.total, 3);
    assert_eq!(stats.correlations, 1);
    assert_eq!(stats.by_group.get(&TopicGroup::Discover), Some(&2));
    assert_eq!(stats.first_event, Some(now));
}

// --- schema registry --------------------------------------------------------

#[test]
fn the_registry_records_schema_shape_and_detects_drift() {
    let mut registry = SchemaRegistry::new();
    registry
        .register(&Tick {
            symbol: "A".into(),
            price: 1.0,
        })
        .unwrap();
    registry
        .register(&Anomaly {
            symbol: "A".into(),
            z_score: 1.0,
        })
        .unwrap();

    let tick = registry.get(Topic::MarketTick).unwrap();
    assert_eq!(tick.version, 1);
    assert_eq!(tick.fields, vec!["price".to_string(), "symbol".to_string()]);

    let before = registry.fingerprint();

    // A payload that gains a field without a version bump changes the
    // fingerprint, which is exactly what the contract test compares.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct TickV2 {
        symbol: String,
        price: f64,
        venue: String,
    }
    impl EventBody for TickV2 {
        const TOPIC: Topic = Topic::MarketTick;
        const SCHEMA_VERSION: u32 = 1;
    }
    let mut drifted = SchemaRegistry::new();
    drifted
        .register(&TickV2 {
            symbol: "A".into(),
            price: 1.0,
            venue: "X".into(),
        })
        .unwrap();
    drifted
        .register(&Anomaly {
            symbol: "A".into(),
            z_score: 1.0,
        })
        .unwrap();
    assert_ne!(before, drifted.fingerprint(), "shape drift must be visible");
}

#[test]
fn two_body_types_cannot_claim_the_same_topic() {
    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct Impostor {
        x: u8,
    }
    impl EventBody for Impostor {
        const TOPIC: Topic = Topic::MarketTick;
        const SCHEMA_VERSION: u32 = 1;
    }

    let mut registry = SchemaRegistry::new();
    registry
        .register(&Tick {
            symbol: "A".into(),
            price: 1.0,
        })
        .unwrap();
    let err = registry.register(&Impostor { x: 1 }).unwrap_err();
    assert!(err.to_string().contains("already claimed"), "{err}");
}

#[test]
fn unregistered_topics_are_reported() {
    let mut registry = SchemaRegistry::new();
    registry
        .register(&Tick {
            symbol: "A".into(),
            price: 1.0,
        })
        .unwrap();
    let missing = registry.unregistered_topics();
    assert!(!missing.contains(&Topic::MarketTick));
    assert!(missing.contains(&Topic::OrderFilled));
}

// --- helper used by other crates -------------------------------------------

#[test]
fn publisher_reports_how_many_events_a_handler_emitted() {
    let (ctx, now) = context();
    let emitted = Rc::new(RefCell::new(0usize));
    let mut bus = EventBus::new();
    let counter = emitted.clone();
    bus.on::<Tick, _>("fanout", move |tick, any, publisher: &mut Publisher<'_>| {
        for i in 0..3 {
            publisher.publish(
                any,
                "fanout",
                any.occurred_at,
                Anomaly {
                    symbol: tick.body.symbol.clone(),
                    z_score: f64::from(i),
                },
            )?;
        }
        *counter.borrow_mut() = publisher.emitted();
        Ok(HandlerOutcome::Handled)
    });
    bus.publish(
        &ctx,
        root_lineage("feed"),
        now,
        Tick {
            symbol: "A".into(),
            price: 1.0,
        },
    )
    .unwrap();
    bus.drain(&ctx).unwrap();
    assert_eq!(*emitted.borrow(), 3);
}

fn _assert_any_event_is_inspectable(event: &AnyEvent) -> Result<()> {
    let _ = event.summary();
    let _ = event.ingestion_latency();
    Ok(())
}

// --- the chain outlives the machine -----------------------------------------

#[test]
fn an_appended_record_is_on_the_platter_before_append_returns() {
    // The chain's whole purpose is to be evidence after the fact. Before this
    // was enforced, `append` wrote and returned without an fsync: the record
    // reached the page cache, a power cut removed it, and — because the chain
    // is computed over what was *retained* — the shortened log still verified.
    // A silently missing decision that leaves a valid-looking chain behind is
    // the worst shape this defect could take.
    //
    // This test cannot cut power, and says so rather than implying otherwise.
    // What it pins is the guarantee the code makes: synchronous is the
    // default, it is the only mode under which the promise holds, and an
    // acknowledged append is readable through a handle that is not this one.
    use qip_events::log::Durability;

    let dir = std::env::temp_dir().join(format!("qip-events-durable-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("events.jsonl");

    let (ctx, now) = context();
    {
        let mut log = EventLog::open(&path).unwrap();
        assert_eq!(
            log.durability(),
            Durability::Synchronous,
            "a file-backed audit log defaulted to a mode that loses records on power loss"
        );
        assert!(log.durability().survives_power_loss());

        let event = Envelope::new(
            ctx.ids().generate(now),
            now,
            now,
            root_lineage("feed"),
            Tick {
                symbol: "DURABLE".to_string(),
                price: 1.0,
            },
        )
        .erase()
        .unwrap();
        log.append(&event).unwrap();

        // Read through a separate handle while the log is still open: the
        // bytes left this process rather than sitting in its buffers.
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("DURABLE"),
            "an acknowledged append is not in the file"
        );
    }

    let reopened = EventLog::open(&path).unwrap();
    assert_eq!(reopened.len(), 1, "the reopened log lost the record");
    assert!(
        reopened.verify_chain().is_ok(),
        "the reopened chain is broken"
    );

    // The other mode exists and is honest about what it gives up.
    assert!(!Durability::OsBuffered.survives_power_loss());

    let _ = std::fs::remove_dir_all(&dir);
}

// --- bounded runtime state --------------------------------------------------
//
// Every service publishes through this bus and records through this log, so a
// collection here that grows without limit grows without limit in every
// process the platform runs. These tests exist because four of them did.

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Trade {
    symbol: String,
    size: u32,
}

impl EventBody for Trade {
    const TOPIC: Topic = Topic::MarketTrade;
    const SCHEMA_VERSION: u32 = 1;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Fill {
    order: String,
}

impl EventBody for Fill {
    const TOPIC: Topic = Topic::OrderFilled;
    const SCHEMA_VERSION: u32 = 1;
}

fn erased<T: EventBody>(ctx: &Context, now: Timestamp, body: T) -> AnyEvent {
    Envelope::new(
        ctx.ids().generate(now),
        now,
        now,
        root_lineage("test"),
        body,
    )
    .erase()
    .unwrap()
}

fn tick(symbol: &str) -> Tick {
    Tick {
        symbol: symbol.to_string(),
        price: 1.0,
    }
}

#[test]
fn a_zero_capacity_is_refused_at_construction_rather_than_raised_to_one() {
    // Silently promoting zero to one would let a configuration mistake run,
    // and the bus would then look like a platform that had stopped producing
    // events rather than one that had been misconfigured.
    for (label, error) in [
        (
            "queue",
            EventBus::new()
                .max_queue_depth(0)
                .err()
                .map(|e| e.to_string()),
        ),
        (
            "dedup",
            EventBus::new()
                .dedup_capacity(0)
                .err()
                .map(|e| e.to_string()),
        ),
        (
            "failures",
            EventBus::new()
                .max_recorded_failures(0)
                .err()
                .map(|e| e.to_string()),
        ),
        (
            "log",
            EventLog::in_memory()
                .with_capacity(0)
                .err()
                .map(|e| e.to_string()),
        ),
    ] {
        let message = error.unwrap_or_else(|| panic!("zero {label} capacity was accepted"));
        assert!(
            message.contains("zero"),
            "the {label} refusal must name the value it refused: {message}"
        );
    }

    // And the same constructors admit a good value — a gate that refuses
    // everything is not a gate.
    assert!(EventBus::new().max_queue_depth(1).is_ok());
    assert!(EventBus::new().dedup_capacity(1).is_ok());
    assert!(EventBus::new().max_recorded_failures(1).is_ok());
    assert!(EventLog::in_memory().with_capacity(1).is_ok());
}

#[test]
fn publishing_into_a_full_queue_is_refused_and_the_refusal_is_counted() {
    let (ctx, now) = context();
    let mut bus = EventBus::new().max_queue_depth(2).unwrap();
    for symbol in ["A", "B"] {
        bus.publish(&ctx, root_lineage("feed"), now, tick(symbol))
            .unwrap();
    }
    // Premise: the queue really is at capacity, so the next publish is the
    // first one the bound could refuse.
    assert_eq!(bus.queued(), 2);
    assert_eq!(bus.publishes_refused(), 0);

    let refusal = bus
        .publish(&ctx, root_lineage("feed"), now, tick("C"))
        .unwrap_err()
        .to_string();
    assert!(
        refusal.contains("max_queue_depth"),
        "the refusal must name what to change: {refusal}"
    );
    assert_eq!(bus.publishes_refused(), 1);
    assert_eq!(bus.queued(), 2, "a refused publish must not enqueue");

    // Refusing the newest keeps the oldest: nothing already accepted is lost.
    let dispatched = bus.drain(&ctx).unwrap();
    assert_eq!(dispatched, 2);
}

#[test]
fn a_handler_cannot_publish_past_the_queue_capacity_either() {
    // The most likely way to fill this queue is a handler publishing in
    // response to what it is handling, so the handler's publisher is the last
    // place that may be allowed to bypass the bound.
    let (ctx, now) = context();
    let mut bus = EventBus::new().max_queue_depth(1).unwrap();
    let emitted = Rc::new(RefCell::new(0usize));
    let counter = emitted.clone();
    bus.on::<Tick, _>("twin", move |_, any, publisher| {
        publisher.publish(
            any,
            "twin",
            any.occurred_at,
            Anomaly {
                symbol: "A".into(),
                z_score: 1.0,
            },
        )?;
        *counter.borrow_mut() = publisher.emitted();
        publisher.publish(
            any,
            "twin",
            any.occurred_at,
            Anomaly {
                symbol: "A".into(),
                z_score: 2.0,
            },
        )?;
        Ok(HandlerOutcome::Handled)
    });
    bus.publish(&ctx, root_lineage("feed"), now, tick("A"))
        .unwrap();
    let dispatched = bus.drain(&ctx).unwrap();

    // Premise: the first publish went through, so what follows is the bound
    // firing and not the handler failing for some other reason.
    assert_eq!(
        *emitted.borrow(),
        1,
        "the first handler publish must succeed"
    );
    assert_eq!(dispatched, 2, "the tick and the one anomaly that fitted");
    assert_eq!(bus.publishes_refused(), 1);
    assert_eq!(bus.failure_count(), 1);
    let failures: Vec<&DispatchFailure> = bus.failures().collect();
    assert!(
        failures[0].error.contains("event queue is full"),
        "the handler must be told why: {}",
        failures[0].error
    );
}

#[test]
fn the_deduplication_window_forgets_its_oldest_key_and_counts_the_loss() {
    // Eviction here is a correctness event, not just memory: past the window a
    // redelivery is dispatched a second time. That is tolerable only because
    // it is counted, so an operator can see the window is too short.
    let (ctx, now) = context();
    let dispatched = Rc::new(RefCell::new(Vec::<String>::new()));
    let mut bus = EventBus::new().dedup_capacity(2).unwrap();
    let record = dispatched.clone();
    bus.on::<Tick, _>("recorder", move |t, _, _| {
        record.borrow_mut().push(t.body.symbol.clone());
        Ok(HandlerOutcome::Handled)
    });

    for symbol in ["A", "B", "C"] {
        bus.publish(&ctx, root_lineage("feed"), now, tick(symbol))
            .unwrap();
        bus.drain(&ctx).unwrap();
    }
    assert_eq!(*dispatched.borrow(), vec!["A", "B", "C"]);
    assert_eq!(bus.dedup_evicted(), 1, "A must have left the window");

    // Premise: a key still inside the window is still suppressed, so the
    // window is working and only the evicted key comes back.
    bus.publish(&ctx, root_lineage("feed"), now, tick("C"))
        .unwrap();
    bus.drain(&ctx).unwrap();
    assert_eq!(
        *dispatched.borrow(),
        vec!["A", "B", "C"],
        "a key inside the window must still suppress its duplicate"
    );
    assert_eq!(bus.duplicates_suppressed(), 1);

    bus.publish(&ctx, root_lineage("feed"), now, tick("A"))
        .unwrap();
    bus.drain(&ctx).unwrap();
    assert_eq!(
        *dispatched.borrow(),
        vec!["A", "B", "C", "A"],
        "a key past the window is admitted again, which is what the counter warns about"
    );
    assert!(bus.dedup_evicted() >= 2);
}

#[test]
fn a_drain_that_hits_its_ceiling_leaves_the_next_drain_no_worse_off() {
    // This compounded once: the ceiling fired, the backlog stayed, and every
    // following drain started deeper and dispatched less real work.
    let (ctx, now) = context();
    let mut bus = EventBus::new().max_events_per_drain(50);
    bus.on::<Anomaly, _>("amplifier", |anomaly, any, publisher| {
        publisher.publish(
            any,
            "amplifier",
            any.occurred_at,
            Anomaly {
                symbol: anomaly.body.symbol.clone(),
                z_score: anomaly.body.z_score + 1.0,
            },
        )?;
        Ok(HandlerOutcome::Handled)
    });
    bus.publish(
        &ctx,
        root_lineage("feed"),
        now,
        Anomaly {
            symbol: "A".into(),
            z_score: 1.0,
        },
    )
    .unwrap();
    // Premise: there is work queued, so the drain has something to abandon.
    assert_eq!(bus.queued(), 1);

    let refusal = bus.drain(&ctx).unwrap_err().to_string();
    assert!(
        refusal.contains("publishing in a loop"),
        "the diagnosis must survive: {refusal}"
    );
    assert!(
        refusal.contains("1 queued events were abandoned"),
        "the loss must be stated, not implied: {refusal}"
    );
    assert_eq!(bus.queued(), 0, "the backlog must not survive the refusal");
    assert_eq!(bus.events_abandoned(), 1);
    assert_eq!(
        bus.drain(&ctx).unwrap(),
        0,
        "the next drain must start from nothing, not from the runaway's output"
    );
}

#[test]
fn the_failure_list_is_capped_and_says_how_much_it_dropped() {
    // A handler that fails on every event used to grow this once per event.
    let (ctx, now) = context();
    let mut bus = EventBus::new().max_recorded_failures(2).unwrap();
    bus.on::<Tick, _>("broken", |t, _, _| {
        Err(qip_core::Error::io(format!("broke on {}", t.body.symbol)))
    });
    for symbol in ["A", "B", "C"] {
        bus.publish(&ctx, root_lineage("feed"), now, tick(symbol))
            .unwrap();
    }
    // Premise: all three were dispatched, so all three failed and the cap is
    // what limits the list rather than the traffic.
    assert_eq!(bus.drain(&ctx).unwrap(), 3);

    assert_eq!(bus.failure_count(), 2, "the list must stop at its capacity");
    assert_eq!(bus.failures_dropped(), 1, "the loss must be visible");
    let errors: Vec<String> = bus.failures().map(|f| f.error.clone()).collect();
    assert!(
        errors[0].contains("broke on B") && errors[1].contains("broke on C"),
        "the most recent failures describe the current state: {errors:?}"
    );
}

#[test]
fn log_retention_spends_replaceable_records_before_it_spends_observations() {
    let (ctx, now) = context();
    let mut log = EventLog::in_memory().with_capacity(3).unwrap();
    log.append(&erased(&ctx, now, tick("T1"))).unwrap();
    log.append(&erased(
        &ctx,
        now,
        Trade {
            symbol: "TR1".into(),
            size: 1,
        },
    ))
    .unwrap();
    log.append(&erased(&ctx, now, tick("T2"))).unwrap();
    // Premise: retention is full and holds both kinds, so the next append has
    // a real choice to make.
    assert_eq!(log.len(), 3);
    assert_eq!(log.by_topic(Topic::MarketTick).len(), 2);

    for index in 2..5 {
        log.append(&erased(
            &ctx,
            now,
            Trade {
                symbol: format!("TR{index}"),
                size: index,
            },
        ))
        .unwrap();
    }

    assert_eq!(log.len(), 3, "retention must hold");
    assert_eq!(
        log.evicted_replaceable(),
        2,
        "both ticks go before any trade does"
    );
    assert_eq!(
        log.evicted_observations(),
        1,
        "and only then is a trade dropped, counted separately so it is not \
         buried under the routine loss"
    );
    assert!(
        log.by_topic(Topic::MarketTick).is_empty(),
        "the replaceable records should be the ones gone"
    );
}

#[test]
fn a_full_log_refuses_the_append_rather_than_dropping_an_audit_record() {
    // Dropping a fill to make room for the next one would leave the platform
    // acting with no account of what it did. Stopping is the lesser failure.
    let (ctx, now) = context();
    let mut log = EventLog::in_memory().with_capacity(2).unwrap();
    for order in ["ORD-1", "ORD-2"] {
        log.append(&erased(
            &ctx,
            now,
            Fill {
                order: order.to_string(),
            },
        ))
        .unwrap();
    }
    // Premise: both audit records are retained and retention is full.
    assert_eq!(log.len(), 2);
    assert_eq!(log.by_topic(Topic::OrderFilled).len(), 2);

    let refusal = log
        .append(&erased(
            &ctx,
            now,
            Fill {
                order: "ORD-3".into(),
            },
        ))
        .unwrap_err()
        .to_string();
    assert!(
        refusal.contains("permanent retention"),
        "the refusal must say why nothing could be dropped: {refusal}"
    );
    assert!(
        refusal.contains("larger capacity"),
        "and what to do instead: {refusal}"
    );
    assert_eq!(log.appends_refused(), 1);
    assert_eq!(log.len(), 2, "a refused append must change nothing");
    assert_eq!(log.evicted_replaceable() + log.evicted_observations(), 0);
}

#[test]
fn a_refused_append_writes_nothing_to_the_file() {
    // The file write used to happen before capacity was considered. A record
    // on disk that the log then refused is a history nobody can reconcile.
    let dir = std::env::temp_dir().join(format!("qip-log-refusal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("events.jsonl");
    let (ctx, now) = context();
    {
        let mut log = EventLog::open_with_capacity(&path, 2).unwrap();
        for order in ["ORD-1", "ORD-2"] {
            log.append(&erased(
                &ctx,
                now,
                Fill {
                    order: order.to_string(),
                },
            ))
            .unwrap();
        }
        // Premise: the accepted appends did reach the file.
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);

        assert!(
            log.append(&erased(
                &ctx,
                now,
                Fill {
                    order: "ORD-3".into()
                }
            ))
            .is_err()
        );
    }
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        written.lines().count(),
        2,
        "the refused record must not be on disk"
    );
    assert!(!written.contains("ORD-3"));

    // And a log that cannot hold the file's audit records refuses to open it,
    // rather than loading the whole file into the memory the ceiling exists to
    // protect.
    let refusal = EventLog::open_with_capacity(&path, 1)
        .unwrap_err()
        .to_string();
    assert!(
        refusal.contains("permanent retention"),
        "opening must refuse for the same stated reason: {refusal}"
    );
    // Premise for that refusal: a large enough ceiling admits the same file.
    assert_eq!(EventLog::open_with_capacity(&path, 2).unwrap().len(), 2);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn evicting_the_last_retained_record_restarts_neither_sequence_nor_chain() {
    // Deriving the next sequence from the last retained record made eviction
    // of the whole tail silently reissue sequence 1 and re-anchor the chain at
    // genesis: two records with one sequence number, and a chain that still
    // verifies while describing a history that never happened.
    let (ctx, now) = context();
    let mut log = EventLog::in_memory().with_capacity(1).unwrap();
    log.append(&erased(&ctx, now, tick("T1"))).unwrap();
    // Premise: exactly one record, which the next append must evict.
    assert_eq!(log.len(), 1);
    let first_hash = log.records()[0].record_hash.clone();
    assert_eq!(log.records()[0].sequence, 1);

    let sequence = log.append(&erased(&ctx, now, tick("T2"))).unwrap();
    assert_eq!(log.len(), 1, "the tail was evicted to make room");
    assert_eq!(sequence, 2, "sequence numbers must never be reissued");
    assert_eq!(
        log.records()[0].previous_hash,
        first_hash,
        "the chain must still name the record it followed"
    );
    assert_ne!(
        log.records()[0].previous_hash,
        qip_events::log::GENESIS_HASH
    );
}

#[test]
fn a_reused_event_id_is_refused_rather_than_silently_shadowing_the_earlier_record() {
    // by_event_id is a plain map keyed on event id. Before this test's
    // subject existed, a second append reusing an id already in the log
    // would overwrite that map entry with no error: EventLog::get would then
    // return the *second* record's content for a lookup naming the id of the
    // first, even though the first record was still sitting in the hash
    // chain, unreachable by identity. That is the same shape of gap as a
    // chain-position hash reused for different contents and silently
    // treated as a duplicate — just one layer up, in the lookup index rather
    // than the chain itself.
    let (ctx, now) = context();
    let mut log = EventLog::in_memory();
    let shared_id = ctx.ids().generate(now);

    let first = Envelope::new(
        shared_id.clone(),
        now,
        now,
        root_lineage("feed"),
        tick("T1"),
    )
    .erase()
    .unwrap();
    log.append(&first).unwrap();

    // Premise: the first record is indexed and reachable by its own id
    // before the second append happens at all.
    assert_eq!(log.len(), 1);
    assert_eq!(
        log.get(&shared_id).map(|e| e.payload["symbol"].clone()),
        Some(serde_json::json!("T1"))
    );

    let second = Envelope::new(
        shared_id.clone(),
        now,
        now,
        root_lineage("feed"),
        tick("T2"),
    )
    .erase()
    .unwrap();
    let result = log.append(&second);

    assert!(
        result.is_err(),
        "reusing an id already in the log must be refused, not silently indexed over"
    );
    assert_eq!(log.len(), 1, "the refused append must write nothing");
    assert_eq!(
        log.get(&shared_id).map(|e| e.payload["symbol"].clone()),
        Some(serde_json::json!("T1")),
        "the first record's content must still be the one this id resolves to"
    );
}

#[test]
fn a_file_with_a_reused_event_id_fails_to_load_rather_than_shadowing_the_earlier_record() {
    // The live-append refusal only helps if the same check runs when a log
    // is reconstructed from disk — otherwise a hand-edited or corrupted file
    // that duplicated an id would load cleanly with the index silently
    // pointing at the later record only.
    let dir = std::env::temp_dir().join(format!("qip-dup-event-id-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("events.jsonl");

    let (ctx, now) = context();
    {
        let mut log = EventLog::open(&path).unwrap();
        log.append(&erased(&ctx, now, tick("T1"))).unwrap();
        assert!(log.verify_chain().is_ok());
    }

    // Duplicate the one stored line, changing only its payload and leaving
    // its own hashes internally consistent — the corruption under test is
    // the reused event id, not a broken chain link.
    let text = std::fs::read_to_string(&path).unwrap();
    let mut record: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    record["event"]["payload"]["symbol"] = serde_json::json!("T2");
    let duplicate_line = serde_json::to_string(&record).unwrap();
    std::fs::write(&path, format!("{}\n{duplicate_line}\n", text.trim())).unwrap();

    let reopened = EventLog::open(&path);
    assert!(
        reopened.is_err(),
        "a file that reuses an event id across two records must fail to load"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The chain over what a capped log *retains* verifies after eviction, and
/// still catches a record edited on disk; and the last sequence survives a
/// reopen. `verify_chain` is held to genesis and reports the first retained
/// link once the log has evicted its head — the limit the module doc states
/// — which made it the wrong question for a consumer rebuilding state from
/// the retained span, and until 2026-09-12 no consumer asked any question at
/// all: the kernel restored its reference ledger from frames it had never
/// checked against their hashes.
///
/// Mutated by making `verify_retained_chain` skip the recomputed-hash
/// comparison — confirmed the tampered half then reads as intact and this
/// fails, then restored.
#[test]
fn the_retained_chain_verifies_after_eviction_and_catches_an_edited_record() {
    let dir = std::env::temp_dir().join(format!("qip-log-retained-chain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("events.jsonl");
    let (ctx, now) = context();
    {
        let mut log = EventLog::open(&path).unwrap();
        for index in 0..5 {
            let event = Envelope::new(
                ctx.ids().generate(now),
                now,
                now,
                root_lineage("feed"),
                Tick {
                    symbol: format!("T{index}"),
                    price: 100.0 + index as f64,
                },
            )
            .erase()
            .unwrap();
            log.append(&event).unwrap();
        }
        assert_eq!(
            log.last_sequence(),
            5,
            "premise: five records, five sequences"
        );
    }

    // Reopened under a ceiling of three, the two oldest ticks are evicted at
    // load: the retained span no longer starts at genesis.
    let capped = EventLog::open_with_capacity(&path, 3).unwrap();
    assert_eq!(capped.len(), 3, "premise: the ceiling evicted the head");
    assert_eq!(
        capped.last_sequence(),
        5,
        "the sequence must survive eviction, or a restarted process would mint the evicted \
         sequences again"
    );
    assert!(
        capped.verify_chain().is_err(),
        "premise: from genesis the evicted head reads as the first broken link, which is why a \
         second question exists"
    );
    assert_eq!(
        capped.verify_retained_chain(),
        Ok(()),
        "every retained record hashes to what its predecessor committed to"
    );

    // Edit the payload of the fourth record on disk, leaving its hashes as
    // they were: the edited record no longer hashes to what it claims. The
    // first holder is released first: a log another handle holds is refused.
    drop(capped);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut record: serde_json::Value = serde_json::from_str(&lines[3]).unwrap();
    record["event"]["payload"]["price"] = serde_json::json!(999.0);
    lines[3] = serde_json::to_string(&record).unwrap();
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();

    let tampered = EventLog::open_with_capacity(&path, 3).unwrap();
    assert_eq!(
        tampered.verify_retained_chain(),
        Err(4),
        "the edited record must be named by its sequence"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file-backed log is one process's: a second open of the same path while
/// the first handle lives is refused, and admitted once the first is
/// dropped. Two writers on one file would each mint the next sequence from
/// the same tail — two records under one number, and every id minted from
/// that tail (a campaign id carries it) minted twice; until 2026-09-12
/// nothing refused the second writer, and the uniqueness the campaign id
/// claimed across restarts was silently false across concurrent writers.
///
/// Mutated by deleting the `try_lock` match in `open_with_capacity` —
/// confirmed the second open then succeeds and this fails, then restored.
#[test]
fn a_log_another_handle_holds_is_refused_until_the_handle_is_released() {
    let dir = std::env::temp_dir().join(format!("qip-log-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("events.jsonl");
    let (ctx, now) = context();

    let mut first = EventLog::open(&path).unwrap();
    first.append(&erased(&ctx, now, tick("T1"))).unwrap();
    let refused = EventLog::open(&path).expect_err("a log another handle holds was opened");
    assert_eq!(refused.code(), "denied", "got {refused:?}");
    assert!(
        refused.message().contains("held by another process"),
        "the refusal does not say who holds it: {refused}"
    );
    // The first holder is unaffected by the refused attempt.
    first.append(&erased(&ctx, now, tick("T2"))).unwrap();
    assert_eq!(first.len(), 2);

    drop(first);
    let second = EventLog::open(&path).expect("the lock is released with the handle");
    assert_eq!(
        second.len(),
        2,
        "the second holder loads what the first wrote, and nothing was written twice"
    );
    assert_eq!(second.verify_chain(), Ok(()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An inspection of a log a writer holds is refused by a message naming the
/// holder, succeeds once the writer is gone, and holds nothing afterwards:
/// the writer's open is not refused behind it, and an append to the
/// inspected log is refused by name and reaches neither memory nor the
/// file. Until 2026-09-12 the only open was the writer's, so `qip replay`
/// on a running node's journal got the lock refusal relabelled as "not an
/// event log this platform wrote" — an operator sent to look for corruption
/// in a file that was merely in use. And until later the same day an
/// inspected log accepted `append` silently: the record reached memory,
/// minted the next sequence over the file's chain and put nothing on disk,
/// which this test then asserted as the intended shape. A chain the file
/// does not hold, with no error to say so, is the thing a read-only open
/// exists to make impossible.
///
/// Mutated two ways. Deleting the `try_lock_shared` match in
/// `inspect_with_capacity` — confirmed the inspection then succeeds while
/// the writer holds the file and the first assertion fails. Deleting the
/// `inspected` refusal at the top of `append` — confirmed the append then
/// succeeds, the log reads four records, and the refusal assertion fails.
/// Each restored.
#[test]
fn an_inspection_of_a_held_log_names_the_holder_and_once_released_holds_nothing_itself() {
    let dir = std::env::temp_dir().join(format!("qip-log-inspect-held-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("events.jsonl");
    let (ctx, now) = context();

    let mut writer = EventLog::open(&path).unwrap();
    writer.append(&erased(&ctx, now, tick("T1"))).unwrap();
    writer.append(&erased(&ctx, now, tick("T2"))).unwrap();
    let refused = EventLog::inspect(&path).expect_err("a log a writer holds was inspected");
    assert_eq!(refused.code(), "denied", "got {refused:?}");
    assert!(
        refused.message().contains("held by another process"),
        "the refusal does not say who holds it: {refused}"
    );
    // The writer is unaffected by the refused inspection.
    writer.append(&erased(&ctx, now, tick("T3"))).unwrap();
    drop(writer);

    let mut inspected = EventLog::inspect(&path).expect("a released log is inspectable");
    assert_eq!(
        inspected.len(),
        3,
        "the inspection loads what the writer wrote"
    );
    assert_eq!(inspected.verify_chain(), Ok(()));
    // The inspection released its lock with the read: a writer opens the
    // file afterwards. And the inspected log's own append is refused by
    // name — the refusal says which open it came through and which to use
    // instead — so it reaches neither memory nor the file, and the file
    // still holds exactly what the writer wrote.
    let reopened = EventLog::open(&path).expect("an inspection must not hold the file");
    assert_eq!(reopened.len(), 3);
    drop(reopened);
    let refused = inspected
        .append(&erased(&ctx, now, tick("T4")))
        .expect_err("an inspected log accepted an append");
    assert_eq!(refused.code(), "denied", "got {refused:?}");
    assert!(
        refused.message().contains("EventLog::inspect")
            && refused.message().contains("EventLog::open"),
        "the refusal does not name the open it came through and the one to use: {refused}"
    );
    assert_eq!(
        inspected.len(),
        3,
        "a refused append to an inspected log reached memory"
    );
    assert_eq!(inspected.verify_chain(), Ok(()));
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        on_disk
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        3,
        "an append to an inspected log reached the file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An inspection of a path that does not exist is refused by name and
/// leaves nothing behind — neither the file nor its parent directory. The
/// writer's open creates both and returns an empty log, which is right for
/// a node starting its journal and wrong for a checker: pointed at a
/// mistyped path it would have verified nothing against nothing and left an
/// empty file where the operator would look next.
///
/// Mutated by opening the file in `inspect_with_capacity` through
/// `OpenOptions::new().create(true).append(true).read(true)` — confirmed
/// the file then exists after the refusal and this fails, then restored.
#[test]
fn an_inspection_of_a_missing_path_refuses_by_name_and_creates_nothing() {
    let dir = std::env::temp_dir().join(format!("qip-log-inspect-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // The directory exists and the file does not: the one case in which a
    // creating open would succeed and leave a file behind. A path whose
    // parent is absent too fails any open, and proved nothing about
    // creation when this test first used one.
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("events.jsonl");
    assert!(!path.exists(), "premise: the file is not there yet");

    let refused = EventLog::inspect(&path).expect_err("a missing path was inspected");
    assert_eq!(refused.code(), "not_found", "got {refused:?}");
    assert!(
        refused.message().contains(&path.display().to_string())
            && refused.message().contains("will not create"),
        "the refusal does not name the path or say it creates nothing: {refused}"
    );
    assert!(!path.exists(), "the inspection created the file");
    // Nor a directory: the writer's open creates the parent, and a checker
    // pointed at a mistyped directory must not.
    let nested = dir.join("nested").join("events.jsonl");
    let _ = EventLog::inspect(&nested).expect_err("a path under a missing directory was inspected");
    assert!(
        !dir.join("nested").exists(),
        "the inspection created the parent directory"
    );

    // The contrast that makes the assertion above meaningful: the writer's
    // open does create it, and that is the behaviour a checker must not
    // inherit.
    let created = EventLog::open(&path).expect("the writer's open creates the path");
    assert!(created.is_empty() && path.exists());
    drop(created);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A journal on read-only storage — an archive, which is exactly where the
/// CLI's replay is pointed — is inspectable, where the writer's open, which
/// needs append, is refused. Until 2026-09-12 `qip replay` used the
/// writer's open and reported such a journal as "not an event log this
/// platform wrote".
///
/// The read-only half is provable only by a process the mode bits bind: a
/// privileged one (uid 0, which is what a container build often runs as)
/// opens a `0o444` file for append regardless, so the premise is probed
/// rather than assumed, and when it does not hold the test still proves
/// the inspection loads the file and says which half it could not prove.
/// CI's runner is unprivileged, so the whole property is proven there.
///
/// Mutated, as an unprivileged user, by opening the file in
/// `inspect_with_capacity` through `OpenOptions::new().append(true).read(true)`
/// — confirmed the inspection is then refused with a permission error and
/// this fails, then restored.
#[cfg(unix)]
#[test]
fn a_journal_on_read_only_storage_is_inspectable_where_the_writers_open_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("qip-log-inspect-ro-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("events.jsonl");
    let (ctx, now) = context();
    let mut writer = EventLog::open(&path).unwrap();
    writer.append(&erased(&ctx, now, tick("T1"))).unwrap();
    writer.append(&erased(&ctx, now, tick("T2"))).unwrap();
    drop(writer);

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let mode_binds = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .is_err();
    if mode_binds {
        let refused = EventLog::open(&path).expect_err("the writer's open succeeded read-only");
        assert_eq!(refused.code(), "io", "got {refused:?}");
    } else {
        eprintln!(
            "this process ignores the mode bits (privileged); the refusal of the writer's open \
             on read-only storage is not proven by this run"
        );
    }
    let inspected =
        EventLog::inspect(&path).expect("a journal on read-only storage is inspectable");
    assert_eq!(inspected.len(), 2);
    assert_eq!(inspected.verify_chain(), Ok(()));

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The retained chain verifies across an *interior* eviction, not only a
/// head one, and still names an edited record after the gap by sequence.
/// `make_room` evicts the oldest evictable record wherever it sits, so once
/// a permanent record is older than every evictable one the gap opens in
/// the middle of the retained span; until 2026-09-12 `verify_retained_chain`
/// re-anchored only the head and failed at the first record after such a
/// gap, and a file-backed kernel that had interleaved a permanent record
/// with its observations refused every restart over a log it had honestly
/// written, naming tampering.
///
/// Mutated by making `verify_chain_from` hold a record to its retained
/// predecessor across a gap (the `GapPolicy::Evicted` arm returning `hash`)
/// — confirmed the honest log then fails at sequence 5 and this fails, then
/// restored. And by deleting the recomputed-hash comparison — confirmed the
/// edited record then reads as intact and the tampered half fails, then
/// restored.
#[test]
fn the_retained_chain_verifies_across_an_interior_eviction_and_still_names_an_edited_record() {
    let dir =
        std::env::temp_dir().join(format!("qip-log-interior-eviction-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("events.jsonl");
    let (ctx, now) = context();
    {
        // Capacity three; tick, tick, fill, tick, tick, tick. The fill is
        // Act-group and permanent, so once it is the oldest retained record
        // the next eviction takes the tick *after* it.
        let mut log = EventLog::open_with_capacity(&path, 3).unwrap();
        log.append(&erased(&ctx, now, tick("T1"))).unwrap();
        log.append(&erased(&ctx, now, tick("T2"))).unwrap();
        log.append(&erased(
            &ctx,
            now,
            Fill {
                order: "ORD-1".to_string(),
            },
        ))
        .unwrap();
        log.append(&erased(&ctx, now, tick("T4"))).unwrap();
        log.append(&erased(&ctx, now, tick("T5"))).unwrap();
        log.append(&erased(&ctx, now, tick("T6"))).unwrap();
        let retained: Vec<u64> = log.records().iter().map(|r| r.sequence).collect();
        assert_eq!(
            retained,
            vec![3, 5, 6],
            "premise: the gap is interior — the permanent fill is retained ahead of it"
        );
        assert_eq!(
            log.verify_retained_chain(),
            Ok(()),
            "an honest log with an interior eviction must verify over what it retains"
        );
    }

    // The same shape reloaded from disk under the same ceiling.
    let reopened = EventLog::open_with_capacity(&path, 3).unwrap();
    let retained: Vec<u64> = reopened.records().iter().map(|r| r.sequence).collect();
    assert_eq!(
        retained,
        vec![3, 5, 6],
        "premise: the load evicts the same records"
    );
    assert_eq!(reopened.verify_retained_chain(), Ok(()));
    assert!(
        reopened.verify_chain().is_err(),
        "from genesis the retained span still does not verify, which is why a second question \
         exists"
    );

    // Edit the payload of the sixth record — after the gap — leaving its
    // hashes as written: the edit must still be named by its sequence. The
    // holder above is released first: a log another handle holds is refused.
    drop(reopened);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut record: serde_json::Value = serde_json::from_str(&lines[5]).unwrap();
    record["event"]["payload"]["price"] = serde_json::json!(999.0);
    lines[5] = serde_json::to_string(&record).unwrap();
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
    let tampered = EventLog::open_with_capacity(&path, 3).unwrap();
    assert_eq!(
        tampered.verify_retained_chain(),
        Err(6),
        "an edited record after the gap must be named by its sequence"
    );

    // A line removed from the file is not an eviction and must not be read
    // as one: the load refuses the file before any chain question is asked.
    drop(tampered);
    let mut without_fourth: Vec<String> = text.lines().map(str::to_string).collect();
    without_fourth.remove(3);
    std::fs::write(&path, format!("{}\n", without_fourth.join("\n"))).unwrap();
    let refused = EventLog::open_with_capacity(&path, 3)
        .expect_err("a file with a line removed loaded as if the log had evicted it");
    assert!(
        refused
            .message()
            .contains("sequence 5 where 4 was expected"),
        "the refusal does not name the gap: {refused}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
// --- the snapshot window ------------------------------------------------------
//
// The count ceiling bounds the working set by size; until 2026-09-19 nothing
// bounded it by age, so a quiet deployment that never reached the ceiling
// held every book snapshot it had ever recorded. The blueprint's retention
// for event-anchored book state is ninety days rolling (§54.2, §22.1). These
// tests hold the roll to four properties: it takes only the replaceable
// class, it can never take a permanent record, a replay across the roll
// still verifies — including after a restart, where the roll is re-derived
// from the file rather than remembered — and it does not run unless a caller
// asked for it, because two consumers of the retained span replay from
// genesis and refuse a gap (the module doc names them).

/// A day's worth of records: one replaceable snapshot, one permanent audit
/// record, one observation. Three classes, so a roll that took the wrong
/// one has something to take.
fn a_day_of_records(ctx: &Context, at: Timestamp, day: i64, log: &mut EventLog) {
    log.append(&erased(ctx, at, tick(&format!("SNAP{day}"))))
        .unwrap();
    log.append(&erased(
        ctx,
        at,
        Fill {
            order: format!("FILL{day}"),
        },
    ))
    .unwrap();
    log.append(&erased(
        ctx,
        at,
        Trade {
            symbol: format!("TR{day}"),
            size: 1,
        },
    ))
    .unwrap();
}

#[test]
fn the_snapshot_window_rolls_replaceable_records_by_age_and_never_a_permanent_one() {
    let (ctx, start) = context();
    let mut log = EventLog::in_memory()
        .with_snapshot_window(qip_events::log::SNAPSHOT_WINDOW)
        .unwrap();
    assert_eq!(
        log.snapshot_window(),
        Some(Duration::from_days(90)),
        "premise: the window set is the blueprint's ninety days"
    );

    // Nine days of records spread over eighty days, all inside one window
    // of each other, so nothing has rolled yet and the premise is clean.
    for day in (0..=80).step_by(10) {
        a_day_of_records(
            &ctx,
            start.saturating_add(Duration::from_days(day)),
            day,
            &mut log,
        );
    }
    assert_eq!(log.len(), 27, "premise: every record is retained");
    assert_eq!(log.by_topic(Topic::MarketTick).len(), 9);
    assert_eq!(log.by_topic(Topic::OrderFilled).len(), 9);
    assert_eq!(log.by_topic(Topic::MarketTrade).len(), 9);
    assert_eq!(log.rolled_by_age(), 0, "premise: nothing has aged out yet");

    // Day 200. Every snapshot recorded before day 110 is now past the
    // window; nothing else is a candidate whatever its age.
    log.append(&erased(
        &ctx,
        start.saturating_add(Duration::from_days(200)),
        tick("SNAP200"),
    ))
    .unwrap();

    assert_eq!(
        log.rolled_by_age(),
        9,
        "the nine stale snapshots roll off, and are counted as a roll"
    );
    assert_eq!(
        log.by_topic(Topic::MarketTick).len(),
        1,
        "only the snapshot inside the window remains"
    );
    assert_eq!(
        log.by_topic(Topic::OrderFilled).len(),
        9,
        "a permanent record is never a candidate for the roll, however old"
    );
    assert_eq!(
        log.by_topic(Topic::MarketTrade).len(),
        9,
        "an observation is the fallback series' business and is not rolled by this window"
    );
    assert_eq!(
        log.evicted_replaceable() + log.evicted_observations(),
        0,
        "the roll is retention working as stated, not pressure, and is counted apart"
    );

    // A replay across the roll: every retained record still hashes to what
    // it claimed, every retained link still holds, and the order is the
    // order things happened in.
    assert_eq!(log.verify_retained_chain(), Ok(()));
    let mut sequences = Vec::new();
    let visited = log
        .replay(|event| {
            sequences.push(event.sequence);
            Ok(())
        })
        .unwrap();
    assert_eq!(visited, log.len());
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "a replay across the roll must still be in sequence order: {sequences:?}"
    );
}

#[test]
fn a_reopened_log_re_derives_the_roll_from_its_file_and_still_verifies() {
    // The roll is of the index, not the file. A restart that asks for the
    // same window reads the file back and must arrive at the same working
    // set the writer held — the stale snapshots rolled, every permanent
    // record present — rather than at the whole file, and the retained span
    // must verify across the gaps the roll left. A restart that does not ask
    // holds the whole file: the bound is the writer's choice, not the file's.
    let dir = std::env::temp_dir().join(format!("qip-log-snapshot-window-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("events.jsonl");
    let (ctx, start) = context();

    let written = {
        let mut log = EventLog::open(&path)
            .unwrap()
            .with_snapshot_window(Duration::from_days(90))
            .unwrap();
        for day in (0..=80).step_by(10) {
            a_day_of_records(
                &ctx,
                start.saturating_add(Duration::from_days(day)),
                day,
                &mut log,
            );
        }
        log.append(&erased(
            &ctx,
            start.saturating_add(Duration::from_days(200)),
            tick("SNAP200"),
        ))
        .unwrap();
        assert_eq!(
            log.rolled_by_age(),
            9,
            "premise: the writer rolled nine snapshots"
        );
        log.len()
    };

    let reopened = EventLog::open(&path)
        .unwrap()
        .with_snapshot_window(Duration::from_days(90))
        .unwrap();
    assert_eq!(
        reopened.len(),
        written,
        "the reopened log must hold what the writer held, not the whole file"
    );
    assert_eq!(
        reopened.rolled_by_age(),
        9,
        "the roll is re-derived from the file, not remembered"
    );
    assert_eq!(reopened.by_topic(Topic::MarketTick).len(), 1);
    assert_eq!(
        reopened.by_topic(Topic::OrderFilled).len(),
        9,
        "no permanent record was lost across the restart"
    );
    assert_eq!(reopened.verify_retained_chain(), Ok(()));
    assert_eq!(
        reopened.replay(|_| Ok(())).unwrap(),
        written,
        "a replay after the restart visits exactly the retained span"
    );

    // The honest limit, pinned so it is not mistaken for a gap that closed:
    // the file is still append-only and every line ever written is on it,
    // and a reader that did not ask for the window gets all of it. Bounding
    // disk is the segmenting change the module doc says needs an ADR.
    drop(reopened);
    let unrolled = EventLog::open(&path).unwrap();
    assert_eq!(
        unrolled.len(),
        28,
        "a reopen without the window must hold the whole file"
    );
    assert_eq!(unrolled.rolled_by_age(), 0);
    drop(unrolled);
    let lines = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    assert_eq!(lines, 28, "the roll bounds the index, not the file");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_log_nobody_gave_a_window_rolls_nothing_however_old_the_snapshot() {
    // Off by default is a constraint, not a preference: `Platform::
    // resume_fabric` and `FabricJournal::resume` replay the retained span
    // from genesis and refuse a gap, so a log that rolled on its own could
    // not be resumed once it held a fabric record and a ninety-day-old
    // snapshot. On 2026-09-19 a default of ninety days made `qip-cli replay`
    // refuse a one-cycle journal for exactly that reason. This test is what
    // stops the default drifting back on to make the bound look reached.
    let (ctx, start) = context();
    let mut log = EventLog::in_memory();
    // Premise: the older snapshot is well past the blueprint's window
    // behind the newer one, so a log that rolled on its own would have
    // taken it.
    let gap = Duration::from_days(400);
    assert!(gap > qip_events::log::SNAPSHOT_WINDOW);
    log.append(&erased(&ctx, start, tick("OLD"))).unwrap();
    log.append(&erased(&ctx, start.saturating_add(gap), tick("NEW")))
        .unwrap();
    assert_eq!(
        log.by_topic(Topic::MarketTick).len(),
        2,
        "a log with no window rolled a snapshot on its own"
    );
    assert_eq!(log.rolled_by_age(), 0);
    assert_eq!(log.snapshot_window(), None, "and it reports no window");
}

#[test]
fn a_snapshot_window_that_retains_nothing_is_refused_rather_than_read_as_a_policy() {
    // Zero could mean "roll everything" or "roll nothing"; either reading is
    // a retention policy nobody wrote down.
    for window in [Duration::ZERO, Duration::from_days(-1)] {
        let error = EventLog::in_memory()
            .with_snapshot_window(window)
            .err()
            .unwrap_or_else(|| panic!("a window of {window:?} was accepted"));
        assert!(
            error.message().contains("positive duration"),
            "the refusal must say what to give instead: {}",
            error.message()
        );
    }
    let widened = EventLog::in_memory()
        .with_snapshot_window(Duration::from_days(365))
        .unwrap();
    assert_eq!(widened.snapshot_window(), Some(Duration::from_days(365)));
}
// --- the declared retention class (ADR 0089) ---------------------------------
//
// Blueprint §56.4 rule 33: every retained byte belongs to a declared
// retention class. Until ADR 0089 the log retained by topic *group* with two
// topics named by exception; now every topic declares a §22.1 row and the
// log's two retention seams read that row and nothing else. These tests hold
// three things: the declaration itself (the reviewer's copy of the table,
// so a swapped row fails here and not only in a roll), the property that
// the class and not the group decides what the roll takes, and the one tier
// the group-derived rule got backwards.

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Story {
    headline: String,
}

impl EventBody for Story {
    const TOPIC: Topic = Topic::NewsReceived;
    const SCHEMA_VERSION: u32 = 1;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Release {
    by: String,
}

impl EventBody for Release {
    const TOPIC: Topic = Topic::KillSwitchReleased;
    const SCHEMA_VERSION: u32 = 1;
}

#[test]
fn every_topic_declares_the_retention_class_its_record_carries() {
    use qip_events::retention::RetentionClass;
    // Premise: the closed set is the size the registry says, so a topic
    // added to `ALL` without a row below is a failure here and not a silent
    // default.
    assert_eq!(Topic::ALL.len(), 79, "the topic registry changed size");

    let expected = |topic: Topic| match topic {
        Topic::MarketTick | Topic::MarketQuote | Topic::MarketOrderBook => {
            RetentionClass::Transient
        }
        Topic::MarketBar => RetentionClass::FallbackSeries,
        Topic::MarketTrade
        | Topic::MarketCorporateAction
        | Topic::FundamentalUpdated
        | Topic::MacroUpdated
        | Topic::NewsReceived
        | Topic::AlternativeDataReceived
        | Topic::ReferenceDataUpdated
        | Topic::DataReferenceRecorded => RetentionClass::Referenced,
        Topic::DataQualityFailed => RetentionClass::CompactDerived,
        Topic::EntityUpdated
        | Topic::EntityResolved
        | Topic::RelationshipUpdated
        | Topic::WorldModelUpdated
        | Topic::FeatureComputed => RetentionClass::DerivedState,
        Topic::SignalGenerated
        | Topic::AnomalyDetected
        | Topic::RegimeChanged
        | Topic::OpportunityDetected
        | Topic::OpportunityRanked => RetentionClass::Episodic,
        Topic::InvestigationStarted
        | Topic::HypothesisCreated
        | Topic::EvidenceAttached
        | Topic::HypothesisChallenged
        | Topic::HypothesisApproved
        | Topic::HypothesisRejected
        | Topic::ThesisInvalidated
        | Topic::AgentRunCompleted => RetentionClass::Semantic,
        Topic::SimulationStarted
        | Topic::SimulationCompleted
        | Topic::ScenarioEvaluated
        | Topic::StrategyCreated => RetentionClass::CompactDerived,
        Topic::OptimizationRequested
        | Topic::OptimizationCompleted
        | Topic::SolverBenchmarked
        | Topic::PortfolioProposed
        | Topic::RiskEvaluated
        | Topic::RiskApproved
        | Topic::PolicyDistributed
        | Topic::RiskRejected
        | Topic::ComplianceEvaluated
        | Topic::RiskRuleRecalibration
        | Topic::VenueWithdrawn
        | Topic::VenueReinstated
        | Topic::OrderProposed
        | Topic::OrderApproved
        | Topic::OrderSubmitted
        | Topic::OrderAmended
        | Topic::OrderCancelled
        | Topic::OrderRejected
        | Topic::OrderFilled
        | Topic::PositionUpdated
        | Topic::PnlUpdated
        | Topic::ReconciliationCompleted
        | Topic::RegionDark
        | Topic::RegionLit
        | Topic::ServiceStarted
        | Topic::ServiceStopped
        | Topic::KillSwitchEngaged
        | Topic::KillSwitchReleased
        | Topic::AutonomyLevelChanged
        | Topic::BudgetExhausted
        | Topic::SystemAlert => RetentionClass::Irreplaceable,
        Topic::OutcomeObserved
        | Topic::AttributionCompleted
        | Topic::HypothesisScored
        | Topic::ModelEvaluated
        | Topic::LearningCompleted
        | Topic::LessonRecorded
        | Topic::SourceRevisionDetected
        | Topic::ResearchCampaignClosed
        | Topic::ResearchCampaignFlagged
        | Topic::RiskRuleDefended
        | Topic::RiskRuleDormant
        | Topic::SizingReviewed
        | Topic::FamilyAllocationReviewed => RetentionClass::Episodic,
    };
    for topic in Topic::ALL {
        assert_eq!(
            topic.retention_class(),
            expected(topic),
            "{topic} is filed under a different §22.1 row than this table says"
        );
        // And the two predicates the router, the mesh and the older tests
        // ask are the class's own answer, not a second list.
        assert_eq!(
            topic.is_lossy_tolerable(),
            topic.retention_class().is_replaceable(),
            "{topic}: lossy-tolerable disagrees with its class"
        );
        assert_eq!(
            topic.requires_permanent_retention(),
            topic.retention_class().is_permanent(),
            "{topic}: permanence disagrees with its class"
        );
    }
}

#[test]
fn a_topics_declared_retention_class_and_not_its_group_decides_whether_the_log_rolls_it() {
    use qip_events::retention::{Retention, RetentionClass};
    // Premise, stated about the *classes* and not about the topics: the
    // transient row is the replaceable one and the referenced row is not,
    // so the property below can only pass if each topic reaches the roll
    // through its declared row. A tick and a story are both Sense-group
    // topics — under the group-derived rule they were told apart by a list
    // written beside the group, and a topic missing from that list was an
    // observation by default.
    assert!(RetentionClass::Transient.is_replaceable());
    assert!(!RetentionClass::Referenced.is_replaceable());
    assert_eq!(
        RetentionClass::Referenced.retention(),
        Retention::ManifestOnly
    );
    assert_eq!(Topic::MarketTick.group(), Topic::NewsReceived.group());

    let (ctx, start) = context();
    let mut log = EventLog::in_memory()
        .with_snapshot_window(Duration::from_days(90))
        .unwrap();
    log.append(&erased(&ctx, start, tick("OLD-TICK"))).unwrap();
    log.append(&erased(
        &ctx,
        start,
        Story {
            headline: "OLD-STORY".into(),
        },
    ))
    .unwrap();
    log.append(&erased(
        &ctx,
        start,
        Fill {
            order: "OLD-FILL".into(),
        },
    ))
    .unwrap();
    assert_eq!(log.len(), 3, "premise: all three records are retained");

    // Day 200: everything above is well past the window.
    log.append(&erased(
        &ctx,
        start.saturating_add(Duration::from_days(200)),
        tick("NEW-TICK"),
    ))
    .unwrap();

    assert_eq!(
        log.by_topic(Topic::MarketTick).len(),
        1,
        "the old tick's class is {}, which is replaceable, so the roll takes it",
        Topic::MarketTick.retention_class().as_str()
    );
    assert_eq!(
        log.by_topic(Topic::NewsReceived).len(),
        1,
        "the story shares the tick's group and is filed under {}, which is a manifest the \
         log only indexes; a roll that took it was reading the group",
        Topic::NewsReceived.retention_class().as_str()
    );
    assert_eq!(
        log.by_topic(Topic::OrderFilled).len(),
        1,
        "a fill is {}, which is permanent, and no retention path may take it",
        Topic::OrderFilled.retention_class().as_str()
    );
    assert_eq!(log.rolled_by_age(), 1);
}

#[test]
fn the_kill_switchs_release_is_as_permanent_as_its_engagement() {
    // Under the group-derived rule `KillSwitchEngaged` was named as a
    // permanent exception in the System group and `KillSwitchReleased` was
    // not, so a full log would evict the record that says trading resumed
    // and keep the one that says it stopped. Both are the platform's own
    // control record and only this platform has them; the declared class
    // makes them one tier.
    assert_eq!(
        Topic::KillSwitchReleased.retention_class(),
        Topic::KillSwitchEngaged.retention_class()
    );
    let (ctx, now) = context();
    let mut log = EventLog::in_memory().with_capacity(2).unwrap();
    for by in ["A", "B"] {
        log.append(&erased(&ctx, now, Release { by: by.into() }))
            .unwrap();
    }
    assert_eq!(log.len(), 2, "premise: the log is full of releases");
    let refused = log
        .append(&erased(&ctx, now, tick("T")))
        .expect_err("a full log of kill-switch releases evicted one to admit a tick");
    assert!(
        refused.message().contains("(class transient)"),
        "the refusal must name the incoming record's class: {refused}"
    );
    assert_eq!(
        log.by_topic(Topic::KillSwitchReleased).len(),
        2,
        "a release was dropped to make room"
    );
    assert_eq!(log.evicted_observations(), 0);
}
