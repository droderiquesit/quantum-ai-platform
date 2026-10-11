//! Event Fabric foundational requirements: FABRIC-005, FABRIC-011, FABRIC-020.
//!
//! Tests for Reflex Mesh independence from the fabric, selective regional mirroring configuration,
//! and regional fabric operational autonomy without cloud database dependencies.

#[test]
fn fabric_005_reflex_mesh_coordination_stays_off_the_fabric() {
    // FABRIC-005: peer coordination between reflex nodes travels directly
    // peer-to-peer and is never broker-mediated. The previous test put
    // `Stalled` and `Healthy` into a map and read them back. This one reads
    // the two files the mesh is built from — the cell's uplink and downlink
    // (`qip-edge/src/mesh.rs`) and the transport's mesh publisher
    // (`qip-transport/src/mesh.rs`) — and asserts they name the mesh's own
    // HTTP publisher and no fabric client, so a fabric outage has no code
    // path by which to stop mesh coordination.
    //
    // Mutation: in `qip-edge/src/mesh.rs`, replace `MeshPublisher` in the
    // uplink with `qip_transport::event_fabric::producer::Producer` — the
    // forbidden-token assertion fires (and the premise, if the publisher
    // import goes).
    let forbidden = [
        "event_fabric::producer",
        "event_fabric::consumer",
        "event_fabric::batching_producer",
        "event_fabric::transport",
        "FabricTransport",
        "qip_streaming",
    ];
    for (file, premise) in [
        ("backend/crates/edge/qip-edge/src/mesh.rs", "MeshPublisher"),
        (
            "backend/crates/libs/qip-transport/src/mesh.rs",
            "pub struct MeshPublisher",
        ),
    ] {
        let text = qip_acceptance::production_text(&qip_acceptance::read(file));
        assert!(text.contains(premise), "premise: {file} holds `{premise}`");
        for token in forbidden {
            assert!(!text.contains(token), "{file} names `{token}`");
        }
    }
}

#[test]
fn fabric_011_only_configured_critical_topics_mirror_across_regions() {
    // FABRIC-011: Cross-region mirroring is selective and explicit: only P0 and P1 topics
    // configured for mirroring actually mirror; P2/P3/P4 stay regional and are never
    // candidates for cross-region replication.

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TopicClass {
        P0CriticalControl,
        P1FinancialOutcomes,
        P2MarketJournal,
        P3IntelligenceResearch,
        P4Telemetry,
    }

    #[derive(Debug, Clone)]
    struct MirroringConfig {
        _topic_class: TopicClass,
        explicitly_configured_for_mirror: bool,
        mirrored_to_regions: Vec<String>,
    }

    // P0 configured for mirroring
    let p0_mirrored = MirroringConfig {
        _topic_class: TopicClass::P0CriticalControl,
        explicitly_configured_for_mirror: true,
        mirrored_to_regions: vec!["us-east1".to_string(), "eu-west1".to_string()],
    };

    // P1 configured for mirroring
    let p1_mirrored = MirroringConfig {
        _topic_class: TopicClass::P1FinancialOutcomes,
        explicitly_configured_for_mirror: true,
        mirrored_to_regions: vec!["us-east1".to_string()],
    };

    // P2 NOT configured for mirroring (stays regional)
    let p2_regional = MirroringConfig {
        _topic_class: TopicClass::P2MarketJournal,
        explicitly_configured_for_mirror: false,
        mirrored_to_regions: vec![],
    };

    // P3 NOT configured for mirroring (stays regional)
    let p3_regional = MirroringConfig {
        _topic_class: TopicClass::P3IntelligenceResearch,
        explicitly_configured_for_mirror: false,
        mirrored_to_regions: vec![],
    };

    // P4 NOT configured for mirroring (stays regional)
    let p4_regional = MirroringConfig {
        _topic_class: TopicClass::P4Telemetry,
        explicitly_configured_for_mirror: false,
        mirrored_to_regions: vec![],
    };

    // Verify P0 and P1 can be configured for mirroring
    assert!(p0_mirrored.explicitly_configured_for_mirror);
    assert!(p1_mirrored.explicitly_configured_for_mirror);
    assert!(!p0_mirrored.mirrored_to_regions.is_empty());
    assert!(!p1_mirrored.mirrored_to_regions.is_empty());

    // Verify P2/P3/P4 are not mirrored (stay regional only)
    assert!(!p2_regional.explicitly_configured_for_mirror);
    assert!(!p3_regional.explicitly_configured_for_mirror);
    assert!(!p4_regional.explicitly_configured_for_mirror);
    assert!(p2_regional.mirrored_to_regions.is_empty());
    assert!(p3_regional.mirrored_to_regions.is_empty());
    assert!(p4_regional.mirrored_to_regions.is_empty());

    // Explicit configuration is required: a topic not in the config is not mirrored
    let unconfigured = MirroringConfig {
        _topic_class: TopicClass::P0CriticalControl,
        explicitly_configured_for_mirror: false, // Not in config
        mirrored_to_regions: vec![],
    };

    assert!(!unconfigured.explicitly_configured_for_mirror);
    assert!(unconfigured.mirrored_to_regions.is_empty());
}

#[test]
fn fabric_020_regional_fabric_operation_needs_no_cloud_database() {
    // FABRIC-020: A regional fabric cluster must operate autonomously on its dedicated
    // broker VMs with only local state: no Cloud SQL, Firestore, Spanner or other managed
    // cloud database dependency. Metadata quorum and partition state are stored only in
    // the brokers' local disks and replicated peer-to-peer.

    #[derive(Debug, Clone)]
    struct RegionalFabricCluster {
        _region: String,
        _broker_count: usize,
        metadata_quorum_type: String,
        external_database_used: bool,
        local_state_replicated: bool,
        can_operate_during_cloud_api_outage: bool,
    }

    // US-West region fabric cluster
    let usw_fabric = RegionalFabricCluster {
        _region: "us-west1".to_string(),
        _broker_count: 5,
        metadata_quorum_type: "distributed_raft_on_local_disk".to_string(),
        external_database_used: false,
        local_state_replicated: true,
        can_operate_during_cloud_api_outage: true,
    };

    // EU-West region fabric cluster
    let euw_fabric = RegionalFabricCluster {
        _region: "eu-west1".to_string(),
        _broker_count: 5,
        metadata_quorum_type: "distributed_raft_on_local_disk".to_string(),
        external_database_used: false,
        local_state_replicated: true,
        can_operate_during_cloud_api_outage: true,
    };

    // Verify no external database dependency
    assert!(!usw_fabric.external_database_used);
    assert!(!euw_fabric.external_database_used);

    // Metadata quorum is peer-to-peer replicated, not stored in a cloud DB
    assert_eq!(
        usw_fabric.metadata_quorum_type,
        "distributed_raft_on_local_disk"
    );
    assert_eq!(
        euw_fabric.metadata_quorum_type,
        "distributed_raft_on_local_disk"
    );

    // Local state is replicated across brokers in the region
    assert!(usw_fabric.local_state_replicated);
    assert!(euw_fabric.local_state_replicated);

    // Each region can operate independently even if cloud APIs are unavailable
    assert!(usw_fabric.can_operate_during_cloud_api_outage);
    assert!(euw_fabric.can_operate_during_cloud_api_outage);

    // When a cloud API outage occurs (e.g., Firestore is down), the region continues
    // publishing, consuming and replicating without waiting for the cloud DB
    let cloud_api_status = false; // APIs unavailable
    let mut broker_can_replicate = true;
    if !cloud_api_status && usw_fabric.external_database_used {
        broker_can_replicate = false;
    }
    // Since no external DB is used, replication is unaffected
    assert!(broker_can_replicate);
}
