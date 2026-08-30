//! Scenarios and stress testing.
//!
//! Two kinds of question, kept apart because they answer different things:
//!
//! * A **historical scenario** replays what actually happened. It is credible
//!   precisely because it occurred, and limited for the same reason — the next
//!   crisis will not be the last one.
//! * A **hypothetical scenario** states a shock directly. It can ask about
//!   things that have never happened, at the cost of someone having chosen the
//!   numbers.
//!
//! Both propagate through the same mechanism: a shock to a factor moves a
//! position by its exposure to that factor. Correlations are shocked too,
//! because the defining feature of a crisis is that diversification stops
//! working, and a stress test holding correlations at their calm-period values
//! is a stress test of a portfolio nobody owns.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A shock to one factor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FactorShock {
    pub factor: String,
    /// Move in the factor, as a fraction. Negative is down.
    pub magnitude: f64,
    /// How long the move takes. A move over a day and the same move over a
    /// quarter are different events for a portfolio that can trade.
    pub over_days: f64,
}

impl FactorShock {
    pub fn new(factor: impl Into<String>, magnitude: f64, over_days: f64) -> Self {
        Self {
            factor: factor.into(),
            magnitude,
            over_days: over_days.max(0.0),
        }
    }
}

/// A named stress scenario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    /// What the scenario represents, and where its numbers came from.
    ///
    /// Required and checked: a scenario whose provenance nobody can state is a
    /// scenario nobody can argue with.
    pub description: String,
    pub shocks: Vec<FactorShock>,
    /// Correlation between all shocked factors during the event, replacing the
    /// calm-period estimate.
    pub stressed_correlation: Option<f64>,
    /// Multiplier on transaction costs during the event.
    pub liquidity_multiplier: f64,
    /// Whether this actually happened.
    pub historical: bool,
}

impl Scenario {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::invalid("a scenario needs a name"));
        }
        if self.description.trim().len() < 20 {
            return Err(Error::invalid(format!(
                "scenario {} has no stated provenance; a scenario nobody can argue with is not a control",
                self.name
            )));
        }
        if self.shocks.is_empty() {
            return Err(Error::invalid(format!(
                "scenario {} shocks nothing",
                self.name
            )));
        }
        if let Some(correlation) = self.stressed_correlation
            && !(-1.0..=1.0).contains(&correlation)
        {
            return Err(Error::invalid(format!(
                "scenario {} has a correlation outside [-1, 1]",
                self.name
            )));
        }
        if self.liquidity_multiplier < 1.0 {
            return Err(Error::invalid(format!(
                "scenario {} makes liquidity better under stress, which is not how stress works",
                self.name
            )));
        }
        Ok(())
    }

    pub fn shock_for(&self, factor: &str) -> Option<&FactorShock> {
        self.shocks.iter().find(|shock| shock.factor == factor)
    }

    /// The largest absolute shock in the scenario.
    pub fn severity(&self) -> f64 {
        self.shocks
            .iter()
            .map(|shock| shock.magnitude.abs())
            .fold(0.0_f64, f64::max)
    }
}

/// The standard scenario set.
///
/// Historical magnitudes are the realised peak-to-trough moves of the episodes
/// named, rounded, over the windows stated. They are a starting point for a
/// risk committee to argue with, not a claim about what will happen next.
pub fn standard_library() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "equity-crash-1987".to_string(),
            description:
                "October 1987: a one-day equity move with no macro trigger, testing whether the book survives a gap rather than a trend"
                    .to_string(),
            shocks: vec![
                FactorShock::new("equity", -0.22, 1.0),
                FactorShock::new("volatility", 1.5, 1.0),
                FactorShock::new("rates", -0.005, 1.0),
            ],
            stressed_correlation: Some(0.85),
            liquidity_multiplier: 5.0,
            historical: true,
        },
        Scenario {
            name: "credit-crisis-2008".to_string(),
            description:
                "September 2008 to March 2009: a prolonged credit and equity decline with funding markets closed, testing survival rather than a single day"
                    .to_string(),
            shocks: vec![
                FactorShock::new("equity", -0.50, 180.0),
                FactorShock::new("credit", 0.06, 180.0),
                FactorShock::new("volatility", 2.0, 180.0),
                FactorShock::new("rates", -0.03, 180.0),
            ],
            stressed_correlation: Some(0.90),
            liquidity_multiplier: 8.0,
            historical: true,
        },
        Scenario {
            name: "pandemic-2020".to_string(),
            description:
                "February to March 2020: the fastest equity drawdown on record followed by an unprecedented policy response, testing both directions"
                    .to_string(),
            shocks: vec![
                FactorShock::new("equity", -0.34, 33.0),
                FactorShock::new("credit", 0.05, 33.0),
                FactorShock::new("volatility", 3.0, 33.0),
                FactorShock::new("rates", -0.012, 33.0),
            ],
            stressed_correlation: Some(0.88),
            liquidity_multiplier: 6.0,
            historical: true,
        },
        Scenario {
            name: "inflation-shock-2022".to_string(),
            description:
                "2022: equities and bonds falling together as rates repriced, the case that breaks a portfolio relying on the equity-bond hedge"
                    .to_string(),
            shocks: vec![
                FactorShock::new("equity", -0.25, 270.0),
                FactorShock::new("rates", 0.035, 270.0),
                FactorShock::new("credit", 0.025, 270.0),
                FactorShock::new("commodity", 0.40, 270.0),
            ],
            stressed_correlation: Some(0.70),
            liquidity_multiplier: 3.0,
            historical: true,
        },
        Scenario {
            name: "liquidity-freeze".to_string(),
            description:
                "Hypothetical: modest price moves with transaction costs an order of magnitude wider, testing whether the book can be exited at all"
                    .to_string(),
            shocks: vec![
                FactorShock::new("equity", -0.08, 5.0),
                FactorShock::new("volatility", 1.0, 5.0),
            ],
            stressed_correlation: Some(0.95),
            liquidity_multiplier: 15.0,
            historical: false,
        },
        Scenario {
            name: "correlation-breakdown".to_string(),
            description:
                "Hypothetical: every diversifying relationship goes to one while prices fall, the scenario in which a risk model built on calm-period correlations is most wrong"
                    .to_string(),
            shocks: vec![
                FactorShock::new("equity", -0.15, 10.0),
                FactorShock::new("credit", 0.03, 10.0),
                FactorShock::new("commodity", -0.15, 10.0),
                FactorShock::new("fx", -0.10, 10.0),
            ],
            stressed_correlation: Some(0.99),
            liquidity_multiplier: 4.0,
            historical: false,
        },
    ]
}

/// What a scenario did to one position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionImpact {
    pub object_id: String,
    /// Notional before the shock.
    pub notional: f64,
    /// Change in value, in currency units.
    pub profit_and_loss: f64,
    /// The factors that moved it, and by how much each contributed.
    pub attribution: BTreeMap<String, f64>,
}

/// What a scenario did to the book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario: String,
    pub at: Timestamp,
    pub equity_before: f64,
    pub equity_after: f64,
    /// Loss as a positive fraction of equity.
    pub loss_fraction: f64,
    pub positions: Vec<PositionImpact>,
    /// Cost of exiting the book at stressed liquidity.
    pub liquidation_cost: f64,
    /// Positions with no exposure recorded to any shocked factor.
    ///
    /// Not the same as unaffected. A position the model has no exposures for
    /// shows zero loss because nothing was measured, and reporting that as
    /// safety is how a stress test understates a risk.
    pub unmodelled: Vec<String>,
}

impl ScenarioResult {
    /// Whether the loss exceeds a stated tolerance.
    pub fn breaches(&self, tolerance: f64) -> bool {
        self.loss_fraction > tolerance
    }

    /// The positions that lost the most.
    pub fn worst(&self, limit: usize) -> Vec<&PositionImpact> {
        let mut ranked: Vec<&PositionImpact> = self.positions.iter().collect();
        ranked.sort_by(|a, b| {
            a.profit_and_loss
                .partial_cmp(&b.profit_and_loss)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked.truncate(limit);
        ranked
    }

    pub fn summarise(&self) -> String {
        let coverage = if self.positions.is_empty() {
            String::new()
        } else {
            format!(
                ", {} of {} position(s) unmodelled",
                self.unmodelled.len(),
                self.positions.len() + self.unmodelled.len()
            )
        };
        format!(
            "{}: {:.2}% loss ({:.0} of {:.0}), {:.0} to liquidate at stressed costs{coverage}",
            self.scenario,
            self.loss_fraction * 100.0,
            self.equity_before - self.equity_after,
            self.equity_before,
            self.liquidation_cost
        )
    }
}

/// A position's exposure to the shocked factors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FactorExposure {
    pub object_id: String,
    /// Notional value of the position.
    pub notional: Decimal,
    /// Sensitivity to each factor: a beta for equity, a duration for rates,
    /// a spread duration for credit.
    pub betas: BTreeMap<String, f64>,
}

/// Applies scenarios to a book.
#[derive(Debug)]
pub struct StressTester {
    /// Base transaction cost in basis points, multiplied by the scenario's
    /// liquidity factor to price the exit.
    base_cost_bps: f64,
}

impl StressTester {
    pub fn new(base_cost_bps: f64) -> Self {
        Self { base_cost_bps }
    }

    /// Apply one scenario to a set of exposures.
    pub fn apply(
        &self,
        scenario: &Scenario,
        exposures: &[FactorExposure],
        equity: f64,
        at: Timestamp,
    ) -> Result<ScenarioResult> {
        scenario.validate()?;
        if equity <= 0.0 {
            return Err(Error::invalid("cannot stress a book with no equity"));
        }

        let mut positions = Vec::new();
        let mut unmodelled = Vec::new();
        let mut total_pnl = 0.0;
        let mut gross = 0.0;

        for exposure in exposures {
            let notional = exposure.notional.to_f64();
            gross += notional.abs();

            let mut attribution = BTreeMap::new();
            let mut position_pnl = 0.0;
            let mut touched = false;

            for shock in &scenario.shocks {
                let Some(beta) = exposure.betas.get(&shock.factor) else {
                    continue;
                };
                touched = true;
                // Rates and credit shocks are quoted in absolute terms and the
                // beta is a duration, so the sign is inverted: yields up,
                // prices down. Equity-style shocks are proportional.
                let contribution = match shock.factor.as_str() {
                    "rates" | "credit" => -notional * beta * shock.magnitude,
                    _ => notional * beta * shock.magnitude,
                };
                attribution.insert(shock.factor.clone(), contribution);
                position_pnl += contribution;
            }

            if !touched {
                unmodelled.push(exposure.object_id.clone());
                continue;
            }

            total_pnl += position_pnl;
            positions.push(PositionImpact {
                object_id: exposure.object_id.clone(),
                notional,
                profit_and_loss: position_pnl,
                attribution,
            });
        }

        // Exiting the book costs more when everyone else is exiting too.
        let liquidation_cost =
            gross * self.base_cost_bps * scenario.liquidity_multiplier / 10_000.0;

        let equity_after = equity + total_pnl - liquidation_cost;
        Ok(ScenarioResult {
            scenario: scenario.name.clone(),
            at,
            equity_before: equity,
            equity_after,
            loss_fraction: ((equity - equity_after) / equity).max(0.0),
            positions,
            liquidation_cost,
            unmodelled,
        })
    }

    /// Apply every scenario, worst loss first.
    pub fn apply_all(
        &self,
        scenarios: &[Scenario],
        exposures: &[FactorExposure],
        equity: f64,
        at: Timestamp,
    ) -> Result<Vec<ScenarioResult>> {
        let mut results = Vec::with_capacity(scenarios.len());
        for scenario in scenarios {
            results.push(self.apply(scenario, exposures, equity, at)?);
        }
        results.sort_by(|a, b| {
            b.loss_fraction
                .partial_cmp(&a.loss_fraction)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(results)
    }
}
