//! The ambient mesh's records, its attention router and its promotion gate.
//!
//! The Proactive Ambient Model Mesh (blueprint v11.6 §9.5) notices things
//! nobody asked about. Three failures follow from that and each is made
//! structural here rather than left to the callers:
//!
//! * a discovery nobody can replay — so a detection is an [`AmbientSignal`]
//!   and every activation it causes is an [`AttentionEvent`] naming it;
//! * a storm of material signals waking every specialist at once — so the
//!   [`AttentionRouter`] spends a fixed budget per window and records the
//!   excess as `Deferred` or `Shed`, never as silence;
//! * an ambient guess reaching capital by a shorter path than anything else —
//!   so an ambient output is [`Advisory`], has no way to become a [`Promoted`]
//!   except through [`promote`], and nothing in this module touches an order.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// The most a severity can be, in basis points. Severity is an integer so a
/// record compares exactly and a replay cannot drift on rounding.
pub const MAX_SEVERITY_BP: u32 = 10_000;

/// The four kinds of deviation the Attention Router combines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalClass {
    Surprise,
    Opportunity,
    Risk,
    Assumption,
}

impl SignalClass {
    pub const ALL: [Self; 4] = [
        Self::Surprise,
        Self::Opportunity,
        Self::Risk,
        Self::Assumption,
    ];
}

/// The five outcomes a material deviation becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pathway {
    ResearchTask,
    SpecialistActivation,
    RiskReview,
    CapitalPreparation,
    ReflexModelUpdate,
}

/// What ran the detector: a schedule or an event, never a caller. A signal
/// that cannot say which is a signal that waited to be asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Schedule(String),
    Event(String),
}

/// One ambient detection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmbientSignal {
    id: String,
    class: SignalClass,
    subject: String,
    severity_bp: u32,
    trigger: Trigger,
    detected_at: Timestamp,
    observed_deviation: Decimal,
    expected_baseline: Decimal,
    horizon: Duration,
    novelty_bp: u32,
    affected_entities: Vec<String>,
    urgency_bp: u32,
    wake_targets: Vec<Pathway>,
    evidence_ids: Vec<String>,
    expiry: Timestamp,
}

/// The CONTRACT-025 fields a detector states about what it saw, beyond the
/// identity [`AmbientSignal::new`] already takes positionally.
///
/// Named fields rather than nine more positional arguments: novelty and
/// urgency are both `u32` basis points and the entity and evidence lists are
/// both `Vec<String>`, so a positional call that transposed either pair would
/// compile and record the wrong fact. The deviation and baseline are
/// [`Decimal`] rather than `f64` so the record keeps `Eq` and replays exactly,
/// for the same reason severity is an integer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    pub observed_deviation: Decimal,
    pub expected_baseline: Decimal,
    /// How far ahead the deviation matters. Zero is a statement (it matters
    /// now); negative is a detector bug.
    pub horizon: Duration,
    /// Novelty/surprise, in basis points up to [`MAX_SEVERITY_BP`].
    pub novelty_bp: u32,
    /// The entities and assets affected; at least one, none empty.
    pub affected_entities: Vec<String>,
    /// Urgency, in basis points up to [`MAX_SEVERITY_BP`].
    pub urgency_bp: u32,
    /// The pathways the detector suggests waking. Typed as [`Pathway`] so a
    /// suggestion names something the router can actually wake; at least one.
    pub wake_targets: Vec<Pathway>,
    /// Evidence lineage; at least one id, none empty.
    pub evidence_ids: Vec<String>,
    /// After this instant the signal wakes nothing (see [`AttentionRouter::route`]).
    pub expiry: Timestamp,
}

impl AmbientSignal {
    /// Refuses an empty id, subject or trigger name, and a severity above
    /// [`MAX_SEVERITY_BP`] — it is not clamped, because a clamped severity is
    /// a detector bug that then survives.
    ///
    /// Refuses, too, a [`Detection`] missing any CONTRACT-025 field: no
    /// affected entity, no wake target, no evidence id, an empty entity or
    /// evidence id, a novelty or urgency above [`MAX_SEVERITY_BP`], a negative
    /// horizon, or an expiry before detection. A signal with no evidence
    /// lineage is a discovery nobody can replay; one with no expiry would wake
    /// a specialist about a deviation that stopped mattering hours ago.
    pub fn new(
        id: impl Into<String>,
        class: SignalClass,
        subject: impl Into<String>,
        severity_bp: u32,
        trigger: Trigger,
        detected_at: Timestamp,
        detection: Detection,
    ) -> Result<Self> {
        let (id, subject) = (id.into(), subject.into());
        let trigger_name = match &trigger {
            Trigger::Schedule(n) | Trigger::Event(n) => n,
        };
        if id.is_empty() || subject.is_empty() || trigger_name.is_empty() {
            return Err(Error::invalid(
                "an ambient signal needs an id, a subject and a named trigger; supply all three",
            ));
        }
        if severity_bp > MAX_SEVERITY_BP {
            return Err(Error::invalid(format!(
                "severity {severity_bp} bp exceeds {MAX_SEVERITY_BP}; scale the detector, it is not clamped"
            )));
        }
        let Detection {
            observed_deviation,
            expected_baseline,
            horizon,
            novelty_bp,
            affected_entities,
            urgency_bp,
            wake_targets,
            evidence_ids,
            expiry,
        } = detection;
        if novelty_bp > MAX_SEVERITY_BP {
            return Err(Error::invalid(format!(
                "novelty {novelty_bp} bp exceeds {MAX_SEVERITY_BP}; scale the detector, it is not clamped"
            )));
        }
        if urgency_bp > MAX_SEVERITY_BP {
            return Err(Error::invalid(format!(
                "urgency {urgency_bp} bp exceeds {MAX_SEVERITY_BP}; scale the detector, it is not clamped"
            )));
        }
        if horizon < Duration::ZERO {
            return Err(Error::invalid(
                "a negative horizon is not a horizon; state how far ahead the deviation matters, zero for now",
            ));
        }
        if expiry < detected_at {
            return Err(Error::invalid(
                "an expiry before detection means the signal was dead when it was raised; state when it stops mattering",
            ));
        }
        if affected_entities.is_empty() || affected_entities.iter().any(String::is_empty) {
            return Err(Error::invalid(
                "an ambient signal must name the entities it affects, none blank",
            ));
        }
        if wake_targets.is_empty() {
            return Err(Error::invalid(
                "an ambient signal must suggest at least one pathway to wake",
            ));
        }
        if evidence_ids.is_empty() || evidence_ids.iter().any(String::is_empty) {
            return Err(Error::invalid(
                "an ambient signal must carry its evidence lineage, none blank; a signal without it cannot be replayed",
            ));
        }
        Ok(Self {
            id,
            class,
            subject,
            severity_bp,
            trigger,
            detected_at,
            observed_deviation,
            expected_baseline,
            horizon,
            novelty_bp,
            affected_entities,
            urgency_bp,
            wake_targets,
            evidence_ids,
            expiry,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn class(&self) -> SignalClass {
        self.class
    }
    pub fn subject(&self) -> &str {
        &self.subject
    }
    pub fn severity_bp(&self) -> u32 {
        self.severity_bp
    }
    pub fn trigger(&self) -> &Trigger {
        &self.trigger
    }
    pub fn detected_at(&self) -> Timestamp {
        self.detected_at
    }
    pub fn observed_deviation(&self) -> Decimal {
        self.observed_deviation
    }
    pub fn expected_baseline(&self) -> Decimal {
        self.expected_baseline
    }
    pub fn horizon(&self) -> Duration {
        self.horizon
    }
    pub fn novelty_bp(&self) -> u32 {
        self.novelty_bp
    }
    pub fn affected_entities(&self) -> &[String] {
        &self.affected_entities
    }
    pub fn urgency_bp(&self) -> u32 {
        self.urgency_bp
    }
    pub fn wake_targets(&self) -> &[Pathway] {
        &self.wake_targets
    }
    pub fn evidence_ids(&self) -> &[String] {
        &self.evidence_ids
    }
    pub fn expiry(&self) -> Timestamp {
        self.expiry
    }
    /// Whether the signal has stopped mattering at `now`. The expiry instant
    /// itself is still live: a signal stated to matter until `t` matters at `t`.
    pub fn is_expired_at(&self, now: Timestamp) -> bool {
        now > self.expiry
    }
}

/// What the router did with a material signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// The pathway was activated and the budget charged.
    Activated,
    /// The budget was spent; held for a later window.
    Deferred,
    /// The budget was spent and the hold queue was full; dropped on the record.
    Shed,
    /// The signal was past its expiry when the router reached it — on arrival
    /// or when a new window drained it from the hold queue. It woke nothing
    /// and charged no budget, and is dropped on the record rather than in
    /// silence, so a storm that outlasted its own signals is visible.
    Expired,
}

/// One routing decision, referencing the signal that caused it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionEvent {
    pub id: String,
    pub signal_id: String,
    pub pathway: Pathway,
    pub disposition: Disposition,
    pub at: Timestamp,
}

/// Materiality, class-to-pathway routing and the attention budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingPolicy {
    materiality_bp: u32,
    routes: BTreeMap<SignalClass, Pathway>,
    budget_per_window: u32,
    window: Duration,
    defer_capacity: usize,
}

impl RoutingPolicy {
    /// Refuses an incomplete route table, a zero budget or window, and a
    /// materiality above [`MAX_SEVERITY_BP`] (which nothing could reach).
    pub fn new(
        materiality_bp: u32,
        routes: BTreeMap<SignalClass, Pathway>,
        budget_per_window: u32,
        window: Duration,
        defer_capacity: usize,
    ) -> Result<Self> {
        if let Some(missing) = SignalClass::ALL.iter().find(|c| !routes.contains_key(c)) {
            return Err(Error::invalid(format!(
                "routing policy names no pathway for {missing:?}; route all four classes"
            )));
        }
        if materiality_bp > MAX_SEVERITY_BP {
            return Err(Error::invalid(
                "materiality above 10000 bp admits nothing; lower it",
            ));
        }
        if budget_per_window == 0 || window <= Duration::ZERO {
            return Err(Error::invalid(
                "an attention budget needs a positive count and window; a zero budget is a switched-off mesh, so do not run it",
            ));
        }
        Ok(Self {
            materiality_bp,
            routes,
            budget_per_window,
            window,
            defer_capacity,
        })
    }

    /// The default routing: surprise to a specialist, opportunity to research,
    /// risk to a risk review, assumption to a reflex-model update.
    pub fn standard(
        materiality_bp: u32,
        budget_per_window: u32,
        window: Duration,
        defer_capacity: usize,
    ) -> Result<Self> {
        let routes = BTreeMap::from([
            (SignalClass::Surprise, Pathway::SpecialistActivation),
            (SignalClass::Opportunity, Pathway::ResearchTask),
            (SignalClass::Risk, Pathway::RiskReview),
            (SignalClass::Assumption, Pathway::ReflexModelUpdate),
        ]);
        Self::new(
            materiality_bp,
            routes,
            budget_per_window,
            window,
            defer_capacity,
        )
    }

    pub fn pathway_for(&self, class: SignalClass) -> Option<Pathway> {
        self.routes.get(&class).copied()
    }
}

/// The Ambient Attention Brain. Pure state: time arrives as an argument.
#[derive(Debug)]
pub struct AttentionRouter {
    policy: RoutingPolicy,
    window_start: Option<Timestamp>,
    used: u32,
    deferred: VecDeque<AmbientSignal>,
    issued: u64,
}

impl AttentionRouter {
    pub fn new(policy: RoutingPolicy) -> Self {
        Self {
            policy,
            window_start: None,
            used: 0,
            deferred: VecDeque::new(),
            issued: 0,
        }
    }

    /// Signals held for a later window.
    pub fn deferred_len(&self) -> usize {
        self.deferred.len()
    }

    /// Route one signal at `now`. Returns every event this call produced: the
    /// activations of signals deferred earlier (a new window drains the hold
    /// queue first, oldest first, so a storm cannot starve what it displaced)
    /// and then this signal's own. A signal below materiality produces none —
    /// below the bar is not an activation and not a loss.
    ///
    /// A material signal consumed after its expiry — arriving late, or held
    /// past it — is recorded [`Disposition::Expired`] and wakes nothing.
    ///
    /// Refuses a `now` before the current window opened: a clock that steps
    /// back would refill a budget that was already spent.
    pub fn route(&mut self, signal: &AmbientSignal, now: Timestamp) -> Result<Vec<AttentionEvent>> {
        let mut out = Vec::new();
        match self.window_start {
            Some(start) if now < start => {
                return Err(Error::invalid(
                    "route called with a time before the open attention window; pass a monotonic time",
                ));
            }
            Some(start) if now.since(start) < self.policy.window => {}
            _ => {
                self.window_start = Some(now);
                self.used = 0;
                while self.used < self.policy.budget_per_window {
                    let Some(held) = self.deferred.pop_front() else {
                        break;
                    };
                    // A held signal that went stale while it waited wakes
                    // nothing and does not spend the slot a live one could.
                    if held.is_expired_at(now) {
                        let event = self.event(&held, Disposition::Expired, now)?;
                        out.push(event);
                        continue;
                    }
                    let event = self.event(&held, Disposition::Activated, now)?;
                    self.used += 1;
                    out.push(event);
                }
            }
        }
        if signal.severity_bp < self.policy.materiality_bp {
            return Ok(out);
        }
        if signal.is_expired_at(now) {
            let event = self.event(signal, Disposition::Expired, now)?;
            out.push(event);
            return Ok(out);
        }
        let disposition = if self.used < self.policy.budget_per_window && self.deferred.is_empty() {
            self.used += 1;
            Disposition::Activated
        } else if self.deferred.len() < self.policy.defer_capacity {
            self.deferred.push_back(signal.clone());
            Disposition::Deferred
        } else {
            Disposition::Shed
        };
        let event = self.event(signal, disposition, now)?;
        out.push(event);
        Ok(out)
    }

    fn event(
        &mut self,
        s: &AmbientSignal,
        d: Disposition,
        at: Timestamp,
    ) -> Result<AttentionEvent> {
        let pathway = self.policy.pathway_for(s.class).ok_or_else(|| {
            Error::invalid("routing policy lost a class; construct it through RoutingPolicy::new")
        })?;
        self.issued += 1;
        Ok(AttentionEvent {
            id: format!("attention-{}", self.issued),
            signal_id: s.id.clone(),
            pathway,
            disposition: d,
            at,
        })
    }
}

/// An ambient output. It can be read; it cannot be acted on. There is no
/// accessor returning the owned value and no conversion to [`Promoted`] other
/// than [`promote`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advisory<T> {
    output: T,
    origin_signal: String,
}

impl<T> Advisory<T> {
    pub fn new(output: T, origin_signal: impl Into<String>) -> Self {
        Self {
            output,
            origin_signal: origin_signal.into(),
        }
    }
    pub fn view(&self) -> &T {
        &self.output
    }
    pub fn origin_signal(&self) -> &str {
        &self.origin_signal
    }
}

/// The evidence a promotion is judged on: the same four boundaries any other
/// model output crosses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromotionCase {
    /// References to the held-out evaluation, nonempty.
    pub evidence: Vec<String>,
    pub evaluation_passed: bool,
    /// A named authority, as for any other promotion.
    pub approver: String,
    /// The deterministic pre-trade controls were run on the proposal and held.
    pub deterministic_controls_held: bool,
}

/// An ambient output that crossed the gate. Only [`promote`] builds one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Promoted<T> {
    output: T,
    pub approver: String,
    pub evidence: Vec<String>,
}

impl<T> Promoted<T> {
    pub fn into_inner(self) -> T {
        self.output
    }
}

/// The only road from [`Advisory`] to [`Promoted`]. Refuses, naming what is
/// missing, rather than guessing; a refused promotion consumes the advisory,
/// so the mesh must propose again with evidence.
pub fn promote<T>(advisory: Advisory<T>, case: &PromotionCase) -> Result<Promoted<T>> {
    let missing = if case.evidence.is_empty() || case.evidence.iter().any(|e| e.is_empty()) {
        Some("evaluation evidence")
    } else if !case.evaluation_passed {
        Some("a passing evaluation")
    } else if case.approver.is_empty() {
        Some("a named approver")
    } else if !case.deterministic_controls_held {
        Some("the deterministic controls holding")
    } else {
        None
    };
    if let Some(what) = missing {
        return Err(Error::denied(format!(
            "ambient output from signal {} is advisory until promoted; promotion needs {what}",
            advisory.origin_signal
        )));
    }
    Ok(Promoted {
        output: advisory.output,
        approver: case.approver.clone(),
        evidence: case.evidence.clone(),
    })
}

/// The outcome of an evidence acquisition decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionStatus {
    Requested,
    InProgress,
    Complete,
    Halted,
}

/// Ranking criteria for candidate evidence acquisitions: economic significance,
/// uncertainty reduction, urgency, and acquisition cost. All are 0-10000 basis points
/// so they compare exactly and survive replay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcquisitionCriteria {
    /// Economic significance of reducing the uncertainty this evidence addresses,
    /// in basis points (0 to 10000). Higher means more economically material.
    pub economic_significance_bp: u32,
    /// Uncertainty reduction capability: how much this evidence would reduce
    /// decision uncertainty, in basis points. Higher means more informative.
    pub uncertainty_reduction_bp: u32,
    /// Urgency: how time-sensitive the decision is, in basis points. Higher means
    /// more urgent to acquire now rather than later.
    pub urgency_bp: u32,
    /// Acquisition cost: estimated cost to obtain this evidence, in basis points
    /// of the decision value. Higher means more expensive to acquire.
    pub acquisition_cost_bp: u32,
}

impl AcquisitionCriteria {
    /// Constructs acquisition criteria. Refuses any basis point value above 10000.
    pub fn new(
        economic_significance_bp: u32,
        uncertainty_reduction_bp: u32,
        urgency_bp: u32,
        acquisition_cost_bp: u32,
    ) -> Result<Self> {
        if economic_significance_bp > MAX_SEVERITY_BP
            || uncertainty_reduction_bp > MAX_SEVERITY_BP
            || urgency_bp > MAX_SEVERITY_BP
            || acquisition_cost_bp > MAX_SEVERITY_BP
        {
            return Err(Error::invalid(
                "acquisition criteria: all basis points must be 0-10000; do not exceed MAX_SEVERITY_BP",
            ));
        }
        Ok(Self {
            economic_significance_bp,
            uncertainty_reduction_bp,
            urgency_bp,
            acquisition_cost_bp,
        })
    }

    /// Computes the net value of acquiring this evidence: the expected value of
    /// information (EVI) minus the cost. Returns 0 if EVI is less than cost.
    /// EVI is computed as a weighted average: economic_significance (40%) +
    /// uncertainty_reduction (40%) + urgency (20%).
    pub fn net_value_bp(&self) -> u32 {
        let evi = ((self.economic_significance_bp as u64 * 40
            + self.uncertainty_reduction_bp as u64 * 40
            + self.urgency_bp as u64 * 20)
            / 100) as u32;
        evi.saturating_sub(self.acquisition_cost_bp)
    }
}

/// A request to acquire evidence to reduce uncertainty in a decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfoRequest {
    id: String,
    decision_id: String,
    criteria: AcquisitionCriteria,
    /// When acquisition must complete, or after which new acquisitions are rejected.
    deadline: Timestamp,
    /// When the request was issued.
    requested_at: Timestamp,
    status: AcquisitionStatus,
    /// Cumulative cost spent on acquisitions against this request so far, in basis points.
    cumulative_cost_bp: u32,
}

impl InfoRequest {
    /// Constructs an information acquisition request. Refuses empty id or decision_id,
    /// or a deadline before the request time.
    pub fn new(
        id: impl Into<String>,
        decision_id: impl Into<String>,
        criteria: AcquisitionCriteria,
        deadline: Timestamp,
        requested_at: Timestamp,
    ) -> Result<Self> {
        let (id, decision_id) = (id.into(), decision_id.into());
        if id.is_empty() || decision_id.is_empty() {
            return Err(Error::invalid(
                "info request: id and decision_id must not be empty; supply both",
            ));
        }
        if deadline < requested_at {
            return Err(Error::invalid(
                "info request: deadline must not be before the request time; pass a future deadline",
            ));
        }
        Ok(Self {
            id,
            decision_id,
            criteria,
            deadline,
            requested_at,
            status: AcquisitionStatus::Requested,
            cumulative_cost_bp: 0,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn decision_id(&self) -> &str {
        &self.decision_id
    }
    pub fn criteria(&self) -> &AcquisitionCriteria {
        &self.criteria
    }
    pub fn deadline(&self) -> Timestamp {
        self.deadline
    }
    pub fn requested_at(&self) -> Timestamp {
        self.requested_at
    }
    pub fn status(&self) -> AcquisitionStatus {
        self.status
    }
    pub fn cumulative_cost_bp(&self) -> u32 {
        self.cumulative_cost_bp
    }

    /// Records that an acquisition was completed at a cost. Refuses zero cost,
    /// a cost that would overflow cumulative cost, or an attempt to record after
    /// completion or halting.
    pub fn record_acquisition(&mut self, acquisition_cost_bp: u32) -> Result<()> {
        if acquisition_cost_bp == 0 {
            return Err(Error::invalid(
                "info request: acquisition cost must be positive; do not record zero-cost acquisitions",
            ));
        }
        if self.status == AcquisitionStatus::Complete || self.status == AcquisitionStatus::Halted {
            return Err(Error::denied(
                "info request: cannot record acquisition after request is complete or halted; check status",
            ));
        }
        self.cumulative_cost_bp = self
            .cumulative_cost_bp
            .checked_add(acquisition_cost_bp)
            .ok_or_else(|| {
                Error::numeric("info request: cumulative cost overflowed; reduce budget")
            })?;
        self.status = AcquisitionStatus::InProgress;
        Ok(())
    }

    /// Completes the request. Refuses if the request was not in progress.
    pub fn complete(&mut self) -> Result<()> {
        if self.status != AcquisitionStatus::InProgress {
            return Err(Error::invalid(
                "info request: can only complete a request that is in progress; check status",
            ));
        }
        self.status = AcquisitionStatus::Complete;
        Ok(())
    }

    /// Halts acquisition for this request. Returns true if the request was halted,
    /// false if it was already complete or halted.
    pub fn halt(&mut self) -> bool {
        if self.status == AcquisitionStatus::Requested
            || self.status == AcquisitionStatus::InProgress
        {
            self.status = AcquisitionStatus::Halted;
            true
        } else {
            false
        }
    }

    /// Checks whether acquisition should halt: true if marginal value (next acquisition's
    /// net value) is below the minimum viable threshold, or if the deadline has been reached.
    pub fn should_halt(&self, now: Timestamp, min_viable_value_bp: u32) -> bool {
        if now >= self.deadline {
            return true;
        }
        if self.criteria.net_value_bp() < min_viable_value_bp {
            return true;
        }
        false
    }
}
