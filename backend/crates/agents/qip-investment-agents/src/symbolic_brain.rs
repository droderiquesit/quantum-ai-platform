//! The Symbolic Reasoning Brain (R12-REASON-3026): the specialist that
//! reports what the six symbolic reasoners — ontology, temporal, causal,
//! rule, probabilistic and constraint — concluded about the hypothesis the
//! kernel last derived.
//!
//! The reasoners live in `qip-reasoning-engine` and the kernel runs them in
//! REASON (`reason_symbolically`), journaling a derivation per hypothesis.
//! Until this agent existed no specialist in the organisation read that
//! record, so the panel could not say whether the claim it was weighing had
//! been checked against rules, a timeline, a causal graph, a posterior or a
//! constraint problem, and removing "the symbolic brain" removed nothing.
//!
//! This crate cannot depend on the reasoning engine (an architecture anchor
//! keeps that edge dev-only), so the agent reads [`crate::desk::SymbolicView`]:
//! plain data the kernel copies out of the derivation it is about to journal.
//! It restates conclusions and re-derives none; a reasoner that refused is
//! reported as refused, never as a pass.
//!
//! **No derivation is no data.** Before the kernel has derived anything the
//! finding is a no-view, so the scope is not covered by an empty record.

use crate::desk::{Desk, SymbolicView};
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::sync::Arc;

/// The input every fact this agent states is computed from.
const SOURCE: &[&str] = &["symbolic-derivation"];

/// How many reasoners produced a conclusion rather than a refusal.
fn reasoners_derived(view: &SymbolicView) -> usize {
    [
        view.rules_eligible.is_some(),
        // The timeline derived only if it held at least one link: no link
        // asked is nothing derived, and every link refused is the temporal
        // reasoner refusing, not ordering.
        view.timeline_links > view.timeline_links_refused,
        view.claim_probability.is_some(),
        view.blocking_constraints.is_some(),
        view.causal_explanations.is_some() || view.causal_reach.is_some(),
        view.subject_type.is_some() || view.facts_inferred.is_some(),
    ]
    .into_iter()
    .filter(|derived| *derived)
    .count()
}

/// Reads the kernel's latest symbolic derivation.
#[derive(Debug)]
pub struct SymbolicReasoningAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl SymbolicReasoningAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for SymbolicReasoningAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let view = self.desk.symbolic.get(ctx)?;
        if !view.observed {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "no symbolic derivation has been written: the kernel has not derived a \
                 hypothesis, so there is no rule, timeline, causal, ontology, probabilistic \
                 or constraint conclusion to read",
            ));
        }

        let derived = reasoners_derived(&view);
        let rules = match view.rules_eligible {
            Some(true) => "eligible",
            Some(false) => "not eligible",
            None => "refused",
        };
        let acting = match &view.blocking_constraints {
            Some(conflict) if conflict.is_empty() => "satisfiable".to_string(),
            Some(conflict) => format!("blocked by {}", conflict.join("+")),
            None => "refused".to_string(),
        };
        let claim = format!(
            "symbolic derivation of {} for {} (cycle {}): {derived} of 6 reasoners derived; \
             rules {rules}, {} of {} timeline link(s) refused, acting {acting}, {} \
             assumption(s) violated{}",
            view.hypothesis_id,
            view.subject,
            view.cycle,
            view.timeline_links_refused,
            view.timeline_links,
            view.violated_assumptions.len(),
            if view.infeasible {
                ", the parts contradict each other"
            } else {
                ""
            },
        );

        // What the derivation found does not hold is a finding about the
        // claim, never a missing input: filed as missing it would read as a
        // gap in the panel's evidence, and the verifier society's
        // missing-variable critic would refute the thesis for the very
        // reason the reasoners were run.
        let mut caveats = vec![
            "restates the kernel's journaled derivation; it runs no reasoner and a \
             reasoner that refused is reported as refused, not as a pass"
                .to_string(),
            "advisory: nothing here sizes or sends".to_string(),
        ];
        if !view.agrees_with_review {
            caveats.push("the rule pack's eligibility disagrees with the review's approval".into());
        }
        caveats.extend(
            view.violated_assumptions
                .iter()
                .map(|assumption| format!("violated: {assumption}")),
        );

        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(Direction::Neutral, 0.0)
            .fact(computed(
                ctx,
                "reasoners_derived",
                derived as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "timeline_links_refused",
                view.timeline_links_refused as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "assumptions_violated",
                view.violated_assumptions.len() as f64,
                "count",
                SOURCE,
            ))
            .fact(computed(
                ctx,
                "derivation_infeasible",
                f64::from(u8::from(view.infeasible)),
                "flag",
                SOURCE,
            ))
            .evidence(
                view.blocking_constraints
                    .iter()
                    .flatten()
                    .map(|id| format!("blocking_constraint:{id}"))
                    .collect(),
            )
            .caveats(caveats);
        // Each figure below is stated only where its reasoner produced it: a
        // probability nobody computed would read as a coin flip.
        if let Some(eligible) = view.rules_eligible {
            builder = builder.fact(computed(
                ctx,
                "rules_eligible",
                f64::from(u8::from(eligible)),
                "flag",
                SOURCE,
            ));
        }
        if let Some(p) = view.claim_probability {
            builder = builder.fact(computed(ctx, "claim_probability", p, "probability", SOURCE));
        }
        if let Some(u) = view.claim_uncertainty {
            builder = builder.fact(computed(ctx, "claim_uncertainty", u, "ratio", SOURCE));
        }
        if let Some(conflict) = &view.blocking_constraints {
            builder = builder.fact(computed(
                ctx,
                "constraints_blocking",
                conflict.len() as f64,
                "count",
                SOURCE,
            ));
        }
        if let Some(n) = view.causal_explanations {
            builder = builder.fact(computed(
                ctx,
                "causal_explanations",
                n as f64,
                "count",
                SOURCE,
            ));
        }
        if let Some(n) = view.facts_inferred {
            builder = builder.fact(computed(ctx, "ontology_facts", n as f64, "count", SOURCE));
        }
        builder.build()
    }
}
