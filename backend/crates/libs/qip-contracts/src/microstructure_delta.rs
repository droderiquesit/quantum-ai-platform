//! `MicrostructureDelta` (blueprint v12.0 §27, R12-CONTRACT-7074): what a
//! cell observed the venue do, against what its model said the venue would
//! do, sent back to the slow brain as residuals.
//!
//! The nearest thing before this was a histogram (`qip_venue_fill_error_bps`)
//! — a chart, not a record, and nothing could learn from it. A residual is
//! only useful to a learner if it says what was modelled, what was observed,
//! over how many samples and in which window; a delta missing any of those
//! is refused, because a residual nobody can weight is noise that looks like
//! signal.

use crate::validated::{Contract, Validated, after, listed, named, refuse};
use crate::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The six residual families §27 names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidualKind {
    FillProbability,
    Slippage,
    Impact,
    Liquidity,
    LeadLag,
    VenueBehaviour,
}

/// One modelled-versus-observed pair. Units are the kind's own (basis points
/// for slippage and impact, probability in basis points for fills, notional
/// for liquidity, nanoseconds for lead/lag); the pair shares one unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Residual {
    pub kind: ResidualKind,
    pub instrument: String,
    pub modelled: Decimal,
    pub observed: Decimal,
    /// How many observations the pair summarises; at least one.
    pub samples: u64,
}

impl Residual {
    /// Observed minus modelled. Signed: the sign is the finding.
    pub fn error(&self) -> Decimal {
        self.observed - self.modelled
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicrostructureDeltaFields {
    pub schema_version: u32,
    pub delta_id: String,
    pub cell: String,
    pub venue: VenueId,
    /// The model version the residuals are measured against.
    pub model_version: String,
    pub window_start: Timestamp,
    pub window_end: Timestamp,
    pub residuals: Vec<Residual>,
}

/// The §27 microstructure delta. Built only through [`Validated::new`].
pub type MicrostructureDelta = Validated<MicrostructureDeltaFields>;

impl Contract for MicrostructureDeltaFields {
    const NAME: &'static str = "MicrostructureDelta";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank id, cell, venue, model version or instrument; an
    /// empty or inverted window; no residual; a residual over zero samples;
    /// and two residuals of one kind for one instrument, which would let a
    /// learner count the same window twice.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "delta_id", &self.delta_id)?;
        named(c, "cell", &self.cell)?;
        named(c, "venue", self.venue.as_str())?;
        named(c, "model_version", &self.model_version)?;
        after(
            c,
            "window_end",
            self.window_end,
            "window_start",
            self.window_start,
        )?;
        listed(c, "residuals", &self.residuals)?;
        let mut seen = BTreeSet::new();
        for r in &self.residuals {
            named(c, "residuals.instrument", &r.instrument)?;
            if r.samples == 0 {
                return refuse(
                    c,
                    "carries a residual over zero samples; a pair nobody observed is invented",
                );
            }
            if !seen.insert((r.kind, r.instrument.as_str())) {
                return refuse(
                    c,
                    format!(
                        "carries two {:?} residuals for {}; state one per kind and instrument",
                        r.kind, r.instrument
                    ),
                );
            }
        }
        Ok(())
    }
}
