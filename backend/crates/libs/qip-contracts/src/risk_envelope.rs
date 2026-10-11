//! `RiskEnvelope` (blueprint v12.0 §27, R12-CONTRACT-7067): the risk pack a
//! reflex cell holds, typed.
//!
//! What travels today is `policy::RiskEnvelopeSnapshot`, whose limits are an
//! opaque `serde_json::Value` the risk engine owns the schema of. That is
//! honest about the shape conflict, but it means a cell can receive an
//! envelope with no scenario limit, no health requirement and no emergency
//! mode and nothing between the brains notices. This is the typed record the
//! blueprint lists: hard limits, scenario limits, causal-driver limits, the
//! model and data health the cell must see before trading, and the
//! emergency mode in force. The snapshot is left in place; moving the
//! payload onto this type is a change to the policy frame and the risk
//! engine together, and is not made inside a contract file.

use crate::validated::{
    Contract, Validated, after, basis_points, each_named, listed, named, positive,
    positive_duration, refuse,
};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A ceiling on one measured quantity (gross notional, single-name
/// concentration, order rate, …), in that quantity's own units.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardLimit {
    pub metric: String,
    pub max: Decimal,
}

/// The most the book may lose under one named stress scenario.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioLimit {
    pub scenario: String,
    pub max_loss: Decimal,
}

/// The most exposure the book may hold to one causal driver (rates, oil,
/// a named counterparty), in the account currency.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriverLimit {
    pub driver: String,
    pub max_exposure: Decimal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthSubject {
    Model,
    Data,
}

/// What a model or feed must show before the cell may trade on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthRequirement {
    pub subject: HealthSubject,
    /// The model or feed id.
    pub id: String,
    /// Older than this and the subject is unhealthy.
    pub max_staleness: Duration,
    /// The least calibration (models) or completeness (data), in basis points.
    pub min_quality_bp: u32,
}

/// The emergency posture in force. `Normal` is stated, never assumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmergencyMode {
    Normal,
    /// Orders may only reduce exposure.
    ReduceOnly,
    /// Flatten everything, then halt.
    Flatten,
    Halt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskEnvelopeFields {
    pub schema_version: u32,
    pub envelope_id: String,
    /// At least one.
    pub hard_limits: Vec<HardLimit>,
    /// At least one.
    pub scenario_limits: Vec<ScenarioLimit>,
    /// May be empty: a statement that no causal driver is limited.
    pub driver_limits: Vec<DriverLimit>,
    /// At least one.
    pub health_requirements: Vec<HealthRequirement>,
    pub emergency_mode: EmergencyMode,
    pub issued_at: Timestamp,
    pub expires_at: Timestamp,
}

/// The §27 typed risk envelope. Built only through [`Validated::new`].
pub type RiskEnvelope = Validated<RiskEnvelopeFields>;

fn unique<'a>(contract: &str, field: &str, keys: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for key in keys {
        named(contract, field, key)?;
        if !seen.insert(key) {
            return refuse(
                contract,
                format!("states `{key}` twice in `{field}`; two limits on one thing disagree"),
            );
        }
    }
    Ok(())
}

impl Contract for RiskEnvelopeFields {
    const NAME: &'static str = "RiskEnvelope";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank id; no hard limit, scenario limit or health
    /// requirement; a limit that is not positive (a zero ceiling is a halt
    /// written as a limit, and a negative one can never be met); a limit
    /// stated twice for one metric, scenario, driver or subject; a
    /// non-positive staleness; a quality above one; and an expiry not after
    /// issue.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "envelope_id", &self.envelope_id)?;
        listed(c, "hard_limits", &self.hard_limits)?;
        unique(
            c,
            "hard_limits.metric",
            self.hard_limits.iter().map(|l| l.metric.as_str()),
        )?;
        for l in &self.hard_limits {
            positive(c, "hard_limits.max", l.max)?;
        }
        listed(c, "scenario_limits", &self.scenario_limits)?;
        unique(
            c,
            "scenario_limits.scenario",
            self.scenario_limits.iter().map(|l| l.scenario.as_str()),
        )?;
        for l in &self.scenario_limits {
            positive(c, "scenario_limits.max_loss", l.max_loss)?;
        }
        unique(
            c,
            "driver_limits.driver",
            self.driver_limits.iter().map(|l| l.driver.as_str()),
        )?;
        for l in &self.driver_limits {
            positive(c, "driver_limits.max_exposure", l.max_exposure)?;
        }
        listed(c, "health_requirements", &self.health_requirements)?;
        let ids: Vec<String> = self
            .health_requirements
            .iter()
            .map(|h| format!("{:?}:{}", h.subject, h.id))
            .collect();
        each_named(c, "health_requirements", &ids)?;
        for h in &self.health_requirements {
            named(c, "health_requirements.id", &h.id)?;
            positive_duration(c, "health_requirements.max_staleness", h.max_staleness)?;
            basis_points(c, "health_requirements.min_quality_bp", h.min_quality_bp)?;
        }
        after(
            c,
            "expires_at",
            self.expires_at,
            "issued_at",
            self.issued_at,
        )
    }
}
