//! The in-tree mesh, wearing this crate's transport ports.
//!
//! ADR 0011 replaced Google Pub/Sub with `qip-transport`: an HTTP/1.1 client
//! and a mesh that carries `qip_events::AnyEvent` frames between the regional
//! cells and the central plane. That transport is real and it is tested against
//! a real socket. What was missing was the forty lines that make it a
//! [`Publisher`] and a [`Subscriber`], so a caller could hold it without naming
//! it — and those forty lines lived in `qip-transport`'s own test file, proven
//! and shipping in nothing.
//!
//! They live here because the dependency edge only runs one way. `qip-transport`
//! is a library and this is a service, and
//! `a_library_never_depends_on_a_service_or_an_application` in the acceptance
//! suite forbids the library reaching up. A service depending on a library is
//! the permitted direction, so the adapter belongs on this side of the edge,
//! beside [`crate::local`] and [`crate::durable`].
//!
//! # What this transport does *not* promise
//!
//! Two things, and both are stated rather than discovered:
//!
//! * **It is not durable.** The outbound queue, the peer's inbox and the
//!   default dead-letter sink are all in process memory. A pod restart on
//!   either end loses whatever was in flight. [`TransportDescriptor::durable`]
//!   therefore says `false` even though [`TransportDescriptor::path`] says
//!   `Durable` — the path is which routing class may travel here, not a claim
//!   about what survives. Claiming a durability it does not have is the mistake
//!   [`crate::pubsub`] refuses to make in the other direction, and it would be
//!   the same mistake.
//! * **Delivery is at-least-once.** A publisher that loses an acknowledgement
//!   cannot tell that apart from a message that never arrived, so it retries and
//!   the peer sees the message twice. The duplicate is *detectable* by
//!   idempotency key inside a bounded window and no further. Every consumer of
//!   this transport must be idempotent; that is a precondition for using it, not
//!   advice.
//!
//! Both facts are readable before anything is built, through
//! [`crate::registry::TransportKind::guarantees`], which is the point of having
//! them written down as data.
//!
//! # A hot event is refused twice
//!
//! Once here, on the routing class, and once inside
//! `qip_transport::MeshPublisher::enqueue`, on the topic alone. The second is
//! deliberately the stricter of the two and is not this module's to relax: a
//! network hop with a retry ladder in front of a decision measured in
//! microseconds is exactly the latency [`crate::routing`] exists to protect, and
//! a tick is replaceable within milliseconds anyway.

use std::sync::Arc;

use qip_contracts::time::Watermark;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use qip_transport::{
    DeadLetterSink, Delivery, MeshConfig, MeshPublisher, PublisherStats, RemoteSubscriber, Sleeper,
    SubscriberStats,
};

use crate::envelope::StreamEnvelope;
use crate::ports::{PublishReceipt, Publisher, Subscriber, TransportDescriptor};
use crate::routing::{RoutingClass, TransportPath};

/// Both halves of one mesh link: what this cell sends to a peer, and what it
/// pulls back from that peer's inbox.
///
/// One type rather than two because that is the topology. A regional cell
/// publishes state deltas up to the central plane and pulls signed capital
/// envelopes back down, and both directions address the same peer. Holding them
/// together is also what lets the registry hand out a single object that is a
/// [`Publisher`] and a [`Subscriber`] at once, which a caller that wants to
/// round-trip anything needs.
#[derive(Debug)]
pub struct MeshTransport {
    publisher: MeshPublisher,
    subscriber: RemoteSubscriber,
}

impl MeshTransport {
    /// What a production deployment must supply beyond a peer address.
    ///
    /// Delegated to `qip_transport::MeshPublisher` rather than restated, so the
    /// list a start-up check renders cannot drift from the list the transport
    /// itself publishes. Chief among them is mutual TLS: this wire speaks
    /// plaintext and authenticates nobody.
    pub const REQUIREMENTS: [&'static str; 4] = MeshPublisher::REQUIREMENTS;

    /// Build both halves against one peer.
    ///
    /// The clock, the sleeper and the dead-letter sink are parameters for the
    /// reason `qip-transport` makes them parameters: they are the three places
    /// this transport would otherwise reach for something ambient, and each of
    /// them is what a test has to replace to assert on a retry ladder instead of
    /// spending it. Nothing here constructs a default for any of the three.
    pub fn connect(
        config: MeshConfig,
        clock: Arc<dyn Clock>,
        sleeper: Arc<dyn Sleeper>,
        dead_letters: Box<dyn DeadLetterSink>,
    ) -> Result<Self> {
        let publisher =
            MeshPublisher::new(config.clone(), clock, Arc::clone(&sleeper), dead_letters)?;
        let subscriber = RemoteSubscriber::new(config, sleeper)?;
        Ok(Self {
            publisher,
            subscriber,
        })
    }

    /// The sending half, for the counters and the dead letters this port does
    /// not expose.
    pub fn publisher(&self) -> &MeshPublisher {
        &self.publisher
    }

    /// The pulling half, for the same reason.
    pub fn subscriber(&self) -> &RemoteSubscriber {
        &self.subscriber
    }

    pub fn publisher_stats(&self) -> PublisherStats {
        self.publisher.stats()
    }

    pub fn subscriber_stats(&self) -> SubscriberStats {
        self.subscriber.stats()
    }

    /// The peer this link addresses, in both directions.
    pub fn peer(&self) -> String {
        self.publisher.peer().to_string()
    }

    /// Why this envelope may not cross the mesh, if it may not.
    ///
    /// The mirror of the refusal in [`crate::durable`], and for the same reason
    /// stated the other way round: a hot event reaching here means a router sent
    /// it to the wrong place, and the round trip it was about to make is the
    /// latency its class exists to avoid.
    fn refuse_hot(envelope: &StreamEnvelope) -> Result<()> {
        if matches!(envelope.routing_class(), RoutingClass::Hot) {
            return Err(Error::invalid(format!(
                "{} is routed hot and must not cross the mesh: a network hop with a retry ladder \
                 and a dead-letter path in front of a decision measured in microseconds is the \
                 latency the class exists to avoid, and the event is replaceable within \
                 milliseconds",
                envelope.event_id()
            )));
        }
        Ok(())
    }

    fn receipt(&self, delivery: Delivery) -> PublishReceipt {
        PublishReceipt {
            transport: delivery.transport,
            path: TransportPath::Durable,
            // The publisher's own send sequence, not the peer's. The peer
            // reports `None` for a message it recognised as a duplicate and
            // therefore did not queue again, and a receipt whose position
            // disappeared on redelivery would be a worse answer than one that
            // consistently describes this side of the link.
            position: delivery.position,
            // No record hash. The mesh chains nothing: the frame's own payload
            // hash is checked by the receiving endpoint, but there is no
            // append-only chain over the sequence of them, and reporting one
            // here would be a claim that a truncated history is detectable.
            // It is not — that is what `crate::durable` is for.
            record_hash: None,
            accepted_at: delivery.accepted_at,
        }
    }

    fn descriptor_inner(&self) -> TransportDescriptor {
        let inner = self.publisher.descriptor();
        TransportDescriptor {
            name: inner.name,
            // Warm and cold events are what crosses this wire; a hot one is
            // refused, so the path it serves is the durable one.
            path: TransportPath::Durable,
            // `false`, from the transport's own descriptor rather than asserted
            // here. Nothing on this link survives the process that held it.
            durable: inner.durable,
            available: inner.available,
            production_requirement: inner.production_requirement,
        }
    }
}

impl Publisher for MeshTransport {
    fn descriptor(&self) -> TransportDescriptor {
        self.descriptor_inner()
    }

    fn publish(&mut self, envelope: StreamEnvelope, at: Timestamp) -> Result<PublishReceipt> {
        Self::refuse_hot(&envelope)?;
        let delivery = self.publisher.publish_frame(envelope.to_frame()?, at)?;
        Ok(self.receipt(delivery))
    }

    /// Send a batch, refusing the whole of it before anything is queued.
    ///
    /// Overridden rather than left to the default loop, which stops at the first
    /// failure and leaves the caller unable to say which half was taken.
    /// `qip_transport::MeshPublisher::publish_frames` admits all of the batch or
    /// none of it, so the question does not arise — and the framing and the
    /// routing check happen for every envelope first, so a batch with one bad
    /// member is refused rather than half-sent.
    fn publish_batch(
        &mut self,
        envelopes: Vec<StreamEnvelope>,
        at: Timestamp,
    ) -> Result<Vec<PublishReceipt>> {
        let mut frames = Vec::with_capacity(envelopes.len());
        for envelope in &envelopes {
            Self::refuse_hot(envelope)?;
            frames.push(envelope.to_frame()?);
        }
        let deliveries = self.publisher.publish_frames(frames, at)?;
        Ok(deliveries
            .into_iter()
            .map(|delivery| self.receipt(delivery))
            .collect())
    }
}

impl Subscriber for MeshTransport {
    fn descriptor(&self) -> TransportDescriptor {
        self.descriptor_inner()
    }

    /// Pull everything the peer knew at or before `until`, verifying each frame
    /// on the way in.
    ///
    /// `StreamEnvelope::from_frame` recomputes the payload hash, so this is not
    /// a formality: an envelope edited between here and the peer is refused
    /// rather than handed to a consumer. That check is not authentication and
    /// does not pretend to be — see `qip_transport`'s crate documentation, where
    /// the answer is mTLS.
    fn poll(&mut self, until: Timestamp) -> Result<Vec<StreamEnvelope>> {
        self.subscriber
            .poll(until)?
            .iter()
            .map(StreamEnvelope::from_frame)
            .collect()
    }

    fn watermark(&self) -> Option<Watermark> {
        self.subscriber.watermark()
    }

    /// Whether the peer said it was holding anything at the **last** poll.
    ///
    /// This is the one place the port's shape does not survive the wire intact,
    /// and it is stated rather than hidden. The local and durable transports
    /// answer this question exactly, by looking at a buffer they own. The mesh's
    /// buffer is on another host, so the only answer available without a network
    /// round trip is the `remaining` hint the peer attached to the previous
    /// response — which is one round trip out of date, and which the peer
    /// computed against *that* poll's `until` rather than this one's. So `until`
    /// is ignored here, and a subscriber that has never polled reports nothing
    /// pending even when the peer is full.
    ///
    /// Asking the peer afresh would be a poll, and a predicate documented as
    /// cheap that opened a socket would be worse than an imprecise one. A caller
    /// that intends to consume should poll; this is for a scheduler deciding
    /// whether to bother.
    fn has_pending(&self, _until: Timestamp) -> bool {
        self.subscriber.pending_hint() > 0
    }
}
