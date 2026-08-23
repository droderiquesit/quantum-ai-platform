//! Choosing a transport by name, and knowing what you chose before you use it.
//!
//! Four transports implement [`Publisher`] and [`Subscriber`]: [`crate::local`]
//! in process, [`crate::durable`] on the hash-chained log, [`crate::mesh`] over
//! the in-tree HTTP mesh, and [`crate::pubsub`], which refuses. A caller that
//! names one of those types in its own code is a caller that has to be edited
//! to change its mind. This module is how a deployment states the choice as
//! configuration instead.
//!
//! Two rules shape it, and both are about the same failure.
//!
//! # Selecting something unavailable fails at construction, loudly
//!
//! There is no fallback. Asking for a transport that cannot work in this build
//! returns an error naming what is missing, and never quietly hands back one
//! that does work. [`crate::pubsub`] documents why at length: a service that
//! silently served a local queue instead of the bus it was configured for
//! would pass every smoke test, publish to nothing, and be discovered by
//! whoever was waiting for the messages a week later. A registry is exactly
//! where that mistake would be introduced, so it is refused here explicitly.
//!
//! # Guarantees are readable before anything is built
//!
//! [`TransportKind::guarantees`] answers what a transport promises without
//! constructing it, so a caller can refuse one whose guarantees are too weak
//! for the topic it is about to carry. That is what [`select_for`] does with a
//! [`RoutingClass`]: a `Warm` or `Cold` event must not be lost, and the local
//! path is lossy under overload by design, so pairing them is refused as a
//! configuration error rather than discovered as missing data.
//!
//! The mapping is [`RoutingClass::path`]'s, not a second copy of it. That
//! matters: this module was first written believing the rule ran one way only —
//! that a lossy transport must refuse a durable event, but a durable transport
//! could carry a hot one at worst wastefully. The transports themselves
//! disagree, and they are right. A hot event on the durable path is refused
//! because an append and a hash on the venue-critical path is precisely the
//! latency the class exists to avoid. Restating a rule is how a registry drifts
//! from the thing it configures, so this one asks.

use crate::durable::DurableLogTransport;
use crate::local::LocalQueue;
use crate::mesh::MeshTransport;
use crate::ports::{Publisher, Subscriber, TransportDescriptor};
use crate::pubsub::PubSubBinding;
use crate::routing::{RoutingClass, TransportPath};
use qip_core::SystemClock;
use qip_core::error::{Error, Result};
use qip_transport::deadletter::MemoryDeadLetters;
use qip_transport::mesh::MeshConfig;
use qip_transport::retry::ThreadSleeper;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Which transport a deployment is asking for.
///
/// Serialisable, because the whole point is that this arrives as configuration
/// rather than as a type named in code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    /// In-process, bounded, lossy under overload.
    Local,
    /// The append-only hash-chained log.
    Durable,
    /// The in-tree HTTP mesh between cells and the central plane.
    Mesh,
    /// Google Pub/Sub. Present so that configuring it fails with a legible
    /// reason rather than an unknown-name error, which would read as a typo.
    PubSub,
}

impl TransportKind {
    pub const ALL: [Self; 4] = [Self::Local, Self::Durable, Self::Mesh, Self::PubSub];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Durable => "durable",
            Self::Mesh => "mesh",
            Self::PubSub => "pubsub",
        }
    }

    /// Parse a configured name, listing the alternatives when it is not one.
    ///
    /// The error names every known transport because the caller who typed it
    /// wrongly is the caller who needs the list, and making them find this file
    /// to learn it is a poor trade for four words of message.
    pub fn parse(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == name)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "unknown transport {name:?}; this build has {}",
                    Self::ALL
                        .iter()
                        .map(|kind| kind.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }

    /// What this transport promises, without building one.
    ///
    /// The descriptor a constructed transport reports must agree with this;
    /// `the_advertised_guarantees_match_what_the_transport_reports` checks it,
    /// because a registry whose advertised guarantees drifted from the real
    /// ones would be worse than no registry at all.
    pub fn guarantees(self) -> TransportDescriptor {
        match self {
            Self::Local => TransportDescriptor {
                name: "local".to_string(),
                path: TransportPath::Local,
                durable: false,
                available: true,
                production_requirement: None,
            },
            Self::Durable => TransportDescriptor {
                name: "durable".to_string(),
                path: TransportPath::Durable,
                durable: true,
                available: true,
                production_requirement: None,
            },
            Self::Mesh => TransportDescriptor {
                name: "mesh".to_string(),
                path: TransportPath::Durable,
                // In-memory queue, inbox and dead letters on both ends. The
                // path says which routing class may travel here; it is not a
                // claim that anything survives a restart.
                durable: false,
                available: true,
                production_requirement: None,
            },
            Self::PubSub => TransportDescriptor {
                name: "pubsub".to_string(),
                path: TransportPath::Durable,
                durable: true,
                available: false,
                production_requirement: Some(
                    "a gRPC client, a TLS stack and a Google auth flow, none of which this \
                     build has; ADR 0011 replaced this transport with the in-tree mesh"
                        .to_string(),
                ),
            },
        }
    }

    /// Whether this transport may carry that class of event.
    ///
    /// Defers to [`RoutingClass::path`] rather than restating it. Each class
    /// has exactly one permissible path, in both directions, and the
    /// transports enforce the same rule at publish time — this only moves the
    /// refusal to configuration, where it is cheaper to discover.
    pub fn may_carry(self, class: RoutingClass) -> bool {
        self.guarantees().path == class.path()
    }
}

/// How to reach a transport that needs more than a name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportConfig {
    pub kind: TransportKind,
    /// The name receipts are stamped with.
    pub name: String,
    /// Bound on an in-process queue. Ignored by the transports that have none.
    #[serde(default = "default_capacity")]
    pub capacity: usize,
    /// `host:port` of the peer, for [`TransportKind::Mesh`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    /// Topic and subscription, for [`TransportKind::PubSub`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<PubSubBinding>,
}

/// How many exhausted envelopes the default sink keeps.
///
/// Bounded, and it evicts the oldest under sustained failure rather than
/// growing. A deployment that must not lose a dead letter supplies its own
/// sink; this default is for the case where losing the oldest is preferable to
/// losing the process.
const DEAD_LETTER_CAPACITY: usize = 1024;

const fn default_capacity() -> usize {
    1024
}

impl TransportConfig {
    pub fn new(kind: TransportKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
            capacity: default_capacity(),
            peer: None,
            binding: None,
        }
    }

    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    pub fn with_peer(mut self, peer: impl Into<String>) -> Self {
        self.peer = Some(peer.into());
        self
    }

    pub fn with_binding(mut self, binding: PubSubBinding) -> Self {
        self.binding = Some(binding);
        self
    }
}

/// Build a publisher from configuration.
///
/// Refuses an unavailable transport rather than substituting a working one.
pub fn publisher(config: &TransportConfig) -> Result<Box<dyn Publisher>> {
    refuse_if_unavailable(config.kind)?;
    match config.kind {
        TransportKind::Local => Ok(Box::new(LocalQueue::new(&config.name, config.capacity)?)),
        TransportKind::Durable => Ok(Box::new(DurableLogTransport::in_memory(&config.name))),
        TransportKind::Mesh => Ok(Box::new(mesh_transport(config)?)),
        TransportKind::PubSub => Err(unreachable_transport(config.kind)),
    }
}

/// Build a subscriber from configuration. Same refusals as [`publisher`].
pub fn subscriber(config: &TransportConfig) -> Result<Box<dyn Subscriber>> {
    refuse_if_unavailable(config.kind)?;
    match config.kind {
        TransportKind::Local => Ok(Box::new(LocalQueue::new(&config.name, config.capacity)?)),
        TransportKind::Durable => Ok(Box::new(DurableLogTransport::in_memory(&config.name))),
        TransportKind::Mesh => Ok(Box::new(mesh_transport(config)?)),
        TransportKind::PubSub => Err(unreachable_transport(config.kind)),
    }
}

/// Build a publisher for a routing class, refusing a transport too weak for it.
///
/// The check a caller would otherwise have to remember: a `Warm` or `Cold`
/// event must not be lost, and the local path is lossy by design.
pub fn select_for(config: &TransportConfig, class: RoutingClass) -> Result<Box<dyn Publisher>> {
    if !config.kind.may_carry(class) {
        return Err(Error::denied(format!(
            "a {} event travels the {} path and the {} transport is the {} path; {}",
            class.as_str(),
            class.path().as_str(),
            config.kind.as_str(),
            config.kind.guarantees().path.as_str(),
            if class.path() == TransportPath::Durable {
                "the local queue drops its oldest entry under load and nothing routed \
                 warm or cold is replaceable"
            } else {
                "an append and a hash on the venue-critical path is the latency the hot \
                 class exists to avoid"
            }
        )));
    }
    publisher(config)
}

fn refuse_if_unavailable(kind: TransportKind) -> Result<()> {
    let guarantees = kind.guarantees();
    if guarantees.available {
        return Ok(());
    }
    Err(unreachable_transport(kind))
}

fn unreachable_transport(kind: TransportKind) -> Error {
    let guarantees = kind.guarantees();
    Error::unavailable(format!(
        "the {} transport is not usable in this build: {}. It is not substituted with a \
         working transport, because a service that quietly published somewhere else would \
         pass every smoke test and deliver nothing",
        kind.as_str(),
        guarantees
            .production_requirement
            .unwrap_or_else(|| "no reason was recorded".to_string())
    ))
}

fn mesh_transport(config: &TransportConfig) -> Result<MeshTransport> {
    let peer = config.peer.as_ref().ok_or_else(|| {
        Error::invalid(
            "the mesh transport needs a peer address; without one there is nothing to reach",
        )
    })?;
    let mesh = MeshConfig::new(&config.name, peer).with_queue_capacity(config.capacity);
    MeshTransport::connect(
        mesh,
        Arc::new(SystemClock),
        Arc::new(ThreadSleeper),
        Box::new(MemoryDeadLetters::new(DEAD_LETTER_CAPACITY)),
    )
}
