//! Satellite imagery and geospatial data from commercial providers.
//!
//! This family covers satellite-based observations for Earth surface monitoring,
//! including imagery, spectral data, and derived indices (NDVI, temperature, etc.)
//! used for supply chain and commodity analysis.
//!
//! # Status: BLOCKED
//!
//! All production satellite data providers require:
//! - Paid commercial subscriptions (Maxar, Planet Labs, Copernicus, etc.)
//! - Account credentials and API keys
//! - Significant computational infrastructure for processing raw imagery
//!
//! Public satellite sources (Landsat, Sentinel-2) exist but:
//! - Require specialized imagery processing infrastructure
//! - Have 5-30 day latency unsuitable for trading
//! - Are blocked by C3 infrastructure constraints
//!
//! This connector is a placeholder documenting the family. No implementation
//! exists until satellite data terms are evaluated (ADR 0034) and C3 provides
//! the processing infrastructure.

use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::error::Result;

/// Placeholder for geospatial/satellite connector.
///
/// Not implemented in this build.
#[derive(Clone, Debug)]
pub struct GeospatialSatelliteConnector;

impl GeospatialSatelliteConnector {
    pub const SOURCE_ID: &str = "geospatial-satellite";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is not implemented in this build; satellite data sources require C3 infrastructure and ADR 0034 licensing review",
            Self::SOURCE_ID
        )))
    }
}

impl SourceConnector for GeospatialSatelliteConnector {
    fn manifest(&self) -> &SourceManifest {
        unreachable!("GeospatialSatelliteConnector is not implemented")
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
