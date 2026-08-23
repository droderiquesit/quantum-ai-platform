//! The router that sends each event down the path its class names.
//!
//! There is no policy here beyond `envelope.transport_path()`. That is the
//! point: the decision was already made and checked when the envelope was
//! sealed, so this type cannot make a different one. A hot event reaches the
//! local queue and the durable publisher is not called at all — not called
//! rather than called and skipped, which is why routing a hot event succeeds
//! even when the durable transport is entirely unavailable.

use qip_core::Timestamp;
use qip_core::error::Result;
use serde::{Deserialize, Serialize};

use crate::envelope::StreamEnvelope;
use crate::local::LocalQueue;
use crate::ports::{PublishReceipt, Publisher, TransportDescriptor};
use crate::routing::TransportPath;

/// How many events took each path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutedCounts {
    pub local: u64,
    pub durable: u64,
}

/// A local queue and a durable publisher, with the class deciding between them.
#[derive(Debug)]
pub struct TieredPublisher {
    local: LocalQueue,
    durable: Box<dyn Publisher>,
    counts: RoutedCounts,
}

impl TieredPublisher {
    pub fn new(local: LocalQueue, durable: Box<dyn Publisher>) -> Self {
        Self {
            local,
            durable,
            counts: RoutedCounts::default(),
        }
    }

    /// Publish onto whichever transport the envelope's class names.
    pub fn route(&mut self, envelope: StreamEnvelope, at: Timestamp) -> Result<PublishReceipt> {
        match envelope.transport_path() {
            TransportPath::Local => {
                let receipt = self.local.publish(envelope, at)?;
                self.counts.local += 1;
                Ok(receipt)
            }
            TransportPath::Durable => {
                let receipt = self.durable.publish(envelope, at)?;
                self.counts.durable += 1;
                Ok(receipt)
            }
        }
    }

    /// Route a batch, reporting the first failure and what it was routing.
    ///
    /// Stops rather than continuing past an error, for the reason
    /// [`Publisher::publish_batch`] gives: a partially published batch with an
    /// error at the end leaves the caller unable to say which half landed.
    pub fn route_batch(
        &mut self,
        envelopes: Vec<StreamEnvelope>,
        at: Timestamp,
    ) -> Result<Vec<PublishReceipt>> {
        let mut receipts = Vec::with_capacity(envelopes.len());
        for envelope in envelopes {
            receipts.push(self.route(envelope, at)?);
        }
        Ok(receipts)
    }

    pub fn counts(&self) -> RoutedCounts {
        self.counts
    }

    pub fn local(&self) -> &LocalQueue {
        &self.local
    }

    pub fn local_mut(&mut self) -> &mut LocalQueue {
        &mut self.local
    }

    pub fn durable(&self) -> &dyn Publisher {
        self.durable.as_ref()
    }

    pub fn durable_mut(&mut self) -> &mut dyn Publisher {
        self.durable.as_mut()
    }

    /// Both transports, for a start-up log line.
    pub fn descriptors(&self) -> [TransportDescriptor; 2] {
        [self.local.descriptor(), self.durable.descriptor()]
    }
}
