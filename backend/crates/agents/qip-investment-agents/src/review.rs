//! The adversarial reviewer.
//!
//! This agent's job is to be unhelpful. It looks for the reason the
//! organisation's conclusion is wrong, and it is measured on how often it finds
//! one — a reviewer that approves everything has been captured by the people it
//! reviews, which is why the manifest puts it under a different owner and the
//! roster check enforces that.
//!
//! It cannot publish a hypothesis. An adversary that can advance its own theses
//! stops being a check on the others and becomes one of them.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::sync::Arc;

/// A hit rate below which an agent's past record argues against its current
/// view. Set at a coin flip: an agent that has been right less than half the
/// time on resolved episodes has a record that counts against it.
const POOR_HIT_RATE: f64 = 0.5;

/// Attacks the organisation's own conclusions.
#[derive(Debug)]
pub struct AdversarialReviewer {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl AdversarialReviewer {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

/// One objection the reviewer found.
#[derive(Clone, Debug, PartialEq)]
pub struct Objection {
    pub against: String,
    pub finding: String,
}

impl Agent for AdversarialReviewer {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let subject = brief
            .objects
            .first()
            .map(|o| o.as_str().to_string())
            .unwrap_or_else(|| "the question".to_string());

        let mut objections: Vec<Objection> = Vec::new();
        let mut evidence = Vec::new();

        // --- has the organisation been wrong about this before? ---
        {
            let memory = self.desk.memory.get(ctx)?;
            for agent_id in memory
                .episodes()
                .iter()
                .map(|e| e.agent_id.clone())
                .collect::<std::collections::BTreeSet<_>>()
            {
                let Some(rate) = memory.hit_rate(&agent_id, brief.as_of) else {
                    continue;
                };
                let resolved = memory
                    .episodes_as_of(&agent_id, brief.as_of)
                    .iter()
                    .filter(|e| e.outcome.is_some())
                    .count();
                // A poor rate over three episodes is noise; over ten it is a
                // record. The threshold is on the count, not on a p-value,
                // because the reviewer raises a question rather than a verdict.
                if rate < POOR_HIT_RATE && resolved >= 10 {
                    objections.push(Objection {
                        against: agent_id.clone(),
                        finding: format!(
                            "{agent_id} has been right {:.0}% of the time over {resolved} resolved episodes",
                            rate * 100.0
                        ),
                    });
                    evidence.push(format!("memory:hit-rate@{agent_id}"));
                }
            }

            // A lesson that contradicts the current situation is the most
            // useful thing memory holds.
            for lesson in memory.applicable_lessons(&brief.context, brief.as_of) {
                if lesson.smoothed_support() > 0.65 {
                    objections.push(Objection {
                        against: "the current reasoning".to_string(),
                        finding: format!(
                            "a prior lesson applies here ({} support over {} episodes): {}",
                            format_args!("{:.2}", lesson.smoothed_support()),
                            lesson.supporting_episodes.len(),
                            lesson.statement
                        ),
                    });
                    evidence.push(format!("lesson:{}", lesson.lesson_id.as_str()));
                }
            }
        }

        // --- is the position already on? ---
        // A thesis the book is already expressing is not new information; it
        // is a reason to check whether the original reasoning still holds.
        if self.desk.book.is_available(ctx)
            && let Some(object) = brief.objects.first()
        {
            let book = self.desk.book.get(ctx)?;
            if let Some(position) = book.portfolio.position(object)
                && !position.quantity().is_zero()
            {
                objections.push(Objection {
                    against: "novelty".to_string(),
                    finding: format!(
                        "the book already holds {} of {}; this is a re-argument of an existing position, not a new one",
                        position.quantity(),
                        object.as_str()
                    ),
                });
                evidence.push(format!("position:{}", object.as_str()));
            }
        }

        // --- does the research library hold anything contradicting it? ---
        if self.desk.library.is_available(ctx) && !brief.question.trim().is_empty() {
            let library = self.desk.library.get(ctx)?;
            let hits = library.search_as_of(&brief.question, 5, brief.as_of);
            for hit in hits.iter().take(3) {
                evidence.push(format!("document:{}", hit.document.id));
            }
            if hits.is_empty() {
                objections.push(Objection {
                    against: "the evidence base".to_string(),
                    finding: "the research library holds nothing relevant to the question, so the thesis rests entirely on the current reading".to_string(),
                });
            }
        }

        if objections.is_empty() {
            return Ok(no_data(
                ctx,
                brief.as_of,
                format!("found no grounds to object to the treatment of {subject}"),
            ));
        }

        // The reviewer always argues against. Its conviction is in the
        // objections, and the direction is negative by construction — an
        // adversary that reported a positive view would be doing someone
        // else's job.
        let conviction = (objections.len() as f64 / 4.0).min(0.9);

        FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "{} objection(s) to the current treatment of {subject}: {}",
                objections.len(),
                objections
                    .iter()
                    .map(|o| o.finding.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        )
        .direction(Direction::Negative, conviction)
        .fact(computed(
            ctx,
            "objection_count",
            objections.len() as f64,
            "count",
            &["memory", "portfolio", "library"],
        ))
        .evidence(evidence)
        .falsifiers(vec![
            "each objection is answered on its own terms".to_string(),
            "the record the objection rests on turns out not to apply to this situation"
                .to_string(),
        ])
        .caveats(vec![
            "an objection is a question, not a verdict; the control function decides".to_string(),
        ])
        .build()
    }
}
