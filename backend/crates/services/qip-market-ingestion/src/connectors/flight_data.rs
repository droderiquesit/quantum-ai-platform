//! Aviation flight data, delay information, and aircraft utilization tracking.
//!
//! Flight data includes scheduled/actual departure/arrival times, aircraft tail
//! numbers, and operational metrics from commercial aviation sources.
//!
//! # Status: BLOCKED
//!
//! All production aviation data requires:
//! - Paid subscriptions (FlightRadar24 API, Aviation Stack, ADS-B Exchange, etc.)
//! - Commercial pilot/airline account credentials
//! - Geofencing compliance for military/sensitive airspace
//!
//! The free tier of some providers (FlightRadar24, ADS-B Exchange) permits
//! non-commercial research only and explicitly forbid trading use.
//!
//! Blocked until ADR 0034 evaluates terms and C3 provides egress infrastructure.

use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::error::Result;

#[derive(Clone, Debug)]
pub struct FlightDataConnector;

impl FlightDataConnector {
    pub const SOURCE_ID: &str = "flight-data";
    pub const DATASET: &str = "aviation.flights";
    pub const REGION: &str = "Global";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is blocked: all production sources require paid accounts and non-commercial restrictions",
            Self::SOURCE_ID
        )))
    }
}

impl SourceConnector for FlightDataConnector {
    fn manifest(&self) -> &SourceManifest {
        unreachable!("FlightDataConnector is not implemented")
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
