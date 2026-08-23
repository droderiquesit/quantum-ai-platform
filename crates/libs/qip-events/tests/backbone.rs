//! Topics, envelopes, the deterministic bus and the hash-chained log.

use qip_core::error::Result;
use qip_core::{Context, CorrelationId, Duration, Lineage, Timestamp};
use qip_events::bus::{HandlerOutcome, Publisher};
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
    assert_eq!(bus.failures()[0].handler, "broken");
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
        replay_bus.publish_raw(event.clone());
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
    let mut log = EventLog::in_memory().with_capacity(5);

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
