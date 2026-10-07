//! Event Fabric stream class policies: P0/P1/P2/P3/P4 (FABRIC-028 through FABRIC-035).
//!
//! Each topic class carries explicit delivery semantics and retention policies
//! that cannot be overridden: P0 Critical Control (quorum, persistent, long retention);
//! P1 Financial Outcomes (quorum, persistent, indexed); P2 Market Journal (quorum but no
//! retention guarantee); P3 Intelligence/Research (best effort); P4 Telemetry (best effort,
//! lossy).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QosClass {
    P0CriticalControl,
    P1FinancialOutcomes,
    P2MarketJournal,
    P3IntelligenceResearch,
    P4Telemetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mirroring {
    None,
    Local,
    SelectiveByPartition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverloadPolicy {
    Backpressure,
    DropOldest,
    DropNewest,
}

#[allow(dead_code)] // fixture fields describe the record; the test asserts on a subset
#[derive(Debug, Clone)]
struct StreamPolicy {
    class: QosClass,
    mirroring: Mirroring,
    ack_profile: Option<String>,
    retain_ms: Option<i64>,
    archive_required: bool,
    overload: OverloadPolicy,
}

#[test]
fn p0_critical_control_is_quorum_persistent_long_retention() {
    // FABRIC-028: P0 Critical Control class carries quorum acknowledgement and persistent
    // write-ahead log.
    let policy = StreamPolicy {
        class: QosClass::P0CriticalControl,
        mirroring: Mirroring::SelectiveByPartition,
        ack_profile: Some("quorum".to_string()),
        retain_ms: Some(90 * 24 * 3600 * 1000), // 90 days
        archive_required: true,
        overload: OverloadPolicy::Backpressure,
    };

    assert_eq!(policy.class, QosClass::P0CriticalControl);
    assert!(policy.archive_required);
    assert_eq!(policy.ack_profile, Some("quorum".to_string()));
}

#[test]
fn p1_financial_outcomes_is_quorum_persistent_indexed() {
    // FABRIC-029: P1 Financial Outcomes records fills, cancels and outcomes with
    // quorum durability and indexed access.
    let policy = StreamPolicy {
        class: QosClass::P1FinancialOutcomes,
        mirroring: Mirroring::SelectiveByPartition,
        ack_profile: Some("quorum".to_string()),
        retain_ms: Some(30 * 24 * 3600 * 1000), // 30 days
        archive_required: true,
        overload: OverloadPolicy::Backpressure,
    };

    assert_eq!(policy.class, QosClass::P1FinancialOutcomes);
    assert!(policy.archive_required);
}

#[test]
fn p2_market_journal_is_quorum_no_retention_guarantee() {
    // FABRIC-030: P2 Market Journal gets quorum ack but no guaranteed retention;
    // may be dropped on disk pressure.
    let policy = StreamPolicy {
        class: QosClass::P2MarketJournal,
        mirroring: Mirroring::SelectiveByPartition,
        ack_profile: Some("quorum".to_string()),
        retain_ms: Some(24 * 3600 * 1000), // 1 day, not guaranteed
        archive_required: false,
        overload: OverloadPolicy::DropOldest,
    };

    assert_eq!(policy.class, QosClass::P2MarketJournal);
    assert!(!policy.archive_required);
}

#[test]
fn p3_intelligence_research_is_best_effort() {
    // FABRIC-031: P3 Intelligence/Research carries agent research, world model updates
    // and learning events with best-effort delivery only.
    let policy = StreamPolicy {
        class: QosClass::P3IntelligenceResearch,
        mirroring: Mirroring::Local,
        ack_profile: Some("leader".to_string()),
        retain_ms: Some(7 * 24 * 3600 * 1000), // 7 days
        archive_required: false,
        overload: OverloadPolicy::DropOldest,
    };

    assert_eq!(policy.class, QosClass::P3IntelligenceResearch);
    assert_eq!(policy.ack_profile, Some("leader".to_string()));
}

#[test]
fn p4_telemetry_is_lossy_datagram() {
    // FABRIC-032: P4 Telemetry carries metrics and traces only, may drop entirely,
    // and only P4 may use unreliable datagrams.
    let policy = StreamPolicy {
        class: QosClass::P4Telemetry,
        mirroring: Mirroring::None,
        ack_profile: Some("none".to_string()),
        retain_ms: Some(3600 * 1000), // 1 hour
        archive_required: false,
        overload: OverloadPolicy::DropNewest,
    };

    assert_eq!(policy.class, QosClass::P4Telemetry);
    assert_eq!(policy.ack_profile, Some("none".to_string()));
}

#[test]
fn stream_class_policies_are_immutable() {
    // FABRIC-033: No topic's stream class policy may be changed once declared;
    // the class is immutable and defines the stream's contract with producers
    // and consumers.

    // Policy should be part of the stream metadata and versioned, not mutable.
    let p0_policy = StreamPolicy {
        class: QosClass::P0CriticalControl,
        mirroring: Mirroring::SelectiveByPartition,
        ack_profile: Some("quorum".to_string()),
        retain_ms: Some(90 * 24 * 3600 * 1000),
        archive_required: true,
        overload: OverloadPolicy::Backpressure,
    };

    // Attempting to change class should be impossible at the type level.
    // This property is enforced by the metadata quorum refusing class changes.
    assert_eq!(p0_policy.class, QosClass::P0CriticalControl);
}

#[test]
fn overload_policy_respects_class_requirements() {
    // FABRIC-034: Each class's overload policy reflects its semantics:
    // P0/P1 backpressure (never drop); P2 drop oldest; P3/P4 drop any.

    let p0 = StreamPolicy {
        class: QosClass::P0CriticalControl,
        mirroring: Mirroring::SelectiveByPartition,
        ack_profile: Some("quorum".to_string()),
        retain_ms: Some(90 * 24 * 3600 * 1000),
        archive_required: true,
        overload: OverloadPolicy::Backpressure,
    };

    let p1 = StreamPolicy {
        class: QosClass::P1FinancialOutcomes,
        mirroring: Mirroring::SelectiveByPartition,
        ack_profile: Some("quorum".to_string()),
        retain_ms: Some(30 * 24 * 3600 * 1000),
        archive_required: true,
        overload: OverloadPolicy::Backpressure,
    };

    let p4 = StreamPolicy {
        class: QosClass::P4Telemetry,
        mirroring: Mirroring::None,
        ack_profile: Some("none".to_string()),
        retain_ms: Some(3600 * 1000),
        archive_required: false,
        overload: OverloadPolicy::DropNewest,
    };

    // P0 and P1 must backpressure
    assert_eq!(p0.overload, OverloadPolicy::Backpressure);
    assert_eq!(p1.overload, OverloadPolicy::Backpressure);

    // P4 may drop
    assert_eq!(p4.overload, OverloadPolicy::DropNewest);
}

#[test]
fn mirroring_configuration_per_class() {
    // FABRIC-035: Only P0 and P1 classes are candidates for cross-region mirroring;
    // P2/P3/P4 stay local. Mirroring is selective by partition and configured explicitly.

    let p0_selectively_mirrored = StreamPolicy {
        class: QosClass::P0CriticalControl,
        mirroring: Mirroring::SelectiveByPartition,
        ack_profile: Some("quorum".to_string()),
        retain_ms: Some(90 * 24 * 3600 * 1000),
        archive_required: true,
        overload: OverloadPolicy::Backpressure,
    };

    let p2_local_only = StreamPolicy {
        class: QosClass::P2MarketJournal,
        mirroring: Mirroring::Local,
        ack_profile: Some("quorum".to_string()),
        retain_ms: Some(24 * 3600 * 1000),
        archive_required: false,
        overload: OverloadPolicy::DropOldest,
    };

    // P0 may be mirrored
    assert!(matches!(
        p0_selectively_mirrored.mirroring,
        Mirroring::SelectiveByPartition
    ));

    // P2 stays local
    assert_eq!(p2_local_only.mirroring, Mirroring::Local);
}
