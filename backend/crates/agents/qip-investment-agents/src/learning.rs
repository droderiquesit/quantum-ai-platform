//! Learning and attribution.
//!
//! Scores what the organisation predicted against what happened, per agent and
//! per thesis class, and turns the difference into lessons.
//!
//! Two properties keep this honest. It cannot score a thesis whose horizon has
//! not passed — scoring early would let a thesis be graded on the part of the
//! path that suited it. And a lesson is recorded with its counter-examples,
//! never without them: a rule that holds four times in five is a useful rule,
//! and hiding the fifth case makes it look like a law.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::memory::PromotionPolicy;
use qip_agents::runtime::{Agent, AgentContext};
use qip_ai::evaluation::{Calibration, PredictionOutcome};
use qip_core::error::Result;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Resolved episodes needed before a hit rate means anything.
///
/// Ten is not statistical significance; it is the point below which reporting
/// a rate at all would be misleading.
const MINIMUM_RESOLVED: usize = 10;

/// Scores predictions against outcomes.
#[derive(Debug)]
pub struct LearningAttribution {
    manifest: AgentManifest,
    desk: Arc<Desk>,
    promotion: PromotionPolicy,
}

impl LearningAttribution {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self {
            manifest,
            desk,
            promotion: PromotionPolicy::default(),
        }
    }

    pub fn with_promotion_policy(mut self, promotion: PromotionPolicy) -> Self {
        self.promotion = promotion;
        self
    }

    pub fn promotion_policy(&self) -> PromotionPolicy {
        self.promotion
    }
}

impl Agent for LearningAttribution {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let memory = self.desk.memory.get(ctx)?;

        let agents: BTreeSet<String> = memory
            .episodes()
            .iter()
            .map(|episode| episode.agent_id.clone())
            .collect();
        if agents.is_empty() {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "no episodes have been recorded, so there is nothing to score",
            ));
        }

        // Only episodes whose outcome is known by the as-of time. An episode
        // resolved tomorrow cannot inform today's assessment.
        let mut outcomes = Vec::new();
        let mut scored = 0usize;
        let mut unresolved = 0usize;
        let mut rates: Vec<(String, f64, usize)> = Vec::new();

        for agent_id in &agents {
            let episodes = memory.episodes_as_of(agent_id, brief.as_of);
            let resolved: Vec<_> = episodes
                .iter()
                .filter(|episode| {
                    episode
                        .outcome
                        .as_ref()
                        .is_some_and(|outcome| outcome.resolved_at <= brief.as_of)
                })
                .collect();
            unresolved += episodes.len() - resolved.len();
            scored += resolved.len();

            for episode in &resolved {
                if let Some(outcome) = &episode.outcome {
                    outcomes.push(
                        PredictionOutcome::new(episode.conviction, outcome.correct)
                            .in_cohort(agent_id.clone()),
                    );
                }
            }
            if resolved.len() >= MINIMUM_RESOLVED {
                let correct = resolved
                    .iter()
                    .filter(|episode| {
                        episode
                            .outcome
                            .as_ref()
                            .is_some_and(|outcome| outcome.correct)
                    })
                    .count();
                rates.push((
                    agent_id.clone(),
                    correct as f64 / resolved.len() as f64,
                    resolved.len(),
                ));
            }
        }

        if outcomes.is_empty() {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!(
                    "no episode has resolved by {}; {unresolved} are still open",
                    brief.as_of
                ),
            ));
        }

        // Calibration over convictions: does an agent claiming 0.8 turn out to
        // be right about 80% of the time? An overconfident organisation is
        // worse than an uncertain one, because it sizes on the confidence.
        let calibration = Calibration::assess(&outcomes, 5);
        let overconfident = calibration.is_overconfident(0.05);
        let adjustment = calibration.confidence_adjustment();

        let mut caveats = vec![format!(
            "{scored} resolved episode(s) scored; {unresolved} remain open and are excluded"
        )];
        if rates.is_empty() {
            caveats.push(format!(
                "no agent has {MINIMUM_RESOLVED} resolved episodes, so per-agent hit rates are withheld"
            ));
        }

        let mut builder = FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "across {scored} resolved episode(s) the organisation is {}, with a Brier score of {:.3}",
                if overconfident {
                    "overconfident"
                } else {
                    "reasonably calibrated"
                },
                calibration.brier_score
            ),
        )
        // Attribution is a measurement, not a market view.
        .direction(Direction::Neutral, 0.0)
        .fact(computed(
            ctx,
            "brier_score",
            calibration.brier_score,
            "score",
            &["episodes"],
        ))
        .fact(computed(
            ctx,
            "confidence_adjustment",
            adjustment,
            "ratio",
            &["episodes"],
        ))
        .fact(computed(
            ctx,
            "resolved_episode_count",
            scored as f64,
            "count",
            &["episodes"],
        ))
        .fact(computed(
            ctx,
            "open_episode_count",
            unresolved as f64,
            "count",
            &["episodes"],
        ))
        .evidence(vec!["memory:episodes".to_string()])
        .falsifiers(vec![
            "the next cohort of resolved episodes shows the opposite calibration".to_string(),
        ])
        .caveats(caveats)
        .follow_ups(if overconfident {
            vec![format!(
                "scale reported convictions by {adjustment:.2} until calibration improves"
            )]
        } else {
            Vec::new()
        });

        for (agent_id, rate, count) in &rates {
            builder = builder.fact(computed(
                ctx,
                &format!("hit_rate_{agent_id}"),
                *rate,
                "ratio",
                &["episodes"],
            ));
            // The sample size behind the rate, not just the rate itself: a
            // hit rate reported without the count it rests on cannot be told
            // apart from one backed by ten episodes or a thousand, and the
            // reviewer downstream needs the count to weigh the claim.
            builder = builder.fact(computed(
                ctx,
                &format!("hit_rate_{agent_id}_episode_count"),
                *count as f64,
                "count",
                &["episodes"],
            ));
        }

        builder.build()
    }
}
