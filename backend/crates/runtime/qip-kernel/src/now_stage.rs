//! The NOW Brain as the UNDERSTAND stage runs it (blueprint v12.0 §9.6; GCP
//! v3.0 §11.2; WORLD-3075..3078, WORLD-7196, WORLD-9016..9021, AMBIENT-3079,
//! DATA-3080, EVID-3081).
//!
//! `qip_world_model::now` holds the estimator; this module is the composition
//! root's half. Every cycle it turns what the platform has observed into
//! bitemporally stamped readings at the eight §9.6 scopes, runs one fenced
//! reconciler step, keeps the surprises for the deep brain's attention
//! router, and hands the kernel one [`NowReview`] — compact belief deltas and
//! manifest references, never a raw payload — to append to the log.
//!
//! # Advisory, and structurally so
//!
//! [`NowStage::run`] takes its inputs by value and returns a record. It holds
//! no handle to a thesis, a size or an order, and nothing on the order path
//! reads it; the one consumer inside the cycle is the forecast lattice, which
//! is advisory too. The reflex cell never waits on it: the cell does not
//! depend on this crate at all, so a stale or absent NOW state cannot stall a
//! pass (WORLD-9018) — staleness lowers the confidence [`NowStage::brain`]
//! serves, and that is the whole of its effect.
//!
//! # What each scope is fed from, honestly
//!
//! Instrument, sector, region, venue and global market state from the bar
//! history; portfolio state from the book's exposure; infrastructure state
//! from the kill switch and the platform's blind spots; economy state from
//! the world model's macro surprises; company state from fundamentals the
//! world model holds about entities. Supply chain and policy have a domain
//! and no feed: nothing the platform ingests today states them, and a
//! variable fed from nothing would be a number nobody measured.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use qip_contracts::world_model::ModelDisagreement;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_world_model::now::{
    BeliefDelta, Constraint, Domain, FencingToken, ManifestRef, NowBrain, Observation,
    PartitionKey, Scale, Scope, ScopeKind, SurpriseEvent, VariableKey, VariableSpec,
};
/// Re-exported so a binary reading the surprise queue names the same types
/// without a second dependency path to the world model.
pub use qip_world_model::now::{Specialist, SurpriseEvent as NowSurprise};
use serde::{Deserialize, Serialize};

/// Surprises kept for the deep brain between its sweeps. A reader that falls
/// further behind than this loses the oldest and is told how many.
pub const NOW_SURPRISES_KEPT: usize = 256;

/// Returns an instrument needs before its realised volatility is read.
pub const NOW_MIN_VOLATILITY_RETURNS: usize = 5;

/// Returns the realised volatility is computed over.
pub const NOW_VOLATILITY_WINDOW: usize = 20;

/// The seed the particle filter draws from, so a replay reproduces it.
pub const NOW_SEED: u64 = 0x004e_4f57_5354_4147;

/// The source the bar-derived readings name, and their manifest's source.
pub const BAR_SOURCE: &str = "kernel/bar-history";

/// Metric names this stage writes. Labels are [`Domain::as_str`] only.
pub const NOW_SURPRISES_TOTAL: &str = "qip_now_surprises_total";
pub const NOW_VARIABLES: &str = "qip_now_variables";
pub const NOW_WORKER_FAILURES_TOTAL: &str = "qip_now_worker_failures_total";

/// What the record's idempotency key starts with.
pub const IDEMPOTENCY_PREFIX: &str = "now:";

/// One instrument as the bar history shows it at the pass.
#[derive(Clone, Debug, PartialEq)]
pub struct InstrumentReading {
    pub subject: String,
    /// Simple returns, oldest first.
    pub returns: Vec<f64>,
    /// When the last bar closed: the reading's valid and knowable instant.
    pub close_time: Timestamp,
    pub spread_ratio: Option<f64>,
    pub sector: Option<String>,
    pub country: Option<String>,
    pub venue: Option<String>,
}

/// Everything one pass reads.
#[derive(Clone, Debug, PartialEq)]
pub struct NowInputs {
    pub cycle: u64,
    pub home_region: String,
    pub instruments: Vec<InstrumentReading>,
    /// Gross exposure over gross exposure plus cash, where the book has any.
    pub exposure_fraction: Option<f64>,
    pub halted: bool,
    /// Priced instruments too thin to read, over all priced instruments.
    pub blind_spot_share: Option<f64>,
    /// `(series, surprise)` readable now.
    pub macro_surprises: Vec<(String, f64)>,
    /// `(feature, entity, value)` readable now.
    pub company_features: Vec<(String, String, f64)>,
}

/// The pass, as the log keeps it. Compact by construction: deltas and
/// manifest references, never the readings themselves (DATA-3080).
///
/// Unknown fields are refused. Every nested type already refused them, but
/// this record did not, so a raw payload smuggled in beside the deltas
/// decoded without complaint and "only deltas" was a property of the
/// writer, not of the schema. The field set is unchanged, so every version-1
/// record ever journaled decodes exactly as before and the schema version
/// stays 1. What tightens is what a record may *add*.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NowReview {
    pub cycle: u64,
    pub at: Timestamp,
    pub epoch: u64,
    pub deltas: Vec<BeliefDelta>,
    pub surprises: Vec<SurpriseEvent>,
    pub disagreements: Vec<ModelDisagreement>,
    pub refused: Vec<(String, String)>,
    pub worker_failures: BTreeMap<String, usize>,
    pub vetoed: BTreeMap<String, usize>,
    pub expired: usize,
    pub unchanged: usize,
    /// The estimators that ran this pass.
    pub ensemble: Vec<String>,
    /// Live variables per scope kind after the pass.
    pub variables_by_scope: BTreeMap<String, usize>,
}

impl EventBody for NowReview {
    const TOPIC: Topic = Topic::WorldModelUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{IDEMPOTENCY_PREFIX}{}", self.cycle))
    }
}

impl NowReview {
    /// A short clause for the stage line.
    pub fn detail(&self) -> String {
        let surprises = if self.surprises.is_empty() {
            String::new()
        } else {
            format!(
                ", {} surprise(s) waking forecast, adversarial, sensing and risk specialists",
                self.surprises.len()
            )
        };
        format!(
            "; NOW state: {} belief delta(s) across {} scope(s) from {} estimator(s){surprises}",
            self.deltas.len(),
            self.variables_by_scope.len(),
            self.ensemble.len()
        )
    }
}

/// The stage: the brain, its fencing token, and the surprise queue.
#[derive(Debug)]
pub struct NowStage {
    brain: NowBrain,
    token: FencingToken,
    surprises: VecDeque<SurpriseEvent>,
    /// Surprises ever raised; the sequence number of the newest.
    raised: u64,
    last_surprised: BTreeSet<String>,
}

impl NowStage {
    pub fn new() -> Result<Self> {
        let mut brain = NowBrain::standard(NOW_SEED)?;
        let token = brain.lead();
        Ok(Self {
            brain,
            token,
            surprises: VecDeque::new(),
            raised: 0,
            last_surprised: BTreeSet::new(),
        })
    }

    pub fn brain(&self) -> &NowBrain {
        &self.brain
    }

    /// Remove an estimator, as a worker outage would (WORLD-9019).
    pub fn remove_worker(&mut self, id: &str) -> bool {
        self.brain.remove_worker(id)
    }

    /// Instruments a surprise named on the last pass: the forecast
    /// specialist's wake list, read by the lattice in the same cycle.
    pub fn surprised_instruments(&self) -> &BTreeSet<String> {
        &self.last_surprised
    }

    /// Surprises raised after sequence number `seen`, oldest first, the
    /// sequence number of the newest, and how many were lost because the
    /// reader fell more than [`NOW_SURPRISES_KEPT`] behind.
    pub fn surprises_since(&self, seen: u64) -> (Vec<&SurpriseEvent>, u64, u64) {
        let oldest_kept = self.raised - self.surprises.len() as u64;
        let lost = oldest_kept.saturating_sub(seen);
        let skip = usize::try_from(seen.saturating_sub(oldest_kept)).unwrap_or(usize::MAX);
        (
            self.surprises.iter().skip(skip).collect(),
            self.raised,
            lost,
        )
    }

    /// One pass.
    pub fn run(&mut self, inputs: NowInputs, now: Timestamp) -> NowReview {
        let mut refused = Vec::new();
        for (feature, _, _) in &inputs.company_features {
            if let Err(e) = self.brain.catalogue_mut().declare(company_spec(feature)) {
                refused.push((company_name(feature), e.message().to_string()));
            }
        }
        let (mut observations, mut built_refused) = observations(&inputs);
        refused.append(&mut built_refused);
        let (mut own, mut own_refused) = own_observations(&inputs, now);
        observations.append(&mut own);
        refused.append(&mut own_refused);
        let step = match self.brain.step(self.token, observations, now) {
            Ok(step) => step,
            Err(e) => {
                refused.push(("now-brain".to_string(), e.message().to_string()));
                Default::default()
            }
        };
        refused.extend(step.refused);
        self.last_surprised = step
            .surprises
            .iter()
            .filter(|s| s.key.scope.kind == ScopeKind::Instrument)
            .map(|s| s.key.scope.id.clone())
            .collect();
        for s in &step.surprises {
            self.surprises.push_back(s.clone());
            self.raised += 1;
            while self.surprises.len() > NOW_SURPRISES_KEPT {
                self.surprises.pop_front();
            }
        }
        let mut variables_by_scope: BTreeMap<String, usize> = BTreeMap::new();
        for p in self.brain.store().partitions().cloned().collect::<Vec<_>>() {
            if let Some(vars) = self.brain.store().partition(&p) {
                for key in vars.keys() {
                    *variables_by_scope
                        .entry(key.scope.kind.as_str().to_string())
                        .or_default() += 1;
                }
            }
        }
        NowReview {
            cycle: inputs.cycle,
            at: now,
            epoch: self.token.epoch(),
            deltas: step.deltas,
            surprises: step.surprises,
            disagreements: step.disagreements,
            refused,
            worker_failures: step.worker_failures,
            vetoed: step.vetoed,
            expired: step.expired,
            unchanged: step.unchanged,
            ensemble: self
                .brain
                .workers()
                .into_iter()
                .map(|(id, _)| id.to_string())
                .collect(),
            variables_by_scope,
        }
    }
}

fn company_name(feature: &str) -> String {
    format!("fundamental.{feature}")
}

/// A fundamental's spec: relative scale, because a revenue figure's units
/// are its source's; quarterly freshness, because that is how often one is
/// restated.
fn company_spec(feature: &str) -> VariableSpec {
    VariableSpec {
        name: company_name(feature),
        domain: Domain::Companies,
        scale: Scale::Relative { floor: 1e-9 },
        fresh_for: Duration::from_days(92),
        expires_after: Duration::from_days(400),
        constraint: Constraint::Unbounded,
        drift_per_day: 0.01,
        reading_noise: 0.05,
    }
}

fn deviation(values: &[f64]) -> Option<f64> {
    if values.len() < 2 {
        return None;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    Some((values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt())
}

/// Build the pass's readings at every scope the inputs feed. Pure, so a
/// replay of the same inputs builds the same readings. Returns what it could
/// not build, named, rather than dropping it.
pub fn observations(inputs: &NowInputs) -> (Vec<Observation>, Vec<(String, String)>) {
    let mut out = Vec::new();
    let mut refused = Vec::new();
    let home = inputs.home_region.as_str();
    let mut push = |kind: ScopeKind,
                    id: &str,
                    name: &str,
                    domain: Domain,
                    region: &str,
                    value: f64,
                    source: &str,
                    reference: String,
                    at: Timestamp| {
        let built = (|| -> Result<Observation> {
            Ok(Observation {
                key: VariableKey::new(
                    if kind == ScopeKind::Global {
                        Scope::global()
                    } else {
                        Scope::new(kind, id)?
                    },
                    name,
                )?,
                partition: PartitionKey::new(domain, region)?,
                value,
                source: source.to_string(),
                valid_at: at,
                known_at: at,
                manifest: ManifestRef::new(source, reference)?,
            })
        })();
        match built {
            Ok(o) => out.push(o),
            Err(e) => refused.push((
                format!("{}:{id}/{name}", kind.as_str()),
                e.message().to_string(),
            )),
        }
    };
    for i in &inputs.instruments {
        let Some(&last) = i.returns.last() else {
            continue;
        };
        let region = i.country.as_deref().unwrap_or(home);
        let reference = format!("{}@{}", i.subject, i.close_time);
        let at = i.close_time;
        push(
            ScopeKind::Instrument,
            &i.subject,
            "return",
            Domain::Markets,
            region,
            last,
            BAR_SOURCE,
            reference.clone(),
            at,
        );
        // Aggregates name the instrument as the source, so a sector read from
        // five instruments rests on five sources and says so.
        if let Some(sector) = &i.sector {
            push(
                ScopeKind::Sector,
                sector,
                "return",
                Domain::Markets,
                "global",
                last,
                &i.subject,
                reference.clone(),
                at,
            );
        }
        if let Some(country) = &i.country {
            push(
                ScopeKind::Region,
                country,
                "return",
                Domain::Markets,
                country,
                last,
                &i.subject,
                reference.clone(),
                at,
            );
        }
        push(
            ScopeKind::Global,
            "global",
            "return",
            Domain::Markets,
            "global",
            last,
            &i.subject,
            reference.clone(),
            at,
        );
        if i.returns.len() >= NOW_MIN_VOLATILITY_RETURNS {
            let from = i.returns.len().saturating_sub(NOW_VOLATILITY_WINDOW);
            if let Some(vol) = deviation(&i.returns[from..]) {
                push(
                    ScopeKind::Instrument,
                    &i.subject,
                    "realised_volatility",
                    Domain::Volatility,
                    region,
                    vol,
                    BAR_SOURCE,
                    reference.clone(),
                    at,
                );
            }
        }
        if let Some(ratio) = i.spread_ratio {
            push(
                ScopeKind::Instrument,
                &i.subject,
                "spread_ratio",
                Domain::Liquidity,
                region,
                ratio,
                BAR_SOURCE,
                reference.clone(),
                at,
            );
            if let Some(venue) = &i.venue {
                push(
                    ScopeKind::Venue,
                    venue,
                    "spread_ratio",
                    Domain::Liquidity,
                    "global",
                    ratio,
                    &i.subject,
                    reference.clone(),
                    at,
                );
            }
        }
    }
    (out, refused)
}

/// Readings the platform states about itself at `now`: the book's exposure,
/// the kill switch, blind spots, macro surprises and company fundamentals.
/// Split from [`observations`] because their instant is the pass's own.
pub fn own_observations(
    inputs: &NowInputs,
    now: Timestamp,
) -> (Vec<Observation>, Vec<(String, String)>) {
    let mut out = Vec::new();
    let mut refused = Vec::new();
    let home = inputs.home_region.as_str();
    let cycle = inputs.cycle;
    let mut push = |scope: Result<Scope>,
                    name: String,
                    domain: Domain,
                    region: &str,
                    value: f64,
                    source: &str,
                    reference: String| {
        let built = (|| -> Result<Observation> {
            Ok(Observation {
                key: VariableKey::new(scope?, name.clone())?,
                partition: PartitionKey::new(domain, region)?,
                value,
                source: source.to_string(),
                valid_at: now,
                known_at: now,
                manifest: ManifestRef::new(source, reference)?,
            })
        })();
        if let Err(e) = built.map(|o| out.push(o)) {
            refused.push((name, e.message().to_string()));
        }
    };
    if let Some(fraction) = inputs.exposure_fraction {
        push(
            Scope::new(ScopeKind::Portfolio, "desk"),
            "exposure_fraction".into(),
            Domain::Positioning,
            home,
            fraction,
            "kernel/book",
            format!("cycle-{cycle}"),
        );
    }
    push(
        Scope::new(ScopeKind::Infrastructure, home),
        "halted".into(),
        Domain::SystemHealth,
        home,
        if inputs.halted { 1.0 } else { 0.0 },
        "kernel/kill-switch",
        format!("cycle-{cycle}"),
    );
    if let Some(share) = inputs.blind_spot_share {
        push(
            Scope::new(ScopeKind::Infrastructure, home),
            "blind_spot_share".into(),
            Domain::SystemHealth,
            home,
            share,
            "kernel/coverage",
            format!("cycle-{cycle}"),
        );
    }
    for (series, value) in &inputs.macro_surprises {
        push(
            Ok(Scope::global()),
            "macro_surprise".into(),
            Domain::Economy,
            "global",
            *value,
            series,
            format!("world/macro_surprise/{series}"),
        );
    }
    for (feature, entity, value) in &inputs.company_features {
        push(
            Scope::new(ScopeKind::Company, entity.as_str()),
            company_name(feature),
            Domain::Companies,
            "global",
            *value,
            "world/fundamental",
            format!("{feature}/{entity}"),
        );
    }
    (out, refused)
}
