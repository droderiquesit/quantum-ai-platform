//! Port congestion, berthing delays, and container terminal utilization.
//!
//! Port data includes vessel queues, berthing schedules, throughput metrics,
//! and container dwell times at major ports.
//!
//! # Status: BLOCKED
//!
//! Production port data sources require:
//! - Paid terminal operator subscriptions (private gateways, not public APIs)
//! - Port authority credentials and authentication
//! - Commercial shipping company accounts
//!
//! No free public port API exists. Even Vessel Finder and similar services
//! require API subscription with non-trading use restrictions.
//!
//! Blocked until ADR 0034 evaluates commercial terms.

use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::error::Result;

#[derive(Clone, Debug)]
pub struct PortCongestionConnector;

impl PortCongestionConnector {
    pub const SOURCE_ID: &str = "port-congestion";
    pub const DATASET: &str = "logistics.port_metrics";
    pub const REGION: &str = "Global";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is blocked: all sources are private terminal operator subscriptions",
            Self::SOURCE_ID
        )))
    }
}

impl SourceConnector for PortCongestionConnector {
    fn manifest(&self) -> &SourceManifest {
        unreachable!("PortCongestionConnector is not implemented")
    }

    fn decode(&self, _payload: &serde_json::Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is blocked",
            Self::SOURCE_ID
        )))
    }

    fn map(
        &self,
        _event: &RawEvent,
        _ingest_time: qip_core::Timestamp,
    ) -> Result<crate::adapter::SensedRecord> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is blocked",
            Self::SOURCE_ID
        )))
    }
}
