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
use qip_core::{Duration, Timestamp};
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AmbientSignal {
    id: String,
    class: SignalClass,
    subject: String,
    severity_bp: u32,
    trigger: Trigger,
    detected_at: Timestamp,
    observed_deviation: f64,
    expected_baseline: f64,
    horizon: Duration,
    novelty_bp: u32,
    affected_entities: Vec<String>,
    urgency_bp: u32,
    suggested_actions: Vec<String>,
    evidence_ids: Vec<String>,
    expiry: Timestamp,
}

impl AmbientSignal {
    /// Refuses an empty id, subject or trigger name, a severity above
    /// [`MAX_SEVERITY_BP`], novelty or urgency above [`MAX_SEVERITY_BP`], or an
    /// expiry before detected_at. The horizon must be non-negative. At least one
    /// affected entity and one evidence id are required.
    ///
    /// All 15 parameters are required by CONTRACT-025 specification; none are optional.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        class: SignalClass,
        subject: impl Into<String>,
        severity_bp: u32,
        trigger: Trigger,
        detected_at: Timestamp,
        observed_deviation: f64,
        expected_baseline: f64,
        horizon: Duration,
        novelty_bp: u32,
        affected_entities: Vec<String>,
        urgency_bp: u32,
        suggested_actions: Vec<String>,
        evidence_ids: Vec<String>,
        expiry: Timestamp,
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
        if novelty_bp > MAX_SEVERITY_BP {
            return Err(Error::invalid(format!(
                "novelty {novelty_bp} bp exceeds {MAX_SEVERITY_BP}; scale it"
            )));
        }
        if urgency_bp > MAX_SEVERITY_BP {
            return Err(Error::invalid(format!(
                "urgency {urgency_bp} bp exceeds {MAX_SEVERITY_BP}; scale it"
            )));
        }
        if expiry < detected_at {
            return Err(Error::invalid(
                "signal expiry cannot be before detection time",
            ));
        }
        if horizon < Duration::ZERO {
            return Err(Error::invalid("horizon must be non-negative"));
        }
        if affected_entities.is_empty() {
            return Err(Error::invalid("signal must affect at least one entity"));
        }
        if affected_entities.iter().any(|e| e.is_empty()) {
            return Err(Error::invalid(
                "affected entities must not be empty strings",
            ));
        }
        if evidence_ids.is_empty() {
            return Err(Error::invalid(
                "signal must reference at least one evidence item",
            ));
        }
        if evidence_ids.iter().any(|e| e.is_empty()) {
            return Err(Error::invalid("evidence ids must not be empty strings"));
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
            suggested_actions,
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
    pub fn observed_deviation(&self) -> f64 {
        self.observed_deviation
    }
    pub fn expected_baseline(&self) -> f64 {
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
    pub fn suggested_actions(&self) -> &[String] {
        &self.suggested_actions
    }
    pub fn evidence_ids(&self) -> &[String] {
        &self.evidence_ids
    }
    pub fn expiry(&self) -> Timestamp {
        self.expiry
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
                    let event = self.event(&held, Disposition::Activated, now)?;
                    self.used += 1;
                    out.push(event);
                }
            }
        }
        if signal.severity_bp < self.policy.materiality_bp {
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
