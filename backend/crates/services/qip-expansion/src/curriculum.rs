//! Step 3 onward: score a candidate on the eleven declared inputs
//! (EXPAND-015), admit it against the platform's bounds (EXPAND-060), rank it
//! (EXPAND-014, EXPAND-042) and start only what this round's budget covers
//! (EXPAND-059). A task needing a tool nobody has registered is held as
//! blocked and the tool becomes work of its own (EXPAND-008).
//!
//! Every refusal happens in [`ResearchQueue::admit`], before anything is
//! queued: a task that may not run is not ranked low, it is absent.

use crate::gap::RaisedGap;
use qip_agents::research::{ResearchRegistry, Screening};
use qip_agents::tools::{ToolKind, ToolPermission, ToolRegistry, ToolSpec};
use qip_contracts::expansion::CurriculumItem;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use std::collections::BTreeSet;

/// Entries the queue holds. A full queue refuses rather than evicting a task
/// nobody has answered.
pub const MAX_QUEUE: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskKind {
    DataAcquisition,
    Simulation,
    Label,
    Experiment,
    Campaign,
    /// Building a tool some other task is blocked on.
    ToolBuild,
}

impl TaskKind {
    /// The five kinds of research the curriculum decides between (§23.3).
    pub const RESEARCH: [Self; 5] = [
        Self::DataAcquisition,
        Self::Simulation,
        Self::Label,
        Self::Experiment,
        Self::Campaign,
    ];
}

/// The eleven ranking inputs as a candidate states them. Each is optional
/// here so that a missing one is refused by name instead of read as zero.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Criteria {
    pub expected_economic_value: Option<Decimal>,
    pub uncertainty_reduction: Option<f64>,
    pub strategic_coverage: Option<f64>,
    pub risk_reduction: Option<f64>,
    pub information_gain: Option<f64>,
    pub reuse_across_domains: Option<f64>,
    pub compute_data_cost: Option<Decimal>,
    pub feasibility: Option<f64>,
    pub novelty: Option<f64>,
    pub failure_frequency: Option<f64>,
    pub urgency: Option<f64>,
}

/// A candidate's score: all eleven inputs, and the priority they give.
/// Built only by [`Criteria::score`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    pub expected_economic_value: Decimal,
    pub uncertainty_reduction: f64,
    pub strategic_coverage: f64,
    pub risk_reduction: f64,
    pub information_gain: f64,
    pub reuse_across_domains: f64,
    pub compute_data_cost: Decimal,
    pub feasibility: f64,
    pub novelty: f64,
    pub failure_frequency: f64,
    pub urgency: f64,
    priority: f64,
}

impl Score {
    pub const fn priority(&self) -> f64 {
        self.priority
    }
}

fn stated<T>(name: &str, value: Option<T>) -> Result<T> {
    value.ok_or_else(|| {
        Error::invalid(format!(
            "the candidate states no {name}; a missing criterion is not scored as zero, so measure it or state the zero"
        ))
    })
}

fn fraction(name: &str, value: Option<f64>) -> Result<f64> {
    let value = stated(name, value)?;
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(Error::invalid(format!(
            "{name} must be a fraction in [0, 1], not {value}"
        )));
    }
    Ok(value)
}

impl Criteria {
    /// Refuses a candidate missing any of the eleven inputs.
    ///
    /// Novelty multiplies and never adds: a candidate with nothing but novelty
    /// has a priority of zero, below any candidate with economic value or
    /// information gain (EXPAND-042).
    // ponytail: equal weights on the additive terms; fit a weights table once
    // closed items have outcomes to fit it to.
    pub fn score(&self) -> Result<Score> {
        let expected_economic_value =
            stated("expected_economic_value", self.expected_economic_value)?;
        if expected_economic_value.is_negative() {
            return Err(Error::invalid(
                "expected_economic_value must not be negative; a task expected to lose money is not research to rank",
            ));
        }
        let compute_data_cost = stated("compute_data_cost", self.compute_data_cost)?;
        if !compute_data_cost.is_positive() {
            return Err(Error::invalid(
                "compute_data_cost must be greater than zero; a task nobody costed cannot be fitted to a budget",
            ));
        }
        let information_gain = stated("information_gain", self.information_gain)?;
        if !information_gain.is_finite() || information_gain < 0.0 {
            return Err(Error::invalid(format!(
                "information_gain must be finite and not negative, not {information_gain}"
            )));
        }
        let feasibility = fraction("feasibility", self.feasibility)?;
        if feasibility <= 0.0 {
            return Err(Error::denied(
                "feasibility is zero; a task that cannot lawfully or operationally run is refused, not ranked last",
            ));
        }
        let uncertainty_reduction = fraction("uncertainty_reduction", self.uncertainty_reduction)?;
        let strategic_coverage = fraction("strategic_coverage", self.strategic_coverage)?;
        let risk_reduction = fraction("risk_reduction", self.risk_reduction)?;
        let reuse_across_domains = fraction("reuse_across_domains", self.reuse_across_domains)?;
        let novelty = fraction("novelty", self.novelty)?;
        let failure_frequency = fraction("failure_frequency", self.failure_frequency)?;
        let urgency = fraction("urgency", self.urgency)?;

        // Money meets statistics here. Value per unit of cost is a ratio used
        // only to order candidates; it never moves or accounts for money.
        let value_per_cost = expected_economic_value.to_f64() / compute_data_cost.to_f64();
        let merit = value_per_cost
            + uncertainty_reduction
            + strategic_coverage
            + risk_reduction
            + information_gain
            + reuse_across_domains
            + failure_frequency
            + urgency;
        let priority = feasibility * merit * (1.0 + novelty);
        if !priority.is_finite() {
            return Err(Error::numeric(
                "the priority is not finite; restate the economic value and cost in the same currency",
            ));
        }
        Ok(Score {
            expected_economic_value,
            uncertainty_reduction,
            strategic_coverage,
            risk_reduction,
            information_gain,
            reuse_across_domains,
            compute_data_cost,
            feasibility,
            novelty,
            failure_frequency,
            urgency,
            priority,
        })
    }
}

/// One thing a task has to be able to reach before it can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    /// A data source, by its Source Registry id.
    Source(String),
    Tool {
        name: String,
        kind: ToolKind,
        permission: ToolPermission,
    },
}

impl Need {
    fn name(&self) -> &str {
        match self {
            Self::Source(name) | Self::Tool { name, .. } => name.trim(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub kind: TaskKind,
    pub item: CurriculumItem,
    pub criteria: Criteria,
    /// The signal this work answers, where one raised it. Kept on the queued
    /// entry as its lineage.
    pub origin: Option<RaisedGap>,
    /// One per name in `item.tools_and_data`, saying what that name is.
    pub needs: Vec<Need>,
    pub jurisdiction: String,
}

/// What a candidate is admitted against. The caller states each; this crate
/// evaluates no licence and decides no jurisdiction of its own.
#[derive(Debug, Clone, Copy)]
pub struct Bounds<'a> {
    /// Sources whose licence has been evaluated and permits research use.
    pub licensed_sources: &'a BTreeSet<String>,
    pub tools: &'a ToolRegistry,
    pub eligible_jurisdictions: &'a BTreeSet<String>,
    pub research: &'a ResearchRegistry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Queued,
    Started,
    /// Did not fit this round's budget. Considered again next round.
    Deferred {
        reason: String,
    },
    /// Needs tools nobody has registered. Admit it again once they exist.
    Blocked {
        tools: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    kind: TaskKind,
    item: CurriculumItem,
    score: Score,
    origin: Option<RaisedGap>,
    state: State,
}

impl Entry {
    pub const fn kind(&self) -> TaskKind {
        self.kind
    }
    pub const fn item(&self) -> &CurriculumItem {
        &self.item
    }
    pub const fn score(&self) -> &Score {
        &self.score
    }
    pub const fn origin(&self) -> Option<&RaisedGap> {
        self.origin.as_ref()
    }
    pub const fn state(&self) -> &State {
        &self.state
    }
}

/// What one round may spend. Compute is charged with a task's
/// `compute_data_cost`, capital with its item's own `budget`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub compute: Decimal,
    pub capital: Decimal,
}

/// The research questions one round started and deferred, in rank order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Round {
    pub started: Vec<String>,
    pub deferred: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ResearchQueue {
    entries: Vec<Entry>,
    proposals: Vec<ToolSpec>,
}

impl ResearchQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a candidate or refuse it. Nothing is queued on a refusal.
    pub fn admit(&mut self, candidate: Candidate, bounds: &Bounds<'_>) -> Result<State> {
        let Candidate {
            kind,
            item,
            criteria,
            origin,
            needs,
            jurisdiction,
        } = candidate;
        // A blocked task queues one build item per tool beside itself.
        let arriving = 1 + needs
            .iter()
            .filter(|n| matches!(n, Need::Tool { .. }))
            .count();
        if self.entries.len() + arriving > MAX_QUEUE {
            return Err(Error::unavailable(format!(
                "the research queue holds {MAX_QUEUE} items; close one before admitting another"
            )));
        }
        item.validate()?;
        let question = item.research_question.trim();
        if self.entry(question).is_some() {
            return Err(Error::invalid(format!(
                "`{question}` is already in the queue; one question is one item"
            )));
        }
        if let Screening::PriorNull { evidence, .. } = bounds.research.screen(question) {
            return Err(Error::denied(format!(
                "`{question}` was already tested and found null ({evidence}); cite what has changed before asking it again"
            )));
        }
        let score = criteria.score()?;
        if !bounds.eligible_jurisdictions.contains(jurisdiction.trim()) {
            return Err(Error::denied(format!(
                "jurisdiction `{jurisdiction}` is not one this platform is eligible to research in; the task is refused, not ranked lower"
            )));
        }
        let declared: BTreeSet<&str> = needs.iter().map(Need::name).collect();
        let listed: BTreeSet<&str> = item.tools_and_data.iter().map(|n| n.trim()).collect();
        if declared != listed {
            return Err(Error::invalid(format!(
                "tools_and_data lists {listed:?} but the needs declare {declared:?}; say of every name whether it is a source or a tool"
            )));
        }

        let mut missing = Vec::new();
        for need in &needs {
            match need {
                Need::Source(id) => {
                    if !bounds.licensed_sources.contains(id.trim()) {
                        return Err(Error::denied(format!(
                            "source `{id}` has no evaluated licence permitting research use; evaluate its licensing posture before anything is acquired"
                        )));
                    }
                }
                Need::Tool {
                    name,
                    kind,
                    permission,
                } => match bounds.tools.get(name) {
                    Some(_) => bounds.tools.authorise(name.trim(), *permission)?,
                    None => missing.push(bounds.tools.propose(name, *kind)?),
                },
            }
        }

        if missing.is_empty() {
            self.entries.push(Entry {
                kind,
                item,
                score,
                origin,
                state: State::Queued,
            });
            return Ok(State::Queued);
        }

        // The build inherits the blocked task's score and budget: a missing
        // tool is worth the work it unblocks, and nobody has costed the build.
        for tool in &missing {
            if self.proposals.iter().any(|p| p.name() == tool.name()) {
                continue;
            }
            self.entries.push(Entry {
                kind: TaskKind::ToolBuild,
                item: CurriculumItem {
                    research_question: format!(
                        "build the {:?} tool `{}` that `{question}` is blocked on",
                        tool.kind(),
                        tool.name()
                    ),
                    tools_and_data: vec![tool.name().to_string()],
                    evaluation_suite:
                        "the tool's behaviour and data handling, before any scope beyond read"
                            .to_string(),
                    ..item.clone()
                },
                score,
                origin: origin.clone(),
                state: State::Queued,
            });
            self.proposals.push(tool.clone());
        }
        let state = State::Blocked {
            tools: missing.iter().map(|t| t.name().to_string()).collect(),
        };
        self.entries.push(Entry {
            kind,
            item,
            score,
            origin,
            state: state.clone(),
        });
        Ok(state)
    }

    /// Every entry, highest priority first. Ties go to the question's text so
    /// the order replays.
    pub fn ranked(&self) -> Vec<&Entry> {
        let mut ranked: Vec<&Entry> = self.entries.iter().collect();
        ranked.sort_by(|a, b| {
            b.score
                .priority
                .total_cmp(&a.score.priority)
                .then_with(|| a.item.research_question.cmp(&b.item.research_question))
        });
        ranked
    }

    pub fn entry(&self, question: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.item.research_question.trim() == question.trim())
    }

    /// Tool specs proposed for tools a task was blocked on. None is registered.
    pub fn proposals(&self) -> &[ToolSpec] {
        &self.proposals
    }

    /// Start the highest-ranked eligible items the budget covers and defer the
    /// rest by name. A blocked or already started item is not eligible.
    pub fn schedule(&mut self, budget: Budget) -> Result<Round> {
        if budget.compute.is_negative() || budget.capital.is_negative() {
            return Err(Error::invalid(
                "an expansion budget cannot be negative; state zero to start nothing",
            ));
        }
        let order: Vec<String> = self
            .ranked()
            .into_iter()
            .filter(|e| matches!(e.state, State::Queued | State::Deferred { .. }))
            .map(|e| e.item.research_question.clone())
            .collect();
        let (mut compute, mut capital) = (budget.compute, budget.capital);
        let mut round = Round::default();
        for question in order {
            let Some(entry) = self
                .entries
                .iter_mut()
                .find(|e| e.item.research_question == question)
            else {
                continue;
            };
            let (cost, hold) = (entry.score.compute_data_cost, entry.item.budget);
            match (compute.checked_sub(cost), capital.checked_sub(hold)) {
                (Some(c), Some(k)) if !c.is_negative() && !k.is_negative() => {
                    (compute, capital) = (c, k);
                    entry.state = State::Started;
                    round.started.push(question);
                }
                _ => {
                    entry.state = State::Deferred {
                        reason: format!(
                            "needs {cost} of compute and {hold} of capital; {compute} and {capital} remain this round"
                        ),
                    };
                    round.deferred.push(question);
                }
            }
        }
        Ok(round)
    }

    /// Remove a started item once its result is recorded elsewhere.
    pub fn close(&mut self, question: &str) -> Result<Entry> {
        let at = self
            .entries
            .iter()
            .position(|e| e.item.research_question.trim() == question.trim())
            .ok_or_else(|| Error::not_found(format!("`{question}` is not in the queue")))?;
        if self.entries[at].state != State::Started {
            return Err(Error::invalid(format!(
                "`{question}` was never started; only a started item has a result to close on"
            )));
        }
        Ok(self.entries.remove(at))
    }
}
