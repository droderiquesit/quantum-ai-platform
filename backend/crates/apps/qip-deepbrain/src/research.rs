//! The deep brain's research loop: the expansion engine, the ambient mesh and
//! value-of-information sensing, run between cycles over what the platform
//! recorded (EXPAND-011/025/046..051/063..069, REASON-3061/3062,
//! WORLD-3075, AMBIENT-3104/7200).
//!
//! # Why this exists
//!
//! Each piece was built and tested as a library and none had a caller. The
//! curriculum ranked candidates nobody produced; `anomaly_to_observation`
//! mapped anomalies nobody detected here; the `AttentionRouter` routed
//! signals nothing emitted; `InfoRequest` priced acquisitions nothing
//! launched. A control with no caller reads as a capability and is not one,
//! which is the failure this file closes for all four at once, because they
//! are one loop: a detection is an ambient signal, the router decides what
//! wakes, what wakes becomes research, and research that needs evidence asks
//! what the cheapest discriminating observation is worth.
//!
//! # What it may not do
//!
//! Nothing here acts on a book. It reads `&Platform` — never `&mut` — so it
//! cannot place, size, or cancel anything, and every output the mesh
//! produces is an [`Advisory`]: there is no `PromotionCase` constructed in
//! this file, so nothing here can reach [`qip_contracts::ambient::promote`].
//! A queued research item is a record; a "launched" information request is a
//! record that the next sweep re-reads, or a record that an acquisition is
//! owed.

use qip_contracts::ambient::{
    AcquisitionCriteria, Advisory, AmbientSignal, AttentionRouter, Detection, Disposition,
    InfoRequest, Pathway, RoutingPolicy, SignalClass, Trigger,
};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_expansion::campaigns::{TickCampaignDetector, TickCampaignTrigger};
use qip_expansion::curriculum::Budget;
use qip_expansion::desk::{
    Admission, DeskConfig, ExpansionDesk, SweepReport, declared_cost, task_kind,
};
use qip_expansion::detectors::{
    Decline, PersistenceTracker, Reading, Refusal, Residual, anomaly_clusters, contradictions,
    coverage, incompetent_specialists, low_confidence, refusal_clusters, repeated_vetoes,
    split_by_wanting, unexplained_residuals,
};
use qip_expansion::gap::{GapTrigger, Observation, RaisedGap, raise};
use qip_kernel::Platform;
use qip_observability::metrics::{Metrics, labels, names};
use qip_opportunity_engine::detector::{DetectionContext, DetectorRegistry};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::now::NowEstimator;

/// Prices per subject the anomaly sweep reads. The detectors look at recent
/// windows; handing them a whole session's history costs a copy per cycle
/// and buys nothing.
pub const SWEEP_PRICES: usize = 256;
/// Resolved predictions before a hit rate is read as a finding.
pub const MIN_EVALUATED: usize = 20;
/// The hit rate below which a class or agent is a gap: worse than a coin.
pub const HIT_RATE_FLOOR: f64 = 0.5;
/// Regretted declines at one gate before it is a learning objective.
pub const MIN_REGRETS: usize = 3;
/// Refusals of one constraint at one venue before it is an execution gap.
pub const MIN_REFUSALS: usize = 3;
/// Mean fill miss, in basis points, past which a venue's twin is a gap and
/// past which one fill opens a tick-replay campaign.
pub const RESIDUAL_BPS: f64 = 25.0;
/// Subjects sharing a symptom before it is surfaced as an unknown-unknown.
pub const CLUSTER_SUBJECTS: usize = 3;
/// Consecutive sweeps a subject's discrepancy must persist.
pub const PERSIST_SWEEPS: u32 = 3;
/// Share of an asset class's instruments that must have history.
pub const COVERAGE_FLOOR: f64 = 0.5;
/// Lowest net value, in basis points, an information request may launch at.
pub const MIN_VIABLE_BP: u32 = 1_000;
/// The urgency a gap's information request is ranked at, which this loop
/// does not measure: no detector here knows a decision's deadline.
///
/// The rank is §9.10's product (R12-AMBIENT-3106, `net_value_bp`), so an
/// unmeasured factor must enter as the product's identity, the full scale,
/// and not as zero. Zero annihilates: this loop stated urgency as `0` when
/// the rank was an additive 40/40/20 sum, in which zero is the neutral
/// contribution, and once the rank became the product that same `0` scored
/// every request at nothing, so no gap — however significant — could launch
/// one, and the value-of-information path this file exists to give a caller
/// had none again. Full scale claims no urgency: it removes time from the
/// ranking, leaving significance, uncertainty and cost to decide, and the
/// request's deadline is what still stops an acquisition that runs too long.
/// A stated zero elsewhere — a detector's economic value — stays zero and
/// ranks nothing, which is the product working.
pub const UNMEASURED_URGENCY_BP: u32 = qip_contracts::ambient::MAX_SEVERITY_BP;
/// The cost of the cheapest discriminating observation the loop has: reading
/// the next sweep's prices again. Basis points of the decision's value.
pub const RESWEEP_COST_BP: u32 = 500;
/// Advisories and information requests kept for the cycle line.
pub const RETAINED: usize = 64;

/// How the loop is configured. Read in `main.rs`; nothing here reads the
/// environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchConfig {
    /// Sweep every this many cycles. Zero disables the loop.
    pub every_cycles: u64,
    /// The jurisdiction research is filed under; admission refuses it unless
    /// the discovery desk's eligible set contains it.
    pub jurisdiction: String,
    /// Compute units one sweep may start research within.
    pub compute_per_sweep: Decimal,
    /// How long an ambient signal and an information request stay live.
    pub signal_ttl: Duration,
}

impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            every_cycles: 1,
            jurisdiction: "US".to_string(),
            compute_per_sweep: Decimal::from_int(10),
            signal_ttl: Duration::from_hours(1),
        }
    }
}

impl ResearchConfig {
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let mut config = Self::default();
        if let Some(raw) = lookup("QIP_DEEPBRAIN_RESEARCH_EVERY") {
            config.every_cycles = raw.trim().parse().map_err(|_| {
                Error::invalid(format!(
                    "QIP_DEEPBRAIN_RESEARCH_EVERY is {raw:?}, not a cycle count; 0 disables the research loop"
                ))
            })?;
        }
        if let Some(raw) = lookup("QIP_DEEPBRAIN_RESEARCH_JURISDICTION") {
            if raw.trim().is_empty() {
                return Err(Error::invalid(
                    "QIP_DEEPBRAIN_RESEARCH_JURISDICTION is set but blank; unset it for the default or name one",
                ));
            }
            config.jurisdiction = raw.trim().to_string();
        }
        if let Some(raw) = lookup("QIP_DEEPBRAIN_RESEARCH_COMPUTE") {
            let units = Decimal::parse(raw.trim()).filter(|d| !d.is_negative()).ok_or_else(|| {
                Error::invalid(format!(
                    "QIP_DEEPBRAIN_RESEARCH_COMPUTE is {raw:?}, not a non-negative number of compute units"
                ))
            })?;
            config.compute_per_sweep = units;
        }
        Ok(config)
    }
}

/// Instruments per asset class, for the coverage detector. Built in `main`
/// from the universe before it moves into the platform, which does not
/// expose it afterwards.
pub fn coverage_map(
    universe: &qip_financial::universe::Universe,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for object in universe.iter() {
        map.entry(format!("{:?}", object.asset_class))
            .or_default()
            .insert(object.object_id.to_string());
    }
    map
}

/// The ambient class a gap's trigger is.
pub const fn signal_class(trigger: GapTrigger) -> SignalClass {
    match trigger {
        GapTrigger::Surprise => SignalClass::Surprise,
        GapTrigger::MissedOpportunity => SignalClass::Opportunity,
        GapTrigger::UnexplainedPnl | GapTrigger::Contradiction => SignalClass::Risk,
        GapTrigger::LowConfidence | GapTrigger::ModelDrift | GapTrigger::UnsupportedRequest => {
            SignalClass::Assumption
        }
    }
}

const fn pathway_str(pathway: Pathway) -> &'static str {
    match pathway {
        Pathway::ResearchTask => "research_task",
        Pathway::SpecialistActivation => "specialist_activation",
        Pathway::RiskReview => "risk_review",
        Pathway::CapitalPreparation => "capital_preparation",
        Pathway::ReflexModelUpdate => "reflex_model_update",
    }
}

const fn disposition_str(disposition: Disposition) -> &'static str {
    match disposition {
        Disposition::Activated => "activated",
        Disposition::Deferred => "deferred",
        Disposition::Shed => "shed",
        Disposition::Expired => "expired",
    }
}

const fn trigger_str(trigger: GapTrigger) -> &'static str {
    match trigger {
        GapTrigger::Surprise => "surprise",
        GapTrigger::LowConfidence => "low_confidence",
        GapTrigger::Contradiction => "contradiction",
        GapTrigger::MissedOpportunity => "missed_opportunity",
        GapTrigger::UnexplainedPnl => "unexplained_pnl",
        GapTrigger::ModelDrift => "model_drift",
        GapTrigger::UnsupportedRequest => "unsupported_request",
    }
}

const fn campaign_str(trigger: TickCampaignTrigger) -> &'static str {
    match trigger {
        TickCampaignTrigger::UnexplainedSlippage => "unexplained_slippage",
        TickCampaignTrigger::FillPredictionError => "fill_prediction_error",
        TickCampaignTrigger::QueuePredictionError => "queue_prediction_error",
        TickCampaignTrigger::AbnormalLeadLag => "abnormal_lead_lag",
        TickCampaignTrigger::WorldEventReaction => "world_event_reaction",
        TickCampaignTrigger::PaperReplayDivergence => "paper_replay_divergence",
        TickCampaignTrigger::NewVenue => "new_venue",
    }
}

/// A fraction in `[0, 1]` as basis points. The input is validated upstream
/// (`GapSignal::validate` refuses a severity outside it); a value outside is
/// refused here too rather than clamped.
/// The cheapest observation worth launching for a gap, and the criteria it
/// is ranked on: re-reading the next sweep when an anomaly raised the gap,
/// then the class's declared acquisition, each kept only if its §9.10 rank
/// clears [`MIN_VIABLE_BP`]. `None` is a gap no observation is worth paying
/// for — including every gap whose economic significance is a stated zero.
fn cheapest_viable(
    economic_bp: u32,
    uncertainty_bp: u32,
    anomaly_origin: bool,
    acquisition_bp: u32,
) -> Option<(&'static str, AcquisitionCriteria)> {
    let mut options = vec![("acquisition", acquisition_bp)];
    if anomaly_origin {
        options.insert(0, ("resweep", RESWEEP_COST_BP));
    }
    options.into_iter().find_map(|(source, cost)| {
        AcquisitionCriteria::new(economic_bp, uncertainty_bp, UNMEASURED_URGENCY_BP, cost)
            .ok()
            .filter(|c| c.net_value_bp() >= MIN_VIABLE_BP)
            .map(|c| (source, c))
    })
}

fn basis_points(fraction: f64) -> Result<u32> {
    if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
        return Err(Error::invalid(format!(
            "{fraction} is not a fraction in [0, 1]; a severity outside it is a detector bug"
        )));
    }
    // Statistic to integer: rounding a fraction to the nearest basis point.
    Ok((fraction * 10_000.0).round() as u32)
}

/// What one sweep did, for the cycle line.
#[derive(Debug, Clone, Default)]
pub struct ResearchRound {
    pub anomalies: usize,
    pub raised: usize,
    pub unclassified: usize,
    pub admissions: BTreeMap<&'static str, usize>,
    pub attention: BTreeMap<(&'static str, &'static str), usize>,
    pub info_launched: usize,
    pub info_halted: usize,
    pub campaigns_opened: usize,
    pub desk: SweepReport,
    pub now: BTreeMap<&'static str, (f64, f64)>,
    /// NOW Brain surprises read this sweep, and how many the kernel's queue
    /// had already dropped because this loop fell behind.
    pub now_surprises: usize,
    pub now_surprises_lost: u64,
    /// Specialists a NOW surprise woke, by specialist (AMBIENT-3079).
    pub now_wakes: BTreeMap<&'static str, usize>,
}

impl ResearchRound {
    pub fn describe(&self) -> String {
        format!(
            "research: {} anomaly(ies), {} gap(s) raised ({} unclassifiable), admissions {:?}; \
             {} started, {} closed, {} withdrawn, {} in queue; {} info request(s) launched, {} \
             halted; {} tick campaign(s) opened; {} NOW surprise(s) woke {:?}; outputs \
             advisory, none promoted",
            self.anomalies,
            self.raised,
            self.unclassified,
            self.admissions,
            self.desk.started.len(),
            self.desk.closed.len(),
            self.desk.withdrawn.len(),
            self.desk.queued,
            self.info_launched,
            self.info_halted,
            self.campaigns_opened,
            self.now_surprises,
            self.now_wakes,
        )
    }
}

/// The loop's state across sweeps.
pub struct ResearchLoop {
    config: ResearchConfig,
    desk: ExpansionDesk,
    router: AttentionRouter,
    detectors: DetectorRegistry,
    persistence: PersistenceTracker,
    ticks: TickCampaignDetector,
    now_estimate: NowEstimator,
    coverage: BTreeMap<String, BTreeSet<String>>,
    /// Signals routed but not yet activated, by signal id. Bounded by the
    /// router's hold queue: an entry leaves when its signal is activated,
    /// shed, expired or found below materiality.
    pending: BTreeMap<String, RaisedGap>,
    /// NOW surprise signals routed but not yet activated, by signal id, with
    /// the specialist each would wake. Bounded like `pending`.
    pending_wakes: BTreeMap<String, qip_kernel::now_stage::Specialist>,
    /// The kernel NOW queue's sequence number this loop has read through.
    surprises_seen: u64,
    issued: u64,
    fills_read: usize,
    advisories: VecDeque<Advisory<String>>,
    info_requests: VecDeque<InfoRequest>,
}

impl std::fmt::Debug for ResearchLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResearchLoop")
            .field("every_cycles", &self.config.every_cycles)
            .field("queued", &self.desk.queue().len())
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

/// Tools this node actually has, by the names `plan::AcquisitionAction`
/// resolves them by. Each is a desk in this binary: the discovery desk, the
/// learning desk's fit, the platform's stress tester. Anything else a gap
/// asks for is a tool to build, and is queued as one.
pub fn registered_tools() -> Result<qip_agents::tools::ToolRegistry> {
    use qip_agents::tools::ToolKind;
    let mut tools = qip_agents::tools::ToolRegistry::new();
    tools.register("source-discovery", ToolKind::Connector)?;
    tools.register("model-fit", ToolKind::ModelEndpoint)?;
    tools.register("stress-simulator", ToolKind::Simulator)?;
    Ok(tools)
}

impl ResearchLoop {
    pub fn new(
        config: ResearchConfig,
        coverage: BTreeMap<String, BTreeSet<String>>,
    ) -> Result<Self> {
        let desk = ExpansionDesk::new(
            DeskConfig {
                quiet_sweeps: 3,
                max_started_sweeps: 24,
                jurisdiction: config.jurisdiction.clone(),
                owner: "qip-deepbrain/research".to_string(),
            },
            registered_tools()?,
        )?;
        // Materiality 10%: a detection weaker than that is below the bar and
        // wakes nothing. 256 activations an hour, 256 held: far above what a
        // sweep raises today, so the budget binds only in a storm, which is
        // when it is meant to.
        let policy = RoutingPolicy::standard(1_000, 256, Duration::from_hours(1), 256)?;
        Ok(Self {
            desk,
            router: AttentionRouter::new(policy),
            detectors: DetectorRegistry::standard(),
            persistence: PersistenceTracker::new(PERSIST_SWEEPS)?,
            ticks: TickCampaignDetector::new()?,
            now_estimate: NowEstimator::new(0.001, 0.01)?,
            coverage,
            pending: BTreeMap::new(),
            pending_wakes: BTreeMap::new(),
            surprises_seen: 0,
            issued: 0,
            fills_read: 0,
            advisories: VecDeque::new(),
            info_requests: VecDeque::new(),
            config,
        })
    }

    pub const fn desk(&self) -> &ExpansionDesk {
        &self.desk
    }

    pub fn advisories(&self) -> impl Iterator<Item = &Advisory<String>> {
        self.advisories.iter()
    }

    pub fn info_requests(&self) -> impl Iterator<Item = &InfoRequest> {
        self.info_requests.iter()
    }

    pub fn now(&self) -> &NowEstimator {
        &self.now_estimate
    }

    /// Run a sweep if this cycle is on the cadence.
    pub fn maybe_sweep(
        &mut self,
        platform: &Platform,
        cycle: u64,
        now: Timestamp,
        licensed_sources: &BTreeSet<String>,
        eligible_jurisdictions: &BTreeSet<String>,
    ) -> Result<Option<ResearchRound>> {
        if self.config.every_cycles == 0 || !cycle.is_multiple_of(self.config.every_cycles) {
            return Ok(None);
        }
        self.sweep(platform, now, licensed_sources, eligible_jurisdictions)
            .map(Some)
    }

    /// One sweep over the platform's recorded state.
    pub fn sweep(
        &mut self,
        platform: &Platform,
        now: Timestamp,
        licensed_sources: &BTreeSet<String>,
        eligible_jurisdictions: &BTreeSet<String>,
    ) -> Result<ResearchRound> {
        let metrics = platform.telemetry().metrics.clone();
        let mut round = ResearchRound::default();
        let mut observations: Vec<Observation> = Vec::new();

        // --- the anomaly path: the deep brain's own sweep over prices -----
        let mut context = DetectionContext::new(now);
        for (subject, prices) in platform.price_history() {
            let from = prices.len().saturating_sub(SWEEP_PRICES);
            context = context.with_prices(subject.clone(), prices[from..].to_vec());
        }
        let anomalies = self.detectors.detect_all(&context);
        round.anomalies = anomalies.len();
        let mut members: Vec<(String, Observation)> = Vec::new();
        let mut readings: Vec<Reading> = Vec::new();
        let mut flagged: BTreeSet<String> = BTreeSet::new();
        let mut by_detector: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for anomaly in &anomalies {
            flagged.insert(anomaly.subject.clone());
            by_detector
                .entry(anomaly.detector.as_str())
                .or_default()
                .insert(anomaly.subject.as_str());
            readings.push(Reading {
                subject: anomaly.subject.clone(),
                source: anomaly.detector.clone(),
                up: anomaly.observed > anomaly.expected,
            });
            for observation in
                split_by_wanting(crate::signal_mapper::anomaly_to_observation(anomaly))
            {
                members.push((anomaly.subject.clone(), observation));
            }
        }
        observations.extend(anomaly_clusters(&members, CLUSTER_SUBJECTS)?);
        observations.extend(members.into_iter().map(|(_, o)| o));
        observations.extend(contradictions(&readings));
        observations.extend(self.persistence.observe(&flagged));

        // --- calibration, declines, refusals, fills, coverage -------------
        if let Some(calibration) = platform.calibration() {
            observations.extend(low_confidence(
                &calibration.hit_rate_by_class,
                calibration.evaluated,
                MIN_EVALUATED,
                HIT_RATE_FLOOR,
            )?);
            observations.extend(incompetent_specialists(
                &calibration.hit_rate_by_agent,
                calibration.evaluated,
                MIN_EVALUATED,
                HIT_RATE_FLOOR,
            )?);
        }
        let declines: Vec<Decline> = platform
            .declined_scores()
            .iter()
            .map(|d| Decline {
                gate: d.gate.clone(),
                regret: d.regret,
            })
            .collect();
        observations.extend(repeated_vetoes(&declines, MIN_REGRETS)?);
        let refusals: Vec<Refusal> = platform
            .feasibility_refusals()
            .iter()
            .map(|r| Refusal {
                venue: r.venue.clone(),
                constraint: r.constraint.clone(),
            })
            .collect();
        observations.extend(refusal_clusters(&refusals, MIN_REFUSALS)?);
        let fills = platform.fill_scores();
        let residuals: Vec<Residual> = fills
            .iter()
            .filter_map(|f| {
                f.trade_error_bps.map(|bps| Residual {
                    subject: f.venue.clone(),
                    error_bps: bps,
                })
            })
            .collect();
        observations.extend(unexplained_residuals(&residuals, RESIDUAL_BPS)?);
        // A shrunken list means the platform retained fewer scores than were
        // read; read them all again rather than index past the end.
        if fills.len() < self.fills_read {
            self.fills_read = 0;
        }
        let threshold = Decimal::from_f64(RESIDUAL_BPS)
            .ok_or_else(|| Error::numeric("the residual threshold has no decimal form"))?;
        for fill in &fills[self.fills_read..] {
            let instruments = BTreeSet::from([fill.object_id.to_string()]);
            let mut opened: Vec<TickCampaignTrigger> = Vec::new();
            if let Some(c) = self.ticks.observe_venue(&fill.venue, instruments.clone()) {
                opened.push(c.trigger);
            }
            if let Some(bps) = fill.trade_error_bps
                && let Some(residual) = Decimal::from_f64(bps)
                && let Some(c) = self.ticks.observe_residual(
                    TickCampaignTrigger::FillPredictionError,
                    residual,
                    threshold,
                    Some(&fill.venue),
                    instruments,
                )?
            {
                opened.push(c.trigger);
            }
            for trigger in opened {
                round.campaigns_opened += 1;
                metrics.count(
                    names::TICK_CAMPAIGNS_OPENED,
                    labels([("trigger", campaign_str(trigger))]),
                );
            }
        }
        self.fills_read = fills.len();
        let covered: BTreeSet<String> = platform.price_history().keys().cloned().collect();
        observations.extend(coverage(&self.coverage, &covered, COVERAGE_FLOOR)?);

        // --- NOW: three domains from the readings already taken -----------
        let subjects = platform.price_history().len().max(1) as f64;
        let market: Vec<f64> = by_detector
            .values()
            .map(|s| s.len() as f64 / subjects)
            .collect();
        let mut execution: BTreeMap<&str, (f64, usize)> = BTreeMap::new();
        for r in &residuals {
            let slot = execution.entry(r.subject.as_str()).or_default();
            slot.0 += r.error_bps.abs();
            slot.1 += 1;
        }
        let execution: Vec<f64> = execution.values().map(|(s, n)| s / *n as f64).collect();
        let beliefs: Vec<f64> = platform
            .calibration()
            .map(|c| c.hit_rate_by_class.values().copied().collect())
            .unwrap_or_default();
        for (domain, values) in crate::now::DOMAINS
            .into_iter()
            .zip([market, execution, beliefs])
        {
            if values.is_empty() && self.now_estimate.estimate(domain).is_none() {
                continue;
            }
            let estimate = self.now_estimate.observe(domain, &values)?;
            metrics.gauge(
                names::NOW_STATE,
                labels([("domain", domain)]),
                estimate.mean,
            );
            metrics.gauge(
                names::NOW_UNCERTAINTY,
                labels([("domain", domain)]),
                estimate.std_dev(),
            );
            round
                .now
                .insert(domain, (estimate.mean, estimate.std_dev()));
        }

        // --- raise, emit, route, admit ------------------------------------
        let mut raised_now: Vec<RaisedGap> = Vec::new();
        for observation in observations {
            let trigger = observation.trigger;
            match raise(observation) {
                Ok(raised) => raised_now.push(raised),
                Err(_) => {
                    round.unclassified += 1;
                    metrics.count(
                        names::RESEARCH_UNCLASSIFIED,
                        labels([("trigger", trigger_str(trigger))]),
                    );
                }
            }
        }
        round.raised = raised_now.len();
        // The NOW Brain's surprises since the last sweep (AMBIENT-3079): each
        // becomes one signal per specialist it should wake, routed through
        // the same router and the same budget as every gap, so a storm of
        // surprises is deferred or shed on the record rather than waking
        // everything at once.
        let (surprises, newest, lost) = platform.now_brain().surprises_since(self.surprises_seen);
        round.now_surprises = surprises.len();
        round.now_surprises_lost = lost;
        let mut routable: Vec<(Option<RaisedGap>, AmbientSignal)> = Vec::new();
        for surprise in surprises {
            for (specialist, signal) in
                crate::now::surprise_signals(surprise, now, self.config.signal_ttl)?
            {
                self.pending_wakes
                    .insert(signal.id().to_string(), specialist);
                routable.push((None, signal));
            }
        }
        self.surprises_seen = newest;
        for raised in raised_now {
            let signal = self.emit(&raised, now)?;
            routable.push((Some(raised), signal));
        }
        for (raised, signal) in routable {
            if let Some(raised) = raised {
                self.pending.insert(signal.id().to_string(), raised);
            }
            let events = self.router.route(&signal, now)?;
            if !events.iter().any(|e| e.signal_id == signal.id()) {
                // Below materiality: not an activation and not a loss.
                self.pending.remove(signal.id());
                self.pending_wakes.remove(signal.id());
            }
            for event in events {
                let pathway = pathway_str(event.pathway);
                let disposition = disposition_str(event.disposition);
                *round.attention.entry((pathway, disposition)).or_default() += 1;
                metrics.count(
                    names::AMBIENT_ATTENTION,
                    labels([("pathway", pathway), ("disposition", disposition)]),
                );
                match event.disposition {
                    Disposition::Deferred => {}
                    Disposition::Shed | Disposition::Expired => {
                        self.pending.remove(&event.signal_id);
                        self.pending_wakes.remove(&event.signal_id);
                    }
                    Disposition::Activated => {
                        if let Some(specialist) = self.pending_wakes.remove(&event.signal_id) {
                            *round.now_wakes.entry(specialist.as_str()).or_default() += 1;
                            metrics.count(
                                crate::now::NOW_WAKES,
                                labels([("specialist", specialist.as_str())]),
                            );
                            continue;
                        }
                        let Some(gap) = self.pending.remove(&event.signal_id) else {
                            continue;
                        };
                        self.admit(
                            gap,
                            event.pathway,
                            &event.signal_id,
                            now,
                            &metrics,
                            &mut round,
                            licensed_sources,
                            eligible_jurisdictions,
                        )?;
                    }
                }
            }
        }

        // --- end the sweep -------------------------------------------------
        let report = self.desk.finish_sweep(Budget {
            compute: self.config.compute_per_sweep,
            capital: self.config.compute_per_sweep,
        })?;
        metrics.gauge(
            names::RESEARCH_QUEUE_ITEMS,
            labels([]),
            report.queued as f64,
        );
        metrics.increment(
            names::RESEARCH_ITEMS_STARTED,
            labels([]),
            report.started.len() as u64,
        );
        metrics.increment(
            names::RESEARCH_ITEMS_WITHDRAWN,
            labels([]),
            report.withdrawn.len() as u64,
        );
        for closed in &report.closed {
            let reason = match closed.reason {
                qip_expansion::desk::CloseReason::GapQuiet => "gap_quiet",
                qip_expansion::desk::CloseReason::OutOfTime => "out_of_time",
                qip_expansion::desk::CloseReason::Promoted => "promoted",
                qip_expansion::desk::CloseReason::NullRecorded => "null_recorded",
            };
            metrics.count(names::RESEARCH_ITEMS_CLOSED, labels([("reason", reason)]));
        }
        // A request launched as a re-sweep was answered by this sweep's
        // reading; one past its deadline halts.
        for request in &mut self.info_requests {
            if request.status() == qip_contracts::ambient::AcquisitionStatus::InProgress {
                request.complete()?;
            } else if request.should_halt(now, MIN_VIABLE_BP) && request.halt() {
                round.info_halted += 1;
            }
        }
        round.desk = report;
        Ok(round)
    }

    /// The ambient signal a raised gap is.
    fn emit(&mut self, raised: &RaisedGap, now: Timestamp) -> Result<AmbientSignal> {
        self.issued += 1;
        let class = signal_class(raised.trigger);
        let severity = basis_points(raised.signal.severity)?;
        let pathway = match class {
            SignalClass::Surprise => Pathway::SpecialistActivation,
            SignalClass::Opportunity => Pathway::ResearchTask,
            SignalClass::Risk => Pathway::RiskReview,
            SignalClass::Assumption => Pathway::ReflexModelUpdate,
        };
        AmbientSignal::new(
            format!("ambient-{}", self.issued),
            class,
            qip_expansion::desk::question_for(raised),
            severity,
            Trigger::Event(trigger_str(raised.trigger).to_string()),
            now,
            Detection {
                observed_deviation: Decimal::from_f64(raised.signal.severity)
                    .ok_or_else(|| Error::numeric("a severity has no decimal form"))?,
                expected_baseline: Decimal::ZERO,
                horizon: Duration::ZERO,
                novelty_bp: severity,
                affected_entities: raised.signal.affected_domains.clone(),
                urgency_bp: 0,
                wake_targets: vec![pathway],
                evidence_ids: raised.signal.evidence.clone(),
                expiry: now.saturating_add(self.config.signal_ttl),
            },
        )
    }

    /// An activated signal becomes research, an advisory, and — where the
    /// gap asks for evidence — a priced information request.
    #[allow(clippy::too_many_arguments)]
    fn admit(
        &mut self,
        gap: RaisedGap,
        pathway: Pathway,
        signal_id: &str,
        now: Timestamp,
        metrics: &Metrics,
        round: &mut ResearchRound,
        licensed_sources: &BTreeSet<String>,
        eligible_jurisdictions: &BTreeSet<String>,
    ) -> Result<()> {
        let class = gap.signal.gap_class;
        let question = qip_expansion::desk::question_for(&gap);
        let anomaly_origin = !gap
            .signal
            .evidence
            .first()
            .is_some_and(|e| e.starts_with("detector:"));
        let severity = basis_points(gap.signal.severity)?;
        let economic = gap.signal.economic_value;
        let admission = self
            .desk
            .offer(gap, licensed_sources, eligible_jurisdictions);
        let outcome = admission.as_str();
        *round.admissions.entry(outcome).or_default() += 1;
        metrics.count(
            names::RESEARCH_GAPS,
            labels([("class", class_str(class)), ("outcome", outcome)]),
        );
        if !matches!(admission, Admission::Queued | Admission::Blocked { .. }) {
            return Ok(());
        }
        if self.advisories.len() == RETAINED {
            self.advisories.pop_front();
        }
        self.advisories
            .push_back(Advisory::new(format!("{pathway:?}: {question}"), signal_id));

        // Value of information (AMBIENT-3104): what reducing this gap's
        // uncertainty is worth, against the cheapest observation that could
        // discriminate. Economic significance is the gap's economic value in
        // basis points — the anomaly mapper states it on [0, 100], so ×100
        // is its basis-point form, and a detector's stated zero stays zero.
        let economic_bp = basis_points(economic.to_f64() / 100.0)?;
        // Options in cost order: re-reading the next sweep is possible only
        // for a gap the price sweep raised; the acquisition the class asks
        // for is always an option, at its declared cost in thousands of
        // basis points.
        let acquisition_bp =
            u32::try_from(declared_cost(task_kind(class)).raw() / Decimal::ONE.raw())
                .unwrap_or(u32::MAX)
                .saturating_mul(1_000)
                .min(qip_contracts::ambient::MAX_SEVERITY_BP);
        let chosen = cheapest_viable(economic_bp, severity, anomaly_origin, acquisition_bp);
        let Some((source, criteria)) = chosen else {
            return Ok(());
        };
        let cost = criteria.acquisition_cost_bp;
        let mut request = InfoRequest::new(
            format!("info-{signal_id}"),
            question,
            criteria,
            now.saturating_add(self.config.signal_ttl),
            now,
        )?;
        if request.should_halt(now, MIN_VIABLE_BP) {
            request.halt();
            round.info_halted += 1;
            metrics.count(names::INFO_REQUESTS, labels([("outcome", "halted")]));
        } else {
            request.record_acquisition(cost)?;
            round.info_launched += 1;
            metrics.count(
                names::INFO_REQUESTS,
                labels([("outcome", "launched"), ("source", source)]),
            );
        }
        if self.info_requests.len() == RETAINED {
            self.info_requests.pop_front();
        }
        self.info_requests.push_back(request);
        Ok(())
    }
}

const fn class_str(class: qip_contracts::expansion::GapClass) -> &'static str {
    use qip_contracts::expansion::GapClass;
    match class {
        GapClass::Data => "data",
        GapClass::Ontology => "ontology",
        GapClass::Causal => "causal",
        GapClass::Memory => "memory",
        GapClass::Model => "model",
        GapClass::Tool => "tool",
        GapClass::Specialist => "specialist",
        GapClass::Execution => "execution",
        GapClass::ComputeQuantumResearch => "compute_quantum_research",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure this prevents has happened: the loop stated urgency as
    /// zero, which was harmless under the additive rank and, once the rank
    /// became §9.10's product, scored every request at zero — the
    /// value-of-information path had a caller and could launch nothing. A
    /// gap with half the decision's value at stake and 80% uncertainty is
    /// worth the 500 bp re-sweep: 0.5 × 0.8 × 1 × 0.95 = 3800 bp.
    #[test]
    fn a_gap_with_economic_value_launches_on_the_product_rank() {
        let chosen = cheapest_viable(5_000, 8_000, true, 2_000);
        let (source, criteria) = chosen.expect("a significant, uncertain gap is worth a re-sweep");
        assert_eq!(
            source, "resweep",
            "the cheapest viable observation is the re-sweep"
        );
        assert_eq!(criteria.net_value_bp(), 3_800);
        assert!(criteria.net_value_bp() >= MIN_VIABLE_BP);
    }

    /// The other half, so the test above is not passing on a rank that
    /// launches everything: the same gap with no economic significance — what
    /// every detector that measures no money states — is worth no
    /// observation at any cost, and neither is one the loop is certain of.
    #[test]
    fn a_gap_with_a_zero_factor_launches_nothing_at_any_cost() {
        assert!(cheapest_viable(0, qip_contracts::ambient::MAX_SEVERITY_BP, true, 0).is_none());
        assert!(cheapest_viable(qip_contracts::ambient::MAX_SEVERITY_BP, 0, true, 0).is_none());
    }
}
