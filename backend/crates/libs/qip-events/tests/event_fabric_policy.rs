//! Contract tests for the event fabric's envelope, hybrid logical clock,
//! stream policy, catalogue and topic bindings (CONTRACT-036, CONTRACT-037,
//! CONTRACT-044, FABRIC-041/049/057/067/075/013; ADR 0100).

use qip_core::{Context, CorrelationId, Duration, Lineage, Timestamp};
use qip_events::envelope::EventBody;
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::envelope::{FabricEnvelope, FabricFacts};
use qip_events::event_fabric::hlc::{HlcTimestamp, PartitionClock};
use qip_events::event_fabric::policy::QosClass;
use qip_events::topic::Topic;
use serde::{Deserialize, Serialize};

fn context() -> (Context, Timestamp) {
    let now = Timestamp::from_civil(2026, 9, 25);
    let (ctx, _clock) = Context::deterministic(now, 11);
    (ctx, now)
}

fn root_lineage(producer: &str) -> Lineage {
    Lineage::root(
        CorrelationId::from_string("COR00000000000000000000009"),
        producer,
    )
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TestTick {
    price: i64,
}

impl EventBody for TestTick {
    const TOPIC: Topic = Topic::MarketTick;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("tick-{}", self.price))
    }
}

fn sample_facts() -> FabricFacts {
    FabricFacts {
        stream: "reflex-journal".to_string(),
        region: "us-east".to_string(),
        partition: 3,
        ordering_key: "cell-7".to_string(),
        producer_id: "cell-7-producer".to_string(),
        producer_epoch: 2,
        producer_sequence: 41,
        leader_epoch: 1,
        offset: 1000,
        qos_class: QosClass::P2MarketJournal,
        auth_context: "reflex:cell-7".to_string(),
        provenance: "seeded-tape:v1".to_string(),
    }
}

// --- envelope ----------------------------------------------------------------

#[test]
fn an_envelope_round_trips_every_contract_036_field_and_one_missing_field_is_refused() {
    let (ctx, now) = context();
    let logical = HlcTimestamp::new(now, 4);
    let envelope = FabricEnvelope::seal(
        ctx.ids().generate(now),
        root_lineage("event-fabric-test"),
        TestTick { price: 101 },
        now,
        now,
        logical,
        sample_facts(),
    )
    .expect("a fully populated envelope must be accepted");

    // Assert the premise first: every field carries the value given, not a
    // default, before trusting the round trip below.
    assert_eq!(envelope.stream(), "reflex-journal");
    assert_eq!(envelope.region(), "us-east");
    assert_eq!(envelope.partition(), 3);
    assert_eq!(envelope.ordering_key(), "cell-7");
    assert_eq!(envelope.producer_id(), "cell-7-producer");
    assert_eq!(envelope.producer_epoch(), 2);
    assert_eq!(envelope.producer_sequence(), 41);
    assert_eq!(envelope.leader_epoch(), 1);
    assert_eq!(envelope.offset(), 1000);
    assert_eq!(envelope.logical_timestamp(), logical);
    assert_eq!(envelope.qos_class(), QosClass::P2MarketJournal);
    assert_eq!(envelope.auth_context(), "reflex:cell-7");
    assert_eq!(envelope.provenance(), "seeded-tape:v1");
    assert_eq!(envelope.topic(), Topic::MarketTick);
    assert_eq!(envelope.schema_version(), 1);
    assert_eq!(envelope.occurred_at(), now);
    assert_eq!(envelope.recorded_at(), now);
    assert_eq!(envelope.idempotency_key(), Some("tick-101"));

    let wire = serde_json::to_value(&envelope).expect("a fabric envelope must serialise");
    let decoded: FabricEnvelope =
        serde_json::from_value(wire.clone()).expect("every CONTRACT-036 field must round trip");
    assert_eq!(decoded, envelope);

    // A missing (defaulted-to-empty) ordering key must be refused rather
    // than silently accepted as "no preference": CONTRACT-036 decides
    // fencing and ordering from this field, and an empty one is every
    // record sharing one wrong partition key.
    let mut broken = wire;
    broken["ordering_key"] = serde_json::json!("");
    let err = serde_json::from_value::<FabricEnvelope>(broken)
        .expect_err("an empty ordering key must be refused");
    assert!(err.to_string().contains("ordering key"), "{err}");
}

// --- catalogue: stream declarations ------------------------------------------

fn valid_stream_json(
    name: &str,
    qos_class: &str,
    ack_profile: &str,
    replication_factor: i64,
    mirroring: &str,
    topics: Vec<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "qos_class": qos_class,
        "partition_key": "cell",
        "ordering": "per_partition",
        "retention": "event_anchored",
        "replication_factor": replication_factor,
        "mirroring": mirroring,
        "overload_policy": "throttle_with_gap",
        "ack_profile": ack_profile,
        "byte_quota_per_producer": 1_048_576,
        "message_quota_per_producer": 10_000,
        "lag_limit": 1_000,
        "entitlement_dataset": "internal-reflex",
        "entitlement_usage": "trade",
        "seal_age_ms": 500,
        "peak_bytes_per_second": 5_000_000,
        "topics": topics,
    })
}

fn catalogue_bytes(streams: Vec<serde_json::Value>, grants: Vec<serde_json::Value>) -> Vec<u8> {
    serde_json::json!({ "streams": streams, "grants": grants })
        .to_string()
        .into_bytes()
}

#[test]
fn a_stream_missing_any_declaration_is_refused_by_name() {
    let mut stream = valid_stream_json("control", "p0_control", "quorum", 1, "none", vec![]);
    // Assert the premise: the fully populated declaration parses first, so
    // the refusal below is caused by the missing field and nothing else.
    Catalogue::parse(&catalogue_bytes(vec![stream.clone()], vec![]))
        .expect("a fully declared stream must be accepted");

    stream.as_object_mut().unwrap().remove("lag_limit");
    let err = Catalogue::parse(&catalogue_bytes(vec![stream], vec![]))
        .expect_err("a stream missing lag_limit must be refused");
    assert!(err.to_string().contains("lag_limit"), "{err}");
}

#[test]
fn an_ack_profile_weaker_than_the_stream_class_is_refused() {
    let ok = valid_stream_json("outcomes", "p1_outcomes", "quorum", 1, "none", vec![]);
    Catalogue::parse(&catalogue_bytes(vec![ok], vec![]))
        .expect("a P1 stream acknowledged at quorum must be accepted");

    let weak = valid_stream_json("outcomes", "p1_outcomes", "leader_only", 1, "none", vec![]);
    let err = Catalogue::parse(&catalogue_bytes(vec![weak], vec![]))
        .expect_err("a P1 stream acknowledged only at the leader must be refused");
    assert!(err.to_string().contains("p1_outcomes"), "{err}");
}

#[test]
fn a_replication_factor_other_than_one_is_refused_naming_c2() {
    let ok = valid_stream_json(
        "journal",
        "p2_market_journal",
        "leader_only",
        1,
        "none",
        vec![],
    );
    Catalogue::parse(&catalogue_bytes(vec![ok], vec![]))
        .expect("replication factor one must be accepted");

    let bad = valid_stream_json(
        "journal",
        "p2_market_journal",
        "leader_only",
        3,
        "none",
        vec![],
    );
    let err = Catalogue::parse(&catalogue_bytes(vec![bad], vec![]))
        .expect_err("replication factor three must be refused");
    assert!(err.to_string().contains("C2"), "{err}");
}

#[test]
fn mirroring_other_than_none_is_refused_naming_c8() {
    let ok = valid_stream_json(
        "journal",
        "p2_market_journal",
        "leader_only",
        1,
        "none",
        vec![],
    );
    Catalogue::parse(&catalogue_bytes(vec![ok], vec![])).expect("mirroring none must be accepted");

    let bad = valid_stream_json(
        "journal",
        "p2_market_journal",
        "leader_only",
        1,
        "selective",
        vec![],
    );
    let err = Catalogue::parse(&catalogue_bytes(vec![bad], vec![]))
        .expect_err("a selective mirroring policy must be refused");
    assert!(err.to_string().contains("C8"), "{err}");
}

#[test]
fn a_stream_admitting_a_transient_topic_outside_p4_is_refused() {
    // Assert the premise: `market.tick` really is lossy-tolerable, so the
    // refusal below is about which classes may admit it, not about the
    // topic being unadmittable anywhere.
    assert!(Topic::MarketTick.is_lossy_tolerable());

    let telemetry = valid_stream_json(
        "ticks-sampled",
        "p4_telemetry",
        "none",
        1,
        "none",
        vec!["market.tick"],
    );
    Catalogue::parse(&catalogue_bytes(vec![telemetry], vec![]))
        .expect("a P4 telemetry stream may admit a lossy-tolerable topic");

    let journal = valid_stream_json(
        "ticks-journal",
        "p2_market_journal",
        "leader_only",
        1,
        "none",
        vec!["market.tick"],
    );
    let err = Catalogue::parse(&catalogue_bytes(vec![journal], vec![]))
        .expect_err("a P2 market-journal stream must refuse a lossy-tolerable topic");
    assert!(err.to_string().contains("market.tick"), "{err}");
}

#[test]
fn a_stream_named_for_autonomy_or_live_is_refused() {
    let allowed = valid_stream_json(
        "livestock.x",
        "p3_research",
        "leader_only",
        1,
        "none",
        vec![],
    );
    Catalogue::parse(&catalogue_bytes(vec![allowed], vec![])).expect(
        "a name that merely starts with the same letters as a reserved prefix must be accepted",
    );

    for name in ["live.x", "autonomy.x", "ceiling.x"] {
        let bad = valid_stream_json(name, "p3_research", "leader_only", 1, "none", vec![]);
        let err = Catalogue::parse(&catalogue_bytes(vec![bad], vec![]))
            .expect_err(&format!("stream named '{name}' must be refused"));
        assert!(err.to_string().contains(name), "{err}");
    }
}

// --- hlc ----------------------------------------------------------------------

#[test]
fn the_partition_hlc_never_decreases_and_never_follows_a_producer_past_its_future_cap() {
    let t0 = Timestamp::from_civil(2026, 9, 25);
    let mut clock = PartitionClock::new(t0);

    // Premise: the clock starts where it was told to, at logical zero.
    assert_eq!(clock.last(), HlcTimestamp::new(t0, 0));

    let first = clock
        .tick(t0.saturating_add(Duration::from_millis(10)))
        .expect("a forward tick must be accepted");
    assert_eq!(
        first,
        HlcTimestamp::new(t0.saturating_add(Duration::from_millis(10)), 0)
    );

    // A caller supplying an earlier physical time than the clock already
    // holds must not move it backwards; it must still make forward
    // progress through the logical counter.
    let second = clock.tick(t0).expect("a backward tick must be accepted");
    assert!(
        second >= first,
        "the clock decreased: {second:?} < {first:?}"
    );
    assert_eq!(second.physical, first.physical);
    assert_eq!(second.logical, first.logical + 1);

    // The wall clock stepping backwards is exactly the case an HLC exists
    // for, and `receive` must resist it too, not only `tick`: a `now` *and*
    // a producer reading both behind the clock's own last physical time must
    // still leave the clock at its own last physical time, not regress to
    // whichever of `now` or the producer happens to be newer between
    // themselves. A comparison that dropped the clock's own last reading
    // from the max would pass `now.max(producer.physical)` here as `t0`,
    // strictly behind `second.physical` — a decrease.
    let backward_now = t0;
    let backward_producer = HlcTimestamp::new(t0, 0);
    let third = clock
        .receive(backward_now, backward_producer, Duration::from_secs(5))
        .expect("a receive with both readings behind the clock must be accepted");
    assert_eq!(
        third.physical, second.physical,
        "a receive with an earlier now and an earlier producer reading must not regress the clock's physical time"
    );
    assert!(
        third > second,
        "the clock decreased: {third:?} < {second:?}"
    );
    assert_eq!(third.logical, second.logical + 1);

    let cap = Duration::from_secs(5);
    let now = third.physical;

    // In bounds: a producer at most `cap` ahead is merged, moving the clock
    // forward to the newest reading among the three and never behind it.
    let close_producer = HlcTimestamp::new(now.saturating_add(Duration::from_secs(2)), 9);
    let merged = clock
        .receive(now, close_producer, cap)
        .expect("a producer within the cap must be accepted");
    assert_eq!(merged.physical, close_producer.physical);
    assert_eq!(merged.logical, close_producer.logical + 1);
    assert!(merged >= third);

    // Out of bounds: a producer claiming to be more than the cap ahead of
    // `now` must be refused outright, never silently adopted — adopting it
    // would drag every future local event behind an unverified reading.
    let far_producer = HlcTimestamp::new(now.saturating_add(Duration::from_secs(3600)), 0);
    let before = clock.last();
    let err = clock
        .receive(now, far_producer, cap)
        .expect_err("a producer more than the cap ahead must be refused");
    assert!(err.to_string().contains("ahead"), "{err}");
    assert_eq!(
        clock.last(),
        before,
        "a refused merge must not mutate the clock"
    );
}

#[test]
fn a_partition_hlc_refuses_to_advance_its_logical_counter_past_its_maximum_rather_than_wrap() {
    let t0 = Timestamp::from_civil(2026, 9, 25);
    let mut clock = PartitionClock::new(t0);

    // Premise: the clock starts at logical zero, so the refusal below is
    // caused by the producer's saturated counter and nothing else.
    let before = clock.last();
    assert_eq!(before.logical, 0);

    // A producer reporting the same physical time as the clock's own last
    // reading, with its logical counter already at u64::MAX, forces the
    // tie-break increment to overflow. Wrapping to zero would read as a
    // clock that had just reset — the exact decrease this type exists to
    // prevent — so it must be refused instead.
    let saturated_producer = HlcTimestamp::new(t0, u64::MAX);
    let err = clock
        .receive(t0, saturated_producer, Duration::from_secs(5))
        .expect_err("a logical counter already at its maximum must be refused rather than wrapped");
    assert!(err.to_string().contains(&u64::MAX.to_string()), "{err}");
    assert_eq!(
        clock.last(),
        before,
        "a refused advance must not mutate the clock"
    );
}

// --- catalogue: grants ---------------------------------------------------------

#[test]
fn a_grant_naming_an_undeclared_stream_or_an_unknown_permission_is_refused() {
    let stream = valid_stream_json("control", "p0_control", "quorum", 1, "none", vec![]);

    let good_grant = serde_json::json!({
        "identity": "release-controller",
        "stream": "control",
        "permission": "consume",
        "key_scope": "any",
    });
    Catalogue::parse(&catalogue_bytes(vec![stream.clone()], vec![good_grant]))
        .expect("a grant naming a declared stream with a known permission must be accepted");

    let undeclared = serde_json::json!({
        "identity": "release-controller",
        "stream": "missing-stream",
        "permission": "consume",
        "key_scope": "any",
    });
    let err = Catalogue::parse(&catalogue_bytes(vec![stream.clone()], vec![undeclared]))
        .expect_err("a grant naming an undeclared stream must be refused");
    assert!(err.to_string().contains("missing-stream"), "{err}");

    let unknown_permission = serde_json::json!({
        "identity": "release-controller",
        "stream": "control",
        "permission": "delete",
        "key_scope": "any",
    });
    let err = Catalogue::parse(&catalogue_bytes(vec![stream], vec![unknown_permission]))
        .expect_err("an unknown permission must be refused");
    assert!(err.to_string().contains("delete"), "{err}");
}

#[test]
fn a_cell_grant_must_be_scoped_to_its_own_key_and_only_the_release_controller_may_produce_to_p0() {
    let control = valid_stream_json("control", "p0_control", "quorum", 1, "none", vec![]);
    let outcomes = valid_stream_json("outcomes", "p1_outcomes", "quorum", 1, "none", vec![]);
    let streams = vec![control, outcomes];

    // Baseline: a cell scoped to its own key, consuming from a non-P0
    // stream, is accepted.
    let baseline_grant = serde_json::json!({
        "identity": "reflex:cell-3",
        "stream": "outcomes",
        "permission": "consume",
        "key_scope": "own_key",
    });
    Catalogue::parse(&catalogue_bytes(streams.clone(), vec![baseline_grant]))
        .expect("a cell scoped to its own key must be accepted");

    // A cell identity scoped to `any` key must be refused, whatever it asks
    // to do: an `any`-scoped cell could read or forge another cell's
    // partition.
    let any_scope = serde_json::json!({
        "identity": "reflex:cell-3",
        "stream": "outcomes",
        "permission": "consume",
        "key_scope": "any",
    });
    let err = Catalogue::parse(&catalogue_bytes(streams.clone(), vec![any_scope]))
        .expect_err("a cell grant scoped to any key must be refused");
    assert!(err.to_string().contains("reflex:cell-3"), "{err}");

    // Only the release controller may produce to a P0 stream, even with an
    // otherwise-correct own-key scope.
    let cell_to_p0 = serde_json::json!({
        "identity": "reflex:cell-3",
        "stream": "control",
        "permission": "produce",
        "key_scope": "own_key",
    });
    let err = Catalogue::parse(&catalogue_bytes(streams.clone(), vec![cell_to_p0]))
        .expect_err("a cell must not be granted produce on a P0 stream");
    assert!(err.to_string().contains("release-controller"), "{err}");

    let controller_to_p0 = serde_json::json!({
        "identity": "release-controller",
        "stream": "control",
        "permission": "produce",
        "key_scope": "own_key",
    });
    Catalogue::parse(&catalogue_bytes(streams, vec![controller_to_p0]))
        .expect("the release controller must be able to produce to a P0 stream");
}
