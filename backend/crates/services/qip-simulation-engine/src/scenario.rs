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
use std::collections::{BTreeMap, BTreeSet};

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
    /// Factors the scenario shocks that no position in the book carries a
    /// beta for, so the shock contributed to nobody's profit and loss.
    ///
    /// The same gap `unmodelled` reports for a position, one level up: a
    /// scenario naming a commodity shock over a book with no recorded
    /// commodity exposure is indistinguishable, in `loss_fraction` alone,
    /// from one where that shock happened to net to zero. It did not net to
    /// zero — it was never applied to anything, and a risk committee reading
    /// the loss figure has no way to know that unless it is named here.
    pub unmodelled_factors: Vec<String>,
    /// The correlation the scenario assumed between its shocked factors,
    /// carried through from [`Scenario::stressed_correlation`] so a reader of
    /// the result alone — without the scenario definition beside it — can
    /// see what regime the number rests on, the same way `liquidity_multiplier`
    /// is visible through `liquidation_cost`.
    pub stressed_correlation: Option<f64>,
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
        let unmodelled_factors = if self.unmodelled_factors.is_empty() {
            String::new()
        } else {
            format!(
                ", {} shocked factor(s) with no exposure in the book: {}",
                self.unmodelled_factors.len(),
                self.unmodelled_factors.join(", ")
            )
        };
        let correlation = match self.stressed_correlation {
            Some(correlation) => format!(", correlation {correlation:.2}"),
            None => String::new(),
        };
        format!(
            "{}: {:.2}% loss ({:.0} of {:.0}), {:.0} to liquidate at stressed costs{coverage}{unmodelled_factors}{correlation}",
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

        // A shock the scenario names but that touched no position's betas
        // contributed nothing to `total_pnl` — indistinguishable, from the
        // loss figure alone, from a shock whose contribution genuinely
        // netted to zero. Naming it is what tells the two apart.
        let exposed_factors: BTreeSet<&str> = exposures
            .iter()
            .flat_map(|exposure| exposure.betas.keys())
            .map(String::as_str)
            .collect();
        let unmodelled_factors: Vec<String> = scenario
            .shocks
            .iter()
            .map(|shock| shock.factor.as_str())
            .collect::<BTreeSet<&str>>()
            .into_iter()
            .filter(|factor| !exposed_factors.contains(factor))
            .map(str::to_string)
            .collect();

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
            unmodelled_factors,
            stressed_correlation: scenario.stressed_correlation,
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

// ---------------------------------------------------------------------------
// Blueprint §23.7's third and fourth methods.
//
// The library above answers two of the section's four rows: historical
// replay (shocks that occurred, at the correlations that held then) and
// correlation stress (shocks scaled beyond history, assuming the same
// structure). Both reach a position only through a beta to a named factor,
// so a position the factor model cannot measure is stressed by nothing, and
// a position two mechanisms downstream of a driver the library never names
// is stressed as if that driver did not exist. The two constructions below
// are the rows that close those gaps, and each states what it cannot do.
// ---------------------------------------------------------------------------

/// A move that arrived at a node by propagation through the causal graph.
///
/// The simulation engine does not depend on the world model, so the walk
/// happens elsewhere and its result crosses this seam as plain facts: which
/// node moved, by how much, how many hops from the origin, and how long
/// after the origin's own move. `magnitude` is a signed fraction of the
/// target's own price, with every transmission and sign flip along the path
/// already applied — which is why [`causal_exposures`] loads a position on
/// its own node at exactly one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropagatedShock {
    pub target: String,
    pub magnitude: f64,
    /// Hops from the origin; zero is the origin itself.
    pub order: usize,
    pub over_days: f64,
}

/// Where the size of a driver's shock came from.
///
/// Recorded on the scenario rather than left in the caller, because a
/// causal stress whose origin shock was invented reads identically, in its
/// loss figure, to one whose origin shock was measured — and the section's
/// own rationale for the library is that a scenario nobody can argue with is
/// not a control.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverShockSizing {
    /// The largest single-period move the platform's own tape holds for the
    /// driver: a move that happened, so nobody has to defend its plausibility.
    ObservedWorstPeriod,
    /// [`STANDARD_DRIVER_SHOCK`], for a driver the tape has never priced — a
    /// macro node a document claimed, or an instrument with one close.
    StandardDriverShock,
}

impl DriverShockSizing {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ObservedWorstPeriod => "observed_worst_period",
            Self::StandardDriverShock => "standard_driver_shock",
        }
    }
}

/// The shock applied at a driver the tape cannot size, as a fraction.
///
/// Ten percent is the round figure between the library's mildest hypothetical
/// equity move (`liquidity-freeze`, 8%) and its mildest historical one
/// (`correlation-breakdown`, 15%): large enough that a chain of two
/// mechanisms at half transmission each still clears the propagation floor,
/// small enough that it is not a claim about a crisis. It is a number someone
/// chose, and [`DriverShockSizing::StandardDriverShock`] on the scenario says
/// so wherever it was used.
pub const STANDARD_DRIVER_SHOCK: f64 = 0.10;

/// The name prefix every causal scenario carries, so a reader of a report can
/// tell the section's third method from its first two without the definition
/// beside it.
pub const CAUSAL_SCENARIO_PREFIX: &str = "causal:";

/// The name of the section's fourth method's scenario, fixed so the chart it
/// reaches carries a bounded label.
pub const ADVERSARIAL_SCENARIO_NAME: &str = "adversarial-worst-plausible";

/// The two factor names [`StressTester::apply`] reads as yield moves and
/// signs the other way. A propagated shock is a price move and must never be
/// filed under either, so a causal target with one of these names is refused
/// rather than silently inverted.
const YIELD_QUOTED_FACTORS: [&str; 2] = ["rates", "credit"];

/// Build the section's third method from one propagation: a shock at
/// `origin`, walked through mechanisms, landing on whichever of `exposures`
/// it reached.
///
/// Returns `Ok(None)` when the walk reached no held position — a driver the
/// book does not sit downstream of is not a scenario, and a scenario shocking
/// nothing would be refused by [`Scenario::validate`] anyway. The origin
/// itself is a target when it is held: a driver the book holds moves by the
/// initial shock before anything downstream does.
///
/// Every shock in the result is filed under the *target's own id* rather than
/// a factor name, which is the whole difference from the library: the
/// position is reached because a path in the graph reaches it, not because a
/// regression over the tape gave it a beta. Apply the result to
/// [`causal_exposures`] of the same book, never to the factor-loaded ones.
pub fn causal_scenario(
    origin: &str,
    initial_shock: f64,
    sizing: DriverShockSizing,
    propagated: &[PropagatedShock],
    exposures: &[FactorExposure],
) -> Result<Option<Scenario>> {
    if origin.trim().is_empty() {
        return Err(Error::invalid(
            "a causal scenario needs a driver to shock; an origin with no name cannot be \
             propagated from",
        ));
    }
    if !initial_shock.is_finite() || initial_shock == 0.0 {
        return Err(Error::invalid(format!(
            "a driver shock of {initial_shock} at {origin} is not a move; size it from the tape \
             or from STANDARD_DRIVER_SHOCK"
        )));
    }
    let held: BTreeSet<&str> = exposures
        .iter()
        .map(|exposure| exposure.object_id.as_str())
        .collect();

    let mut shocks: Vec<FactorShock> = Vec::new();
    let mut reached: BTreeSet<String> = BTreeSet::new();
    let mut deepest = 0usize;
    if held.contains(origin) {
        shocks.push(FactorShock::new(origin, initial_shock, 0.0));
        reached.insert(origin.to_string());
    }
    for effect in propagated {
        if !effect.magnitude.is_finite() {
            return Err(Error::numeric(format!(
                "the propagation from {origin} reached {} with a magnitude of {}, which is not a \
                 move; repair the edge's strength or confidence at its source rather than \
                 stressing on it",
                effect.target, effect.magnitude
            )));
        }
        if !held.contains(effect.target.as_str()) {
            continue;
        }
        if YIELD_QUOTED_FACTORS.contains(&effect.target.as_str()) {
            return Err(Error::invalid(format!(
                "a held position is named {:?}, which the stress tester reads as a yield move \
                 and signs the other way; a propagated price move cannot be filed under it — \
                 rename the object",
                effect.target
            )));
        }
        // The walk keeps the strongest path per target, so a target appears
        // once; refusing a second appearance rather than summing keeps this
        // from double-charging a position if that ever changes upstream.
        if !reached.insert(effect.target.clone()) {
            return Err(Error::invalid(format!(
                "the propagation from {origin} reached {} twice; a target must carry one \
                 strongest path, or the position is charged for two moves it can only make one of",
                effect.target
            )));
        }
        deepest = deepest.max(effect.order);
        shocks.push(FactorShock::new(
            effect.target.clone(),
            effect.magnitude,
            effect.over_days,
        ));
    }
    if shocks.is_empty() {
        return Ok(None);
    }
    let description = format!(
        "Causal propagation: a {:+.2}% move at driver {origin}, sized by {}, walked through the \
         causal graph's mechanisms to {} held position(s) up to {deepest} hop(s) away. Reaches \
         exposures no beta connects to the driver; says nothing about positions no path reaches.",
        initial_shock * 100.0,
        sizing.as_str(),
        shocks.len(),
    );
    Ok(Some(Scenario {
        name: format!("{CAUSAL_SCENARIO_PREFIX}{origin}"),
        description,
        shocks,
        // A propagated move is one path, not a joint distribution; there is
        // no correlation to assume and none is asserted.
        stressed_correlation: None,
        // Calm-market exit costs. A causal shock is a move at one driver, and
        // the section's row makes no claim about liquidity; inflating the
        // exit here would be a second, unstated scenario inside this one.
        liquidity_multiplier: 1.0,
        historical: false,
    }))
}

/// The book loaded on its own nodes, for applying a [`causal_scenario`].
///
/// Each position carries exactly one sensitivity — one, to its own object id
/// — because the propagation already carried every transmission along the
/// path, and a beta on top of it would count the mechanism twice. The
/// factor-loaded exposures the library is applied to are left untouched.
pub fn causal_exposures(exposures: &[FactorExposure]) -> Vec<FactorExposure> {
    exposures
        .iter()
        .map(|exposure| FactorExposure {
            object_id: exposure.object_id.clone(),
            notional: exposure.notional,
            betas: BTreeMap::from([(exposure.object_id.clone(), 1.0)]),
        })
        .collect()
}

/// One move in the adversarial sequence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdversarialStep {
    pub factor: String,
    /// The move, signed against the book.
    pub magnitude: f64,
    pub over_days: f64,
    /// The book's signed currency sensitivity to a unit move in the factor,
    /// after the yield-quoted sign convention: the sum of notional times beta
    /// over every position carrying the factor. Its sign is what chose the
    /// move's sign.
    pub book_sensitivity: f64,
    /// This step's own loss, in currency units. Never negative.
    pub loss: f64,
    /// Loss as a fraction of equity once this step and every step before it
    /// have landed, with the exit cost charged from the first step.
    pub cumulative_loss_fraction: f64,
}

/// The section's fourth method: the worst plausible sequence of factor
/// moves given the positions actually held.
///
/// *Plausible* is bounded by the library: no factor moves further than the
/// largest magnitude any scenario in the library states for it, and no
/// faster than the shortest window any states. *Worst* is decided by the
/// book: each factor moves in the direction that loses money on the net
/// sensitivity the held positions carry to it. *Sequence* is the order in
/// which the moves land, largest loss first — under a linear model every
/// order sums to the same total, so the ordering that matters is the one
/// that breaches soonest, and `first_breach` names the step at which it
/// does. A desk reading it learns how many adverse moves the book absorbs
/// before it is outside tolerance, which no single-point scenario says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdversarialSequence {
    pub scenario: Scenario,
    pub steps: Vec<AdversarialStep>,
    pub result: ScenarioResult,
    /// Zero-based index of the first step whose cumulative loss exceeds the
    /// tolerance it was constructed against, if any does.
    pub first_breach: Option<usize>,
    /// The tolerance `first_breach` was judged against.
    pub tolerance: f64,
    /// Factors the library shocks that no held position carries a beta for.
    /// The sequence could not include them, and a reader must not take their
    /// absence for a book that is immune to them.
    pub unsized_factors: Vec<String>,
}

impl AdversarialSequence {
    /// The stage's one-line detail.
    pub fn summarise(&self) -> String {
        let path: Vec<String> = self
            .steps
            .iter()
            .map(|step| format!("{} {:+.2}%", step.factor, step.magnitude * 100.0))
            .collect();
        let breach = match self.first_breach {
            Some(index) => format!(
                ", breaches {:.0}% tolerance at step {} of {}",
                self.tolerance * 100.0,
                index + 1,
                self.steps.len()
            ),
            None => format!(
                ", inside {:.0}% tolerance after every step",
                self.tolerance * 100.0
            ),
        };
        let unsized_note = if self.unsized_factors.is_empty() {
            String::new()
        } else {
            format!(
                ", {} library factor(s) with no exposure to size: {}",
                self.unsized_factors.len(),
                self.unsized_factors.join(", ")
            )
        };
        format!(
            "{}: {:.2}% loss over [{}]{breach}{unsized_note}",
            self.scenario.name,
            self.result.loss_fraction * 100.0,
            path.join(", ")
        )
    }
}

impl StressTester {
    /// Construct and apply the adversarial sequence for a book, or `None`
    /// when the book carries no sensitivity to any factor the library
    /// shocks — there is then nothing to sign against, and a sequence of
    /// zero steps would be a report of safety nobody measured.
    ///
    /// `library` is the plausibility envelope; `tolerance` is the loss
    /// fraction `first_breach` is judged against and is the caller's, not
    /// this module's.
    pub fn adversarial_sequence(
        &self,
        library: &[Scenario],
        exposures: &[FactorExposure],
        equity: f64,
        tolerance: f64,
        at: Timestamp,
    ) -> Result<Option<AdversarialSequence>> {
        if library.is_empty() {
            return Err(Error::invalid(
                "an adversarial sequence needs a library to bound plausibility; with no scenario \
                 stated there is no largest move to stay inside",
            ));
        }
        if !(0.0..1.0).contains(&tolerance) {
            return Err(Error::invalid(format!(
                "a stress tolerance of {tolerance} is not a fraction of equity below one"
            )));
        }
        for scenario in library {
            scenario.validate()?;
        }
        // The envelope: per factor, the largest magnitude and the shortest
        // window any library scenario states.
        let mut envelope: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
        let mut liquidity_multiplier = 1.0_f64;
        for scenario in library {
            liquidity_multiplier = liquidity_multiplier.max(scenario.liquidity_multiplier);
            for shock in &scenario.shocks {
                let entry = envelope
                    .entry(shock.factor.as_str())
                    .or_insert((0.0, f64::INFINITY));
                entry.0 = entry.0.max(shock.magnitude.abs());
                entry.1 = entry.1.min(shock.over_days);
            }
        }

        let mut steps: Vec<AdversarialStep> = Vec::new();
        let mut unsized_factors: Vec<String> = Vec::new();
        for (factor, (bound, over_days)) in &envelope {
            // The same convention `apply` uses, mirrored here so the sign
            // this chooses is the sign that loses money there.
            let convention = if YIELD_QUOTED_FACTORS.contains(factor) {
                -1.0
            } else {
                1.0
            };
            let mut carried = false;
            let mut sensitivity = 0.0_f64;
            for exposure in exposures {
                if let Some(beta) = exposure.betas.get(*factor) {
                    carried = true;
                    sensitivity += convention * exposure.notional.to_f64() * beta;
                }
            }
            if !carried {
                unsized_factors.push((*factor).to_string());
                continue;
            }
            if !sensitivity.is_finite() {
                return Err(Error::numeric(format!(
                    "the book's sensitivity to {factor} is {sensitivity}, which cannot be signed \
                     against; a beta or a notional that is not a number is a source to repair"
                )));
            }
            if sensitivity == 0.0 {
                // Carried and exactly flat: a long and a short that cancel.
                // No direction hurts, so no step — and it is not unsized,
                // because it was measured.
                continue;
            }
            let magnitude = -bound * sensitivity.signum();
            steps.push(AdversarialStep {
                factor: (*factor).to_string(),
                magnitude,
                over_days: *over_days,
                book_sensitivity: sensitivity,
                loss: bound * sensitivity.abs(),
                cumulative_loss_fraction: 0.0,
            });
        }
        if steps.is_empty() {
            return Ok(None);
        }
        // Largest loss first, ties by name so the sequence is reproducible.
        steps.sort_by(|a, b| {
            b.loss
                .partial_cmp(&a.loss)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.factor.cmp(&b.factor))
        });

        let scenario = Scenario {
            name: ADVERSARIAL_SCENARIO_NAME.to_string(),
            description: format!(
                "Adversarial: every factor the held book is sensitive to, moved to the largest \
                 magnitude any of the {} library scenarios states for it, in the direction that \
                 loses on the book's net sensitivity, landing largest loss first, at the \
                 library's widest exit cost. Constructed from the positions, not from history.",
                library.len()
            ),
            shocks: steps
                .iter()
                .map(|step| FactorShock::new(step.factor.clone(), step.magnitude, step.over_days))
                .collect(),
            // Every move lands against the book at once: the worst case is
            // the one in which nothing diversifies.
            stressed_correlation: Some(1.0),
            liquidity_multiplier,
            historical: false,
        };
        let result = self.apply(&scenario, exposures, equity, at)?;

        // The exit cost is charged at the first step: a book that must be
        // unwound pays to unwind whichever move comes first.
        let mut cumulative = result.liquidation_cost;
        let mut first_breach = None;
        for (index, step) in steps.iter_mut().enumerate() {
            cumulative += step.loss;
            step.cumulative_loss_fraction = cumulative / equity;
            if first_breach.is_none() && step.cumulative_loss_fraction > tolerance {
                first_breach = Some(index);
            }
        }
        Ok(Some(AdversarialSequence {
            scenario,
            steps,
            result,
            first_breach,
            tolerance,
            unsized_factors,
        }))
    }
}
