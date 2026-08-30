//! Portfolio construction and the quantum optimisation gate.
//!
//! [`PortfolioConstruction`] proposes; it cannot approve, veto or submit. The
//! separation is enforced by the manifest, not by this file's good intentions.
//!
//! [`QuantumOptimization`] does not run a quantum optimiser. It decides whether
//! a problem is one where a quantum method could plausibly help, and it always
//! requires a classical baseline first. A platform that reaches for a quantum
//! solver because it has one is a platform that will report an advantage it
//! cannot substantiate.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data, out_of_scope};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use qip_quant::strategy::TargetWeights;
use std::sync::Arc;

/// Largest fraction of equity any single name may reach.
///
/// A concentration cap is not negotiable against an exposure target: when the
/// cap makes the target unreachable the book is deliberately under-invested and
/// the proposal says so.
const POSITION_CAP: f64 = 0.08;

/// Gross exposure the construction targets, as a fraction of equity.
const TARGET_GROSS: f64 = 0.90;

/// Weights below this are dropped: a 0.1% position costs the same in
/// operational overhead as a 5% one and contributes nothing.
const MINIMUM_WEIGHT: f64 = 0.005;

/// Turns conviction into target weights.
#[derive(Debug)]
pub struct PortfolioConstruction {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl PortfolioConstruction {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for PortfolioConstruction {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn accepts(&self, brief: &AgentBrief) -> bool {
        !brief.objects.is_empty()
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        if brief.objects.is_empty() {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "construction needs instruments to size",
            ));
        }

        // Conviction per name comes from the world model's feature store,
        // where the reasoning stage records approved theses. Construction does
        // not form views; it expresses ones already approved.
        let mut convictions = Vec::new();
        let mut missing = Vec::new();
        {
            let world = self.desk.world.get(ctx)?;
            for object in &brief.objects {
                match world.features().value_as_of(
                    "approved_conviction",
                    object.as_str(),
                    brief.as_of,
                    brief.as_of,
                ) {
                    Some(value) if value.value.is_finite() => {
                        convictions.push((object.clone(), value.value));
                    }
                    _ => missing.push(format!("approved_conviction for {}", object.as_str())),
                }
            }
        }

        if convictions.is_empty() {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "no approved conviction is recorded for any name in scope; construction expresses approved theses, it does not create them",
            ));
        }

        let mut raw = TargetWeights::new();
        for (object, conviction) in &convictions {
            raw.set(object, *conviction);
        }
        let targets = raw
            .capped_to_gross(TARGET_GROSS, POSITION_CAP)
            .pruned(MINIMUM_WEIGHT);

        let achievable_gross = TARGET_GROSS.min(POSITION_CAP * convictions.len() as f64);
        let cap_binds = achievable_gross < TARGET_GROSS - 1e-9;

        // Turnover against the book as it stands, which is what the proposal
        // actually costs to implement.
        let turnover = if self.desk.book.is_available(ctx) {
            let book = self.desk.book.get(ctx)?;
            let valuation = book.portfolio.value(&book.marks, brief.as_of);
            let equity = valuation.equity.to_f64();
            if equity.abs() > 1e-9 {
                let current: std::collections::BTreeMap<String, f64> = book
                    .portfolio
                    .positions()
                    .filter_map(|position| {
                        let mark = book.marks.get(position.object_id.as_str())?;
                        let notional = position.quantity().to_f64()
                            * mark.to_f64()
                            * position.contract_multiplier.to_f64();
                        Some((position.object_id.as_str().to_string(), notional / equity))
                    })
                    .collect();
                Some(targets.turnover_from(&current))
            } else {
                None
            }
        } else {
            None
        };

        let mut caveats = vec![format!(
            "positions are capped at {:.0}% of equity each",
            POSITION_CAP * 100.0
        )];
        if cap_binds {
            caveats.push(format!(
                "{} names against a {:.0}% cap cannot reach the {:.0}% gross target; the book is deliberately under-invested at {:.0}%",
                convictions.len(),
                POSITION_CAP * 100.0,
                TARGET_GROSS * 100.0,
                achievable_gross * 100.0
            ));
        }

        let mut builder = FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "proposes {} position(s) at {:.1}% gross exposure",
                targets.len(),
                targets.gross() * 100.0
            ),
        )
        // A construction proposal is not a directional view; the views it
        // expresses were formed and approved elsewhere.
        .direction(Direction::Neutral, 0.0)
        .fact(computed(
            ctx,
            "proposed_gross_exposure",
            targets.gross(),
            "ratio",
            &["approved_conviction"],
        ))
        .fact(computed(
            ctx,
            "proposed_net_exposure",
            targets.net(),
            "ratio",
            &["approved_conviction"],
        ))
        .fact(computed(
            ctx,
            "position_count",
            targets.len() as f64,
            "count",
            &["approved_conviction"],
        ))
        .fact(computed(
            ctx,
            "largest_position_weight",
            targets
                .weights
                .values()
                .fold(0.0_f64, |a, b| a.max(b.abs())),
            "ratio",
            &["approved_conviction"],
        ))
        .evidence(
            convictions
                .iter()
                .map(|(object, _)| format!("feature:approved_conviction@{}", object.as_str()))
                .collect(),
        )
        .falsifiers(vec![
            "the risk function rejects the proposal against a limit".to_string(),
            "realised turnover cost exceeds the expected edge of the theses being expressed"
                .to_string(),
        ])
        .caveats(caveats);

        if let Some(turnover) = turnover {
            builder = builder.fact(computed(
                ctx,
                "turnover_fraction",
                turnover,
                "ratio",
                &["portfolio", "approved_conviction"],
            ));
        } else {
            missing.push("current book weights, so turnover could not be computed".to_string());
        }
        if !missing.is_empty() {
            builder = builder.missing(missing);
        }
        builder.build()
    }
}

// --- quantum ----------------------------------------------------------------

/// Problem size below which a classical solver is simply better.
///
/// Under fifty assets an exact quadratic solve takes microseconds, and any
/// quantum formulation would spend longer on encoding than the classical
/// solver spends solving.
const QUANTUM_MINIMUM_ASSETS: usize = 50;

/// Decides whether a problem is one a quantum method could help with.
#[derive(Debug)]
pub struct QuantumOptimization {
    manifest: AgentManifest,
    desk: Arc<Desk>,
    /// Whether a quantum backend is reachable in this deployment. Supplied
    /// rather than probed, so a test and a production run differ only in
    /// configuration.
    backend_available: bool,
}

impl QuantumOptimization {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>, backend_available: bool) -> Self {
        Self {
            manifest,
            desk,
            backend_available,
        }
    }
}

impl Agent for QuantumOptimization {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let asset_count = brief.objects.len();

        // Discrete structure is what an annealing formulation is for:
        // cardinality constraints, lot sizes, and integer holdings. A
        // continuous mean-variance problem is a solved problem classically.
        let discrete_constraints = brief
            .questions
            .iter()
            .filter(|q| {
                let q = q.to_ascii_lowercase();
                q.contains("cardinality") || q.contains("lot") || q.contains("integer")
            })
            .count();

        let warranted = asset_count >= QUANTUM_MINIMUM_ASSETS && discrete_constraints > 0;

        let mut caveats = vec![
            "no quantum result is used without a classical baseline solved on the same problem"
                .to_string(),
        ];
        let mut missing = Vec::new();
        if !self.backend_available {
            missing.push(
                "a reachable quantum backend; this deployment has no credential or endpoint configured, so the classical solver is used"
                    .to_string(),
            );
            caveats.push(
                "the platform runs the classical solver whenever the quantum path is unavailable, which is the default"
                    .to_string(),
            );
        }

        let recommendation = match (warranted, self.backend_available) {
            (true, true) => "benchmark the annealing formulation against the classical baseline",
            (true, false) => {
                "the problem would be worth benchmarking, but no backend is reachable; use the classical solver"
            }
            (false, _) => "use the classical solver",
        };

        let mut builder = FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "{asset_count} assets with {discrete_constraints} discrete constraint(s): {recommendation}"
            ),
        )
        // Never a directional view: this agent says which solver to use.
        .direction(Direction::Neutral, 0.0)
        .fact(computed(
            ctx,
            "asset_count",
            asset_count as f64,
            "count",
            &["brief"],
        ))
        .fact(computed(
            ctx,
            "discrete_constraint_count",
            discrete_constraints as f64,
            "count",
            &["brief"],
        ))
        .fact(computed(
            ctx,
            "quantum_warranted",
            f64::from(u8::from(warranted)),
            "boolean",
            &["asset_count", "discrete_constraint_count"],
        ))
        .fact(computed(
            ctx,
            "backend_available",
            f64::from(u8::from(self.backend_available)),
            "boolean",
            &["configuration"],
        ))
        .evidence(vec!["configuration:quantum-backend".to_string()])
        .falsifiers(vec![
            "the classical baseline matches or beats the quantum result on the same problem, which would make the quantum path unwarranted"
                .to_string(),
        ])
        .caveats(caveats);

        // Reading the book is what makes the asset count meaningful when the
        // brief does not carry one.
        if asset_count == 0 && self.desk.book.is_available(ctx) {
            let book = self.desk.book.get(ctx)?;
            let held = book.portfolio.position_count();
            builder = builder.fact(computed(
                ctx,
                "held_position_count",
                held as f64,
                "count",
                &["portfolio"],
            ));
        }

        if !missing.is_empty() {
            builder = builder.missing(missing);
        }
        builder.build()
    }
}
