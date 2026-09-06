//! The causal analyst.
//!
//! Traces how a shock reaches an instrument through the world model's recorded
//! causal edges, and reports the mechanism rather than the correlation.
//!
//! The distinction this agent exists to enforce: that two things move together
//! is a statistic, and the world model's relationship graph holds plenty of
//! those. That one moves *because of* the other is a claim with a mechanism, a
//! lag and evidence behind it, and only the causal graph holds those. An
//! unevidenced edge is reported as unevidenced, never used as though it were a
//! fact.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, conviction_from_z, no_data, out_of_scope};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::sync::Arc;

/// Hops the analyst will trace. Beyond four the chain is longer than anyone
/// can check, and the red team rejects it anyway.
const MAX_ORDER: usize = 4;

/// Effects below this fraction of the original shock are dropped.
const MAGNITUDE_FLOOR: f64 = 0.02;

/// The size of the hypothetical shock traced, as a fraction.
///
/// A unit shock would make magnitudes read as percentages of something
/// undefined; one percent keeps the reported numbers interpretable.
const REFERENCE_SHOCK: f64 = 0.01;

/// Traces transmission through the causal graph.
#[derive(Debug)]
pub struct CausalAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl CausalAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for CausalAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn accepts(&self, brief: &AgentBrief) -> bool {
        // Needs both ends: something that moved, and something to trace it to.
        !brief.entities.is_empty() && !brief.objects.is_empty()
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let (Some(origin), Some(target)) = (brief.entities.first(), brief.objects.first()) else {
            return Ok(out_of_scope(
                ctx,
                brief.as_of,
                "a causal trace needs an origin entity and a target instrument",
            ));
        };

        let world = self.desk.world.get(ctx)?;
        let causal = world.causal();

        // No absorb arm records a causal edge, so on any deployed platform
        // this is the arm taken for every origin; the finding names the
        // record kind that would change that rather than only that the graph
        // is empty here.
        if causal.outgoing(origin, brief.as_of).is_empty() {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "the causal graph records no mechanism out of {origin} as of {}; needs {}",
                    brief.as_of,
                    qip_world_model::vocabulary::CAUSAL_CLAIM_NEEDED
                ),
            ));
        }

        let propagation = causal.propagate(
            origin,
            REFERENCE_SHOCK,
            MAX_ORDER,
            MAGNITUDE_FLOOR,
            brief.as_of,
            brief.as_of,
        );

        let Some(effect) = propagation
            .effects
            .iter()
            .find(|e| e.target == target.as_str())
        else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "no evidenced path from {origin} to {} within {MAX_ORDER} hops ({} effects reached, {} truncated)",
                    target.as_str(),
                    propagation.effects.len(),
                    propagation.truncated
                ),
            ));
        };

        // Transmission as a multiple of the shock: what a one percent move at
        // the origin becomes at the target.
        let transmission = effect.magnitude / REFERENCE_SHOCK;

        // Every edge on the path is checked for evidence. An unevidenced link
        // does not invalidate the chain, but it does have to be said out loud.
        let mut unevidenced = Vec::new();
        for pair in effect.path.windows(2) {
            for edge in causal.outgoing(&pair[0], brief.as_of) {
                if edge.effect == pair[1] && !edge.is_evidenced() {
                    unevidenced.push(format!("{} -> {}", pair[0], pair[1]));
                }
            }
        }

        let mut caveats = vec![format!(
            "an order-{} effect compounds {} mechanisms, each of which can fail independently",
            effect.order,
            effect.chain.len()
        )];
        if !unevidenced.is_empty() {
            caveats.push(format!(
                "unevidenced causal claims on the path: {}",
                unevidenced.join(", ")
            ));
        }

        // Confidence along the chain is already the product of the per-edge
        // confidences, so it falls away quickly with length — which is the
        // intended behaviour, not a defect to be corrected.
        let conviction = conviction_from_z(transmission * 3.0) * effect.confidence;

        let direction = if transmission > 0.0 {
            Direction::Positive
        } else if transmission < 0.0 {
            Direction::Negative
        } else {
            Direction::Neutral
        };

        let mut builder = FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "a shock at {origin} reaches {} at order {}: {}",
                target.as_str(),
                effect.order,
                effect.explain()
            ),
        )
        .direction(direction, conviction)
        .fact(computed(
            ctx,
            "transmission_multiple",
            transmission,
            "ratio",
            &["causal_graph"],
        ))
        .fact(computed(
            ctx,
            "path_confidence",
            effect.confidence,
            "probability",
            &["causal_graph"],
        ))
        .fact(computed(
            ctx,
            "path_length",
            effect.order as f64,
            "hops",
            &["causal_graph"],
        ))
        .evidence(
            effect
                .path
                .windows(2)
                .map(|pair| format!("causal:{}->{}", pair[0], pair[1]))
                .collect(),
        )
        .falsifiers(vec![
            format!(
                "the observable named on the weakest link fails to appear within {:?}",
                effect.expected_at.since(brief.as_of)
            ),
            "the target moves without the origin having moved, indicating a common cause"
                .to_string(),
        ])
        .caveats(caveats)
        .follow_ups(
            unevidenced
                .iter()
                .map(|edge| format!("find evidence for the causal claim {edge}"))
                .collect(),
        );

        if !unevidenced.is_empty() {
            builder = builder.missing(
                unevidenced
                    .iter()
                    .map(|edge| format!("evidence for causal edge {edge}"))
                    .collect(),
            );
        }
        builder.build()
    }
}
