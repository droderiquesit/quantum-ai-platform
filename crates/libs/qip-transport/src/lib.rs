//! `qip-transport` — the streaming backbone, in-tree.
//!
//! ADR 0011 replaced Google Pub/Sub with "an in-tree HTTP/1.1 client and mesh
//! transport". This is it: the wire nine regional cells push state deltas up
//! and the central plane pushes signed capital envelopes down.
//!
//! Two halves:
//!
//! * [`http`] — an HTTP/1.1 **client** on `std::net::TcpStream`, the other half
//!   of `qip_api::http`'s server, with every limit the server has and one more
//!   reason for them: here it is the *peer* that is untrusted.
//! * [`mesh`] — the transport that carries `qip_events::AnyEvent` frames over
//!   that client, with the reliability a managed bus would have provided.
//!
//! # What the reliability costs, made explicit
//!
//! ADR 0011 says plainly that "retries, backpressure, at-least-once delivery,
//! dead-lettering, ordering guarantees — every one of these is now code in
//! `qip-transport` that has to be right". Each is a module here, and each
//! states what it does *not* do:
//!
//! | property | where | what it is, exactly |
//! |---|---|---|
//! | retry | [`retry`] | bounded exponential backoff, at most [`RetryPolicy::max_attempts`] sends, jitter drawn from a seeded [`qip_core::Xoshiro256`] and subtracted so the cap is a real cap |
//! | backpressure | [`queue`] | a bounded outbound queue that **refuses** with [`TransportError::QueueFull`]; it never drops and never grows |
//! | at-least-once | [`mesh`] | duplicates are possible in both directions and **detectable** by idempotency key; consumers must be idempotent |
//! | dead letters | [`deadletter`] | an exhausted message is recorded with its frame and its reason, never silently dropped |
//! | ordering | [`mesh`] | per-publisher FIFO with head-of-line blocking; no global order, no per-key order across publishers |
//!
//! ## Delivery is at-least-once. It is not exactly-once.
//!
//! Stated here as well as in [`mesh`] because it is the property most likely to
//! be assumed rather than read. A publisher that sends a message, has it
//! accepted, and loses the acknowledgement cannot distinguish that from the
//! message never arriving; it retries, and the peer sees it twice. No
//! arrangement of retries at this layer removes that, and refusing to retry
//! only converts it into at-most-once, which for a capital envelope is worse.
//!
//! So: **every consumer of this transport must be idempotent.** The transport's
//! job is to make the duplicate detectable — every message carries
//! `AnyEvent::dedup_key`, and the receiving inbox reports a repeat rather than
//! queueing it again — within a bounded window. Past that window the
//! consumer's own idempotency is the only thing left.
//!
//! # Security: plaintext, unauthenticated, and bounded by the network
//!
//! This transport has **no TLS and no authentication of the peer.** Anything
//! that can route a packet to the port can publish a state delta or drain the
//! inbox, and anything on the path can read and alter what is in flight.
//!
//! The deployment boundary that makes that acceptable, and which is therefore
//! load bearing rather than incidental:
//!
//! * peers are inside one VPC, addressed by cluster DNS,
//! * a default-deny egress NetworkPolicy permits only the named mesh peers,
//! * the port is not exposed through any ingress.
//!
//! **Production must add mutual TLS.** The realistic answer is a service mesh
//! sidecar terminating mTLS, so peer identity comes from workload certificates
//! that this process never handles; the alternative is a TLS stack, which this
//! build does not have and which is not a dependency ADR 0011 takes.
//! [`MeshPublisher::REQUIREMENTS`] lists this as data so a start-up check can
//! render the whole list rather than discovering it one incident at a time.
//!
//! What is deliberately **not** here is a hand-rolled handshake. ADR 0009
//! already names writing crypto in-tree as the worst of the available options
//! and the one that looks most principled: an authentication scheme invented
//! here, guarding real money, reviewed by nobody who does this for a living,
//! would be worse than plaintext — because plaintext is honest about what it
//! protects and a home-made handshake is not.
//!
//! The one integrity check that *is* here is not cryptography and does not
//! pretend to be: the receiving endpoint recomputes each frame's payload hash
//! and refuses a mismatch. It catches corruption in transit and a naive edit.
//! It stops nobody who can also recompute a SHA-256.
//!
//! # No ambient anything
//!
//! Consistent with `qip-core`'s first rule. The clock is an injected
//! [`qip_core::Clock`], the jitter is a seeded [`qip_core::Xoshiro256`], and
//! even the *waiting* goes through an injected [`Sleeper`] — so a test asserts
//! the backoff schedule instead of spending it. The only wall-clock dependency
//! left is the socket timeouts, which are the operating system's.
//!
//! # Blocking I/O, on purpose
//!
//! `std::net` with explicit read, write and connect timeouts, one connection
//! per request, one thread per publisher. The same model as the server, and no
//! async runtime. Mesh traffic is a few small documents per second per cell;
//! the machinery to do that without blocking a thread would cost more in
//! dependencies and in unreviewable complexity than it saves.

pub mod breaker;
pub mod deadletter;
pub mod error;
pub mod http;
pub mod mesh;
pub mod queue;
pub mod retry;
pub mod spool;
pub mod stream;

pub use deadletter::{DeadLetter, DeadLetterReason, DeadLetterSink, MemoryDeadLetters};
pub use error::TransportError;
pub use http::{
    ClientLimits, HttpClient, HttpError, HttpRequest, HttpResponse, Method, Phase, Url,
};
pub use mesh::{
    Admission, DeadLettered, Delivery, EndpointResponse, FlushReport, InboxFrame, InboxHealth,
    InboxStats, MeshBatch, MeshConfig, MeshDescriptor, MeshEndpoint, MeshInbox, MeshMessage,
    MeshPublisher, PollRequest, PollResponse, PublishAck, PublisherStats, RemoteSubscriber,
    SubscriberStats,
};
pub use queue::{OutboundQueue, QueueStats};
pub use retry::{RecordingSleeper, RetryPolicy, Sleeper, ThreadSleeper};
