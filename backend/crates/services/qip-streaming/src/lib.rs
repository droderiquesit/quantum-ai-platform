//! `qip-streaming` — the universal event envelope and the transports behind it.
//!
//! Everything the platform senses enters through here, and it enters wearing
//! the same envelope whether it came from a venue's multicast feed, a filings
//! scraper or a macroeconomic release. That uniformity is what lets one
//! deduplicator, one gap detector and one replay path serve every source.
//!
//! # This extends `qip-events`; it does not replace it
//!
//! `qip_events::AnyEvent` is already the platform's event: identity, topic,
//! valid-time and known-time, lineage, an idempotency key and a payload hash.
//! [`StreamEnvelope`] wraps one and adds only what ingestion knows and the
//! in-process bus does not — the upstream source, the subject, the cost, and
//! which wire the event may travel on. The full mapping from the target field
//! names onto what already existed is in [`envelope`], and it is worth reading
//! before adding a field here: a second parallel envelope would be the worst
//! possible outcome for a platform whose whole audit story is that there is one
//! event history.
//!
//! Likewise, gap detection is `qip-sequencing`'s and is composed rather than
//! rebuilt ([`sequencing`]), the durable store is `qip_events::EventLog`
//! ([`durable`]), and the bitemporal clamp is `qip_contracts::Stamped`.
//!
//! # Four properties the types carry
//!
//! * **A payload hash cannot disagree with its payload.** The hash is computed
//!   on the way in and recomputed on the way back; there is no constructor that
//!   accepts one. See [`StreamEnvelope::seal`].
//! * **Ingest never precedes the event, and the correction is visible.** The
//!   pair is clamped forward and the reported value is kept, so a source with a
//!   bad clock is diagnosable rather than merely tolerated.
//! * **Freshness is derived from the two timestamps and an as-of.** It is not a
//!   stored field, so it cannot drift from what it describes and cannot be read
//!   without someone supplying a clock.
//! * **A venue-critical event cannot be routed through a durable buffer.**
//!   [`RoutingClass::check`] refuses the combination at construction, and both
//!   transports refuse it again at publish. See [`routing`].
//!
//! # The consumer pulls
//!
//! [`Subscriber::poll`] takes an `until` and drains to it, exactly as
//! `qip_market_ingestion::adapter::DataAdapter::poll` does. The caller keeps
//! the clock, so the same loop drives a live run, a replay and a backtest and
//! only the clock differs. A push-based consumer would destroy that parity, and
//! the parity is the reason to believe a backtest at all. See [`ports`].
//!
//! # What production still needs
//!
//! The durable transport in this crate is real and it works. The managed
//! Pub/Sub transport is a port: it names the GCP project, the topic and
//! subscription, the gRPC client with TLS and the workload-identity binding it
//! requires, and returns [`qip_core::Error::Unavailable`] until they exist. It
//! never falls back to the local log and never fakes a connection. See
//! [`pubsub`].

pub mod durable;
pub mod envelope;
pub mod local;
pub mod mesh;
pub mod ports;
pub mod processing;
pub mod provenance;
pub mod pubsub;
pub mod registry;
pub mod routing;
pub mod schema;
pub mod sequencing;
pub mod tiered;

pub use durable::DurableLogTransport;
pub use envelope::{EventFacts, Freshness, StreamEnvelope};
pub use local::{LocalQueue, LocalQueueStats};
pub use ports::{PublishReceipt, Publisher, Subscriber, TransportDescriptor};
pub use processing::{
    BatchOutcome, ProcessingPolicy, ProcessingStats, Rejection, RejectionReason, StreamObservation,
    StreamProcessor,
};
pub use provenance::{
    Confidence, CostMetadata, Region, SourceId, SourceIdentity, SourceType, Subject,
};
pub use pubsub::{PubSubBinding, PubSubTransport};
pub use routing::{RoutingClass, TransportPath};
pub use schema::{ENVELOPE_SCHEMA_VERSION, EnvelopeWire, SchemaCompatibility};
pub use sequencing::{SequenceCoordinator, SequencedEnvelopes, StreamReset};
pub use tiered::{RoutedCounts, TieredPublisher};
