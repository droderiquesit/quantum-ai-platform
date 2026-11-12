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

use qip_contracts::expansion::{MemoryEntry, MemoryKind};
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

/// The calibration surprise below which a resolved episode is compressed
/// rather than kept whole (ADR 0152 decision 8).
///
/// Positive on purpose: a threshold of zero would retain everything, and a
/// gate that can never refuse reads as curation while doing none.
pub const DEFAULT_IMPORTANCE_THRESHOLD: f64 = 0.3;

/// How many admitted [`MemoryEntry`] records the registry keeps whole, newest
/// last, beside its per-kind counts (R12-EXPAND-6057). The counts are the
/// record of what was admitted; the entries are a bounded working set for a
/// reader that wants the last few. The set never grows with the platform's
/// uptime, and what leaves it is decided by the entries' own declared
/// policies, not by arrival order: see [`MemoryRegistry::admit`].
pub const RETAINED_MEMORY_ENTRIES: usize = 256;

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
    /// The method as rules an agent can follow (R12-WORLD-3034): for each
    /// subject the supporting calls were about, the lean that came out
    /// right. Empty for a procedure recorded as prose steps only, which no
    /// agent can check itself against.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub rules: Vec<ProcedureRule>,
}

/// One step of a procedure an agent can execute: on `subject`, lean `lean`,
/// because `support` of the procedure's correct calls on that subject did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureRule {
    /// The instrument the calls were about, as the brief names it.
    pub subject: String,
    /// The lean that resolved correct: `positive` or `negative`, the
    /// spelling of [`crate::finding::Direction::as_str`].
    pub lean: String,
    /// Correct calls on `subject` that leaned this way. At least one.
    pub support: usize,
}

impl Procedure {
    /// The rule this procedure holds for `subject`, if any.
    pub fn rule_for(&self, subject: &str) -> Option<&ProcedureRule> {
        self.rules.iter().find(|rule| rule.subject == subject)
    }
}

/// The name an agent's own procedure is kept under in research memory.
///
/// One spelling, here, because the kernel writes under it and the agent
/// reads under it: two spellings would deliver a procedure nobody finds.
pub fn procedure_name_for(agent_id: &str) -> String {
    format!("procedure:{agent_id}")
}

/// One agent's resolved calls in one regime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegimeRecord {
    pub correct: usize,
    pub resolved: usize,
}

impl RegimeRecord {
    /// The fraction right, or `None` when nothing has resolved: no record is
    /// no record, never an even chance.
    pub fn hit_rate(&self) -> Option<f64> {
        (self.resolved > 0).then(|| self.correct as f64 / self.resolved as f64)
    }
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
    /// The regime in force when the work happened, as the platform keys it
    /// (`{market}/{volatility}`), when the writer knew one (R12-AMBIENT-3024).
    /// What [`ResearchMemory::reliability_by_regime`] groups on: an agent
    /// right in a calm tape and wrong in a stressed one has two records, and
    /// one pooled hit rate would describe neither.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub regime: Option<String>,
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
///
/// The floors are private since ADR 0198: a policy whose fields could be set
/// to zero was a gate anyone could hold open, so [`Self::new`] refuses a
/// support floor under two episodes and a smoothed floor at or under a coin
/// flip, and a decoded policy goes through the same check.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PromotionPolicyFields")]
pub struct PromotionPolicy {
    /// Independent supporting episodes required.
    minimum_support: usize,
    /// Smoothed support the lesson must reach.
    minimum_smoothed_support: f64,
}

/// The wire form of [`PromotionPolicy`], checked on decode.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct PromotionPolicyFields {
    pub minimum_support: usize,
    pub minimum_smoothed_support: f64,
}

impl TryFrom<PromotionPolicyFields> for PromotionPolicy {
    type Error = Error;
    fn try_from(fields: PromotionPolicyFields) -> Result<Self> {
        Self::new(fields.minimum_support, fields.minimum_smoothed_support)
    }
}

impl PromotionPolicy {
    /// A policy with its floors stated. Refuses fewer than two supporting
    /// episodes (one observation is a coincidence, and zero is no gate) and a
    /// smoothed floor that is not a real fraction above one half (at or
    /// under it, a lesson is promoted on evidence that went the wrong way as
    /// often as the right way).
    pub fn new(minimum_support: usize, minimum_smoothed_support: f64) -> Result<Self> {
        if minimum_support < 2 {
            return Err(Error::invalid(format!(
                "a promotion policy needs at least two supporting episodes, got \
                 {minimum_support}; a lesson learned from one observation is a coincidence \
                 and a floor of zero is no gate"
            )));
        }
        if !minimum_smoothed_support.is_finite()
            || minimum_smoothed_support <= 0.5
            || minimum_smoothed_support > 1.0
        {
            return Err(Error::invalid(format!(
                "a promotion policy's smoothed-support floor must be in (0.5, 1], got \
                 {minimum_smoothed_support}; at or under one half the evidence went the wrong \
                 way as often as the right way"
            )));
        }
        Ok(Self {
            minimum_support,
            minimum_smoothed_support,
        })
    }

    pub const fn minimum_support(&self) -> usize {
        self.minimum_support
    }

    pub const fn minimum_smoothed_support(&self) -> f64 {
        self.minimum_smoothed_support
    }
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

    /// Every agent's resolved record per regime, as known at `as_of`
    /// (R12-AMBIENT-3024): `(agent, regime)` to [`RegimeRecord`].
    ///
    /// Only episodes that carry a regime, had resolved by `as_of` and had
    /// happened by then are counted. An episode with no regime is left out
    /// rather than filed under a regime nobody recorded; [`Self::hit_rate`]
    /// still counts it in the pooled figure.
    pub fn reliability_by_regime(
        &self,
        as_of: Timestamp,
    ) -> BTreeMap<(String, String), RegimeRecord> {
        let mut records: BTreeMap<(String, String), RegimeRecord> = BTreeMap::new();
        for episode in &self.episodes {
            let (Some(regime), Some(outcome)) = (&episode.regime, &episode.outcome) else {
                continue;
            };
            if episode.occurred_at > as_of || outcome.resolved_at > as_of {
                continue;
            }
            let record = records
                .entry((episode.agent_id.clone(), regime.clone()))
                .or_default();
            record.resolved += 1;
            if outcome.correct {
                record.correct += 1;
            }
        }
        records
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
            rules: Vec::new(),
        });
        Ok(version)
    }

    /// Record the next version of procedure `name` as a method: `rules` an
    /// agent can follow, rendered as its steps, under every refusal
    /// [`Self::record_procedure`] makes. Also refused: no rule, a blank
    /// subject, a subject named twice, a lean other than `positive` or
    /// `negative`, or a rule supported by no call. A rule nobody could
    /// follow is a step written for the record and not for the desk.
    pub fn record_method(
        &mut self,
        name: &str,
        rules: Vec<ProcedureRule>,
        supporting_episodes: Vec<AgentRunId>,
        at: Timestamp,
    ) -> Result<u32> {
        if rules.is_empty() {
            return Err(Error::invalid(format!(
                "procedure `{}` is a method with no rule",
                name.trim()
            )));
        }
        let mut subjects: Vec<&str> = Vec::new();
        for rule in &rules {
            if rule.subject.trim().is_empty() {
                return Err(Error::invalid("a procedure rule must name its subject"));
            }
            if subjects.contains(&rule.subject.as_str()) {
                return Err(Error::invalid(format!(
                    "procedure rule for `{}` is given twice; one subject has one lean",
                    rule.subject
                )));
            }
            subjects.push(rule.subject.as_str());
            if rule.lean != "positive" && rule.lean != "negative" {
                return Err(Error::invalid(format!(
                    "procedure rule for `{}` leans `{}`; only positive or negative can be followed",
                    rule.subject, rule.lean
                )));
            }
            if rule.support == 0 {
                return Err(Error::invalid(format!(
                    "procedure rule for `{}` is supported by no call",
                    rule.subject
                )));
            }
        }
        let steps = rules
            .iter()
            .map(|rule| {
                format!(
                    "on {}: lean {} ({} correct call(s))",
                    rule.subject, rule.lean, rule.support
                )
            })
            .collect();
        let version = self.record_procedure(name, steps, supporting_episodes, at)?;
        if let Some(latest) = self
            .procedures
            .get_mut(name.trim())
            .and_then(|versions| versions.back_mut())
        {
            latest.rules = rules;
        }
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

/// What a [`MemoryRegistry`] did with one resolved episode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Surprising enough to keep whole.
    Retained,
    /// Below the threshold: counted into the agent's summary, detail given up.
    Compressed,
}

/// What compression keeps of the episodes it summarised.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompressedSummary {
    pub episodes: usize,
    pub correct: usize,
    /// Sum of the summarised episodes' importance, so a mean is recoverable.
    pub importance_sum: f64,
}

/// Value policy over resolved episodes: which are kept whole and which are
/// compressed (ADR 0152 decision 8, R12-EXPAND-6057).
///
/// The registry only classifies and summarises. It never holds the verdict
/// on the calibration: [`ResearchMemory::hit_rate`] is computed over every
/// resolved episode `ResearchMemory` holds, before any consult, because a
/// consult that dropped the unsurprising calls would keep the surprising
/// misses and bias the rate the platform scores its analysts on.
#[derive(Clone, Debug)]
pub struct MemoryRegistry {
    importance_threshold: f64,
    retained: BTreeMap<String, usize>,
    compressed: BTreeMap<String, CompressedSummary>,
    /// Entries admitted through [`Self::admit`], by kind (R12-EXPAND-6057).
    admitted: BTreeMap<MemoryKind, usize>,
    /// Entries [`Self::admit`] refused because a declared field was missing
    /// or out of range, and entries [`Self::admit_version`] refused for a
    /// version that did not raise the latest. Counted on the registry, so a
    /// producer that builds a malformed entry or replays a stale one is
    /// visible here rather than only in its own error.
    refused: usize,
    /// At most [`RETAINED_MEMORY_ENTRIES`] admitted entries, in admission
    /// order, each with the version of its id it was admitted as and each
    /// still inside its own retention policy.
    entries: VecDeque<(u32, MemoryEntry)>,
    /// The newest creation instant admitted, in epoch milliseconds: the
    /// registry's clock for retention, so expiry replays from the entries
    /// alone and reads no wall clock.
    clock_ms: u64,
    /// Entries dropped because their retention policy's age had passed.
    expired: usize,
    /// Entries dropped at capacity because their value was the least held.
    evicted_for_value: usize,
}

impl MemoryRegistry {
    /// A registry with a positive importance threshold in `(0, 1]`.
    ///
    /// Zero, negative, above one and non-finite are refused rather than
    /// clamped: zero retains everything, so it is a gate that cannot fire.
    pub fn new(importance_threshold: f64) -> Result<Self> {
        if !importance_threshold.is_finite()
            || importance_threshold <= 0.0
            || importance_threshold > 1.0
        {
            return Err(Error::invalid(format!(
                "a memory registry needs an importance threshold in (0, 1], got \
                 {importance_threshold}; zero would retain every episode and never compress one"
            )));
        }
        Ok(Self {
            importance_threshold,
            retained: BTreeMap::new(),
            compressed: BTreeMap::new(),
            admitted: BTreeMap::new(),
            refused: 0,
            entries: VecDeque::new(),
            clock_ms: 0,
            expired: 0,
            evicted_for_value: 0,
        })
    }

    /// The registry at [`DEFAULT_IMPORTANCE_THRESHOLD`].
    pub fn standard() -> Self {
        Self {
            importance_threshold: DEFAULT_IMPORTANCE_THRESHOLD,
            retained: BTreeMap::new(),
            compressed: BTreeMap::new(),
            admitted: BTreeMap::new(),
            refused: 0,
            entries: VecDeque::new(),
            clock_ms: 0,
            expired: 0,
            evicted_for_value: 0,
        }
    }

    pub fn importance_threshold(&self) -> f64 {
        self.importance_threshold
    }

    /// `|outcome − predicted probability|` in `[0, 1]`: the outcome is `1`
    /// when the conclusion was right, the prediction is the conviction held.
    /// `None` for an episode whose outcome is not yet known, which is not an
    /// episode in this registry's sense.
    pub fn importance(episode: &Episode) -> Option<f64> {
        let outcome = episode.outcome.as_ref()?;
        let hit = if outcome.correct { 1.0 } else { 0.0 };
        Some((hit - episode.conviction).abs().clamp(0.0, 1.0))
    }

    /// Classify one resolved episode and count it; `None` for an unresolved
    /// one, which is neither retained nor summarised.
    pub fn consult(&mut self, episode: &Episode) -> Option<Disposition> {
        let importance = Self::importance(episode)?;
        let correct = episode.outcome.as_ref().is_some_and(|o| o.correct);
        if importance < self.importance_threshold {
            let summary = self.compressed.entry(episode.agent_id.clone()).or_default();
            summary.episodes += 1;
            summary.correct += usize::from(correct);
            summary.importance_sum += importance;
            Some(Disposition::Compressed)
        } else {
            *self.retained.entry(episode.agent_id.clone()).or_default() += 1;
            Some(Disposition::Retained)
        }
    }

    pub fn retained_for(&self, agent_id: &str) -> usize {
        self.retained.get(agent_id).copied().unwrap_or(0)
    }

    pub fn summary_for(&self, agent_id: &str) -> Option<&CompressedSummary> {
        self.compressed.get(agent_id)
    }

    /// Admit one typed memory entry — semantic knowledge, an episode, a
    /// procedure, a failure, a simulation or a compressed abstraction — with
    /// its retention and value policies declared (R12-EXPAND-6057).
    ///
    /// Refuses, through `MemoryEntry::validate`, an entry missing a declared
    /// field or carrying one out of range: a blank id or content, a retention
    /// policy that keeps nothing (`max_age_ms == 0`), a value threshold
    /// outside `[0, 1]`, a zero creation instant, an importance outside
    /// `[0, 1]`, or no tags. The refusal is counted on the registry as well
    /// as returned, because the producer that built the entry is the party
    /// least likely to read its own error.
    ///
    /// The declared policies are what decide what the working set keeps.
    /// On every admission the registry's clock advances to the newest
    /// creation instant it has seen, and every held entry older than its own
    /// `retention_policy.max_age_ms` by that clock leaves ([`Self::expired`]).
    /// When the set is still full, the entry of least value leaves
    /// ([`Self::evicted_for_value`]): one below its own
    /// `value_policy.importance_threshold` before any at or above it, then
    /// the lowest importance, then the oldest. First-in-first-out would let a
    /// burst of unsurprising calls push out the one costly failure the
    /// registry exists to remember, and would read a retention policy that
    /// was declared and never consulted.
    ///
    /// Versioned by id (R12-COVERAGE-7170): an id not held is admitted as
    /// version one, and an id already held is admitted as the next version
    /// beside the one it follows, which stays readable through
    /// [`Self::entry_at`]. The failure it prevents: two entries under one id
    /// were both held with nothing to say which was current, so a reader
    /// taking the first match and one taking the last read different
    /// memories under the same name.
    pub fn admit(&mut self, entry: MemoryEntry) -> Result<()> {
        if let Err(error) = entry.validate() {
            self.refused += 1;
            return Err(error);
        }
        let next = match self.latest_version(&entry.id) {
            None => 1,
            Some(latest) => match latest.checked_add(1) {
                Some(next) => next,
                None => {
                    self.refused += 1;
                    return Err(Error::numeric(format!(
                        "memory entry `{}` has no version left to take; admit its successor \
                         under a new id",
                        entry.id
                    )));
                }
            },
        };
        self.hold(next, entry);
        Ok(())
    }

    /// Admit `entry` as exactly `version` of its id, for a producer that
    /// numbers its own revisions (a replayed journal, a distilled abstraction
    /// rewritten on new evidence).
    ///
    /// Refuses everything [`Self::admit`] refuses, a version of zero, and a
    /// version not above the latest held for the id: a late-arriving old
    /// revision admitted over a newer one would become the entry a reader
    /// taking the latest gets, which is the current memory replaced by a
    /// stale one. The refusal is counted like any other.
    pub fn admit_version(&mut self, entry: MemoryEntry, version: u32) -> Result<()> {
        if let Err(error) = entry.validate() {
            self.refused += 1;
            return Err(error);
        }
        if version == 0 {
            self.refused += 1;
            return Err(Error::invalid(format!(
                "memory entry `{}` was offered as version 0; versions start at 1, so admit it \
                 through `admit` and let the registry number it",
                entry.id
            )));
        }
        if let Some(latest) = self.latest_version(&entry.id)
            && version <= latest
        {
            self.refused += 1;
            return Err(Error::invalid(format!(
                "memory entry `{}` version {version} is not above the latest held version \
                 {latest}; admit version {} or higher, or use `admit` to take the next",
                entry.id,
                latest.saturating_add(1)
            )));
        }
        self.hold(version, entry);
        Ok(())
    }

    /// Count and keep one admitted entry under the declared policies (see
    /// [`Self::admit`]): expired entries leave, then at capacity the least
    /// valuable one does, whichever version of whichever id it is.
    fn hold(&mut self, version: u32, entry: MemoryEntry) {
        *self.admitted.entry(entry.kind).or_default() += 1;
        self.clock_ms = self.clock_ms.max(entry.created_at_ms);
        let clock = self.clock_ms;
        let before = self.entries.len();
        self.entries.retain(|(_, held)| {
            clock.saturating_sub(held.created_at_ms) <= held.retention_policy.max_age_ms
        });
        self.expired += before - self.entries.len();
        if self.entries.len() >= RETAINED_MEMORY_ENTRIES {
            let least = self
                .entries
                .iter()
                .enumerate()
                .min_by(|(i, (_, a)), (j, (_, b))| {
                    let valued =
                        |e: &MemoryEntry| e.importance >= e.value_policy.importance_threshold;
                    valued(a)
                        .cmp(&valued(b))
                        .then(a.importance.total_cmp(&b.importance))
                        .then(i.cmp(j))
                })
                .map(|(i, _)| i);
            if let Some(index) = least {
                self.entries.remove(index);
                self.evicted_for_value += 1;
            }
        }
        self.entries.push_back((version, entry));
    }

    /// The highest version held for `id`, or `None` when no version of it is
    /// in the working set.
    ///
    /// Read off the held entries rather than a counter per id, because ids
    /// are minted per resolved call and a counter kept for each would grow
    /// with the platform's uptime. The consequence, stated rather than
    /// hidden: once every version of an id has left the working set the
    /// registry no longer knows the id, and a later entry under it is version
    /// one again. The event log is the record; this is a working set over it.
    pub fn latest_version(&self, id: &str) -> Option<u32> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.id == id)
            .map(|(version, _)| *version)
            .max()
    }

    /// The entry admitted as `version` of `id`, while it is still held.
    pub fn entry_at(&self, id: &str, version: u32) -> Option<&MemoryEntry> {
        self.entries
            .iter()
            .find(|(held, entry)| *held == version && entry.id == id)
            .map(|(_, entry)| entry)
    }

    /// Entries dropped because their declared retention age had passed.
    pub fn expired(&self) -> usize {
        self.expired
    }

    /// Entries dropped at capacity as the least valuable held.
    pub fn evicted_for_value(&self) -> usize {
        self.evicted_for_value
    }

    /// The held entries of `kind` carrying `tag`, in admission order: what a
    /// running decision reads back from the registry.
    pub fn held(&self, kind: MemoryKind, tag: &str) -> impl Iterator<Item = &MemoryEntry> {
        self.entries
            .iter()
            .map(|(_, entry)| entry)
            .filter(move |e| e.kind == kind && e.tags.iter().any(|t| t == tag))
    }

    /// Entries admitted under `kind` since the registry was built.
    pub fn admitted(&self, kind: MemoryKind) -> usize {
        self.admitted.get(&kind).copied().unwrap_or(0)
    }

    /// Entries [`Self::admit`] refused.
    pub fn refused(&self) -> usize {
        self.refused
    }

    /// The retained entries, oldest first.
    pub fn entries(&self) -> impl Iterator<Item = &MemoryEntry> {
        self.entries.iter().map(|(_, entry)| entry)
    }
}
