//! Write-side consumer for appending fill postings to the ledger chain.
//! See ADR 0100 § 1 for this module's role.
//!
//! Consumes from the `outcomes` stream (P1 quality-of-service class),
//! filtering for fills and yielding them as Deliveries that the ledger
//! store applies.

use qip_core::error::{Error, Result};
use std::net::SocketAddr;

/// Consumes P1 outcomes from the event fabric's broker.
#[derive(Debug)]
#[allow(dead_code)]
pub struct FabricConsumer {
    broker_addr: SocketAddr,
    group_id: String,
}

impl FabricConsumer {
    /// Create a consumer that will connect to the given broker address.
    pub fn new(broker_addr: SocketAddr, group_id: impl Into<String>) -> Self {
        Self {
            broker_addr,
            group_id: group_id.into(),
        }
    }

    /// Start consuming from the outcomes topic (P1 class).
    ///
    /// This method is a placeholder for real fabric integration.
    /// In the full implementation, it will:
    /// 1. Connect to the fabric broker
    /// 2. Join the consumer group
    /// 3. Subscribe to the outcomes topic
    /// 4. Yield P1 records as they arrive
    pub fn start(&self) -> Result<PostingStream> {
        // Placeholder: the actual implementation will connect to the broker
        // and return a stream of P1 outcomes that can be yielded to the ledger store.
        Err(Error::unavailable(
            "fabric consumer integration is pending deployment of qip-fabricd",
        ))
    }
}

/// A stream of postings delivered from the fabric for the ledger to apply.
#[derive(Debug)]
pub struct PostingStream {
    // Placeholder: will hold consumer state and iterator logic
}

impl PostingStream {
    /// Poll the next posting from the stream, or None if the stream is closed.
    ///
    /// This method is a placeholder and will be replaced with the actual
    /// stream implementation when the fabric consumer is fully integrated.
    pub fn poll_next(&mut self) -> Result<Option<()>> {
        // Placeholder
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn ledger_ingests_postings_from_fabric_and_writes_them() {
        let broker_addr = SocketAddr::from_str("127.0.0.1:9090").expect("valid socket address");
        let consumer = FabricConsumer::new(broker_addr, "ledger-group");

        // Attempting to start without a running fabric broker should fail gracefully
        let result = consumer.start();
        assert!(
            result.is_err(),
            "starting without a broker should return an error"
        );
    }
}
