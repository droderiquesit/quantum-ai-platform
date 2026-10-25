//! The deep brain's run loop reaching the agency (R12-AGENCY-7029,
//! R12-COVERAGE-7190): every ambient signal the research sweep routed and the
//! attention router activated is offered to `qip-agency`'s one door, promoted,
//! cleared by the action guard and written to an internal outbox for a person
//! to read — and, since the learning lane (R12-AGENCY-1006 / 1025 / 1152 /
//! 6107 / 7148), every draft is held to what it did.
//!
//! # Why this exists
//!
//! The ambient mesh ended in an advisory and the agency began at an operator's
//! request document, and nothing joined them. The claim that authority and
//! conduct gates sit between detection and execution was therefore a claim
//! about a path that did not run. This module is the path, called from
//! [`crate::node::run_with`] on the same cycle that journals the sweep.
//!
//! # What it may not do
//!
//! Act. The only effect reachable from here is `qip_agency::outbox`'s append
//! (ADR 0114 Amendment 1, owner decision D5). No `Adapter` is implemented in
//! this file and none is named; the door owns its adapter. Publishing,
//! outreach, pricing and operational change stay refused.
//!
//! # The approvers are policy, not people
//!
//! The promotion approver and the plan approver are identities this module
//! names ([`PROMOTION_APPROVER`], [`PLAN_APPROVER`]); the guard enforces that
//! neither is the acting identity. They record that the standing intake policy
//! admitted the draft, which is all a draft that only a person can act on
//! needs: the person who reads the outbox is the approval that matters. A
//! change that let this path act would need a named human approver here first.
//!
//! # What a draft is held to
//!
//! `qip_agency::engine::learn` — observe the effect, attribute it, update —
//! had no caller on any running path: the CLI learns from a *simulated*
//! outcome after a shadowed pass, and the outbox write is the one executed
//! act in the platform, so it is the one place a *measured* outcome exists.
//! The measurement is the honest one and no more. A draft asks a person to
//! read about a subject; nothing in this process reads the outbox, so the
//! only thing the world reports back is the subject's own reading when a
//! later sweep raises it again. That reading, less the reading at the sweep
//! that drafted (the status quo carried forward — the no-action baseline),
//! is the effect filed. The attribution says what it is: not identified, at
//! identifiability zero, with no control assigned, because ADR 0114 refuses
//! an experiment on the people a draft reaches. The draft's policy predicts
//! an uplift of exactly zero and the memory keeps the outcome as measured, so
//! what the platform learns is that a draft is not shown to move anything —
//! which is the truth, and the opposite of a memory that credits the draft
//! with whatever the subject did next.
//!
//! The eight stages before `ObserveEffect` on a learning record are **not**
//! an engine pass's record. The door runs the guard, not `engine::run`; the
//! eight are passed here to seat the learn stages at their position, and a
//! reader should take `passed` as evidence from `ObserveEffect` on. The
//! record says so in its own field names rather than hiding it.
//!
//! # Failure
//!
//! A sweep's routing never stops the node. An opportunity the door refuses is
//! counted by reason in [`AgencyRound`]; an outbox write that fails is held
//! and retried on the next cycle, bounded (see `qip_agency::ambient_intake`).
//! A learning a stage refuses is journaled withheld with the stage and the
//! reason; a learning journal that refuses the record is counted on the
//! cycle line and the memory still holds the outcome, because the learning
//! ran whether or not its record landed (principle 6). A retried draft is
//! written without the reading its sweep took, so it is not awaited for a
//! follow-up; that gap is counted, not hidden.

use crate::ambient_journal::JournalDestination;
use crate::research::ResearchRound;
use qip_agency::ambient_intake::{
    AmbientIntake, AmbientOpportunity, Intake, IntakeConfig, OUTBOX_AUDIENCE,
};
use qip_agency::attribution::{Effect, EffectAttribution, HorizonObservation, Identification};
use qip_agency::autonomy::{ActionPolicy, Authority};
use qip_agency::engine::{self, Stage, StageLog};
use qip_agency::memory::{ActionOutcome, ActionOutcomeDraft, ActionOutcomeMemory, Observed};
use qip_agency::outbox::{DRAFT_TOOL, OUTBOX_CHANNEL};
use qip_agency::plan::Step;
use qip_contracts::action::Method;
use qip_contracts::ambient::{AmbientSignal, AttentionEvent, Disposition, Pathway, SignalClass};
use qip_core::error::{Error, Result};
use qip_core::lineage::Lineage;
use qip_core::{CorrelationId, Decimal, EventId, Timestamp};
use qip_events::{Envelope, EventBody, EventLog, Topic};
use qip_observability::metrics::{Metrics, labels, names};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The variable naming the outbox's file.
pub const AGENCY_OUTBOX_VARIABLE: &str = "QIP_DEEPBRAIN_AGENCY_OUTBOX";

/// The identity every draft is sent under.
pub const ACTING_IDENTITY: &str = "deepbrain/ambient-drafter";

/// Who promotes an ambient output to the agency.
pub const PROMOTION_APPROVER: &str = "deepbrain/ambient-promotion-policy";

/// Who approves the plan of a draft.
pub const PLAN_APPROVER: &str = "deepbrain/ambient-plan-policy";

/// The producer every learning record carries. Distinct from the outbox's
/// producer so `qip_agency::outbox::drafts` never decodes a learning as a
/// draft.
pub const LEARNING_ORIGIN: &str = "deepbrain/agency-learning";

/// The variable the drafting step moves, as the door names it. The engine
/// compares the tools that ran, not their variables; this is carried so the
/// step on the learning record reads as the step the door executed.
pub const DRAFT_VARIABLE: &str = "outbox";

/// Measured outcomes a regime must hold before the memory has an opinion
/// about drafting in it.
pub const RECOMMENDATION_SUPPORT: usize = 3;

/// Outcomes the memory keeps; the learning journal is the durable record.
const MEMORY_CAPACITY: usize = 256;

/// Drafts awaiting a follow-up reading. Bounded because a subject a detector
/// raises once and never again would otherwise be awaited forever.
const PENDING_CAPACITY: usize = 256;

/// What one cycle's routing did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgencyRound {
    /// Activated attention events the sweep produced.
    pub offered: usize,
    pub drafted: usize,
    pub duplicate: usize,
    /// Refused by the guard, by reason.
    pub withheld: BTreeMap<String, usize>,
    /// Outbox writes held for a retry, and abandoned after retries.
    pub deferred: usize,
    pub abandoned: usize,
    /// Refused before the guard (promotion, provenance, expiry), by reason.
    pub refused: BTreeMap<String, usize>,
    /// Retried drafts that landed this cycle.
    pub retried_drafted: usize,
    /// Drafts whose subject this sweep read again, learned from through
    /// observe-effect, attribute and update.
    pub learned: usize,
    /// Follow-ups a learn stage refused, by reason.
    pub learning_withheld: BTreeMap<String, usize>,
    /// Learnings that ran and whose record the journal refused.
    pub learning_unjournaled: usize,
    /// Drafts still awaiting a follow-up reading when the sweep ended.
    pub awaiting: usize,
    /// Awaited drafts dropped unobserved to make room, and retried drafts
    /// written without the reading a follow-up needs.
    pub unobserved: usize,
    /// Times the memory was asked before a draft, and what it said: a
    /// sequence with a positive measured effect in the regime, or nothing.
    pub asked: usize,
    pub recommended_draft: usize,
    pub recommended_nothing: usize,
}

impl AgencyRound {
    pub fn describe(&self) -> String {
        format!(
            "agency: {} offered, {} drafted ({} retried), {} duplicate, {} held for retry, {} \
             abandoned, withheld {:?}, refused {:?}; memory asked {} (draft {}, nothing {}); \
             {} learned, learning withheld {:?}, {} unjournaled, {} awaiting, {} unobserved; \
             drafts only, nothing sent",
            self.offered,
            self.drafted,
            self.retried_drafted,
            self.duplicate,
            self.deferred,
            self.abandoned,
            self.withheld,
            self.refused,
            self.asked,
            self.recommended_draft,
            self.recommended_nothing,
            self.learned,
            self.learning_withheld,
            self.learning_unjournaled,
            self.awaiting,
            self.unobserved
        )
    }

    fn tally(&mut self, outcome: Intake, retried: bool) {
        match outcome {
            Intake::Drafted { .. } => {
                self.drafted += 1;
                if retried {
                    self.retried_drafted += 1;
                    self.unobserved += 1;
                }
            }
            Intake::Duplicate { .. } => self.duplicate += 1,
            Intake::Withheld { reason } => *self.withheld.entry(reason).or_default() += 1,
            Intake::Deferred { .. } => self.deferred += 1,
            Intake::Abandoned { .. } => self.abandoned += 1,
        }
    }
}

/// Where the outbox is written: beside the platform's log when that is a file.
pub fn destination(
    value: Option<String>,
    platform_log: &qip_kernel::EventLogDestination,
) -> Result<JournalDestination> {
    match value {
        Some(raw) if raw.trim().is_empty() => Err(Error::invalid(format!(
            "{AGENCY_OUTBOX_VARIABLE} is set but blank; unset it to write beside the event log, \
             or name a file"
        ))),
        Some(raw) => Ok(JournalDestination::File(raw.into())),
        None => Ok(match platform_log {
            qip_kernel::EventLogDestination::InMemory => JournalDestination::InMemory,
            qip_kernel::EventLogDestination::File { path, .. } => {
                let mut name = path.as_os_str().to_owned();
                name.push(".outbox.jsonl");
                JournalDestination::File(name.into())
            }
        }),
    }
}

/// Where the learning journal is written: beside the outbox, as the outbox
/// is beside the event log.
pub fn learning_destination(outbox: &JournalDestination) -> JournalDestination {
    match outbox {
        JournalDestination::InMemory => JournalDestination::InMemory,
        JournalDestination::File(path) => {
            let mut name = path.as_os_str().to_owned();
            name.push(".learning.jsonl");
            JournalDestination::File(name.into())
        }
    }
}

/// What the memory kept and the policy was held to, for one follow-up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearnedDraft {
    pub outcome: ActionOutcome,
    pub attribution: EffectAttribution,
    /// `observed - counterfactual`, the figure the policy was held to.
    pub effect: Decimal,
    pub predicted_uplift_low: Decimal,
    pub predicted_uplift_high: Decimal,
    pub authority_before: Authority,
    pub authority_after: Authority,
}

/// One draft's follow-up, learned from or withheld, on the learning journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftLearning {
    pub draft_id: String,
    /// The signal the draft answered, and the later one that read its
    /// subject again.
    pub signal_id: String,
    pub follow_up_signal_id: String,
    pub subject: String,
    pub drafted_cycle: u64,
    pub observed_cycle: u64,
    /// The stages the learning passed. The first eight are seated, not run
    /// (module documentation); evidence begins at `ObserveEffect`.
    pub passed: Vec<String>,
    pub failed: Option<String>,
    pub learned: Option<LearnedDraft>,
    /// Why nothing was learned, when a stage refused the follow-up.
    pub withheld: Option<String>,
}

impl EventBody for DraftLearning {
    /// The topic the outbox and the agency journal use, for the reason
    /// `qip_agency::journal` gives; the producer keeps the three apart.
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

/// A draft awaiting the reading a later sweep takes of its subject.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingDraft {
    draft_id: String,
    signal_id: String,
    pathway: Pathway,
    cycle: u64,
    drafted_at: Timestamp,
    /// The subject's reading when drafted: the status quo the follow-up is
    /// measured against.
    reading: Decimal,
}

/// The regime a draft met, as the memory keys it: what was detected and
/// which pathway attention woke. Both enums, so the key set is bounded.
fn regime(class: SignalClass, pathway: Pathway) -> String {
    format!("{class:?}/{pathway:?}")
}

/// The run loop's handle on the agency door.
#[derive(Debug)]
pub struct AmbientAgency {
    intake: AmbientIntake,
    learning: EventLog,
    memory: ActionOutcomeMemory,
    policy: ActionPolicy,
    pending: BTreeMap<(SignalClass, String), PendingDraft>,
    /// Whether the channel-health series' help text has been registered.
    described: bool,
}

impl AmbientAgency {
    /// Open the door on `destination`, refusing an outbox whose chain breaks,
    /// with the learning journal beside it.
    pub fn open(destination: &JournalDestination) -> Result<Self> {
        let open = |d: &JournalDestination| match d {
            JournalDestination::InMemory => Ok(EventLog::in_memory()),
            JournalDestination::File(path) => EventLog::open(path),
        };
        let outbox = open(destination)?;
        let learning = open(&learning_destination(destination))?;
        Self::over_with(outbox, learning)
    }

    /// Open the door on a log the caller built (a test holds a small one),
    /// with an in-memory learning journal.
    pub fn over(log: EventLog) -> Result<Self> {
        Self::over_with(log, EventLog::in_memory())
    }

    /// Open the door on an outbox and a learning journal the caller built.
    pub fn over_with(outbox: EventLog, learning: EventLog) -> Result<Self> {
        let intake =
            AmbientIntake::open(IntakeConfig::new(ACTING_IDENTITY, PLAN_APPROVER), outbox)?;
        learning.verify_retained_chain().map_err(|sequence| {
            Error::denied(format!(
                "the learning journal's hash chain breaks at sequence {sequence}; move the file \
                 aside rather than extend it"
            ))
        })?;
        // A draft predicts no uplift and no side effect: it is a notice for
        // a person, and the policy is held to exactly that.
        let policy = ActionPolicy::new(
            Method::Communication,
            Decimal::ZERO,
            Decimal::ZERO,
            BTreeSet::new(),
        )?;
        Ok(Self {
            intake,
            learning,
            memory: ActionOutcomeMemory::with_capacity(MEMORY_CAPACITY)?,
            policy,
            pending: BTreeMap::new(),
            described: false,
        })
    }

    pub fn intake(&self) -> &AmbientIntake {
        &self.intake
    }

    pub fn intake_mut(&mut self) -> &mut AmbientIntake {
        &mut self.intake
    }

    /// The learning journal: one [`DraftLearning`] per follow-up.
    pub fn learning_log(&self) -> &EventLog {
        &self.learning
    }

    /// The outcomes this process has learned.
    pub fn memory(&self) -> &ActionOutcomeMemory {
        &self.memory
    }

    /// The drafting policy, and the authority it holds: shadow, always, since
    /// nothing on this path widens it.
    pub fn policy(&self) -> &ActionPolicy {
        &self.policy
    }

    /// Drafts awaiting a follow-up reading.
    pub fn awaiting(&self) -> usize {
        self.pending.len()
    }

    /// Route one sweep: retry what was held, learn from every awaited draft
    /// whose subject this sweep read again, then offer each activated
    /// attention event whose signal this sweep produced. `correlation` is the
    /// ambient journal's for the sweep; without a journal the draft is filed
    /// under a correlation of its own naming the cycle. The rate window is the
    /// cycle, which never goes backwards.
    ///
    /// The guard's channel-health verdicts and the drafts appended are
    /// recorded into `metrics` whether the sweep ends cleanly or not, so a
    /// refusal part-way through a sweep still reports what the guard had
    /// already decided (R12-OBS-7132).
    pub fn route(
        &mut self,
        cycle: u64,
        round: &ResearchRound,
        correlation: Option<&CorrelationId>,
        now: Timestamp,
        metrics: &Metrics,
    ) -> Result<AgencyRound> {
        let mut done = AgencyRound::default();
        let routed = self.route_into(cycle, round, correlation, now, &mut done);
        self.record_channel_health(metrics, done.drafted);
        routed.map(|()| done)
    }

    /// R12-OBS-7132: each health dimension the guard decided on the internal
    /// outbox channel, by outcome, and the drafts that landed. The verdicts
    /// are drained from the door, which took them where the guard decided;
    /// nothing here re-runs a check. Every label is a constant or an enum's
    /// literal, so the series are bounded at ten health keys and one draft
    /// key.
    fn record_channel_health(&mut self, metrics: &Metrics, drafted: usize) {
        if !self.described {
            metrics.describe(
                names::AGENCY_CHANNEL_HEALTH,
                "the action guard's verdict on each channel-health dimension of the agency \
                 door's drafts, by channel, dimension and outcome",
            );
            metrics.describe(
                names::AGENCY_DRAFTS_APPENDED,
                "drafts the agency door appended to its internal outbox, by channel",
            );
            self.described = true;
        }
        for ((dimension, outcome), times) in self.intake.take_channel_health() {
            metrics.increment(
                names::AGENCY_CHANNEL_HEALTH,
                labels([
                    ("channel", OUTBOX_CHANNEL),
                    ("dimension", dimension.as_str()),
                    ("outcome", outcome.as_str()),
                ]),
                times,
            );
        }
        if drafted > 0 {
            metrics.increment(
                names::AGENCY_DRAFTS_APPENDED,
                labels([("channel", OUTBOX_CHANNEL)]),
                u64::try_from(drafted).unwrap_or(u64::MAX),
            );
        }
    }

    fn route_into(
        &mut self,
        cycle: u64,
        round: &ResearchRound,
        correlation: Option<&CorrelationId>,
        now: Timestamp,
        done: &mut AgencyRound,
    ) -> Result<()> {
        for outcome in self.intake.retry(now, cycle)? {
            done.tally(outcome, true);
        }
        let own;
        let correlation = match correlation {
            Some(shared) => shared,
            None => {
                own = CorrelationId::from_string(format!("ambient-sweep-{cycle}"));
                &own
            }
        };
        self.observe(cycle, round, correlation, now, done);
        for event in &round.attention_events {
            if event.disposition != Disposition::Activated {
                continue;
            }
            done.offered += 1;
            let Some(signal) = round.signals.iter().find(|s| s.id() == event.signal_id) else {
                // A signal deferred in an earlier sweep: this round no longer
                // holds it, and a draft built without the signal would invent
                // its provenance.
                *done
                    .refused
                    .entry("signal not in this sweep".to_string())
                    .or_default() += 1;
                continue;
            };
            self.ask(signal, event, done)?;
            let admitted = AmbientOpportunity::advise(signal, event, correlation)
                .and_then(|advisory| self.intake.admit(advisory, PROMOTION_APPROVER, now, cycle));
            match admitted {
                Ok(outcome) => {
                    if let Intake::Drafted { draft_id, .. } = &outcome {
                        self.await_follow_up(draft_id.clone(), signal, event, cycle, now, done);
                    }
                    done.tally(outcome, false);
                }
                Err(error) => {
                    *done.refused.entry(error.message().to_string()).or_default() += 1;
                }
            }
        }
        done.awaiting = self.pending.len();
        Ok(())
    }

    /// R12-AGENCY-6108: ask the memory what worked in this regime for this
    /// audience before drafting, and put its answer on the cycle line. The
    /// answer does not gate the draft: the only effect this path measures is
    /// unidentified, and withholding a notice to a person on it would be
    /// acting on a correlation. What the ask proves is that the memory is
    /// read on the running path. The memory reads only records above zero
    /// confidence (ADR 0145) and this path files every outcome at zero, so
    /// the honest answer today is always "nothing"; an identified outcome
    /// would change the answer and still not the draft.
    fn ask(
        &self,
        signal: &AmbientSignal,
        event: &AttentionEvent,
        done: &mut AgencyRound,
    ) -> Result<()> {
        done.asked += 1;
        match self.memory.recommend_for(
            &regime(signal.class(), event.pathway),
            Some(OUTBOX_AUDIENCE),
            RECOMMENDATION_SUPPORT,
        )? {
            Some(_) => done.recommended_draft += 1,
            None => done.recommended_nothing += 1,
        }
        Ok(())
    }

    /// Hold a written draft until a later sweep reads its subject again.
    fn await_follow_up(
        &mut self,
        draft_id: String,
        signal: &AmbientSignal,
        event: &AttentionEvent,
        cycle: u64,
        now: Timestamp,
        done: &mut AgencyRound,
    ) {
        let key = (signal.class(), signal.subject().to_string());
        if !self.pending.contains_key(&key) && self.pending.len() >= PENDING_CAPACITY {
            // Full: the draft longest awaited leaves unobserved, on the
            // cycle line. A subject that stops being raised is never read
            // again, and holding it would hold every later draft out.
            if let Some(oldest) = self
                .pending
                .iter()
                .min_by_key(|(_, p)| p.cycle)
                .map(|(k, _)| k.clone())
            {
                self.pending.remove(&oldest);
                done.unobserved += 1;
            }
        }
        if self
            .pending
            .insert(
                key,
                PendingDraft {
                    draft_id,
                    signal_id: signal.id().to_string(),
                    pathway: event.pathway,
                    cycle,
                    drafted_at: now,
                    reading: signal.observed_deviation(),
                },
            )
            .is_some()
        {
            // Two drafts on one subject in one sweep: the later reading is
            // the status quo, and the earlier draft goes unobserved.
            done.unobserved += 1;
        }
    }

    /// Learn from every awaited draft whose subject a signal in this sweep
    /// reads again. A draft's own sweep never observes it: the reading it
    /// was drafted on is not a follow-up.
    fn observe(
        &mut self,
        cycle: u64,
        round: &ResearchRound,
        correlation: &CorrelationId,
        now: Timestamp,
        done: &mut AgencyRound,
    ) {
        for signal in &round.signals {
            let key = (signal.class(), signal.subject().to_string());
            if self.pending.get(&key).is_none_or(|p| p.cycle >= cycle) {
                continue;
            }
            let Some(pending) = self.pending.remove(&key) else {
                continue;
            };
            let learning = self.learn_from(&pending, signal, cycle, now);
            match &learning.withheld {
                None => done.learned += 1,
                Some(reason) => {
                    *done.learning_withheld.entry(reason.clone()).or_default() += 1;
                }
            }
            if self.journal(learning, correlation, now).is_err() {
                done.learning_unjournaled += 1;
            }
        }
    }

    /// Run observe-effect, attribute and update on one follow-up, and say
    /// what happened whether or not a stage refused it.
    fn learn_from(
        &mut self,
        pending: &PendingDraft,
        follow_up: &AmbientSignal,
        cycle: u64,
        now: Timestamp,
    ) -> DraftLearning {
        let mut log = StageLog::new();
        let before = self.policy.authority();
        let result = self.learn_stages(&mut log, pending, follow_up, now);
        let passed = log.passed().iter().map(|s| format!("{s:?}")).collect();
        let failed = log.failed().map(|s| format!("{s:?}"));
        let (learned, withheld) = match result {
            Ok((outcome, attribution, effect, after)) => {
                let (low, high) = self.policy.predicted_uplift();
                (
                    Some(LearnedDraft {
                        outcome,
                        attribution,
                        effect,
                        predicted_uplift_low: low,
                        predicted_uplift_high: high,
                        authority_before: before,
                        authority_after: after,
                    }),
                    None,
                )
            }
            Err(refusal) => (None, Some(refusal.message().to_string())),
        };
        DraftLearning {
            draft_id: pending.draft_id.clone(),
            signal_id: pending.signal_id.clone(),
            follow_up_signal_id: follow_up.id().to_string(),
            subject: follow_up.subject().to_string(),
            drafted_cycle: pending.cycle,
            observed_cycle: cycle,
            passed,
            failed,
            learned,
            withheld,
        }
    }

    fn learn_stages(
        &mut self,
        log: &mut StageLog,
        pending: &PendingDraft,
        follow_up: &AmbientSignal,
        now: Timestamp,
    ) -> Result<(ActionOutcome, EffectAttribution, Decimal, Authority)> {
        // Seated, not run: see the module documentation.
        for stage in [
            Stage::Observe,
            Stage::Predict,
            Stage::Specify,
            Stage::IdentifyLevers,
            Stage::Generate,
            Stage::Simulate,
            Stage::Gate,
            Stage::Act,
        ] {
            log.pass(stage)?;
        }
        let delay_ms = u64::try_from(now.since(pending.drafted_at).as_millis()).map_err(|_| {
            Error::invalid(
                "the follow-up sweep is dated before the draft; no delay can be stated for it",
            )
        })?;
        let observed = follow_up.observed_deviation();
        let effect = observed
            .checked_sub(pending.reading)
            .ok_or_else(|| Error::numeric("the subject's move since the draft overflowed"))?;
        let subject = follow_up.subject();
        let step = Step {
            tool: DRAFT_TOOL.to_string(),
            variable: DRAFT_VARIABLE.to_string(),
            exposure: Decimal::ZERO,
        };
        let draft = ActionOutcomeDraft {
            intervention_context: Some(format!(
                "ambient draft {} on signal {} at cycle {}, read again by signal {}",
                pending.draft_id,
                pending.signal_id,
                pending.cycle,
                follow_up.id()
            )),
            environment_state: Some(regime(follow_up.class(), pending.pathway)),
            audience: Some(OUTBOX_AUDIENCE.to_string()),
            delay_secs: Some(delay_ms / 1000),
            action_sequence: Some(vec![DRAFT_TOOL.to_string()]),
            causal_hypothesis: Some(format!(
                "a draft written for a person to read moves nothing on `{subject}` by itself; \
                 the figure is the subject's reading at the following sweep against its reading \
                 when drafted, filed so the draft is held to a null effect rather than credited \
                 with whatever the subject did"
            )),
            predicted_effect: Some(Decimal::ZERO),
            actual_observations: Some(observed),
            attribution_confidence: Some(Decimal::ZERO),
            side_effects: Some(Vec::new()),
            no_action_counterfactual: Some(pending.reading),
            observed: Some(Observed::Measured),
        };
        // The statistics/money crossing: the reading is a `Decimal`, the
        // attribution's effect an `f64` statistic.
        let attribution = EffectAttribution {
            goal_id: format!("{}:goal", pending.draft_id),
            intervention_id: format!("{}:draft", pending.draft_id),
            action_id: format!("{}:outbox@{}", pending.draft_id, pending.cycle),
            action_class: Method::Communication,
            identification: Identification::NotIdentified,
            observations: vec![HorizonObservation {
                horizon_ms: delay_ms,
                value: effect.to_f64(),
            }],
            counterfactual_method: "status quo: the subject's reading at the sweep that drafted, \
                                    carried forward; no control was assigned, since ADR 0114 \
                                    refuses an experiment on the people a draft reaches"
                .to_string(),
            effect: Effect {
                mean: effect.to_f64(),
                std_dev: 0.0,
            },
            causal_chain: vec![
                format!("{DRAFT_TOOL}@{DRAFT_VARIABLE}"),
                format!("{OUTBOX_AUDIENCE} reads the draft, if anyone does"),
                subject.to_string(),
            ],
            confounders: vec![
                "every other cause of the subject's reading; the draft controls for none of them"
                    .to_string(),
            ],
            competing_explanations: vec![
                "the subject moved on its own; nothing shows a person read the draft, let alone \
                 acted on it"
                    .to_string(),
            ],
            side_effects: vec!["none observed".to_string()],
            identifiability: 0.0,
            resulting_updates: vec![
                "the Communication policy is held to a predicted uplift of zero, and the outcome \
                 is kept in the action outcome memory as measured and not identified"
                    .to_string(),
            ],
        };
        let outcome = draft.clone().build()?;
        let after = engine::learn(
            log,
            std::slice::from_ref(&step),
            &mut self.policy,
            &mut self.memory,
            draft,
            &attribution,
        )?;
        Ok((outcome, attribution, effect, after))
    }

    fn journal(
        &mut self,
        learning: DraftLearning,
        correlation: &CorrelationId,
        now: Timestamp,
    ) -> Result<()> {
        let id = EventId::from_string(format!(
            "evt-agency-learning-{}-{}",
            learning.draft_id, learning.observed_cycle
        ));
        let envelope = Envelope::new(
            id,
            now,
            now,
            Lineage::root(correlation.clone(), LEARNING_ORIGIN),
            learning,
        )
        .erase()?;
        self.learning.append(&envelope)?;
        Ok(())
    }
}

/// Every learning record in `log`, in log order. One that does not decode is
/// refused, not skipped.
pub fn learnings(log: &EventLog) -> Result<Vec<DraftLearning>> {
    let mut out = Vec::new();
    for event in log.events() {
        if event.topic != Topic::ComplianceEvaluated || event.lineage.producer != LEARNING_ORIGIN {
            continue;
        }
        out.push(event.decode::<DraftLearning>()?.body);
    }
    Ok(out)
}
