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

/// A figure a producer either measured or says, with a reason, it cannot.
///
/// Used for exactly two fields of [`ForecastFields`] — `regime_confidence_bp`
/// and `expected_impact` — because a zero there is a claim: a demand forecast
/// has no market regime and no profit and loss of its own, and a record that
/// wrote `0` would assert a certainty or a neutrality nobody computed (ADR
/// 0159 decision 22). The uncertainty split, the tail mass and the
/// distribution have no such arm: a producer that cannot state them produces no
/// [`Forecast`].
///
/// Encoded without a tag: a stated value is the bare figure, byte for byte what
/// a version-1 record carried, so an old record decodes as `Value` and
/// re-encodes unchanged; an unmeasurable figure is `{"unmeasurable": "<why>"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Stated<T> {
    /// The producer measured it.
    Value(T),
    /// The producer cannot, and says why. The reason is required.
    Unmeasurable { unmeasurable: String },
}

impl<T: Copy> Stated<T> {
    /// The measured figure, or a refusal naming `field` and the producer's
    /// reason. A consumer never reads a size off an unmeasurable field.
    pub fn measured(&self, contract: &str, field: &str) -> Result<T> {
        match self {
            Self::Value(value) => Ok(*value),
            Self::Unmeasurable { unmeasurable } => Err(Error::invalid(format!(
                "a {contract} states `{field}` as unmeasurable ({unmeasurable}); nothing is \
                 sized or scored off a figure its producer says it cannot measure"
            ))),
        }
    }
}

impl<T> Stated<T> {
    /// Whether the producer stated the figure unmeasurable.
    pub fn is_unmeasurable(&self) -> bool {
        matches!(self, Self::Unmeasurable { .. })
    }

    fn check_reason(&self, contract: &str, field: &str) -> Result<()> {
        if let Self::Unmeasurable { unmeasurable } = self {
            named(contract, &format!("{field}.unmeasurable"), unmeasurable)?;
        }
        Ok(())
    }
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
    /// Confidence that the regime the model assumes is the one in force, or an
    /// explicit statement that the producer has no regime to be confident in.
    pub regime_confidence_bp: Stated<u32>,
    /// The expected P&L or risk impact of acting on the forecast, in the
    /// account currency. Signed. Or an explicit statement that the producer
    /// has no profit and loss of its own to state.
    pub expected_impact: Stated<Decimal>,
    pub calibration: Calibration,
    /// Evidence and feature ids the forecast was computed from; at least one.
    pub evidence_lineage: Vec<String>,
    /// After this instant the forecast must not be read.
    pub expires_at: Timestamp,
}

/// The §9.14 forecast. Built only through [`Validated::new`].
pub type Forecast = Validated<ForecastFields>;

/// The version that introduced [`Stated`]. A record at the version before it
/// states both figures as measured, which is exactly what its bare numbers say.
const FORECAST_BEFORE_STATED: u32 = 1;

impl Contract for ForecastFields {
    const NAME: &'static str = "Forecast";
    const SCHEMA_VERSION: u32 = 2;

    /// Version 1 still decodes: its two figures were bare numbers and are
    /// `Stated::Value` here, and [`Self::check`] refuses a version-1 record that
    /// claims an unmeasurable arm it predates.
    fn reads(version: u32) -> bool {
        version == Self::SCHEMA_VERSION || version == FORECAST_BEFORE_STATED
    }

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
        if self.schema_version == FORECAST_BEFORE_STATED
            && (self.regime_confidence_bp.is_unmeasurable()
                || self.expected_impact.is_unmeasurable())
        {
            return refuse(
                c,
                "claims an unmeasurable figure at schema version 1, which predates that arm; \
                 write the record at the current version",
            );
        }
        match &self.regime_confidence_bp {
            Stated::Value(bp) => basis_points(c, "regime_confidence_bp", *bp)?,
            unmeasurable => unmeasurable.check_reason(c, "regime_confidence_bp")?,
        }
        self.expected_impact.check_reason(c, "expected_impact")?;
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
    ///
    /// Refused, naming the field, when the regime confidence is unmeasurable:
    /// the compact state carries it as its one confidence figure, and
    /// substituting a number for a statement that there is none would be the
    /// zero this arm exists to prevent.
    pub fn state(&self) -> Result<ForecastState> {
        let confidence_bp = self
            .regime_confidence_bp
            .measured(ForecastFields::NAME, "regime_confidence_bp")?;
        ForecastState::new(ForecastStateFields {
            schema_version: ForecastStateFields::SCHEMA_VERSION,
            target: self.target.clone(),
            horizon: self.horizon,
            distribution: self.distribution.clone(),
            confidence_bp,
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

/// Who a forecast task is put to (R12-WORLD-10091): the four kinds of
/// solver the forecast router fans one question out to, so that each answers
/// independently and their disagreement is a measurement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForecastSolver {
    /// A fitted model in the forecast lattice's population.
    SpecialistModel,
    /// An agent of the organisation, reasoning over the evidence.
    Agent,
    /// A world-model federation member.
    WorldModel,
    /// A simulator: the twin's future tree or a scenario run.
    Simulator,
}

impl ForecastSolver {
    pub const ALL: [Self; 4] = [
        Self::SpecialistModel,
        Self::Agent,
        Self::WorldModel,
        Self::Simulator,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SpecialistModel => "specialist_model",
            Self::Agent => "agent",
            Self::WorldModel => "world_model",
            Self::Simulator => "simulator",
        }
    }
}

/// Why a forecast task was issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForecastTrigger {
    /// The routine question every promoted current-state vector raises.
    StateVector,
    /// The NOW Brain was surprised by the subject this cycle.
    Surprise,
}

/// One independent forecast question put to one kind of solver at one
/// horizon (R12-WORLD-10091): the task contract the forecast router fans a
/// state vector or a surprise out as. It names the subject, the horizon the
/// answer is due over and the instant it is due by, so a reader can say
/// which solver was asked what, and when an answer became late.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForecastTask {
    pub task_id: String,
    pub subject: String,
    /// The horizon's name, e.g. `hours_days`.
    pub horizon: String,
    /// The span the forecast is over.
    pub span: Duration,
    pub solver: ForecastSolver,
    pub trigger: ForecastTrigger,
    pub issued_at: Timestamp,
    /// `issued_at + span`.
    pub due_by: Timestamp,
}

impl ForecastTask {
    /// A task, refusing a blank subject or horizon name and a span that is
    /// not positive: a question over no time asks nothing, and a blank
    /// subject asks it about nothing.
    pub fn new(
        subject: &str,
        horizon: &str,
        span: Duration,
        solver: ForecastSolver,
        trigger: ForecastTrigger,
        issued_at: Timestamp,
    ) -> Result<Self> {
        if subject.trim().is_empty() || horizon.trim().is_empty() {
            return Err(Error::invalid(
                "a forecast task names its subject and horizon; a blank one asks about nothing",
            ));
        }
        if span <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "a forecast task for {subject} at {horizon} spans {span:?}, which is no time; \
                 state the horizon's span"
            )));
        }
        Ok(Self {
            task_id: format!(
                "{subject}:{horizon}:{}@{}",
                solver.as_str(),
                issued_at.as_nanos()
            ),
            subject: subject.to_string(),
            horizon: horizon.to_string(),
            span,
            solver,
            trigger,
            issued_at,
            due_by: issued_at.saturating_add(span),
        })
    }
}
