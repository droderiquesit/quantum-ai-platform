//! The single sanctioned door from the ambient mesh to the agency
//! (R12-AGENCY-7029, R12-COVERAGE-7190, blueprint §9.5 and §24).
//!
//! # The failure this prevents
//!
//! Detection triggering action directly. Until this module the ambient mesh
//! (`qip_contracts::ambient`) ended in an [`Advisory`] and the agency began at
//! a `Request` an operator wrote; nothing joined them, so the claim that "the
//! authority and conduct gates sit between detection and execution" was backed
//! by a text search over order-path crates and by no running path at all.
//!
//! # The road, and the only road
//!
//! ```text
//! AmbientSignal + AttentionEvent
//!   -> AmbientOpportunity::advise      Advisory<AmbientOpportunity>
//!   -> AmbientIntake::admit            qip_contracts::ambient::promote
//!   -> AmbientIntake::submit           Promoted<AmbientOpportunity>
//!   -> ActionGuard::preflight          justification, approvals, tool registry,
//!                                      seven conduct checks, coordinated voices
//!   -> ActionGuard::authorise          rate and budget charged and audited
//!   -> DraftingAdapter::call           append to the internal outbox
//! ```
//!
//! [`AmbientIntake::submit`] accepts a `Promoted` and nothing else, and
//! `Promoted` is built only by `promote`, which wants evidence, a passing
//! evaluation, a named approver and the deterministic controls holding. The
//! approver `promote` names becomes the *goal approver* the guard checks, and
//! the guard refuses an approver who is the acting identity.
//!
//! # What the door cannot do
//!
//! Act. The only adapter is [`DraftingAdapter`], whose whole effect is an
//! append to the outbox (ADR 0114 Amendment 1, owner decision D5): the tool is
//! registered on the one channel `internal-outbox`, with the scope
//! `outbox:append`. Publishing, outreach, pricing and operational change stay
//! refused. The deterministic-controls clause of the promotion is held true
//! here for one stated reason and no other: a draft has no order path
//! to run pre-trade controls over, so the clause is vacuous, not waived.
//!
//! # Failure and retry
//!
//! * A refusal by the guard (conduct, registry, approvals, justification) is
//!   **terminal**: [`Intake::Withheld`], nothing written, never retried — the
//!   same opportunity would be refused the same way, and retrying it would
//!   only spend the audit.
//! * A failed outbox write (the log full, the disk refusing) is **transient**:
//!   [`Intake::Deferred`], held in a bounded queue and offered again by
//!   [`AmbientIntake::retry`] up to `max_attempts`, then dropped on the record
//!   as [`Intake::Abandoned`] rather than retried for ever. Every attempt that
//!   reached the adapter is billed to the registry, because it ran.
//! * A draft is idempotent on its signal id: a signal already drafted returns
//!   [`Intake::Duplicate`] and charges nothing, so a retry after a write that
//!   in fact landed cannot write twice.
//! * Retention is bounded: the retry queue holds `retry_capacity` entries and
//!   a full queue abandons the new one on the record.

use crate::conduct_brain::{
    CommunicationDraft, ConductPolicy, Disclosure, EvidenceCitation, MANDATORY_RULES,
};
use crate::goal::{GoalClass, GoalSpec, GoalSpecDraft};
use crate::guard::{
    ActionGuard, ApprovalDecl, ChannelHealth, GuardPolicy, HealthDimension, HealthOutcome,
    PassDecl, Voice, voice_fingerprint,
};
use crate::outbox::{
    DRAFT_TOOL, DraftContext, DraftingAdapter, OUTBOX_CHANNEL, OutboxDraft, drafts,
};
use crate::plan::Step;
use qip_contracts::action::{Ground, Method, ToolRegistration};
use qip_contracts::ambient::{
    Advisory, AmbientSignal, AttentionEvent, Disposition, Pathway, Promoted, PromotionCase,
    SignalClass, Trigger, promote,
};
use qip_core::{CorrelationId, Decimal, Error, EventId, Timestamp};
use qip_events::EventLog;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The source id the draft's citations carry; the conduct gate's provenance
/// check admits only sources a policy names, and the policy names this one.
pub const AMBIENT_SOURCE: &str = "ambient-mesh";

/// The audience an outbox draft addresses.
pub const OUTBOX_AUDIENCE: &str = "research-desk";

/// The one disclosure kind every outbox draft must carry, and the gate
/// requires (R12-AGENCY-1059): that it is an internal draft nothing sends.
/// A reader of the outbox sees on each record that it was never a
/// publication, rather than inferring it from the channel name.
pub const OUTBOX_DISCLOSURE: &str = "internal_draft";

/// The text of that disclosure.
pub const OUTBOX_DISCLOSURE_TEXT: &str =
    "Internal draft for the research desk; nothing is sent outside the platform.";

/// The variable the drafting step is "about"; carried only so a `Step` is whole.
const OUTBOX_VARIABLE: &str = "outbox";

/// What one draft costs the door's budget, in whole registry units: the
/// price the guard charges it at and the budget its sub-goal declares.
const DRAFT_COST: Decimal = Decimal::ONE;

/// What ambient attention hands the agency: the facts of one activated
/// detection, with the provenance the draft must keep. Wrapped in an
/// [`Advisory`] by [`Self::advise`]; it is never acted on in this form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbientOpportunity {
    pub signal_id: String,
    pub class: SignalClass,
    pub pathway: Pathway,
    pub subject: String,
    /// What ran the detector: `schedule:<name>` or `event:<name>`.
    pub source: String,
    pub detected_at: Timestamp,
    /// When attention decided to wake the pathway.
    pub attended_at: Timestamp,
    pub expiry: Timestamp,
    /// The correlation the detection is journaled under.
    pub correlation_id: String,
    pub evidence_ids: Vec<String>,
    /// The router's priority for the activation, in basis points.
    pub priority_bp: u32,
}

impl AmbientOpportunity {
    /// Join a signal to the attention event that answered it. Refuses an
    /// event for another signal, one that did not activate (a deferred, shed
    /// or expired signal woke nothing and is not an opportunity), and a blank
    /// correlation — provenance that cannot be joined back is provenance
    /// lost.
    pub fn advise(
        signal: &AmbientSignal,
        event: &AttentionEvent,
        correlation: &CorrelationId,
    ) -> Result<Advisory<Self>, Error> {
        if event.signal_id != signal.id() {
            return Err(Error::invalid(format!(
                "attention event {} answers signal {}, not {}; an opportunity joins a signal \
                 to its own decision",
                event.id,
                event.signal_id,
                signal.id()
            )));
        }
        if event.disposition != Disposition::Activated {
            return Err(Error::denied(format!(
                "signal {} was {:?}, not activated; it woke nothing and proposes nothing",
                signal.id(),
                event.disposition
            )));
        }
        let source = match signal.trigger() {
            Trigger::Schedule(name) => format!("schedule:{name}"),
            Trigger::Event(name) => format!("event:{name}"),
        };
        let opportunity = Self {
            signal_id: signal.id().to_string(),
            class: signal.class(),
            pathway: event.pathway,
            subject: signal.subject().to_string(),
            source,
            detected_at: signal.detected_at(),
            attended_at: event.at,
            expiry: signal.expiry(),
            correlation_id: correlation.as_str().to_string(),
            evidence_ids: signal.evidence_ids().to_vec(),
            priority_bp: event.priority_bp,
        };
        opportunity.require_provenance()?;
        Ok(Advisory::new(opportunity, event.feedback_link.clone()))
    }

    /// Every provenance field present. Re-run at the door because the fields
    /// are public: a value assembled by hand is checked as strictly as one
    /// [`Self::advise`] built.
    pub fn require_provenance(&self) -> Result<(), Error> {
        let blank = |s: &str| s.trim().is_empty();
        let missing = if blank(&self.signal_id) {
            Some("a signal id")
        } else if blank(&self.source) || self.source.ends_with(':') {
            Some("the detector that raised it")
        } else if blank(&self.correlation_id) {
            Some("a correlation id")
        } else if blank(&self.subject) {
            Some("a subject")
        } else if self.evidence_ids.is_empty() || self.evidence_ids.iter().any(|e| blank(e)) {
            Some("evidence lineage")
        } else if self.priority_bp > qip_contracts::ambient::MAX_SEVERITY_BP {
            Some("a priority within 0..=10000 basis points")
        } else if self.expiry < self.detected_at {
            Some("an expiry at or after detection")
        } else {
            None
        };
        match missing {
            Some(what) => Err(Error::invalid(format!(
                "ambient opportunity from signal `{}` lacks {what}; a draft nobody can trace \
                 back to its detection is refused",
                self.signal_id
            ))),
            None => Ok(()),
        }
    }

    fn draft_id(&self) -> String {
        format!("draft-{}", self.signal_id)
    }
}

/// Standing configuration of the door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntakeConfig {
    /// The identity every draft is sent under and the goal acts as.
    pub acting_identity: String,
    /// Approves the plan of every draft. Must not be the acting identity;
    /// refused when the door is opened.
    pub plan_approver: String,
    pub jurisdiction: String,
    /// Drafts per rate window the registry admits.
    pub rate_per_window: u64,
    /// Whole units the registry may charge in total, one per draft.
    pub budget_units: u64,
    /// Attempts at a failing outbox write before a draft is abandoned.
    pub max_attempts: u32,
    /// Drafts held awaiting a retry.
    pub retry_capacity: usize,
}

impl IntakeConfig {
    pub fn new(acting_identity: impl Into<String>, plan_approver: impl Into<String>) -> Self {
        Self {
            acting_identity: acting_identity.into(),
            plan_approver: plan_approver.into(),
            jurisdiction: "INTERNAL".to_string(),
            rate_per_window: 16,
            budget_units: 100_000,
            max_attempts: 3,
            retry_capacity: 64,
        }
    }

    fn validate(&self) -> Result<(), Error> {
        for (name, value) in [
            ("acting_identity", &self.acting_identity),
            ("plan_approver", &self.plan_approver),
            ("jurisdiction", &self.jurisdiction),
        ] {
            if value.trim().is_empty() {
                return Err(Error::invalid(format!("intake `{name}` is blank")));
            }
        }
        if self.acting_identity == self.plan_approver {
            return Err(Error::denied(
                "the plan approver is the acting identity; an identity approving its own draft \
                 is one record wearing two names",
            ));
        }
        if self.rate_per_window == 0 || self.budget_units == 0 {
            return Err(Error::invalid(
                "intake rate and budget must be above zero; a zero limit is a door that never opens",
            ));
        }
        if self.max_attempts == 0 || self.retry_capacity == 0 {
            return Err(Error::invalid(
                "intake max_attempts and retry_capacity must be above zero",
            ));
        }
        Ok(())
    }
}

/// What the door did with one opportunity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intake {
    /// Cleared every gate and written to the outbox.
    Drafted { draft_id: String, event_id: EventId },
    /// Already drafted; nothing charged, nothing written.
    Duplicate { draft_id: String },
    /// The guard refused. Terminal: nothing written, never retried.
    Withheld { reason: String },
    /// The guard cleared and the outbox write failed; held for a retry.
    Deferred { reason: String, attempts: u32 },
    /// Retries exhausted or the queue full; dropped on the record.
    Abandoned { reason: String, attempts: u32 },
}

#[derive(Debug, Clone)]
struct Pending {
    promoted: Promoted<AmbientOpportunity>,
    attempts: u32,
}

/// The door. Holds one guard for the life of the process, so the registry's
/// rate, budget and revocations hold across every opportunity.
#[derive(Debug)]
pub struct AmbientIntake {
    guard: ActionGuard,
    adapter: DraftingAdapter,
    goal: GoalSpec,
    config: IntakeConfig,
    drafted: BTreeSet<String>,
    retry: VecDeque<Pending>,
    /// Channel-health verdicts not yet taken; see
    /// [`Self::take_channel_health`].
    health: BTreeMap<(HealthDimension, HealthOutcome), u64>,
}

impl AmbientIntake {
    /// Build the guard and resume from the outbox: every draft already
    /// written is remembered (idempotence survives a restart) and every tool
    /// decision it was charged under is re-applied (a restart does not refill
    /// the budget).
    pub fn open(config: IntakeConfig, outbox: EventLog) -> Result<Self, Error> {
        config.validate()?;
        let policy = GuardPolicy {
            conduct: ConductPolicy {
                authorized_sources: vec![AMBIENT_SOURCE.to_string()],
                authorized_identities: vec![config.acting_identity.clone()],
                required_disclosures: vec![OUTBOX_DISCLOSURE.to_string()],
                channel_policies: BTreeMap::from([(
                    OUTBOX_CHANNEL.to_string(),
                    vec![OUTBOX_AUDIENCE.to_string()],
                )]),
                permitted_jurisdictions: vec![config.jurisdiction.clone()],
                conduct_rules: MANDATORY_RULES.iter().map(|r| (*r).to_string()).collect(),
            },
            tools: vec![ToolRegistration {
                tool: DRAFT_TOOL.to_string(),
                acting_identity: config.acting_identity.clone(),
                permission_scope: BTreeSet::from(["outbox:append".to_string()]),
                disclosure_policy: "internal draft; nothing is sent outside the platform"
                    .to_string(),
                channels: BTreeSet::from([OUTBOX_CHANNEL.to_string()]),
                max_actions_per_window: config.rate_per_window,
                budget_units: config.budget_units,
                revocation_path: format!("revoke tool `{DRAFT_TOOL}` on the intake guard"),
                provenance: "in-tree: qip-agency DraftingAdapter appends to the local outbox; \
                             no network call and no dependency beyond the workspace's"
                    .to_string(),
                sandbox_tests: BTreeSet::from([
                    "ambient_intake::an_activated_detection_is_promoted_guarded_and_drafted_with_its_provenance"
                        .to_string(),
                ]),
                // One draft is one budget unit and one local append.
                cost_latency: qip_contracts::action::CostLatency {
                    cost_units: 1,
                    latency_ms: 100,
                },
                allowed_callers: BTreeSet::from([config.acting_identity.clone()]),
            }],
        };
        let mut guard = ActionGuard::from_policy(policy)?;
        let adapter = DraftingAdapter::open(outbox)?;
        let mut drafted = BTreeSet::new();
        let mut decisions = Vec::new();
        let mut voices = Vec::new();
        for written in drafts(adapter.log())? {
            drafted.insert(written.origin_signal.clone());
            decisions.extend(written.tool_decisions);
            voices.push(Voice {
                tool: DRAFT_TOOL.to_string(),
                identity: config.acting_identity.clone(),
                fingerprint: voice_fingerprint(&written.content),
                // Kept within the guard's bound, and otherwise left out so
                // the post-release check reports it unreadable rather than
                // holding an unbounded text (R12-GOV-3030).
                content: (written.content.len() <= crate::guard::MAX_RELEASED_CONTENT_BYTES)
                    .then(|| written.content.clone()),
            });
        }
        guard.resume(decisions)?;
        guard.resume_voices(voices)?;
        let goal = GoalSpecDraft {
            class: Some(GoalClass::Research),
            target_state: Some(
                "an ambient opportunity is drafted to the internal outbox for a person to read"
                    .to_string(),
            ),
            entities_affected: Some(vec![OUTBOX_AUDIENCE.to_string()]),
            time_horizon_secs: Some(3_600),
            success_metric: Some("drafts written to the outbox".to_string()),
            acceptable_uncertainty: Some(Decimal::ONE),
            budget: Some(Decimal::from_int(
                i64::try_from(config.budget_units).map_err(|_| {
                    Error::numeric("the intake budget does not fit a signed unit count")
                })?,
            )),
            risk_envelope: Some(Decimal::ZERO),
            jurisdictions: Some(vec![config.jurisdiction.clone()]),
            acting_identity: Some(config.acting_identity.clone()),
            prohibited_side_effects: Some(vec![
                "external_send".to_string(),
                "public_statement".to_string(),
                "order_submission".to_string(),
            ]),
            prohibited_methods: Some(vec![
                "publish".to_string(),
                "outreach".to_string(),
                "pricing".to_string(),
            ]),
            stop_conditions: Some(vec!["the outbox cannot be written".to_string()]),
            // The door's only effect is a draft in the internal outbox, which
            // sends nothing, so a stop leaves nothing half-applied outside it.
            // What a stop unwinds is the retry queue: every held draft is
            // dropped on the record (`AmbientIntake::revoke`).
            rollback_conditions: Some(vec![
                "on a stop, drop every draft held for a retry on the record; written drafts stay \
                 in the internal outbox, which sends nothing"
                    .to_string(),
            ]),
        }
        .build()?;
        Ok(Self {
            guard,
            adapter,
            goal,
            config,
            drafted,
            retry: VecDeque::new(),
            health: BTreeMap::new(),
        })
    }

    pub fn guard(&self) -> &ActionGuard {
        &self.guard
    }

    /// The channel-health verdicts since the last call, counted by dimension
    /// and outcome, and cleared (R12-OBS-7132). The channel is
    /// [`OUTBOX_CHANNEL`], the only one this door drafts on. Every
    /// preflight the guard ran contributes the dimensions it reached; every
    /// adapter call contributes [`HealthDimension::Reach`]. At most ten
    /// keys, so a caller that never drains it holds a bounded map.
    pub fn take_channel_health(&mut self) -> BTreeMap<(HealthDimension, HealthOutcome), u64> {
        std::mem::take(&mut self.health)
    }

    fn note_health(&mut self, dimension: HealthDimension, outcome: HealthOutcome) {
        *self.health.entry((dimension, outcome)).or_default() += 1;
    }

    pub fn outbox(&self) -> &EventLog {
        self.adapter.log()
    }

    /// Drafts awaiting a retry.
    pub fn pending(&self) -> usize {
        self.retry.len()
    }

    /// Revoke the drafting tool: the next draft is refused and audited, and
    /// every draft held for a retry is dropped on the record now, returned
    /// as [`Intake::Abandoned`].
    ///
    /// A held draft has already cleared the guard once. Left in the queue,
    /// it waits for a retry that the revocation refuses anyway. Worse, a
    /// later [`Self::reauthorise`] would send it, on an authority given
    /// before the stop. A stop that leaves pending work queued behind it
    /// stops only new work.
    pub fn revoke(&mut self) -> Result<Vec<Intake>, Error> {
        self.guard.revoke(DRAFT_TOOL)?;
        Ok(self
            .retry
            .drain(..)
            .map(|pending| Intake::Abandoned {
                reason: "the drafting tool was revoked while this draft was held for a retry"
                    .to_string(),
                attempts: pending.attempts,
            })
            .collect())
    }

    /// Restore the drafting tool on `operator`'s authority. Drafts dropped at
    /// the revocation stay dropped: an opportunity worth drafting again is
    /// offered again.
    pub fn reauthorise(&mut self, operator: &str) -> Result<(), Error> {
        self.guard.reauthorise(DRAFT_TOOL, operator)
    }

    /// Promote an advisory on `approver`'s authority and submit it. The
    /// promotion's evidence is the signal's own evidence lineage and its
    /// evaluation is the router's: an activated, unexpired signal. A refused
    /// promotion consumes the advisory, as `promote` documents.
    pub fn admit(
        &mut self,
        advisory: Advisory<AmbientOpportunity>,
        approver: &str,
        now: Timestamp,
        window: u64,
    ) -> Result<Intake, Error> {
        let view = advisory.view();
        let case = PromotionCase {
            evidence: view.evidence_ids.clone(),
            evaluation_passed: now <= view.expiry,
            approver: approver.to_string(),
            deterministic_controls_held: true,
        };
        let promoted = promote(advisory, &case)?;
        self.submit(promoted, now, window)
    }

    /// The door itself. A `Promoted` and nothing else.
    pub fn submit(
        &mut self,
        promoted: Promoted<AmbientOpportunity>,
        now: Timestamp,
        window: u64,
    ) -> Result<Intake, Error> {
        promoted.clone().into_inner().require_provenance()?;
        if now > promoted.clone().into_inner().expiry {
            return Err(Error::denied(
                "the signal expired before it reached the agency; an expired detection proposes \
                 nothing",
            ));
        }
        self.attempt(
            Pending {
                promoted,
                attempts: 0,
            },
            now,
            window,
        )
    }

    /// Offer every held draft once more. Returns what became of each.
    pub fn retry(&mut self, now: Timestamp, window: u64) -> Result<Vec<Intake>, Error> {
        let held: Vec<Pending> = self.retry.drain(..).collect();
        let mut out = Vec::with_capacity(held.len());
        for pending in held {
            out.push(self.attempt(pending, now, window)?);
        }
        Ok(out)
    }

    fn attempt(
        &mut self,
        mut pending: Pending,
        now: Timestamp,
        window: u64,
    ) -> Result<Intake, Error> {
        let opportunity = pending.promoted.clone().into_inner();
        let draft_id = opportunity.draft_id();
        if self.drafted.contains(&opportunity.signal_id) || self.adapter.holds(&draft_id) {
            self.drafted.insert(opportunity.signal_id.clone());
            return Ok(Intake::Duplicate { draft_id });
        }
        let sub_goal = match self.sub_goal(&opportunity, now) {
            Ok(goal) => goal,
            Err(refusal) => {
                return Ok(Intake::Withheld {
                    reason: format!(
                        "the door's standing goal does not decompose into a sub-goal for this \
                         draft: {}",
                        refusal.message()
                    ),
                });
            }
        };
        let step = Step {
            tool: DRAFT_TOOL.to_string(),
            variable: OUTBOX_VARIABLE.to_string(),
            exposure: Decimal::ZERO,
        };
        let communication = CommunicationDraft {
            id: draft_id.clone(),
            content: format!(
                "Ambient {:?} on {}: wake {:?}. Evidence {}. Draft for a person to read.",
                opportunity.class,
                opportunity.subject,
                opportunity.pathway,
                opportunity.evidence_ids.join(", ")
            ),
            channel: OUTBOX_CHANNEL.to_string(),
            sender_identity: self.config.acting_identity.clone(),
            evidence_citations: opportunity
                .evidence_ids
                .iter()
                .map(|evidence_id| EvidenceCitation {
                    source_id: AMBIENT_SOURCE.to_string(),
                    evidence_id: evidence_id.clone(),
                    // No resolver stands behind this door: a signal's
                    // evidence ids are the detectors' own labels (`up:...`,
                    // an anomaly-cluster tag), not records of the evidence
                    // fabric, so there is nothing to compare a claim with.
                    claim: None,
                    // The router's own priority is the only graded number the
                    // mesh states about this detection; it is not a probability
                    // and the citation does not claim one.
                    confidence_bps: i64::from(opportunity.priority_bp),
                })
                .collect(),
            disclosures: vec![Disclosure {
                kind: OUTBOX_DISCLOSURE.to_string(),
                text: OUTBOX_DISCLOSURE_TEXT.to_string(),
            }],
            target_audience: OUTBOX_AUDIENCE.to_string(),
            jurisdiction: self.config.jurisdiction.clone(),
        };
        let pass = PassDecl {
            communications: BTreeMap::from([(DRAFT_TOOL.to_string(), communication.clone())]),
            grounds: vec![Ground::Evidence(format!(
                "ambient signal {} raised by {} on evidence {}",
                opportunity.signal_id,
                opportunity.source,
                opportunity.evidence_ids.join(", ")
            ))],
            approvals: ApprovalDecl {
                goal_approver: pending.promoted.approver.clone(),
                goal_approved_at: now,
                plan_approver: self.config.plan_approver.clone(),
                plan_approved_at: now,
                execute_at: now,
            },
            rate_window: window,
        };
        self.guard.begin_pass(pass)?;
        let result = self.through_guard(
            &mut pending,
            &opportunity,
            &sub_goal,
            &step,
            communication,
            now,
            window,
        );
        self.guard.end_pass();
        match result? {
            Admission::Withheld(reason) => Ok(Intake::Withheld { reason }),
            Admission::Written(event_id) => {
                self.drafted.insert(opportunity.signal_id);
                Ok(Intake::Drafted { draft_id, event_id })
            }
            Admission::WriteFailed(reason) => {
                pending.attempts += 1;
                let attempts = pending.attempts;
                if attempts >= self.config.max_attempts {
                    return Ok(Intake::Abandoned {
                        reason: format!("after {attempts} attempt(s): {reason}"),
                        attempts,
                    });
                }
                if self.retry.len() >= self.config.retry_capacity {
                    return Ok(Intake::Abandoned {
                        reason: format!("the retry queue is full; {reason}"),
                        attempts,
                    });
                }
                self.retry.push_back(pending);
                Ok(Intake::Deferred { reason, attempts })
            }
        }
    }

    /// R12-AGENCY-3002: the standing goal split, through
    /// [`GoalSpec::decompose`], into the one sub-goal this draft serves and
    /// the remainder of the door's work. The draft is then guarded against
    /// the sub-goal, not the standing goal, so what one draft may spend and
    /// for how long is a declaration the decomposition checked against its
    /// parent: one unit of the budget, no exposure, the parent's identity,
    /// jurisdiction and every prohibition, and a horizon that ends when the
    /// signal expires. A signal expiring at this instant leaves no horizon
    /// and is refused here rather than drafted with none.
    fn sub_goal(
        &self,
        opportunity: &AmbientOpportunity,
        now: Timestamp,
    ) -> Result<GoalSpec, Error> {
        let parent = &self.goal;
        let left = opportunity.expiry.as_secs().saturating_sub(now.as_secs());
        let horizon = u64::try_from(left)
            .unwrap_or(0)
            .min(parent.time_horizon_secs);
        let remainder = parent
            .budget
            .checked_sub(DRAFT_COST)
            .ok_or_else(|| Error::numeric("the door's budget underflowed"))?;
        let part = |target: String, budget: Decimal, horizon: u64| GoalSpecDraft {
            class: Some(parent.class),
            target_state: Some(target),
            entities_affected: Some(parent.entities_affected.clone()),
            time_horizon_secs: Some(horizon),
            success_metric: Some(parent.success_metric.clone()),
            acceptable_uncertainty: Some(parent.acceptable_uncertainty),
            budget: Some(budget),
            risk_envelope: Some(Decimal::ZERO),
            jurisdictions: Some(parent.jurisdictions.clone()),
            acting_identity: None,
            prohibited_side_effects: Some(Vec::new()),
            prohibited_methods: Some(Vec::new()),
            stop_conditions: Some(parent.stop_conditions.clone()),
            rollback_conditions: Some(parent.rollback_conditions.clone()),
        };
        let mut children = parent.decompose(vec![
            part(
                format!(
                    "ambient signal {} on {} is drafted to the internal outbox for a person to \
                     read",
                    opportunity.signal_id, opportunity.subject
                ),
                DRAFT_COST,
                horizon,
            ),
            part(
                "the door's remaining ambient drafts".to_string(),
                remainder,
                parent.time_horizon_secs,
            ),
        ])?;
        Ok(children.remove(0))
    }

    #[allow(clippy::too_many_arguments)]
    fn through_guard(
        &mut self,
        pending: &mut Pending,
        opportunity: &AmbientOpportunity,
        sub_goal: &GoalSpec,
        step: &Step,
        communication: CommunicationDraft,
        now: Timestamp,
        window: u64,
    ) -> Result<Admission, Error> {
        // The drafting tool informs a person; it is a communication, and the
        // conduct gate's step check judges it as one.
        let priced = [(step.clone(), DRAFT_COST, Method::Communication)];
        let mut health = ChannelHealth::default();
        let guard = &self.guard;
        let preflight = guard.preflight_observed(sub_goal, &priced, &mut health);
        // Recorded at the seam where the guard decided, before its verdict
        // is acted on, so a refused draft's failing dimension is counted as
        // surely as a cleared one's passes.
        for (channel, dimension, outcome) in health.verdicts() {
            if channel != OUTBOX_CHANNEL {
                return Err(Error::guard(format!(
                    "the door drafts only on `{OUTBOX_CHANNEL}` and the guard judged a draft on \
                     `{channel}`; nothing was written"
                )));
            }
            self.note_health(dimension, outcome);
        }
        let (mut chain, cleared) = match preflight {
            Ok(passed) => passed,
            Err(refusal) => return Ok(Admission::Withheld(refusal.message().to_string())),
        };
        let audit_from = self.guard.audit().len();
        self.guard.execution(
            &mut chain,
            &self.goal.acting_identity,
            std::slice::from_ref(step),
        )?;
        for charged in &cleared {
            if let Err(refusal) = self.guard.authorise(charged) {
                return Ok(Admission::Withheld(refusal.message().to_string()));
            }
        }
        let execution = chain.execution.clone().ok_or_else(|| {
            Error::guard("the guard cleared a step and wrote no execution record")
        })?;
        let draft = OutboxDraft {
            draft_id: opportunity.draft_id(),
            content: communication.content,
            origin_signal: opportunity.signal_id.clone(),
            origin_class: format!("{:?}", opportunity.class),
            origin_pathway: format!("{:?}", opportunity.pathway),
            source: opportunity.source.clone(),
            detected_at: opportunity.detected_at,
            expiry: opportunity.expiry,
            correlation_id: opportunity.correlation_id.clone(),
            evidence_ids: opportunity.evidence_ids.clone(),
            priority_bp: opportunity.priority_bp,
            promoted_by: pending.promoted.approver.clone(),
            promotion_evidence: pending.promoted.evidence.clone(),
            goal_approval: chain.goal,
            plan_approval: chain.plan,
            execution,
            tool_decisions: self.guard.audit()[audit_from..].to_vec(),
            rate_window: window,
            attempt: pending.attempts + 1,
            sub_goal: Some(sub_goal.clone()),
        };
        let draft_id = draft.draft_id.clone();
        self.adapter.stage(DraftContext {
            draft,
            recorded_at: now,
        });
        // The single adapter call in this module, made only after preflight
        // and authorise cleared: the same order `engine::execute` keeps.
        match crate::engine::Adapter::call(&mut self.adapter, step) {
            Ok(()) => {
                self.note_health(HealthDimension::Reach, HealthOutcome::Pass);
                Ok(Admission::Written(EventId::from_string(format!(
                    "evt-outbox-{draft_id}"
                ))))
            }
            Err(error) => {
                self.note_health(HealthDimension::Reach, HealthOutcome::Fail);
                Ok(Admission::WriteFailed(error.message().to_string()))
            }
        }
    }
}

enum Admission {
    Withheld(String),
    Written(EventId),
    WriteFailed(String),
}
