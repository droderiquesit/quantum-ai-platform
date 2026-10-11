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

use qip_contracts::forecast::Forecast;
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_world_model::lattice::{
    Calibrator, Inconsistency, Lane, LatticeHorizon, Population, Question, Resolution, ScoreRecord,
    Scorecard, TargetInput, WeightUpdate, constrain, issue, route,
};
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

/// What the record's idempotency key starts with.
pub const IDEMPOTENCY_PREFIX: &str = "lattice:";

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
}

/// The pass, as the log keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LatticeReview {
    pub cycle: u64,
    pub at: Timestamp,
    /// Every forecast issued this pass, whole.
    pub issued: Vec<Forecast>,
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
        })
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
        for target in order {
            let surprised = inputs.surprised.contains(&target.subject);
            let Some(input) = target_input(target) else {
                continue;
            };
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
                    domain: "markets".to_string(),
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

        LatticeReview {
            cycle: inputs.cycle,
            at: now,
            issued,
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
    let first = t.closes.first()?.0;
    let span_days = last_at.since(first).as_days_f64();
    let bar_days = span_days / (t.closes.len() - 1) as f64;
    if !(bar_days.is_finite() && bar_days > 0.0) {
        return None;
    }
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
    })
}
