//! Automatic Identification System (AIS) maritime vessel tracking data.
//!
//! AIS broadcasts provide real-time vessel position, course, speed and destination
//! for cargo and tanker ships. Free public AIS feeds aggregate receivers across
//! coastlines and international waters.
//!
//! # Status: PARTIAL
//!
//! Free public AIS sources exist (AIS Hub, MarineTraffic API tier, etc.) and are
//! unauthenticated. However, production implementation is blocked on:
//!
//! - C3: Vessel identification and cargo manifest enrichment infrastructure
//! - ADR 0034: License terms for major aggregators (MarineTraffic requires
//!   evaluation of their terms and data use restrictions)
//! - Egress allowlist: No deployed process has outbound access to AIS providers
//!
//! A basic connector skeleton exists below, documenting the expected contract.
//! It would follow the [`crate::connectors::nws_station_observations`] pattern
//! but produce vessel tracking `AlternativeDataPoint` records instead of weather.

use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::error::Result;

/// Placeholder for AIS shipping connector.
///
/// Not implemented in this build. Free public sources exist but require
/// egress infrastructure (C3) and licensing terms evaluation (ADR 0034).
#[derive(Clone, Debug)]
pub struct ShippingAisConnector;

impl ShippingAisConnector {
    pub const SOURCE_ID: &str = "shipping-ais";
    pub const DATASET: &str = "shipping.vessel_positions";
    pub const REGION: &str = "Global";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is not implemented in this build; free AIS sources exist but require egress infrastructure (C3) and licensing review (ADR 0034)",
            Self::SOURCE_ID
        )))
    }
}

impl SourceConnector for ShippingAisConnector {
    fn manifest(&self) -> &SourceManifest {
        unreachable!("ShippingAisConnector is not implemented")
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
