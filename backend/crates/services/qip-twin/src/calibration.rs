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

/// The exponentially weighted sufficient statistics of a venue's sized
/// fills — weight, `sqrt(participation)`, its square, the error and their
/// product — from which the impact slope is a closed-form least-squares
/// fit (R12-TICK-2080). Kept rather than the slope alone so the fit is
/// re-derived from every fill it rests on; a slope stepped toward each fill
/// took a thousand fills to reach a size law two fills can state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SizeStatistics {
    pub weight: f64,
    pub root: f64,
    pub root_squared: f64,
    pub error: f64,
    pub root_error: f64,
}

/// The weighted variance of `sqrt(participation)` below which the sized
/// fills are, for the twin's purposes, one size: a standard deviation of
/// root participation under five percent of a bar. Below it the slope is
/// leverage on noise rather than a size law — one-share fills on bars of
/// slightly different volume differ in root by a ten-thousandth, and a few
/// basis points of spread noise across that gap fitted as a slope of a
/// hundred thousand basis points per unit root, which sent the twin's
/// impact coefficient to zero or to the moon on the first two fills. The
/// slope holds still until the fills span real size.
pub const SIZE_VARIANCE_FLOOR: f64 = 0.0025;

/// One venue's calibration state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VenueCalibration {
    /// Exponentially weighted signed fill error, in basis points, with the
    /// kernel's sign: negative means the twin filled better than reality.
    pub bias_bps: f64,
    pub observations: u64,
    /// The part of the fill error that grows with the square root of the
    /// fill's participation in the bar's volume, in basis points per unit of
    /// `sqrt(participation)` (R12-TICK-2080): the twin's impact coefficient
    /// error, signed as `bias_bps` is. Zero until a fill arrives with its
    /// participation; a fill without one moves the bias alone, because an
    /// error with no size beside it cannot be split between spread and
    /// impact without guessing.
    #[serde(default)]
    pub impact_slope_bps: f64,
    /// Fills observed with their participation, the only ones that can move
    /// `impact_slope_bps`.
    #[serde(default)]
    pub participation_observations: u64,
    /// The sized fills' statistics the slope is fitted from.
    #[serde(default)]
    pub sizes: SizeStatistics,
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
        self.observe_with_participation(venue, error_bps, None)
    }

    /// Observe one fill's signed error with the fill's participation in the
    /// bar's volume, when the caller knows it (R12-TICK-2080).
    ///
    /// The error is modelled as `bias + slope * sqrt(participation)`, the
    /// square-root law the twin prices impact by. The slope is the
    /// exponentially weighted least-squares fit of the error on
    /// `sqrt(participation)` over the sized fills, held still while every
    /// sized fill is one size; the bias is the exponentially weighted mean
    /// of what the size law leaves over. With no participation the residual
    /// is the whole error and the slope is left alone: an error with no size
    /// beside it cannot be split between spread and impact. A
    /// participation outside `[0, 1]` or not a number is refused, like the
    /// error: a fill that took more than the bar traded is not a fill the
    /// twin can learn a size law from.
    pub fn observe_with_participation(
        &mut self,
        venue: &str,
        error_bps: f64,
        participation: Option<f64>,
    ) -> Result<&VenueCalibration> {
        if !error_bps.is_finite() {
            return Err(Error::invalid(format!(
                "a fill error of {error_bps} bps on {venue} is not a number the twin can learn from"
            )));
        }
        if let Some(participation) = participation
            && !(0.0..=1.0).contains(&participation)
        {
            return Err(Error::invalid(format!(
                "a fill participation of {participation} on {venue} is not a fraction of the \
                 bar's volume; the twin cannot learn a size law from it"
            )));
        }
        let rate = self.learning_rate;
        let entry = self.venues.entry(venue.to_string()).or_default();
        match participation {
            Some(participation) => {
                // The size law is fitted, not stepped toward: the slope is
                // the exponentially weighted least-squares slope of the error
                // on sqrt(participation) over the sized fills, and so is
                // exact after two fills of different size on a clean error
                // and never wanders where a gradient step would. The bias
                // then absorbs what the size law leaves over, so the two
                // never double-charge one error. One size alone cannot
                // separate spread from impact, and the slope holds still
                // until a second size arrives rather than guess from one.
                let root = participation.sqrt();
                let forget = 1.0 - rate;
                let sizes = &mut entry.sizes;
                sizes.weight = sizes.weight * forget + 1.0;
                sizes.root = sizes.root * forget + root;
                sizes.root_squared = sizes.root_squared * forget + root * root;
                sizes.error = sizes.error * forget + error_bps;
                sizes.root_error = sizes.root_error * forget + root * error_bps;
                let mean_root = sizes.root / sizes.weight;
                let variance = sizes.root_squared / sizes.weight - mean_root * mean_root;
                if variance > SIZE_VARIANCE_FLOOR {
                    let covariance =
                        sizes.root_error / sizes.weight - mean_root * (sizes.error / sizes.weight);
                    entry.impact_slope_bps = covariance / variance;
                }
                let spread_residual = error_bps - entry.impact_slope_bps * root;
                entry.bias_bps += rate * (spread_residual - entry.bias_bps);
                entry.participation_observations += 1;
            }
            None => {
                entry.bias_bps += rate * (error_bps - entry.bias_bps);
            }
        }
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
            model.half_spread_bps = (base.half_spread_bps - calibration.bias_bps).max(0.0);
            // The slope is the error per unit of sqrt(participation), which
            // is exactly the unit the impact coefficient is quoted in, so the
            // correction is the same subtraction the spread takes: a twin
            // that underpriced size (negative slope) pays more impact.
            model.impact_coefficient_bps =
                (base.impact_coefficient_bps - calibration.impact_slope_bps).max(0.0);
        }
        model
    }
}
