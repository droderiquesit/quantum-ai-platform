//! Agent memory.
//!
//! Two kinds, kept apart deliberately:
//!
//! * **Episodic** — what happened on a particular run. Append-only, and the
//!   raw material for attribution: "this agent said this, on this evidence, at
//!   this time, and here is what followed".
//! * **Semantic** — lessons distilled from many episodes. A lesson is only
//!   promoted after it has been supported by a minimum number of independent
//!   episodes, because a rule learned from one observation is a coincidence.
//!
//! Everything is bitemporal. A lesson recorded on Tuesday cannot inform a
//! backtest of Monday, and [`ResearchMemory::lessons_as_of`] is the only way to
//! read lessons, so that guarantee is structural rather than a convention.

use qip_core::ids::{AgentRunId, LessonId};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a run concluded, kept verbatim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub run_id: AgentRunId,
    pub agent_id: String,
    /// When the work happened.
    pub occurred_at: Timestamp,
    /// The question the agent was asked.
    pub question: String,
    /// What it concluded, in its own words.
    pub conclusion: String,
    /// Ids of the evidence it relied on, for reconstruction.
    pub evidence: Vec<String>,
    /// Conviction at the time, in `[0, 1]`.
    pub conviction: f64,
    /// Set once the outcome is known — the join point for learning.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub outcome: Option<EpisodeOutcome>,
}

/// What actually happened after an episode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeOutcome {
    pub resolved_at: Timestamp,
    /// Whether the conclusion turned out to be right.
    pub correct: bool,
    /// Realised effect in basis points, when the episode led to a position.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub realised_bps: Option<f64>,
    /// Why it was right or wrong, for the lesson that may follow.
    pub explanation: String,
}

/// A generalisation that survived enough episodes to be worth applying.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    pub lesson_id: LessonId,
    /// When the lesson became known. Reads are filtered on this.
    pub learned_at: Timestamp,
    /// The agent or process that learned it.
    pub learned_by: String,
    /// When the lesson applies, e.g. `regime=stressed`.
    pub applies_when: String,
    /// The lesson itself.
    pub statement: String,
    /// Episodes that support it.
    pub supporting_episodes: Vec<AgentRunId>,
    /// Episodes that contradict it, kept rather than discarded: a lesson with
    /// counter-examples is weaker, not invalid, and hiding them would make it
    /// look stronger than it is.
    pub contradicting_episodes: Vec<AgentRunId>,
    /// Superseded lessons are kept for reconstruction of past decisions.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub superseded_at: Option<Timestamp>,
}

impl Lesson {
    /// Support ratio in `[0, 1]`: how often the lesson held.
    pub fn support(&self) -> f64 {
        let for_it = self.supporting_episodes.len() as f64;
        let against = self.contradicting_episodes.len() as f64;
        let total = for_it + against;
        if total <= 0.0 { 0.0 } else { for_it / total }
    }

    /// Laplace-smoothed support, which is what should actually be trusted.
    ///
    /// Three-for-nothing is not certainty; smoothing reports 0.80 rather than
    /// 1.00 and stops a young lesson dominating an old one.
    pub fn smoothed_support(&self) -> f64 {
        let for_it = self.supporting_episodes.len() as f64;
        let against = self.contradicting_episodes.len() as f64;
        (for_it + 1.0) / (for_it + against + 2.0)
    }

    pub fn is_live(&self, at: Timestamp) -> bool {
        at >= self.learned_at && self.superseded_at.is_none_or(|s| at < s)
    }
}

/// The minimum evidence before an observation becomes a lesson.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromotionPolicy {
    /// Independent supporting episodes required.
    pub minimum_support: usize,
    /// Smoothed support the lesson must reach.
    pub minimum_smoothed_support: f64,
}

impl Default for PromotionPolicy {
    fn default() -> Self {
        // Four episodes at 0.65 smoothed support means at least three of four
        // went the right way. Two-of-two smooths to 0.75 but is one coin flip
        // from noise, so the count floor does the real work.
        Self {
            minimum_support: 4,
            minimum_smoothed_support: 0.65,
        }
    }
}

/// Episodic and semantic memory for the research organisation.
#[derive(Clone, Debug, Default)]
pub struct ResearchMemory {
    episodes: Vec<Episode>,
    lessons: Vec<Lesson>,
    by_agent: BTreeMap<String, Vec<usize>>,
}

impl ResearchMemory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_episode(&mut self, episode: Episode) {
        self.by_agent
            .entry(episode.agent_id.clone())
            .or_default()
            .push(self.episodes.len());
        self.episodes.push(episode);
    }

    /// Attach an outcome to a recorded episode.
    pub fn resolve(&mut self, run_id: &AgentRunId, outcome: EpisodeOutcome) -> bool {
        let Some(episode) = self.episodes.iter_mut().find(|e| &e.run_id == run_id) else {
            return false;
        };
        episode.outcome = Some(outcome);
        true
    }

    pub fn record_lesson(&mut self, lesson: Lesson) {
        self.lessons.push(lesson);
    }

    pub fn episodes(&self) -> &[Episode] {
        &self.episodes
    }

    pub fn episode_count(&self) -> usize {
        self.episodes.len()
    }

    pub fn lesson_count(&self) -> usize {
        self.lessons.len()
    }

    /// Episodes for one agent that had happened by `as_of`.
    pub fn episodes_as_of(&self, agent_id: &str, as_of: Timestamp) -> Vec<&Episode> {
        self.by_agent
            .get(agent_id)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|i| self.episodes.get(*i))
                    .filter(|e| e.occurred_at <= as_of)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Lessons that were known at `as_of` and had not been superseded.
    ///
    /// The only read path for lessons. A caller cannot accidentally consult
    /// tomorrow's knowledge, because there is no method that would let it.
    pub fn lessons_as_of(&self, as_of: Timestamp) -> Vec<&Lesson> {
        self.lessons.iter().filter(|l| l.is_live(as_of)).collect()
    }

    /// Live lessons whose applicability condition matches `situation`.
    pub fn applicable_lessons(&self, situation: &str, as_of: Timestamp) -> Vec<&Lesson> {
        let needle = situation.to_ascii_lowercase();
        self.lessons_as_of(as_of)
            .into_iter()
            .filter(|l| {
                let condition = l.applies_when.to_ascii_lowercase();
                condition.is_empty() || needle.contains(&condition) || condition.contains(&needle)
            })
            .collect()
    }

    /// An agent's hit rate over resolved episodes up to `as_of`.
    ///
    /// Returns `None` rather than a default when nothing has resolved: an
    /// agent with no track record has no track record, and reporting 0.5 would
    /// invent one.
    pub fn hit_rate(&self, agent_id: &str, as_of: Timestamp) -> Option<f64> {
        let resolved: Vec<&Episode> = self
            .episodes_as_of(agent_id, as_of)
            .into_iter()
            .filter(|e| e.outcome.is_some())
            .collect();
        if resolved.is_empty() {
            return None;
        }
        let correct = resolved
            .iter()
            .filter(|e| e.outcome.as_ref().is_some_and(|o| o.correct))
            .count();
        Some(correct as f64 / resolved.len() as f64)
    }

    /// Promote a candidate lesson if the evidence clears the policy.
    ///
    /// Returns the reason for refusal rather than a bare `false`, so a
    /// near-miss is visible to whoever is tuning the policy.
    pub fn promote(
        &mut self,
        lesson: Lesson,
        policy: PromotionPolicy,
    ) -> std::result::Result<LessonId, String> {
        if lesson.supporting_episodes.len() < policy.minimum_support {
            return Err(format!(
                "{} supporting episodes, {} required",
                lesson.supporting_episodes.len(),
                policy.minimum_support
            ));
        }
        if lesson.smoothed_support() < policy.minimum_smoothed_support {
            return Err(format!(
                "smoothed support {:.3} below the {:.3} required",
                lesson.smoothed_support(),
                policy.minimum_smoothed_support
            ));
        }
        let id = lesson.lesson_id.clone();
        self.lessons.push(lesson);
        Ok(id)
    }

    /// Mark a lesson superseded from `at`, keeping it readable for earlier
    /// dates so past decisions stay reconstructable.
    pub fn supersede(&mut self, lesson_id: &LessonId, at: Timestamp) -> bool {
        let Some(lesson) = self.lessons.iter_mut().find(|l| &l.lesson_id == lesson_id) else {
            return false;
        };
        lesson.superseded_at = Some(at);
        true
    }
}
