//! Event Fabric stream classes P0–P4 (FABRIC-028 through FABRIC-035), driven
//! through `qip_events::event_fabric::policy` — the one place a stream's class
//! contract is decided.
//!
//! The previous version of this file declared its own `QosClass`,
//! `Mirroring`, `OverloadPolicy` and `StreamPolicy` types, filled them with the
//! values the requirement names, and asserted those values back. It imported
//! nothing from the platform, so it passed with `policy.rs` deleted. Every
//! test here calls production code, and two are pins of production gaps,
//! named as such.
//!
//! The acknowledgement profiles are compared through `QosClass::ack_floor`
//! and `AckProfile`'s order rather than by naming the strongest variant, so a
//! rename of that variant (W0.6) does not touch these tests.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_events::event_fabric::policy::{
    AckProfile, Entitlement, Mirroring, Ordering, OverloadPolicy, QosClass, StreamPolicy,
    StreamPolicySpec,
};
use qip_events::retention::RetentionClass;
use std::collections::BTreeSet;

fn spec(class: QosClass) -> StreamPolicySpec {
    StreamPolicySpec {
        qos_class: class,
        partition_key: "instrument".to_string(),
        ordering: Ordering::PerPartition,
        retention: RetentionClass::Irreplaceable,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: class.ack_floor(),
        byte_quota_per_producer: 1 << 20,
        message_quota_per_producer: 1_000,
        lag_limit: 10_000,
        entitlement: Entitlement::new("market-data", "research").expect("a valid entitlement"),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 16,
    }
}

const ALL_ACKS: [AckProfile; 3] = [
    AckProfile::None,
    AckProfile::LeaderOnly,
    AckProfile::Archived,
];

/// FABRIC-028/029/067. Every class refuses every acknowledgement weaker than
/// its floor and admits its floor; P0 and P1 therefore admit only the
/// strongest profile.
///
/// Mutation: delete the `if spec.ack_profile < floor` refusal in
/// `StreamPolicy::new` — a P0 policy declaring `AckProfile::None` is admitted
/// and the refusal assertion fires.
#[test]
fn every_class_refuses_an_acknowledgement_weaker_than_its_floor_and_admits_the_floor() {
    for class in QosClass::ALL {
        let floor = class.ack_floor();
        // Premise: the floor itself is admitted, so the refusals below are
        // about weakness and not a constructor that refuses everything.
        assert!(
            StreamPolicy::new(spec(class)).is_ok(),
            "{} floor refused",
            class.as_str()
        );
        for ack in ALL_ACKS {
            let result = StreamPolicy::new(StreamPolicySpec {
                ack_profile: ack,
                ..spec(class)
            });
            if ack < floor {
                let error = result.expect_err("a weaker acknowledgement was admitted");
                assert!(
                    error.to_string().contains(class.as_str()),
                    "the refusal names the class: {error}"
                );
            } else {
                assert!(result.is_ok(), "{} refused {ack:?}", class.as_str());
            }
        }
    }
    // P0 and P1 admit exactly one profile: the strongest there is.
    for class in [QosClass::P0Control, QosClass::P1Outcomes] {
        assert_eq!(
            class.ack_floor(),
            *ALL_ACKS.iter().max().expect("non-empty")
        );
    }
}

/// FABRIC-030/031/032. The floor weakens in class order, P0 = P1 > P2 = P3 >
/// P4, and only telemetry may run with no acknowledgement at all.
///
/// Mutation: change `QosClass::ack_floor`'s P4 arm to `LeaderOnly`, or its
/// P2/P3 arm to `None` — the matching equality fires.
#[test]
fn the_acknowledgement_floor_weakens_in_class_order_and_only_telemetry_may_have_none() {
    let floors: Vec<AckProfile> = QosClass::ALL.iter().map(QosClass::ack_floor).collect();
    assert_eq!(floors.len(), 5, "premise: five classes");
    assert!(
        floors.windows(2).all(|pair| pair[0] >= pair[1]),
        "{floors:?}"
    );
    assert_eq!(floors[0], floors[1], "P0 and P1 share the floor");
    assert!(floors[1] > floors[2], "P2 is weaker than P1");
    assert_eq!(floors[2], AckProfile::LeaderOnly);
    assert_eq!(floors[3], AckProfile::LeaderOnly);
    assert_eq!(floors[4], AckProfile::None);
    let without: Vec<&str> = QosClass::ALL
        .iter()
        .filter(|class| class.ack_floor() == AckProfile::None)
        .map(QosClass::as_str)
        .collect();
    assert_eq!(without, ["p4_telemetry"]);
}

/// FABRIC-031/032/035. A lossy-tolerable topic (raw ticks) may reach only a
/// research or telemetry stream.
///
/// Mutation: add `Self::P2MarketJournal` to
/// `QosClass::admits_lossy_tolerable_topics` — the set equality fires.
#[test]
fn only_research_and_telemetry_streams_admit_lossy_tolerable_topics() {
    let admitting: BTreeSet<&str> = QosClass::ALL
        .iter()
        .filter(|class| class.admits_lossy_tolerable_topics())
        .map(QosClass::as_str)
        .collect();
    assert_eq!(admitting, BTreeSet::from(["p3_research", "p4_telemetry"]));
}

/// Each class stamps its own batch schema id; two classes sharing one would
/// let the broker file a P4 batch under a P0 stream's registration.
///
/// Mutation: make `batch_schema_id`'s P4 arm return `3` — the distinctness
/// assertion fires.
#[test]
fn every_class_stamps_a_distinct_batch_schema_id() {
    let ids: BTreeSet<u32> = QosClass::ALL
        .iter()
        .map(QosClass::batch_schema_id)
        .collect();
    assert_eq!(ids.len(), QosClass::ALL.len());
}

/// FABRIC-033: a declared policy cannot be changed. The type has private
/// fields and the only constructor validates; what would break that is a
/// `&mut self` method on `StreamPolicy`, so the test reads `policy.rs`'s
/// production text and asserts the `impl StreamPolicy` block has none.
///
/// Mutation: add `pub fn set_ack_profile(&mut self, ack: AckProfile)` to
/// `impl StreamPolicy` — the scan finds it and fires.
#[test]
fn a_stream_policy_offers_no_way_to_change_a_declared_value() {
    let source = qip_acceptance::production_text(&qip_acceptance::read(
        "backend/crates/libs/qip-events/src/event_fabric/policy.rs",
    ));
    let start = source
        .find("impl StreamPolicy {")
        .expect("premise: the impl block exists in production text");
    // The block ends at the first line that is exactly a closing brace.
    let block: Vec<&str> = source[start..]
        .lines()
        .take_while(|line| *line != "}")
        .collect();
    assert!(
        block
            .iter()
            .any(|line| line.contains("pub fn qos_class(&self)")),
        "premise: the scan reached the accessors"
    );
    let mutators: Vec<&&str> = block
        .iter()
        .filter(|line| line.contains("&mut self"))
        .collect();
    assert!(
        mutators.is_empty(),
        "a declared policy can be changed: {mutators:?}"
    );
    // And the built value reports exactly what was declared.
    let built = StreamPolicy::new(spec(QosClass::P1Outcomes)).expect("admitted");
    assert_eq!(built.qos_class(), QosClass::P1Outcomes);
    assert_eq!(built.overload_policy(), OverloadPolicy::RefuseProducer);
}

/// FABRIC-034: mirroring is refused by name until fabric-mirror (C8) is
/// built; only `None` is admitted.
///
/// Mutation: delete the `spec.mirroring != Mirroring::None` refusal —
/// `Selective` is admitted and the assertion fires.
#[test]
fn every_mirroring_policy_but_none_is_refused_until_the_mirror_exists() {
    assert!(
        StreamPolicy::new(spec(QosClass::P2MarketJournal)).is_ok(),
        "premise"
    );
    for mirroring in [Mirroring::Selective, Mirroring::Full] {
        let error = StreamPolicy::new(StreamPolicySpec {
            mirroring,
            ..spec(QosClass::P2MarketJournal)
        })
        .expect_err("a mirror that does not exist was admitted");
        assert!(error.to_string().contains("(C8)"), "{error}");
    }
}

/// Whether a stream must reach the archive before its spool may release it is
/// decided by its retention class, not its QoS class.
///
/// Mutation: move `Self::Transient` into the `true` arm of
/// `RetentionClass::archive_required` — the second assertion fires.
#[test]
fn archive_requirement_follows_the_retention_class() {
    let irreplaceable = StreamPolicy::new(spec(QosClass::P4Telemetry)).expect("admitted");
    assert!(irreplaceable.archive_required());
    let transient = StreamPolicy::new(StreamPolicySpec {
        retention: RetentionClass::Transient,
        ..spec(QosClass::P0Control)
    })
    .expect("admitted");
    assert!(!transient.archive_required());
}

/// GAP PIN (FABRIC-049/050). ADR 0100 §5 says P0 and P1 are never dropped,
/// and `OverloadPolicy` documents `RefuseProducer` as theirs — but
/// `StreamPolicy::new` does not check the declared overload policy against
/// the class. A P0 control stream declaring `SampleOrShed` is admitted today.
/// This test asserts that, so the gap is evidence rather than a sentence.
///
/// Mutation that makes it fail (the intended one): add a refusal in
/// `StreamPolicy::new` for a P0/P1 stream whose overload policy is not
/// `RefuseProducer`. When that lands, invert this test.
#[test]
fn gap_a_p0_control_stream_declaring_shed_first_overload_is_admitted() {
    let shed = StreamPolicy::new(StreamPolicySpec {
        overload_policy: OverloadPolicy::SampleOrShed,
        ..spec(QosClass::P0Control)
    })
    .expect("today the constructor does not check overload against class");
    assert_eq!(shed.qos_class(), QosClass::P0Control);
    assert_eq!(shed.overload_policy(), OverloadPolicy::SampleOrShed);
}
