//! The one production adapter `qip-agency` may have: a drafting adapter whose
//! only effect is appending a draft artefact to an internal outbox on the
//! hash-chained log (ADR 0114 Amendment 1, owner decision D5).
//!
//! It calls no outside system and holds no credential. "Executed" for a draft
//! means a draft was written, which is exactly what the record says; a person
//! reads the draft and acts, if at all, through their own credential outside
//! this platform. Publishing, outreach, pricing and operational change stay
//! refused (ADR 0114 D8).
//!
//! # Why this file names nothing ambient
//!
//! The adapter is the end of the road, so it must not be possible to hand it
//! anything but a [`DraftContext`] of plain text and a [`Step`]. Whatever
//! produced the text — a detector, an operator — is the caller's business and
//! was judged by the guard before this adapter was reached. A test in
//! `tests/ambient_intake.rs` reads this file and fails if an ambient type is
//! ever named here, so the way from detection to adapter stays through
//! promotion and the guard and not around them.
//!
//! # Topic
//!
//! `compliance.evaluated` under the producer [`OUTBOX_ORIGIN`], for the reason
//! [`crate::journal`] gives: a new topic would have to enter the fabric's
//! bindings and schema lock, which are another lane's, and the Decide group's
//! retention class refuses an append when the log is full rather than evicting
//! a record, so a draft is never silently lost to make room.

use crate::engine::Adapter;
use crate::goal::GoalSpec;
use crate::plan::Step;
use qip_contracts::action::{ExecutionRecord, GoalApproval, PlanApproval, ToolAudit};
use qip_core::lineage::Lineage;
use qip_core::{CorrelationId, Error, EventId, Timestamp};
use qip_events::{Envelope, EventBody, EventLog, Topic};
use serde::{Deserialize, Serialize};

/// The producer every outbox record carries.
pub const OUTBOX_ORIGIN: &str = "agency/outbox";

/// The tool the drafting adapter is registered as.
pub const DRAFT_TOOL: &str = "outbox-draft";

/// The one channel that tool may use: the internal outbox.
pub const OUTBOX_CHANNEL: &str = "internal-outbox";

/// One draft, as written to the outbox. Every field is plain data so the
/// record decodes without this crate's other types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboxDraft {
    pub draft_id: String,
    /// What the draft says. Never sent anywhere.
    pub content: String,
    /// Which signal the draft is about, and what kind of detection it was.
    pub origin_signal: String,
    pub origin_class: String,
    pub origin_pathway: String,
    /// What ran the detector (`schedule:<name>` or `event:<name>`).
    pub source: String,
    pub detected_at: Timestamp,
    pub expiry: Timestamp,
    /// The correlation the detection travelled under, so the draft joins the
    /// ambient journal's chain.
    pub correlation_id: String,
    pub evidence_ids: Vec<String>,
    pub priority_bp: u32,
    /// Who promoted the output, and on what evidence.
    pub promoted_by: String,
    pub promotion_evidence: Vec<String>,
    /// GOV-6124's three records for this draft.
    pub goal_approval: GoalApproval,
    pub plan_approval: PlanApproval,
    pub execution: ExecutionRecord,
    /// The registry decisions this draft was charged under; resumed on start
    /// so a restart does not refill the budget.
    pub tool_decisions: Vec<ToolAudit>,
    pub rate_window: u64,
    /// Which attempt wrote it (1 for the first).
    pub attempt: u32,
    /// The sub-goal of the door's standing goal this draft served, as
    /// `GoalSpec::decompose` validated it (R12-AGENCY-3002). `None` only on
    /// a record written before the door decomposed its goal.
    #[serde(default)]
    pub sub_goal: Option<GoalSpec>,
}

impl EventBody for OutboxDraft {
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

/// What the adapter writes next, staged by the intake immediately before the
/// guard-cleared call.
#[derive(Debug, Clone)]
pub struct DraftContext {
    pub draft: OutboxDraft,
    pub recorded_at: Timestamp,
}

/// The drafting adapter. Its whole effect is [`EventLog::append`].
#[derive(Debug)]
pub struct DraftingAdapter {
    log: EventLog,
    staged: Option<DraftContext>,
}

impl DraftingAdapter {
    /// Open the outbox on `log`, refusing one whose chain does not verify: a
    /// draft appended to an edited log would extend a record nobody can trust.
    pub fn open(log: EventLog) -> Result<Self, Error> {
        log.verify_retained_chain().map_err(|sequence| {
            Error::denied(format!(
                "the outbox's hash chain breaks at sequence {sequence}; move the file aside \
                 rather than extend it"
            ))
        })?;
        Ok(Self { log, staged: None })
    }

    pub fn log(&self) -> &EventLog {
        &self.log
    }

    /// Whether a draft with this id is already written.
    pub fn holds(&self, draft_id: &str) -> bool {
        let id = event_id(draft_id);
        self.log.events().any(|e| e.event_id == id)
    }

    /// Stage the next draft. The only way `call` has anything to write.
    pub fn stage(&mut self, context: DraftContext) {
        self.staged = Some(context);
    }
}

fn event_id(draft_id: &str) -> EventId {
    EventId::from_string(format!("evt-outbox-{draft_id}"))
}

impl Adapter for DraftingAdapter {
    fn call(&mut self, step: &Step) -> Result<(), Error> {
        // Consumed on every call, success or failure: a stale staging must
        // never be written by the next, unrelated step.
        let context = self.staged.take().ok_or_else(|| {
            Error::denied("the outbox was called with no draft staged; nothing is written")
        })?;
        if step.tool != DRAFT_TOOL {
            return Err(Error::denied(format!(
                "the outbox writes drafts only; tool `{}` is not `{DRAFT_TOOL}`",
                step.tool
            )));
        }
        let draft = context.draft;
        let lineage = Lineage::root(
            CorrelationId::from_string(draft.correlation_id.clone()),
            OUTBOX_ORIGIN,
        );
        let occurred_at = draft.detected_at;
        let envelope = Envelope::new(
            event_id(&draft.draft_id),
            occurred_at,
            context.recorded_at,
            lineage,
            draft,
        )
        .erase()?;
        self.log.append(&envelope)?;
        Ok(())
    }
}

/// Every outbox draft in `log`, in log order. One that does not decode is
/// refused, not skipped.
pub fn drafts(log: &EventLog) -> Result<Vec<OutboxDraft>, Error> {
    let mut out = Vec::new();
    for event in log.events() {
        if event.topic != Topic::ComplianceEvaluated || event.lineage.producer != OUTBOX_ORIGIN {
            continue;
        }
        out.push(event.decode::<OutboxDraft>()?.body);
    }
    Ok(out)
}
