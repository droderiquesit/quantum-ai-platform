//! Event fabric envelope wrapping `AnyEvent` with fabric-specific metadata.
//!
//! Carries `FabricEnvelope` and its use of the hybrid logical clock (`hlc`).
//! `StreamPolicy` and the QoS classes live in `policy`, not here.
//! See ADR 0100 §1 for the event fabric's architecture.
//! Implemented by SLICE-14.
