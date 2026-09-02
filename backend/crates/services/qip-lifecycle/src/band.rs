//! The holdout band: what "inside its holdout band" means, as a value.
//!
//! The Phase 3 gate (blueprint §51.1) asks whether a strategy survives a
//! live venue *inside its holdout band*, and ADR 0023 records that no band
//! was defined anywhere in the tree — step 9 had nothing to be inside of.
//! This module defines it as an output of validation: the holdout gate
//! produces a [`HoldoutBand`] from the same [`DeflatedSharpe`] it admits on,
//! the ledger carries it on the holdout admission, and the demotion monitor
//! judges live returns against it.
//!
//! The band is an interval on the annualised Sharpe ratio, centred on the
//! holdout's figure and `z` standard errors wide, with the standard error
//! taken under the same non-normal correction `deflated_sharpe` applies —
//! negative skew and fat tails widen it. A live figure is judged with its
//! own estimation error added in quadrature, because thirty days of live
//! returns carry a far wider error than a year of holdout, and a band that
//! ignored it would demote nearly every real strategy for noise. The band
//! is two-sided on purpose: a live result far *above* the holdout is not
//! good news, it is a different strategy or a data fault, and either is a
//! reason to stop and look.
//!
//! What is recorded with the bounds is what makes them reproducible: the
//! method, the instant, the observations, and the lifetime trial count the
//! validation was corrected against.

use crate::scoring;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_numerics::stats;
use qip_simulation_engine::validation::DeflatedSharpe;
use serde::{Deserialize, Serialize};

/// How a band's bounds were derived from the holdout.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum BandMethod {
    /// `centre ± z · standard_error`, with the standard error of the Sharpe
    /// ratio under non-normal returns (Bailey and López de Prado), and the
    /// live figure's own error added in quadrature when judged.
    SharpeStandardError { confidence: f64, z: f64 },
}

impl BandMethod {
    /// The two-sided 95% method: `z = Φ⁻¹(0.975)`.
    pub const NINETY_FIVE: Self = Self::SharpeStandardError {
        confidence: 0.95,
        z: 1.959_963_984_540_054,
    };

    pub fn z(self) -> f64 {
        match self {
            Self::SharpeStandardError { z, .. } => z,
        }
    }

    pub fn describe(self) -> String {
        match self {
            Self::SharpeStandardError { confidence, z } => format!(
                "Sharpe standard error, {:.0}% two-sided (z = {z:.3})",
                confidence * 100.0
            ),
        }
    }
}

/// Standard error of a per-period Sharpe ratio under non-normal returns.
///
/// `sqrt((1 − γ₃·SR + ¼·γ₄·SR²) / (n − 1))`, floored where skew and
/// kurtosis would drive it below zero. This is the formula inside
/// [`qip_simulation_engine::validation::deflated_sharpe`], stated here
/// because the engine does not expose it; the tidy home is an accessor on
/// [`DeflatedSharpe`], and until it exists this is the one restatement.
pub fn sharpe_standard_error(
    periodic_sharpe: f64,
    skewness: f64,
    excess_kurtosis: f64,
    observations: usize,
) -> Result<f64> {
    if observations < 2 {
        return Err(Error::invalid(format!(
            "{observations} observation(s) give a Sharpe ratio no standard error"
        )));
    }
    let n = observations as f64;
    let variance = (1.0 - skewness * periodic_sharpe
        + 0.25 * excess_kurtosis * periodic_sharpe * periodic_sharpe)
        / (n - 1.0);
    let standard_error = variance.max(1e-18).sqrt();
    if !standard_error.is_finite() {
        return Err(Error::numeric(
            "the Sharpe standard error is not finite; the inputs are not numbers",
        ));
    }
    Ok(standard_error)
}

/// The interval a strategy's live Sharpe must stay inside.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoldoutBand {
    /// The holdout's annualised Sharpe, which the band is centred on.
    pub centre: f64,
    /// Bounds for a live figure measured without error. [`Self::judge`]
    /// widens them by the live sample's own error.
    pub lower: f64,
    pub upper: f64,
    /// Standard error of `centre`, annualised.
    pub standard_error: f64,
    pub method: BandMethod,
    /// When the validation that produced the band ran.
    pub as_of: Timestamp,
    /// The family's lifetime trial count the validation was corrected
    /// against. Not an input to the bounds; recorded so the band can be
    /// traced to the accounting it was admitted under.
    pub trials: u64,
    /// Holdout observations behind `centre`.
    pub observations: usize,
    /// Periods per year `centre` was annualised by; a live figure is put on
    /// the same scale before it is judged.
    pub periods_per_year: f64,
}

impl HoldoutBand {
    /// Derive the band from the deflated Sharpe the holdout gate admitted on.
    pub fn from_deflated(
        deflated: &DeflatedSharpe,
        periods_per_year: f64,
        as_of: Timestamp,
    ) -> Result<Self> {
        Self::with_method(deflated, periods_per_year, as_of, BandMethod::NINETY_FIVE)
    }

    pub fn with_method(
        deflated: &DeflatedSharpe,
        periods_per_year: f64,
        as_of: Timestamp,
        method: BandMethod,
    ) -> Result<Self> {
        // The same annualisation `deflated_sharpe` applied to `observed`,
        // undone to recover the per-period figure the standard error is
        // defined on.
        let scale = periods_per_year.max(1.0).sqrt();
        let periodic = deflated.observed / scale;
        let standard_error = sharpe_standard_error(
            periodic,
            deflated.skewness,
            deflated.excess_kurtosis,
            deflated.observations,
        )? * scale;
        let half_width = method.z() * standard_error;
        let (lower, upper) = (
            deflated.observed - half_width,
            deflated.observed + half_width,
        );
        if !(lower.is_finite() && upper.is_finite()) {
            return Err(Error::numeric(
                "the holdout band is not finite; the deflated Sharpe it was built from is not \
                 a number",
            ));
        }
        let trials = u64::try_from(deflated.trials).map_err(|_| {
            Error::numeric(format!("{} trials does not fit the band", deflated.trials))
        })?;
        Ok(Self {
            centre: deflated.observed,
            lower,
            upper,
            standard_error,
            method,
            as_of,
            trials,
            observations: deflated.observations,
            periods_per_year,
        })
    }

    /// Judge a live return series against the band.
    ///
    /// The live figure is annualised on the band's scale through the same
    /// scoring the validation used, and its own standard error is added to
    /// the band's in quadrature before the bounds are applied. The verdict
    /// carries the effective bounds so a reviewer can see what was tested.
    pub fn judge(&self, live_returns: &[f64]) -> Result<BandVerdict> {
        let observations = live_returns.len();
        let scale = self.periods_per_year.max(1.0).sqrt();
        let periodic = scoring::periodic_sharpe(live_returns)?;
        let live_standard_error = sharpe_standard_error(
            periodic,
            stats::skewness(live_returns),
            stats::excess_kurtosis(live_returns),
            observations,
        )? * scale;
        let live = periodic * scale;
        let half_width = self.method.z()
            * (self.standard_error * self.standard_error
                + live_standard_error * live_standard_error)
                .sqrt();
        let (lower, upper) = (self.centre - half_width, self.centre + half_width);
        if !(live.is_finite() && lower.is_finite() && upper.is_finite()) {
            return Err(Error::numeric(
                "the live Sharpe or its bounds are not finite; the live returns are not numbers",
            ));
        }
        Ok(BandVerdict {
            live,
            live_standard_error,
            lower,
            upper,
            observations,
            inside: lower <= live && live <= upper,
        })
    }

    pub fn describe(&self) -> String {
        format!(
            "holdout band [{:.2}, {:.2}] around an annualised Sharpe of {:.2} (standard error \
             {:.2}) over {} observation(s) at {:.0} period(s) a year, by {}, corrected against \
             {} lifetime trial(s), as of {}",
            self.lower,
            self.upper,
            self.centre,
            self.standard_error,
            self.observations,
            self.periods_per_year,
            self.method.describe(),
            self.trials,
            self.as_of.to_rfc3339()
        )
    }
}

/// What judging a live series against a band concluded.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BandVerdict {
    /// The live Sharpe, annualised on the band's scale.
    pub live: f64,
    pub live_standard_error: f64,
    /// The bounds actually applied: the band's, widened by the live error.
    pub lower: f64,
    pub upper: f64,
    pub observations: usize,
    pub inside: bool,
}

impl BandVerdict {
    pub fn describe(&self) -> String {
        format!(
            "live Sharpe {:.2} (standard error {:.2}) over {} observation(s) is {} the \
             holdout band [{:.2}, {:.2}]",
            self.live,
            self.live_standard_error,
            self.observations,
            if self.inside { "inside" } else { "outside" },
            self.lower,
            self.upper
        )
    }
}
