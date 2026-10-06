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

/// One compute resource type the Meta-Intelligence brain allocates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputeResourceType {
    Cpu,
    Gpu,
    Tpu,
    Qpu,
}

impl ComputeResourceType {
    pub const ALL: [Self; 4] = [Self::Cpu, Self::Gpu, Self::Tpu, Self::Qpu];
}

/// Model reputation tracked by domain, horizon, and regime. All fields are
/// normalized to 0-10000 basis points so scale does not require decimals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelReputation {
    domain: String,
    horizon: String,
    regime: String,
    accuracy_bp: u32,
    signal_quality_bp: u32,
    forecast_skill_bp: u32,
}

impl ModelReputation {
    /// Creates a reputation record. Refuses empty domain, horizon, or regime,
    /// and any metric above MAX_SEVERITY_BP (the standard bound for normalized
    /// metrics in this module).
    pub fn new(
        domain: impl Into<String>,
        horizon: impl Into<String>,
        regime: impl Into<String>,
        accuracy_bp: u32,
        signal_quality_bp: u32,
        forecast_skill_bp: u32,
    ) -> Result<Self> {
        let (domain, horizon, regime) = (domain.into(), horizon.into(), regime.into());
        for (name, value) in [
            ("domain", domain.as_str()),
            ("horizon", horizon.as_str()),
            ("regime", regime.as_str()),
        ] {
            if value.is_empty() {
                return Err(Error::invalid(format!("{name} must not be empty")));
            }
        }
        for (name, value) in [
            ("accuracy_bp", accuracy_bp),
            ("signal_quality_bp", signal_quality_bp),
            ("forecast_skill_bp", forecast_skill_bp),
        ] {
            if value > MAX_SEVERITY_BP {
                return Err(Error::invalid(format!(
                    "{name} {value} exceeds maximum {MAX_SEVERITY_BP}; do not clamp it"
                )));
            }
        }
        Ok(Self {
            domain,
            horizon,
            regime,
            accuracy_bp,
            signal_quality_bp,
            forecast_skill_bp,
        })
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }
    pub fn horizon(&self) -> &str {
        &self.horizon
    }
    pub fn regime(&self) -> &str {
        &self.regime
    }
    pub fn accuracy_bp(&self) -> u32 {
        self.accuracy_bp
    }
    pub fn signal_quality_bp(&self) -> u32 {
        self.signal_quality_bp
    }
    pub fn forecast_skill_bp(&self) -> u32 {
        self.forecast_skill_bp
    }

    /// The overall reputation as a weighted average. Accuracy is weighted 40%,
    /// signal quality 30%, forecast skill 30%.
    pub fn overall_bp(&self) -> u32 {
        ((self.accuracy_bp as u64 * 40
            + self.signal_quality_bp as u64 * 30
            + self.forecast_skill_bp as u64 * 30)
            / 100) as u32
    }
}

/// Compute allocation to a work item, recorded for auditability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputeAllocation {
    work_id: String,
    resource_type: ComputeResourceType,
    units: u32,
    allocated_at: Timestamp,
    model_reputation: ModelReputation,
    rationale: String,
}

impl ComputeAllocation {
    /// Records an allocation decision. Refuses empty work_id, rationale, or
    /// zero units.
    pub fn new(
        work_id: impl Into<String>,
        resource_type: ComputeResourceType,
        units: u32,
        allocated_at: Timestamp,
        model_reputation: ModelReputation,
        rationale: impl Into<String>,
    ) -> Result<Self> {
        let (work_id, rationale) = (work_id.into(), rationale.into());
        if work_id.is_empty() || rationale.is_empty() {
            return Err(Error::invalid(
                "work_id and rationale must not be empty; supply both",
            ));
        }
        if units == 0 {
            return Err(Error::invalid(
                "allocation units must be positive; do not record zero-unit allocations",
            ));
        }
        Ok(Self {
            work_id,
            resource_type,
            units,
            allocated_at,
            model_reputation,
            rationale,
        })
    }

    pub fn work_id(&self) -> &str {
        &self.work_id
    }
    pub fn resource_type(&self) -> ComputeResourceType {
        self.resource_type
    }
    pub fn units(&self) -> u32 {
        self.units
    }
    pub fn model_reputation(&self) -> &ModelReputation {
        &self.model_reputation
    }
    pub fn rationale(&self) -> &str {
        &self.rationale
    }
}
