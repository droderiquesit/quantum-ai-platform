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
//!
//! # What runs here since 2026-10-11 (lane R2-DEEP)
//!
//! * **Evidence.** The world fabric the connectors now feed
//!   (`Platform::absorb_evidence`) is scanned for poisoning every sweep; a
//!   finding is raised as a data gap naming the sources, so a poisoned feed
//!   becomes research into replacing it (R12-EVID-2039).
//! * **Curiosity.** Information requests are no longer launched one by one
//!   as their gaps wake: every candidate a sweep produces is ordered by
//!   `rank_acquisitions` — §9.10's product — and launched in that order
//!   within the sweep's acquisition budget; the rest are halted as
//!   outranked (R12-AMBIENT-3106).
//! * **The compute market.** Every open research item bids for the next
//!   unit of CPU, GPU, TPU or QPU; the Meta-Intelligence market grants by
//!   marginal value, the desk starts only what was granted, what each
//!   granted unit realised is measured the next sweep, and work that stops
//!   yielding is retired (R12-AMBIENT-7202, 3110..3114).
//! * **Research carried out.** A granted, started item's acquisition is
//!   carried out research-only, and a fill-bias campaign is run across the
//!   four environments, evaluated on nine criteria and concluded through
//!   the desk — promoted or its null recorded (R12-EXPAND-6025, 6049..6051).
//! * **The Cognitive Compiler.** A promoted fill-bias capability is
//!   formalised as a `SymbolicDerivation` and distilled into a bounded
//!   `ReflexPackage` (R12-REASON-7203, 3115). It is signed and published
//!   only through an installed `PackageSigner` (ADR 0110); without one it is
//!   held unsigned and nothing is written.

use qip_agents::registries::OntologyRegistry;
use qip_contracts::ambient::{
    AcquisitionCriteria, Advisory, AmbientSignal, AttentionEvent, AttentionRouter,
    AuthorityBoundary, ComputeResourceType, Detection, Disposition, InfoRequest, ModelReputation,
    Pathway, RoutingPolicy, SignalClass, Trigger, rank_acquisitions,
};
use qip_contracts::expansion::{CapabilityEntry, GapClass, OntologyTypeFamily, OntologyTypeSpec};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_expansion::campaigns::{self, TickCampaign, TickCampaignDetector, TickCampaignTrigger};
use qip_expansion::curriculum::{Budget, State, TaskKind};
use qip_expansion::desk::{
    Admission, DeskConfig, ExpansionDesk, SweepReport, declared_cost, task_kind,
};
use qip_expansion::detectors::{
    Decline, PersistenceTracker, Reading, Refusal, Residual, anomaly_clusters, contradictions,
    coverage, incompetent_specialists, low_confidence, model_drift, model_error_rate,
    refusal_clusters, repeated_vetoes_priced, split_by_wanting, unexplained_residuals,
};
use qip_expansion::executor::{
    AcquisitionOutcome, AcquisitionRecord, CampaignPolicy, CampaignResult, FillResidual, acquire,
    fill_venue, run_fill_bias_campaign,
};
use qip_expansion::gap::{GapTrigger, Observation, RaisedGap, Wanting, raise};
use qip_expansion::meta::{
    DomainResidual, MarginalValue, MetaIntelligence, MetaPolicy, ReputationKey, WorkBid, WorkKind,
    resource_str,
};
use qip_kernel::Platform;
use qip_kernel::evidence_intake::{PoisoningClass, PoisoningFinding, PoisoningThresholds};
use qip_observability::metrics::{Metrics, labels, names};
use qip_opportunity_engine::detector::{DetectionContext, DetectorRegistry};
use qip_reasoning_engine::compiler::{
    CognitiveCompiler, Compiled, CompilerBounds, Condition, Discovery, Finding as Established,
    PackageSigner, ReflexRule, SignedReflexPackage, Verification,
};
use qip_risk::limits::LimitCheck;
use qip_simulation_engine::scenario::{FactorShock, Scenario};
use qip_storage::BlobStore;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

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
/// Hosted-model replies counted before their error rate is judged, and the
/// share refused or malformed past which one surprise is raised. One in five
/// is far above a provider's ordinary failure rate and below the point where
/// the templates are doing most of the narrating unnoticed.
pub const MODEL_REPLIES_MIN: u64 = 20;
pub const MODEL_ERROR_RATE: f64 = 0.2;
/// Share of an asset class's instruments that must have history.
pub const COVERAGE_FLOOR: f64 = 0.5;
/// Lowest net value, in basis points, an information request may launch at.
pub const MIN_VIABLE_BP: u32 = 1_000;
/// The urgency a request with its whole window still ahead is ranked at,
/// in basis points: half scale (R12-AMBIENT-3106).
///
/// Urgency is derived from the gap's deadline by [`urgency_bp`] — the share
/// of the signal's window already consumed when the request is admitted,
/// rising to the full scale at the deadline — so two gaps equal in every
/// other factor rank the nearer deadline first. The rank is §9.10's product
/// (`net_value_bp`), so the fresh end of that scale cannot be zero: this
/// loop once stated urgency as `0` under an additive rank, and when the rank
/// became the product that same `0` scored every request at nothing, so no
/// gap — however significant — could launch one. Half scale is the honest
/// floor rather than the full scale this constant replaced: a request whose
/// window has only begun is not without urgency, because the window is the
/// signal's time to live and evidence that arrives after it answers a
/// question nobody is still asking; but it is less urgent than the same
/// request a sweep before its deadline, and the full-scale constant made the
/// two read alike.
pub const URGENCY_FLOOR_BP: u32 = qip_contracts::ambient::MAX_SEVERITY_BP / 2;

/// The urgency of acquiring evidence for a decision due at `deadline`, read
/// at `now` over a window of `horizon`: [`URGENCY_FLOOR_BP`] with the whole
/// window ahead, the full scale at (or past) the deadline, linear between.
/// A zero or absent horizon is a decision already due.
pub fn urgency_bp(now: Timestamp, deadline: Timestamp, horizon: Duration) -> u32 {
    let scale = qip_contracts::ambient::MAX_SEVERITY_BP;
    let window = horizon.as_millis();
    if window <= 0 || deadline <= now {
        return scale;
    }
    let remaining = deadline.since(now).as_millis().clamp(0, window);
    // Statistic to integer basis points: the consumed share of the window,
    // mapped onto [floor, scale]; both ends are reached exactly.
    let consumed = (window - remaining) as f64 / window as f64;
    let span = f64::from(scale - URGENCY_FLOOR_BP);
    URGENCY_FLOOR_BP + (consumed * span).round() as u32
}
/// The cost of the cheapest discriminating observation the loop has: reading
/// the next sweep's prices again. Basis points of the decision's value.
pub const RESWEEP_COST_BP: u32 = 500;
/// Advisories and information requests kept for the cycle line.
pub const RETAINED: usize = 64;
/// What one sweep may spend on information acquisitions, in basis points of
/// decision value, unless `QIP_DEEPBRAIN_RESEARCH_ACQUISITION_BP` says
/// otherwise: two acquisitions at the full decision's value.
pub const ACQUISITION_BUDGET_BP: u32 = 20_000;
/// The blob namespace signed reflex packages are published under.
pub const REFLEX_PACKAGES_NAMESPACE: &str = "reflex-packages";

/// Research acquisitions carried out, by `action` (the nine
/// `AcquisitionAction` names) and `outcome` (`gathered`, `not_executable`).
pub const RESEARCH_ACQUISITIONS: &str = "qip_research_acquisitions_total";
/// Research campaigns judged, by `outcome` (`promoted`, `null_recorded`,
/// `refused`, `waiting`).
pub const EXPANSION_CAMPAIGNS: &str = "qip_expansion_campaigns_total";
/// Compute units the market granted, by `resource`.
pub const META_UNITS_GRANTED: &str = "qip_meta_units_granted_total";
/// Bids the market declined, by `reason` (`Decline::as_str`).
pub const META_BIDS_DECLINED: &str = "qip_meta_bids_declined_total";
/// Work the market retired as persistently unproductive.
pub const META_WORK_RETIRED: &str = "qip_meta_work_retired_total";
/// The latest realised marginal value per unit, in basis points, by `kind`.
pub const META_MARGINAL_VALUE: &str = "qip_meta_marginal_value_bp";
/// Specialist populations proposed for an economically meaningful residual.
pub const META_SPECIALISTS_PROPOSED: &str = "qip_meta_specialists_proposed_total";
/// Reflex packages, by `outcome` (`held_unsigned`, `signed`, `published`,
/// `refused`).
pub const REFLEX_PACKAGES: &str = "qip_reflex_packages_total";

/// The severity a poisoning finding is raised at, by class: stated, since
/// the detector reports a pattern and not a magnitude. Spoofing — a source
/// whose own signals contradict its identity — is the least ambiguous.
pub const fn poisoning_severity(class: PoisoningClass) -> f64 {
    match class {
        PoisoningClass::Spoofing => 1.0,
        PoisoningClass::SourcePoisoning => 0.9,
        PoisoningClass::CoordinatedManipulation => 0.8,
        PoisoningClass::SyntheticContent => 0.7,
        PoisoningClass::Spam => 0.5,
    }
}

/// One fill the twin scored, as the research loop reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct FillObservation {
    pub venue: String,
    pub object_id: String,
    pub filled_at: qip_core::Timestamp,
    pub trade_error_bps: Option<f64>,
}

/// One paper fill's slippage against its arrival price.
#[derive(Clone, Debug, PartialEq)]
pub struct SlippageObservation {
    pub venue: String,
    pub object_id: String,
    pub slippage_bps: f64,
}

/// A compiled reflex package and, if a signer was installed, its signed form.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledReflex {
    pub question: String,
    pub compiled: Compiled,
    pub signed: Option<SignedReflexPackage>,
}

/// What the market granted a work item last sweep, awaiting its outcome.
#[derive(Clone, Debug, PartialEq)]
struct Granted {
    kind: WorkKind,
    units: u32,
    expected_bp: f64,
    domain: String,
    /// The reputation cell the grant was bid under (R12-AMBIENT-3111): the
    /// question's horizon bucket and the regime the platform classified at
    /// the grant, so the window's outcome is recorded against the cell it
    /// was promised in and not a pooled one.
    horizon: String,
    regime: String,
    /// The uncertainty the granted question carried at the grant, so the
    /// next sweep measures what the window reduced rather than what it
    /// counted (R12-AMBIENT-10189).
    uncertainty: Uncertainty,
}

/// The uncertainty a research question carries, as the market reads it at
/// a grant and again at the window's end. `None` inside either arm is a
/// figure the platform could not form — too few residuals, no calibration
/// for the class — and is kept as such, never read as zero.
#[derive(Clone, Debug, PartialEq)]
enum Uncertainty {
    /// A fill-bias item: the standard error of the twin's bias estimate at
    /// the venue, over every scored residual there.
    FillBias { standard_error: Option<f64> },
    /// Any other item: the calibration hit rate of the belief class the
    /// question is filed under.
    Calibration { hit_rate: Option<f64> },
}

impl Uncertainty {
    /// The share of `before`'s uncertainty this reading removed, in
    /// `[0, 1]`. A figure that could not be formed on one side is no
    /// reduction, except the one honest case of a fill estimate that went
    /// from unmeasurable to measured, which is the whole of the uncertainty
    /// it started with.
    fn reduction_from(&self, before: &Self) -> f64 {
        match (before, self) {
            (
                Self::FillBias {
                    standard_error: Some(b),
                },
                Self::FillBias {
                    standard_error: Some(a),
                },
            ) if *b > 0.0 && b.is_finite() && a.is_finite() => ((b - a) / b).clamp(0.0, 1.0),
            (
                Self::FillBias {
                    standard_error: None,
                },
                Self::FillBias {
                    standard_error: Some(a),
                },
            ) if a.is_finite() => 1.0,
            (Self::Calibration { hit_rate: Some(b) }, Self::Calibration { hit_rate: Some(a) })
                if *b < 1.0 && b.is_finite() && a.is_finite() =>
            {
                ((a - b) / (1.0 - b)).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

/// The standard error of the mean of `values`: the uncertainty of a bias
/// estimate made from them. `None` below two values, where no spread exists
/// to estimate from.
fn standard_error(values: impl Iterator<Item = f64>) -> Option<f64> {
    let values: Vec<f64> = values.filter(|v| v.is_finite()).collect();
    let n = values.len();
    if n < 2 {
        return None;
    }
    let mean = values.iter().sum::<f64>() / n as f64;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0);
    Some((variance / n as f64).sqrt())
}

/// The horizon bucket a question is filed under in the reputation book
/// (R12-AMBIENT-3111): the window left to its deadline when it was
/// admitted, in the coarse bands a research desk plans in.
pub fn horizon_bucket(remaining: Duration) -> &'static str {
    if remaining <= Duration::from_hours(1) {
        "hour"
    } else if remaining <= Duration::from_days(1) {
        "day"
    } else if remaining <= Duration::from_days(7) {
        "week"
    } else {
        "beyond_week"
    }
}

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
    /// What one sweep may spend on information acquisitions, in basis points.
    pub acquisition_budget_bp: u32,
    /// QPU units one sweep's market may grant. Zero by default: quantum
    /// research is off until somebody budgets it, and every grant it does
    /// make charges its classical baseline to the CPU budget (ADR 0006).
    pub qpu_per_sweep: Decimal,
    /// The most compute units each research agent — the reputation cell
    /// its bids are weighted by — may be charged in one market round,
    /// across every resource (R12-AMBIENT-9034). Empty by default: an agent
    /// the map does not name is bounded by the resource budgets alone.
    pub agent_budgets: BTreeMap<ReputationKey, Decimal>,
    /// Whether the sweep reads the DECIDE stage's pre-positioning plan and
    /// raises a capital-preparation signal per recommended movement
    /// (R12-AMBIENT-3072). On by default; a sweep with it off raises none.
    pub capital_sentinel: bool,
    /// How far ahead the sentinel asks the planner to look. A day by
    /// default, the horizon DECIDE plans over, so the two read one plan: a
    /// horizon shorter than a venue's settlement refuses every lane as
    /// settling too late, and the sentinel then reports nothing to move.
    pub capital_horizon: Duration,
}

/// The variable naming per-agent compute budgets:
/// `domain/horizon/regime=units`, comma-separated.
pub const AGENT_BUDGETS_VARIABLE: &str = "QIP_DEEPBRAIN_RESEARCH_AGENT_BUDGETS";

/// Parse [`AGENT_BUDGETS_VARIABLE`]. Every entry needs all three key parts
/// and a non-negative number of units; a malformed entry or a key named
/// twice is refused rather than skipped, because an agent whose bound was
/// silently dropped is an agent bounded by nothing.
pub fn parse_agent_budgets(raw: &str) -> Result<BTreeMap<ReputationKey, Decimal>> {
    let mut out = BTreeMap::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let refuse = || {
            Error::invalid(format!(
                "{AGENT_BUDGETS_VARIABLE} entry {entry:?} is not domain/horizon/regime=units with \
                 non-negative units"
            ))
        };
        let (key, units) = entry.split_once('=').ok_or_else(refuse)?;
        // The regime is the last part and may itself hold a `/` — the
        // platform's regime key is `market/volatility`.
        let parts: Vec<&str> = key.splitn(3, '/').collect();
        let [domain, horizon, regime] = parts.as_slice() else {
            return Err(refuse());
        };
        let units = Decimal::parse(units.trim())
            .filter(|d| !d.is_negative())
            .ok_or_else(refuse)?;
        let key = ReputationKey::new(domain, horizon, regime)?;
        if out.insert(key, units).is_some() {
            return Err(Error::invalid(format!(
                "{AGENT_BUDGETS_VARIABLE} names {domain}/{horizon}/{regime} twice; state one budget"
            )));
        }
    }
    Ok(out)
}

impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            every_cycles: 1,
            jurisdiction: "US".to_string(),
            compute_per_sweep: Decimal::from_int(10),
            signal_ttl: Duration::from_hours(1),
            acquisition_budget_bp: ACQUISITION_BUDGET_BP,
            qpu_per_sweep: Decimal::ZERO,
            agent_budgets: BTreeMap::new(),
            capital_sentinel: true,
            capital_horizon: CAPITAL_HORIZON,
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
        if let Some(raw) = lookup("QIP_DEEPBRAIN_RESEARCH_ACQUISITION_BP") {
            config.acquisition_budget_bp = raw.trim().parse().map_err(|_| {
                Error::invalid(format!(
                    "QIP_DEEPBRAIN_RESEARCH_ACQUISITION_BP is {raw:?}, not a whole number of basis \
                     points; 0 launches no acquisition"
                ))
            })?;
        }
        if let Some(raw) = lookup("QIP_DEEPBRAIN_RESEARCH_QPU") {
            config.qpu_per_sweep = Decimal::parse(raw.trim())
                .filter(|d| !d.is_negative())
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "QIP_DEEPBRAIN_RESEARCH_QPU is {raw:?}, not a non-negative number of QPU \
                         units; 0 grants no quantum research"
                    ))
                })?;
        }
        if let Some(raw) = lookup(AGENT_BUDGETS_VARIABLE) {
            config.agent_budgets = parse_agent_budgets(&raw)?;
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
    urgency_bp: u32,
    anomaly_origin: bool,
    acquisition_bp: u32,
) -> Option<(&'static str, AcquisitionCriteria)> {
    let mut options = vec![("acquisition", acquisition_bp)];
    if anomaly_origin {
        options.insert(0, ("resweep", RESWEEP_COST_BP));
    }
    options.into_iter().find_map(|(source, cost)| {
        AcquisitionCriteria::new(economic_bp, uncertainty_bp, urgency_bp, cost)
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
    /// Claims the world fabric held when the sweep scanned it.
    pub evidence_held: usize,
    /// Poisoning findings over the world fabric, by class.
    pub poisoning: BTreeMap<&'static str, usize>,
    /// Information requests a better-ranked request left no budget for.
    pub info_outranked: usize,
    /// Units granted and bids declined by the compute market, by reason.
    pub units_granted: u32,
    pub declined: BTreeMap<&'static str, usize>,
    /// Windows measured, and work retired as unproductive.
    pub measured: usize,
    pub retired: usize,
    pub specialists_proposed: usize,
    /// Acquisitions carried out, by outcome.
    pub acquisitions: BTreeMap<&'static str, usize>,
    /// Campaigns judged, by outcome.
    pub campaigns: BTreeMap<&'static str, usize>,
    /// Reflex packages, by outcome.
    pub packages: BTreeMap<&'static str, usize>,
    /// Every ambient signal this sweep routed, in routing order, for the
    /// ambient journal (`ambient_journal.rs`): a count on a cycle line is
    /// not a record anybody can replay.
    pub signals: Vec<AmbientSignal>,
    /// Every attention event the router returned this sweep, in order.
    pub attention_events: Vec<AttentionEvent>,
    /// The specialist each activated NOW-surprise signal woke, by signal id.
    pub woken: BTreeMap<String, &'static str>,
    /// The research question each woken specialist was given, by specialist
    /// (R12-AMBIENT-3079): a wake is a queued item the specialist's brain
    /// owns, not a counter.
    pub wake_items: BTreeMap<&'static str, String>,
    /// Risk limits the book is approaching — inside the limit, past its
    /// warning threshold — each raised this sweep as a risk-review signal
    /// before the limit fires (R12-AMBIENT-3070), by limit name.
    pub limit_warnings: BTreeMap<String, f64>,
    /// Pre-positioning movements the sweep recommended and the router
    /// activated for capital preparation (R12-AMBIENT-3065/3072): advice for
    /// the treasury, never a transfer.
    pub capital_advice: Vec<CapitalAdvice>,
    /// The evolution round that ran in this cycle's gap before the sweep,
    /// where one ran (R12-EXPAND-1117), so the sweep record shows the model
    /// evolution its research followed.
    pub evolution: Option<EvolutionTrace>,
    /// One conditional scenario per risk or assumption signal this sweep
    /// routed whose subject the platform holds a tape for
    /// (R12-AMBIENT-1005/3064): the measured move that raised the signal,
    /// stated as a shock to stress against, generated with nobody asking.
    pub scenarios: Vec<ConditionalScenario>,
    /// Every assumption the loop holds and how it stands after this sweep
    /// (R12-AMBIENT-1143), in key order: written every sweep, so an
    /// assumption last challenged three sweeps ago reads as holding for
    /// three sweeps rather than as nothing.
    pub assumptions: Vec<AssumptionState>,
    /// Every information request this sweep launched, in the Curiosity
    /// Engine's rank order, as the scout job the node hands the discovery
    /// desk (R12-AMBIENT-9033): a launch that only recorded its cost reached
    /// no scout, so the platform paid for evidence nothing went to find.
    pub scouted: Vec<ScoutRequest>,
    /// The registries the sweep consulted and what each held
    /// (R12-EXPAND-1115): a sweep record that cannot say which sources,
    /// types, tools and brains it ran against cannot be replayed against the
    /// same ones.
    pub registries: RegistryCounts,
    /// The sweep's started items compiled into a proven `qip_agency` plan
    /// (R12-REASON-3061): what the sweep chose, inside which goal, at what
    /// cost, citing which evidence. `None` where nothing started.
    pub plan: Option<crate::research_plan::SweepPlanRecord>,
    /// Why the plan could not be proven, where it could not: a sweep whose
    /// started items cost more than its budget is recorded, not trimmed.
    pub plan_refused: Option<String>,
    /// Every tick campaign concluded this sweep on its re-measured venue
    /// (R12-EXPAND-6042): the microstructure delta a learner reads, the
    /// delta's id being the campaign's.
    pub deltas: Vec<qip_contracts::microstructure_delta::MicrostructureDelta>,
}

/// Fills a campaign's venue must have scored since the campaign opened
/// before it is concluded on them. Fewer is a re-measurement too thin to
/// tell a residual from noise; the campaign stays open until it has them.
pub const MIN_REMEASURED: usize = 5;

/// One recommended pre-positioning movement, as the capital-preparation
/// pathway receives it (R12-AMBIENT-3065). An advisory and nothing more: the
/// pathway's authority is `AdvisoryOnly`, so this names where capital should
/// be and when, and moves none.
#[derive(Debug, Clone, PartialEq)]
pub struct CapitalAdvice {
    pub signal_id: String,
    pub from: String,
    pub to: String,
    /// The kind of demand the movement is for, as `DemandKind` renders.
    pub kind: String,
    /// The movement in the base currency, the figure that faces the limits.
    pub amount_base: Decimal,
    pub instruct_at: Timestamp,
    pub needed_by: Timestamp,
}

/// The default look-ahead of the capital sentinel: a day, the horizon the
/// kernel's DECIDE stage plans over, so the two read one plan.
pub const CAPITAL_HORIZON: Duration = Duration::from_days(1);

/// What each registry a sweep consulted held when it ran.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryCounts {
    /// Licensed sources the discovery desk handed this sweep.
    pub sources: usize,
    /// Ontology types registered — every type the sweep's detectors read.
    pub ontology_types: usize,
    pub tools: usize,
    /// Brain Registry entries of the specialists a surprise can wake.
    pub brains: usize,
}

/// The ontology types the research sweep's detectors read, by family and
/// name, with what each is (R12-EXPAND-1115). Checked present at
/// construction: a detector reading a type nobody registered is reading a
/// shape nobody agreed.
pub const SWEEP_ONTOLOGY: [(OntologyTypeFamily, &str, &str); 5] = [
    (
        OntologyTypeFamily::Market,
        "price_series",
        "the closes per subject the anomaly detectors and the NOW estimator read",
    ),
    (
        OntologyTypeFamily::Asset,
        "asset_class",
        "the asset-class coverage map the coverage detector reads",
    ),
    (
        OntologyTypeFamily::Event,
        "anomaly",
        "a detector's anomaly, as the signal mapper turns it into a gap",
    ),
    (
        OntologyTypeFamily::Entity,
        "evidence_claim",
        "a claim in the world fabric the poisoning scan reads",
    ),
    (
        OntologyTypeFamily::Lifecycle,
        "research_item",
        "a curriculum item's lifecycle on the expansion desk",
    ),
];

/// The ontology registry the research loop runs against: every type in
/// [`SWEEP_ONTOLOGY`], at version 1.
pub fn sweep_ontology() -> Result<OntologyRegistry> {
    let mut registry = OntologyRegistry::new();
    for (family, name, description) in SWEEP_ONTOLOGY {
        registry.register(OntologyTypeSpec {
            family,
            name: name.to_string(),
            version: 1,
            schema: format!("qip-deepbrain/research:{name}"),
            description: description.to_string(),
        })?;
    }
    Ok(registry)
}

/// A launched information request as a scout job: what to find evidence
/// about, and the rank it was launched at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoutRequest {
    pub request_id: String,
    pub question: String,
    /// `AcquisitionCriteria::net_value_bp` at launch.
    pub rank_bp: u32,
    /// The entities the gap named — what a discovery query is built for.
    pub entities: Vec<String>,
}

/// A stress scenario conditioned on one signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionalScenario {
    pub signal_id: String,
    pub scenario: Scenario,
}

/// One assumption's health: when it was last challenged, how often, and how
/// many sweeps it has held since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssumptionState {
    /// The research question the challenge raised — the assumption named by
    /// what it would take to repair it.
    pub assumption: String,
    /// The trigger of the latest challenge (`low_confidence`,
    /// `model_drift`, `unsupported_request`).
    pub trigger: String,
    pub first_challenged: Timestamp,
    pub last_challenged: Timestamp,
    pub challenges: u32,
    /// Sweeps since the last challenge; zero on the sweep that raised it.
    pub sweeps_since: u32,
}

impl AssumptionState {
    /// `challenged` on the sweep a challenge arrived, `holding` after.
    pub const fn state(&self) -> &'static str {
        if self.sweeps_since == 0 {
            "challenged"
        } else {
            "holding"
        }
    }
}

/// Assumption health by state (`challenged`, `holding`), per sweep.
pub const AMBIENT_ASSUMPTIONS: &str = "qip_ambient_assumptions";
/// Conditional scenarios generated, per sweep.
pub const AMBIENT_SCENARIOS: &str = "qip_ambient_scenarios_total";

/// What the evolution desk did in the cycle before a sweep, as the sweep
/// record carries it: enough to see a challenger minted or a champion
/// crowned without opening the desk's own log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvolutionTrace {
    pub subject: String,
    pub proposed: usize,
    pub registered: usize,
    pub challengers_minted: usize,
    pub crowned: Option<String>,
}

impl ResearchRound {
    pub fn describe(&self) -> String {
        format!(
            "research: {} anomaly(ies), {} gap(s) raised ({} unclassifiable), admissions {:?}; \
             {} started, {} closed, {} withdrawn, {} in queue; {} info request(s) launched, {} \
             halted, {} outranked; {} tick campaign(s) opened; {} NOW surprise(s) woke {:?}; \
             evidence: {} claim(s) held, poisoning {:?}; market: {} unit(s) granted, declined \
             {:?}, {} window(s) measured, {} retired, {} specialist population(s) proposed; \
             acquisitions {:?}; campaigns {:?}; reflex packages {:?}; outputs advisory, none \
             promoted; capabilities are paper and shadow only",
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
            self.info_outranked,
            self.campaigns_opened,
            self.now_surprises,
            self.now_wakes,
            self.evidence_held,
            self.poisoning,
            self.units_granted,
            self.declined,
            self.measured,
            self.retired,
            self.specialists_proposed,
            self.acquisitions,
            self.campaigns,
            self.packages,
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
    /// Signals routed but not yet activated, by signal id, with each
    /// signal's expiry — the deadline its information request is urgent
    /// against. Bounded by the router's hold queue: an entry leaves when its
    /// signal is activated, shed, expired or found below materiality.
    pending: BTreeMap<String, (RaisedGap, Timestamp)>,
    /// NOW surprise signals routed but not yet activated, by signal id, with
    /// the specialist each would wake. Bounded like `pending`.
    pending_wakes: BTreeMap<String, qip_kernel::now_stage::Specialist>,
    /// Capital-preparation signals routed but not yet activated, by signal
    /// id, with the movement each recommends. Bounded like `pending`.
    pending_capital: BTreeMap<String, CapitalAdvice>,
    /// The kernel NOW queue's sequence number this loop has read through.
    surprises_seen: u64,
    issued: u64,
    fills_read: usize,
    slippage_read: usize,
    advisories: VecDeque<Advisory<String>>,
    info_requests: VecDeque<InfoRequest>,
    /// This sweep's information-request candidates and the observation
    /// each would launch with, ranked together before any launches.
    candidates: Vec<(InfoRequest, &'static str)>,
    /// The entities each candidate's gap named, by request id, for the
    /// scout job a launch becomes. Emptied by every launch pass.
    candidate_entities: BTreeMap<String, Vec<String>>,
    meta: MetaIntelligence,
    compiler: CognitiveCompiler,
    signer: Option<Box<dyn PackageSigner + Send>>,
    package_store: Option<Arc<dyn BlobStore>>,
    campaign_policy: CampaignPolicy,
    /// When each started item started, by question. Bounded by the queue.
    started_at: BTreeMap<String, Timestamp>,
    /// Post-start residuals each started fill campaign had at its last
    /// window, for the market's realised-value measure.
    evaluated: BTreeMap<String, usize>,
    /// Compute the market charged each started item across its windows.
    charged: BTreeMap<String, Decimal>,
    /// Last sweep's grants, awaiting this sweep's measurement.
    granted: BTreeMap<String, Granted>,
    /// Realised and expected basis points per reputation cell — domain,
    /// horizon bucket and regime — the record the reputation book is filled
    /// from (R12-AMBIENT-3111).
    domain_record: BTreeMap<ReputationKey, (f64, f64)>,
    /// The horizon bucket each admitted question was filed under, from its
    /// signal's deadline at admission. Bounded by the queue.
    horizons: BTreeMap<String, &'static str>,
    /// The last measured window per question — units granted and what they
    /// realised — so the next window's marginal value is the gain over
    /// that level and not an average (R12-AMBIENT-3112). Bounded by the
    /// queue.
    last_window: BTreeMap<String, (u32, f64)>,
    acquisitions: VecDeque<AcquisitionRecord>,
    capabilities: VecDeque<CapabilityEntry>,
    packages: VecDeque<CompiledReflex>,
    /// The Brain Registry entry of every specialist a NOW surprise can wake,
    /// by specialist (R12-CONTRACT-7086). Checked complete at construction.
    brains: BTreeMap<&'static str, qip_contracts::BrainSpec>,
    /// Model references the learning round found past their drift threshold
    /// since the last sweep, consumed by it (R12-EXPAND-6046).
    drifted_models: Vec<String>,
    /// The hosted model's reply tally, where one is installed (6040).
    replies: Option<(String, crate::language::ReplyTally)>,
    /// Assumption health by assumption, bounded by [`RETAINED`]: the entry
    /// longest unchallenged leaves first when a new one arrives at the cap.
    assumptions: BTreeMap<String, AssumptionState>,
    /// The ontology the sweep's detectors read (R12-EXPAND-1115).
    ontology: OntologyRegistry,
    /// How many tools the node registered, read once at construction.
    tool_count: usize,
    /// Fill-prediction campaigns opened and not yet concluded, with the
    /// instant each opened, bounded by [`RETAINED`] (R12-EXPAND-6042): a
    /// campaign counted and dropped could never learn what its venue did
    /// next.
    open_campaigns: VecDeque<(TickCampaign, Timestamp)>,
}

impl std::fmt::Debug for ResearchLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResearchLoop")
            .field("every_cycles", &self.config.every_cycles)
            .field("queued", &self.desk.queue().len())
            .field("pending", &self.pending.len())
            .field(
                "signer",
                &self.signer.as_ref().map(|s| s.key_id().to_string()),
            )
            .finish_non_exhaustive()
    }
}

/// Tools this node actually has, by the names `plan::AcquisitionAction`
/// resolves them by. Each is a desk in this binary: the discovery desk, the
/// learning desk's fit, the platform's stress tester. Anything else a gap
/// asks for is a tool to build, and is queued as one.
/// How many tools a registry holds, across every kind.
pub fn tool_count(tools: &qip_agents::tools::ToolRegistry) -> usize {
    qip_agents::tools::ToolKind::ALL
        .iter()
        .map(|kind| tools.by_kind(*kind).len())
        .sum()
}

pub fn registered_tools() -> Result<qip_agents::tools::ToolRegistry> {
    registered_tools_holding(&[])
}

/// The tool `plan::AcquisitionAction::FormulateSolver` resolves a quantum
/// backend by, and the classical solver it needs beside it (ADR 0006: a
/// quantum result is scored against the classical baseline, so a node with
/// one and not the other cannot evaluate a formulation).
pub const QUANTUM_BACKEND_TOOL: &str = "quantum-backend";
pub const CLASSICAL_SOLVER_TOOL: &str = "classical-solver";

/// What this process runs beyond the three desks every node has: the
/// classical router's solver always, the in-tree quantum simulator only where
/// the compute settings attached it, and one connector per catalogued source
/// the node polls (R12-EXPAND-6059).
///
/// Derived from the settings the composition root read, never listed. A tool
/// registered here that the process does not run would let a gap that needs
/// it be admitted unblocked and then do nothing; a tool the process runs and
/// leaves out is built again by the desk's queue. `tick-replay` and
/// `venue-adapter` are deliberately absent: the node holds neither
/// (R12-EXPAND-6037), and the desk proposes them as one pack.
pub fn node_tools(
    compute: &qip_kernel::compute_routing::ComputeSettings,
    connector_sources: &[String],
) -> Vec<(String, qip_agents::tools::ToolKind)> {
    use qip_agents::tools::ToolKind;
    let mut held = vec![(CLASSICAL_SOLVER_TOOL.to_string(), ToolKind::Solver)];
    if compute.provider != qip_kernel::compute_routing::QuantumProviderChoice::Off {
        held.push((QUANTUM_BACKEND_TOOL.to_string(), ToolKind::QuantumBackend));
    }
    held.extend(
        connector_sources
            .iter()
            .map(|id| (format!("connector/{id}"), ToolKind::Connector)),
    );
    held
}

/// [`registered_tools`] plus what [`node_tools`] says this process runs.
pub fn registered_tools_holding(
    extra: &[(String, qip_agents::tools::ToolKind)],
) -> Result<qip_agents::tools::ToolRegistry> {
    use qip_agents::tools::ToolKind;
    let mut tools = qip_agents::tools::ToolRegistry::new();
    tools.register("source-discovery", ToolKind::Connector)?;
    tools.register("model-fit", ToolKind::ModelEndpoint)?;
    tools.register("stress-simulator", ToolKind::Simulator)?;
    for (name, kind) in extra {
        tools.register(name, *kind)?;
    }
    Ok(tools)
}

impl ResearchLoop {
    pub fn new(
        config: ResearchConfig,
        coverage: BTreeMap<String, BTreeSet<String>>,
    ) -> Result<Self> {
        Self::new_holding(config, coverage, &[])
    }

    /// The loop a composition root builds: [`Self::new_holding`] over what
    /// [`node_tools`] reads from the settings the root parsed.
    pub fn new_for_node(
        config: ResearchConfig,
        coverage: BTreeMap<String, BTreeSet<String>>,
        compute: &qip_kernel::compute_routing::ComputeSettings,
        connector_sources: &[String],
    ) -> Result<Self> {
        Self::new_holding(config, coverage, &node_tools(compute, connector_sources))
    }

    /// [`Self::new`] with the tools [`node_tools`] derived from the running
    /// process added to the registry the desk and the brain specs are
    /// checked against.
    pub fn new_holding(
        config: ResearchConfig,
        coverage: BTreeMap<String, BTreeSet<String>>,
        extra_tools: &[(String, qip_agents::tools::ToolKind)],
    ) -> Result<Self> {
        let desk = ExpansionDesk::new(
            DeskConfig {
                quiet_sweeps: 3,
                max_started_sweeps: DESK_MAX_STARTED_SWEEPS as u64,
                jurisdiction: config.jurisdiction.clone(),
                owner: "qip-deepbrain/research".to_string(),
            },
            registered_tools_holding(extra_tools)?,
        )?;
        // Materiality 10%: a detection weaker than that is below the bar and
        // wakes nothing. 256 activations an hour, 256 held: far above what a
        // sweep raises today, so the budget binds only in a storm, which is
        // when it is meant to.
        let policy = RoutingPolicy::standard(1_000, 256, Duration::from_hours(1), 256)?;
        // Every specialist a surprise can wake is a registered brain whose
        // tools this node actually has. Refused at start-up rather than at
        // the first wake: a spec naming a tool nobody registered is a brain
        // that would be woken to do work it cannot do.
        let brains = crate::now::specialist_registry()?;
        let tools = registered_tools_holding(extra_tools)?;
        let ontology = sweep_ontology()?;
        Self::check_ontology(&ontology)?;
        let tool_count = tool_count(&tools);
        for specialist in qip_kernel::now_stage::Specialist::ALL {
            let spec = brains.get(specialist.as_str()).ok_or_else(|| {
                Error::invalid(format!(
                    "the {} specialist has no BrainSpec; register it before it can be woken",
                    specialist.as_str()
                ))
            })?;
            if let Some(missing) = spec.tools.iter().find(|t| tools.get(t).is_none()) {
                return Err(Error::invalid(format!(
                    "BrainSpec {} names tool `{missing}`, which this node does not register; \
                     register the tool or correct the spec",
                    spec.id
                )));
            }
        }
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
            pending_capital: BTreeMap::new(),
            surprises_seen: 0,
            issued: 0,
            fills_read: 0,
            slippage_read: 0,
            advisories: VecDeque::new(),
            info_requests: VecDeque::new(),
            candidates: Vec::new(),
            candidate_entities: BTreeMap::new(),
            meta: MetaIntelligence::new(MetaPolicy::default())?,
            compiler: CognitiveCompiler::new(CompilerBounds::default())?,
            signer: None,
            package_store: None,
            campaign_policy: CampaignPolicy::default(),
            started_at: BTreeMap::new(),
            evaluated: BTreeMap::new(),
            charged: BTreeMap::new(),
            granted: BTreeMap::new(),
            domain_record: BTreeMap::new(),
            horizons: BTreeMap::new(),
            last_window: BTreeMap::new(),
            acquisitions: VecDeque::new(),
            capabilities: VecDeque::new(),
            packages: VecDeque::new(),
            brains,
            drifted_models: Vec::new(),
            replies: None,
            assumptions: BTreeMap::new(),
            ontology,
            tool_count,
            open_campaigns: VecDeque::new(),
            config,
        })
    }

    /// Refuse an ontology missing a type the sweep's detectors read.
    fn check_ontology(ontology: &OntologyRegistry) -> Result<()> {
        for (family, name, _) in SWEEP_ONTOLOGY {
            if ontology.get_latest(family, name).is_none() {
                return Err(Error::invalid(format!(
                    "the research sweep reads {family:?}.{name}, which the ontology registry \
                     does not hold; register the type before the sweep runs"
                )));
            }
        }
        Ok(())
    }

    /// The ontology registry the sweep runs against.
    /// Extend a type of the sweep's ontology on a passing evaluation
    /// (R12-EXPAND-6056): the new version cites the evaluation, and an
    /// evaluation that failed, or none, leaves the registry at the version
    /// it held.
    pub fn extend_ontology(
        &mut self,
        family: OntologyTypeFamily,
        name: &str,
        schema: &str,
        description: &str,
        evaluation: &qip_expansion::promotion::EvaluationRecord,
    ) -> Result<u32> {
        self.ontology.extend(
            family,
            name,
            schema,
            description,
            &qip_agents::registries::CitedEvaluation {
                id: format!("evaluation:{}", evaluation.question()),
                passed: evaluation.passed(),
            },
        )
    }

    pub const fn ontology(&self) -> &OntologyRegistry {
        &self.ontology
    }

    /// Sign every reflex package the compiler emits with `signer`, and only
    /// then may one be published. ADR 0110's KMS signer is installed here
    /// when it exists; until then packages are held unsigned.
    pub fn with_package_signer(mut self, signer: Box<dyn PackageSigner + Send>) -> Self {
        self.signer = Some(signer);
        self
    }

    /// Hand the next sweep the models the learning round found drifted, so
    /// each raises a `GapTrigger::ModelDrift` gap. Taken by the sweep, so a
    /// drift is reported once per learning round that measured it, not on
    /// every sweep after.
    pub fn note_drifted_models(&mut self, references: impl IntoIterator<Item = String>) {
        self.drifted_models.extend(references);
    }

    /// Judge `model`'s refused and malformed replies each sweep, once
    /// `MODEL_REPLIES_MIN` have accumulated.
    pub fn with_reply_tally(mut self, model: String, tally: crate::language::ReplyTally) -> Self {
        self.replies = Some((model, tally));
        self
    }

    /// Publish signed packages to `store`. Without a signer nothing is
    /// written whatever the store: an unsigned package cannot be loaded.
    pub fn with_package_store(mut self, store: Arc<dyn BlobStore>) -> Self {
        self.package_store = Some(store);
        self
    }

    pub const fn meta(&self) -> &MetaIntelligence {
        &self.meta
    }

    pub fn acquisitions(&self) -> impl Iterator<Item = &AcquisitionRecord> {
        self.acquisitions.iter()
    }

    pub fn capabilities(&self) -> impl Iterator<Item = &CapabilityEntry> {
        self.capabilities.iter()
    }

    pub fn packages(&self) -> impl Iterator<Item = &CompiledReflex> {
        self.packages.iter()
    }

    /// The Brain Registry entries of the specialists this loop can wake.
    pub const fn brains(&self) -> &BTreeMap<&'static str, qip_contracts::BrainSpec> {
        &self.brains
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
        let fills: Vec<FillObservation> = platform
            .fill_scores()
            .iter()
            .map(|f| FillObservation {
                venue: f.venue.clone(),
                object_id: f.object_id.to_string(),
                filled_at: f.filled_at,
                trade_error_bps: f.trade_error_bps,
            })
            .collect();
        let slippage = slippage_of(platform);
        self.sweep_over(
            platform,
            &fills,
            &slippage,
            now,
            licensed_sources,
            eligible_jurisdictions,
        )
    }

    /// The sweep, over fills and slippage the caller read. `sweep` reads them
    /// from the platform; a test hands a campaign the fills a paper venue
    /// would have produced without running a book to produce them, through
    /// the identical path.
    pub fn sweep_over(
        &mut self,
        platform: &Platform,
        fills: &[FillObservation],
        slippage: &[SlippageObservation],
        now: Timestamp,
        licensed_sources: &BTreeSet<String>,
        eligible_jurisdictions: &BTreeSet<String>,
    ) -> Result<ResearchRound> {
        let metrics = platform.telemetry().metrics.clone();
        let mut round = ResearchRound::default();
        // The registries this sweep runs against, consulted before any
        // detector reads: an ontology that lost a type stops the sweep by
        // name rather than letting a detector read an unregistered shape.
        Self::check_ontology(&self.ontology)?;
        round.registries = RegistryCounts {
            sources: licensed_sources.len(),
            ontology_types: self.ontology.type_count(),
            tools: self.tool_count,
            brains: self.brains.len(),
        };
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
        let declines: Vec<(&str, bool, qip_twin::value::Simulated<Decimal>)> = platform
            .declined_scores()
            .iter()
            .map(|d| (d.gate.as_str(), d.regret, d.would_have_earned))
            .collect();
        observations.extend(veto_observations(&declines)?);
        let refusals: Vec<Refusal> = platform
            .feasibility_refusals()
            .iter()
            .map(|r| Refusal {
                venue: r.venue.clone(),
                constraint: r.constraint.clone(),
            })
            .collect();
        observations.extend(refusal_clusters(&refusals, MIN_REFUSALS)?);
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
        observations.extend(model_drift(&std::mem::take(&mut self.drifted_models)));
        if let Some((model, tally)) = &self.replies
            && let Some(counts) = tally.take_at_least(MODEL_REPLIES_MIN)
        {
            observations.extend(model_error_rate(
                model,
                counts.total(),
                counts.failed(),
                MODEL_REPLIES_MIN,
                MODEL_ERROR_RATE,
            )?);
        }
        // A shrunken list means the platform retained fewer scores than were
        // read; read them all again rather than index past the end.
        if fills.len() < self.fills_read {
            self.fills_read = 0;
        }
        let threshold = Decimal::from_f64(RESIDUAL_BPS)
            .ok_or_else(|| Error::numeric("the residual threshold has no decimal form"))?;
        // Every campaign this sweep opens, kept rather than only counted
        // (RM-P9-08): each becomes the observation the curriculum queues a
        // tick replay from, below. Counted alone, a venue whose twin kept
        // missing raised a counter and no research, and the residual that
        // opened the campaign stayed the loss nobody could explain.
        let mut opened_campaigns: Vec<TickCampaign> = Vec::new();
        for fill in &fills[self.fills_read..] {
            let instruments = BTreeSet::from([fill.object_id.clone()]);
            let mut opened: Vec<TickCampaign> = Vec::new();
            if let Some(c) = self.ticks.observe_venue(&fill.venue, instruments.clone()) {
                opened.push(c);
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
                opened.push(c);
            }
            for campaign in opened {
                round.campaigns_opened += 1;
                metrics.count(
                    names::TICK_CAMPAIGNS_OPENED,
                    labels([("trigger", campaign_str(campaign.trigger))]),
                );
                opened_campaigns.push(campaign);
            }
        }
        self.fills_read = fills.len();
        // Slippage against arrival, a trigger production never fed until
        // 2026-10-11 (R12-EXPAND-6069): each paper fill once, by venue.
        if slippage.len() < self.slippage_read {
            self.slippage_read = 0;
        }
        for fill in &slippage[self.slippage_read..] {
            // Basis points to the detector's fraction of price.
            let Some(fraction) = Decimal::from_f64(fill.slippage_bps / 10_000.0) else {
                continue;
            };
            if let Some(c) = self.ticks.observe_slippage(
                fraction,
                Some(&fill.venue),
                BTreeSet::from([fill.object_id.clone()]),
            ) {
                round.campaigns_opened += 1;
                metrics.count(
                    names::TICK_CAMPAIGNS_OPENED,
                    labels([("trigger", campaign_str(c.trigger))]),
                );
                opened_campaigns.push(c);
            }
        }
        self.slippage_read = slippage.len();
        // Conclude the campaigns earlier sweeps opened whose venue has been
        // re-measured since, then keep this sweep's for a later one.
        self.conclude_campaigns(platform, fills, now, &mut round)?;
        for campaign in &opened_campaigns {
            if campaign.trigger == TickCampaignTrigger::FillPredictionError
                && campaign.venue.is_some()
            {
                if self.open_campaigns.len() == RETAINED {
                    self.open_campaigns.pop_front();
                }
                self.open_campaigns.push_back((campaign.clone(), now));
            }
        }
        // One observation per trigger and venue, not one per campaign: a
        // venue whose twin misses a dozen fills is one question that stays
        // raised, not a dozen duplicates the desk would refuse.
        observations.extend(campaigns::observations(&opened_campaigns)?);
        let covered: BTreeSet<String> = platform.price_history().keys().cloned().collect();
        observations.extend(coverage(&self.coverage, &covered, COVERAGE_FLOOR)?);

        // --- evidence: the world fabric the connectors feed ---------------
        round.evidence_held = platform.evidence_held();
        for finding in platform.evidence_poisoning(now, &PoisoningThresholds::default()) {
            *round.poisoning.entry(finding.class.as_str()).or_default() += 1;
            observations.push(poisoning_observation(&finding));
        }

        // --- the market's specialist proposals -----------------------------
        if let Some(calibration) = platform.calibration()
            && calibration.evaluated >= MIN_EVALUATED
        {
            let exposure = platform.equity().abs();
            let residuals: Vec<DomainResidual> = calibration
                .hit_rate_by_class
                .iter()
                .filter(|(_, rate)| rate.is_finite() && **rate < HIT_RATE_FLOOR)
                .map(|(class, rate)| DomainResidual {
                    domain: class.clone(),
                    residual: (HIT_RATE_FLOOR - rate) / HIT_RATE_FLOOR,
                    exposure,
                })
                .collect();
            for proposal in self.meta.propose_specialists(&residuals)? {
                round.specialists_proposed += 1;
                metrics.count(META_SPECIALISTS_PROPOSED, labels([]));
                observations.push(Observation {
                    trigger: GapTrigger::LowConfidence,
                    wanting: vec![Wanting::Specialist],
                    observation: format!(
                        "belief class `{}` needs a specialist population: its residual error is \
                         economically meaningful",
                        proposal.domain
                    ),
                    affected_domains: vec![proposal.domain.clone()],
                    evidence: vec![
                        "meta:specialist_proposal".to_string(),
                        format!("economic_weight:{}", proposal.economic_weight),
                    ],
                    severity: proposal.residual,
                    // The residual as a share of the book it is applied to,
                    // on the anomaly mapper's [0, 100] scale.
                    economic_value: Decimal::from_f64(proposal.residual * 100.0)
                        .unwrap_or(Decimal::ZERO),
                    // The same residual is also what the book carries while
                    // the class goes unmodelled: an error the desk sizes
                    // against. Equal to the economic figure here because one
                    // measurement states both; the anomaly mapper is where
                    // the two part.
                    risk_value: Decimal::from_f64(proposal.residual * 100.0)
                        .unwrap_or(Decimal::ZERO),
                });
            }
        }

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
        // The pre-limit sentinel (R12-AMBIENT-3070): a limit the book is
        // approaching — inside it, past its warning threshold — is a risk
        // review raised now, while a smaller order still fixes it, rather
        // than a breach the pre-trade veto reports once nothing can be
        // sent. Routed like every other signal, so a book sitting at 90% of
        // every limit at once is deferred on the record rather than waking
        // a review per sweep per limit.
        let state = platform.risk_state_from(platform.risk_figures());
        let check = platform.limits().check(&state);
        for (signal, utilisation) in
            limit_approach_signals(&check, now, self.config.signal_ttl, &mut self.issued)?
        {
            round
                .limit_warnings
                .insert(signal.subject().to_string(), utilisation);
            routable.push((None, signal));
        }
        // The capital sentinel (R12-AMBIENT-3072): the DECIDE stage's
        // pre-positioning plan, one capital-preparation signal per
        // recommended movement. A plan that cannot be built is counted, not
        // raised past: the sweep's other work does not wait on the treasury.
        if self.config.capital_sentinel {
            match platform.pre_position(now, self.config.capital_horizon) {
                Ok(plan) => {
                    for planned in &plan.moves {
                        let movement = CapitalMovement {
                            from: planned.from.to_string(),
                            to: planned.to.to_string(),
                            kind: format!("{:?}", planned.kind),
                            amount_base: planned.amount_base,
                            net_value: planned.net_value,
                            benefit_lower_bound: planned.benefit_lower_bound,
                            instruct_at: planned.instruct_at,
                            needed_by: planned.needed_by,
                        };
                        let (signal, advice) = capital_signal(
                            &movement,
                            plan.base_currency().to_string(),
                            now,
                            self.config.signal_ttl,
                            &mut self.issued,
                        )?;
                        self.pending_capital.insert(signal.id().to_string(), advice);
                        routable.push((None, signal));
                    }
                }
                Err(_) => metrics.count(
                    names::RESEARCH_UNCLASSIFIED,
                    labels([("trigger", "capital_plan")]),
                ),
            }
        }
        // What each question's gap measured this sweep: the market's
        // information value for a bid, read before the gaps move into the
        // desk.
        let mut severity_now: BTreeMap<String, u32> = BTreeMap::new();
        for raised in &raised_now {
            severity_now.insert(
                qip_expansion::desk::question_for(raised),
                basis_points(raised.signal.severity)?,
            );
        }
        let challenged: Vec<(String, &'static str)> = raised_now
            .iter()
            .filter(|r| signal_class(r.trigger) == SignalClass::Assumption)
            .map(|r| (qip_expansion::desk::question_for(r), trigger_str(r.trigger)))
            .collect();
        self.hold_assumptions(&challenged, now, &metrics, &mut round);
        for raised in raised_now {
            let signal = self.emit(&raised, now)?;
            routable.push((Some(raised), signal));
        }
        for (raised, signal) in routable {
            if let Some(raised) = raised {
                self.pending
                    .insert(signal.id().to_string(), (raised, signal.expiry()));
            }
            let events = self.router.route(&signal, now)?;
            round.signals.push(signal.clone());
            round.attention_events.extend(events.iter().cloned());
            if !events.iter().any(|e| e.signal_id == signal.id()) {
                // Below materiality: not an activation and not a loss.
                self.pending.remove(signal.id());
                self.pending_wakes.remove(signal.id());
                self.pending_capital.remove(signal.id());
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
                        self.pending_capital.remove(&event.signal_id);
                    }
                    Disposition::Activated => {
                        if let Some(specialist) = self.pending_wakes.remove(&event.signal_id) {
                            // Only a registered brain is woken. Construction
                            // proved every specialist has a spec, so this
                            // refusal is the record of a registry that lost
                            // one, not a path that runs.
                            if !self.brains.contains_key(specialist.as_str()) {
                                return Err(Error::invalid(format!(
                                    "the {} specialist was woken with no BrainSpec registered",
                                    specialist.as_str()
                                )));
                            }
                            round
                                .woken
                                .insert(event.signal_id.clone(), specialist.as_str());
                            *round.now_wakes.entry(specialist.as_str()).or_default() += 1;
                            metrics.count(
                                crate::now::NOW_WAKES,
                                labels([("specialist", specialist.as_str())]),
                            );
                            // A wake is work, not a count (R12-AMBIENT-3079):
                            // the woken brain is handed the surprise as a
                            // research question of its own, offered to the
                            // desk under its registered spec, so what the
                            // specialist is to do is a queued item with a
                            // plan and a budget rather than a tick on a
                            // counter that nothing reads.
                            let AuthorityBoundary::AdvisoryOnly = event.authority;
                            if let Some(question) = self.wake_specialist(
                                specialist,
                                &signal,
                                &event.feedback_link,
                                now,
                                &metrics,
                                &mut round,
                                licensed_sources,
                                eligible_jurisdictions,
                            )? {
                                round.wake_items.insert(specialist.as_str(), question);
                            }
                            continue;
                        }
                        // Dispatch by pathway (R12-AMBIENT-3065): a capital
                        // signal is advice for the treasury, not a research
                        // gap, so it is filed as an advisory and recorded on
                        // the round rather than offered to the desk.
                        if event.pathway == Pathway::CapitalPreparation {
                            if let Some(advice) = self.pending_capital.remove(&event.signal_id) {
                                self.file_advisory(
                                    format!(
                                        "{:?}: move {} {} to {} by {}",
                                        event.pathway,
                                        advice.amount_base,
                                        advice.kind,
                                        advice.to,
                                        advice.instruct_at.to_date_string(),
                                    ),
                                    &event.feedback_link,
                                );
                                round.capital_advice.push(advice);
                            }
                            continue;
                        }
                        let Some((gap, deadline)) = self.pending.remove(&event.signal_id) else {
                            continue;
                        };
                        // Irrefutable while attention has one authority: a
                        // second variant would be the first activation that
                        // could act, and it stops compiling here first.
                        let AuthorityBoundary::AdvisoryOnly = event.authority;
                        // The advisory is filed under the event's feedback
                        // link, so its outcome joins back to the decision
                        // that woke it (R12-CONTRACT-7079).
                        self.admit(
                            gap,
                            deadline,
                            event.pathway,
                            &event.feedback_link,
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

        // --- conditional scenarios: what the signal's move would do again --
        self.conditional_scenarios(platform, &metrics, &mut round)?;

        // --- curiosity: rank every candidate, then launch ------------------
        self.launch_ranked(&metrics, &mut round)?;

        // --- the compute market ---------------------------------------------
        let residuals = fill_residuals(fills);
        self.measure_last_round(platform, &residuals, now, &metrics, &mut round)?;
        let granted = self.run_market(
            platform,
            &residuals,
            &severity_now,
            now,
            &metrics,
            &mut round,
        )?;

        // --- end the sweep -------------------------------------------------
        let starting: Decimal = granted
            .iter()
            .filter(|(q, _)| {
                self.desk
                    .queue()
                    .entry(q)
                    .is_some_and(|e| *e.state() != State::Started)
            })
            .try_fold(Decimal::ZERO, |sum, (_, cost)| sum.checked_add(*cost))
            .ok_or_else(|| Error::numeric("the round's granted compute overflowed"))?;
        let allowed: BTreeSet<String> = granted.keys().cloned().collect();
        let report = self.desk.finish_sweep_within(
            Budget {
                compute: starting,
                capital: self.config.compute_per_sweep,
            },
            Some(&allowed),
        )?;
        for question in &report.started {
            self.started_at.insert(question.clone(), now);
            self.evaluated.insert(question.clone(), 0);
        }
        self.plan_sweep(&report.started, &mut round);
        // A grant is charged to an item only once the item runs on it: a
        // grant the desk could not start (capital, or a close this sweep)
        // bought nothing, and charging it would fail a campaign's cost
        // criterion on compute it never spent.
        for (question, cost) in &granted {
            if self
                .desk
                .queue()
                .entry(question)
                .is_some_and(|e| *e.state() == State::Started)
            {
                let charged = self
                    .charged
                    .entry(question.clone())
                    .or_insert(Decimal::ZERO);
                *charged = charged
                    .checked_add(*cost)
                    .ok_or_else(|| Error::numeric("an item's charge overflowed"))?;
            }
        }
        for question in report
            .withdrawn
            .iter()
            .chain(report.closed.iter().map(|c| &c.question))
        {
            self.forget(question);
            self.meta.forget(question);
        }

        // --- research carried out: acquisitions, campaigns, compilation ----
        self.carry_out(
            &allowed,
            &residuals,
            now,
            eligible_jurisdictions,
            &metrics,
            &mut round,
        )?;
        metrics.gauge(
            names::RESEARCH_QUEUE_ITEMS,
            labels([]),
            report.queued as f64,
        );
        // The depth read against the queue objective's ceiling
        // (R12-OBS-9164): a gauge alone carries no good/total split.
        metrics.count(
            names::RESEARCH_QUEUE_CHECKS,
            labels([(
                "outcome",
                if report.queued as f64 <= qip_observability::slo::RESEARCH_QUEUE_CEILING {
                    "within"
                } else {
                    "over"
                },
            )]),
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
                qip_expansion::desk::CloseReason::Unproductive => "unproductive",
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
            SignalClass::Capital => Pathway::CapitalPreparation,
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
        deadline: Timestamp,
        pathway: Pathway,
        feedback_link: &str,
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
        let entities = gap.signal.affected_domains.clone();
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
        // The horizon the question is filed under in the reputation book:
        // what was left of its signal's window when it was admitted.
        let remaining = if deadline > now {
            deadline.since(now)
        } else {
            Duration::ZERO
        };
        self.horizons
            .insert(question.clone(), horizon_bucket(remaining));
        self.file_advisory(format!("{pathway:?}: {question}"), feedback_link);

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
        // Urgency is the gap's own deadline (R12-AMBIENT-3106): the signal's
        // expiry, read against its window, so a gap the router deferred for
        // a sweep ranks ahead of the same gap raised this instant.
        let urgency = urgency_bp(now, deadline, self.config.signal_ttl);
        let chosen = cheapest_viable(
            economic_bp,
            severity,
            urgency,
            anomaly_origin,
            acquisition_bp,
        );
        let Some((source, criteria)) = chosen else {
            return Ok(());
        };
        let mut request = InfoRequest::new(
            format!("info-{feedback_link}"),
            question,
            criteria,
            deadline.max(now),
            now,
        )?;
        if request.should_halt(now, MIN_VIABLE_BP) {
            request.halt();
            round.info_halted += 1;
            metrics.count(names::INFO_REQUESTS, labels([("outcome", "halted")]));
            self.retain_request(request);
        } else {
            // Not launched here: every candidate this sweep raises is ranked
            // against the others first (`launch_ranked`).
            self.candidate_entities
                .insert(request.id().to_string(), entities);
            self.candidates.push((request, source));
        }
        Ok(())
    }

    /// Compile the started items into the sweep's plan (R12-REASON-3061). A
    /// refusal is kept on the round: the desk already bounds compute, so a
    /// plan that exceeds the goal's budget is the two disagreeing, which a
    /// reader of the round should see rather than a sweep that fails.
    fn plan_sweep(&self, started: &[String], round: &mut ResearchRound) {
        let items: Vec<crate::research_plan::StartedItem> = started
            .iter()
            .filter_map(|question| {
                let entry = self.desk.queue().entry(question)?;
                Some(crate::research_plan::StartedItem {
                    question: question.clone(),
                    tool: tool_for(entry.kind()),
                    cost: qip_expansion::desk::declared_cost(entry.kind()),
                    cites: entry
                        .origin()
                        .map(|origin| origin.signal.evidence.clone())
                        .unwrap_or_default(),
                })
            })
            .collect();
        match crate::research_plan::compile(
            &items,
            self.config.compute_per_sweep,
            &self.config.jurisdiction,
            u64::try_from(self.config.signal_ttl.as_nanos() / 1_000_000_000)
                .unwrap_or(1)
                .max(1),
        ) {
            Ok(plan) => round.plan = plan,
            Err(error) => round.plan_refused = Some(error.message().to_string()),
        }
    }

    /// Keep an advisory, the oldest leaving first at the cap.
    fn file_advisory(&mut self, text: String, feedback_link: &str) {
        if self.advisories.len() == RETAINED {
            self.advisories.pop_front();
        }
        self.advisories
            .push_back(Advisory::new(text, feedback_link));
    }

    /// The work a woken specialist is given (R12-AMBIENT-3079): the surprise
    /// that woke it, as a research question filed under the brain's
    /// registered competency domains and offered to the desk through the
    /// same admission as every gap. Returns the question where the desk now
    /// holds it — queued by this wake or already open from an earlier one —
    /// and `None` where admission refused it, which the round's admissions
    /// record by outcome.
    #[allow(clippy::too_many_arguments)]
    fn wake_specialist(
        &mut self,
        specialist: qip_kernel::now_stage::Specialist,
        signal: &AmbientSignal,
        feedback_link: &str,
        now: Timestamp,
        metrics: &Metrics,
        round: &mut ResearchRound,
        licensed_sources: &BTreeSet<String>,
        eligible_jurisdictions: &BTreeSet<String>,
    ) -> Result<Option<String>> {
        let Some(spec) = self.brains.get(specialist.as_str()) else {
            return Ok(None);
        };
        let mut evidence = vec![format!("brain:{}", spec.id)];
        evidence.extend(signal.evidence_ids().iter().cloned());
        let observation = Observation {
            trigger: GapTrigger::Surprise,
            wanting: vec![Wanting::Specialist],
            observation: format!(
                "{} is woken to explain the surprise on {}",
                spec.id,
                signal.subject()
            ),
            affected_domains: spec.competency_domains.clone(),
            evidence,
            // The surprise's own size, back on the detector scale.
            severity: f64::from(signal.severity_bp())
                / f64::from(qip_contracts::ambient::MAX_SEVERITY_BP),
            // A surprise states a deviation, not money; a stated zero launches
            // no acquisition, and the specialist's tools are what answer it.
            economic_value: Decimal::ZERO,
            // A deviation is, however, exposure: the surprise's own size on
            // the anomaly mapper's [0, 100] scale, so a large unexplained
            // surprise ranks as dangerous to leave open even though nothing
            // prices what closing it would earn. f64 to Decimal here, from
            // the basis-point severity the signal carries.
            risk_value: Decimal::from_f64(
                f64::from(signal.severity_bp())
                    / f64::from(qip_contracts::ambient::MAX_SEVERITY_BP)
                    * 100.0,
            )
            .unwrap_or(Decimal::ZERO),
        };
        let raised = raise(observation)?;
        let question = qip_expansion::desk::question_for(&raised);
        let (_, pathway) = crate::now::route_of(specialist);
        self.admit(
            raised,
            signal.expiry(),
            pathway,
            feedback_link,
            now,
            metrics,
            round,
            licensed_sources,
            eligible_jurisdictions,
        )?;
        Ok(self
            .desk
            .queue()
            .entry(&question)
            .is_some()
            .then_some(question))
    }

    /// Fold this sweep's assumption challenges into the held table and put
    /// the whole table on the round (R12-AMBIENT-1143).
    fn hold_assumptions(
        &mut self,
        challenged: &[(String, &'static str)],
        now: Timestamp,
        metrics: &Metrics,
        round: &mut ResearchRound,
    ) {
        for state in self.assumptions.values_mut() {
            state.sweeps_since = state.sweeps_since.saturating_add(1);
        }
        for (assumption, trigger) in challenged {
            if !self.assumptions.contains_key(assumption) && self.assumptions.len() >= RETAINED {
                // The longest-held entry leaves; ties by key, so a replay
                // evicts the same one.
                if let Some(oldest) = self
                    .assumptions
                    .iter()
                    .max_by(|a, b| a.1.sweeps_since.cmp(&b.1.sweeps_since).then(b.0.cmp(a.0)))
                    .map(|(k, _)| k.clone())
                {
                    self.assumptions.remove(&oldest);
                }
            }
            let state = self
                .assumptions
                .entry(assumption.clone())
                .or_insert_with(|| AssumptionState {
                    assumption: assumption.clone(),
                    trigger: (*trigger).to_string(),
                    first_challenged: now,
                    last_challenged: now,
                    challenges: 0,
                    sweeps_since: 0,
                });
            state.trigger = (*trigger).to_string();
            state.last_challenged = now;
            state.challenges = state.challenges.saturating_add(1);
            state.sweeps_since = 0;
        }
        round.assumptions = self.assumptions.values().cloned().collect();
        for state in ["challenged", "holding"] {
            let count = round
                .assumptions
                .iter()
                .filter(|a| a.state() == state)
                .count();
            metrics.gauge(
                AMBIENT_ASSUMPTIONS,
                labels([("state", state)]),
                count as f64,
            );
        }
    }

    /// One conditional scenario per risk or assumption signal routed this
    /// sweep (R12-AMBIENT-1005): for each affected subject the platform
    /// holds a tape for, the latest measured move, stated as a one-day
    /// shock — "if the move that raised this signal happens again". The
    /// number is measured, not chosen; a signal on nothing priced gets no
    /// scenario rather than an invented one.
    fn conditional_scenarios(
        &mut self,
        platform: &Platform,
        metrics: &Metrics,
        round: &mut ResearchRound,
    ) -> Result<()> {
        let history = platform.price_history();
        let mut generated = Vec::new();
        for signal in &round.signals {
            if !matches!(signal.class(), SignalClass::Risk | SignalClass::Assumption) {
                continue;
            }
            let shocks: Vec<FactorShock> = signal
                .affected_entities()
                .iter()
                .filter_map(|subject| {
                    let prices = history.get(subject)?;
                    let [.., previous, last] = prices.as_slice() else {
                        return None;
                    };
                    let moved = last / previous - 1.0;
                    (moved.is_finite() && moved != 0.0)
                        .then(|| FactorShock::new(format!("price:{subject}"), moved, 1.0))
                })
                .collect();
            if shocks.is_empty() {
                continue;
            }
            let scenario = Scenario {
                name: format!("conditional:{}", signal.id()),
                description: format!(
                    "hypothetical, conditioned on ambient signal {} ({:?}, {} bp): the latest \
                     measured move of each affected subject, repeated over one day",
                    signal.id(),
                    signal.class(),
                    signal.severity_bp()
                ),
                shocks,
                stressed_correlation: None,
                liquidity_multiplier: 1.0,
                historical: false,
            };
            scenario.validate()?;
            generated.push(ConditionalScenario {
                signal_id: signal.id().to_string(),
                scenario,
            });
        }
        metrics.increment(AMBIENT_SCENARIOS, labels([]), generated.len() as u64);
        round.scenarios = generated;
        Ok(())
    }

    /// Conclude each retained fill-prediction campaign whose venue has
    /// scored at least [`MIN_REMEASURED`] fills on its instruments since it
    /// opened (R12-EXPAND-6042). Per instrument, the modelled figure is the
    /// twin's calibrated fill bias at the venue — what the model now expects
    /// its error to be — and the observed figure the mean trade error of
    /// those fills; the model version names the calibration state it was
    /// read from. A campaign whose venue has not been re-measured enough
    /// stays open; a refused conclusion drops the campaign, because a
    /// refusal of this shape will not change on more fills.
    fn conclude_campaigns(
        &mut self,
        platform: &Platform,
        fills: &[FillObservation],
        now: Timestamp,
        round: &mut ResearchRound,
    ) -> Result<()> {
        let mut still_open = VecDeque::new();
        for (campaign, opened_at) in std::mem::take(&mut self.open_campaigns) {
            let Some(venue) = campaign.venue.clone() else {
                continue;
            };
            let mut remeasured = Vec::new();
            for instrument in &campaign.affected_instruments {
                let errors: Vec<f64> = fills
                    .iter()
                    .filter(|f| {
                        f.venue == venue && &f.object_id == instrument && f.filled_at > opened_at
                    })
                    .filter_map(|f| f.trade_error_bps)
                    .filter(|e| e.is_finite())
                    .collect();
                if errors.len() < MIN_REMEASURED {
                    continue;
                }
                let calibration = platform.fill_calibration().venue(&venue);
                // Statistic to Decimal for the contract: both figures are
                // basis points of price, not money.
                let modelled = calibration.map_or(0.0, |c| c.bias_bps);
                let observed = errors.iter().sum::<f64>() / errors.len() as f64;
                let (Some(modelled), Some(observed)) =
                    (Decimal::from_f64(modelled), Decimal::from_f64(observed))
                else {
                    continue;
                };
                remeasured.push(qip_expansion::campaigns::Remeasured {
                    instrument: instrument.clone(),
                    modelled,
                    observed,
                    samples: errors.len() as u64,
                });
            }
            if remeasured.is_empty() {
                still_open.push_back((campaign, opened_at));
                continue;
            }
            let observations = platform
                .fill_calibration()
                .venue(&venue)
                .map_or(0, |c| c.observations);
            let model_version = format!("twin-fill-calibration/{venue}@{observations}");
            if let Ok(delta) = TickCampaignDetector::conclude(
                &campaign,
                "qip-deepbrain",
                &model_version,
                (opened_at, now),
                remeasured,
            ) {
                round.deltas.push(delta);
            }
        }
        self.open_campaigns = still_open;
        Ok(())
    }

    fn retain_request(&mut self, request: InfoRequest) {
        if self.info_requests.len() == RETAINED {
            self.info_requests.pop_front();
        }
        self.info_requests.push_back(request);
    }

    /// The Curiosity Engine's production caller (R12-AMBIENT-3106): this
    /// sweep's candidates ordered by `rank_acquisitions` — §9.10's product,
    /// highest first, ties by id — and launched in that order while the
    /// sweep's acquisition budget covers each one's cost. A candidate the
    /// budget no longer covers is halted as `outranked`: a better-ranked
    /// request took what it would have spent. Until this call each gap's
    /// request launched the moment its gap woke, so the order gaps happened
    /// to be raised in decided what the budget bought.
    fn launch_ranked(&mut self, metrics: &Metrics, round: &mut ResearchRound) -> Result<()> {
        let candidates = std::mem::take(&mut self.candidates);
        let mut entities = std::mem::take(&mut self.candidate_entities);
        let sources: BTreeMap<String, &'static str> = candidates
            .iter()
            .map(|(r, source)| (r.id().to_string(), *source))
            .collect();
        let requests: Vec<InfoRequest> = candidates.into_iter().map(|(r, _)| r).collect();
        let order: Vec<String> = rank_acquisitions(&requests)
            .into_iter()
            .map(|r| r.id().to_string())
            .collect();
        let mut by_id: BTreeMap<String, InfoRequest> = requests
            .into_iter()
            .map(|r| (r.id().to_string(), r))
            .collect();
        let mut remaining = self.config.acquisition_budget_bp;
        for id in &order {
            let Some(mut request) = by_id.remove(id) else {
                continue;
            };
            let cost = request.criteria().acquisition_cost_bp;
            if cost <= remaining {
                remaining -= cost;
                request.record_acquisition(cost)?;
                round.info_launched += 1;
                round.scouted.push(ScoutRequest {
                    request_id: id.clone(),
                    question: request.decision_id().to_string(),
                    rank_bp: request.criteria().net_value_bp(),
                    entities: entities.remove(id).unwrap_or_default(),
                });
                let source = sources.get(id).copied().unwrap_or("acquisition");
                metrics.count(
                    names::INFO_REQUESTS,
                    labels([("outcome", "launched"), ("source", source)]),
                );
            } else {
                request.halt();
                round.info_outranked += 1;
                metrics.count(names::INFO_REQUESTS, labels([("outcome", "outranked")]));
            }
            self.retain_request(request);
        }
        // Anything the ranker left out has a zero rank, which the viability
        // floor already refused; halted rather than dropped if one appears.
        for (_, mut request) in by_id {
            request.halt();
            round.info_halted += 1;
            metrics.count(names::INFO_REQUESTS, labels([("outcome", "halted")]));
            self.retain_request(request);
        }
        Ok(())
    }

    fn forget(&mut self, question: &str) {
        self.started_at.remove(question);
        self.evaluated.remove(question);
        self.charged.remove(question);
        self.granted.remove(question);
        self.horizons.remove(question);
        self.last_window.remove(question);
    }

    /// The uncertainty a question carries now, as the market reads it: a
    /// fill-bias item's bias estimate at its venue, any other item's
    /// calibration on its belief class.
    fn uncertainty_of(
        platform: &Platform,
        lineage: &qip_expansion::desk::Lineage,
        domain: &str,
        residuals: &[FillResidual],
    ) -> Uncertainty {
        match fill_venue(lineage) {
            Some(venue) => Uncertainty::FillBias {
                standard_error: standard_error(
                    residuals
                        .iter()
                        .filter(|r| r.venue == venue)
                        .map(|r| r.error_bps),
                ),
            },
            None => Uncertainty::Calibration {
                hit_rate: platform
                    .calibration()
                    .and_then(|c| c.hit_rate_by_class.get(domain).copied())
                    .filter(|rate| rate.is_finite()),
            },
        }
    }

    /// The regime a question is bid and recorded under: the platform's own
    /// classification for the question's subject, where the subject is an
    /// instrument the platform holds a tape for, else for the domain itself
    /// — which the platform classifies from the book alone (a drawdown is
    /// a crisis whatever the subject), so a belief-class question is keyed
    /// by the book's regime rather than a literal `all`.
    fn regime_of(platform: &Platform, subject: &str, domain: &str) -> String {
        if platform.price_history().contains_key(subject) {
            platform.regime_context(subject)
        } else {
            platform.regime_context(domain)
        }
    }

    /// Measure what each of last sweep's grants realised (R12-AMBIENT-3112)
    /// and retire what the market says has stopped yielding (3114).
    ///
    /// Realised value is the uncertainty the window was observed to remove,
    /// times what the grant expected (R12-AMBIENT-10189): for a fill-bias
    /// item the shrinkage of the bias estimate's standard error at its venue
    /// between the grant and now, for any other item the rise in the
    /// calibration hit rate of its belief class. Until 2026-10-18 the figure
    /// was a count — new residuals over the stages' need — which paid a
    /// window that gathered twenty residuals saying nothing new as if it had
    /// settled the question. An item whose acquisition has no executor
    /// reduces nothing and realises zero — honestly — and is retired after
    /// the policy's window rather than holding a slot until it times out.
    ///
    /// The marginal value put on the gauge is the gain over the question's
    /// previous grant level (R12-AMBIENT-3112): what the units added since
    /// the last measured window realised per added unit, or this window's
    /// per-unit figure where the level did not grow. An average over every
    /// unit ever granted would read a question that stopped yielding as
    /// still yielding what its first window did.
    fn measure_last_round(
        &mut self,
        platform: &Platform,
        residuals: &[FillResidual],
        now: Timestamp,
        metrics: &Metrics,
        round: &mut ResearchRound,
    ) -> Result<()> {
        let last = std::mem::take(&mut self.granted);
        for (question, grant) in last {
            let Some(lineage) = self.desk.lineage(&question).cloned() else {
                continue;
            };
            let started = self
                .desk
                .queue()
                .entry(&question)
                .is_some_and(|e| *e.state() == State::Started);
            if !started {
                continue;
            }
            if let (Some(venue), Some(started_at)) =
                (fill_venue(&lineage), self.started_at.get(&question))
            {
                let post = residuals
                    .iter()
                    .filter(|r| r.venue == venue && r.at >= *started_at)
                    .count();
                self.evaluated.insert(question.clone(), post);
            }
            let uncertainty_now =
                Self::uncertainty_of(platform, &lineage, &grant.domain, residuals);
            let reduction = uncertainty_now.reduction_from(&grant.uncertainty);
            let realised = grant.expected_bp * reduction;
            let measured = self.meta.record_outcome(
                &question,
                grant.kind,
                grant.units,
                grant.expected_bp,
                realised,
                now,
            )?;
            round.measured += 1;
            let marginal_bp = match self.last_window.get(&question) {
                Some((units_before, realised_before)) if grant.units > *units_before => {
                    (realised - realised_before) / f64::from(grant.units - units_before)
                }
                _ => measured.per_unit_bp,
            };
            self.last_window
                .insert(question.clone(), (grant.units, realised));
            metrics.gauge(
                META_MARGINAL_VALUE,
                labels([("kind", grant.kind.as_str())]),
                marginal_bp,
            );
            if let Ok(key) = ReputationKey::new(&grant.domain, &grant.horizon, &grant.regime) {
                self.record_cell(key, &measured);
            }
            if self.meta.is_retired(&question) {
                self.desk.retire(&question)?;
                self.forget(&question);
                round.retired += 1;
                metrics.count(META_WORK_RETIRED, labels([]));
            }
        }
        Ok(())
    }

    /// Fold one window into its cell's record and the reputation book: a
    /// research domain whose work delivers what it promised earns a
    /// reputation in the horizon and regime it delivered it in, and one that
    /// never has is uninformed rather than averaged in (R12-AMBIENT-3111).
    /// Keyed by the cell and not the domain alone, so a window measured in
    /// one regime leaves another regime's cell uninformed. A full book is
    /// left as it is; the cell is not recorded rather than an old one
    /// silently replaced.
    fn record_cell(&mut self, key: ReputationKey, measured: &MarginalValue) {
        let slot = self.domain_record.entry(key.clone()).or_default();
        slot.0 += measured.realised_bp;
        slot.1 += measured.expected_bp;
        let (realised, expected) = *slot;
        // The market's own prior on both sides, so one empty window lowers a
        // domain's standing without zeroing it — a zero here would decline
        // every bid in the domain before the retirement rule could measure
        // the work that earned it (`meta::REALISATION_PRIOR_BP`).
        let prior = qip_expansion::meta::REALISATION_PRIOR_BP;
        let ratio = ((realised + prior) / (expected + prior)).clamp(0.0, 1.0);
        // Statistic to integer basis points, inside [0, 10000] by the line
        // above: a delivery ratio above one is full delivery, not more.
        let bp = (ratio * f64::from(qip_contracts::ambient::MAX_SEVERITY_BP)).round() as u32;
        // A blank cell or a full book refuses the record; the cell then
        // stays uninformed, which is the honest reading of a record that
        // could not be kept.
        if let Ok(reputation) =
            ModelReputation::new(&key.domain, &key.horizon, &key.regime, bp, bp, bp)
            && self.meta.reputation_mut().record(reputation).is_err()
        {
            self.domain_record.remove(&key);
        }
    }

    /// One market round over every open research item, and what it granted:
    /// question to compute charged. Bids are the desk's queued, deferred and
    /// started items; a blocked item has nothing to spend compute on.
    #[allow(clippy::too_many_arguments)]
    fn run_market(
        &mut self,
        platform: &Platform,
        residuals: &[FillResidual],
        severity_now: &BTreeMap<String, u32>,
        now: Timestamp,
        metrics: &Metrics,
        round: &mut ResearchRound,
    ) -> Result<BTreeMap<String, Decimal>> {
        let mut bids: Vec<WorkBid> = Vec::new();
        // Each bid's cell and the uncertainty it bids against, kept for the
        // grant so the next sweep measures against the same reading.
        let mut cells: BTreeMap<String, (ReputationKey, Uncertainty)> = BTreeMap::new();
        let default_horizon = horizon_bucket(self.config.signal_ttl);
        for entry in self.desk.queue().ranked() {
            if matches!(entry.state(), State::Blocked { .. }) || entry.kind() == TaskKind::ToolBuild
            {
                continue;
            }
            let question = entry.item().research_question.clone();
            let Some(lineage) = self.desk.lineage(&question) else {
                continue;
            };
            let resource = match lineage.class {
                GapClass::ComputeQuantumResearch => ComputeResourceType::Qpu,
                GapClass::Model => ComputeResourceType::Gpu,
                _ => ComputeResourceType::Cpu,
            };
            let unit_cost = entry.score().compute_data_cost;
            // A value outside its scale is refused for this bid alone and
            // counted: every open item bids every sweep, so one item with a
            // malformed figure would otherwise fail every sweep after it.
            let information_value_bp = match severity_now.get(&question) {
                Some(bp) => Ok(*bp),
                None => basis_points(entry.item().expected_information_value),
            };
            let economic_value_bp =
                basis_points(entry.item().expected_economic_value.to_f64() / 100.0);
            let (Ok(information_value_bp), Ok(economic_value_bp)) =
                (information_value_bp, economic_value_bp)
            else {
                *round.declined.entry("invalid_value").or_default() += 1;
                metrics.count(META_BIDS_DECLINED, labels([("reason", "invalid_value")]));
                continue;
            };
            let domain = entry
                .origin()
                .and_then(|o| o.signal.affected_domains.first().cloned())
                .filter(|d| !d.trim().is_empty())
                .unwrap_or_else(|| class_str(lineage.class).to_string());
            // The cell the bid is judged in (R12-AMBIENT-3111): the
            // question's horizon bucket from its admission and the regime
            // the platform classifies now, so a domain that delivered in a
            // quiet tape is not presumed to deliver in a crisis.
            let horizon = self
                .horizons
                .get(&question)
                .copied()
                .unwrap_or(default_horizon);
            let regime = Self::regime_of(platform, &domain, &domain);
            let reputation = ReputationKey::new(&domain, horizon, &regime)?;
            let uncertainty = Self::uncertainty_of(platform, lineage, &domain, residuals);
            cells.insert(question.clone(), (reputation.clone(), uncertainty));
            bids.push(WorkBid {
                work_id: question.clone(),
                kind: work_kind(entry.kind()),
                subject: question,
                resource,
                information_value_bp,
                economic_value_bp,
                unit_cost,
                max_units: 1,
                reputation,
                classical_baseline_cost: (resource == ComputeResourceType::Qpu)
                    .then_some(unit_cost),
            });
        }
        let budget = BTreeMap::from([
            (ComputeResourceType::Cpu, self.config.compute_per_sweep),
            (ComputeResourceType::Gpu, self.config.compute_per_sweep),
            (ComputeResourceType::Tpu, Decimal::ZERO),
            (ComputeResourceType::Qpu, self.config.qpu_per_sweep),
        ]);
        // Each research agent is bounded by its stated budget as well as the
        // resources (R12-AMBIENT-9034): without it one well-reputed agent
        // could take every unit of the round.
        let allocation =
            self.meta
                .allocate_within(&bids, &budget, &self.config.agent_budgets, now)?;
        let mut granted: BTreeMap<String, Decimal> = BTreeMap::new();
        for grant in &allocation.grants {
            let question = grant.allocation.work_id().to_string();
            let total = grant
                .charged
                .values()
                .try_fold(Decimal::ZERO, |sum, v| sum.checked_add(*v))
                .ok_or_else(|| Error::numeric("a grant's charge overflowed"))?;
            round.units_granted += grant.allocation.units();
            metrics.increment(
                META_UNITS_GRANTED,
                labels([("resource", resource_str(grant.allocation.resource_type()))]),
                u64::from(grant.allocation.units()),
            );
            let (key, uncertainty) = cells.remove(&question).unwrap_or_else(|| {
                (
                    ReputationKey {
                        domain: String::new(),
                        horizon: default_horizon.to_string(),
                        regime: String::new(),
                    },
                    Uncertainty::Calibration { hit_rate: None },
                )
            });
            self.granted.insert(
                question.clone(),
                Granted {
                    kind: grant.kind,
                    units: grant.allocation.units(),
                    expected_bp: grant.expected_bp,
                    domain: key.domain,
                    horizon: key.horizon,
                    regime: key.regime,
                    uncertainty,
                },
            );
            granted.insert(question, total);
        }
        for (_, decline) in &allocation.declined {
            *round.declined.entry(decline.as_str()).or_default() += 1;
            metrics.count(META_BIDS_DECLINED, labels([("reason", decline.as_str())]));
        }
        Ok(granted)
    }

    /// Carry out what the market granted to items that have started: the
    /// acquisition, research only, and for a fill-bias item the campaign,
    /// its evaluation and the desk's conclusion. A promoted capability is
    /// handed to the Cognitive Compiler.
    fn carry_out(
        &mut self,
        granted: &BTreeSet<String>,
        residuals: &[FillResidual],
        now: Timestamp,
        eligible_jurisdictions: &BTreeSet<String>,
        metrics: &Metrics,
        round: &mut ResearchRound,
    ) -> Result<()> {
        let started: Vec<(String, qip_expansion::desk::Lineage)> = self
            .desk
            .started()
            .into_iter()
            .filter(|(q, _)| granted.contains(q))
            .map(|(q, l)| (q, l.clone()))
            .collect();
        let jurisdiction_eligible = eligible_jurisdictions.contains(&self.config.jurisdiction);
        for (question, lineage) in started {
            let record = acquire(&question, &lineage, residuals, now)?;
            let action = record.action.as_str();
            let outcome = record.outcome.as_str();
            *round.acquisitions.entry(outcome).or_default() += 1;
            metrics.count(
                RESEARCH_ACQUISITIONS,
                labels([("action", action), ("outcome", outcome)]),
            );
            let gathered = matches!(record.outcome, AcquisitionOutcome::Gathered { .. });
            if self.acquisitions.len() == RETAINED {
                self.acquisitions.pop_front();
            }
            self.acquisitions.push_back(record);
            if !gathered || fill_venue(&lineage).is_none() {
                continue;
            }
            let Some(started_at) = self.started_at.get(&question).copied() else {
                continue;
            };
            let charged = self
                .charged
                .get(&question)
                .copied()
                .unwrap_or(Decimal::ZERO);
            // The most the curriculum ever authorised this item: its budget
            // for every sweep it may run.
            let item_budget = self
                .desk
                .queue()
                .entry(&question)
                .map_or(Decimal::ZERO, |e| e.item().budget);
            let authorised = item_budget
                .checked_mul(Decimal::from_int(DESK_MAX_STARTED_SWEEPS))
                .ok_or_else(|| Error::numeric("an item's authorised budget overflowed"))?;
            let Some(result) = run_fill_bias_campaign(
                &question,
                &lineage,
                residuals,
                started_at,
                &self.campaign_policy,
                jurisdiction_eligible,
                charged,
                authorised,
            )?
            else {
                *round.campaigns.entry("waiting").or_default() += 1;
                metrics.count(EXPANSION_CAMPAIGNS, labels([("outcome", "waiting")]));
                continue;
            };
            let outcome = match self.desk.conclude(&result.campaign, &result.evaluation) {
                Ok(capability) => {
                    if self.capabilities.len() == RETAINED {
                        self.capabilities.pop_front();
                    }
                    self.capabilities.push_back(capability);
                    self.distil(&question, &result, now, metrics, round)?;
                    "promoted"
                }
                // `conclude` closes a complete campaign that failed a
                // criterion and records its null; anything else it refuses
                // leaves the item running.
                Err(_) if self.desk.queue().entry(&question).is_none() => "null_recorded",
                Err(_) => "refused",
            };
            *round.campaigns.entry(outcome).or_default() += 1;
            metrics.count(EXPANSION_CAMPAIGNS, labels([("outcome", outcome)]));
            if self.desk.queue().entry(&question).is_none() {
                self.forget(&question);
                self.meta.forget(&question);
            }
        }
        Ok(())
    }

    /// The Cognitive Compiler's production caller: a promoted fill-bias
    /// correction formalised and distilled into a bounded reflex package,
    /// signed and published only through an installed signer.
    fn distil(
        &mut self,
        question: &str,
        result: &CampaignResult,
        now: Timestamp,
        metrics: &Metrics,
        round: &mut ResearchRound,
    ) -> Result<()> {
        let correction = &result.correction;
        let (low, high) = result.bias_band_bps;
        // Residual uncertainty: the bootstrap band's half-width against the
        // estimate. An interval as wide as the estimate is total uncertainty,
        // and the basis-point form saturates there rather than overflowing.
        let fraction = if correction.bias_bps.abs() > 0.0 {
            ((high - low) / 2.0 / correction.bias_bps.abs()).min(1.0)
        } else {
            1.0
        };
        let uncertainty_bp = (fraction.max(0.0) * 10_000.0).round() as u32;
        // Statistic to an integer adjustment: the twin's entry is moved by
        // minus the bias, rounded to a whole basis point.
        let add_bps = -(correction.bias_bps.round() as i32);
        let digests: Vec<String> = result
            .campaign
            .stages()
            .iter()
            .map(|(_, provenance)| provenance.dataset_digest.clone())
            .collect();
        let established = result
            .campaign
            .stages()
            .iter()
            .zip(digests.iter())
            .map(|((environment, _), digest)| Established {
                statement: format!("the {environment:?} stage ran on dataset {digest}"),
                evidence: vec![digest.clone()],
            })
            .collect();
        let venue_id: String = correction
            .venue
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let discovery = Discovery {
            discovery_id: format!("fill-bias-{venue_id}"),
            question: question.to_string(),
            conclusion: format!(
                "the twin's entry at `{}` is corrected by {add_bps} bp",
                correction.venue
            ),
            findings: established,
            verification: Verification {
                evaluation: format!("evaluation:{question}"),
                criteria_passed: result
                    .evaluation
                    .findings()
                    .values()
                    .filter(|f| f.passed)
                    .count() as u32,
            },
            uncertainty_bp,
            rules: vec![ReflexRule {
                id: format!("fill-bias@{venue_id}"),
                when: vec![Condition::Equals {
                    input: "venue".to_string(),
                    value: correction.venue.clone(),
                }],
                output: "twin_entry_bps".to_string(),
                add_bps,
            }],
        };
        let compiled = match self.compiler.compile(&discovery, now) {
            Ok(compiled) => compiled,
            Err(_) => {
                *round.packages.entry("refused").or_default() += 1;
                metrics.count(REFLEX_PACKAGES, labels([("outcome", "refused")]));
                return Ok(());
            }
        };
        let signed = match &self.signer {
            Some(signer) => Some(
                self.compiler
                    .sign(compiled.package.clone(), signer.as_ref())?,
            ),
            None => None,
        };
        let outcome = match (&signed, &self.package_store) {
            (Some(signed), Some(store)) => {
                store.put(
                    &format!("{}.json", signed.package.package_id),
                    signed.encode()?,
                )?;
                "published"
            }
            (Some(_), None) => "signed",
            (None, _) => "held_unsigned",
        };
        *round.packages.entry(outcome).or_default() += 1;
        metrics.count(REFLEX_PACKAGES, labels([("outcome", outcome)]));
        if self.packages.len() == RETAINED {
            self.packages.pop_front();
        }
        self.packages.push_back(CompiledReflex {
            question: question.to_string(),
            compiled,
            signed,
        });
        Ok(())
    }
}

/// The desk's `max_started_sweeps`, as `ResearchLoop::new` configures it.
const DESK_MAX_STARTED_SWEEPS: i64 = 24;

/// The registered tool a task of this kind runs on, by the names
/// [`registered_tools`] registers them under.
const fn tool_for(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::DataAcquisition => "source-discovery",
        TaskKind::Simulation | TaskKind::Campaign => "stress-simulator",
        TaskKind::Experiment | TaskKind::Label | TaskKind::ToolBuild => "model-fit",
    }
}

/// What a research item's compute is spent on.
const fn work_kind(kind: TaskKind) -> WorkKind {
    match kind {
        TaskKind::Simulation | TaskKind::Campaign => WorkKind::Simulation,
        TaskKind::DataAcquisition => WorkKind::DataSource,
        TaskKind::Experiment => WorkKind::Model,
        TaskKind::Label | TaskKind::ToolBuild => WorkKind::Question,
    }
}

/// Fill scores with a residual, as the executor reads them.
fn fill_residuals(fills: &[FillObservation]) -> Vec<FillResidual> {
    fills
        .iter()
        .filter_map(|f| {
            f.trade_error_bps.map(|error_bps| FillResidual {
                venue: f.venue.clone(),
                at: f.filled_at,
                error_bps,
            })
        })
        .collect()
}

/// Every paper fill's slippage against its arrival, from the outcome record.
pub fn slippage_of(platform: &Platform) -> Vec<SlippageObservation> {
    platform
        .outcomes()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision.action {
            qip_twin::capture::Action::Filled { venue, .. }
                if entry.outcome.slippage_bps_f64().is_finite() =>
            {
                Some(SlippageObservation {
                    venue: venue.to_string(),
                    object_id: entry.decision.object_id.to_string(),
                    slippage_bps: entry.outcome.slippage_bps_f64(),
                })
            }
            _ => None,
        })
        .collect()
}

/// The repeated-veto gaps of a sweep's declined scores — `(gate, regret,
/// would have earned)` — each priced at what its gate forwent
/// (R12-EXPAND-6048): the summed positive counterfactual earnings of the
/// gate's regretted declines, so a gate that vetoed a hundred profitable
/// orders outranks one that vetoed a hundred worthless ones. Until
/// 2026-10-18 the sweep called the unpriced detector and every veto gap
/// stated an economic value of zero.
///
/// The earnings are the twin's counterfactual, crossed from `Decimal` to
/// `f64` through `as_f64_for_statistics` and back for ranking only; a
/// figure with no decimal form is refused rather than read as zero.
pub fn veto_observations(
    declines: &[(&str, bool, qip_twin::value::Simulated<Decimal>)],
) -> Result<Vec<Observation>> {
    let priced: Vec<(Decline, Decimal)> = declines
        .iter()
        .map(|(gate, regret, earned)| {
            let value = earned.as_f64_for_statistics();
            let value = Decimal::from_f64(value).ok_or_else(|| {
                Error::numeric(format!(
                    "the decline at gate `{gate}` states earnings {value} with no decimal form; \
                     correct the counterfactual before it is ranked"
                ))
            })?;
            Ok((
                Decline {
                    gate: (*gate).to_string(),
                    regret: *regret,
                },
                value,
            ))
        })
        .collect::<Result<_>>()?;
    repeated_vetoes_priced(&priced, MIN_REGRETS)
}

/// The risk-review signals a limit check raises before any limit fires
/// (R12-AMBIENT-3070): one per limit inside its bound but past its warning
/// threshold, severity the share of the limit in use. A breach is not
/// among them — the pre-trade veto owns a breach, and raising it here too
/// would be two claims about one fact. Returned with each limit's
/// utilisation for the round's record. `issued` is the loop's signal
/// counter, advanced here so the ids stay unique across every signal the
/// sweep emits.
pub fn limit_approach_signals(
    check: &LimitCheck,
    now: Timestamp,
    ttl: Duration,
    issued: &mut u64,
) -> Result<Vec<(AmbientSignal, f64)>> {
    let mut out = Vec::new();
    for warning in check.warnings() {
        // Share of the limit in use, on [0, 1]: observed over bound for a
        // ceiling, bound over observed for a floor the book is sinking
        // towards. A utilisation that is not a finite positive number is a
        // figure nobody computed, and no signal is raised on it.
        let utilisation = warning.utilisation;
        if !utilisation.is_finite() || utilisation <= 0.0 {
            continue;
        }
        let used = if utilisation <= 1.0 {
            utilisation
        } else {
            1.0 / utilisation
        };
        let severity = basis_points(used)?;
        *issued += 1;
        let decimal = |value: f64| {
            Decimal::from_f64(value)
                .ok_or_else(|| Error::numeric("a limit figure has no decimal form"))
        };
        let mut affected: Vec<String> = warning.subject.iter().cloned().collect();
        affected.push(warning.limit_kind.clone());
        let signal = AmbientSignal::new(
            format!("ambient-{issued}"),
            SignalClass::Risk,
            format!("limit:{}", warning.limit_name),
            severity,
            Trigger::Event("limit_approach".to_string()),
            now,
            Detection {
                observed_deviation: decimal(warning.observed)?,
                expected_baseline: decimal(warning.bound)?,
                horizon: Duration::ZERO,
                novelty_bp: severity,
                affected_entities: affected,
                urgency_bp: severity,
                wake_targets: vec![Pathway::RiskReview],
                evidence_ids: vec![
                    format!("limit:{}", warning.limit_name),
                    format!("utilisation:{utilisation:.4}"),
                ],
                expiry: now.saturating_add(ttl),
            },
        )?;
        out.push((signal, utilisation));
    }
    Ok(out)
}

/// The figures of one planned movement the sweep reads, copied out of the
/// capital fabric's plan so this file names none of that crate's types.
#[derive(Debug, Clone)]
struct CapitalMovement {
    from: String,
    to: String,
    kind: String,
    amount_base: Decimal,
    net_value: Decimal,
    benefit_lower_bound: Decimal,
    instruct_at: Timestamp,
    needed_by: Timestamp,
}

/// The capital-preparation signal one recommended movement is, and the
/// advice it carries to the pathway (R12-AMBIENT-3072).
///
/// Severity is the share of the movement's gross benefit that survives its
/// costs — net value over the benefit at its lower bound, both in the
/// destination currency — so a movement that nets almost nothing is a faint
/// signal and one that keeps most of its benefit is a loud one. A movement
/// whose benefit is not positive carries no such share and is refused as a
/// detector bug, not scored at zero.
fn capital_signal(
    movement: &CapitalMovement,
    base: String,
    now: Timestamp,
    ttl: Duration,
    issued: &mut u64,
) -> Result<(AmbientSignal, CapitalAdvice)> {
    let share = movement
        .net_value
        .checked_div(movement.benefit_lower_bound)
        .filter(|_| movement.benefit_lower_bound.is_positive())
        .map(Decimal::to_f64)
        .ok_or_else(|| {
            Error::numeric("a pre-positioning movement has no positive benefit to take a share of")
        })?;
    // Statistic to integer: a share is a fraction of a benefit, and a net
    // value above the benefit would be the planner's bug, refused by
    // `basis_points` rather than clamped.
    let severity = basis_points(share)?;
    *issued += 1;
    let id = format!("ambient-{issued}");
    let (from, to, kind) = (
        movement.from.clone(),
        movement.to.clone(),
        movement.kind.clone(),
    );
    let signal = AmbientSignal::new(
        id.clone(),
        SignalClass::Capital,
        format!("capital:{to}:{kind}"),
        severity,
        Trigger::Event("pre_positioning".to_string()),
        now,
        Detection {
            observed_deviation: movement.amount_base,
            expected_baseline: Decimal::ZERO,
            horizon: movement.needed_by.since(now),
            novelty_bp: severity,
            affected_entities: vec![to.clone(), from.clone()],
            urgency_bp: severity,
            wake_targets: vec![Pathway::CapitalPreparation],
            evidence_ids: vec![
                format!("plan:{}", movement.instruct_at.as_nanos()),
                format!("base:{base}"),
            ],
            expiry: now.saturating_add(ttl),
        },
    )?;
    let advice = CapitalAdvice {
        signal_id: id,
        from,
        to,
        kind,
        amount_base: movement.amount_base,
        instruct_at: movement.instruct_at,
        needed_by: movement.needed_by,
    };
    Ok((signal, advice))
}

/// A poisoning finding as a gap: the platform needs a source it can trust
/// for what the implicated sources were saying — a data gap — raised as a
/// contradiction, since every class is evidence disagreeing with itself or
/// with the world. The text names the class and the sources, so one
/// campaign re-found every sweep is one research item and not many.
fn poisoning_observation(finding: &PoisoningFinding) -> Observation {
    let sources: Vec<&str> = finding.sources.iter().map(String::as_str).collect();
    let mut evidence = vec![
        format!("poisoning:{}", finding.class.as_str()),
        format!("measured:{}", finding.evidence),
    ];
    evidence.extend(finding.sources.iter().map(|s| format!("source:{s}")));
    evidence.extend(finding.claims.iter().take(8).map(|c| format!("claim:{c}")));
    Observation {
        trigger: GapTrigger::Contradiction,
        wanting: vec![Wanting::Source],
        observation: format!("{} from {}", finding.class.as_str(), sources.join(", ")),
        affected_domains: vec!["evidence".to_string()],
        evidence,
        severity: poisoning_severity(finding.class),
        // The detector measures a pattern, not money; stated zero.
        economic_value: Decimal::ZERO,
        // Nor a position's exposure: a poisoned source is held out of the
        // evidence fabric before it can move a book.
        risk_value: Decimal::ZERO,
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
    use qip_contracts::ambient::AcquisitionStatus;

    /// The failure this prevents has happened: the loop stated urgency as
    /// zero, which was harmless under the additive rank and, once the rank
    /// became §9.10's product, scored every request at zero — the
    /// value-of-information path had a caller and could launch nothing. A
    /// gap with half the decision's value at stake and 80% uncertainty is
    /// worth the 500 bp re-sweep: 0.5 × 0.8 × 1 × 0.95 = 3800 bp.
    #[test]
    fn a_gap_with_economic_value_launches_on_the_product_rank() {
        let chosen = cheapest_viable(
            5_000,
            8_000,
            qip_contracts::ambient::MAX_SEVERITY_BP,
            true,
            2_000,
        );
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
    /// The Curiosity Engine decides what the sweep's acquisition budget
    /// buys (R12-AMBIENT-3106): the better-ranked request launches even
    /// though it arrived second, and the one the budget no longer covers is
    /// halted as outranked. Mutation: launch candidates in arrival order
    /// (iterate `requests` instead of `rank_acquisitions(&requests)`) —
    /// `info-a` launches, `info-b` is outranked, and both status assertions
    /// fail.
    #[test]
    fn the_sweep_budget_buys_the_best_ranked_request_not_the_first_raised() {
        let config = ResearchConfig {
            acquisition_budget_bp: 2_000,
            ..ResearchConfig::default()
        };
        let mut research = ResearchLoop::new(config, BTreeMap::new()).expect("assembles");
        let at = Timestamp::from_secs(1_800_000_000);
        let request = |id: &str, significance: u32| {
            InfoRequest::new(
                id,
                format!("question {id}"),
                AcquisitionCriteria::new(significance, significance, 10_000, 1_500)
                    .expect("criteria"),
                at.saturating_add(Duration::from_hours(1)),
                at,
            )
            .expect("request")
        };
        let low = request("info-a", 5_000);
        let high = request("info-b", 9_000);
        assert!(
            high.criteria().net_value_bp() > low.criteria().net_value_bp(),
            "premise: the second request ranks higher"
        );
        research.candidates.push((low, "acquisition"));
        research.candidates.push((high, "acquisition"));
        let mut round = ResearchRound::default();

        research
            .launch_ranked(&Metrics::new("test"), &mut round)
            .expect("launches");

        let status = |id: &str| {
            research
                .info_requests()
                .find(|r| r.id() == id)
                .map(InfoRequest::status)
        };
        assert_eq!(status("info-b"), Some(AcquisitionStatus::InProgress));
        assert_eq!(status("info-a"), Some(AcquisitionStatus::Halted));
        assert_eq!((round.info_launched, round.info_outranked), (1, 1));
    }

    /// Urgency is the deadline (R12-AMBIENT-3106): two requests equal in
    /// every other factor, one with its whole window ahead and one a sweep
    /// from its deadline, rank the nearer deadline first, and the budget
    /// for one request buys that one. The ids are chosen so that the
    /// rank's tie-break by id would pick the far request: under the
    /// mutation that returns a constant from `urgency_bp` both rank equal,
    /// `info-a` (the far one) launches, and both status assertions fail.
    #[test]
    fn two_gaps_equal_in_every_other_factor_rank_the_nearer_deadline_first() {
        let at = Timestamp::from_secs(1_800_000_000);
        let window = Duration::from_hours(1);
        let far = urgency_bp(at, at.saturating_add(window), window);
        let near = urgency_bp(at, at.saturating_add(Duration::from_mins(5)), window);
        assert_eq!(far, URGENCY_FLOOR_BP, "a full window ahead is the floor");
        assert_eq!(
            urgency_bp(at, at, window),
            qip_contracts::ambient::MAX_SEVERITY_BP,
            "at the deadline the scale's top"
        );
        assert!(
            near > far,
            "premise: the nearer deadline is the more urgent"
        );

        let config = ResearchConfig {
            acquisition_budget_bp: 2_000,
            ..ResearchConfig::default()
        };
        let mut research = ResearchLoop::new(config, BTreeMap::new()).expect("assembles");
        let request = |id: &str, urgency: u32, deadline: Timestamp| {
            InfoRequest::new(
                id,
                format!("question {id}"),
                AcquisitionCriteria::new(6_000, 6_000, urgency, 1_500).expect("criteria"),
                deadline,
                at,
            )
            .expect("request")
        };
        research.candidates.push((
            request("info-a", far, at.saturating_add(window)),
            "acquisition",
        ));
        research.candidates.push((
            request("info-b", near, at.saturating_add(Duration::from_mins(5))),
            "acquisition",
        ));
        let mut round = ResearchRound::default();
        research
            .launch_ranked(&Metrics::new("test"), &mut round)
            .expect("launches");
        let status = |id: &str| {
            research
                .info_requests()
                .find(|r| r.id() == id)
                .map(InfoRequest::status)
        };
        assert_eq!(status("info-b"), Some(AcquisitionStatus::InProgress));
        assert_eq!(status("info-a"), Some(AcquisitionStatus::Halted));
    }

    /// A repeated-veto gap carries what its gate forwent (R12-EXPAND-6048):
    /// three regretted declines that would have earned 100, 50 and a loss
    /// price the gap at 150, and a gate with as many regrets that forwent
    /// nothing prices at zero. Mutation: call the unpriced
    /// `repeated_vetoes` in `veto_observations` — the first gap prices at
    /// zero and the assertion fails.
    #[test]
    fn a_repeated_veto_gap_carries_the_earnings_its_gate_forwent() {
        use qip_twin::value::Simulated;
        let d = |v: i64| Simulated::of(Decimal::from_int(v));
        let declines = [
            ("tight-gate", true, d(100)),
            ("tight-gate", true, d(50)),
            ("tight-gate", true, d(-20)),
            ("idle-gate", true, d(0)),
            ("idle-gate", true, d(0)),
            ("idle-gate", true, d(0)),
        ];
        let raised = veto_observations(&declines).expect("prices");
        let value_of = |gate: &str| {
            raised
                .iter()
                .find(|o| {
                    o.evidence
                        .iter()
                        .any(|e| e.ends_with(&format!("gate:{gate}")))
                })
                .map(|o| o.economic_value)
        };
        assert_eq!(raised.len(), 2, "premise: both gates repeat: {raised:?}");
        assert_eq!(value_of("tight-gate"), Some(Decimal::from_int(150)));
        assert_eq!(value_of("idle-gate"), Some(Decimal::ZERO));
    }

    /// Every launched request becomes a scout job on the round, in rank
    /// order, carrying the entities its gap named (R12-AMBIENT-9033); an
    /// outranked one becomes none. Mutation: push the scout job before the
    /// budget check — the outranked `info-a` is scouted and the assertion
    /// fails.
    #[test]
    fn a_launched_request_becomes_a_scout_job_in_rank_order_and_an_outranked_one_none() {
        let config = ResearchConfig {
            acquisition_budget_bp: 2_000,
            ..ResearchConfig::default()
        };
        let mut research = ResearchLoop::new(config, BTreeMap::new()).expect("assembles");
        let at = Timestamp::from_secs(1_800_000_000);
        for (id, significance) in [("info-a", 5_000), ("info-b", 9_000), ("info-c", 7_000)] {
            let request = InfoRequest::new(
                id,
                format!("question {id}"),
                AcquisitionCriteria::new(significance, significance, 10_000, 900)
                    .expect("criteria"),
                at.saturating_add(Duration::from_hours(1)),
                at,
            )
            .expect("request");
            research
                .candidate_entities
                .insert(id.to_string(), vec![format!("entity-{id}")]);
            research.candidates.push((request, "acquisition"));
        }
        let mut round = ResearchRound::default();
        research
            .launch_ranked(&Metrics::new("test"), &mut round)
            .expect("launches");
        let scouted: Vec<(&str, &str)> = round
            .scouted
            .iter()
            .map(|s| (s.request_id.as_str(), s.entities[0].as_str()))
            .collect();
        assert_eq!(
            scouted,
            [("info-b", "entity-info-b"), ("info-c", "entity-info-c")],
            "{round:?}"
        );
        assert!(round.scouted[0].rank_bp > round.scouted[1].rank_bp);
        assert!(research.candidate_entities.is_empty());
    }

    /// Realised value is the uncertainty a window was observed to remove
    /// (R12-AMBIENT-10189): a belief class whose calibration did not move
    /// realises nothing, one that moved realises the share of its remaining
    /// miss it closed, and a fill estimate whose standard error shrank
    /// realises the shrinkage. Mutation: return `1.0` from the calibration
    /// arm when the two rates are equal — the first assertion fails.
    // Exact zero is the property: a reduction that is merely small is a
    // window paid for something it did not do.
    #[allow(clippy::float_cmp)]
    #[test]
    fn a_grant_whose_class_calibration_did_not_move_realises_zero() {
        let same = Uncertainty::Calibration {
            hit_rate: Some(0.6),
        };
        assert_eq!(same.reduction_from(&same), 0.0);
        let better = Uncertainty::Calibration {
            hit_rate: Some(0.8),
        };
        assert!((better.reduction_from(&same) - 0.5).abs() < 1e-12);
        let unknown = Uncertainty::Calibration { hit_rate: None };
        assert_eq!(unknown.reduction_from(&same), 0.0);
        assert_eq!(same.reduction_from(&unknown), 0.0);
        let wide = Uncertainty::FillBias {
            standard_error: Some(4.0),
        };
        let narrow = Uncertainty::FillBias {
            standard_error: Some(3.0),
        };
        assert!((narrow.reduction_from(&wide) - 0.25).abs() < 1e-12);
        assert_eq!(
            wide.reduction_from(&narrow),
            0.0,
            "a wider estimate is no reduction"
        );
        assert_eq!(
            standard_error([35.0, 45.0, 35.0, 45.0].into_iter()),
            Some((100.0 / 3.0 / 4.0_f64).sqrt())
        );
        assert_eq!(standard_error([40.0].into_iter()), None);
    }

    #[test]
    fn a_gap_with_a_zero_factor_launches_nothing_at_any_cost() {
        let full = qip_contracts::ambient::MAX_SEVERITY_BP;
        assert!(cheapest_viable(0, full, full, true, 0).is_none());
        assert!(cheapest_viable(full, 0, full, true, 0).is_none());
    }
}
