//! The Temporal Forecast Lattice as the UNDERSTAND stage runs it (blueprint
//! v12.0 §9.7 and §9.14; GCP v3.0 §11.3; WORLD-3082..3085, WORLD-7197,
//! WORLD-9022, WORLD-9024..9027).
//!
//! `qip_world_model::lattice` holds the horizons, the populations, the router
//! and the calibrator; this module is the composition root's half. Each pass
//! it resolves the forecasts reality has now answered and scores them per
//! horizon and per regime, refreshes the ensemble weights (or freezes them
//! at the last safe set if the previous pass's scores never reached the
//! log), issues a calibrated [`Forecast`] at every central horizon whose
//! last one has gone stale — and, out of turn, at every horizon of every
//! instrument the NOW Brain was surprised by, which is the forecast
//! specialist's wake — checks adjacent horizons against each other, and
//! hands the kernel one [`LatticeReview`] to append to the log.
//!
//! # What the central cycle may not issue
//!
//! The microsecond row promotes only to the local reflex. The cycle issues
//! from [`Lane::Central`], and [`qip_world_model::lattice::issue`] refuses
//! that lane at that row, so no central forecast can masquerade as a reflex
//! input. The reflex cell's own microstructure forecasts are not this
//! module's and are not claimed here.
//!
//! # The strategic rows are a separate service
//!
//! Weeks-months and quarters-years are served only while
//! [`LatticeStage::set_strategic_available`] says the strategic service is
//! up. While it is down their questions accumulate as a bounded backlog and
//! nothing else changes: the cycle, and everything downstream of it, runs
//! exactly as before (WORLD-9025).
//!
//! # Advisory, and structurally so
//!
//! Nothing here reaches the order path: no thesis, size or order reads a
//! lattice forecast. The forecasts are journaled whole, so a later reader
//! can score them without trusting this process's memory.

use std::collections::{BTreeMap, BTreeSet};

use qip_contracts::forecast::{Forecast, ForecastState};
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_world_model::causal::CausalGraph;
use qip_world_model::lattice::{
    Calibrator, CatalystExpectation, ForecastContext, Inconsistency, Lane, LatticeHorizon,
    MAX_REGIMES, Population, Question, Resolution, ScoreRecord, Scorecard, StructuralClaim,
    TargetInput, WeightUpdate, constrain, issue, route,
};
use qip_world_model::now::WorldReading;
use serde::{Deserialize, Serialize};

/// Instruments forecast per pass: surprised ones first, then identifier
/// order.
pub const LATTICE_MAX_SUBJECTS: usize = 32;

/// Forecasts awaiting reality. Past it the oldest is abandoned, on the
/// record.
pub const LATTICE_MAX_PENDING: usize = 4_096;

/// Questions the strategic backlog holds while the service is down.
pub const LATTICE_MAX_BACKLOG: usize = 1_024;

/// Returns a target needs before it is forecast at all.
pub const LATTICE_MIN_RETURNS: usize = 5;

/// Metric names this stage writes. Labels are [`LatticeHorizon::as_str`].
pub const LATTICE_ISSUED_TOTAL: &str = "qip_lattice_forecasts_issued_total";
pub const LATTICE_BRIER: &str = "qip_lattice_brier_score";
pub const LATTICE_COVERAGE: &str = "qip_lattice_interval_coverage";
pub const LATTICE_PINBALL: &str = "qip_lattice_pinball_loss";
pub const LATTICE_INCONSISTENCIES_TOTAL: &str = "qip_lattice_inconsistencies_total";
pub const LATTICE_WEIGHTS_FROZEN: &str = "qip_lattice_weights_frozen";
/// 1 when a horizon's running calibration is outside its objective, 0 when it
/// meets it. Written only for a horizon with enough resolutions to judge: an
/// unjudged horizon has no series, because a 0 there would read as met.
pub const LATTICE_CALIBRATION_SLO_BREACHED: &str = "qip_lattice_calibration_slo_breached";

/// The running direction-call Brier score a horizon must stay under. A forecast
/// that says nothing (probability one half) scores 0.25 whatever happens, so
/// 0.30 is the line past which a horizon is doing worse than not forecasting
/// direction at all, with room for the sampling noise of a short run.
pub const CALIBRATION_BRIER_BOUND: f64 = 0.30;

/// Resolutions a horizon needs before its score is judged against the bound.
/// Below it a handful of lucky or unlucky calls decide the verdict.
pub const CALIBRATION_MIN_RESOLVED: u64 = 20;

/// Whether `card` breaches the calibration objective, or `None` while it has
/// too few resolutions to say. Pure so the bound, the minimum and the
/// comparison are testable without a cycle.
pub fn calibration_slo_breached(
    card: &qip_world_model::lattice::Scorecard,
    bound: f64,
    minimum: u64,
) -> Option<bool> {
    if card.resolved < minimum {
        return None;
    }
    card.brier().map(|brier| brier > bound)
}

/// What the record's idempotency key starts with.
pub const IDEMPOTENCY_PREFIX: &str = "lattice:";

/// The domain every instrument close is asked in, which the reputation
/// cache is keyed by beside the row and the model (WORLD-9022).
pub const LATTICE_DOMAIN: &str = "markets";

/// How long a macro surprise is read as a pending catalyst for the hours-days
/// row after it became knowable (WORLD-3086): two days, the row's horizon
/// plus one, so a release the cycle learned of this morning widens tomorrow's
/// forecast and one from last week does not.
pub const CATALYST_WINDOW: Duration = Duration::from_days(2);

/// The share of a macro surprise (a fraction of the consensus expectation)
/// expected to pass through to an instrument's close, as an absolute move.
/// One fifth: the hours-days row is widened by the reaction, not moved by it,
/// and a figure larger than the surprise would claim the market reprices by
/// more than the release missed. Capped at [`MAX_CATALYST_MOVE`].
pub const MACRO_PASS_THROUGH: f64 = 0.2;

/// The largest expected absolute reaction a catalyst may carry, as a
/// fraction of the level. A surprise past this is a data error or a crisis,
/// and either way a wider row than this is not a forecast.
pub const MAX_CATALYST_MOVE: f64 = 0.10;

/// What the pass cannot read from the bars: the signals the row specialists
/// consume, set by the composition root before each pass from the world
/// model ([`signals_from_world`]) and cleared by the pass that read them, so
/// a signal is never read twice as if it were fresh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LatticeSignals {
    /// Order-flow imbalance per subject, `[-1, 1]`, buy-positive
    /// (WORLD-3085). Nothing in the kernel yet absorbs a
    /// `qip_world_model::order_flow::FlowObservation`, so this is empty on
    /// every production path today; the seam exists so the flow member
    /// reads flow the day something feeds it.
    pub flow_imbalance: BTreeMap<String, f64>,
    /// A catalyst pending for every subject: a macro surprise that became
    /// knowable inside [`CATALYST_WINDOW`], the largest if several.
    pub pending_catalyst: Option<CatalystExpectation>,
    /// Live causal claims bearing on each subject (WORLD-3088).
    pub structural_claims: BTreeMap<String, Vec<StructuralClaim>>,
}

/// Read the row specialists' signals from the world model as it is known at
/// `now`: every live, unretired causal claim into each subject for the
/// structural member, and the latest macro surprise inside
/// [`CATALYST_WINDOW`] as the pending catalyst for the event member. Point in
/// time on both: a claim retired by `now` is absent, and a surprise not yet
/// knowable at `now` is absent, so a replay at an earlier instant sees what
/// was known then.
pub fn signals_from_world(
    causal: &CausalGraph,
    macro_surprises: &[WorldReading],
    subjects: &[String],
    now: Timestamp,
) -> LatticeSignals {
    let mut structural_claims = BTreeMap::new();
    for subject in subjects {
        let claims: Vec<StructuralClaim> = causal
            .incoming(subject, now)
            .into_iter()
            .map(|edge| StructuralClaim {
                cause: edge.cause.clone(),
                mechanism: edge.mechanism.as_str().to_string(),
                transmission: edge.signed_transmission(),
                lag: edge.lag,
            })
            .collect();
        if !claims.is_empty() {
            structural_claims.insert(subject.clone(), claims);
        }
    }
    let since = now.saturating_sub(CATALYST_WINDOW);
    let pending_catalyst = macro_surprises
        .iter()
        .filter(|r| r.available_at <= now && r.available_at >= since && r.value.is_finite())
        .map(|r| CatalystExpectation {
            source: format!("{}/{}", r.feature, r.subject),
            expected_abs_move: (r.value.abs() * MACRO_PASS_THROUGH).min(MAX_CATALYST_MOVE),
        })
        .fold(None, |best: Option<CatalystExpectation>, c| match best {
            Some(b) if b.expected_abs_move >= c.expected_abs_move => Some(b),
            _ => Some(c),
        });
    LatticeSignals {
        flow_imbalance: BTreeMap::new(),
        pending_catalyst,
        structural_claims,
    }
}

/// One instrument as the pass reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct LatticeTarget {
    pub subject: String,
    /// `(close_time, close)` of the observed bars, oldest first, every one
    /// closed at or before the pass.
    pub closes: Vec<(Timestamp, f64)>,
    pub regime: String,
    pub held_quantity: f64,
    /// The NOW Brain's latent return for the subject: mean, variance, and
    /// the confidence it reads at now. Absent where nothing is promoted.
    pub now_return: Option<(f64, f64, u32)>,
}

/// Everything one pass reads.
#[derive(Clone, Debug, PartialEq)]
pub struct LatticeInputs {
    pub cycle: u64,
    pub targets: Vec<LatticeTarget>,
    /// Instruments the NOW Brain was surprised by this cycle.
    pub surprised: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct Pending {
    subject: String,
    horizon: LatticeHorizon,
    issued_at: Timestamp,
    model_means: BTreeMap<String, f64>,
    quantiles: [f64; 3],
    p_up: f64,
    reference: f64,
    regime: String,
    domain: String,
}

/// The pass, as the log keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LatticeReview {
    pub cycle: u64,
    pub at: Timestamp,
    /// Every forecast issued this pass, whole.
    pub issued: Vec<Forecast>,
    /// The §24.9 control-plane projection of each forecast in `issued`, in
    /// the same order: the compact record a control loop reads without the
    /// model internals. Defaulted so a record journaled before the
    /// projection existed still decodes.
    #[serde(default)]
    pub states: Vec<ForecastState>,
    /// Forecasts issued out of turn because the NOW Brain was surprised.
    pub woken: Vec<String>,
    /// Questions answered by the default ensemble because the reputation
    /// cache had nothing for them.
    pub fallbacks: usize,
    pub scored: Vec<ScoreRecord>,
    pub inconsistencies: Vec<Inconsistency>,
    /// Forecasts given up on unscored: reality never answered, or the
    /// pending bound displaced them.
    pub abandoned: Vec<String>,
    pub refused: Vec<(String, String)>,
    /// Strategic questions waiting on the strategic service.
    pub backlog: usize,
    pub weights: WeightUpdate,
    /// The running scorecard per horizon after the pass.
    pub scorecards: BTreeMap<String, Scorecard>,
}

impl EventBody for LatticeReview {
    const TOPIC: Topic = Topic::WorldModelUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{IDEMPOTENCY_PREFIX}{}", self.cycle))
    }
}

impl LatticeReview {
    pub fn detail(&self) -> String {
        if self.issued.is_empty() && self.scored.is_empty() && self.backlog == 0 {
            return String::new();
        }
        let backlog = if self.backlog == 0 {
            String::new()
        } else {
            format!(", {} strategic question(s) in backlog", self.backlog)
        };
        let frozen = if self.weights == WeightUpdate::Frozen {
            ", ensemble weights frozen at the last safe set"
        } else {
            ""
        };
        format!(
            "; forecast lattice issued {} forecast(s), scored {}, flagged {} cross-horizon \
             inconsistency(ies){backlog}{frozen}",
            self.issued.len(),
            self.scored.len(),
            self.inconsistencies.len()
        )
    }
}

/// The stage's state across passes.
#[derive(Debug)]
pub struct LatticeStage {
    population: Population,
    calibrator: Calibrator,
    pending: BTreeMap<String, Pending>,
    /// When the latest forecast for each (subject, horizon) expires.
    current: BTreeMap<(String, LatticeHorizon), Timestamp>,
    strategic_available: bool,
    backlog: BTreeSet<(String, LatticeHorizon)>,
    calibrator_available: bool,
    signals: LatticeSignals,
    /// Per subject, per regime label: the sum and count of the bar returns
    /// observed under that label, from which the regime-path member reads
    /// the drift the regime has shown (WORLD-3087). Learned here, on the
    /// default path, from the closes every pass already carries. Bounded:
    /// subjects are forgotten with their forecast slots, and a subject holds
    /// at most [`MAX_REGIMES`] labels.
    regime_returns: BTreeMap<String, BTreeMap<String, (f64, u64)>>,
    /// The close each subject's regime return was last taken through, so a
    /// bar is counted once however many passes see it.
    regime_seen: BTreeMap<String, Timestamp>,
}

impl LatticeStage {
    pub fn new() -> qip_core::error::Result<Self> {
        Ok(Self {
            population: Population::standard()?,
            calibrator: Calibrator::new(),
            pending: BTreeMap::new(),
            current: BTreeMap::new(),
            strategic_available: true,
            backlog: BTreeSet::new(),
            calibrator_available: true,
            signals: LatticeSignals::default(),
            regime_returns: BTreeMap::new(),
            regime_seen: BTreeMap::new(),
        })
    }

    /// Hand the next pass the signals it cannot read from bars. Read once by
    /// the next [`Self::run`] and then cleared.
    pub fn set_signals(&mut self, signals: LatticeSignals) {
        self.signals = signals;
    }

    /// The drift per bar, as a return, each regime label has shown for
    /// `subject`, for a reader checking what the regime-path member read.
    pub fn regime_drifts(&self, subject: &str) -> BTreeMap<String, f64> {
        self.regime_returns
            .get(subject)
            .map(|by_regime| {
                by_regime
                    .iter()
                    .filter(|(_, (_, n))| *n > 0)
                    .map(|(regime, (sum, n))| (regime.clone(), sum / *n as f64))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Count the newest bar return of `target` under its regime label, once.
    fn learn_regime_return(&mut self, target: &LatticeTarget) {
        let Some(&[(_, previous), (at, latest)]) = target.closes.last_chunk::<2>() else {
            return;
        };
        if !(previous > 0.0 && latest.is_finite()) {
            return;
        }
        if self
            .regime_seen
            .get(&target.subject)
            .is_some_and(|seen| *seen >= at)
        {
            return;
        }
        self.regime_seen.insert(target.subject.clone(), at);
        let by_regime = self
            .regime_returns
            .entry(target.subject.clone())
            .or_default();
        if !by_regime.contains_key(&target.regime) && by_regime.len() >= MAX_REGIMES {
            return;
        }
        let entry = by_regime.entry(target.regime.clone()).or_insert((0.0, 0));
        entry.0 += (latest - previous) / previous;
        entry.1 += 1;
    }

    pub fn calibrator(&self) -> &Calibrator {
        &self.calibrator
    }

    pub fn population(&self) -> &Population {
        &self.population
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Whether the strategic service is serving (WORLD-9025). Set by the
    /// deep brain's run loop from its own health surface before each cycle;
    /// see `qip_deepbrain::node::gate_strategic_service`.
    pub fn set_strategic_available(&mut self, available: bool) {
        self.strategic_available = available;
    }

    /// What [`Self::set_strategic_available`] last said.
    pub const fn strategic_available(&self) -> bool {
        self.strategic_available
    }

    /// Whether the calibrator's last output reached durable storage. The
    /// kernel sets this from the journal's answer; false freezes weights on
    /// the next pass (WORLD-9027).
    pub fn set_calibrator_available(&mut self, available: bool) {
        self.calibrator_available = available;
    }

    /// One pass.
    pub fn run(&mut self, inputs: LatticeInputs, now: Timestamp) -> LatticeReview {
        let mut refused = Vec::new();
        let mut abandoned = Vec::new();
        let by_subject: BTreeMap<&str, &LatticeTarget> = inputs
            .targets
            .iter()
            .map(|t| (t.subject.as_str(), t))
            .collect();

        // --- resolve what reality has answered ----------------------------
        let mut scored = Vec::new();
        let ids: Vec<String> = self.pending.keys().cloned().collect();
        for id in ids {
            let Some(p) = self.pending.get(&id) else {
                continue;
            };
            let due = p.issued_at.saturating_add(p.horizon.representative());
            if due > now {
                continue;
            }
            // The answer must close inside one horizon past due: a five-minute
            // forecast is not answered by the next day's close, which is a
            // different question.
            let latest = due.saturating_add(p.horizon.representative()).min(now);
            let answer = by_subject.get(p.subject.as_str()).and_then(|t| {
                t.closes
                    .iter()
                    .find(|(at, _)| *at >= due && *at <= latest)
                    .copied()
            });
            match answer {
                Some((observed_at, outcome)) => {
                    let resolution = Resolution {
                        forecast_id: id.clone(),
                        domain: p.domain.clone(),
                        horizon: p.horizon,
                        regime: p.regime.clone(),
                        model_means: p.model_means.clone(),
                        quantiles: p.quantiles,
                        p_up: p.p_up,
                        reference: p.reference,
                        issued_at: p.issued_at,
                        observed_at,
                        outcome,
                        resolved_at: now,
                    };
                    match self.calibrator.score(&resolution) {
                        Ok(record) => scored.push(record),
                        Err(e) => refused.push((id.clone(), e.message().to_string())),
                    }
                    self.pending.remove(&id);
                }
                None => {
                    // Reality has had as long again as the horizon, and a
                    // week, to answer; past that the forecast is abandoned
                    // on the record rather than scored against nothing.
                    let grace = p.horizon.representative().max(Duration::from_days(7));
                    if now.since(due) > grace {
                        abandoned.push(id.clone());
                        self.pending.remove(&id);
                    }
                }
            }
        }

        // --- weights: refresh, or freeze at the last safe set ---------------
        let weights = self.calibrator.refresh_weights(self.calibrator_available);

        // --- issue ------------------------------------------------------------
        let mut order: Vec<&LatticeTarget> = inputs
            .targets
            .iter()
            .filter(|t| inputs.surprised.contains(&t.subject))
            .collect();
        order.extend(
            inputs
                .targets
                .iter()
                .filter(|t| !inputs.surprised.contains(&t.subject)),
        );
        order.truncate(LATTICE_MAX_SUBJECTS);
        let mut issued = Vec::new();
        let mut woken = Vec::new();
        let mut fallbacks = 0;
        let mut inconsistencies = Vec::new();
        let signals = std::mem::take(&mut self.signals);
        for target in order {
            let surprised = inputs.surprised.contains(&target.subject);
            self.learn_regime_return(target);
            let Some(mut input) = target_input(target) else {
                continue;
            };
            input.context = self.context_for(target, &signals, &input);
            let mut chain: Vec<Forecast> = Vec::new();
            for horizon in LatticeHorizon::ALL {
                if horizon.lane() != Lane::Central {
                    continue;
                }
                let slot = (target.subject.clone(), horizon);
                let due = self.current.get(&slot).is_none_or(|exp| *exp <= now);
                if !due && !surprised {
                    continue;
                }
                if horizon.is_strategic() && !self.strategic_available {
                    if self.backlog.len() < LATTICE_MAX_BACKLOG {
                        self.backlog.insert(slot);
                    }
                    continue;
                }
                self.backlog.remove(&slot);
                let question = Question {
                    target: input.target.clone(),
                    domain: LATTICE_DOMAIN.to_string(),
                    horizon,
                };
                let reputation = self.calibrator.weights();
                let routed = match route(
                    &self.population,
                    &question,
                    (!reputation.is_empty()).then_some(reputation),
                ) {
                    Ok(r) => r,
                    Err(e) => {
                        refused.push((input.target.clone(), e.message().to_string()));
                        continue;
                    }
                };
                if routed.fallback {
                    fallbacks += 1;
                }
                match issue(
                    Lane::Central,
                    &input,
                    horizon,
                    &self.population,
                    &routed,
                    self.calibrator.calibration(horizon, now),
                    now,
                ) {
                    Ok(made) => {
                        let id = made.forecast.forecast_id.clone();
                        self.current.insert(slot, made.forecast.expires_at);
                        self.pending.insert(
                            id.clone(),
                            Pending {
                                subject: target.subject.clone(),
                                horizon,
                                issued_at: now,
                                model_means: made.model_means,
                                quantiles: made.quantiles,
                                p_up: made.p_up,
                                reference: made.reference,
                                regime: target.regime.clone(),
                                domain: question.domain.clone(),
                            },
                        );
                        if surprised {
                            woken.push(id);
                        }
                        chain.push(made.forecast);
                    }
                    Err(e) => refused.push((input.target.clone(), e.message().to_string())),
                }
            }
            let fields: Vec<&qip_contracts::forecast::ForecastFields> =
                chain.iter().map(|f| f.fields()).collect();
            inconsistencies.extend(constrain(&fields));
            issued.extend(chain);
        }
        while self.pending.len() > LATTICE_MAX_PENDING {
            let oldest = self
                .pending
                .iter()
                .min_by_key(|(id, p)| (p.issued_at, (*id).clone()))
                .map(|(id, _)| id.clone());
            let Some(id) = oldest else { break };
            self.pending.remove(&id);
            abandoned.push(id);
        }
        // `current` is bounded by subjects seen times four horizons; forget
        // a slot whose forecast expired and whose subject no longer appears.
        let present: BTreeSet<&str> = by_subject.keys().copied().collect();
        self.current
            .retain(|(s, _), exp| *exp > now || present.contains(s.as_str()));
        let slotted: BTreeSet<&str> = self.current.keys().map(|(s, _)| s.as_str()).collect();
        self.regime_returns
            .retain(|s, _| present.contains(s.as_str()) || slotted.contains(s.as_str()));
        self.regime_seen
            .retain(|s, _| present.contains(s.as_str()) || slotted.contains(s.as_str()));

        // A forecast that cannot project is refused on the record rather than
        // dropped from `states` silently, which would leave the two lists
        // different lengths with no explanation.
        let mut states = Vec::with_capacity(issued.len());
        for forecast in &issued {
            match forecast.state() {
                Ok(state) => states.push(state),
                Err(e) => refused.push((forecast.target.clone(), e.message().to_string())),
            }
        }

        LatticeReview {
            cycle: inputs.cycle,
            at: now,
            issued,
            states,
            woken,
            fallbacks,
            scored,
            inconsistencies,
            abandoned,
            refused,
            backlog: self.backlog.len(),
            weights,
            scorecards: LatticeHorizon::ALL
                .iter()
                .filter_map(|h| {
                    self.calibrator
                        .scorecard(*h)
                        .map(|c| (h.as_str().to_string(), *c))
                })
                .collect(),
        }
    }
}

impl LatticeStage {
    /// What the row specialists read for `target`: the regime drifts this
    /// stage has learned (as a drift per day in the target's units), and the
    /// flow, catalyst and claims the composition root handed the pass.
    fn context_for(
        &self,
        target: &LatticeTarget,
        signals: &LatticeSignals,
        input: &TargetInput,
    ) -> ForecastContext {
        let bar_days = bar_days(target).unwrap_or(1.0);
        let regime_drifts = self
            .regime_drifts(&target.subject)
            .into_iter()
            .map(|(regime, per_bar)| (regime, per_bar * input.level / bar_days))
            .collect();
        ForecastContext {
            flow_imbalance: signals.flow_imbalance.get(&target.subject).copied(),
            pending_catalyst: signals.pending_catalyst.clone(),
            regime: Some(target.regime.clone()),
            regime_drifts,
            structural_claims: signals
                .structural_claims
                .get(&target.subject)
                .cloned()
                .unwrap_or_default(),
        }
    }
}

/// The mean spacing of a target's closes in days, or `None` where there are
/// not two closes or they do not advance.
fn bar_days(t: &LatticeTarget) -> Option<f64> {
    let (first, last) = (t.closes.first()?.0, t.closes.last()?.0);
    if t.closes.len() < 2 {
        return None;
    }
    let bar_days = last.since(first).as_days_f64() / (t.closes.len() - 1) as f64;
    (bar_days.is_finite() && bar_days > 0.0).then_some(bar_days)
}

/// The forecast inputs for one target, or `None` where its history is too
/// thin to state a variance. The NOW Brain's latent return, where promoted,
/// supplies the drift and the uncertainty about the level — the higher-level
/// state constraining every horizon — and the observed bar returns supply
/// the variance the target gains per day — the microstructure constraining
/// the higher rows.
fn target_input(t: &LatticeTarget) -> Option<TargetInput> {
    let &(last_at, level) = t.closes.last()?;
    if t.closes.len() <= LATTICE_MIN_RETURNS || !(level.is_finite() && level > 0.0) {
        return None;
    }
    let returns: Vec<f64> = t
        .closes
        .windows(2)
        .filter(|w| w[0].1 > 0.0)
        .map(|w| (w[1].1 - w[0].1) / w[0].1)
        .collect();
    if returns.len() < LATTICE_MIN_RETURNS {
        return None;
    }
    let bar_days = bar_days(t)?;
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let var = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let (drift, level_variance, confidence) = match t.now_return {
        Some((m, v, c)) => (Some(m * level / bar_days), v * level * level, c),
        None => (None, 0.0, 0),
    };
    Some(TargetInput {
        target: format!("{}/close", t.subject),
        level,
        level_variance,
        drift_per_day: drift,
        variance_per_day: var * level * level / bar_days,
        regime_confidence_bp: confidence,
        held_quantity: t.held_quantity,
        evidence: vec![format!(
            "{}#{}@{}",
            crate::now_stage::BAR_SOURCE,
            t.subject,
            last_at
        )],
        context: ForecastContext::default(),
    })
}
