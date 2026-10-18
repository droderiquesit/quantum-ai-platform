//! The deep brain's operations loop: the AIOps entity graph, incident
//! timeline and runbook-only remediation given a production caller, and the
//! critical-path objectives evaluated and recorded every cycle (OBS-7115,
//! OBS-7121, OBS-7122).
//!
//! # Why here
//!
//! `qip_observability::aiops` held the graph, the timeline and a
//! `Remediator` that could only act through an `Effector` some composition
//! root implements — and no root implemented one, so every piece was tested
//! and none ran. The deep brain is the process that owns its own optional
//! load (the research loop, the search, the fit), so it is the one place an
//! automated remediation can be both real and safe: the only effect this
//! file's effector has is to switch off one of this node's own research
//! capabilities until the process restarts. It cannot reach a venue, a
//! ledger, another service or a cloud resource, and an action no runbook
//! names is refused and audited.
//!
//! # What an incident is
//!
//! A level shift in this node's own iteration time — a cycle plus everything
//! run in the gap after it — found by `detect_level_shift`, with no static
//! threshold. The incident's timeline joins it with the data anomalies,
//! model drift and venue behaviour the same window saw, so the operator
//! reads one ordered account instead of four dashboards.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_kernel::Platform;
use qip_observability::aiops::{
    Anomaly, Effector, EntityGraph, EntityKey, Remediator, Runbook, Signal, SignalClass,
    TimelineEvent, TimelineSource, build_timeline, detect_level_shift,
};
use qip_observability::critical_paths;
use qip_observability::metrics::{Metrics, labels, names};
use std::collections::{BTreeSet, VecDeque};

/// The capabilities this node may switch off, by the names runbooks target.
pub const CAPABILITIES: [&str; 1] = ["research"];
/// Iterations the level-shift detector reads either side of a candidate.
pub const SHIFT_HALF: usize = 6;
/// Iteration times retained for the detector.
pub const SERIES_RETAINED: usize = 128;
/// Timeline events retained between incidents.
pub const TIMELINE_RETAINED: usize = 256;
/// Signals the graph keeps per entity.
pub const GRAPH_RETENTION: usize = 64;
/// A fill miss past this many basis points is venue behaviour worth a line
/// on an incident's timeline. The research loop's residual threshold.
pub const VENUE_BEHAVIOUR_BPS: f64 = crate::research::RESIDUAL_BPS;

/// `Solver::Quantum.as_str()`, as journalled in `SolverRoutingJournal::ran`.
pub const QUANTUM_SOLVER: &str = "quantum";

/// The pre-approved runbooks. Committed code, reviewed like any other: an
/// automated action exists only if a line here names it.
pub fn runbooks() -> Vec<Runbook> {
    vec![Runbook {
        name: "shed-research-on-iteration-shift".to_string(),
        action: "disable_capability".to_string(),
        targets: BTreeSet::from(["research".to_string()]),
    }]
}

/// The effector: this node's switchable capabilities, and nothing else.
#[derive(Debug, Default)]
pub struct CapabilitySwitch {
    disabled: BTreeSet<String>,
}

impl CapabilitySwitch {
    pub fn is_disabled(&self, capability: &str) -> bool {
        self.disabled.contains(capability)
    }
}

impl Effector for CapabilitySwitch {
    fn apply(&mut self, action: &str, target: &str) -> Result<String> {
        if action != "disable_capability" {
            return Err(Error::denied(format!(
                "this node's effector carries out disable_capability only, not {action}"
            )));
        }
        if !CAPABILITIES.contains(&target) {
            return Err(Error::denied(format!(
                "`{target}` is not a capability this node can switch off; it can switch off {CAPABILITIES:?}"
            )));
        }
        if self.disabled.insert(target.to_string()) {
            Ok(format!("{target} disabled until the process restarts"))
        } else {
            Ok(format!("{target} was already disabled"))
        }
    }
}

/// What the operations loop observed of one iteration.
#[derive(Debug, Clone)]
pub struct IterationFacts<'a> {
    pub at: Timestamp,
    pub cycle: u64,
    pub cycle_started_at: Timestamp,
    /// Wall time of the cycle plus everything run in the gap after it.
    pub iteration_ms: f64,
    pub problems: usize,
    /// Anomalies the research sweep found, where one ran.
    pub anomalies: Option<usize>,
    /// Models past their drift threshold, by reference.
    pub drifted: &'a [String],
    /// Whether the learning round promoted a model package this iteration.
    pub package_promoted: bool,
}

/// An incident: a level shift, its timeline and what remediation did.
#[derive(Debug, Clone)]
pub struct Incident {
    pub id: String,
    pub shift: Anomaly,
    pub timeline: Vec<TimelineEvent>,
    /// The remediation's result, or the refusal's reason.
    pub remediation: std::result::Result<String, String>,
}

/// What the loop did this iteration.
#[derive(Debug, Clone, Default)]
pub struct OperationsRound {
    pub incident: Option<Incident>,
    /// `succeeded` or `failed` where the cycle attempted a quantum solve.
    pub quantum: Option<&'static str>,
    /// Critical-path objectives measured and missed this iteration.
    pub objectives_missed: Vec<String>,
}

impl OperationsRound {
    pub fn describe(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(incident) = &self.incident {
            parts.push(format!(
                "incident {}: {} ({} timeline event(s)); remediation: {}",
                incident.id,
                TimelineEvent::from(&incident.shift).detail,
                incident.timeline.len(),
                match &incident.remediation {
                    Ok(done) => done.clone(),
                    Err(why) => format!("refused: {why}"),
                }
            ));
        }
        if let Some(outcome) = self.quantum {
            parts.push(format!("quantum job {outcome}"));
        }
        if !self.objectives_missed.is_empty() {
            parts.push(format!(
                "objectives missed: {}",
                self.objectives_missed.join(", ")
            ));
        }
        (!parts.is_empty()).then(|| format!("operations: {}", parts.join("; ")))
    }
}

/// The quantum outcome a cycle's journalled routing records, if a job
/// resolved in it.
///
/// Since the quantum arm went submit-and-poll, the cycle that attempts QAOA
/// is not the cycle that learns how it went: a submission or a job still out
/// is not an outcome, and counting it as a failure — what reading the
/// "attempting QAOA" note did — would page operations for every job in
/// flight. So the outcome is read from the typed dispatch record: a harvested
/// job whose answer was judged and ran succeeded; a harvested job dropped as
/// stale or refused by the judge, an abandoned one and a lost one failed.
pub fn quantum_outcome(
    routing: &qip_kernel::platform::SolverRoutingJournal,
) -> Option<&'static str> {
    let dispatch = &routing.quantum_dispatch;
    if dispatch.harvested.is_some() {
        return Some(if routing.ran.iter().any(|s| s == QUANTUM_SOLVER) {
            "succeeded"
        } else {
            "failed"
        });
    }
    (dispatch.abandoned.is_some() || dispatch.lost.is_some()).then_some("failed")
}

#[derive(Debug)]
pub struct Operations {
    deployment: EntityKey,
    graph: EntityGraph,
    iterations: VecDeque<(Timestamp, f64)>,
    timeline: VecDeque<TimelineEvent>,
    remediator: Remediator<CapabilitySwitch>,
    incidents: u64,
    /// The end of the last shift an incident was opened on, so one shift is
    /// one incident however many iterations still see it in the window.
    last_shift_end: Option<Timestamp>,
    fills_read: usize,
    /// When the last model package was promoted (R12-OBS-9151).
    package_promoted_at: Option<Timestamp>,
}

impl Operations {
    pub fn new(started: Timestamp) -> Result<Self> {
        let mut timeline = VecDeque::new();
        timeline.push_back(TimelineEvent {
            source: TimelineSource::Deployment,
            at: started,
            detail: "qip-deepbrain process started".to_string(),
        });
        Ok(Self {
            deployment: EntityKey::new("deployment", "qip-deepbrain"),
            graph: EntityGraph::with_retention(GRAPH_RETENTION)?,
            iterations: VecDeque::new(),
            timeline,
            remediator: Remediator::new(runbooks(), CapabilitySwitch::default())?,
            incidents: 0,
            last_shift_end: None,
            fills_read: 0,
            package_promoted_at: None,
        })
    }

    /// Whether a runbook has switched `capability` off.
    pub fn is_disabled(&self, capability: &str) -> bool {
        self.remediator.effector().is_disabled(capability)
    }

    pub const fn graph(&self) -> &EntityGraph {
        &self.graph
    }

    pub fn remediator(&self) -> &Remediator<CapabilitySwitch> {
        &self.remediator
    }

    fn push_event(&mut self, event: TimelineEvent) {
        if self.timeline.len() == TIMELINE_RETAINED {
            self.timeline.pop_front();
        }
        self.timeline.push_back(event);
    }

    fn signal(
        &mut self,
        class: SignalClass,
        name: &str,
        value: f64,
        at: Timestamp,
        extra: &[EntityKey],
    ) -> Result<()> {
        let mut entities = vec![self.deployment.clone()];
        entities.extend_from_slice(extra);
        self.graph.record(
            &Signal {
                class,
                name: name.to_string(),
                value,
                at,
            },
            &entities,
        )
    }

    /// Attach scored fills — `(venue, error in bps, scored at)` — to the
    /// graph as trading behaviour, on the venue and the deployment, and put
    /// a fill past [`VENUE_BEHAVIOUR_BPS`] on the timeline.
    pub fn observe_fills(
        &mut self,
        fills: &[(String, f64, Timestamp)],
        at: Timestamp,
    ) -> Result<()> {
        for (venue, bps, scored_at) in fills {
            let key = EntityKey::new("venue", venue.as_str());
            self.signal(
                SignalClass::TradingBehaviour,
                "fill_error_bps",
                *bps,
                at,
                std::slice::from_ref(&key),
            )?;
            if bps.abs() > VENUE_BEHAVIOUR_BPS {
                self.push_event(TimelineEvent {
                    source: TimelineSource::VenueBehaviour,
                    at: *scored_at,
                    detail: format!("{venue} filled {bps:.1} bps away from the twin"),
                });
            }
        }
        Ok(())
    }

    /// Observe one iteration: feed the graph, look for a shift, open and
    /// remediate an incident, record the quantum outcome and the objectives.
    pub fn observe(
        &mut self,
        platform: &Platform,
        facts: &IterationFacts<'_>,
    ) -> Result<OperationsRound> {
        let metrics = platform.telemetry().metrics.clone();
        let mut round = OperationsRound::default();
        let at = facts.at;

        self.signal(
            SignalClass::Infrastructure,
            "iteration_ms",
            facts.iteration_ms,
            at,
            &[],
        )?;
        self.signal(
            SignalClass::Application,
            "stage_problems",
            facts.problems as f64,
            at,
            &[],
        )?;
        if let Some(anomalies) = facts.anomalies {
            self.signal(
                SignalClass::DataQuality,
                "anomalies",
                anomalies as f64,
                at,
                &[],
            )?;
            if anomalies > 0 {
                self.push_event(TimelineEvent {
                    source: TimelineSource::DataAnomaly,
                    at,
                    detail: format!("{anomalies} price anomaly(ies) in cycle {}", facts.cycle),
                });
            }
        }
        for reference in facts.drifted {
            let model = EntityKey::new("model", reference.as_str());
            self.signal(
                SignalClass::ModelHealth,
                "drifted",
                1.0,
                at,
                std::slice::from_ref(&model),
            )?;
            self.push_event(TimelineEvent {
                source: TimelineSource::ModelDrift,
                at,
                detail: format!("{reference} drifted past its threshold"),
            });
        }
        let fills = platform.fill_scores();
        if fills.len() < self.fills_read {
            self.fills_read = 0;
        }
        let fresh: Vec<(String, f64, Timestamp)> = fills[self.fills_read..]
            .iter()
            .filter_map(|f| {
                f.trade_error_bps
                    .map(|bps| (f.venue.clone(), bps, f.scored_at))
            })
            .collect();
        self.fills_read = fills.len();
        self.observe_fills(&fresh, at)?;
        metrics.gauge(
            names::AIOPS_ENTITIES,
            labels([]),
            self.graph.node_count() as f64,
        );
        // Model freshness, recorded every iteration so it grows between
        // promotions: a gauge written only at a promotion would read zero
        // for ever after it, the stalest model looking the freshest.
        if facts.package_promoted {
            self.package_promoted_at = Some(at);
        }
        if let Some(promoted) = self.package_promoted_at {
            metrics.gauge(
                names::MODEL_PACKAGE_AGE,
                labels([]),
                at.since(promoted).as_secs_f64(),
            );
        }

        if self.iterations.len() == SERIES_RETAINED {
            self.iterations.pop_front();
        }
        self.iterations.push_back((at, facts.iteration_ms));
        if self.iterations.len() >= 2 * SHIFT_HALF {
            let points: Vec<(Timestamp, f64)> = self.iterations.iter().copied().collect();
            if let Some(shift) =
                detect_level_shift("qip_deepbrain_iteration_ms", &points, SHIFT_HALF)?
                && self
                    .last_shift_end
                    .is_none_or(|end| shift.window_start > end)
            {
                round.incident = Some(self.open_incident(shift, at, &metrics)?);
            }
        }

        if let Some(routing) = self.cycle_routing(platform, facts)?
            && let Some(outcome) = quantum_outcome(&routing)
        {
            metrics.count(names::QUANTUM_JOBS, labels([("outcome", outcome)]));
            round.quantum = Some(outcome);
        }

        // The node's paths are evaluated by the node, against the registry
        // that records their series; here they would read unmeasured forever.
        let statuses = critical_paths::evaluate_where(&metrics.snapshot(), |path| {
            path.slo.service != critical_paths::EDGE_NODE_SERVICE
        });
        critical_paths::record(&metrics, &statuses);
        round.objectives_missed = statuses
            .iter()
            .filter(|s| s.is_observed() && !s.is_met)
            .map(|s| s.slo.name.clone())
            .collect();
        Ok(round)
    }

    fn open_incident(
        &mut self,
        shift: Anomaly,
        at: Timestamp,
        metrics: &Metrics,
    ) -> Result<Incident> {
        self.incidents += 1;
        let id = format!("INC-deepbrain-{:06}", self.incidents);
        self.last_shift_end = Some(shift.window_end);
        self.push_event(TimelineEvent::from(&shift));
        metrics.count(names::AIOPS_INCIDENTS, labels([]));
        let events: Vec<TimelineEvent> = self.timeline.iter().cloned().collect();
        // From an hour before the shift began, so what led into it is on the
        // same page as the shift itself.
        let timeline = build_timeline(
            shift.window_start.saturating_sub(Duration::from_hours(1)),
            at,
            &events,
        )?;
        let action = "disable_capability";
        let remediation = match self.remediator.remediate(&id, action, "research") {
            Ok(done) => {
                metrics.count(
                    names::AIOPS_REMEDIATIONS,
                    labels([("action", action), ("outcome", "applied")]),
                );
                Ok(done)
            }
            Err(refused) => {
                metrics.count(
                    names::AIOPS_REMEDIATIONS,
                    labels([("action", action), ("outcome", "refused")]),
                );
                Err(refused.message().to_string())
            }
        };
        Ok(Incident {
            id,
            shift,
            timeline,
            remediation,
        })
    }

    /// The cycle's journalled solver routing, read back from the log rather
    /// than from a field the kernel would have to expose for this.
    fn cycle_routing(
        &self,
        platform: &Platform,
        facts: &IterationFacts<'_>,
    ) -> Result<Option<qip_kernel::platform::SolverRoutingJournal>> {
        use qip_events::EventBody;
        let filter = qip_events::EventFilter::new()
            .topic(qip_kernel::CycleJournalEntry::TOPIC)
            .between(facts.cycle_started_at, Timestamp::MAX);
        for envelope in platform.replay_journal(&filter)? {
            let Ok(entry) = envelope.decode::<qip_kernel::CycleJournalEntry>() else {
                continue;
            };
            if entry.body.cycle == facts.cycle {
                return Ok(entry.body.solver_routing);
            }
        }
        Ok(None)
    }
}
