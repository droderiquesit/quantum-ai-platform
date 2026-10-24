//! Contract tests for the event fabric's envelope, hybrid logical clock,
//! stream policy, catalogue and topic bindings (CONTRACT-036, CONTRACT-037,
//! CONTRACT-044, FABRIC-041/049/057/067/075/013; ADR 0100).

use std::collections::BTreeMap;

use qip_core::{Context, CorrelationId, Duration, Lineage, Timestamp};
use qip_events::RetentionClass;
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
        "overload_policy": overload_for(qos_class),
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

/// The overload policy ADR 0100 §5 gives each class, so a fixture stream
/// is refused only for the field a test changes.
fn overload_for(qos_class: &str) -> &'static str {
    match qos_class {
        "p0_control" | "p1_outcomes" => "refuse_producer",
        "p4_telemetry" => "sample_or_shed",
        _ => "throttle_with_gap",
    }
}

fn catalogue_bytes(streams: Vec<serde_json::Value>, grants: Vec<serde_json::Value>) -> Vec<u8> {
    serde_json::json!({ "streams": streams, "grants": grants })
        .to_string()
        .into_bytes()
}

#[test]
fn a_stream_missing_any_declaration_is_refused_by_name() {
    let mut stream = valid_stream_json("control", "p0_control", "archived", 1, "none", vec![]);
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
    let ok = valid_stream_json("outcomes", "p1_outcomes", "archived", 1, "none", vec![]);
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
    let stream = valid_stream_json("control", "p0_control", "archived", 1, "none", vec![]);

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
    let control = valid_stream_json("control", "p0_control", "archived", 1, "none", vec![]);
    let outcomes = valid_stream_json("outcomes", "p1_outcomes", "archived", 1, "none", vec![]);
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

/// FABRIC-110: an ambient or development agent publishes at P3 and P4 only.
///
/// The failure this prevents: the broker's ACL refuses whatever no grant
/// covers, so an agent was kept off the outcome stream only for as long as
/// nobody wrote the grant. With this rule a catalogue carrying that grant
/// does not parse, and a broker neither starts on it nor reloads it.
///
/// Asserts its premise first, twice: the agent *is* granted the two classes
/// it may publish to, and another identity *is* granted the same P1 produce
/// the agent is refused — so each refusal below is about the identity and
/// the class together, not about a grant shape the parser dislikes.
///
/// Mutation (run, failed, restored): the rule's class test admitting
/// `P1Outcomes` beside research and telemetry — the P1 grant parses.
#[test]
fn an_agent_identity_is_granted_produce_or_admin_on_research_and_telemetry_streams_only() {
    let streams = vec![
        valid_stream_json("control", "p0_control", "archived", 1, "none", vec![]),
        valid_stream_json("outcomes", "p1_outcomes", "archived", 1, "none", vec![]),
        valid_stream_json(
            "journal",
            "p2_market_journal",
            "leader_only",
            1,
            "none",
            vec![],
        ),
        valid_stream_json("research", "p3_research", "leader_only", 1, "none", vec![]),
        valid_stream_json("telemetry", "p4_telemetry", "none", 1, "none", vec![]),
    ];
    let grant = |identity: &str, stream: &str, permission: &str| {
        serde_json::json!({
            "identity": identity,
            "stream": stream,
            "permission": permission,
            "key_scope": "any",
        })
    };

    let admitted = vec![
        grant("agent:research-1", "research", "produce"),
        grant("agent:research-1", "telemetry", "produce"),
        grant("agent:research-1", "research", "admin"),
        // Reading outcomes is what research is for.
        grant("agent:research-1", "outcomes", "consume"),
        // The same P1 produce, to an identity that is not an agent.
        grant("ledger-backfill", "outcomes", "produce"),
    ];
    let catalogue = Catalogue::parse(&catalogue_bytes(streams.clone(), admitted))
        .expect("premise: an agent's P3 and P4 grants, and another identity's P1 grant, parse");
    assert_eq!(catalogue.grants().len(), 5);

    for stream in ["control", "outcomes", "journal"] {
        for permission in ["produce", "admin"] {
            let refused = Catalogue::parse(&catalogue_bytes(
                streams.clone(),
                vec![grant("agent:research-1", stream, permission)],
            ))
            .expect_err("an agent must not be granted produce or admin above P3");
            let message = refused.to_string();
            assert!(
                message.contains("agent identity") && message.contains(&format!("'{stream}'")),
                "{permission} on {stream}: {message}"
            );
        }
    }
}

// --- archive-required -----------------------------------------------------

#[test]
fn archive_required_is_decided_for_every_retention_class_with_no_wildcard() {
    // Every one of §22.1's nine rows gets its own named answer, so a tenth
    // row added later has nothing to inherit silently — the match in
    // `RetentionClass::archive_required` has no wildcard arm, and this table
    // would then be missing a key rather than quietly agreeing with it.
    let expected: BTreeMap<RetentionClass, bool> = BTreeMap::from([
        (RetentionClass::Transient, false),
        (RetentionClass::DerivedState, false),
        (RetentionClass::Irreplaceable, true),
        (RetentionClass::CompactDerived, true),
        (RetentionClass::Episodic, true),
        (RetentionClass::Semantic, true),
        (RetentionClass::EventAnchored, true),
        (RetentionClass::FallbackSeries, true),
        (RetentionClass::Referenced, true),
    ]);
    // Premise: the table above names every declared row before any single
    // comparison below is trusted to mean anything.
    assert_eq!(
        expected.len(),
        RetentionClass::ALL.len(),
        "this table must name every retention class or the loop below skips one silently"
    );

    for class in RetentionClass::ALL {
        assert_eq!(
            class.archive_required(),
            expected[&class],
            "'{}' disagrees with the expected archive-required answer",
            class.as_str()
        );
        // The rule is exactly "not replaceable" (§22.1): a working-set record
        // that already rolls by age behind a bound the log does not own is
        // never what an archive exists to rescue, and every other row must
        // be, because nothing else bounds it before an archive does.
        assert_eq!(
            class.archive_required(),
            !class.is_replaceable(),
            "'{}' must be archive-required exactly when it is not replaceable",
            class.as_str()
        );
    }
}

/// FABRIC-057's stated check, over all seven declarations rather than the
/// one (`lag_limit`) the test above samples: creating a stream with any of
/// class, partitioning key, ordering, retention, replication, overload or
/// mirroring missing is refused naming that field, and with all present the
/// stored definition round-trips every declared value.
///
/// Mutation: in `catalogue.rs`, make `parse_stream`'s `required(...)` for
/// `overload_policy` fall back to `"sample_or_shed"` instead of refusing —
/// fails on the `overload_policy` iteration, because the stream is then
/// accepted with a value nobody declared.
#[test]
fn a_stream_missing_any_of_the_seven_declarations_is_refused_naming_it_and_a_full_one_round_trips()
{
    use qip_events::event_fabric::policy::{Mirroring, Ordering, OverloadPolicy};

    let full = valid_stream_json("outcomes", "p1_outcomes", "archived", 1, "none", vec![]);
    let catalogue = Catalogue::parse(&catalogue_bytes(vec![full.clone()], vec![]))
        .expect("a fully declared stream must be accepted");
    let stored = &catalogue
        .stream("outcomes")
        .expect("the accepted stream is stored under its name")
        .policy;
    assert_eq!(stored.qos_class(), QosClass::P1Outcomes);
    assert_eq!(stored.partition_key(), "cell");
    assert_eq!(stored.ordering(), Ordering::PerPartition);
    assert_eq!(stored.retention(), RetentionClass::EventAnchored);
    assert_eq!(stored.replication_factor(), 1);
    assert_eq!(stored.mirroring(), Mirroring::None);
    assert_eq!(stored.overload_policy(), OverloadPolicy::RefuseProducer);

    let seven = [
        "qos_class",
        "partition_key",
        "ordering",
        "retention",
        "replication_factor",
        "overload_policy",
        "mirroring",
    ];
    for field in seven {
        let mut broken = full.clone();
        let removed = broken
            .as_object_mut()
            .expect("a stream declaration is an object")
            .remove(field);
        assert!(
            removed.is_some(),
            "premise: the fixture declares {field}, so removing it removes something"
        );
        let err = Catalogue::parse(&catalogue_bytes(vec![broken], vec![]))
            .expect_err("a stream missing a declaration must be refused");
        assert!(
            err.to_string().contains(field),
            "the refusal for a missing {field} must name it: {err}"
        );
    }
}

/// R12-FABRIC-7055's stated check, over every field the requirement names:
/// byte quota, message quota, lag limit and overload policy. The test above
/// samples `lag_limit` alone, so until this test either quota could have
/// become a parser default — a stream nobody bounded, admitted with a bound
/// nobody chose — and no test would have failed.
///
/// Mutation: in `catalogue.rs`, make `parse_stream`'s `required(...)` for
/// `byte_quota_per_producer` (or `message_quota_per_producer`) fall back to
/// a default instead of refusing — fails on that field's iteration, because
/// the stream is then accepted.
#[test]
fn a_stream_missing_a_byte_or_message_quota_lag_limit_or_overload_policy_is_refused_naming_it() {
    let full = valid_stream_json("outcomes", "p1_outcomes", "archived", 1, "none", vec![]);
    let catalogue = Catalogue::parse(&catalogue_bytes(vec![full.clone()], vec![]))
        .expect("a fully declared stream must be accepted");
    let stored = &catalogue
        .stream("outcomes")
        .expect("the accepted stream is stored under its name")
        .policy;
    // The declared bounds are the stored bounds, so a refusal below is about
    // absence and not about a value the parser would have rewritten anyway.
    assert_eq!(stored.byte_quota_per_producer(), 1_048_576);
    assert_eq!(stored.message_quota_per_producer(), 10_000);
    assert_eq!(stored.lag_limit(), 1_000);

    for field in [
        "byte_quota_per_producer",
        "message_quota_per_producer",
        "lag_limit",
        "overload_policy",
    ] {
        let mut broken = full.clone();
        let removed = broken
            .as_object_mut()
            .expect("a stream declaration is an object")
            .remove(field);
        assert!(
            removed.is_some(),
            "premise: the fixture declares {field}, so removing it removes something"
        );
        let err = Catalogue::parse(&catalogue_bytes(vec![broken], vec![]))
            .expect_err("a stream missing a quota, lag limit or overload policy must be refused");
        // Match the field as a delimited token: `lag_limit` alone would also
        // be a substring of a message naming some longer field.
        let text = err.to_string();
        assert!(
            text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .any(|token| token == field),
            "the refusal for a missing {field} must name it: {text}"
        );
    }
}

/// CONTRACT-036's second half: a record with any mandatory field absent is
/// refused. Removes each key of the wire document in turn, and empties each
/// text fact in turn, so a field added later without validation is caught by
/// the premise count rather than silently admitted.
///
/// Mutation: stop checking `provenance` in `FabricEnvelope::assemble` — the
/// emptied `provenance` below is then admitted and this test fails naming it.
#[test]
fn an_envelope_with_any_mandatory_field_absent_or_emptied_is_refused() {
    let (ctx, now) = context();
    let envelope = FabricEnvelope::seal(
        ctx.ids().generate(now),
        root_lineage("event-fabric-test"),
        TestTick { price: 101 },
        now,
        now,
        HlcTimestamp::new(now, 4),
        sample_facts(),
    )
    .expect("a fully populated envelope must be accepted");
    let wire = serde_json::to_value(&envelope).expect("serialises");
    let keys: Vec<String> = wire
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    assert!(
        keys.len() >= 14,
        "premise: the wire form carries every CONTRACT-036 fact, found {keys:?}"
    );

    for key in &keys {
        let mut broken = wire.clone();
        broken.as_object_mut().expect("an object").remove(key);
        assert!(
            serde_json::from_value::<FabricEnvelope>(broken).is_err(),
            "an envelope with `{key}` removed must be refused"
        );
    }
    for key in [
        "stream",
        "region",
        "ordering_key",
        "producer_id",
        "auth_context",
        "provenance",
    ] {
        let mut broken = wire.clone();
        broken[key] = serde_json::json!("");
        assert!(
            serde_json::from_value::<FabricEnvelope>(broken).is_err(),
            "an envelope with `{key}` emptied must be refused"
        );
    }
}

/// FABRIC-8086 (R2.9): `quorum` means a majority of replicas, and a stream
/// with one replica is refused for declaring it, by name, rather than loaded
/// as the single-broker `archived` profile. Until this, the parser aliased
/// the word silently and the committed catalogue said "quorum" on two
/// replication-factor-one streams — the INCORRECT verdict. Where the stream
/// does declare the managed consensus's replication (ADR 0111), the old
/// spelling still loads, as the replicated profile it always claimed to be.
///
/// This replaces `the_archived_ack_profile_reads_its_old_quorum_spelling_…`,
/// which asserted the alias this item exists to remove.
///
/// Mutation: restore `"archived" | "quorum" => Ok(AckProfile::Archived)` in
/// `parse_ack_profile` — the RF1 `quorum` stream parses and the first
/// `expect_err` fires. Mutation: drop the `"quorum" if replication_factor ==
/// MANAGED_REPLICATION_FACTOR` arm — the RF3 legacy stream is refused and the
/// `expect` on it fires. Mutation: move `#[serde(alias = "quorum")]` back to
/// `Archived` — the serde assertion fires.
#[test]
fn quorum_is_refused_on_a_stream_with_one_replica_and_read_as_managed_majority_only_where_replicated()
 {
    use qip_events::event_fabric::policy::AckProfile;

    let current = valid_stream_json("outcomes", "p1_outcomes", "archived", 1, "none", vec![]);
    let current =
        Catalogue::parse(&catalogue_bytes(vec![current], vec![])).expect("premise: archived loads");
    assert_eq!(
        current
            .stream("outcomes")
            .expect("declared")
            .policy
            .ack_profile(),
        AckProfile::Archived
    );

    let single = valid_stream_json("outcomes", "p1_outcomes", "quorum", 1, "none", vec![]);
    let refused = Catalogue::parse(&catalogue_bytes(vec![single], vec![]))
        .expect_err("quorum on one replica is a claim nothing honours");
    assert!(
        refused
            .to_string()
            .contains("'quorum' at replication factor 1"),
        "{refused}"
    );

    let replicated = valid_stream_json("outcomes", "p1_outcomes", "quorum", 3, "none", vec![]);
    let replicated = Catalogue::parse(&catalogue_bytes(vec![replicated], vec![]))
        .expect("quorum on the managed replication path is the replicated profile");
    let policy = &replicated.stream("outcomes").expect("declared").policy;
    assert_eq!(policy.ack_profile(), AckProfile::ManagedMajority);
    assert!(policy.is_managed());
    assert_eq!(policy.replication_factor(), 3);

    assert_eq!(
        serde_json::to_value(AckProfile::Archived).expect("serialises"),
        serde_json::json!("archived")
    );
    let stored: AckProfile =
        serde_json::from_value(serde_json::json!("quorum")).expect("the stored old name reads");
    assert_eq!(stored, AckProfile::ManagedMajority);
}

/// ADR 0111 §3: `managed_majority` is admitted exactly where it is true — P0
/// and P1 at the managed consensus's replication factor — and refused for
/// P2 to P4 and at any other factor, while an archived stream still may not
/// claim a replica.
///
/// Mutation: delete the class check in `StreamPolicy::new`'s replicated arm —
/// the P2 managed stream is admitted and its `expect_err` fires. Mutation:
/// delete the `!= MANAGED_REPLICATION_FACTOR` check — the RF1 managed stream
/// is admitted and fires.
#[test]
fn managed_majority_is_admitted_for_p0_and_p1_at_rf3_and_refused_everywhere_else() {
    for class in ["p0_control", "p1_outcomes"] {
        let ok = valid_stream_json("s", class, "managed_majority", 3, "none", vec![]);
        Catalogue::parse(&catalogue_bytes(vec![ok], vec![]))
            .unwrap_or_else(|e| panic!("{class} managed at RF3 must load: {e}"));
    }
    for class in ["p2_market_journal", "p3_research", "p4_telemetry"] {
        let bad = valid_stream_json("s", class, "managed_majority", 3, "none", vec![]);
        let err = Catalogue::parse(&catalogue_bytes(vec![bad], vec![]))
            .expect_err("P2-P4 have no replicated path");
        assert!(err.to_string().contains("ADR 0111 §2"), "{class}: {err}");
    }
    for rf in [1, 2, 5] {
        let bad = valid_stream_json("s", "p1_outcomes", "managed_majority", rf, "none", vec![]);
        let err = Catalogue::parse(&catalogue_bytes(vec![bad], vec![]))
            .expect_err("a managed stream must declare the managed replication factor");
        assert!(err.to_string().contains("declare 3"), "rf {rf}: {err}");
    }
    let archived_rf3 = valid_stream_json("s", "p1_outcomes", "archived", 3, "none", vec![]);
    let err = Catalogue::parse(&catalogue_bytes(vec![archived_rf3], vec![]))
        .expect_err("an archived stream has one replica");
    assert!(err.to_string().contains("(C2)"), "{err}");
}

/// R2.9: the committed local catalogue declares what the local broker
/// delivers. It said `"quorum"` on its P0 and P1 streams at replication
/// factor one until 2026-10-11; with the alias gone that file would stop
/// every local broker, and before the alias went it was the INCORRECT row's
/// own evidence.
///
/// Mutation: put `"ack_profile": "quorum"` back on `control.local` — the
/// parse fails and this fires.
#[test]
fn the_committed_local_catalogue_parses_and_claims_no_replication() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../infrastructure/event-fabric/streams.local.json"
    );
    let bytes = std::fs::read(path).expect("premise: the committed catalogue is readable");
    let catalogue = Catalogue::parse(&bytes).expect("the committed catalogue parses");
    assert!(
        !catalogue.streams().is_empty(),
        "premise: it declares streams"
    );
    for (name, declaration) in catalogue.streams() {
        assert_eq!(declaration.policy.replication_factor(), 1, "{name}");
        assert!(!declaration.policy.is_managed(), "{name}");
    }
}

/// FABRIC-8086 step 1. The local catalogue claims no replication, which is
/// right for a broker with no managed log; a fabricd seated on Spanner
/// (ADR 0111) needs a committed catalogue that declares what it delivers,
/// or the managed path has no stream to run and "managed_majority" stays a
/// spelling the tree parses and no file uses. The managed catalogue must
/// differ from the local one in exactly the P0 and P1 replication claim: same
/// stream names and topics, so choosing one is a deployment decision and not
/// a re-binding, and P2 to P4 still RF1 because no mechanism honours more.
///
/// Mutation: set `reflex-journal.local` to `managed_majority` at RF3 in
/// `streams.managed.json` — the parse is refused (ADR 0111 §2) and this
/// fires. Mutation: put `control.local` back to `archived` at RF1 — the
/// managed-count premise fires.
#[test]
fn the_committed_managed_catalogue_claims_rf3_for_p0_and_p1_only_and_binds_what_the_local_one_does()
{
    let read = |name: &str| {
        let path = format!(
            "{}/../../../../infrastructure/event-fabric/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(&path).expect("premise: the committed catalogue is readable");
        Catalogue::parse(&bytes).expect("the committed catalogue parses")
    };
    let local = read("streams.local.json");
    let managed = read("streams.managed.json");
    assert_eq!(
        managed.streams().keys().collect::<Vec<_>>(),
        local.streams().keys().collect::<Vec<_>>(),
        "the two catalogues declare different streams"
    );
    let mut replicated = Vec::new();
    for (name, declaration) in managed.streams() {
        let class = declaration.policy.qos_class();
        let durable = matches!(class, QosClass::P0Control | QosClass::P1Outcomes);
        assert_eq!(declaration.policy.is_managed(), durable, "{name}");
        assert_eq!(
            declaration.policy.replication_factor(),
            if durable { 3 } else { 1 },
            "{name}"
        );
        assert_eq!(
            declaration.topics,
            local.stream(name).expect("same stream").topics,
            "{name} binds different topics"
        );
        if durable {
            replicated.push(name.as_str());
        }
    }
    assert_eq!(
        replicated.len(),
        2,
        "premise: one P0 and one P1 stream are replicated: {replicated:?}"
    );
}

/// FABRIC-7056: a P0 or P1 stream can never be declared sheddable, and only
/// P4 telemetry may shed at all. Before this, `StreamPolicy::new` accepted a
/// P1 outcomes stream declared `sample_or_shed`, and the broker would have
/// been entitled to drop a fill.
///
/// Mutation: make `permits_overload_policy` return `true` — every refusal
/// below is accepted and this fails at the first `expect_err`.
#[test]
fn a_never_dropped_class_cannot_be_declared_sheddable_and_only_telemetry_may_shed() {
    // Premise: each class with its own overload policy is accepted, so each
    // refusal below is the overload policy's doing.
    for class in [
        "p0_control",
        "p1_outcomes",
        "p2_market_journal",
        "p4_telemetry",
    ] {
        let ack = if matches!(class, "p0_control" | "p1_outcomes") {
            "archived"
        } else {
            "leader_only"
        };
        Catalogue::parse(&catalogue_bytes(
            vec![valid_stream_json("s", class, ack, 1, "none", vec![])],
            vec![],
        ))
        .unwrap_or_else(|e| panic!("premise: {class} with its own overload policy loads: {e}"));
    }
    let refused = [
        ("p0_control", "archived", "sample_or_shed"),
        ("p1_outcomes", "archived", "sample_or_shed"),
        ("p1_outcomes", "archived", "throttle_with_gap"),
        ("p1_outcomes", "archived", "allow_backlog"),
        ("p2_market_journal", "leader_only", "sample_or_shed"),
        ("p3_research", "leader_only", "sample_or_shed"),
    ];
    for (class, ack, overload) in refused {
        let mut stream = valid_stream_json("s", class, ack, 1, "none", vec![]);
        stream["overload_policy"] = serde_json::json!(overload);
        let err = Catalogue::parse(&catalogue_bytes(vec![stream], vec![]))
            .expect_err("a class declared with an overload policy it may not have is refused");
        assert!(
            err.to_string().contains("overload policy"),
            "{class}/{overload}: {err}"
        );
    }
}
