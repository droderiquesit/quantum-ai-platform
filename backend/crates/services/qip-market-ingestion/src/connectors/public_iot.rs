//! Public IoT sensor networks and environmental monitoring data.
//!
//! This family covers real-time data from distributed IoT sensor networks,
//! environmental monitoring stations, and public sensor APIs (weather,
//! air quality, water quality, seismic activity, etc.) used for physical
//! world signals and alternative data analysis.
//!
//! # Status: PARTIAL
//!
//! Free public sources exist but fragmented:
//!
//! - **NOAA/NWS:** Weather observations already connected via
//!   [`crate::connectors::nws_station_observations`]; covers temperature,
//!   precipitation, wind at 200+ stations with quality grades.
//!
//! - **OpenWeatherMap, Weather.com:** Free APIs available but require
//!   licensing review (ADR 0034); subject to rate limits and commercial
//!   terms constraints.
//!
//! - **Air Quality (OpenAQ, EPA):** Free public data with attribution, but
//!   sparse coverage and varying update frequencies unsuitable for trading
//!   without significant aggregation and interpolation work.
//!
//! - **USGS/Earthquake Hazards:** Real-time earthquake and seismic monitoring,
//!   public API, but low predictive value for equities and limited regional
//!   reach.
//!
//! Production implementation is blocked on:
//! - Proof of material predictive signal over alternative data class
//! - C3: Egress access for multiple sensor aggregators (not yet deployed)
//! - ADR 0034: Terms evaluation for each free/freemium sensor provider
//!
//! The single-source pattern (NWS) demonstrates the framework; expanding
//! to multiple networks requires infrastructure and signal validation.

use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::error::Result;

/// Placeholder for public IoT sensor network connector.
///
/// Not implemented in this build. Free sensor networks exist (OpenWeatherMap,
/// EPA, USGS) but require egress infrastructure (C3), licensing review
/// (ADR 0034), and signal validation before expanding beyond NWS weather.
#[derive(Clone, Debug)]
pub struct PublicIotConnector;

impl PublicIotConnector {
    pub const SOURCE_ID: &str = "public-iot";
    pub const DATASET: &str = "environment.public_sensors";
    pub const REGION: &str = "Global";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is not implemented in this build; free sensor networks exist (OpenWeatherMap, OpenAQ, USGS) but require egress infrastructure (C3), licensing review (ADR 0034), and signal validation",
            Self::SOURCE_ID
        )))
    }
}

impl SourceConnector for PublicIotConnector {
    fn manifest(&self) -> &SourceManifest {
        unreachable!("PublicIotConnector is not implemented")
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
