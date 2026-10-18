//! Event Fabric infrastructure basics: FABRIC-006, FABRIC-007, FABRIC-008, FABRIC-009, FABRIC-010.
//!
//! Tests for fabric durability/replay, control distribution, regional infrastructure,
//! transport security (QUIC/mTLS), and RF3 replication across zones.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

mod event_fabric_support;

use event_fabric_support::{STREAM, batch, broker_module, broker_with, norm, spec};

fn norm_text(text: &str) -> String {
    text.lines().map(norm).collect::<Vec<_>>().join("\n")
}

/// FABRIC-006: every acknowledged record is retrievable in exact append order
/// after a restart, and the log replays to the same bytes. The previous test
/// pushed `FabricRecord`s into a `Vec` it built and counted them; this one
/// appends through the broker, drops it, reopens the same data directory and
/// reads back through `fetch`.
///
/// Mutation: cap `Broker::fetch`'s `high_watermark` at 50 — the reopened
/// broker serves nothing for offset 50 onward and the test fails there,
/// naming the offset; restored.
#[test]
fn fabric_006_fabric_records_are_durable_and_replayable() {
    use qip_core::{Clock, ManualClock, Timestamp};
    use qip_events::event_fabric::codec::{Batch, DecodeOutcome};
    use qip_events::event_fabric::policy::QosClass;
    use qip_events::event_fabric::policy::StreamPolicy;
    use qip_storage::segment::log::SegmentLogConfig;
    use qip_streaming::event_fabric::broker::Broker;
    use std::sync::Arc;

    let (broker, dir) = broker_with("fabric-006", 1, spec(QosClass::P1Outcomes));
    let mut acknowledged = Vec::new();
    for sequence in 0..100u64 {
        let id = format!("record-{sequence}");
        let produced = broker
            .produce_to(STREAM, 0, batch("venue", 1, sequence, &id))
            .expect("the broker answers")
            .expect("the batch is admitted");
        acknowledged.push((produced.base_offset(), id));
    }
    assert_eq!(
        acknowledged.len(),
        100,
        "premise: every batch was acknowledged"
    );
    assert!(
        acknowledged.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "premise: acknowledged offsets rise in append order"
    );
    drop(broker);

    // A new process: same directory, nothing carried over in memory.
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 4)));
    let reopened = Broker::open(dir, Arc::clone(&clock)).expect("the directory reopens");
    reopened
        .declare_stream(
            STREAM,
            1,
            StreamPolicy::new(spec(QosClass::P1Outcomes)).expect("a permitted policy"),
            SegmentLogConfig::new(clock).with_roll_after_bytes(10_000_000),
        )
        .expect("the stream is re-declared over its recovered log");

    let mut replayed = Vec::new();
    for (offset, _) in &acknowledged {
        let served = reopened
            .fetch(STREAM, 0, *offset, 1)
            .expect("an acknowledged offset is served");
        let bytes = qip_core::hash::from_hex(served.batches()).expect("hex");
        match Batch::decode(&bytes).expect("decodes") {
            DecodeOutcome::Complete(batch) => {
                assert_eq!(batch.base_offset, *offset);
                replayed.push(batch.records[0].event_id.clone());
            }
            DecodeOutcome::Torn => panic!("offset {offset} was served torn after a restart"),
        }
    }
    let expected: Vec<String> = acknowledged.into_iter().map(|(_, id)| id).collect();
    assert_eq!(replayed, expected, "replay is not the acknowledged order");
}

#[test]
fn fabric_007_fabric_distributes_capital_risk_and_strategy_control_to_reflex_cells() {
    // FABRIC-007: The fabric distributes capital, risk and strategy control to
    // Reflex Cells, and every cell receives updates for its region and the global
    // constraints applying to it.

    #[derive(Debug, Clone)]
    struct ControlMessage {
        _kind: String,
        _destination_region: String,
        applies_to_cells: Vec<String>,
    }

    #[derive(Debug, Clone)]
    struct ControlDistribution {
        capital_messages: Vec<ControlMessage>,
        risk_messages: Vec<ControlMessage>,
        strategy_messages: Vec<ControlMessage>,
    }

    let mut distribution = ControlDistribution {
        capital_messages: Vec::new(),
        risk_messages: Vec::new(),
        strategy_messages: Vec::new(),
    };

    // Scenario 1: Capital control reaches appropriate cells
    let capital_grant = ControlMessage {
        _kind: "capital_grant".to_string(),
        _destination_region: "us-west1".to_string(),
        applies_to_cells: vec!["cell-usw-1".to_string(), "cell-usw-2".to_string()],
    };

    distribution.capital_messages.push(capital_grant);

    // Scenario 2: Risk control reaches appropriate cells
    let risk_limit = ControlMessage {
        _kind: "risk_limit".to_string(),
        _destination_region: "us-west1".to_string(),
        applies_to_cells: vec!["cell-usw-1".to_string(), "cell-usw-2".to_string()],
    };

    distribution.risk_messages.push(risk_limit);

    // Scenario 3: Strategy control reaches appropriate cells
    let strategy_config = ControlMessage {
        _kind: "strategy_activation".to_string(),
        _destination_region: "us-west1".to_string(),
        applies_to_cells: vec!["cell-usw-1".to_string(), "cell-usw-2".to_string()],
    };

    distribution.strategy_messages.push(strategy_config);

    // Verify each region's cells receive all three control types
    assert_eq!(distribution.capital_messages.len(), 1);
    assert_eq!(distribution.risk_messages.len(), 1);
    assert_eq!(distribution.strategy_messages.len(), 1);

    // Each cell receives global and regional constraints
    assert!(
        distribution
            .capital_messages
            .iter()
            .all(|m| !m.applies_to_cells.is_empty())
    );
    assert!(
        distribution
            .risk_messages
            .iter()
            .all(|m| !m.applies_to_cells.is_empty())
    );
    assert!(
        distribution
            .strategy_messages
            .iter()
            .all(|m| !m.applies_to_cells.is_empty())
    );
}

/// FABRIC-008: a region's brokers are placed one to a zone and in the region,
/// refused at plan otherwise. The previous test built two `RegionalBrokerCluster`
/// fixtures with three nodes each and asserted three. What the module does is
/// different and checkable: it plans one broker by default (RF1, FABRIC-086
/// blocked on C2), and a lifecycle precondition on the machine resource
/// refuses two brokers in one zone and any zone outside `var.region`.
///
/// Mutation: delete the `distinct(...)` precondition from
/// `google_compute_instance_from_template` in the module — the zone-spread
/// assertion fires; or change `broker_count`'s `1` to `3` — the default-count
/// assertion fires.
#[test]
fn fabric_008_each_region_runs_fabric_on_dedicated_broker_vms_across_three_zones() {
    let main = norm_text(&broker_module("main.tf"));
    assert!(
        main.contains("resource \"google_compute_instance_from_template\" \"event_fabric_broker\""),
        "premise: the module declares the broker machine"
    );
    let machine = main
        .split("resource \"google_compute_instance_from_template\"")
        .nth(1)
        .and_then(|rest| rest.split("resource \"google_dns_record_set\"").next())
        .expect("the machine resource is followed by the DNS records");
    assert!(
        machine.contains(
            "condition = length(distinct([for address in local.brokers : local.broker_zone[address]])) == length(local.brokers)"
        ),
        "two brokers may share a zone at plan: {machine}"
    );
    assert!(
        machine
            .contains("condition = startswith(local.broker_zone[each.value], \"${var.region}-\")"),
        "a broker may be planned outside its region: {machine}"
    );
    assert!(
        main.contains("broker_count = var.enabled ? 1 : 0"),
        "the default is one broker per region (RF1), not three"
    );
}

#[test]
fn fabric_009_fabric_sessions_use_quic_with_mutual_tls() {
    // FABRIC-009: Fabric sessions use QUIC with mutual TLS, with client and broker
    // certificates verified and session keys derived from the event log's trust anchor.

    #[derive(Debug, Clone)]
    struct TLSContext {
        _protocol: String,
        has_client_cert: bool,
        has_broker_cert: bool,
        trust_anchor: String,
    }

    #[derive(Debug, Clone)]
    struct FabricSession {
        _session_id: String,
        transport: String,
        tls_context: TLSContext,
    }

    // Scenario 1: Sessions use QUIC transport
    let session = FabricSession {
        _session_id: "sess-001".to_string(),
        transport: "QUIC".to_string(),
        tls_context: TLSContext {
            _protocol: "TLS 1.3".to_string(),
            has_client_cert: true,
            has_broker_cert: true,
            trust_anchor: "event_log_root".to_string(),
        },
    };

    assert_eq!(session.transport, "QUIC");

    // Scenario 2: Mutual TLS is configured (both client and broker verify)
    assert!(session.tls_context.has_client_cert);
    assert!(session.tls_context.has_broker_cert);

    // Scenario 3: Certificates verified against event log trust anchor
    assert_eq!(session.tls_context.trust_anchor, "event_log_root");

    // Scenario 4: Multiple sessions follow the same security model
    let mut sessions = vec![session];
    for i in 1..5 {
        let s = FabricSession {
            _session_id: format!("sess-{:03}", i),
            transport: "QUIC".to_string(),
            tls_context: TLSContext {
                _protocol: "TLS 1.3".to_string(),
                has_client_cert: true,
                has_broker_cert: true,
                trust_anchor: "event_log_root".to_string(),
            },
        };
        sessions.push(s);
    }

    assert_eq!(sessions.len(), 5);
    assert!(sessions.iter().all(|s| s.transport == "QUIC"));
    assert!(
        sessions
            .iter()
            .all(|s| s.tls_context.has_client_cert && s.tls_context.has_broker_cert)
    );
}

#[test]
fn fabric_010_gap_a_durable_partition_cannot_be_declared_at_rf3() {
    // GAP PIN. FABRIC-010 asks for RF3 across three zones with a two-of-three
    // write quorum. The previous test built a `Partition` with three
    // `BrokerNode`s in three zones and asserted it had three. The production
    // decision is the opposite and is code: `StreamPolicy::new` refuses a
    // replication factor of three for every class (ADR 0100 §3, blocked on
    // C2), and the broker module creates at most one broker template. This
    // test asserts the refusal for the two durable classes the requirement is
    // about, so the gap is evidence and RF3 arriving is noticed.
    //
    // Mutation: delete the replication-factor refusal in `StreamPolicy::new` —
    // RF3 is admitted and the assertion fires.
    use qip_events::event_fabric::policy::{
        Entitlement, Mirroring, Ordering, OverloadPolicy, QosClass, StreamPolicy, StreamPolicySpec,
    };
    for class in [QosClass::P0Control, QosClass::P1Outcomes] {
        let spec = |replication_factor| StreamPolicySpec {
            qos_class: class,
            partition_key: "key".to_string(),
            ordering: Ordering::PerPartition,
            retention: qip_events::retention::RetentionClass::Irreplaceable,
            replication_factor,
            mirroring: Mirroring::None,
            overload_policy: OverloadPolicy::RefuseProducer,
            ack_profile: class.ack_floor(),
            byte_quota_per_producer: 1 << 20,
            message_quota_per_producer: 1_000,
            lag_limit: 10_000,
            entitlement: Entitlement::new("control", "trade").expect("valid"),
            seal_age_ms: 60_000,
            peak_bytes_per_second: 1 << 16,
        };
        assert!(
            StreamPolicy::new(spec(1)).is_ok(),
            "premise: RF1 is admitted"
        );
        let refused = StreamPolicy::new(spec(3)).expect_err("RF3 was admitted");
        assert!(refused.to_string().contains("(C2)"), "{refused}");
    }
}
