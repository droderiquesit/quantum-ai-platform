use qip_contracts::{
    AckPolicy, Entitlement, MirroringPolicy, OverloadBehavior, QoSClass, StreamPolicy, Usage,
};
use qip_core::Timestamp;
use std::time::Duration;

#[test]
fn a_streampolicy_with_all_required_fields_validates_and_missing_any_field_is_refused() {
    // Create a valid policy with all 10 required fields populated
    let expires = Timestamp::from_secs(2_000_000);
    let entitlement = Entitlement::Granted {
        dataset: "market-data".to_string(),
        usage: Usage::Research,
        expires_at: expires,
    };

    let policy = StreamPolicy::new(
        "partition-key",
        3,
        AckPolicy::Quorum,
        Duration::from_secs(3600),
        QoSClass::P0CriticalControl,
        1024 * 1024 * 1024,
        1_000_000,
        5000,
        MirroringPolicy::AsynchronousAll,
        OverloadBehavior::Block,
    )
    .with_entitlement(entitlement);

    // Assertion: complete policy validates successfully
    assert!(policy.validate().is_ok());

    // Assertion: partition key is required and cannot be empty
    let invalid_partition = StreamPolicy::new(
        "   ",
        3,
        AckPolicy::Quorum,
        Duration::from_secs(3600),
        QoSClass::P0CriticalControl,
        1024 * 1024 * 1024,
        1_000_000,
        5000,
        MirroringPolicy::AsynchronousAll,
        OverloadBehavior::Block,
    );
    let result = invalid_partition.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("partition key"));

    // Assertion: replication factor must be greater than zero
    let invalid_replication = StreamPolicy::new(
        "partition-key",
        0,
        AckPolicy::Quorum,
        Duration::from_secs(3600),
        QoSClass::P0CriticalControl,
        1024 * 1024 * 1024,
        1_000_000,
        5000,
        MirroringPolicy::AsynchronousAll,
        OverloadBehavior::Block,
    );
    let result = invalid_replication.validate();
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("replication factor")
    );

    // Assertion: retention must be greater than zero
    let invalid_retention = StreamPolicy::new(
        "partition-key",
        3,
        AckPolicy::Quorum,
        Duration::ZERO,
        QoSClass::P0CriticalControl,
        1024 * 1024 * 1024,
        1_000_000,
        5000,
        MirroringPolicy::AsynchronousAll,
        OverloadBehavior::Block,
    );
    let result = invalid_retention.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("retention"));

    // Assertion: byte limit must be greater than zero
    let invalid_byte_limit = StreamPolicy::new(
        "partition-key",
        3,
        AckPolicy::Quorum,
        Duration::from_secs(3600),
        QoSClass::P0CriticalControl,
        0,
        1_000_000,
        5000,
        MirroringPolicy::AsynchronousAll,
        OverloadBehavior::Block,
    );
    let result = invalid_byte_limit.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("byte limit"));

    // Assertion: message limit must be greater than zero
    let invalid_message_limit = StreamPolicy::new(
        "partition-key",
        3,
        AckPolicy::Quorum,
        Duration::from_secs(3600),
        QoSClass::P0CriticalControl,
        1024 * 1024 * 1024,
        0,
        5000,
        MirroringPolicy::AsynchronousAll,
        OverloadBehavior::Block,
    );
    let result = invalid_message_limit.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("message limit"));

    // Assertion: consumer lag threshold must be greater than zero
    let invalid_lag_threshold = StreamPolicy::new(
        "partition-key",
        3,
        AckPolicy::Quorum,
        Duration::from_secs(3600),
        QoSClass::P0CriticalControl,
        1024 * 1024 * 1024,
        1_000_000,
        0,
        MirroringPolicy::AsynchronousAll,
        OverloadBehavior::Block,
    );
    let result = invalid_lag_threshold.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("consumer lag"));
}

#[test]
fn builder_methods_allow_incremental_policy_construction() {
    let policy = StreamPolicy::new(
        "orders",
        3,
        AckPolicy::Quorum,
        Duration::from_secs(86400),
        QoSClass::P0CriticalControl,
        10_000_000,
        100_000,
        10000,
        MirroringPolicy::None,
        OverloadBehavior::Refuse,
    )
    .with_partition_key("updated-key")
    .with_replication_factor(2)
    .with_ack_policy(AckPolicy::Leader)
    .with_retention(Duration::from_secs(3600))
    .with_durability_qos_class(QoSClass::P1FinancialOutcomes)
    .with_byte_limit(1024 * 1024)
    .with_message_limit(10_000)
    .with_consumer_lag_threshold_ms(1000)
    .with_mirroring_policy(MirroringPolicy::AsynchronousSubset)
    .with_overload_behavior(OverloadBehavior::DropOldest);

    // Verify all updates took effect
    assert_eq!(policy.partition_key, "updated-key");
    assert_eq!(policy.replication_factor, 2);
    assert_eq!(policy.ack_policy, AckPolicy::Leader);
    assert_eq!(policy.retention, Duration::from_secs(3600));
    assert_eq!(policy.durability_qos_class, QoSClass::P1FinancialOutcomes);
    assert_eq!(policy.byte_limit, 1024 * 1024);
    assert_eq!(policy.message_limit, 10_000);
    assert_eq!(policy.consumer_lag_threshold_ms, 1000);
    assert_eq!(policy.mirroring_policy, MirroringPolicy::AsynchronousSubset);
    assert_eq!(policy.overload_behavior, OverloadBehavior::DropOldest);
    assert!(policy.validate().is_ok());
}

#[test]
fn control_plane_streams_use_quorum_ack_and_telemetry_streams_vary_by_qos() {
    // Control-plane stream: P0 critical control requires quorum
    let control_policy = StreamPolicy::new(
        "control",
        3,
        AckPolicy::Quorum,
        Duration::from_secs(86400),
        QoSClass::P0CriticalControl,
        100_000_000,
        1_000_000,
        30000,
        MirroringPolicy::AsynchronousAll,
        OverloadBehavior::Block,
    );
    assert_eq!(control_policy.ack_policy, AckPolicy::Quorum);
    assert_eq!(
        control_policy.durability_qos_class,
        QoSClass::P0CriticalControl
    );
    assert!(control_policy.validate().is_ok());

    // Telemetry stream: P4 lossy telemetry sheds records
    let telemetry_policy = StreamPolicy::new(
        "telemetry",
        1,
        AckPolicy::None,
        Duration::from_secs(3600),
        QoSClass::P4LossyTelemetry,
        10_000_000,
        100_000,
        60000,
        MirroringPolicy::None,
        OverloadBehavior::DropNewest,
    );
    assert_eq!(telemetry_policy.ack_policy, AckPolicy::None);
    assert_eq!(
        telemetry_policy.durability_qos_class,
        QoSClass::P4LossyTelemetry
    );
    assert_eq!(
        telemetry_policy.overload_behavior,
        OverloadBehavior::DropNewest
    );
    assert!(telemetry_policy.validate().is_ok());

    // Financial outcomes stream: P1 requires quorum and long retention
    let outcome_policy = StreamPolicy::new(
        "outcomes",
        3,
        AckPolicy::Quorum,
        Duration::from_secs(86400 * 30),
        QoSClass::P1FinancialOutcomes,
        50_000_000,
        500_000,
        15000,
        MirroringPolicy::AsynchronousAll,
        OverloadBehavior::Block,
    );
    assert!(outcome_policy.validate().is_ok());
}

#[test]
fn mirroring_policies_are_enforced() {
    let no_mirror = StreamPolicy::new(
        "local-only",
        1,
        AckPolicy::Leader,
        Duration::from_secs(3600),
        QoSClass::P4LossyTelemetry,
        5_000_000,
        50_000,
        5000,
        MirroringPolicy::None,
        OverloadBehavior::Block,
    );
    assert_eq!(no_mirror.mirroring_policy, MirroringPolicy::None);

    let all_regions = StreamPolicy::new(
        "global",
        3,
        AckPolicy::Quorum,
        Duration::from_secs(86400),
        QoSClass::P1FinancialOutcomes,
        100_000_000,
        1_000_000,
        10000,
        MirroringPolicy::AsynchronousAll,
        OverloadBehavior::Block,
    );
    assert_eq!(
        all_regions.mirroring_policy,
        MirroringPolicy::AsynchronousAll
    );

    let subset_regions = StreamPolicy::new(
        "regional",
        3,
        AckPolicy::Quorum,
        Duration::from_secs(86400),
        QoSClass::P2MarketJournal,
        100_000_000,
        1_000_000,
        10000,
        MirroringPolicy::AsynchronousSubset,
        OverloadBehavior::Block,
    );
    assert_eq!(
        subset_regions.mirroring_policy,
        MirroringPolicy::AsynchronousSubset
    );
}
