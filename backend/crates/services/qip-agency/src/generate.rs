//! R12-AGENCY-6087 / 6088: the planner generates its own candidates over the
//! affordance graph, and every candidate — generated or proposed — is
//! simulated across every competing model and scenario branch before it is
//! compared.
//!
//! The failure prevented is a planner that only evaluates what an operator
//! thought of, judged against a single assumed world. Before this module the
//! engine's candidates were the request's `proposals`, each carrying a
//! comparison whose expected effect was whatever the proposer wrote.
//!
//! [`Structural`] is what a request declares to make simulation possible:
//! competing [`StructuralModel`]s (declared, or fitted from data —
//! [`StructuralModel::fit`]), scenario branches, the objective and which way
//! is better, and per tool how far one step moves its variable. With it:
//!
//! * [`Structural::generate`] proposes a single-step plan for every
//!   non-deceptive tool edge on every lever, and — when asked for two steps —
//!   every pair of such steps on distinct levers. Each candidate's nine-axis
//!   comparison is *computed*: the effect is the mean simulated effect, the
//!   downside the worst branch's loss, the confidence the weakest lever's
//!   identifiability times the share of branches in which it helps, cost and
//!   time from the graph, reversibility the share of reversible steps. A
//!   deceptive edge is never generated; anything else that is inadmissible is
//!   generated and then refused by [`crate::plan::select`] by name, so the
//!   report says why the obvious lever was not pulled.
//! * [`Structural::check_claim`] holds an operator's proposal to the same
//!   simulation: a proposal claiming more effect than its best branch shows
//!   is refused. A claim no model supports is a guess wearing a number.

use crate::affordance::AffordanceGraph;
use crate::causal::{
    Objective, Scenario, SimulationResult, StructuralModel, StructuralModelDecl, simulate,
};
use crate::comparison::Comparison;
use crate::engine::LeverEvidence;
use crate::plan::{PlanNode, Proposal, Step};
use qip_core::{Decimal, Error};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// A model learned from observations over a declared structure.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FittedDecl {
    pub name: String,
    pub structure: Vec<(String, String)>,
    pub observations: Vec<BTreeMap<String, f64>>,
}

/// What the planner generates, when it is asked to.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateDecl {
    /// The exposure one step of each tool adds, in the goal's envelope unit.
    pub exposure: BTreeMap<String, Decimal>,
    /// One or two: single steps, or also every pair on distinct levers.
    pub max_steps: usize,
}

/// The simulation half of a request, as declared.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuralDecl {
    #[serde(default)]
    pub models: Vec<StructuralModelDecl>,
    #[serde(default)]
    pub fitted: Vec<FittedDecl>,
    pub scenarios: Vec<Scenario>,
    pub objective: Objective,
    /// Per tool, how far one step moves the variable it acts on.
    pub actuation: BTreeMap<String, f64>,
    #[serde(default)]
    pub generate: Option<GenerateDecl>,
}

/// The built simulation context.
#[derive(Debug, Clone)]
pub struct Structural {
    models: Vec<StructuralModel>,
    scenarios: Vec<Scenario>,
    objective: Objective,
    actuation: BTreeMap<String, f64>,
    generate: Option<GenerateDecl>,
}

/// A candidate's label and what simulating it produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Simulated {
    pub label: String,
    pub results: Vec<SimulationResult>,
}

impl Structural {
    pub fn build(decl: StructuralDecl) -> Result<Self, Error> {
        let mut models = Vec::new();
        for model in decl.models {
            models.push(StructuralModel::new(model)?);
        }
        for fitted in decl.fitted {
            models.push(StructuralModel::fit(
                &fitted.name,
                &fitted.structure,
                &fitted.observations,
            )?);
        }
        if let Some(generate) = &decl.generate
            && !(1..=2).contains(&generate.max_steps)
        {
            return Err(Error::invalid(
                "`generate.max_steps` is 1 or 2; longer generated sequences are not supported",
            ));
        }
        let built = Self {
            models,
            scenarios: decl.scenarios,
            objective: decl.objective,
            actuation: decl.actuation,
            generate: decl.generate,
        };
        // Proven simulable once, with no intervention, so a malformed model
        // set is refused as a request rather than discovered per candidate.
        simulate(
            &built.models,
            &built.scenarios,
            &built.objective,
            &BTreeMap::new(),
        )?;
        Ok(built)
    }

    pub fn generates(&self) -> bool {
        self.generate.is_some()
    }

    /// Simulate the steps of `root` that fire under `facts`.
    pub fn simulate(
        &self,
        root: &PlanNode,
        facts: &BTreeSet<String>,
    ) -> Result<Vec<SimulationResult>, Error> {
        let mut shifts: BTreeMap<String, f64> = BTreeMap::new();
        for step in root.simulate(facts) {
            let shift = self.actuation.get(&step.tool).ok_or_else(|| {
                Error::invalid(format!(
                    "tool `{}` has no declared actuation; how far it moves `{}` is needed to \
                     simulate it",
                    step.tool, step.variable
                ))
            })?;
            *shifts.entry(step.variable.clone()).or_insert(0.0) += shift;
        }
        simulate(&self.models, &self.scenarios, &self.objective, &shifts)
    }

    /// Refuse an operator's proposal that claims more than its best branch.
    pub fn check_claim(
        &self,
        proposal: &Proposal,
        results: &[SimulationResult],
    ) -> Result<(), Error> {
        let best = results
            .iter()
            .map(|r| r.effect)
            .fold(f64::NEG_INFINITY, f64::max);
        // The statistics/money crossing: a simulated effect is an `f64`, the
        // comparison axis a `Decimal`; read as a Decimal so the comparison is
        // exact on the claim's side.
        let best = Decimal::from_f64(best)
            .ok_or_else(|| Error::numeric("the best simulated effect is not a finite number"))?;
        if proposal.comparison.expected_causal_effect > best {
            return Err(Error::denied(format!(
                "the proposal claims an expected effect of {}, more than any model and scenario \
                 simulates (best {best}); a claim no model supports is not ranked",
                proposal.comparison.expected_causal_effect
            )));
        }
        Ok(())
    }

    /// Generate candidates over `levers`, each with its computed comparison
    /// and its simulations. `legally_eligible` is the engine's eligibility
    /// verdict for the goal, applied to every generated candidate.
    pub fn generate(
        &self,
        graph: &AffordanceGraph,
        levers: &[String],
        evidence: &BTreeMap<String, LeverEvidence>,
        facts: &BTreeSet<String>,
        legally_eligible: bool,
    ) -> Result<Vec<(Proposal, Simulated)>, Error> {
        let Some(decl) = &self.generate else {
            return Ok(Vec::new());
        };
        let mut singles: Vec<(Step, bool, u64, Decimal)> = Vec::new();
        for lever in levers {
            for edge in graph.edges(lever) {
                if edge.method.is_deceptive() {
                    continue;
                }
                let Some(exposure) = decl.exposure.get(&edge.tool) else {
                    continue;
                };
                singles.push((
                    Step {
                        tool: edge.tool.clone(),
                        variable: lever.clone(),
                        exposure: *exposure,
                    },
                    edge.reversible,
                    edge.latency_ms,
                    edge.cost,
                ));
            }
        }
        let mut shapes: Vec<Vec<usize>> = (0..singles.len()).map(|i| vec![i]).collect();
        if decl.max_steps >= 2 {
            for i in 0..singles.len() {
                for j in i + 1..singles.len() {
                    if singles[i].0.variable != singles[j].0.variable {
                        shapes.push(vec![i, j]);
                    }
                }
            }
        }
        let mut out = Vec::with_capacity(shapes.len());
        for shape in shapes {
            let steps: Vec<&(Step, bool, u64, Decimal)> =
                shape.iter().map(|i| &singles[*i]).collect();
            let root = if steps.len() == 1 {
                PlanNode::Step(steps[0].0.clone())
            } else {
                PlanNode::Sequence(steps.iter().map(|s| PlanNode::Step(s.0.clone())).collect())
            };
            let label = steps
                .iter()
                .map(|s| format!("{}@{}", s.0.tool, s.0.variable))
                .collect::<Vec<_>>()
                .join("+");
            let results = self.simulate(&root, facts)?;
            let comparison = self.compare(&steps, &results, evidence, legally_eligible)?;
            out.push((Proposal { root, comparison }, Simulated { label, results }));
        }
        Ok(out)
    }

    fn compare(
        &self,
        steps: &[&(Step, bool, u64, Decimal)],
        results: &[SimulationResult],
        evidence: &BTreeMap<String, LeverEvidence>,
        legally_eligible: bool,
    ) -> Result<Comparison, Error> {
        let n = results.len() as f64;
        let mean = results.iter().map(|r| r.effect).sum::<f64>() / n;
        let worst = results
            .iter()
            .map(|r| r.effect)
            .fold(f64::INFINITY, f64::min);
        let helps = results.iter().filter(|r| r.effect > 0.0).count() as f64 / n;
        // The weakest lever bounds what is known; an unmeasured lever is
        // known at zero, so its candidate is generated with no confidence and
        // scores nothing rather than being dropped unexplained.
        let mut identified = Decimal::ONE;
        for (step, ..) in steps {
            let known = evidence
                .get(&step.variable)
                .map_or(Decimal::ZERO, |e| e.identifiability);
            identified = identified.min(known);
        }
        // The statistics/money crossing, stated once: the simulated mean,
        // worst branch and share are `f64` statistics, and every comparison
        // axis is a `Decimal`. A non-finite figure is refused, not zeroed.
        let to_decimal = |what: &str, v: f64| {
            Decimal::from_f64(v).ok_or_else(|| {
                Error::numeric(format!(
                    "the simulated {what} ({v}) is not a finite decimal"
                ))
            })
        };
        let confidence = identified
            .checked_mul(to_decimal("share of helping branches", helps)?)
            .ok_or_else(|| Error::numeric("confidence overflowed"))?;
        let mut cost = Decimal::ZERO;
        let mut exposure = Decimal::ZERO;
        let mut latency = 0u64;
        let mut reversible = 0i64;
        for (step, is_reversible, latency_ms, edge_cost) in steps {
            cost = cost
                .checked_add(*edge_cost)
                .ok_or_else(|| Error::numeric("candidate cost overflowed"))?;
            exposure = exposure
                .checked_add(step.exposure)
                .ok_or_else(|| Error::numeric("candidate exposure overflowed"))?;
            latency = latency.max(*latency_ms);
            reversible += i64::from(*is_reversible);
        }
        let count = i64::try_from(steps.len())
            .map_err(|_| Error::numeric("candidate length does not fit"))?;
        let reversibility = Decimal::from_int(reversible)
            .checked_div(Decimal::from_int(count))
            .ok_or_else(|| Error::numeric("reversibility could not be computed"))?;
        Comparison::computed(
            to_decimal("mean effect", mean)?,
            confidence,
            cost,
            exposure,
            latency.div_ceil(1000),
            reversibility,
            legally_eligible,
            // Generated candidates use lawful methods only (deceptive edges
            // are skipped above); the conduct gate in front of the adapter is
            // where a step's public face is judged, and that is not a figure
            // the planner can compute before a draft exists.
            Decimal::ZERO,
            to_decimal("worst-branch loss", (-worst).max(0.0))?,
        )
    }
}
