//! The expansion loop as one object a composition root can hold: raised gaps
//! in, a ranked and budgeted queue out, and every item's lifecycle from
//! admission to close or withdrawal (EXPAND-025, EXPAND-048, EXPAND-063).
//!
//! `curriculum.rs` scores a candidate on eleven inputs and refuses one
//! missing any; until this file nothing built a candidate, because nothing
//! could say where the eleven numbers came from. Here each is derived from
//! something measured — the detector's severity, how often the same gap has
//! been raised, how much of its class is already queued — or stated as zero
//! where nothing measures it, and [`criteria_for`] is the one place that
//! derivation lives, so a reader can check every input against its source.
//!
//! Nothing here acts. A started item is a record that it may run within this
//! round's budget; the desk fetches nothing and reaches no venue.

use crate::curriculum::{
    Bounds, Budget, Candidate, Criteria, Need, ResearchQueue, State, TaskKind,
};
use crate::gap::{GapTrigger, RaisedGap};
use crate::plan::{AcquisitionAction, Derivation, ResearchPlan};
use crate::promotion::{EvaluationRecord, ResearchCampaign, promote};
use qip_agents::research::{ResearchRegistry, Verdict};
use qip_agents::tools::{ToolPermission, ToolRegistry};
use qip_contracts::expansion::{CapabilityEntry, CurriculumItem, GapClass};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Closed items the desk remembers, newest last. Bounded: the event log is
/// the record, this is what an operator reads on the cycle line.
pub const CLOSED_RETAINED: usize = 64;

/// How the desk runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskConfig {
    /// Sweeps a gap may go unobserved before its unstarted item is withdrawn
    /// or its started item closes on the stop condition "the gap went quiet".
    pub quiet_sweeps: u64,
    /// Sweeps a started item may run before it closes on the stop condition
    /// "out of time", whether or not its gap is still raised.
    pub max_started_sweeps: u64,
    /// The jurisdiction the desk researches in. Checked at admission against
    /// the eligible set the caller states; never assumed eligible.
    pub jurisdiction: String,
    /// Who owns the queued items.
    pub owner: String,
}

impl DeskConfig {
    pub fn validate(&self) -> Result<()> {
        if self.quiet_sweeps == 0 || self.max_started_sweeps == 0 {
            return Err(Error::invalid(
                "quiet_sweeps and max_started_sweeps must be at least one; zero would withdraw or close every item the sweep it was queued",
            ));
        }
        if self.jurisdiction.trim().is_empty() || self.owner.trim().is_empty() {
            return Err(Error::invalid(
                "the desk needs a jurisdiction and an owner; neither is assumed",
            ));
        }
        Ok(())
    }
}

/// The compute an item of each kind is costed at, in the desk's compute
/// units. A declared cost model rather than a measurement: nothing in the
/// tree yet meters what a research task spends, and the curriculum refuses
/// an uncosted task, so the table is stated here, once, where it can be
/// changed and seen to change.
pub fn declared_cost(kind: TaskKind) -> Decimal {
    Decimal::from_int(match kind {
        TaskKind::Label => 1,
        TaskKind::DataAcquisition | TaskKind::Experiment | TaskKind::ToolBuild => 2,
        TaskKind::Simulation => 3,
        TaskKind::Campaign => 5,
    })
}

/// The kind of research each gap class is.
pub const fn task_kind(class: GapClass) -> TaskKind {
    match class {
        GapClass::Data | GapClass::Memory => TaskKind::DataAcquisition,
        GapClass::Ontology => TaskKind::Label,
        GapClass::Causal => TaskKind::Simulation,
        GapClass::Execution => TaskKind::Campaign,
        GapClass::Model
        | GapClass::Tool
        | GapClass::Specialist
        | GapClass::ComputeQuantumResearch => TaskKind::Experiment,
    }
}

/// Triggers that are a failure of something the platform already does, as
/// opposed to a lack it noticed (EXPAND-066).
pub const fn is_failure(trigger: GapTrigger) -> bool {
    matches!(
        trigger,
        GapTrigger::UnexplainedPnl
            | GapTrigger::MissedOpportunity
            | GapTrigger::ModelDrift
            | GapTrigger::Contradiction
    )
}

/// What the desk has measured about a gap that a candidate is scored on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measured {
    /// Times this question was raised before this one.
    pub repeats: u64,
    /// Items of the same class already in the queue.
    pub class_open: usize,
}

/// The eleven inputs, each from a named measurement or a stated zero.
pub fn criteria_for(raised: &RaisedGap, kind: TaskKind, measured: Measured) -> Criteria {
    let severity = raised.signal.severity;
    let failure = is_failure(raised.trigger);
    let repeated = measured.repeats as f64 / (measured.repeats as f64 + 1.0);
    let domains = raised.signal.affected_domains.len().max(1) as f64;
    Criteria {
        // The detector's own figure. Every detector in `detectors.rs` states
        // zero; the anomaly mapper states one from the z-score.
        expected_economic_value: Some(raised.signal.economic_value),
        // A severe detection resolved is the most uncertainty removed.
        uncertainty_reduction: Some(severity),
        // A class with nothing queued gains most from one more item.
        strategic_coverage: Some(1.0 / (1.0 + measured.class_open as f64)),
        // Only research into a failure reduces a risk the platform runs.
        risk_reduction: Some(if failure { severity } else { 0.0 }),
        information_gain: Some(severity),
        // Zero for a gap in one domain; approaches one as it spans more.
        reuse_across_domains: Some(1.0 - 1.0 / domains),
        compute_data_cost: Some(declared_cost(kind)),
        // Admission refuses what is infeasible (jurisdiction, licence,
        // tool scope); what reaches scoring is feasible.
        feasibility: Some(1.0),
        // First sighting is fully novel; each repeat halves, thirds, ...
        novelty: Some(1.0 / (measured.repeats as f64 + 1.0)),
        failure_frequency: Some(if failure { repeated } else { 0.0 }),
        // Nothing measures a deadline on a gap yet; stated, not guessed.
        urgency: Some(0.0),
    }
}

/// What happened to one raised gap offered to the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    Queued,
    /// Queued as blocked on these tools; a build item was queued for each.
    Blocked {
        tools: Vec<String>,
    },
    /// The same question is already in the queue; its sighting was counted.
    Duplicate,
    /// Admission refused it, and why.
    Refused {
        reason: String,
    },
}

impl Admission {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Blocked { .. } => "blocked",
            Self::Duplicate => "duplicate",
            Self::Refused { .. } => "refused",
        }
    }
}

/// Why a started item closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// Its gap stopped being raised for `quiet_sweeps` sweeps.
    GapQuiet,
    /// It ran `max_started_sweeps` sweeps.
    OutOfTime,
    /// It was evaluated and promoted to a capability.
    Promoted,
    /// It was evaluated and failed a criterion; the null is recorded.
    NullRecorded,
    /// The compute market retired it: its realised value per unit stayed at
    /// or below the floor for the policy's window (`meta.rs`).
    Unproductive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closed {
    pub question: String,
    pub reason: CloseReason,
}

/// One sweep's outcome, for the cycle line and the series.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub started: Vec<String>,
    pub deferred: Vec<String>,
    pub closed: Vec<Closed>,
    pub withdrawn: Vec<String>,
    /// Items in the queue after the sweep, in any state.
    pub queued: usize,
    /// Tool proposals on record.
    pub proposals: usize,
}

#[derive(Debug, Clone)]
struct Seen {
    count: u64,
    last_sweep: u64,
    started_sweep: Option<u64>,
}

/// What the desk keeps beside each queued item.
#[derive(Debug, Clone, PartialEq)]
pub struct Lineage {
    pub class: GapClass,
    pub action: AcquisitionAction,
    pub derivation: Derivation,
    pub plan: ResearchPlan,
}

#[derive(Debug)]
pub struct ExpansionDesk {
    config: DeskConfig,
    queue: ResearchQueue,
    tools: ToolRegistry,
    research: ResearchRegistry,
    sweep: u64,
    seen: BTreeMap<String, Seen>,
    lineage: BTreeMap<String, Lineage>,
    closed: VecDeque<Closed>,
}

/// The question a raised gap asks. Deterministic, so the same gap raised on
/// two sweeps is one item and not two.
pub fn question_for(raised: &RaisedGap) -> String {
    format!(
        "{:?} gap: {}",
        raised.signal.gap_class,
        raised.signal.observation.trim()
    )
}

impl ExpansionDesk {
    /// `tools` is the registry of tools the caller actually has; a gap whose
    /// acquisition needs one it lacks is queued blocked with a build item.
    pub fn new(config: DeskConfig, tools: ToolRegistry) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            queue: ResearchQueue::new(),
            tools,
            research: ResearchRegistry::new(),
            sweep: 0,
            seen: BTreeMap::new(),
            lineage: BTreeMap::new(),
            closed: VecDeque::new(),
        })
    }

    pub const fn queue(&self) -> &ResearchQueue {
        &self.queue
    }

    pub fn lineage(&self, question: &str) -> Option<&Lineage> {
        self.lineage.get(question.trim())
    }

    pub fn closed(&self) -> impl Iterator<Item = &Closed> {
        self.closed.iter()
    }

    pub const fn sweep(&self) -> u64 {
        self.sweep
    }

    /// Offer one raised gap. Every refusal is returned, never swallowed.
    pub fn offer(
        &mut self,
        raised: RaisedGap,
        licensed_sources: &BTreeSet<String>,
        eligible_jurisdictions: &BTreeSet<String>,
    ) -> Admission {
        let question = question_for(&raised);
        let class = raised.signal.gap_class;
        let sweep = self.sweep;
        let seen = self.seen.entry(question.clone()).or_insert(Seen {
            count: 0,
            last_sweep: sweep,
            started_sweep: None,
        });
        let repeats = seen.count;
        seen.count = seen.count.saturating_add(1);
        seen.last_sweep = sweep;
        if self.queue.entry(&question).is_some() {
            return Admission::Duplicate;
        }
        let class_open = self.lineage.values().filter(|l| l.class == class).count();
        let kind = task_kind(class);
        let action = AcquisitionAction::for_class(class);
        let tools = action.tools();
        let cost = declared_cost(kind);
        let criteria = criteria_for(
            &raised,
            kind,
            Measured {
                repeats,
                class_open,
            },
        );
        let derivation = Derivation::of(&question, &raised);
        let plan = ResearchPlan::synthesise_with(&question, class, |t| self.tools.get(t).is_some());
        let candidate = Candidate {
            kind,
            item: CurriculumItem {
                research_question: question.clone(),
                expected_information_value: raised.signal.severity,
                expected_economic_value: raised.signal.economic_value,
                tools_and_data: tools.iter().map(|(t, _)| (*t).to_string()).collect(),
                budget: cost,
                owner: self.config.owner.clone(),
                evaluation_suite: "the nine criteria of promotion::Criterion, across historical, simulated, paper and shadow".to_string(),
                stop_conditions: vec![
                    format!("the gap is not raised for {} sweep(s)", self.config.quiet_sweeps),
                    format!("{} sweep(s) elapse after start", self.config.max_started_sweeps),
                ],
            },
            criteria,
            origin: Some(raised),
            needs: tools
                .iter()
                .map(|(name, kind)| Need::Tool {
                    name: (*name).to_string(),
                    kind: *kind,
                    permission: ToolPermission::Read,
                })
                .collect(),
            jurisdiction: self.config.jurisdiction.clone(),
        };
        let bounds = Bounds {
            licensed_sources,
            tools: &self.tools,
            eligible_jurisdictions,
            research: &self.research,
        };
        match self.queue.admit(candidate, &bounds) {
            Ok(state) => {
                self.lineage.insert(
                    question,
                    Lineage {
                        class,
                        action,
                        derivation,
                        plan,
                    },
                );
                match state {
                    State::Blocked { tools } => Admission::Blocked { tools },
                    _ => Admission::Queued,
                }
            }
            Err(error) => Admission::Refused {
                reason: error.message().to_string(),
            },
        }
    }

    /// End the sweep: close and withdraw what its stop conditions say, then
    /// start what the budget covers, then advance the sweep counter.
    ///
    /// Closing before scheduling is deliberate: an item closed this sweep
    /// frees nothing in the round budget (which is per round), but a
    /// withdrawn item must not be started on the sweep it went quiet.
    pub fn finish_sweep(&mut self, budget: Budget) -> Result<SweepReport> {
        self.finish_sweep_within(budget, None)
    }

    /// [`Self::finish_sweep`], starting only what `granted` names when it is
    /// given — see `ResearchQueue::schedule_within`.
    pub fn finish_sweep_within(
        &mut self,
        budget: Budget,
        granted: Option<&BTreeSet<String>>,
    ) -> Result<SweepReport> {
        let mut report = SweepReport::default();
        let sweep = self.sweep;
        let entries: Vec<(String, bool)> = self
            .queue
            .ranked()
            .into_iter()
            .map(|e| {
                (
                    e.item().research_question.clone(),
                    *e.state() == State::Started,
                )
            })
            .collect();
        for (question, started) in entries {
            let Some(seen) = self.seen.get(&question) else {
                // A tool-build item: its lifetime is its blocked task's.
                continue;
            };
            let quiet = sweep.saturating_sub(seen.last_sweep) >= self.config.quiet_sweeps;
            if started {
                let ran = seen.started_sweep.map_or(0, |s| sweep.saturating_sub(s));
                let reason = if quiet {
                    Some(CloseReason::GapQuiet)
                } else if ran >= self.config.max_started_sweeps {
                    Some(CloseReason::OutOfTime)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    self.queue.close(&question)?;
                    self.forget(&question);
                    let closed = Closed { question, reason };
                    report.closed.push(closed.clone());
                    self.remember(closed);
                }
            } else if quiet {
                self.queue.withdraw(&question)?;
                self.forget(&question);
                report.withdrawn.push(question);
            }
        }
        // Build items whose blocked task is gone go with it.
        let orphans: Vec<String> = self
            .queue
            .ranked()
            .into_iter()
            .filter(|e| e.kind() == TaskKind::ToolBuild && *e.state() != State::Started)
            .filter(|e| {
                e.origin()
                    .map(question_for)
                    .is_none_or(|q| self.queue.entry(&q).is_none())
            })
            .map(|e| e.item().research_question.clone())
            .collect();
        for question in orphans {
            self.queue.withdraw(&question)?;
            report.withdrawn.push(question);
        }

        let round = self.queue.schedule_within(budget, granted)?;
        for question in &round.started {
            if let Some(seen) = self.seen.get_mut(question) {
                seen.started_sweep = Some(sweep);
            }
        }
        report.started = round.started;
        report.deferred = round.deferred;
        report.queued = self.queue.len();
        report.proposals = self.queue.proposals().len();
        // Sightings of questions no longer queued are kept only while they
        // could still recur within the quiet window; past it they are
        // dropped, so the map is bounded by what was raised recently.
        let quiet_sweeps = self.config.quiet_sweeps;
        let queue = &self.queue;
        self.seen.retain(|q, s| {
            queue.entry(q).is_some() || sweep.saturating_sub(s.last_sweep) < quiet_sweeps
        });
        self.sweep = sweep.saturating_add(1);
        Ok(report)
    }

    /// Close a started item on its evaluation, converting it to a capability
    /// when the campaign ran all four environments and every criterion
    /// passed (EXPAND-025). On a failed criterion the null is recorded in the
    /// research registry with the failed criteria as evidence, so the same
    /// question is refused at admission next time rather than paid for twice.
    pub fn conclude(
        &mut self,
        campaign: &ResearchCampaign,
        evaluation: &EvaluationRecord,
    ) -> Result<CapabilityEntry> {
        let question = campaign.question().to_string();
        let class = self
            .lineage
            .get(&question)
            .map(|l| l.class)
            .ok_or_else(|| Error::not_found(format!("`{question}` has no lineage on this desk")))?;
        let entry = self
            .queue
            .entry(&question)
            .ok_or_else(|| Error::not_found(format!("`{question}` is not in the queue")))?;
        if *entry.state() != State::Started {
            return Err(Error::invalid(format!(
                "`{question}` was never started; only research that ran can be concluded"
            )));
        }
        match promote(class, campaign, evaluation) {
            Ok(capability) => {
                self.queue.close(&question)?;
                self.forget(&question);
                self.research.record(
                    &question,
                    Verdict::Supported {
                        evidence: format!("promoted as {:?}", capability.verb),
                    },
                )?;
                self.remember(Closed {
                    question,
                    reason: CloseReason::Promoted,
                });
                Ok(capability)
            }
            Err(refusal) => {
                let failed = evaluation.failed();
                if !failed.is_empty() && campaign.is_complete() {
                    self.queue.close(&question)?;
                    self.forget(&question);
                    self.research.record(
                        &question,
                        Verdict::Null {
                            evidence: format!("failed {failed:?}"),
                        },
                    )?;
                    self.remember(Closed {
                        question,
                        reason: CloseReason::NullRecorded,
                    });
                }
                Err(refusal)
            }
        }
    }

    /// Every started item and its lineage, in rank order: what the research
    /// executor carries out each sweep.
    pub fn started(&self) -> Vec<(String, &Lineage)> {
        self.queue
            .ranked()
            .into_iter()
            .filter(|e| *e.state() == State::Started)
            .filter_map(|e| {
                let question = e.item().research_question.clone();
                self.lineage.get(&question).map(|l| (question, l))
            })
            .collect()
    }

    /// Retire one item the compute market found unproductive: a started item
    /// closes as [`CloseReason::Unproductive`], an unstarted one is
    /// withdrawn. Returns whether it had started. Refuses a question not in
    /// the queue, so a retirement of something already gone is visible.
    pub fn retire(&mut self, question: &str) -> Result<bool> {
        let started = self
            .queue
            .entry(question)
            .map(|e| *e.state() == State::Started)
            .ok_or_else(|| Error::not_found(format!("`{question}` is not in the queue")))?;
        if started {
            self.queue.close(question)?;
            self.remember(Closed {
                question: question.trim().to_string(),
                reason: CloseReason::Unproductive,
            });
        } else {
            self.queue.withdraw(question)?;
        }
        self.forget(question);
        Ok(started)
    }

    fn forget(&mut self, question: &str) {
        self.lineage.remove(question);
        self.seen.remove(question);
    }

    fn remember(&mut self, closed: Closed) {
        if self.closed.len() == CLOSED_RETAINED {
            self.closed.pop_front();
        }
        self.closed.push_back(closed);
    }
}
