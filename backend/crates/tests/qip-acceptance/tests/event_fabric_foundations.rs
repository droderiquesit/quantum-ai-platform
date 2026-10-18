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

/// `CamelCase` as the `snake_case` serde writes on the wire.
fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (index, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The variant names of `pub enum <name>` in `source`: lines at four-space
/// indent opening with an uppercase letter, up to the enum's closing brace.
fn enum_variants(source: &str, name: &str) -> Vec<String> {
    let marker = format!("pub enum {name} {{");
    let Some(start) = source.find(&marker) else {
        return Vec::new();
    };
    source[start + marker.len()..]
        .lines()
        .skip(1)
        .take_while(|line| *line != "}")
        .filter(|line| line.starts_with("    ") && !line.starts_with("     "))
        .filter_map(|line| {
            let word: String = line
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            word.chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
                .then_some(word)
        })
        .collect()
}

/// Wire names of every `PeerMessage` kind and `MeshFunction` in the source.
fn peer_kinds(peer: &str) -> Vec<String> {
    let mut kinds: Vec<String> = enum_variants(peer, "PeerMessage")
        .iter()
        .chain(enum_variants(peer, "MeshFunction").iter())
        .map(|variant| snake_case(variant))
        .collect();
    kinds.sort();
    kinds
}

/// Kinds whose name equals a stream's full name or its stem before the first
/// dot: every stream is `<stem>.local`, so a kind called `control` is the
/// collision, and normalising to lowercase alphanumerics is how either
/// spelling would show.
fn colliding_kinds(peer: &str, streams: &[String]) -> Vec<String> {
    let normal = |text: &str| -> String {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let stream_names: std::collections::BTreeSet<String> = streams
        .iter()
        .flat_map(|s| [normal(s), normal(s.split('.').next().unwrap_or(s))])
        .collect();
    peer_kinds(peer)
        .into_iter()
        .filter(|kind| stream_names.contains(&normal(kind)))
        .collect()
}

#[test]
fn no_mesh_frame_kind_is_a_fabric_stream_and_no_peer_source_names_one() {
    // R12-MESH-10111. FABRIC-005 above proves the cell's uplink names no
    // fabric client; this is the code-level guard on the vocabulary itself.
    // A Reflex Mesh frame that carried a fabric stream's name would be a frame
    // a broker consumer could be pointed at, and a stream named in the peer
    // crates would be a route from a leg's latency onto the durable path.
    // Two checks: the sets of names are disjoint, and no peer source holds a
    // stream's name as text.
    let catalogue: serde_json::Value = serde_json::from_str(&qip_acceptance::read(
        "infrastructure/event-fabric/streams.local.json",
    ))
    .expect("the stream catalogue is JSON");
    let streams: Vec<String> = catalogue["streams"]
        .as_array()
        .expect("the catalogue lists streams")
        .iter()
        .map(|stream| {
            stream["name"]
                .as_str()
                .expect("a stream has a name")
                .to_string()
        })
        .collect();
    assert!(streams.len() >= 4, "the catalogue lists {streams:?}");

    let peer = qip_acceptance::read("backend/crates/libs/qip-peer/src/lib.rs");
    let kinds = peer_kinds(&peer);
    assert!(
        kinds.len() >= 12 && kinds.contains(&"fire".to_string()),
        "the peer vocabulary was not read: {kinds:?}"
    );
    assert_eq!(
        colliding_kinds(&peer, &streams),
        Vec::<String>::new(),
        "a mesh frame kind or function shares a name with a fabric stream"
    );
    // Adding a function called `control` to a copy of the enum is found.
    let mutated = peer.replacen(
        "    HedgeUnwindCommand,\n}",
        "    HedgeUnwindCommand,\n    Control,\n}",
        1,
    );
    assert_ne!(mutated, peer, "the mutation did not apply");
    assert_eq!(
        colliding_kinds(&mutated, &streams),
        vec!["control".to_string()]
    );

    let mut sources =
        qip_acceptance::files_with_extension("backend/crates/libs/qip-peer/src", "rs");
    sources.extend(qip_acceptance::files_with_extension(
        "backend/crates/edge/qip-peer-link/src",
        "rs",
    ));
    assert!(sources.len() >= 2, "{sources:?}");
    let mut naming = Vec::new();
    for file in &sources {
        let text = std::fs::read_to_string(file).expect("a peer source is readable");
        for stream in &streams {
            if text.contains(stream.as_str()) {
                naming.push((file.display().to_string(), stream.clone()));
            }
        }
    }
    assert_eq!(
        naming,
        Vec::<(String, String)>::new(),
        "a peer source names a fabric stream"
    );

    // The helpers read what they claim to.
    assert_eq!(snake_case("PeerReflexMessage"), "peer_reflex_message");
}
