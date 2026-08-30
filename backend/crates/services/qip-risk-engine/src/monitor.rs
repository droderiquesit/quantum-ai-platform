//! Continuous risk monitoring.
//!
//! Pre-trade checks stop bad orders. They do nothing about a book that becomes
//! unacceptable while it sits — a position that was 6% of equity when it was
//! opened is 12% after the rest of the book halves.
//!
//! [`RiskMonitor::observe`] is called on every material change and decides
//! what to do about it: nothing, warn, reduce, or stop. The escalation is
//! deterministic and stated in [`MonitorPolicy`], so an operator can read what
//! the platform will do before it does it.

use crate::autonomy::{AutonomyLevel, KillSwitch};
use qip_core::time::{Duration, Timestamp};
use qip_risk::limits::{LimitCheck, LimitSet, RiskState};
use serde::{Deserialize, Serialize};

/// What the monitor decided to do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum MonitorAction {
    /// Nothing is wrong.
    Continue,
    /// A limit is close. Recorded and surfaced; nothing is stopped.
    Warn { warnings: Vec<String> },
    /// A limit is breached and the book must be brought back inside it.
    ///
    /// New risk-increasing orders are blocked; reducing orders still pass, or
    /// the platform could not fix the breach it just noticed.
    ReduceOnly { breaches: Vec<String> },
    /// Stop the scope named.
    HaltScope { scope: String, reason: String },
    /// Stop everything.
    HaltGlobally { reason: String },
}

impl MonitorAction {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Warn { .. } => "warn",
            Self::ReduceOnly { .. } => "reduce_only",
            Self::HaltScope { .. } => "halt_scope",
            Self::HaltGlobally { .. } => "halt_globally",
        }
    }

    /// Whether new risk-increasing orders may still be sent.
    pub const fn permits_new_risk(&self) -> bool {
        matches!(self, Self::Continue | Self::Warn { .. })
    }

    /// Whether risk-reducing orders may still be sent.
    ///
    /// True except on a global halt. A reduce-only state that also blocked
    /// reductions would leave the platform unable to fix the breach that
    /// caused it.
    pub const fn permits_reduction(&self) -> bool {
        !matches!(self, Self::HaltGlobally { .. })
    }

    pub fn severity(&self) -> u8 {
        match self {
            Self::Continue => 0,
            Self::Warn { .. } => 1,
            Self::ReduceOnly { .. } => 2,
            Self::HaltScope { .. } => 3,
            Self::HaltGlobally { .. } => 4,
        }
    }
}

/// When the monitor escalates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MonitorPolicy {
    /// Drawdown at which the platform halts entirely.
    ///
    /// The last line: past this the platform has been wrong for long enough
    /// that continuing to trade on the same models is not defensible.
    pub halt_drawdown: f64,
    /// Single-day loss at which the platform halts entirely.
    pub halt_daily_loss: f64,
    /// Consecutive observations in breach before a scope is halted rather than
    /// merely reduced.
    ///
    /// Not one: a single reading can be a stale mark or a bad print, and
    /// halting on it would make the platform stoppable by one bad tick.
    pub breaches_before_halt: usize,
    /// How long a breach may persist before it is treated as unresolvable.
    pub breach_grace_period: Duration,
    /// Whether a breach at a live autonomy level halts rather than reduces.
    ///
    /// True by default: the same breach is more serious when real money is
    /// moving, and the asymmetry should favour stopping.
    pub halt_on_live_breach: bool,
}

impl Default for MonitorPolicy {
    fn default() -> Self {
        Self {
            halt_drawdown: 0.20,
            halt_daily_loss: 0.05,
            breaches_before_halt: 3,
            breach_grace_period: Duration::from_hours(4),
            halt_on_live_breach: true,
        }
    }
}

/// One observation the monitor made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub at: Timestamp,
    pub action: MonitorAction,
    pub check: LimitCheck,
    /// Consecutive observations in breach as of this one.
    pub consecutive_breaches: usize,
}

/// Watches the book against its limits.
#[derive(Debug)]
pub struct RiskMonitor {
    limits: LimitSet,
    policy: MonitorPolicy,
    consecutive_breaches: usize,
    first_breach_at: Option<Timestamp>,
    observations: Vec<Observation>,
}

impl RiskMonitor {
    pub fn new(limits: LimitSet, policy: MonitorPolicy) -> Self {
        Self {
            limits,
            policy,
            consecutive_breaches: 0,
            first_breach_at: None,
            observations: Vec::new(),
        }
    }

    pub fn policy(&self) -> MonitorPolicy {
        self.policy
    }

    /// The limits this monitor enforces.
    ///
    /// Exposed so a caller can populate the `RiskState` fields the limits it
    /// actually holds will read. Two of the limit kinds —
    /// [`LimitKind::MaxValueAtRisk`] and [`LimitKind::MaxExpectedShortfall`] —
    /// look their figure up in a map keyed by confidence and record nothing
    /// when the key is absent, so a caller that guesses which confidences to
    /// compute produces a limit that silently never evaluates. Reading them
    /// from here means the key is derived from the same value the limit will
    /// format, and cannot drift from it.
    pub fn limits(&self) -> &LimitSet {
        &self.limits
    }

    pub fn observations(&self) -> &[Observation] {
        &self.observations
    }

    pub fn consecutive_breaches(&self) -> usize {
        self.consecutive_breaches
    }

    /// How long the current breach has persisted.
    pub fn breach_duration(&self, now: Timestamp) -> Option<Duration> {
        self.first_breach_at.map(|at| now.since(at))
    }

    /// Evaluate the state and decide what to do.
    ///
    /// `scope` names what would be halted if a scoped halt is the answer.
    pub fn observe(
        &mut self,
        state: &RiskState,
        scope: &str,
        level: AutonomyLevel,
        now: Timestamp,
    ) -> MonitorAction {
        let check = self.limits.check(state);

        // The hard stops come first and do not care about consecutive counts:
        // a 20% drawdown is not a bad print.
        let action = if state.drawdown >= self.policy.halt_drawdown {
            MonitorAction::HaltGlobally {
                reason: format!(
                    "drawdown of {:.1}% reached the {:.1}% halt threshold",
                    state.drawdown * 100.0,
                    self.policy.halt_drawdown * 100.0
                ),
            }
        } else if state.daily_loss >= self.policy.halt_daily_loss {
            MonitorAction::HaltGlobally {
                reason: format!(
                    "a single-day loss of {:.1}% reached the {:.1}% halt threshold",
                    state.daily_loss * 100.0,
                    self.policy.halt_daily_loss * 100.0
                ),
            }
        } else if check.is_blocked() {
            self.consecutive_breaches += 1;
            self.first_breach_at.get_or_insert(now);

            let breaches: Vec<String> = check
                .blocking()
                .iter()
                .map(|breach| {
                    format!(
                        "{}: observed {:.6} against {:.6}",
                        breach.limit_name, breach.observed, breach.bound
                    )
                })
                .collect();

            let persisted = self
                .breach_duration(now)
                .is_some_and(|d| d > self.policy.breach_grace_period);
            let repeated = self.consecutive_breaches >= self.policy.breaches_before_halt;
            let live_breach = self.policy.halt_on_live_breach && level.is_live();

            if repeated || persisted || live_breach {
                MonitorAction::HaltScope {
                    scope: scope.to_string(),
                    reason: format!(
                        "{} breach(es) unresolved after {} observation(s): {}",
                        breaches.len(),
                        self.consecutive_breaches,
                        breaches.join("; ")
                    ),
                }
            } else {
                MonitorAction::ReduceOnly { breaches }
            }
        } else {
            self.consecutive_breaches = 0;
            self.first_breach_at = None;
            let warnings: Vec<String> = check
                .warnings()
                .iter()
                .map(|breach| {
                    format!(
                        "{} at {:.1}% of its limit",
                        breach.limit_name,
                        if breach.bound.abs() > 1e-12 {
                            breach.observed / breach.bound * 100.0
                        } else {
                            0.0
                        }
                    )
                })
                .collect();
            if warnings.is_empty() {
                MonitorAction::Continue
            } else {
                MonitorAction::Warn { warnings }
            }
        };

        self.observations.push(Observation {
            at: now,
            action: action.clone(),
            check,
            consecutive_breaches: self.consecutive_breaches,
        });
        action
    }

    /// Apply an action to the kill switch.
    ///
    /// Separated from [`Self::observe`] so the decision and the effect can be
    /// tested independently, and so a caller running in a dry-run mode can see
    /// what would happen without it happening.
    pub fn enforce(&self, action: &MonitorAction, kill_switch: &mut KillSwitch, now: Timestamp) {
        match action {
            MonitorAction::HaltGlobally { reason } => {
                kill_switch.trip_global(now, "risk-monitor", reason.clone());
            }
            MonitorAction::HaltScope { scope, reason } => {
                kill_switch.trip_scope(scope.clone(), now, "risk-monitor", reason.clone());
            }
            _ => {}
        }
    }

    /// Reset the breach counters, e.g. after an operator resolves a breach.
    pub fn reset(&mut self) {
        self.consecutive_breaches = 0;
        self.first_breach_at = None;
    }
}
