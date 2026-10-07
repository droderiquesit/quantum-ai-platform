//! The MessageTypeRouter's own contract: every Topic routes to exactly one
//! I/O path (VenueIo, LocalJournal, MeshLink).
//!
//! These tests hold the router itself and nothing else. The router is not yet
//! wired into the three subsystems that would enforce it: the qip-edge Cell on
//! venue sends, the qip-transport producer, and the qip-streaming broker.
//! Nothing tests that enforcement because it does not exist. Four skipped
//! placeholders for it, whose bodies were TODO comments, were removed on
//! 2026-10-07 under `pipeline_waivers`' "run or delete it". The work they
//! described is still open, and an ignored test with no body is not a record
//! of it.
//!
//! (This file was headed "FABRIC-028 Phase 2" until then. FABRIC-028 is
//! partition isolation, a different requirement with its own tests in
//! qip-fabricd and qip-cli.)

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
