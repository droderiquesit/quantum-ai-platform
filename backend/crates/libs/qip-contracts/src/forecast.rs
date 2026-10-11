//! `Forecast` (blueprint v12.0 §9.14, R12-CONTRACT-3117) and the compact
//! `ForecastState` (§24.9, R12-CONTRACT-6109), plus the distribution and
//! calibration vocabulary the world-model contracts share.
//!
//! Before this, a prediction travelled as whichever of three partial shapes
//! the producer had to hand — a `Hypothesis` with one uncertainty *label*, a
//! `BeliefState` with a confidence and an expiry, a `ForecastLattice` with
//! distributions but no expiry — and none of them could say how much of the
//! uncertainty was the world's (aleatoric, irreducible) and how much was the
//! model's (epistemic, worth buying data to reduce). That split decides
//! whether the curiosity engine should spend on a question at all, so a
//! forecast that cannot state it is refused here.

use crate::validated::{
    Contract, MAX_BP, Validated, after, basis_points, named, names, non_negative,
    positive_duration, probability_mass, refuse,
};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One outcome of a categorical distribution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeMass {
    pub outcome: String,
    pub probability_bp: u32,
}

/// One point of a quantile function: `P(target <= value) = quantile_bp`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuantilePoint {
    pub quantile_bp: u32,
    pub value: Decimal,
}

/// A predictive distribution over a target. Exact integers and decimals so a
/// replay reproduces it bit for bit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum OutcomeDistribution {
    /// At least two distinct outcomes whose masses sum to exactly one.
    Categorical { outcomes: Vec<OutcomeMass> },
    /// At least two points, quantiles strictly increasing inside (0, 1) and
    /// values non-decreasing — a quantile function that goes down is not one.
    Quantiles { points: Vec<QuantilePoint> },
}

impl OutcomeDistribution {
    pub(crate) fn check(&self, contract: &str) -> Result<()> {
        match self {
            Self::Categorical { outcomes } => {
                if outcomes.len() < 2 {
                    return refuse(
                        contract,
                        "states a categorical distribution of fewer than two outcomes; a \
                         certainty is not a forecast",
                    );
                }
                let mut seen = BTreeSet::new();
                for o in outcomes {
                    named(contract, "distribution.outcome", &o.outcome)?;
                    if !seen.insert(o.outcome.as_str()) {
                        return refuse(contract, format!("states outcome `{}` twice", o.outcome));
                    }
                }
                let masses: Vec<u32> = outcomes.iter().map(|o| o.probability_bp).collect();
                probability_mass(contract, "distribution", &masses)
            }
            Self::Quantiles { points } => {
                if points.len() < 2 {
                    return refuse(contract, "states fewer than two quantiles; give a spread");
                }
                for pair in points.windows(2) {
                    if let [a, b] = pair
                        && (b.quantile_bp <= a.quantile_bp || b.value < a.value)
                    {
                        return refuse(
                            contract,
                            "states quantiles that are not increasing, or values that decrease \
                             as the quantile rises",
                        );
                    }
                }
                for p in points {
                    if p.quantile_bp == 0 || p.quantile_bp >= MAX_BP {
                        return refuse(
                            contract,
                            format!(
                                "states quantile {} bp; quantiles lie strictly between 0 and {MAX_BP}",
                                p.quantile_bp
                            ),
                        );
                    }
                }
                Ok(())
            }
        }
    }
}

/// How well the producing model's past forecasts matched what happened.
/// `Uncalibrated` is a statement a consumer may refuse; it is not a default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Calibration {
    Uncalibrated,
    Measured {
        /// The scoring rule, e.g. `brier` or `crps`.
        method: String,
        /// The score, in the rule's own units (lower is better for both).
        score: Decimal,
        /// How many resolved forecasts the score is over; at least one.
        resolved: u64,
        evaluated_at: Timestamp,
    },
}

impl Calibration {
    pub(crate) fn check(&self, contract: &str) -> Result<()> {
        if let Self::Measured {
            method,
            score,
            resolved,
            ..
        } = self
        {
            named(contract, "calibration.method", method)?;
            non_negative(contract, "calibration.score", *score)?;
            if *resolved == 0 {
                return refuse(
                    contract,
                    "claims a measured calibration over zero resolved forecasts; say Uncalibrated",
                );
            }
        }
        Ok(())
    }
}

/// The two kinds of uncertainty §9.14 separates, as variances of the target
/// in its own squared units. Their sum is the total predictive variance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UncertaintyDecomposition {
    /// Reducible: what better data or models would remove.
    pub epistemic: Decimal,
    /// Irreducible: the world's own randomness at this horizon.
    pub aleatoric: Decimal,
}

impl UncertaintyDecomposition {
    /// Refuses a negative component; the sum is checked, never saturated.
    pub fn total(&self) -> Result<Decimal> {
        self.epistemic.checked_add(self.aleatoric).ok_or_else(|| {
            Error::numeric("the uncertainty components overflow when summed; restate their units")
        })
    }

    pub(crate) fn check(&self, contract: &str) -> Result<()> {
        non_negative(contract, "uncertainty.epistemic", self.epistemic)?;
        non_negative(contract, "uncertainty.aleatoric", self.aleatoric)?;
        self.total().map(|_| ())
    }
}

/// Which tail and how much mass sits beyond `threshold`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TailSide {
    Below,
    Above,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TailMass {
    pub side: TailSide,
    pub threshold: Decimal,
    pub probability_bp: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForecastFields {
    pub schema_version: u32,
    pub forecast_id: String,
    /// The model (and version) that produced it.
    pub model: String,
    /// The variable predicted, e.g. `XNYS:ACME/mid_price`.
    pub target: String,
    pub issued_at: Timestamp,
    /// How far past `issued_at` the forecast speaks to.
    pub horizon: Duration,
    pub distribution: OutcomeDistribution,
    pub uncertainty: UncertaintyDecomposition,
    pub tail: TailMass,
    /// How many models were consulted; at least one.
    pub models_consulted: u32,
    /// Spread of the consulted models' point forecasts, in target units.
    pub model_disagreement: Decimal,
    /// Confidence that the regime the model assumes is the one in force.
    pub regime_confidence_bp: u32,
    /// The expected P&L or risk impact of acting on the forecast, in the
    /// account currency. Signed.
    pub expected_impact: Decimal,
    pub calibration: Calibration,
    /// Evidence and feature ids the forecast was computed from; at least one.
    pub evidence_lineage: Vec<String>,
    /// After this instant the forecast must not be read.
    pub expires_at: Timestamp,
}

/// The §9.14 forecast. Built only through [`Validated::new`].
pub type Forecast = Validated<ForecastFields>;

impl Contract for ForecastFields {
    const NAME: &'static str = "Forecast";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank id, model or target; a non-positive horizon; any
    /// distribution, calibration or uncertainty defect; a tail probability
    /// above one; zero models; a negative disagreement, or any disagreement
    /// from a single model (one model cannot disagree with itself, so a
    /// non-zero figure there was computed from something else); no evidence
    /// lineage; and an expiry not after issue.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "forecast_id", &self.forecast_id)?;
        named(c, "model", &self.model)?;
        named(c, "target", &self.target)?;
        positive_duration(c, "horizon", self.horizon)?;
        self.distribution.check(c)?;
        self.uncertainty.check(c)?;
        basis_points(c, "tail.probability_bp", self.tail.probability_bp)?;
        if self.models_consulted == 0 {
            return refuse(c, "consulted zero models; name the model that produced it");
        }
        non_negative(c, "model_disagreement", self.model_disagreement)?;
        if self.models_consulted == 1 && !self.model_disagreement.is_zero() {
            return refuse(
                c,
                "reports disagreement from a single model; one model cannot disagree with itself",
            );
        }
        basis_points(c, "regime_confidence_bp", self.regime_confidence_bp)?;
        self.calibration.check(c)?;
        names(c, "evidence_lineage", &self.evidence_lineage)?;
        after(
            c,
            "expires_at",
            self.expires_at,
            "issued_at",
            self.issued_at,
        )
    }
}

impl Forecast {
    /// Refuses a forecast read at or after its expiry.
    pub fn read_at(&self, now: Timestamp) -> Result<&ForecastFields> {
        if now >= self.expires_at {
            return Err(Error::denied(format!(
                "forecast {} expired; request a fresh one rather than acting on a stale \
                 prediction",
                self.forecast_id
            )));
        }
        Ok(self.fields())
    }

    /// The compact §24.9 [`ForecastState`] this forecast projects to, with
    /// `confidence_bp` taken from the regime confidence — the one figure in a
    /// [`Forecast`] that says how far its own premises hold.
    pub fn state(&self) -> Result<ForecastState> {
        ForecastState::new(ForecastStateFields {
            schema_version: ForecastStateFields::SCHEMA_VERSION,
            target: self.target.clone(),
            horizon: self.horizon,
            distribution: self.distribution.clone(),
            confidence_bp: self.regime_confidence_bp,
            uncertainty: self.uncertainty,
            expires_at: self.expires_at,
        })
    }
}

/// The §24.9 control-plane forecast: the six fields a superintelligence
/// control loop needs and nothing else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForecastStateFields {
    pub schema_version: u32,
    pub target: String,
    pub horizon: Duration,
    pub distribution: OutcomeDistribution,
    pub confidence_bp: u32,
    pub uncertainty: UncertaintyDecomposition,
    pub expires_at: Timestamp,
}

/// The §24.9 `ForecastState`. Built only through [`Validated::new`].
pub type ForecastState = Validated<ForecastStateFields>;

impl Contract for ForecastStateFields {
    const NAME: &'static str = "ForecastState";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank target, a non-positive horizon, a confidence above
    /// one, and any distribution or uncertainty defect. There is no issue
    /// time to order the expiry against; [`Forecast::state`] carries the
    /// parent's, which was.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "target", &self.target)?;
        positive_duration(c, "horizon", self.horizon)?;
        self.distribution.check(c)?;
        basis_points(c, "confidence_bp", self.confidence_bp)?;
        self.uncertainty.check(c)
    }
}
