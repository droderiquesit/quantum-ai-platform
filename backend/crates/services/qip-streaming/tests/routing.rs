//! Which wire an event goes down, and the combinations that cannot exist.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{at, hot_tick, research_source, venue_source, warm_note};
use qip_core::error::{Error, Result};
use qip_events::Topic;
use qip_streaming::{
    Confidence, DurableLogTransport, EventFacts, LocalQueue, PubSubBinding, PubSubTransport,
    Publisher, RoutingClass, SourceType, StreamEnvelope, Subject, Subscriber, TieredPublisher,
    TransportPath,
};

/// Topics spanning both sides of `Topic::is_lossy_tolerable` and both sides of
/// `TopicGroup::is_latency_critical`.
const TOPICS: [Topic; 4] = [
    Topic::MarketTick,
    Topic::MarketQuote,
    Topic::OrderFilled,
    Topic::NewsReceived,
];

#[test]
fn a_hot_routed_event_never_traverses_the_durable_buffer() -> Result<()> {
    // The durable side is a transport that cannot work at all. If a hot event
    // touched it — even to be handed over and refused — routing would fail.
    // It succeeding is the proof that the durable path is not called.
    let durable = PubSubTransport::declared(PubSubBinding::new(
        "projects/PROJECT/topics/qip-sense",
        "projects/PROJECT/subscriptions/qip-sense-worldmodel",
    ));
    let mut tiered = TieredPublisher::new(LocalQueue::new("hot", 64)?, Box::new(durable));

    for sequence in 1..=5 {
        let receipt = tiered.route(hot_tick(sequence, at(0), at(sequence as i64))?, at(10))?;
        assert_eq!(receipt.path, TransportPath::Local);
        assert!(
            receipt.record_hash.is_none(),
            "a hot event is not hashed; that is the cost the class exists to avoid"
        );
    }

    assert_eq!(tiered.counts().durable, 0, "the durable side was reached");
    assert_eq!(tiered.counts().local, 5);
    assert_eq!(tiered.local().depth(), 5);

    // And the durable side really would have been reached by anything else,
    // so the test above is not vacuous.
    let error = tiered
        .route(warm_note("A", at(0), at(10))?, at(10))
        .expect_err("a warm event must be handed to the durable transport");
    assert!(matches!(error, Error::Unavailable(_)));
    Ok(())
}

#[test]
fn a_hot_event_leaves_the_durable_log_untouched() -> Result<()> {
    let mut tiered = TieredPublisher::new(
        LocalQueue::new("hot", 64)?,
        Box::new(DurableLogTransport::in_memory("orders")),
    );
    for sequence in 1..=3 {
        tiered.route(hot_tick(sequence, at(0), at(sequence as i64))?, at(10))?;
    }
    assert_eq!(
        Publisher::descriptor(tiered.durable()).path,
        TransportPath::Durable
    );
    assert_eq!(tiered.counts().durable, 0);

    // Everything hot is drainable from the local queue, so nothing was lost by
    // not writing it down.
    let drained = tiered.local_mut().poll(at(60))?;
    assert_eq!(drained.len(), 3);
    assert!(
        drained
            .iter()
            .all(|envelope| envelope.transport_path() == TransportPath::Local)
    );
    Ok(())
}

#[test]
fn a_venue_critical_market_event_cannot_be_sealed_for_a_durable_buffer() -> Result<()> {
    for class in [RoutingClass::Warm, RoutingClass::Cold] {
        let facts = EventFacts::derived(
            venue_source(),
            Subject::default().with_origin(common::origin(1)),
            Topic::MarketTick,
        )
        .with_routing_class(class)
        .with_confidence(Confidence::CERTAIN);

        let outcome = StreamEnvelope::seal(
            common::event_id("X"),
            common::lineage("X"),
            common::tick(100, at(0)),
            at(0),
            at(1),
            facts,
        );
        let error = outcome.expect_err("a venue tick must not be sealed for the durable buffer");
        assert!(matches!(error, Error::Invalid(_)));
        assert!(
            error.message().contains("latency budget"),
            "the refusal must say what it is protecting: {error}"
        );
    }
    Ok(())
}

#[test]
fn an_event_that_cannot_be_lost_cannot_be_sealed_hot() -> Result<()> {
    let facts = EventFacts::derived(
        research_source(),
        Subject::unattributed(),
        Topic::OrderFilled,
    )
    .with_routing_class(RoutingClass::Hot);
    let outcome = StreamEnvelope::seal(
        common::event_id("Y"),
        common::lineage("Y"),
        common::AuditNote {
            note: "filled".into(),
        },
        at(0),
        at(1),
        facts,
    );
    let error = outcome.expect_err("an unreplaceable event must not be sealed hot");
    assert!(
        error.message().contains("drops its oldest entry"),
        "the refusal must say what the local path would do to it: {error}"
    );
    Ok(())
}

#[test]
fn every_class_has_exactly_one_path_and_the_paths_differ() {
    assert_eq!(RoutingClass::Hot.path(), TransportPath::Local);
    assert_eq!(RoutingClass::Warm.path(), TransportPath::Durable);
    assert_eq!(RoutingClass::Cold.path(), TransportPath::Durable);
    assert!(!TransportPath::Local.is_durable());
    assert!(TransportPath::Durable.is_durable());
    assert!(!RoutingClass::Hot.tolerates_buffering());
    assert!(RoutingClass::Warm.tolerates_buffering());
}

#[test]
fn the_derived_class_is_always_one_the_checks_accept() {
    for source_type in SourceType::ALL {
        for topic in TOPICS {
            let derived = RoutingClass::derive(source_type, topic);
            assert!(
                derived.check(source_type, topic).is_ok(),
                "derive produced {derived} for {source_type}/{topic}, which check rejects"
            );
        }
    }
}

#[test]
fn the_two_rules_partition_the_topics_they_apply_to() {
    for source_type in SourceType::ALL {
        for topic in TOPICS {
            let accepted: Vec<RoutingClass> = RoutingClass::ALL
                .into_iter()
                .filter(|class| class.check(source_type, topic).is_ok())
                .collect();
            assert!(
                !accepted.is_empty(),
                "no class is legal for {source_type}/{topic}, so nothing could be published"
            );

            if !topic.is_lossy_tolerable() {
                assert!(
                    !accepted.contains(&RoutingClass::Hot),
                    "{topic} cannot be lost but may be routed hot"
                );
            }
            if source_type.is_venue_critical()
                && topic.group().is_latency_critical()
                && topic.is_lossy_tolerable()
            {
                assert_eq!(
                    accepted,
                    vec![RoutingClass::Hot],
                    "a venue-critical {topic} must have exactly one legal class"
                );
            }
        }
    }
}

#[test]
fn a_venue_feed_is_the_only_venue_critical_source() {
    let venue_critical: Vec<SourceType> = SourceType::ALL
        .into_iter()
        .filter(SourceType::is_venue_critical)
        .collect();
    assert_eq!(venue_critical, vec![SourceType::VenueFeed]);
}
