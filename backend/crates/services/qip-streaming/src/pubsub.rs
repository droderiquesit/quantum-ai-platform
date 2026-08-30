//! The Google Pub/Sub transport, which this build cannot supply.
//!
//! The target architecture puts Pub/Sub between the ingestion edge and the
//! central plane. Reaching it needs a gRPC client, a TLS stack and a Google
//! auth flow — dependencies `docs/adr/0009-tiered-dependency-policy.md` permits
//! at the I/O edge, and which this phase does not take. So this is a port: it
//! declares the shape production will fill and returns
//! [`qip_core::Error::Unavailable`] naming every piece that is missing.
//!
//! # It never falls back
//!
//! A deployment configured for Pub/Sub does not quietly get
//! [`crate::durable::DurableLogTransport`] instead. That is the hazard
//! `qip-storage` documents and `qip-mesh` honours: a service that silently
//! served a local file would pass every smoke test, publish to nothing, and be
//! discovered by whoever was waiting for the messages a week later. Failing at
//! the first call means the misconfiguration is found by whoever deployed it.
//!
//! # It never fakes a connection
//!
//! There is no in-memory "Pub/Sub emulator" here, and no queue behind this type
//! pretending to be a topic. A fake that behaved like the real thing under test
//! would be a claim that the integration works, and the only honest thing this
//! build can say about that integration is that it has never been run.

use qip_contracts::time::Watermark;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::envelope::StreamEnvelope;
use crate::ports::{PublishReceipt, Publisher, Subscriber, TransportDescriptor};
use crate::routing::TransportPath;

/// The names a deployment intends to use, and nothing that can connect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PubSubBinding {
    /// The topic the publisher would write to.
    pub topic: String,
    /// The subscription the consumer would pull from.
    ///
    /// A *pull* subscription specifically. A push subscription would deliver
    /// over HTTP on Google's schedule, which is the model
    /// [`crate::ports::Subscriber`] exists to avoid — see that module for why
    /// the caller has to keep the clock.
    pub subscription: String,
    /// The GCP project. Always `None` in this build; there is no project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl PubSubBinding {
    pub fn new(topic: impl Into<String>, subscription: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            subscription: subscription.into(),
            project: None,
        }
    }
}

/// A publisher and subscriber for Google Pub/Sub, awaiting everything it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PubSubTransport {
    binding: PubSubBinding,
}

impl PubSubTransport {
    /// The four things production must supply beyond the topic and
    /// subscription names, each phrased as something an operator can go and do.
    pub const REQUIREMENTS: [&'static str; 4] = [
        "a GCP project id, which this build does not have and does not default",
        "the Pub/Sub topic and pull subscription provisioned in that project, \
         with a dead-letter topic and a retry policy",
        "a gRPC client with TLS to pubsub.googleapis.com, which is a dependency \
         this phase does not take (see docs/adr/0009-tiered-dependency-policy.md)",
        "a workload-identity binding from this service's Kubernetes service \
         account to a Google service account holding roles/pubsub.publisher and \
         roles/pubsub.subscriber",
    ];

    /// Declare the binding a deployment intends. Connects to nothing.
    pub fn declared(binding: PubSubBinding) -> Self {
        Self { binding }
    }

    pub fn binding(&self) -> &PubSubBinding {
        &self.binding
    }

    /// Everything still missing, in the order an operator would supply it.
    ///
    /// Returned as data as well as prose so a start-up check can render the
    /// whole list rather than discovering it one failed call at a time — the
    /// same reason `qip_mesh::MeshProvider::describe` exists.
    pub fn missing_configuration(&self) -> Vec<String> {
        let mut missing = vec![format!(
            "{} (the topic this deployment declared is {}, the subscription is {})",
            Self::REQUIREMENTS[0],
            self.binding.topic,
            self.binding.subscription
        )];
        missing.extend(Self::REQUIREMENTS[1..].iter().map(|s| (*s).to_string()));
        missing
    }

    /// The error every operation on this transport returns.
    pub fn unavailable(&self, operation: &str) -> Error {
        Error::unavailable(format!(
            "the Google Pub/Sub transport cannot {operation}: it is a port with no client in this \
             binary. It requires: {}. This deployment will not fall back to the local durable log, \
             because a service that silently published to a file would pass its smoke tests and \
             deliver nothing. See docs/operations/external-dependencies.md",
            self.missing_configuration().join("; ")
        ))
    }

    fn descriptor_inner(&self) -> TransportDescriptor {
        TransportDescriptor {
            name: format!("pubsub:{}", self.binding.topic),
            path: TransportPath::Durable,
            durable: true,
            available: false,
            production_requirement: Some(self.missing_configuration().join("; ")),
        }
    }
}

impl Publisher for PubSubTransport {
    fn descriptor(&self) -> TransportDescriptor {
        self.descriptor_inner()
    }

    fn publish(&mut self, _envelope: StreamEnvelope, _at: Timestamp) -> Result<PublishReceipt> {
        Err(self.unavailable("publish"))
    }

    fn publish_batch(
        &mut self,
        _envelopes: Vec<StreamEnvelope>,
        _at: Timestamp,
    ) -> Result<Vec<PublishReceipt>> {
        Err(self.unavailable("publish a batch"))
    }
}

impl Subscriber for PubSubTransport {
    fn descriptor(&self) -> TransportDescriptor {
        self.descriptor_inner()
    }

    fn poll(&mut self, _until: Timestamp) -> Result<Vec<StreamEnvelope>> {
        Err(self.unavailable("pull"))
    }

    /// No watermark, because nothing has ever been consumed.
    ///
    /// `None` rather than a zero watermark: a zero would be a claim that the
    /// subscription has been read up to position zero, and a consumer resuming
    /// against it would believe it had missed nothing.
    fn watermark(&self) -> Option<Watermark> {
        None
    }

    fn has_pending(&self, _until: Timestamp) -> bool {
        false
    }
}
