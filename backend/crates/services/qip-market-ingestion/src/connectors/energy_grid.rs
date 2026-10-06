//! Energy and power grid data from transmission operators and utilities.
//!
//! This family covers real-time power generation, demand, price, frequency,
//! and grid stability metrics from independent system operators (ISOs) and
//! transmission operators, used for energy market analysis.
//!
//! # Status: PARTIAL
//!
//! Free public sources exist but with constraints:
//!
//! - **US EIA (Energy Information Administration):** Free API for electricity data
//!   but with 2-4 hour publication lag, insufficient for intraday trading and
//!   subject to ADR 0034 licensing review of public data terms.
//!
//! - **ISO/RTO public feeds:** Most US independent system operators (CAISO,
//!   ERCOT, PJM) publish real-time data, but each requires:
//!   - Egress infrastructure access (C3)
//!   - Individual vendor terms evaluation (ADR 0034)
//!   - Separate implementations per operator
//!
//! Production implementation is blocked on:
//! - C3: Outbound access to operator APIs (multiple regional endpoints)
//! - ADR 0034: Licensing terms for each operator's data
//!
//! A basic skeleton could implement one operator (e.g., CAISO) as a proof,
//! but would require regional cell egress capability.

use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::error::Result;

/// Placeholder for energy grid connector.
///
/// Not implemented in this build. Free ISOs exist (CAISO, ERCOT, PJM) but
/// require egress infrastructure (C3) and licensing review (ADR 0034).
#[derive(Clone, Debug)]
pub struct EnergyGridConnector;

impl EnergyGridConnector {
    pub const SOURCE_ID: &str = "energy-grid";
    pub const DATASET: &str = "energy.grid_metrics";
    pub const REGION: &str = "US";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        Err(qip_core::error::Error::invalid(format!(
            "{} is not implemented in this build; free ISO feeds exist (CAISO, ERCOT, PJM) but require egress infrastructure (C3) and licensing review (ADR 0034)",
            Self::SOURCE_ID
        )))
    }
}

impl SourceConnector for EnergyGridConnector {
    fn manifest(&self) -> &SourceManifest {
        unreachable!("EnergyGridConnector is not implemented")
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
