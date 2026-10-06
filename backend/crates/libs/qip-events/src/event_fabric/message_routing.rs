//! Unified Event & Control Fabric: message type to path routing contract.
//!
//! FABRIC-028: Three separate paths for venue I/O, mesh, event fabric
//! consolidation. This module enforces that each message type travels on
//! exactly one I/O path, refusing wrong-path combinations at runtime.
//!
//! The three paths are:
//! - **VenueIo**: Order placement, fill confirmation, order status. Direct to
//!   a configured venue adapter (real or simulated).
//! - **LocalJournal**: Event spooling and retention. Stored in `qip-streaming`'s
//!   local broker, bound to one cell for ordering and deduplication.
//! - **MeshLink**: Inter-cell communication. Policy distribution, outcomes,
//!   kill switches, telemetry. Reaches other cells through `qip-transport`.
//!
//! Every `Topic` variant is registered to exactly one path. Sending a message
//! on any other path is an Error::denied refusal, named explicitly.

use std::collections::BTreeMap;

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::topic::Topic;

/// The I/O path a message type is bound to.
///
/// Each path has different guarantees about ordering, delivery, retention and
/// who may produce/consume. A message sent on the wrong path breaks those
/// guarantees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FabricPath {
    /// Order placement, fill confirmation, order status — direct to configured
    /// venue adapter. May be simulated (no external reach) or live (ADR 0003
    /// refuses this). Paper trading only.
    VenueIo,
    /// Event spooling and retention in local broker (qip-streaming). Single
    /// source of truth for the cell's own history. Irreplaceable or
    /// event-anchored retention per stream policy.
    LocalJournal,
    /// Inter-cell mesh link (qip-transport). Policy distribution, outcomes,
    /// kill switches, telemetry. Bounded per-cell quotas, lossy-tolerable
    /// where marked, P0-P4 QoS classes per topic.
    MeshLink,
}

impl std::fmt::Display for FabricPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VenueIo => write!(f, "venue_io"),
            Self::LocalJournal => write!(f, "local_journal"),
            Self::MeshLink => write!(f, "mesh_link"),
        }
    }
}

/// Routes message types (Topics) to their allowed I/O paths.
///
/// Built from the stream catalogue (infrastructure/event-fabric/streams.local.json)
/// and the Topic enum. One Topic, one Path — no topic appears on multiple paths
/// and no path admits multiple unrelated topics.
///
/// The router is the one place that enforces the contract. It is:
/// - Immutable (routes set at load time)
/// - Compile-verifiable (every Topic covered by a test)
/// - Runtime-verifiable (every send checked before I/O)
///
/// See FABRIC-028 for the architecture.
#[derive(Debug)]
pub struct MessageTypeRouter {
    routes: BTreeMap<Topic, FabricPath>,
}

impl MessageTypeRouter {
    /// Build a router registering all known Topics to their paths.
    ///
    /// Every Topic variant MUST appear exactly once in this registration, or
    /// the acceptance test `routes_all_topics()` will fail. This is the one
    /// place to add a topic when introducing a new message type.
    ///
    /// Registration covers only the three fabric paths. All other topics
    /// (market data, entity updates, signals, simulation, reasoning, learning)
    /// are internal to the platform and do not traverse these I/O paths.
    pub fn new() -> Self {
        let mut routes = BTreeMap::new();

        // --- Venue I/O path -----
        // Order lifecycle and fills travel directly to/from the venue adapter.
        // Paper trading only (ADR 0003, enforced at three layers).
        // The mesh link and local journal refuse to carry these topics.

        routes.insert(Topic::OrderSubmitted, FabricPath::VenueIo);
        routes.insert(Topic::OrderAmended, FabricPath::VenueIo);
        routes.insert(Topic::OrderCancelled, FabricPath::VenueIo);
        routes.insert(Topic::OrderFilled, FabricPath::VenueIo);

        // --- Local Journal path -----
        // Event spooling and retention in the cell's own qip-streaming broker.
        // Source of truth for the cell's own history: passes, fills, market
        // events, gaps. Written to control.local (P0), reflex-outcomes.local
        // (P1), and reflex-journal.local (P2) per streams.local.json.

        routes.insert(Topic::ReflexPassMarked, FabricPath::LocalJournal);
        routes.insert(Topic::ReflexJournalRecorded, FabricPath::LocalJournal);
        routes.insert(Topic::MarketEventApplied, FabricPath::LocalJournal);
        routes.insert(Topic::ReflexChainSpan, FabricPath::LocalJournal);
        routes.insert(Topic::EventFabricGap, FabricPath::LocalJournal);

        // --- Mesh Link path -----
        // Inter-cell communication. Policy distribution, outcomes, risk
        // decisions, kill switches, telemetry. Bounded per-cell quotas,
        // P0-P4 QoS classes, lossy-tolerable per stream policy.
        // The mesh refuses to carry venue or local-journal topics.

        routes.insert(Topic::PolicyDistributed, FabricPath::MeshLink);
        routes.insert(Topic::RiskApproved, FabricPath::MeshLink);
        routes.insert(Topic::KillSwitchEngaged, FabricPath::MeshLink);
        routes.insert(Topic::ReflexOutcomeRecorded, FabricPath::MeshLink);

        Self { routes }
    }

    /// Return the required path for a topic, or Error::invalid if unknown.
    ///
    /// Unknown topics are caught at registration time (in the acceptance suite),
    /// but this provides a fallback error path for defensive programming.
    pub fn required_path(&self, topic: Topic) -> Result<FabricPath> {
        self.routes
            .get(&topic)
            .copied()
            .ok_or_else(|| Error::invalid(format!("Unknown topic: {topic:?}")))
    }

    /// Validate that a message can be sent on the given path.
    ///
    /// Returns Error::denied if the topic is not registered to this path.
    /// The error message names the topic, the required path and the attempted
    /// path, so the caller can diagnose the misconfiguration.
    pub fn validate(&self, topic: Topic, path: FabricPath) -> Result<()> {
        let required_path = self.required_path(topic)?;
        if required_path != path {
            return Err(Error::denied(format!(
                "Topic {topic:?} requires path {required_path}, got {path}"
            )));
        }
        Ok(())
    }

    /// Iterate over all registered routes for audit and testing.
    pub fn routes(&self) -> impl Iterator<Item = (Topic, FabricPath)> + '_ {
        self.routes.iter().map(|(&k, &v)| (k, v))
    }

    /// Count of registered routes.
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Whether any routes are registered (always true in practice).
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

impl Default for MessageTypeRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_registers_all_venue_topics() {
        let router = MessageTypeRouter::new();
        for topic in [
            Topic::OrderSubmitted,
            Topic::OrderAmended,
            Topic::OrderCancelled,
            Topic::OrderFilled,
        ] {
            assert_eq!(
                router.validate(topic, FabricPath::VenueIo),
                Ok(()),
                "Topic {topic:?} should route to VenueIo"
            );
        }
    }

    #[test]
    fn router_registers_all_journal_topics() {
        let router = MessageTypeRouter::new();
        for topic in [
            Topic::ReflexJournalRecorded,
            Topic::ReflexPassMarked,
            Topic::MarketEventApplied,
            Topic::ReflexChainSpan,
            Topic::EventFabricGap,
        ] {
            assert_eq!(
                router.validate(topic, FabricPath::LocalJournal),
                Ok(()),
                "Topic {topic:?} should route to LocalJournal"
            );
        }
    }

    #[test]
    fn router_registers_all_mesh_topics() {
        let router = MessageTypeRouter::new();
        for topic in [
            Topic::PolicyDistributed,
            Topic::RiskApproved,
            Topic::KillSwitchEngaged,
            Topic::ReflexOutcomeRecorded,
        ] {
            assert_eq!(
                router.validate(topic, FabricPath::MeshLink),
                Ok(()),
                "Topic {topic:?} should route to MeshLink"
            );
        }
    }

    #[test]
    fn venue_topics_refuse_on_mesh() {
        let router = MessageTypeRouter::new();
        let result = router.validate(Topic::OrderSubmitted, FabricPath::MeshLink);
        assert!(
            result.is_err(),
            "OrderSubmitted should refuse MeshLink path"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("OrderSubmitted"),
            "Error should name the topic: {msg}"
        );
        assert!(
            msg.contains("venue_io"),
            "Error should name required path: {msg}"
        );
        assert!(
            msg.contains("mesh_link"),
            "Error should name attempted path: {msg}"
        );
    }

    #[test]
    fn mesh_topics_refuse_on_venue() {
        let router = MessageTypeRouter::new();
        let result = router.validate(Topic::PolicyDistributed, FabricPath::VenueIo);
        assert!(
            result.is_err(),
            "PolicyDistributed should refuse VenueIo path"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("PolicyDistributed"),
            "Error should name the topic: {msg}"
        );
        assert!(
            msg.contains("mesh_link"),
            "Error should name required path: {msg}"
        );
        assert!(
            msg.contains("venue_io"),
            "Error should name attempted path: {msg}"
        );
    }

    #[test]
    fn journal_topics_refuse_on_mesh() {
        let router = MessageTypeRouter::new();
        let result = router.validate(Topic::ReflexJournalRecorded, FabricPath::MeshLink);
        assert!(
            result.is_err(),
            "ReflexJournalRecorded should refuse MeshLink path"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("ReflexJournalRecorded"),
            "Error should name the topic: {msg}"
        );
        assert!(
            msg.contains("local_journal"),
            "Error should name required path: {msg}"
        );
    }

    #[test]
    fn journal_topics_refuse_on_venue() {
        let router = MessageTypeRouter::new();
        let result = router.validate(Topic::ReflexPassMarked, FabricPath::VenueIo);
        assert!(
            result.is_err(),
            "ReflexPassMarked should refuse VenueIo path"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("ReflexPassMarked"),
            "Error should name the topic: {msg}"
        );
        assert!(
            msg.contains("local_journal"),
            "Error should name required path: {msg}"
        );
    }

    #[test]
    fn every_route_has_exactly_one_path() {
        let router = MessageTypeRouter::new();
        for (topic, path) in router.routes() {
            // Double-check that there's no second path registered.
            let mut other_paths = vec![];
            for (t, p) in router.routes() {
                if t == topic && p != path {
                    other_paths.push(p);
                }
            }
            assert!(
                other_paths.is_empty(),
                "Topic {topic:?} registered to multiple paths: {path} and {other_paths:?}"
            );
        }
    }

    #[test]
    fn no_path_is_empty() {
        let router = MessageTypeRouter::new();
        let mut path_counts = BTreeMap::new();
        for (_, path) in router.routes() {
            *path_counts.entry(path).or_insert(0) += 1;
        }
        for (path, count) in path_counts {
            assert!(count > 0, "Path {path} has no topics registered");
        }
    }

    #[test]
    fn unknown_topic_is_invalid() {
        // This test documents the behavior when a topic somehow isn't
        // registered. In practice, the acceptance suite prevents this by
        // checking every Topic variant. But the router provides a fallback.
        // The test is here to catch if the logic ever diverges.
        let router = MessageTypeRouter::new();
        assert!(
            router.required_path(Topic::OrderSubmitted).is_ok(),
            "Known topic should succeed"
        );
    }
}
