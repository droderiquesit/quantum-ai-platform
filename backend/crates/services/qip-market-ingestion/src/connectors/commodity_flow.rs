//! Commodity movement and logistics flow tracking data.
//!
//! This family covers tracking of commodity shipments, container movements,
//! border crossings, and supply chain logistics data used for demand signals
//! and delivery-chain analysis.
//!
//! # Status: BLOCKED
//!
//! All production commodity flow data sources require:
//! - Paid subscriptions to logistics aggregators (Flexport, Project44, FourKites, etc.)
//! - EDI access to carrier and customs systems (X.12, EDIFACT)
//! - Proprietary shipper partnerships
//!
//! Public sources (Panjiva, UN Comtrade) exist but:
//! - Have 30-90 day publication lag (unsuitable for trading)
//! - Aggregate customs manifests only (not real-time flow)
//! - Require licensing review (ADR 0034)
//!
//! This connector is a placeholder documenting the family. No implementation
//! exists until terms are evaluated and production timeliness is demonstrated.

use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::error::Result;

/// Placeholder for commodity flow connector.
///
/// Not implemented in this build.
#[derive(Clone, Debug)]
pub struct CommodityFlowConnector;

impl CommodityFlowConnector {
    pub const SOURCE_ID: &str = "commodity-flow";
    pub const DATASET: &str = "logistics.commodity_movements";
    pub const REGION: &str = "Global";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is blocked: production commodity flow requires paid subscriptions (Flexport, Project44, FourKites) with 30-90 day publication lag unsuitable for trading",
            Self::SOURCE_ID
        )))
    }
}

impl SourceConnector for CommodityFlowConnector {
    fn manifest(&self) -> &SourceManifest {
        unreachable!("CommodityFlowConnector is not implemented")
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
