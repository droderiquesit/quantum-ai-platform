//! The NOW Brain — blueprint v12.0 §9.6, GCP v3.0 §11.2 (WORLD-3075..3078,
//! WORLD-9016..9021, AMBIENT-3079, DATA-3080, EVID-3081, WORLD-7196).
//!
//! # The failure this exists to prevent
//!
//! The platform collected facts per subsystem — returns per instrument,
//! spreads per venue, a kill switch, macro surprises — and nothing combined
//! them into an estimate of the *latent* state: what is most likely true now,
//! how sure the platform is, how old that belief is, how fast it is moving
//! and how many independent sources it rests on. Three specific failures
//! followed, and each is made structural here:
//!
//! * **A contradiction read as confidence.** Averaging two sources that say
//!   0.1 and 0.9 reports a confident 0.5. Here the readings' own spread enters
//!   the measurement variance every estimator sees, so a split widens the
//!   posterior rather than hiding inside a mean.
//! * **A stale belief read as a fresh one.** Every [`NowVariable`] carries its
//!   freshness, and [`NowBrain::read`] decays its confidence once it is past
//!   its freshness bound and refuses it outright after its expiry. Staleness
//!   lowers confidence; it never blocks a reader that can act without it.
//! * **One estimator's opinion read as the estimate.** Several state filters
//!   (a Kalman local-level filter, a particle filter, a robust median filter,
//!   and anything else registered through [`StateFilter`]) each propose a
//!   candidate; their disagreement is part of the posterior variance and is
//!   recorded as a [`ModelDisagreement`] when it is material. A missing
//!   worker shrinks the reported ensemble; it does not stop estimation.
//!
//! # What it persists, and what it does not
//!
//! Each update emits a [`BeliefDelta`] — prior and posterior moments plus the
//! manifest references the readings came from — and nothing else. The raw
//! observation is not kept: it is re-fetchable from the source the manifest
//! names (EVID-3081), and a delta with a payload field is refused at decode
//! because the record denies unknown fields (DATA-3080).
//!
//! # Advisory, and structurally so
//!
//! Nothing here holds a handle to an order, a size or a broker. The NOW
//! state is something the cycle and the deep brain *read*; a surprise is a
//! [`SurpriseEvent`] naming the specialists it should wake, never an action.
//!
//! # Point in time
//!
//! An [`Observation`] carries both instants: when the value was true and when
//! the platform could know it. [`NowBrain::step`] refuses any observation
//! knowable after the step's `now`, so a NOW state cannot be built from a
//! value the platform did not yet have.

use crate::world::WorldModel;
use qip_contracts::forecast::{Calibration, OutcomeDistribution, QuantilePoint};
use qip_contracts::validated::{Contract, MAX_BP, Validated};
use qip_contracts::world_model::{
    ArbitrationStatus, BranchParent, DisputingView, ModelDisagreement, ModelDisagreementFields,
    StateVariable, WorldModelState, WorldModelStateFields,
};
use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The schema version every NOW record is written at.
pub const NOW_SCHEMA_VERSION: u32 = 1;

/// The most variables the store holds at once. A key beyond it is refused
/// for the step and named, never silently dropped: an unbounded latent state
/// is an unbounded working set.
pub const MAX_VARIABLES: usize = 4_096;

/// How many standard deviations of predictive error make a change a
/// surprise. Three: under the filter's own model a three-sigma innovation
/// happens about once in four hundred updates, which is rare enough to be
/// worth waking a specialist for and common enough to fire in a real storm.
pub const DEFAULT_SURPRISE_SIGMAS: f64 = 3.0;

/// The 90th percentile of the standard normal: the half-width, in standard
/// deviations, of the central 80% interval every projection states.
pub const Z_80: f64 = 1.281_551_565_544_600_4;

/// Particles the particle filter carries per variable.
pub const PARTICLES: usize = 128;

/// The eight scopes a state vector exists at (WORLD-3077).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Global,
    Region,
    Sector,
    Company,
    Instrument,
    Venue,
    Portfolio,
    Infrastructure,
}

impl ScopeKind {
    pub const ALL: [Self; 8] = [
        Self::Global,
        Self::Region,
        Self::Sector,
        Self::Company,
        Self::Instrument,
        Self::Venue,
        Self::Portfolio,
        Self::Infrastructure,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Region => "region",
            Self::Sector => "sector",
            Self::Company => "company",
            Self::Instrument => "instrument",
            Self::Venue => "venue",
            Self::Portfolio => "portfolio",
            Self::Infrastructure => "infrastructure",
        }
    }
}

/// What the latent state is about (WORLD-3075). A fixed enum because it is a
/// metric label and a partition key; neither may be minted at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Markets,
    Economy,
    Companies,
    SupplyChain,
    Liquidity,
    Positioning,
    Policy,
    Volatility,
    SystemHealth,
}

impl Domain {
    pub const ALL: [Self; 9] = [
        Self::Markets,
        Self::Economy,
        Self::Companies,
        Self::SupplyChain,
        Self::Liquidity,
        Self::Positioning,
        Self::Policy,
        Self::Volatility,
        Self::SystemHealth,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Markets => "markets",
            Self::Economy => "economy",
            Self::Companies => "companies",
            Self::SupplyChain => "supply_chain",
            Self::Liquidity => "liquidity",
            Self::Positioning => "positioning",
            Self::Policy => "policy",
            Self::Volatility => "volatility",
            Self::SystemHealth => "system_health",
        }
    }
}

/// What a state vector is about: a scope kind and the thing within it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub kind: ScopeKind,
    pub id: String,
}

/// The one id the global scope carries, so there is exactly one global vector.
pub const GLOBAL_ID: &str = "global";

impl Scope {
    pub fn global() -> Self {
        Self {
            kind: ScopeKind::Global,
            id: GLOBAL_ID.to_string(),
        }
    }

    /// Refuses a blank id, and a global scope with any id but [`GLOBAL_ID`]:
    /// two global vectors would be two answers to one question.
    pub fn new(kind: ScopeKind, id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a {} scope needs a non-blank id; name the thing the state is about",
                kind.as_str()
            )));
        }
        if kind == ScopeKind::Global && id != GLOBAL_ID {
            return Err(Error::invalid(format!(
                "the global scope's id is `{GLOBAL_ID}`, not `{id}`; there is one global state"
            )));
        }
        Ok(Self { kind, id })
    }
}

/// The partition a variable lives in (WORLD-9017): its domain and its region.
/// A write to one partition never touches another, so a fault confined to one
/// region's market data cannot corrupt another region's state.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionKey {
    pub domain: Domain,
    pub region: String,
}

impl PartitionKey {
    pub fn new(domain: Domain, region: impl Into<String>) -> Result<Self> {
        let region = region.into();
        if region.trim().is_empty() {
            return Err(Error::invalid(
                "a NOW partition needs a non-blank region; use `global` for one that has none",
            ));
        }
        Ok(Self { domain, region })
    }
}

impl fmt::Display for PartitionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.domain.as_str(), self.region)
    }
}

/// One variable: a named quantity at a scope.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariableKey {
    pub scope: Scope,
    pub name: String,
}

impl VariableKey {
    pub fn new(scope: Scope, name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid("a NOW variable needs a non-blank name"));
        }
        Ok(Self { scope, name })
    }
}

impl fmt::Display for VariableKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}/{}",
            self.scope.kind.as_str(),
            self.scope.id,
            self.name
        )
    }
}

/// Where a reading can be fetched again (EVID-3081). The NOW Brain keeps the
/// pointer, not the payload.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestRef {
    /// The source, e.g. `kernel/bar-history`.
    pub source: String,
    /// The reference within it, e.g. `obj-AAA@2026-10-11T00:00:00Z`.
    pub reference: String,
}

impl ManifestRef {
    pub fn new(source: impl Into<String>, reference: impl Into<String>) -> Result<Self> {
        let (source, reference) = (source.into(), reference.into());
        if source.trim().is_empty() || reference.trim().is_empty() {
            return Err(Error::invalid(
                "a manifest reference names its source and the entry within it; a reading \
                 nobody can fetch again cannot back a NOW state",
            ));
        }
        Ok(Self { source, reference })
    }

    /// The single string a contract's evidence lineage carries.
    pub fn lineage(&self) -> String {
        format!("{}#{}", self.source, self.reference)
    }
}

/// A bound a symbolic constraint puts on a variable (WORLD-9020). A candidate
/// whose mean breaks it is not promoted.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Constraint {
    Unbounded,
    NonNegative,
    UnitInterval,
}

impl Constraint {
    pub fn admits(self, value: f64) -> bool {
        match self {
            Self::Unbounded => value.is_finite(),
            Self::NonNegative => value.is_finite() && value >= 0.0,
            Self::UnitInterval => (0.0..=1.0).contains(&value),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unbounded => "unbounded",
            Self::NonNegative => "non_negative",
            Self::UnitInterval => "unit_interval",
        }
    }
}

/// The scale a variable's uncertainty is judged against, for its confidence.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    /// A typical magnitude fixed by what the variable is (a daily return is
    /// about 0.01 whatever its level).
    Absolute(f64),
    /// The variable's own magnitude, floored: for a quantity whose units are
    /// the source's (a revenue figure), only a relative error means anything.
    Relative { floor: f64 },
}

impl Scale {
    fn at(self, mean: f64) -> f64 {
        match self {
            Self::Absolute(s) => s,
            Self::Relative { floor } => mean.abs().max(floor),
        }
    }

    fn valid(self) -> bool {
        match self {
            Self::Absolute(s) | Self::Relative { floor: s } => s.is_finite() && s > 0.0,
        }
    }
}

/// What a variable name means: its domain, scale, freshness and constraint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VariableSpec {
    pub name: String,
    pub domain: Domain,
    pub scale: Scale,
    /// How long a promoted value is fresh. Past it, confidence decays.
    pub fresh_for: Duration,
    /// How long after promotion it may be read at all (WORLD-9021).
    pub expires_after: Duration,
    pub constraint: Constraint,
    /// Process noise per day, as a fraction of the scale: how far the latent
    /// value is assumed to wander between observations.
    pub drift_per_day: f64,
    /// One reading's noise, as a fraction of the scale.
    pub reading_noise: f64,
}

impl VariableSpec {
    fn check(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::invalid("a NOW variable spec needs a name"));
        }
        if !self.scale.valid() {
            return Err(Error::invalid(format!(
                "`{}` declares a scale that is not finite and positive",
                self.name
            )));
        }
        if self.fresh_for <= Duration::ZERO || self.expires_after <= self.fresh_for {
            return Err(Error::invalid(format!(
                "`{}` must be fresh for a positive span and expire after it goes stale; a \
                 variable that expires while fresh is never stale, and staleness is the \
                 signal readers decay on",
                self.name
            )));
        }
        if !(self.drift_per_day.is_finite()
            && self.drift_per_day >= 0.0
            && self.reading_noise.is_finite()
            && self.reading_noise > 0.0)
        {
            return Err(Error::invalid(format!(
                "`{}` needs a finite, non-negative drift and a finite, positive reading noise",
                self.name
            )));
        }
        Ok(())
    }
}

/// The declared variable names. A reading under a name nobody declared is
/// refused, so the label set and the store stay bounded.
#[derive(Clone, Debug, Default)]
pub struct Catalogue {
    specs: BTreeMap<String, VariableSpec>,
}

/// The most specs a catalogue holds; a runtime declaration past it is refused.
pub const MAX_SPECS: usize = 256;

impl Catalogue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a variable name. Refuses a malformed spec, a redeclaration that
    /// differs (two meanings for one name), and a catalogue already full.
    /// Redeclaring the identical spec is a no-op, so a caller may declare on
    /// every pass.
    pub fn declare(&mut self, spec: VariableSpec) -> Result<()> {
        spec.check()?;
        if let Some(existing) = self.specs.get(&spec.name) {
            if *existing == spec {
                return Ok(());
            }
            return Err(Error::invalid(format!(
                "`{}` is already declared with a different meaning; choose a new name",
                spec.name
            )));
        }
        if self.specs.len() >= MAX_SPECS {
            return Err(Error::invalid(format!(
                "the NOW catalogue holds {MAX_SPECS} names; `{}` is refused rather than grown \
                 past its bound",
                spec.name
            )));
        }
        self.specs.insert(spec.name.clone(), spec);
        Ok(())
    }

    pub fn spec(&self, name: &str) -> Option<&VariableSpec> {
        self.specs.get(name)
    }

    pub fn len(&self) -> usize {
        self.specs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// The variables the cycle reads, each with the scale and freshness its
    /// source supports.
    pub fn standard() -> Result<Self> {
        let day = Duration::from_days(1);
        let mut c = Self::new();
        for spec in [
            VariableSpec {
                name: "return".into(),
                domain: Domain::Markets,
                scale: Scale::Absolute(0.01),
                fresh_for: day,
                expires_after: Duration::from_days(7),
                constraint: Constraint::Unbounded,
                drift_per_day: 0.5,
                reading_noise: 0.5,
            },
            VariableSpec {
                name: "realised_volatility".into(),
                domain: Domain::Volatility,
                scale: Scale::Absolute(0.01),
                fresh_for: day,
                expires_after: Duration::from_days(7),
                constraint: Constraint::NonNegative,
                drift_per_day: 0.2,
                reading_noise: 0.2,
            },
            VariableSpec {
                name: "spread_ratio".into(),
                domain: Domain::Liquidity,
                scale: Scale::Absolute(1.0),
                fresh_for: day,
                expires_after: Duration::from_days(7),
                constraint: Constraint::NonNegative,
                drift_per_day: 0.2,
                reading_noise: 0.2,
            },
            VariableSpec {
                name: "exposure_fraction".into(),
                domain: Domain::Positioning,
                scale: Scale::Absolute(0.25),
                fresh_for: day,
                expires_after: Duration::from_days(7),
                constraint: Constraint::UnitInterval,
                drift_per_day: 0.2,
                reading_noise: 0.05,
            },
            VariableSpec {
                name: "halted".into(),
                domain: Domain::SystemHealth,
                scale: Scale::Absolute(0.5),
                fresh_for: Duration::from_hours(1),
                expires_after: day,
                constraint: Constraint::UnitInterval,
                drift_per_day: 1.0,
                reading_noise: 0.05,
            },
            VariableSpec {
                name: "blind_spot_share".into(),
                domain: Domain::SystemHealth,
                scale: Scale::Absolute(0.25),
                fresh_for: day,
                expires_after: Duration::from_days(7),
                constraint: Constraint::UnitInterval,
                drift_per_day: 0.2,
                reading_noise: 0.05,
            },
            VariableSpec {
                name: "macro_surprise".into(),
                domain: Domain::Economy,
                scale: Scale::Absolute(1.0),
                fresh_for: Duration::from_days(31),
                expires_after: Duration::from_days(92),
                constraint: Constraint::Unbounded,
                drift_per_day: 0.05,
                reading_noise: 0.5,
            },
        ] {
            c.declare(spec)?;
        }
        Ok(c)
    }
}

/// One reading, stamped in both time dimensions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub key: VariableKey,
    pub partition: PartitionKey,
    pub value: f64,
    /// Which source produced it; distinct sources make the diversity figure.
    pub source: String,
    /// When the value was true.
    pub valid_at: Timestamp,
    /// When the platform could first know it.
    pub known_at: Timestamp,
    pub manifest: ManifestRef,
}

/// A rate of change, or the explicit statement that none has been measured.
/// A first estimate has no earlier one to difference against, and a zero
/// there would be a claim that the state is not moving.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RateOfChange {
    Unmeasured,
    PerDay { value: f64 },
}

/// When a variable was observed, learned, promoted, and how long it is fresh.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Freshness {
    /// The latest `valid_at` among the readings this promotion rests on.
    pub observed_at: Timestamp,
    /// The latest `known_at` among them.
    pub known_at: Timestamp,
    pub promoted_at: Timestamp,
    pub fresh_for: Duration,
}

/// The record a promoted NOW variable is (WORLD-3076). Every one of the five
/// §9.6 fields is a required field with no `Option`, so a producer that
/// omits one fails to deserialise, and [`Contract::check`] refuses a value
/// that is present but meaningless.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NowVariableFields {
    pub schema_version: u32,
    pub key: VariableKey,
    pub partition: PartitionKey,
    pub mean: f64,
    /// Posterior variance, in the variable's squared units.
    pub uncertainty: f64,
    /// Confidence at promotion, in basis points.
    pub confidence_bp: u32,
    pub freshness: Freshness,
    pub rate_of_change: RateOfChange,
    /// Distinct sources among the readings this promotion rests on.
    pub source_diversity: u32,
    /// Estimators that contributed a candidate (WORLD-9019).
    pub ensemble: u32,
    /// Spread of the contributing estimators' means.
    pub disagreement: f64,
    /// The reconciler's fencing epoch at promotion.
    pub epoch: u64,
    /// The manifest entries behind it; at least one.
    pub manifests: Vec<ManifestRef>,
    pub expires_at: Timestamp,
}

/// A promoted NOW state variable. Built only through [`Validated::new`].
pub type NowVariable = Validated<NowVariableFields>;

fn refuse(contract: &str, why: impl fmt::Display) -> Result<()> {
    Err(Error::invalid(format!("a {contract} {why}")))
}

impl Contract for NowVariableFields {
    const NAME: &'static str = "NowVariable";
    const SCHEMA_VERSION: u32 = NOW_SCHEMA_VERSION;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a non-finite mean or rate, a negative or non-finite
    /// uncertainty or disagreement, a confidence above one, zero sources or
    /// estimators, no manifest, a non-positive freshness span, a promotion
    /// before the readings were knowable, and an expiry not after promotion.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        if !self.mean.is_finite() {
            return refuse(c, "states a non-finite mean");
        }
        if !(self.uncertainty.is_finite() && self.uncertainty >= 0.0) {
            return refuse(c, "states an uncertainty that is negative or not finite");
        }
        if !(self.disagreement.is_finite() && self.disagreement >= 0.0) {
            return refuse(c, "states a disagreement that is negative or not finite");
        }
        if self.confidence_bp > MAX_BP {
            return refuse(c, format!("states confidence {} bp", self.confidence_bp));
        }
        if let RateOfChange::PerDay { value } = self.rate_of_change
            && !value.is_finite()
        {
            return refuse(c, "states a non-finite rate of change; say Unmeasured");
        }
        if self.source_diversity == 0 {
            return refuse(c, "rests on zero sources");
        }
        if self.ensemble == 0 {
            return refuse(c, "was estimated by zero estimators");
        }
        if self.ensemble == 1 && self.disagreement != 0.0 {
            return refuse(
                c,
                "reports disagreement from one estimator; one cannot disagree with itself",
            );
        }
        if self.manifests.is_empty() {
            return refuse(
                c,
                "cites no manifest entry; external evidence must stay re-fetchable",
            );
        }
        if self.freshness.fresh_for <= Duration::ZERO {
            return refuse(c, "is fresh for a non-positive span");
        }
        if self.freshness.known_at > self.freshness.promoted_at {
            return refuse(
                c,
                "was promoted before its readings were knowable; that is look-ahead",
            );
        }
        if self.expires_at <= self.freshness.promoted_at {
            return refuse(c, "expires at or before its promotion");
        }
        Ok(())
    }
}

/// The confidence a variable reads at `now`: its promotion confidence, halved
/// for every `fresh_for` it has aged past fresh. Never zero before expiry —
/// stale state lowers confidence and does not vanish (WORLD-9018).
pub fn confidence_at(v: &NowVariableFields, now: Timestamp) -> u32 {
    let age = now.since(v.freshness.promoted_at);
    if age <= v.freshness.fresh_for {
        return v.confidence_bp;
    }
    let past = (age.as_secs_f64() - v.freshness.fresh_for.as_secs_f64())
        / v.freshness.fresh_for.as_secs_f64();
    // Statistic to integer: rounding a decayed fraction to basis points.
    (f64::from(v.confidence_bp) * 0.5_f64.powf(past)).round() as u32
}

/// The moments of an earlier promotion.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Prior {
    /// The first estimate of this variable.
    Genesis,
    Was {
        mean: f64,
        variance: f64,
    },
}

/// What the NOW Brain persists per update (DATA-3080): the change in belief
/// and the manifest it rests on, never the raw observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeliefDeltaFields {
    pub schema_version: u32,
    pub key: VariableKey,
    pub partition: PartitionKey,
    pub epoch: u64,
    pub prior: Prior,
    pub posterior_mean: f64,
    pub posterior_variance: f64,
    pub confidence_bp: u32,
    pub at: Timestamp,
    /// At least one (EVID-3081).
    pub manifests: Vec<ManifestRef>,
}

/// A NOW belief delta. Built only through [`Validated::new`].
pub type BeliefDelta = Validated<BeliefDeltaFields>;

impl Contract for BeliefDeltaFields {
    const NAME: &'static str = "BeliefDelta";
    const SCHEMA_VERSION: u32 = NOW_SCHEMA_VERSION;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        if !(self.posterior_mean.is_finite()
            && self.posterior_variance.is_finite()
            && self.posterior_variance >= 0.0)
        {
            return refuse(c, "states a posterior that is not finite");
        }
        if let Prior::Was { mean, variance } = self.prior
            && !(mean.is_finite() && variance.is_finite() && variance >= 0.0)
        {
            return refuse(c, "states a prior that is not finite");
        }
        if self.confidence_bp > MAX_BP {
            return refuse(c, format!("states confidence {} bp", self.confidence_bp));
        }
        if self.manifests.is_empty() {
            return refuse(
                c,
                "cites no manifest entry; a delta nobody can re-derive is a raw claim",
            );
        }
        Ok(())
    }
}

/// The specialists a surprise wakes (AMBIENT-3079).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Specialist {
    /// Re-forecast the surprised target out of turn.
    Forecast,
    /// Test whether the premises behind the state still hold.
    Adversarial,
    /// Look harder: buy the observation that would discriminate.
    Sensing,
    /// Review exposure to what moved.
    Risk,
}

impl Specialist {
    pub const ALL: [Self; 4] = [Self::Forecast, Self::Adversarial, Self::Sensing, Self::Risk];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Forecast => "forecast",
            Self::Adversarial => "adversarial",
            Self::Sensing => "sensing",
            Self::Risk => "risk",
        }
    }
}

/// A state change the filter's own model said was unlikely (WORLD-9016).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurpriseEvent {
    pub id: String,
    pub key: VariableKey,
    pub partition: PartitionKey,
    /// The standardised innovation: readings' mean minus the prior mean, over
    /// the predictive standard deviation. Signed.
    pub sigmas: f64,
    pub prior_mean: f64,
    pub posterior_mean: f64,
    pub at: Timestamp,
    /// Which specialists the surprise should wake. Every surprise wakes all
    /// four: a latent-state break is by construction something the forecast
    /// was wrong about, whose premise may be wrong, whose cause is unobserved
    /// and whose exposure is unreviewed. Narrowing that is a routing policy's
    /// decision, made where the router's budget is.
    pub wakes: Vec<Specialist>,
    pub manifests: Vec<ManifestRef>,
}

/// One estimator's proposal for one variable.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub mean: f64,
    pub variance: f64,
}

/// What a worker is told about one variable this step.
#[derive(Clone, Debug)]
pub struct Evidence<'a> {
    pub key: &'a VariableKey,
    /// The last promoted moments, if any.
    pub prior: Option<(f64, f64)>,
    pub readings: &'a [f64],
    /// Variance of the readings' mean as a measurement of the state,
    /// including their own spread — the contradiction term.
    pub measurement_variance: f64,
    /// Variance the state is assumed to have gained since the prior.
    pub process_variance: f64,
}

impl Evidence<'_> {
    fn mean(&self) -> f64 {
        self.readings.iter().sum::<f64>() / self.readings.len() as f64
    }
}

/// The family an estimator belongs to (WORLD-3078).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimatorKind {
    /// A Bayesian state-space filter.
    StateSpace,
    /// A sequential Monte Carlo filter.
    Particle,
    /// A robust filter that discounts outlying readings.
    Robust,
    /// A neural state-space model.
    Neural,
    /// A graph model over related variables.
    Graph,
    /// A causal model.
    Causal,
}

/// A state-filter worker (WORLD-9019). Each proposes a candidate latent value
/// from the evidence; the reconciler combines and promotes.
///
/// `Send + Sync` because the brain lives inside the platform, which the
/// composition roots share across threads.
pub trait StateFilter: fmt::Debug + Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> EstimatorKind;
    /// Propose, or `Ok(None)` to abstain on this variable. An `Err` is a
    /// worker failure: recorded, and the ensemble proceeds without it.
    fn propose(&mut self, evidence: &Evidence<'_>) -> Result<Option<Candidate>>;
    /// Drop whatever per-variable state the worker holds for an expired key.
    fn forget(&mut self, _key: &VariableKey) {}
}

/// The Kalman local-level filter: the prior, inflated by process noise,
/// updated by the readings' mean as one measurement.
#[derive(Debug, Default)]
pub struct KalmanFilter;

impl StateFilter for KalmanFilter {
    fn id(&self) -> &str {
        "kalman-local-level"
    }
    fn kind(&self) -> EstimatorKind {
        EstimatorKind::StateSpace
    }
    fn propose(&mut self, e: &Evidence<'_>) -> Result<Option<Candidate>> {
        let z = e.mean();
        let r = e.measurement_variance;
        Ok(Some(match e.prior {
            None => Candidate {
                mean: z,
                variance: r,
            },
            Some((m, v)) => {
                let p = v + e.process_variance;
                let gain = p / (p + r);
                Candidate {
                    mean: m + gain * (z - m),
                    variance: (1.0 - gain) * p,
                }
            }
        }))
    }
}

/// A bootstrap particle filter with deterministic randomness, so a replay of
/// the same observations reproduces the same estimate.
#[derive(Debug)]
pub struct ParticleFilter {
    rng: Xoshiro256,
    clouds: BTreeMap<VariableKey, Vec<f64>>,
}

impl ParticleFilter {
    pub fn seeded(seed: u64) -> Self {
        Self {
            rng: Xoshiro256::seeded(seed),
            clouds: BTreeMap::new(),
        }
    }
}

impl StateFilter for ParticleFilter {
    fn id(&self) -> &str {
        "particle-bootstrap"
    }
    fn kind(&self) -> EstimatorKind {
        EstimatorKind::Particle
    }
    fn propose(&mut self, e: &Evidence<'_>) -> Result<Option<Candidate>> {
        let z = e.mean();
        let r = e.measurement_variance;
        let cloud: Vec<f64> = match (self.clouds.get(e.key), e.prior) {
            (Some(cloud), Some(_)) => {
                let q = e.process_variance.sqrt();
                cloud
                    .iter()
                    .map(|x| x + self.rng.normal_with(0.0, q))
                    .collect()
            }
            // A worker whose cloud was lost (or never seeded) restarts from
            // the promoted prior rather than abstaining, so it keeps
            // contributing diversity.
            (_, Some((m, v))) => {
                let s = (v + e.process_variance).sqrt();
                (0..PARTICLES).map(|_| self.rng.normal_with(m, s)).collect()
            }
            (_, None) => {
                let s = r.sqrt();
                (0..PARTICLES).map(|_| self.rng.normal_with(z, s)).collect()
            }
        };
        let weights: Vec<f64> = cloud
            .iter()
            .map(|x| (-(z - x).powi(2) / (2.0 * r)).exp())
            .collect();
        let total: f64 = weights.iter().sum();
        if !(total.is_finite() && total > 0.0) {
            self.clouds.remove(e.key);
            return Err(Error::numeric(format!(
                "every particle for {} has negligible likelihood; the cloud collapsed and is \
                 reseeded next step",
                e.key
            )));
        }
        let mean = cloud.iter().zip(&weights).map(|(x, w)| x * w).sum::<f64>() / total;
        let variance = cloud
            .iter()
            .zip(&weights)
            .map(|(x, w)| w * (x - mean).powi(2))
            .sum::<f64>()
            / total;
        // Systematic resampling.
        let step = total / PARTICLES as f64;
        let mut target = self.rng.next_f64() * step;
        let mut acc = 0.0;
        let mut resampled = Vec::with_capacity(PARTICLES);
        for (x, w) in cloud.iter().zip(&weights) {
            acc += w;
            while target < acc && resampled.len() < PARTICLES {
                resampled.push(*x);
                target += step;
            }
        }
        while resampled.len() < PARTICLES {
            resampled.push(mean);
        }
        self.clouds.insert(e.key.clone(), resampled);
        Ok(Some(Candidate { mean, variance }))
    }
    fn forget(&mut self, key: &VariableKey) {
        self.clouds.remove(key);
    }
}

/// A robust filter: the readings' median rather than their mean, with a
/// measurement variance from their median absolute deviation, so one wild
/// source moves it far less than it moves the other two.
#[derive(Debug, Default)]
pub struct RobustFilter;

impl StateFilter for RobustFilter {
    fn id(&self) -> &str {
        "robust-median"
    }
    fn kind(&self) -> EstimatorKind {
        EstimatorKind::Robust
    }
    fn propose(&mut self, e: &Evidence<'_>) -> Result<Option<Candidate>> {
        let median = median(e.readings);
        let deviations: Vec<f64> = e.readings.iter().map(|r| (r - median).abs()).collect();
        let mad = median_of(deviations);
        let n = e.readings.len() as f64;
        let r = ((1.4826 * mad).powi(2) / n).max(e.measurement_variance / n.max(1.0));
        let r = if r > 0.0 { r } else { e.measurement_variance };
        Ok(Some(match e.prior {
            None => Candidate {
                mean: median,
                variance: r,
            },
            Some((m, v)) => {
                let p = v + e.process_variance;
                let gain = p / (p + r);
                Candidate {
                    mean: m + gain * (median - m),
                    variance: (1.0 - gain) * p,
                }
            }
        }))
    }
}

fn median(values: &[f64]) -> f64 {
    median_of(values.to_vec())
}

fn median_of(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// The right to promote. Obtainable only from [`NowStore::lead`], which
/// advances the epoch, so a reconciler holding an older token is fenced out
/// the moment another takes the lead (WORLD-9020).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FencingToken {
    epoch: u64,
}

impl FencingToken {
    pub const fn epoch(self) -> u64 {
        self.epoch
    }
}

/// The partitioned store of promoted state.
#[derive(Clone, Debug, Default)]
pub struct NowStore {
    epoch: u64,
    partitions: BTreeMap<PartitionKey, BTreeMap<VariableKey, NowVariable>>,
}

impl NowStore {
    /// Take the lead: a new epoch, and a token for it. Every older token is
    /// fenced from this instant.
    pub fn lead(&mut self) -> FencingToken {
        self.epoch += 1;
        FencingToken { epoch: self.epoch }
    }

    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    fn fenced(&self, token: FencingToken) -> Result<()> {
        if token.epoch != self.epoch {
            return Err(Error::denied(format!(
                "a reconciler at epoch {} may not promote; the leader is at epoch {}. Take the \
                 lead again rather than writing over the current leader",
                token.epoch, self.epoch
            )));
        }
        Ok(())
    }

    /// Promote one variable into its own partition and no other.
    pub fn promote(&mut self, token: FencingToken, variable: NowVariable) -> Result<()> {
        self.fenced(token)?;
        if variable.epoch != token.epoch {
            return Err(Error::invalid(
                "a variable stamped with another epoch cannot be promoted under this token",
            ));
        }
        self.partitions
            .entry(variable.partition.clone())
            .or_default()
            .insert(variable.key.clone(), variable);
        Ok(())
    }

    pub fn partition(&self, key: &PartitionKey) -> Option<&BTreeMap<VariableKey, NowVariable>> {
        self.partitions.get(key)
    }

    pub fn partitions(&self) -> impl Iterator<Item = &PartitionKey> {
        self.partitions.keys()
    }

    pub fn get(&self, partition: &PartitionKey, key: &VariableKey) -> Option<&NowVariable> {
        self.partitions.get(partition)?.get(key)
    }

    pub fn len(&self) -> usize {
        self.partitions.values().map(BTreeMap::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove every variable expired at `now`, returning their keys.
    fn expire(&mut self, now: Timestamp) -> Vec<VariableKey> {
        let mut gone = Vec::new();
        for vars in self.partitions.values_mut() {
            vars.retain(|k, v| {
                let live = v.expires_at > now;
                if !live {
                    gone.push(k.clone());
                }
                live
            });
        }
        self.partitions.retain(|_, v| !v.is_empty());
        gone
    }
}

/// A served read: the variable and its confidence at the read instant.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading<'a> {
    pub variable: &'a NowVariable,
    pub confidence_bp: u32,
    pub stale: bool,
}

/// How the brain is tuned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NowConfig {
    pub surprise_sigmas: f64,
    /// Estimator disagreement, in units of the variable's scale, past which
    /// it is recorded as a [`ModelDisagreement`].
    pub disagreement_scales: f64,
}

impl Default for NowConfig {
    fn default() -> Self {
        Self {
            surprise_sigmas: DEFAULT_SURPRISE_SIGMAS,
            disagreement_scales: 0.5,
        }
    }
}

/// Everything one step did.
#[derive(Clone, Debug, Default)]
pub struct NowStep {
    pub promoted: Vec<NowVariable>,
    pub deltas: Vec<BeliefDelta>,
    pub surprises: Vec<SurpriseEvent>,
    pub disagreements: Vec<ModelDisagreement>,
    /// Observations or variables refused, with the reason.
    pub refused: Vec<(String, String)>,
    /// Estimator failures this step, by worker id.
    pub worker_failures: BTreeMap<String, usize>,
    /// Candidates a symbolic constraint vetoed, by worker id.
    pub vetoed: BTreeMap<String, usize>,
    /// Variables expired and removed this step.
    pub expired: usize,
    /// Variables whose readings were all already absorbed.
    pub unchanged: usize,
}

/// The NOW Brain: catalogue, workers, the fenced store and calibration.
#[derive(Debug)]
pub struct NowBrain {
    config: NowConfig,
    catalogue: Catalogue,
    workers: Vec<Box<dyn StateFilter>>,
    store: NowStore,
    /// Predictive-interval hits and trials per partition: how often the
    /// readings landed inside the prior's 80% interval.
    coverage: BTreeMap<PartitionKey, (u64, u64)>,
    issued: u64,
}

impl NowBrain {
    /// A brain with no workers. Add them with [`Self::with_worker`].
    pub fn new(config: NowConfig, catalogue: Catalogue) -> Result<Self> {
        let positive = |x: f64| x.is_finite() && x > 0.0;
        if !(positive(config.surprise_sigmas) && positive(config.disagreement_scales)) {
            return Err(Error::invalid(
                "the surprise bound and disagreement bound must be finite and positive",
            ));
        }
        Ok(Self {
            config,
            catalogue,
            workers: Vec::new(),
            store: NowStore::default(),
            coverage: BTreeMap::new(),
            issued: 0,
        })
    }

    /// The three in-tree filters over the standard catalogue.
    pub fn standard(seed: u64) -> Result<Self> {
        Ok(Self::new(NowConfig::default(), Catalogue::standard()?)?
            .with_worker(Box::new(KalmanFilter))
            .with_worker(Box::new(ParticleFilter::seeded(seed)))
            .with_worker(Box::new(RobustFilter)))
    }

    pub fn with_worker(mut self, worker: Box<dyn StateFilter>) -> Self {
        self.workers.push(worker);
        self
    }

    /// Remove a worker by id, returning whether one was removed. Estimation
    /// continues on the rest (WORLD-9019).
    pub fn remove_worker(&mut self, id: &str) -> bool {
        let before = self.workers.len();
        self.workers.retain(|w| w.id() != id);
        self.workers.len() != before
    }

    pub fn workers(&self) -> Vec<(&str, EstimatorKind)> {
        self.workers.iter().map(|w| (w.id(), w.kind())).collect()
    }

    pub fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }

    pub fn catalogue_mut(&mut self) -> &mut Catalogue {
        &mut self.catalogue
    }

    pub fn store(&self) -> &NowStore {
        &self.store
    }

    /// Take the reconciler lead (WORLD-9020).
    pub fn lead(&mut self) -> FencingToken {
        self.store.lead()
    }

    /// Serve a variable at `now`: its confidence decayed by staleness, and
    /// refused once expired (WORLD-9018, WORLD-9021).
    pub fn read(
        &self,
        partition: &PartitionKey,
        key: &VariableKey,
        now: Timestamp,
    ) -> Result<Reading<'_>> {
        let variable = self.store.get(partition, key).ok_or_else(|| {
            Error::invalid(format!(
                "no NOW state for {key} in {partition}; nothing has been promoted"
            ))
        })?;
        if now >= variable.expires_at {
            return Err(Error::denied(format!(
                "the NOW state for {key} expired at {}; read a fresher estimate rather than \
                 acting on an expired one",
                variable.expires_at
            )));
        }
        let stale = now.since(variable.freshness.promoted_at) > variable.freshness.fresh_for;
        Ok(Reading {
            variable,
            confidence_bp: confidence_at(variable, now),
            stale,
        })
    }

    /// The calibration of a partition's predictive intervals: the absolute
    /// gap between the 80% interval's measured coverage and 0.8, over every
    /// update that had a prior. `Uncalibrated` until one has.
    pub fn calibration(&self, partition: &PartitionKey, at: Timestamp) -> Calibration {
        match self.coverage.get(partition) {
            Some(&(hits, trials)) if trials > 0 => {
                let rate = hits as f64 / trials as f64;
                Decimal::from_f64((rate - 0.8).abs()).map_or(Calibration::Uncalibrated, |score| {
                    Calibration::Measured {
                        method: "interval_coverage_80".to_string(),
                        score,
                        resolved: trials,
                        evaluated_at: at,
                    }
                })
            }
            _ => Calibration::Uncalibrated,
        }
    }

    /// One reconciler step at `now` under `token`.
    ///
    /// Refuses the whole step under a fenced token. Otherwise each
    /// observation is admitted or refused on its own — a reading knowable
    /// after `now`, a non-finite value, an undeclared name, a partition whose
    /// domain is not the name's — and each variable is estimated, vetoed,
    /// combined and promoted on its own, so one bad variable never stops the
    /// rest.
    pub fn step(
        &mut self,
        token: FencingToken,
        observations: Vec<Observation>,
        now: Timestamp,
    ) -> Result<NowStep> {
        self.store.fenced(token)?;
        let mut out = NowStep::default();
        for key in self.store.expire(now) {
            for w in &mut self.workers {
                w.forget(&key);
            }
            out.expired += 1;
        }
        let mut groups: BTreeMap<(PartitionKey, VariableKey), Vec<Observation>> = BTreeMap::new();
        for o in observations {
            let label = o.key.to_string();
            let refusal = if o.known_at > now {
                Some(format!(
                    "knowable at {} but read at {now}; a NOW state cannot use a value the \
                     platform did not yet have",
                    o.known_at
                ))
            } else if o.valid_at > o.known_at {
                Some("claims to have been known before it was true".to_string())
            } else if !o.value.is_finite() {
                Some("is not finite".to_string())
            } else {
                match self.catalogue.spec(&o.key.name) {
                    None => Some(format!("names `{}`, which is not declared", o.key.name)),
                    Some(spec) if spec.domain != o.partition.domain => Some(format!(
                        "is filed under {} but `{}` is a {} variable",
                        o.partition,
                        o.key.name,
                        spec.domain.as_str()
                    )),
                    Some(_) => None,
                }
            };
            if let Some(why) = refusal {
                out.refused.push((label, why));
                continue;
            }
            groups
                .entry((o.partition.clone(), o.key.clone()))
                .or_default()
                .push(o);
        }
        for ((partition, key), obs) in groups {
            if let Err(e) = self.update(token, &partition, &key, &obs, now, &mut out) {
                out.refused.push((key.to_string(), e.message().to_string()));
            }
        }
        Ok(out)
    }

    fn update(
        &mut self,
        token: FencingToken,
        partition: &PartitionKey,
        key: &VariableKey,
        obs: &[Observation],
        now: Timestamp,
        out: &mut NowStep,
    ) -> Result<()> {
        let spec = self
            .catalogue
            .spec(&key.name)
            .cloned()
            .ok_or_else(|| Error::invalid(format!("`{}` is not declared", key.name)))?;
        let prior_var = self.store.get(partition, key).cloned();
        if prior_var.is_none() && self.store.len() >= MAX_VARIABLES {
            return Err(Error::invalid(format!(
                "the NOW store holds {MAX_VARIABLES} variables; {key} is refused rather than \
                 grown past the bound"
            )));
        }
        // Idempotency: a reading no newer than what the promoted state already
        // rests on has been absorbed. Absorbing it again would count one
        // observation twice and shrink the uncertainty on no new evidence —
        // the shape a cycle that re-reads the same last bar would otherwise
        // take on every pass.
        let obs: Vec<&Observation> = match &prior_var {
            Some(p) => obs
                .iter()
                .filter(|o| o.valid_at > p.freshness.observed_at)
                .collect(),
            None => obs.iter().collect(),
        };
        if obs.is_empty() {
            out.unchanged += 1;
            return Ok(());
        }
        let readings: Vec<f64> = obs.iter().map(|o| o.value).collect();
        let n = readings.len() as f64;
        let mean_reading = readings.iter().sum::<f64>() / n;
        let scale = spec
            .scale
            .at(prior_var.as_ref().map_or(mean_reading, |p| p.mean));
        let spread = if readings.len() > 1 {
            readings
                .iter()
                .map(|r| (r - mean_reading).powi(2))
                .sum::<f64>()
                / (n - 1.0)
        } else {
            0.0
        };
        // The contradiction term: sources that split add their spread.
        let measurement_variance = (spec.reading_noise * scale).powi(2) / n + spread;
        let elapsed_days = prior_var.as_ref().map_or(0.0, |p| {
            now.since(p.freshness.promoted_at).as_days_f64().max(0.0)
        });
        let process_variance = (spec.drift_per_day * scale).powi(2) * elapsed_days;
        let prior = prior_var.as_ref().map(|p| (p.mean, p.uncertainty));
        let evidence = Evidence {
            key,
            prior,
            readings: &readings,
            measurement_variance,
            process_variance,
        };
        let mut candidates: Vec<(String, Candidate)> = Vec::new();
        for w in &mut self.workers {
            match w.propose(&evidence) {
                Ok(Some(c))
                    if c.mean.is_finite() && c.variance.is_finite() && c.variance >= 0.0 =>
                {
                    if spec.constraint.admits(c.mean) {
                        candidates.push((w.id().to_string(), c));
                    } else {
                        *out.vetoed.entry(w.id().to_string()).or_default() += 1;
                    }
                }
                Ok(Some(_)) | Err(_) => {
                    *out.worker_failures.entry(w.id().to_string()).or_default() += 1;
                }
                Ok(None) => {}
            }
        }
        if candidates.is_empty() {
            return Err(Error::invalid(format!(
                "no estimator produced an admissible candidate for {key} (constraint {}); the \
                 last promoted state stands until its expiry",
                spec.constraint.as_str()
            )));
        }
        let k = candidates.len() as f64;
        let mean = candidates.iter().map(|(_, c)| c.mean).sum::<f64>() / k;
        // Candidates share their data, so their precisions do not add: the
        // ensemble is as uncertain as its average member, plus the spread
        // between members.
        let within = candidates.iter().map(|(_, c)| c.variance).sum::<f64>() / k;
        let between = candidates
            .iter()
            .map(|(_, c)| (c.mean - mean).powi(2))
            .sum::<f64>()
            / k;
        let variance = within + between;
        let (lo, hi) = candidates
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (_, c)| {
                (lo.min(c.mean), hi.max(c.mean))
            });
        let disagreement = if candidates.len() > 1 { hi - lo } else { 0.0 };

        let manifests: Vec<ManifestRef> = obs
            .iter()
            .map(|o| o.manifest.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let sources: BTreeSet<&str> = obs.iter().map(|o| o.source.as_str()).collect();

        if let Some((pm, pv)) = prior {
            let predictive = pv + process_variance + measurement_variance;
            let sigmas = if predictive > 0.0 {
                (mean_reading - pm) / predictive.sqrt()
            } else {
                0.0
            };
            let hit = sigmas.abs() <= Z_80;
            let entry = self.coverage.entry(partition.clone()).or_insert((0, 0));
            entry.1 += 1;
            if hit {
                entry.0 += 1;
            }
            if sigmas.abs() >= self.config.surprise_sigmas {
                self.issued += 1;
                out.surprises.push(SurpriseEvent {
                    id: format!("now-surprise-{}", self.issued),
                    key: key.clone(),
                    partition: partition.clone(),
                    sigmas,
                    prior_mean: pm,
                    posterior_mean: mean,
                    at: now,
                    wakes: Specialist::ALL.to_vec(),
                    manifests: manifests.clone(),
                });
            }
        }

        if candidates.len() > 1 && disagreement > self.config.disagreement_scales * scale {
            self.issued += 1;
            let views = candidates
                .iter()
                .map(|(id, _)| DisputingView {
                    id: id.clone(),
                    supporting_evidence: manifests.iter().map(ManifestRef::lineage).collect(),
                    calibration: Calibration::Uncalibrated,
                })
                .collect();
            out.disagreements
                .push(ModelDisagreement::new(ModelDisagreementFields {
                    schema_version: ModelDisagreementFields::SCHEMA_VERSION,
                    disagreement_id: format!("now-disagreement-{}", self.issued),
                    views,
                    disputed_propositions: vec![format!("the latent value of {key}")],
                    unresolved_assumptions: vec![
                        "each estimator's noise model fits these readings".to_string(),
                    ],
                    status: ArbitrationStatus::Open,
                    raised_at: now,
                })?);
        }

        let confidence = scale.powi(2) / (scale.powi(2) + variance);
        // Statistic to integer: a fraction rounded to basis points.
        let confidence_bp = (confidence * f64::from(MAX_BP)).round() as u32;
        let rate_of_change = match &prior_var {
            Some(p) if elapsed_days > 0.0 => RateOfChange::PerDay {
                value: (mean - p.mean) / elapsed_days,
            },
            _ => RateOfChange::Unmeasured,
        };
        let observed_at = obs.iter().map(|o| o.valid_at).max().unwrap_or(now);
        let known_at = obs.iter().map(|o| o.known_at).max().unwrap_or(now);
        let variable = NowVariable::new(NowVariableFields {
            schema_version: NOW_SCHEMA_VERSION,
            key: key.clone(),
            partition: partition.clone(),
            mean,
            uncertainty: variance,
            confidence_bp,
            freshness: Freshness {
                observed_at,
                known_at,
                promoted_at: now,
                fresh_for: spec.fresh_for,
            },
            rate_of_change,
            source_diversity: u32::try_from(sources.len()).unwrap_or(u32::MAX),
            ensemble: u32::try_from(candidates.len()).unwrap_or(u32::MAX),
            disagreement,
            epoch: token.epoch,
            manifests: manifests.clone(),
            expires_at: now.saturating_add(spec.expires_after),
        })?;
        let delta = BeliefDelta::new(BeliefDeltaFields {
            schema_version: NOW_SCHEMA_VERSION,
            key: key.clone(),
            partition: partition.clone(),
            epoch: token.epoch,
            prior: prior.map_or(Prior::Genesis, |(mean, variance)| Prior::Was {
                mean,
                variance,
            }),
            posterior_mean: mean,
            posterior_variance: variance,
            confidence_bp,
            at: now,
            manifests,
        })?;
        self.store.promote(token, variable.clone())?;
        out.promoted.push(variable);
        out.deltas.push(delta);
        Ok(())
    }

    /// The §11.2 `GlobalState`: every live global-scope variable.
    pub fn global_state(&self, now: Timestamp) -> GlobalState {
        let variables = self
            .live(now)
            .filter(|v| v.key.scope.kind == ScopeKind::Global)
            .cloned()
            .collect();
        GlobalState {
            at: now,
            epoch: self.store.epoch(),
            variables,
        }
    }

    /// The §11.2 `RegionalState` for `region`: every live variable in a
    /// partition of that region.
    pub fn regional_state(&self, region: &str, now: Timestamp) -> RegionalState {
        let variables = self
            .live(now)
            .filter(|v| v.partition.region == region)
            .cloned()
            .collect();
        RegionalState {
            region: region.to_string(),
            at: now,
            epoch: self.store.epoch(),
            variables,
        }
    }

    fn live(&self, now: Timestamp) -> impl Iterator<Item = &NowVariable> {
        self.store
            .partitions
            .values()
            .flat_map(BTreeMap::values)
            .filter(move |v| v.expires_at > now)
    }

    /// One partition as lane H's §27 [`WorldModelState`]: each live variable
    /// as its central 80% interval and median, the manifests as lineage, and
    /// the partition's measured interval coverage as calibration.
    pub fn world_model_state(
        &self,
        partition: &PartitionKey,
        now: Timestamp,
    ) -> Result<WorldModelState> {
        let vars: Vec<&NowVariable> = self
            .store
            .partition(partition)
            .map(|p| p.values().filter(|v| v.expires_at > now).collect())
            .unwrap_or_default();
        if vars.is_empty() {
            return Err(Error::invalid(format!(
                "partition {partition} holds no live state to project"
            )));
        }
        let decimal = |x: f64| {
            Decimal::from_f64(x).ok_or_else(|| {
                Error::numeric(format!("{x} has no decimal form for a world-model state"))
            })
        };
        let mut state = Vec::new();
        let mut lineage = BTreeSet::new();
        let mut horizon = Duration::from_days(36_500);
        let mut updated_at = Timestamp::from_nanos(0);
        let mut expires_at = Timestamp::from_nanos(i64::MAX);
        for v in &vars {
            let sd = v.uncertainty.sqrt();
            state.push(StateVariable {
                name: v.key.to_string(),
                distribution: OutcomeDistribution::Quantiles {
                    points: vec![
                        QuantilePoint {
                            quantile_bp: 1_000,
                            value: decimal(v.mean - Z_80 * sd)?,
                        },
                        QuantilePoint {
                            quantile_bp: 5_000,
                            value: decimal(v.mean)?,
                        },
                        QuantilePoint {
                            quantile_bp: 9_000,
                            value: decimal(v.mean + Z_80 * sd)?,
                        },
                    ],
                },
            });
            lineage.extend(v.manifests.iter().map(ManifestRef::lineage));
            horizon = horizon.min(v.freshness.fresh_for);
            updated_at = updated_at.max(v.freshness.promoted_at);
            expires_at = expires_at.min(v.expires_at);
        }
        WorldModelState::new(WorldModelStateFields {
            schema_version: WorldModelStateFields::SCHEMA_VERSION,
            model: "now-brain/v1".to_string(),
            scope: partition.to_string(),
            horizon,
            state,
            causal_structure: Vec::new(),
            evidence_lineage: lineage.into_iter().collect(),
            assumptions: vec![
                "each latent value follows a local-level random walk between observations"
                    .to_string(),
                "sources are conditionally independent given the latent value".to_string(),
            ],
            calibration: self.calibration(partition, now),
            branch_parent: BranchParent::Root,
            updated_at,
            expires_at,
        })
    }
}

/// The §11.2 `GlobalState` contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalState {
    pub at: Timestamp,
    pub epoch: u64,
    pub variables: Vec<NowVariable>,
}

/// The §11.2 `RegionalState` contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionalState {
    pub region: String,
    pub at: Timestamp,
    pub epoch: u64,
    pub variables: Vec<NowVariable>,
}

/// Company-scope readings from the world model (WORLD-3077's company scope):
/// every feature value readable at `now` in both time dimensions whose
/// subject is an entity in the knowledge graph, as `(feature, entity, value)`.
/// Macro series are excluded; they are economy state, not a company's.
pub fn company_features(world: &WorldModel, now: Timestamp) -> Vec<(String, String, f64)> {
    use crate::graph::NodeKind;
    let entities: BTreeSet<String> = world
        .graph()
        .nodes_of_kind(NodeKind::Entity)
        .into_iter()
        .map(|n| n.id.clone())
        .collect();
    let state = world.state_at(now, now);
    state
        .features
        .into_iter()
        .filter_map(|(name, value)| {
            let (feature, subject) = name.split_once('/')?;
            if feature.starts_with("macro") || !entities.contains(subject) || !value.is_finite() {
                return None;
            }
            Some((feature.to_string(), subject.to_string(), value))
        })
        .collect()
}

/// Macro surprises readable at `now`, as `(series, value)`.
pub fn macro_surprises(world: &WorldModel, now: Timestamp) -> Vec<(String, f64)> {
    let prefix = format!("{}/", crate::vocabulary::names::MACRO_SURPRISE);
    world
        .state_at(now, now)
        .features
        .into_iter()
        .filter_map(|(name, value)| {
            let series = name.strip_prefix(&prefix)?;
            value.is_finite().then(|| (series.to_string(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(days: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000).saturating_add(Duration::from_days(days))
    }

    #[test]
    fn the_confidence_of_a_variable_halves_for_every_freshness_span_it_has_aged_past_fresh() {
        let v = NowVariableFields {
            schema_version: 1,
            key: VariableKey::new(Scope::global(), "return").unwrap_or_else(|e| panic!("{e}")),
            partition: PartitionKey::new(Domain::Markets, "global")
                .unwrap_or_else(|e| panic!("{e}")),
            mean: 0.0,
            uncertainty: 0.0,
            confidence_bp: 8_000,
            freshness: Freshness {
                observed_at: t(0),
                known_at: t(0),
                promoted_at: t(0),
                fresh_for: Duration::from_days(1),
            },
            rate_of_change: RateOfChange::Unmeasured,
            source_diversity: 1,
            ensemble: 1,
            disagreement: 0.0,
            epoch: 1,
            manifests: vec![ManifestRef::new("s", "r").unwrap_or_else(|e| panic!("{e}"))],
            expires_at: t(9),
        };
        assert_eq!(confidence_at(&v, t(1)), 8_000);
        assert_eq!(confidence_at(&v, t(2)), 4_000);
        assert_eq!(confidence_at(&v, t(3)), 2_000);
    }
}
