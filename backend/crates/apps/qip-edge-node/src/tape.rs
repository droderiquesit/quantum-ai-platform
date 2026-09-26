//! ADR 0100 §8: the canonical market-event tape that seeds the node's
//! `SimulatedGateway` — known-at ordering is refused, not sorted. Recording
//! the tape events a pass actually applied is `event_fabric::inputs`, not
//! here. SLICE-19.
