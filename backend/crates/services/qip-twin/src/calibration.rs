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
//! Since ADR 0450 the observation is the twin's *charged cost* under the
//! uncalibrated model against what the venue actually cost, split into a
//! spread term and an impact term fitted in the twin's own impact units —
//! see [`FillCalibration::observe_cost`] for why each of those replaced
//! what came before. Queue position and acknowledgement latency are not
//! calibrated here: the simulated venue reports neither, and the twin's
//! cost model has no term for either (ADR 0450, "What remains").
//!
//! Under ADR 0003 the "live telemetry" is the platform's own paper fills.
//! The calibration is no less a calibration for that; it is a calibration to
//! the simulator the platform actually trades against.

use qip_core::error::{Error, Result};
use qip_financial::costs::MAX_TRADE_COST_BPS;
use qip_simulation_engine::costs::CostModel;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The step each observation moves the bias by. A fifth: half the weight of
/// a fill is gone after three more, so one outlier cannot reprice a venue,
/// and a persistent error is absorbed inside a working day of fills.
pub const DEFAULT_LEARNING_RATE: f64 = 0.2;

/// The exponentially weighted sufficient statistics of a venue's sized
/// fills — weight, the impact unit (`root`, named for the square root of
/// participation it carries), its square, the error and their product — from which the impact slope is a closed-form least-squares
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

/// The weighted variance of the impact unit below which the sized fills
/// are, for the twin's purposes, one size: a standard deviation of the unit
/// under five hundredths (at the reference volatility, root participation
/// under five percent). Below it the slope is
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
    /// The part of the cost error that grows with the twin's impact unit
    /// (`CostModel::impact_unit`: volatility over the reference times the
    /// square root of participation in the window's mean volume), in basis
    /// points per unit (R12-TICK-2080) — the unit the impact coefficient is
    /// quoted in, so this is the coefficient's error, signed as `bias_bps`
    /// is. Zero until a fill arrives with its unit; a fill without one moves
    /// the bias alone, because an error with no size beside it cannot be
    /// split between spread and impact without guessing.
    #[serde(default)]
    pub impact_slope_bps: f64,
    /// Fills observed with their impact unit, the only ones that can move
    /// `impact_slope_bps`.
    #[serde(default)]
    pub participation_observations: u64,
    /// The sized fills' statistics the slope is fitted from.
    #[serde(default)]
    pub sizes: SizeStatistics,
    /// Observations at which a fitted slope or a moved bias was refused
    /// because it would have carried the base impact coefficient or
    /// half-spread to `MAX_TRADE_COST_BPS` or past it.
    #[serde(default)]
    pub refusals: u64,
}

impl VenueCalibration {
    /// How far the calibration has had to move the twin at this venue, in
    /// basis points of notional: the larger of the spread correction and
    /// the impact correction at the venue's own mean impact unit
    /// (R12-RES-7104, R12-TICK-2080). Read by the kernel's misfit test.
    /// The two are not netted: a twin twenty basis points too cheap on
    /// spread and twenty too dear on impact is wrong twice, not right once,
    /// and a misfit read from `bias_bps` alone let a size-dependent misfit
    /// hide in the slope. NaN propagates, so a poisoned fit reads as misfit.
    pub fn correction_bps(&self) -> f64 {
        let impact = if self.sizes.weight > 0.0 {
            self.impact_slope_bps * self.sizes.root / self.sizes.weight
        } else {
            0.0
        };
        if self.bias_bps.is_nan() || impact.is_nan() {
            return f64::NAN;
        }
        self.bias_bps.abs().max(impact.abs())
    }
}

/// What one observation did to a venue's calibration.
#[derive(Clone, Debug, PartialEq)]
pub struct Observed {
    /// The venue's calibration after the observation.
    pub calibration: VenueCalibration,
    /// Whether a fitted slope or a moved bias was refused at this
    /// observation.
    pub refused: bool,
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

    /// Absorb one observed error for `venue` with no size beside it: the
    /// whole error is charged to the spread, because an error with no size
    /// cannot be split between spread and impact without guessing. Refuses
    /// a non-finite error rather than letting one NaN poison the venue's
    /// bias for ever.
    pub fn observe(&mut self, venue: &str, error_bps: f64) -> Result<VenueCalibration> {
        self.observe_cost(venue, &CostModel::frictionless(), error_bps, None)
            .map(|observed| observed.calibration)
    }

    /// Observe one fill as the twin's **charged cost against reality's**,
    /// in the twin's own impact units (R12-TICK-2080).
    ///
    /// `error_bps` is what `base` — the uncalibrated cost model, the one
    /// [`Self::cost_model`] is handed — charges for the venue's own fill, at
    /// the venue's own size and against the liquidity the twin priced with,
    /// less what the venue actually cost (its slippage from the order's
    /// arrival price plus the fees it billed), both in basis points of
    /// notional. Negative means the twin is cheaper than reality, as every
    /// error here is signed. `impact_unit` is
    /// [`CostModel::impact_unit`] at that size and liquidity: the factor
    /// `base`'s impact coefficient was multiplied by.
    ///
    /// Two choices make the fit close on the venue rather than chase itself,
    /// and each replaces a version that did not:
    ///
    /// * **Charged cost, not mark against fill.** The first version fed the
    ///   twin's printed entry price against the fill. That is the venue's
    ///   slippage, and it does not shrink as the twin's model improves, so
    ///   a base already near the venue had the slippage added on top of it
    ///   — every cost charged twice — and the fit never closed.
    /// * **Against `base`, not against the calibrated model.** An error
    ///   measured against the model the calibration itself produced is a
    ///   moving target; one measured against `base` is stationary, so the
    ///   fitted law is `base − reality` and [`Self::cost_model`]'s
    ///   subtraction lands on reality.
    ///
    /// The error is modelled as `bias + slope * impact_unit`. The slope is
    /// the exponentially weighted least-squares slope over the sized fills,
    /// held still while every sized fill is one size
    /// ([`SIZE_VARIANCE_FLOOR`]); the bias is the exponentially weighted
    /// mean of what the size law leaves over. A fitted slope that would
    /// carry `base`'s impact coefficient to [`MAX_TRADE_COST_BPS`] or past
    /// it is **refused** — the venue keeps its previous slope — and so is a
    /// bias that would carry `base`'s half-spread there; each refusal is
    /// counted on [`VenueCalibration::refusals`] and reported on the
    /// returned [`Observed`]. Adopting it would leave the
    /// venue `Unpriceable`, and an unpriceable trade is never scored, so the
    /// venue would never be calibrated again: a correction that ends
    /// correction.
    ///
    /// Refuses an error or unit that is not a finite number, and a negative
    /// unit, rather than let one poison the venue's fit for ever.
    pub fn observe_cost(
        &mut self,
        venue: &str,
        base: &CostModel,
        error_bps: f64,
        impact_unit: Option<f64>,
    ) -> Result<Observed> {
        if !error_bps.is_finite() {
            return Err(Error::invalid(format!(
                "a fill error of {error_bps} bps on {venue} is not a number the twin can learn from"
            )));
        }
        if let Some(unit) = impact_unit
            && !(unit.is_finite() && unit >= 0.0)
        {
            return Err(Error::invalid(format!(
                "an impact unit of {unit} on {venue} is not a size the twin prices impact at; \
                 the twin cannot learn a size law from it"
            )));
        }
        let rate = self.learning_rate;
        let entry = self.venues.entry(venue.to_string()).or_default();
        let mut refused = false;
        let residual = match impact_unit {
            Some(unit) => {
                let forget = 1.0 - rate;
                let sizes = &mut entry.sizes;
                sizes.weight = sizes.weight * forget + 1.0;
                sizes.root = sizes.root * forget + unit;
                sizes.root_squared = sizes.root_squared * forget + unit * unit;
                sizes.error = sizes.error * forget + error_bps;
                sizes.root_error = sizes.root_error * forget + unit * error_bps;
                let mean_unit = sizes.root / sizes.weight;
                let variance = sizes.root_squared / sizes.weight - mean_unit * mean_unit;
                if variance > SIZE_VARIANCE_FLOOR {
                    let covariance =
                        sizes.root_error / sizes.weight - mean_unit * (sizes.error / sizes.weight);
                    let slope = covariance / variance;
                    if slope.is_finite() && base.impact_coefficient_bps - slope < MAX_TRADE_COST_BPS
                    {
                        entry.impact_slope_bps = slope;
                    } else {
                        refused = true;
                    }
                }
                let spread_residual = error_bps - entry.impact_slope_bps * unit;
                entry.participation_observations += 1;
                spread_residual
            }
            None => error_bps,
        };
        // The spread is held to the same ceiling as the impact term, for the
        // same reason: a half-spread past it is a venue that never prices
        // again, so a bias that would carry it there is refused and the
        // previous bias kept.
        let bias = entry.bias_bps + rate * (residual - entry.bias_bps);
        if bias.is_finite() && base.half_spread_bps - bias < MAX_TRADE_COST_BPS {
            entry.bias_bps = bias;
        } else {
            refused = true;
        }
        if refused {
            entry.refusals += 1;
        }
        entry.observations += 1;
        Ok(Observed {
            calibration: entry.clone(),
            refused,
        })
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
