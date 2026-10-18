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
//! * **Procedural** — how a kind of question has been answered well: a named,
//!   versioned sequence of steps, recorded only once a minimum number of held
//!   episodes that followed it resolved correctly. One success is luck; a
//!   procedure written from it would be a ritual.
//!
//! Episodic memory is bounded. [`ResearchMemory::with_capacity`] fixes how
//! many episodes are held, and recording past it gives up the oldest one,
//! counted in [`ResearchMemory::evicted`]. An unbounded store here grew with
//! every agent run for the life of the process; the event log is the record,
//! and this is a working set over it.
//!
//! Everything is bitemporal. A lesson recorded on Tuesday cannot inform a
//! backtest of Monday, and [`ResearchMemory::lessons_as_of`] is the only way to
//! read lessons, so that guarantee is structural rather than a convention.

use qip_core::error::{Error, Result};
use qip_core::ids::{AgentRunId, LessonId};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Correct, resolved episodes a procedure must cite before it is recorded.
pub const MINIMUM_PROCEDURE_SUPPORT: usize = 3;

/// Versions kept per procedure name. Past it the oldest is given up; the
/// version number still rises.
pub const VERSIONS_PER_PROCEDURE: usize = 16;

/// Procedure names held. A new name past it is refused rather than evicting
/// a procedure something may still follow.
pub const MAX_PROCEDURES: usize = 256;

/// A way of answering a kind of question, distilled from episodes that
/// followed it and turned out right.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Procedure {
    /// The kind of question it answers, e.g. `earnings-surprise-review`.
    pub name: String,
    /// One for the first version of a name, then rising by one.
    pub version: u32,
    /// The steps, in order.
    pub steps: Vec<String>,
    /// The resolved, correct episodes it was distilled from.
    pub supporting_episodes: Vec<AgentRunId>,
    /// When it became known. Reads are filtered on this.
    pub learned_at: Timestamp,
}

/// Episodes [`ResearchMemory::new`] holds before giving up the oldest.
pub const DEFAULT_EPISODE_CAPACITY: usize = 4_096;

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
#[derive(Clone, Debug)]
pub struct ResearchMemory {
    episodes: VecDeque<Episode>,
    /// How many episodes have been given up: the position of `episodes[0]`
    /// in the sequence of everything ever recorded. `by_agent` holds those
    /// absolute positions, so evicting the front never renumbers the index.
    evicted: usize,
    capacity: usize,
    lessons: Vec<Lesson>,
    /// Versions per procedure name, oldest first, never empty.
    procedures: BTreeMap<String, VecDeque<Procedure>>,
    by_agent: BTreeMap<String, VecDeque<usize>>,
}

impl Default for ResearchMemory {
    fn default() -> Self {
        Self {
            episodes: VecDeque::new(),
            evicted: 0,
            capacity: DEFAULT_EPISODE_CAPACITY,
            lessons: Vec::new(),
            procedures: BTreeMap::new(),
            by_agent: BTreeMap::new(),
        }
    }
}

impl ResearchMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// A memory holding at most `capacity` episodes.
    ///
    /// Zero is refused rather than read as "unbounded" or clamped to one: a
    /// memory that can hold nothing records nothing, and a caller who meant
    /// the default should say [`ResearchMemory::new`].
    pub fn with_capacity(capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a research memory of capacity zero holds no episode; give a capacity of at \
                 least one, or use ResearchMemory::new for the default",
            ));
        }
        Ok(Self {
            capacity,
            ..Self::default()
        })
    }

    /// Record an episode, giving up the oldest held one if the memory is full.
    pub fn record_episode(&mut self, episode: Episode) {
        if self.episodes.len() >= self.capacity
            && let Some(oldest) = self.episodes.pop_front()
        {
            if let Some(indices) = self.by_agent.get_mut(&oldest.agent_id) {
                // Positions per agent ascend, so the oldest episode is the
                // front of its own agent's list.
                indices.pop_front();
                if indices.is_empty() {
                    self.by_agent.remove(&oldest.agent_id);
                }
            }
            self.evicted += 1;
        }
        let position = self.evicted + self.episodes.len();
        self.by_agent
            .entry(episode.agent_id.clone())
            .or_default()
            .push_back(position);
        self.episodes.push_back(episode);
    }

    /// The most episodes this memory holds.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Agents with at least one held episode, in order. An agent whose every
    /// episode has been given up is not listed, so the per-agent index is
    /// bounded by the episodes and not by every agent that ever ran.
    pub fn agents(&self) -> impl Iterator<Item = &str> {
        self.by_agent.keys().map(String::as_str)
    }

    /// How many episodes have been given up to the bound since construction.
    pub fn evicted(&self) -> usize {
        self.evicted
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

    /// Held episodes, oldest first.
    pub fn episodes(&self) -> &VecDeque<Episode> {
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
                    .filter_map(|i| self.episodes.get(i.checked_sub(self.evicted)?))
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

    /// Record the next version of procedure `name`, returning its version.
    ///
    /// Refused unless it names at least [`MINIMUM_PROCEDURE_SUPPORT`]
    /// distinct episodes this memory holds, each resolved correct by `at`:
    /// a procedure is a claim about what worked, and an episode that was
    /// wrong, unresolved, or no longer held is not evidence that anything
    /// did. Also refused: a blank name, no steps, a blank step.
    pub fn record_procedure(
        &mut self,
        name: &str,
        steps: Vec<String>,
        supporting_episodes: Vec<AgentRunId>,
        at: Timestamp,
    ) -> Result<u32> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid("a procedure must be named"));
        }
        if steps.is_empty() || steps.iter().any(|step| step.trim().is_empty()) {
            return Err(Error::invalid(format!(
                "procedure `{name}` needs at least one step and no blank step"
            )));
        }
        let mut distinct: Vec<&AgentRunId> = Vec::new();
        for run_id in &supporting_episodes {
            if distinct.contains(&run_id) {
                continue;
            }
            let succeeded = self.episodes.iter().any(|episode| {
                &episode.run_id == run_id
                    && episode
                        .outcome
                        .as_ref()
                        .is_some_and(|outcome| outcome.correct && outcome.resolved_at <= at)
            });
            if !succeeded {
                return Err(Error::invalid(format!(
                    "procedure `{name}` cites {run_id}, which is not a held episode resolved \
                     correct by {at:?}; cite only episodes that followed it and were right"
                )));
            }
            distinct.push(run_id);
        }
        if distinct.len() < MINIMUM_PROCEDURE_SUPPORT {
            return Err(Error::invalid(format!(
                "procedure `{name}` cites {} correct episode(s) and {MINIMUM_PROCEDURE_SUPPORT} \
                 are required; one success is not a procedure",
                distinct.len()
            )));
        }
        if !self.procedures.contains_key(name) && self.procedures.len() >= MAX_PROCEDURES {
            return Err(Error::invalid(format!(
                "the memory holds {MAX_PROCEDURES} procedures; supersede one before naming \
                 another"
            )));
        }
        let versions = self.procedures.entry(name.to_string()).or_default();
        let version = versions
            .back()
            .map_or(1, |latest| latest.version.saturating_add(1));
        if versions.len() >= VERSIONS_PER_PROCEDURE {
            versions.pop_front();
        }
        versions.push_back(Procedure {
            name: name.to_string(),
            version,
            steps,
            supporting_episodes,
            learned_at: at,
        });
        Ok(version)
    }

    /// The latest version of procedure `name` known at `as_of`.
    pub fn procedure_as_of(&self, name: &str, as_of: Timestamp) -> Option<&Procedure> {
        self.procedures
            .get(name.trim())
            .and_then(|versions| versions.iter().rev().find(|p| p.learned_at <= as_of))
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
