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
