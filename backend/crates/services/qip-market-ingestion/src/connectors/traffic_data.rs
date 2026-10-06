//! Traffic flow and congestion data from transportation networks.
//!
//! This family covers real-time traffic conditions, congestion levels,
//! incident reports, and routing data used for supply chain and logistics analysis.
//!
//! # Status: BLOCKED
//!
//! All production traffic data sources require:
//! - Paid commercial subscriptions (HERE, TomTom, Google Maps Platform, etc.)
//! - API keys and account credentials
//! - Commercial terms compliance for trading use
//!
//! Free tier sources (OpenStreetMap, OSRM) exist but:
//! - Provide no real-time traffic conditions
//! - Have no incident/congestion data
//! - Require hosted infrastructure for meaningful latency
//!
//! This connector is a placeholder documenting the family. No implementation
//! exists until traffic data licensing terms are evaluated (ADR 0034) and
//! a cost decision is made on commercial subscriptions.

use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::error::Result;

/// Placeholder for traffic data connector.
///
/// Not implemented in this build.
#[derive(Clone, Debug)]
pub struct TrafficDataConnector;

impl TrafficDataConnector {
    pub const SOURCE_ID: &str = "traffic-data";
    pub const DATASET: &str = "logistics.traffic_flow";
    pub const REGION: &str = "Global";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is blocked: all production traffic sources require paid commercial subscriptions (HERE, TomTom, Google Maps Platform)",
            Self::SOURCE_ID
        )))
    }
}

impl SourceConnector for TrafficDataConnector {
    fn manifest(&self) -> &SourceManifest {
        unreachable!("TrafficDataConnector is not implemented")
    }

    fn decode(&self, _payload: &serde_json::Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is not implemented in this build",
            Self::SOURCE_ID
        )))
    }

    fn map(
        &self,
        _event: &RawEvent,
        _ingest_time: qip_core::Timestamp,
    ) -> Result<crate::adapter::SensedRecord> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is not implemented in this build",
            Self::SOURCE_ID
        )))
    }
}
