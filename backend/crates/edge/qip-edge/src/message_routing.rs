//! FABRIC-028 Phase 2a: Message routing validation in the edge cell.
//!
//! The cell produces order-related messages (OrderSubmitted, OrderAmended,
//! OrderCancelled, OrderFilled) that must traverse the VenueIo path only.
//! This module provides validation helpers that the cell's order placement
//! and fill-handling logic can use to prove topics are routed correctly.
//!
//! The module is purely a validation layer — it has no I/O and makes no
//! decisions. It names what went wrong when a routing contract is violated,
//! so the caller can refuse under the right gate and chart the refusal.

use qip_core::error::Result;
use qip_events::event_fabric::message_routing::{FabricPath, MessageTypeRouter};
use qip_events::topic::Topic;

/// Validates that a Topic is registered to route to VenueIo.
///
/// Used before the cell places an order, confirming that the order's topic
/// is one the MessageTypeRouter knows about and expects to see on VenueIo only.
/// If routing were ever changed — a new order variant added without updating
/// the router — this validation catches it before the order is placed.
///
/// # Errors
///
/// Returns `Error::denied` if:
/// - The Topic is not registered at all (unknown topic)
/// - The Topic is registered but not to VenueIo (routing violation)
///
/// The error message names both the topic and the path it was routed to,
/// so the cell can refuse under a gate that names the violation clearly.
pub fn validate_venue_order(topic: Topic) -> Result<()> {
    let router = MessageTypeRouter::new();

    match router.validate(topic, FabricPath::VenueIo) {
        Ok(()) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Batch validation for all order-related topics the cell produces.
///
/// The cell produces four order-related topics: OrderSubmitted, OrderAmended,
/// OrderCancelled, OrderFilled. This function proves all four are registered
/// and routed to VenueIo, without the cell having to call validate_venue_order
/// four times.
///
/// Used in tests and at cell construction time to prove the routing contract
/// is satisfied before the cell begins work.
pub fn validate_all_venue_topics() -> Result<()> {
    let topics = [
        Topic::OrderSubmitted,
        Topic::OrderAmended,
        Topic::OrderCancelled,
        Topic::OrderFilled,
    ];

    for topic in &topics {
        validate_venue_order(*topic)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn venue_order_topics_validate() {
        assert!(validate_venue_order(Topic::OrderSubmitted).is_ok());
        assert!(validate_venue_order(Topic::OrderAmended).is_ok());
        assert!(validate_venue_order(Topic::OrderCancelled).is_ok());
        assert!(validate_venue_order(Topic::OrderFilled).is_ok());
    }

    #[test]
    fn batch_validation_passes_when_all_topics_route_to_venue() {
        assert!(validate_all_venue_topics().is_ok());
    }

    #[test]
    fn validate_venue_order_rejects_non_venue_topics() {
        // These topics are not order-related and should be rejected
        assert!(validate_venue_order(Topic::PolicyDistributed).is_err());
        assert!(validate_venue_order(Topic::KillSwitchEngaged).is_err());
        assert!(validate_venue_order(Topic::MarketEventApplied).is_err());
    }

    #[test]
    fn error_message_names_the_violation() {
        let result = validate_venue_order(Topic::PolicyDistributed);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        // The error should name the topic that was rejected
        assert!(msg.contains("PolicyDistributed") || msg.contains("MeshLink"));
    }
}
