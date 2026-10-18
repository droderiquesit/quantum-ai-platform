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

use qip_contracts::world_model::{ModelDisagreement, WorldModelStateFields};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_world_model::now::{
    BeliefDelta, Constraint, Domain, FencingToken, ManifestRef, NowBrain, Observation,
    PartitionKey, Scale, Scope, ScopeKind, SurpriseEvent, VariableKey, VariableSpec, WorldReading,
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
    /// Macro surprises readable now, one per series, each with the instants
    /// the world model holds for it.
    pub macro_surprises: Vec<WorldReading>,
    /// Company features readable now, each with its instants.
    pub company_features: Vec<WorldReading>,
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
    /// The §27 `WorldModelState` of every partition this pass moved a belief
    /// in, as the brain projects it (R12-CONTRACT-7075): intervals and
    /// manifest lineage, not readings. Absent from records written before
    /// the field existed, and from a pass that moved nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub world_states: Vec<WorldModelStateFields>,
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
    first_reads: FirstReads,
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
            first_reads: FirstReads::default(),
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
        for reading in &inputs.company_features {
            if let Err(e) = self
                .brain
                .catalogue_mut()
                .declare(company_spec(&reading.feature))
            {
                refused.push((company_name(&reading.feature), e.message().to_string()));
            }
        }
        let (mut observations, mut built_refused) = observations(&inputs);
        refused.append(&mut built_refused);
        let (mut own, mut own_refused) = own_observations(&inputs, now);
        observations.append(&mut own);
        refused.append(&mut own_refused);
        let offered: Vec<(Feed, &WorldReading)> = inputs
            .macro_surprises
            .iter()
            .map(|r| (Feed::Macro, r))
            .chain(inputs.company_features.iter().map(|r| (Feed::Company, r)))
            .collect();
        let (stamped, mut unstamped) = self.first_reads.stamp(&offered, now);
        refused.append(&mut unstamped);
        let (mut world, mut world_refused) = world_observations(&stamped);
        observations.append(&mut world);
        refused.append(&mut world_refused);
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
        // Only partitions this pass moved: the state of one nothing touched is
        // the state the log already holds, and journalling it again each pass
        // would grow the log with no new fact. A partition with no live
        // state refuses to project and is recorded as refused rather than
        // dropped, so an empty projection is not read as a quiet one.
        let moved: BTreeSet<_> = step
            .deltas
            .iter()
            .map(|d| d.fields().partition.clone())
            .collect();
        let mut world_states = Vec::new();
        for partition in &moved {
            match self.brain.world_model_state(partition, now) {
                Ok(state) => world_states.push(state.into_fields()),
                Err(e) => refused.push((
                    format!("world-model-state/{partition}"),
                    e.message().to_string(),
                )),
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
            world_states,
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
/// the kill switch and blind spots. Split from [`observations`] because their
/// instant is the pass's own — each is measured again on every pass, so every
/// pass's reading is a new one.
///
/// Macro surprises and company features were here until they were found to
/// be the opposite case: a value the world model holds is read again on
/// every pass it stays readable, so stamped with the pass it reached the
/// brain as a new reading every time. See `FirstReads`, below.
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
    (out, refused)
}

/// Which world feed a reading came from: part of what the stage remembers a
/// reading by, so the two feeds can never be taken for one another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Feed {
    Macro,
    Company,
}

/// The value the stage last read for one world series, and when it first
/// read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FirstRead {
    /// The value's identity in the world model: the instant it describes and
    /// the stored number, bit for bit. Not its `available_at`, which a
    /// release delivered again — a re-poll, an at-least-once redelivery —
    /// moves without being a new release; a revision changes the number and
    /// is one.
    valid_at: Timestamp,
    bits: u64,
    /// The pass this stage first read it at, which is its NOW stamp for as
    /// long as it is read.
    at: Timestamp,
}

/// The instant this stage first read each world-model value it still reads.
///
/// # The failure this prevents
///
/// The NOW Brain absorbs a reading only if its `valid_at` is later than
/// every reading the variable already rests on: a high-water mark, which is
/// how a pass that re-reads the same last bar is kept from counting it
/// twice. This stage stamped every macro surprise and company feature with
/// the pass's own instant, so a release the world model keeps readable for
/// 120 days cleared that mark on every pass, and each cycle with nothing new
/// tightened the belief and raised its confidence as if it had.
///
/// # Why the first read, and not the value's own instants
///
/// Every macro series feeds the one global `macro_surprise` variable, so the
/// stamps of different series meet at one mark and must be ordered the way
/// the readings reach the brain. A reference date is not: a quarterly series
/// published after a monthly one names an earlier period, and stamped with
/// it would fall behind the monthly release and be filtered as absorbed — a
/// new release lost. An ingestion instant is not either: connectors stamp it
/// from the caller's horizon and the streaming path delivers out of order,
/// so a record ingested before one already absorbed can still arrive after
/// it, and would be lost the same way. The pass this stage first read a
/// value at is ordered by construction: a value read again keeps its stamp,
/// and a value first read on a later pass is later. The value's own
/// reference instant stays in the manifest reference, so a belief still
/// names the release it rests on.
///
/// The stamp is never earlier than the value was knowable: a reading the
/// world model says was not knowable at the pass, or that describes an
/// instant after it, is refused rather than stamped, because a stamp of
/// `now` would launder it into the present.
///
/// # A pass at the instant of the last
///
/// `Platform::run_cycle` admits a cycle at the instant of the one before. A
/// value first read at a pass no later than the last instant this stage
/// stamped a value at would tie the mark, be filtered as absorbed, and keep
/// that stamp for good. It is held back instead, named in the review's
/// refusals, and stamped at the first later pass.
///
/// # Bounded
///
/// After each pass the memory holds one entry per world series that pass
/// read and nothing else, so it is never larger than the pass's own input,
/// which the world model's feature store bounds. A value no longer offered
/// is forgotten: it has gone stale or been superseded, and on passes that do
/// not run backwards the world model does not offer it again.
#[derive(Clone, Debug, Default)]
struct FirstReads {
    series: BTreeMap<(Feed, String, String), FirstRead>,
    /// The latest pass at which a value was stamped as new.
    last_stamped: Option<Timestamp>,
}

/// A world reading, the feed it came from, and its NOW stamp.
type Stamped<'a> = (Feed, &'a WorldReading, Timestamp);

impl FirstReads {
    /// Stamp one pass's world readings with the pass each was first read at,
    /// refusing, named, what cannot be stamped honestly, and forget every
    /// series the pass did not offer.
    fn stamp<'a>(
        &mut self,
        offered: &[(Feed, &'a WorldReading)],
        now: Timestamp,
    ) -> (Vec<Stamped<'a>>, Vec<(String, String)>) {
        let mut offers: BTreeMap<(Feed, &str, &str), usize> = BTreeMap::new();
        for (feed, r) in offered {
            *offers
                .entry((*feed, r.feature.as_str(), r.subject.as_str()))
                .or_default() += 1;
        }
        let mut kept = BTreeMap::new();
        let mut stamped = Vec::new();
        let mut refused = Vec::new();
        let mut stamped_new = false;
        for &(feed, r) in offered {
            let label = format!("{}/{}", r.feature, r.subject);
            let offered_twice = offers
                .get(&(feed, r.feature.as_str(), r.subject.as_str()))
                .is_some_and(|&n| n > 1);
            let refusal = if offered_twice {
                Some(format!(
                    "was offered more than once on the pass at {now}; a series holds one value \
                     at a time, and which of them to believe is not this stage's guess"
                ))
            } else if r.available_at > now {
                Some(format!(
                    "knowable at {} but read at {now}; a NOW state cannot use a value the \
                     platform did not yet have",
                    r.available_at
                ))
            } else if r.valid_at > now {
                Some(format!(
                    "describes {}, after the pass at {now}; a value about the future is not a \
                     reading of the present",
                    r.valid_at
                ))
            } else {
                None
            };
            if let Some(why) = refusal {
                refused.push((label, why));
                continue;
            }
            let key = (feed, r.feature.clone(), r.subject.clone());
            let bits = r.value.to_bits();
            let at = match self.series.get(&key) {
                Some(seen) if seen.valid_at == r.valid_at && seen.bits == bits => seen.at,
                _ => match self.last_stamped {
                    Some(last) if now <= last => {
                        refused.push((
                            label,
                            format!(
                                "first read at {now}, no later than {last}, when this stage last \
                                 stamped a new value; held to the next later pass, because \
                                 stamped {now} it would tie what the NOW Brain already rests on \
                                 and be filtered as absorbed for good"
                            ),
                        ));
                        continue;
                    }
                    _ => {
                        stamped_new = true;
                        now
                    }
                },
            };
            kept.insert(
                key,
                FirstRead {
                    valid_at: r.valid_at,
                    bits,
                    at,
                },
            );
            stamped.push((feed, r, at));
        }
        self.series = kept;
        if stamped_new {
            self.last_stamped = Some(now);
        }
        (stamped, refused)
    }
}

/// The world model's readings as NOW observations, each stamped in both time
/// dimensions with the pass this stage first read it at (see
/// [`FirstReads`]). The manifest reference names the value by the instant it
/// describes, so a delta resting on it says which release or filing it was.
fn world_observations(stamped: &[Stamped<'_>]) -> (Vec<Observation>, Vec<(String, String)>) {
    let mut out = Vec::new();
    let mut refused = Vec::new();
    for &(feed, r, at) in stamped {
        let (name, built) = match feed {
            Feed::Macro => (
                "macro_surprise".to_string(),
                world_observation(
                    Ok(Scope::global()),
                    "macro_surprise".to_string(),
                    Domain::Economy,
                    r,
                    &r.subject,
                    format!("world/macro_surprise/{}@{}", r.subject, r.valid_at),
                    at,
                ),
            ),
            Feed::Company => (
                company_name(&r.feature),
                world_observation(
                    Scope::new(ScopeKind::Company, r.subject.as_str()),
                    company_name(&r.feature),
                    Domain::Companies,
                    r,
                    "world/fundamental",
                    format!("{}/{}@{}", r.feature, r.subject, r.valid_at),
                    at,
                ),
            ),
        };
        match built {
            Ok(o) => out.push(o),
            Err(e) => refused.push((name, e.message().to_string())),
        }
    }
    (out, refused)
}

fn world_observation(
    scope: Result<Scope>,
    name: String,
    domain: Domain,
    reading: &WorldReading,
    source: &str,
    reference: String,
    at: Timestamp,
) -> Result<Observation> {
    Ok(Observation {
        key: VariableKey::new(scope?, name)?,
        partition: PartitionKey::new(domain, "global")?,
        value: reading.value,
        source: source.to_string(),
        valid_at: at,
        known_at: at,
        manifest: ManifestRef::new(source, reference)?,
    })
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn inputs(macro_surprises: Vec<WorldReading>) -> NowInputs {
        NowInputs {
            cycle: 1,
            home_region: "US".to_string(),
            instruments: Vec::new(),
            exposure_fraction: None,
            halted: false,
            blind_spot_share: None,
            macro_surprises,
            company_features: Vec::new(),
        }
    }

    fn surprise(series: &str, valid_at: Timestamp, available_at: Timestamp) -> WorldReading {
        WorldReading {
            feature: "macro_surprise".to_string(),
            subject: series.to_string(),
            value: 0.3,
            valid_at,
            available_at,
        }
    }

    /// A world reading the stage cannot stamp honestly is refused, named, and
    /// never reaches the brain: one not yet knowable at the pass, one about an
    /// instant after it, and a series offered twice. The stage replaces a
    /// reading's instants with the pass it was first read at, so without
    /// these refusals that stamp would launder the first two into the present
    /// and let the third count one series twice. The one honest reading
    /// beside them is absorbed, under a manifest naming the period it
    /// describes.
    ///
    /// Mutations in `FirstReads::stamp`: drop the `available_at > now`
    /// refusal, the `valid_at > now` refusal, or the offered-twice refusal
    /// (each lets its series into the manifests); in `world_observations`,
    /// drop `@{valid_at}` from the macro manifest reference.
    #[test]
    fn a_world_reading_that_cannot_be_stamped_honestly_is_refused_and_never_reaches_the_brain()
    -> Result<()> {
        let past = now().saturating_sub(Duration::from_days(1));
        let future = now().saturating_add(Duration::from_mins(1));
        let mut stage = NowStage::new()?;
        let review = stage.run(
            inputs(vec![
                surprise("US.CPI.YOY", past, past),
                surprise("US.PPI.YOY", past, future),
                surprise("US.GDP.QOQ", future, past),
                surprise("US.NFP", past, past),
                surprise("US.NFP", past, past),
            ]),
            now(),
        );
        let key = VariableKey::new(Scope::global(), "macro_surprise")?;
        let delta = review
            .deltas
            .iter()
            .find(|d| d.key == key)
            .unwrap_or_else(|| {
                panic!(
                    "premise: the honest reading was not absorbed; refused {:?}",
                    review.refused
                )
            });
        assert_eq!(
            delta.manifests,
            vec![ManifestRef::new(
                "US.CPI.YOY",
                format!("world/macro_surprise/US.CPI.YOY@{past}")
            )?],
            "the belief rests on something other than the one honest reading"
        );
        for series in ["US.PPI.YOY", "US.GDP.QOQ", "US.NFP"] {
            let label = format!("macro_surprise/{series}");
            assert!(
                review.refused.iter().any(|(refused, _)| *refused == label),
                "{label} was not refused by name: {:?}",
                review.refused
            );
        }
        Ok(())
    }

    /// The memory of first reads holds the series the last pass read and
    /// nothing else, so it is never larger than the pass's own input; and a
    /// series read again keeps the stamp of the pass that first read it.
    ///
    /// Mutation: in `FirstReads::stamp`, keep the earlier entries beside the
    /// pass's own instead of replacing them (the memory keeps all eight).
    #[test]
    fn the_memory_of_first_reads_holds_only_the_series_the_last_pass_read() -> Result<()> {
        let past = now().saturating_sub(Duration::from_days(1));
        let later = now().saturating_add(Duration::from_hours(1));
        let mut stage = NowStage::new()?;
        let eight: Vec<WorldReading> = (0..8)
            .map(|i| surprise(&format!("S{i}"), past, past))
            .collect();
        stage.run(inputs(eight), now());
        assert_eq!(
            stage.first_reads.series.len(),
            8,
            "premise: the first pass remembered the eight series it read"
        );

        stage.run(
            inputs(vec![surprise("S0", past, past), surprise("S1", past, past)]),
            later,
        );
        let remembered: Vec<(&str, Timestamp)> = stage
            .first_reads
            .series
            .iter()
            .map(|((_, _, series), read)| (series.as_str(), read.at))
            .collect();
        assert_eq!(
            remembered,
            vec![("S0", now()), ("S1", now())],
            "the memory is not exactly the two series the last pass read, each stamped with the \
             pass that first read it"
        );
        Ok(())
    }
}
