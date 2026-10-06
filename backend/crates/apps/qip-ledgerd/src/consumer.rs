//! Write-side consumer for appending fill postings to the ledger chain.
//! See ADR 0100 § 1 for this module's role.
//!
//! Consumes from the `outcomes` stream (P1 quality-of-service class),
//! filtering for fills and yielding them as Deliveries that the ledger
//! store applies.
//!
//! Implemented in SLICE-31.

use qip_core::error::Result;

/// Consumes P1 outcomes from the event fabric's broker.
///
/// This is a doc stub pending SLICE-31. It will connect to the fabric broker
/// and stream P1 outcome records to the ledger for processing.
pub struct FabricConsumer;

impl FabricConsumer {
    /// Start consuming from the outcomes topic (P1 class).
    pub fn start(&self) -> Result<()> {
        unimplemented!("fabric consumer integration is implemented in SLICE-31")
    }
}
