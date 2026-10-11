//! The execution twin, recalibrated from the platform's own fills
//! (R12-TICK-2080).
//!
//! The kernel charted how far the twin's entry price sat from each venue's
//! fill (`qip_venue_fill_error_bps`) and nothing read it: the twin priced
//! every counterfactual with the same fixed cost model however wrong it had
//! been, so a twin that was twenty basis points too generous on one venue
//! stayed twenty basis points too generous for ever, and every size it
//! scored on that venue was scored against fills that never existed.
//!
//! [`FillCalibration`] closes the loop. Each observed error moves a per-venue
//! bias toward it by an exponentially weighted step, and
//! [`FillCalibration::cost_model`] hands the twin a cost model whose
//! half-spread carries that bias. A twin that keeps filling better than
//! reality pays a wider spread on the next pricing; one that keeps filling
//! worse pays a narrower one, down to a zero spread and never below — a
//! negative half-spread is a venue that pays the platform to cross, which
//! no evidence here can establish.
//!
//! Under ADR 0003 the "live telemetry" is the platform's own paper fills.
//! The calibration is no less a calibration for that; it is a calibration to
//! the simulator the platform actually trades against.

use qip_core::error::{Error, Result};
use qip_simulation_engine::costs::CostModel;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The step each observation moves the bias by. A fifth: half the weight of
/// a fill is gone after three more, so one outlier cannot reprice a venue,
/// and a persistent error is absorbed inside a working day of fills.
pub const DEFAULT_LEARNING_RATE: f64 = 0.2;

/// One venue's calibration state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueCalibration {
    /// Exponentially weighted signed fill error, in basis points, with the
    /// kernel's sign: negative means the twin filled better than reality.
    pub bias_bps: f64,
    pub observations: u64,
}

/// Per-venue fill calibration for the counterfactual twin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillCalibration {
    learning_rate: f64,
    venues: BTreeMap<String, VenueCalibration>,
}

impl Default for FillCalibration {
    fn default() -> Self {
        Self {
            learning_rate: DEFAULT_LEARNING_RATE,
            venues: BTreeMap::new(),
        }
    }
}

impl FillCalibration {
    /// A calibration moving `learning_rate` of the way to each observation.
    /// Refuses a rate outside `(0, 1]`: zero is a calibration that never
    /// moves, and above one overshoots every observation.
    pub fn new(learning_rate: f64) -> Result<Self> {
        if !(learning_rate > 0.0 && learning_rate <= 1.0) {
            return Err(Error::invalid(format!(
                "a fill calibration's learning rate must lie in (0, 1]; {learning_rate} either \
                 never moves or overshoots"
            )));
        }
        Ok(Self {
            learning_rate,
            venues: BTreeMap::new(),
        })
    }

    /// Absorb one observed error for `venue`. Refuses a non-finite error
    /// rather than letting one NaN poison the venue's bias for ever.
    pub fn observe(&mut self, venue: &str, error_bps: f64) -> Result<&VenueCalibration> {
        if !error_bps.is_finite() {
            return Err(Error::invalid(format!(
                "a fill error of {error_bps} bps on {venue} is not a number the twin can learn from"
            )));
        }
        let rate = self.learning_rate;
        let entry = self
            .venues
            .entry(venue.to_string())
            .or_insert(VenueCalibration {
                bias_bps: 0.0,
                observations: 0,
            });
        entry.bias_bps += rate * (error_bps - entry.bias_bps);
        entry.observations += 1;
        Ok(entry)
    }

    /// The calibration held for `venue`, if any fill has been observed there.
    pub fn venue(&self, venue: &str) -> Option<&VenueCalibration> {
        self.venues.get(venue)
    }

    /// `base` with its half-spread moved by `venue`'s bias. A venue with no
    /// observation prices at `base` exactly.
    pub fn cost_model(&self, base: &CostModel, venue: &str) -> CostModel {
        let mut model = *base;
        if let Some(calibration) = self.venues.get(venue) {
            // A negative bias (twin too generous) widens the spread the twin
            // pays; a positive one narrows it, to zero and not below.
            model.half_spread_bps = (base.half_spread_bps - calibration.bias_bps).max(0.0);
        }
        model
    }
}
