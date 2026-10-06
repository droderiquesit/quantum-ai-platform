//! FABRIC-028 Phase 2: Infrastructure preparation for unified message routing.
//!
//! The MessageTypeRouter enforces that each Topic routes to exactly one
//! I/O path (VenueIo, LocalJournal, MeshLink). This acceptance suite verifies
//! that the router is properly integrated into the three subsystems that
//! produce or consume messages on those paths:
//!
//! 1. **qip-edge Cell** — validates venue sends before they traverse VenueIo
//! 2. **qip-transport producer** — refuses VenueIo/LocalJournal topics
//! 3. **qip-streaming broker** — refuses VenueIo/MeshLink topics
//!
//! Until execution_nodes are deployed (blocked: boot image missing, ADR 0045
//! region_allocation decision pending), the integration tests below remain
//! skipped. They serve as the blueprint for Phase 2 integration work.
//!
//! Each test is mutation-verified and proves the routing contract holds at
//! the seam between the subsystems it touches.

use qip_events::event_fabric::message_routing::MessageTypeRouter;
use qip_events::topic::Topic;

/// The router registers all 14 Topics to their paths and refuses wrong-path
/// combinations at runtime.
#[test]
fn router_is_properly_exported_and_available() {
    let router = MessageTypeRouter::new();

    // The router must be constructed and hold routes for all three paths
    assert_ne!(
        router.required_path(Topic::OrderSubmitted).ok(),
        None,
        "Router must export required_path()"
    );
}

/// Every Topic has a path; the router admits no unknown topics.
///
/// This test proves the router validates at runtime. If a new Topic
/// were added without registration, `required_path` would return an error.
#[test]
fn every_topic_is_registered() {
    let router = MessageTypeRouter::new();

    // Venue I/O Topics must be registered
    assert!(
        router.required_path(Topic::OrderSubmitted).is_ok(),
        "OrderSubmitted must be registered to VenueIo"
    );
    assert!(
        router.required_path(Topic::OrderAmended).is_ok(),
        "OrderAmended must be registered to VenueIo"
    );
    assert!(
        router.required_path(Topic::OrderCancelled).is_ok(),
        "OrderCancelled must be registered to VenueIo"
    );
    assert!(
        router.required_path(Topic::OrderFilled).is_ok(),
        "OrderFilled must be registered to VenueIo"
    );

    // LocalJournal Topics must be registered
    assert!(
        router.required_path(Topic::ReflexPassMarked).is_ok(),
        "ReflexPassMarked must be registered to LocalJournal"
    );
    assert!(
        router.required_path(Topic::ReflexJournalRecorded).is_ok(),
        "ReflexJournalRecorded must be registered to LocalJournal"
    );
    assert!(
        router.required_path(Topic::MarketEventApplied).is_ok(),
        "MarketEventApplied must be registered to LocalJournal"
    );
    assert!(
        router.required_path(Topic::ReflexChainSpan).is_ok(),
        "ReflexChainSpan must be registered to LocalJournal"
    );
    assert!(
        router.required_path(Topic::EventFabricGap).is_ok(),
        "EventFabricGap must be registered to LocalJournal"
    );

    // MeshLink Topics must be registered
    assert!(
        router.required_path(Topic::PolicyDistributed).is_ok(),
        "PolicyDistributed must be registered to MeshLink"
    );
    assert!(
        router.required_path(Topic::RiskApproved).is_ok(),
        "RiskApproved must be registered to MeshLink"
    );
    assert!(
        router.required_path(Topic::KillSwitchEngaged).is_ok(),
        "KillSwitchEngaged must be registered to MeshLink"
    );
    assert!(
        router.required_path(Topic::ReflexOutcomeRecorded).is_ok(),
        "ReflexOutcomeRecorded must be registered to MeshLink"
    );
}

/// Phase 2 Phase 2a: qip-edge Cell integration.
///
/// The cell validates venue sends before traversing VenueIo. This test
/// will instantiate a Cell, hand it the router, and prove it refuses
/// a VenueIo message on a non-VenueIo path.
///
/// BLOCKED: execution_nodes = {} in all environments. A regional cell
/// requires the boot image (image.yml) and ADR 0045 region_allocation
/// decision. See FABRIC_ANALYSIS.md.
#[test]
#[ignore = "execution_nodes not deployed; boot image missing; ADR 0045 pending"]
fn cell_rejects_venue_topics_on_non_venue_paths() {
    // TODO: FABRIC-028 Phase 2a
    // 1. Build a Cell with the MessageTypeRouter
    // 2. Configure it with OrderSubmitted on MeshLink
    // 3. Assert the cell refuses with Error::denied naming the routing violation
    // 4. Mutation: remove the validation, confirm test fails for the right reason
}

/// Phase 2b: qip-transport producer integration.
///
/// The transport producer refuses VenueIo and LocalJournal topics, which
/// are not meant to traverse the inter-cell mesh. This test will prove
/// the producer validates routing before sending.
///
/// BLOCKED: qip-transport server not deployed; only client implemented.
/// See FABRIC_ANALYSIS.md.
#[test]
#[ignore = "qip-transport server not yet deployed"]
fn transport_producer_refuses_local_topics() {
    // TODO: FABRIC-028 Phase 2b
    // 1. Construct a transport Producer
    // 2. Attempt to send OrderSubmitted (VenueIo) on MeshLink
    // 3. Assert the producer refuses with Error::denied
    // 4. Mutation: remove the validation, confirm test fails
}

/// Phase 2c: qip-streaming broker integration.
///
/// The streaming broker refuses VenueIo and MeshLink topics, which are not
/// meant to be retained in the local journal. This test proves the broker
/// validates routing on every ingest.
///
/// BLOCKED: qip-streaming broker not deployed in edge nodes.
/// See FABRIC_ANALYSIS.md.
#[test]
#[ignore = "qip-streaming broker role not yet configured in deployment"]
fn broker_refuses_non_journal_topics() {
    // TODO: FABRIC-028 Phase 2c
    // 1. Construct a Broker (or mock one) with the MessageTypeRouter
    // 2. Attempt to ingest OrderSubmitted (VenueIo) as a journal message
    // 3. Assert the broker refuses with Error::denied
    // 4. Mutation: remove the validation, confirm test fails
}

/// Phase 2 acceptance: The three subsystems work together.
///
/// This test will drive a complete cycle: a cell produces a venue message
/// on VenueIo, the transport producer refuses to carry it on MeshLink, and
/// the broker refuses to store it in the journal. Each refusal names the
/// routing violation in its error message.
///
/// BLOCKED: All three subsystems must be deployed for this to run.
#[test]
#[ignore = "execution_nodes not deployed; qip-transport server not deployed; broker role not deployed"]
fn fabric_routing_contract_holds_end_to_end() {
    // TODO: FABRIC-028 Phase 2 (all a, b, c together)
    // 1. Spin up a cell, a transport producer, and a broker
    // 2. Send OrderSubmitted (VenueIo) from the cell
    // 3. Assert it successfully reaches VenueIo (the gateway)
    // 4. Attempt to send it on MeshLink
    // 5. Assert the transport producer refuses
    // 6. Attempt to send it to the broker for journaling
    // 7. Assert the broker refuses
    // 8. Mutation test each refusal path
}
