//! The tick learning desk: the deep brain's replay of recorded market
//! history, and everything that reads it (R12-TICK-2059, 2061, 2065, 2067,
//! 2068, 2069, 2076, 2081–2086, 2091, 7199, 2057; R12-DATA-1039).
//!
//! The edge node sealed every tick its feed applied into the Tick Lake, and
//! no shipped binary ever opened a market segment again. Labels had no
//! caller, microstructure models were never trained, the fusion of world and
//! market had nothing to read, and synthetic futures had no state to grow
//! from. This desk is that caller. On its cadence it:
//!
//! 1. **pins** the market history its entitlements allow — the last
//!    `lookback_days` of partitions — as a sealed lake manifest, so the round
//!    can be re-run later against exactly the bytes it read;
//! 2. **replays** it tick by tick through
//!    [`qip_simulation_engine::tick_replay`], rebuilding every book
//!    deterministically and flagging every span the venue's sequence cannot
//!    vouch for;
//! 3. **labels** decision instants from the rebuilt books through
//!    [`qip_training::labels::LabelBuilder`] — never across a flagged span,
//!    never from a tick before it was knowable — and **trains** a
//!    fill-probability and a short-horizon volatility model against their
//!    naive baselines on a held-out slice in time, then **benchmarks** the
//!    fill model one row per venue × asset × regime × horizon;
//! 4. records a **model card** naming the manifest, every entitlement the
//!    round read and the regions those entitlements permit;
//! 5. **fuses** the replayed market with the world model's knowable events
//!    ([`qip_kernel::fusion`]) and **distils** the result into the bounded
//!    opportunity map a regional reflex cell carries, writing it only for the
//!    regions the card permits;
//! 6. **grows a synthetic future tree** from a compact twin state fitted on
//!    the replay, scores the previous round's tree against what happened since,
//!    and prunes it — every branch synthetic, none recorded as observed;
//! 7. optionally **measures** what the zstd codec would save, per partition,
//!    without writing a byte (ADR 0113 §3).
//!
//! # Off unless configured, and closed when it is
//!
//! `QIP_DEEPBRAIN_TICK_EVERY` unset or `0` leaves the desk off. Set, it
//! requires `QIP_DEEPBRAIN_TICK_ENTITLEMENTS` — the entitlements the desk
//! may read — and refuses to start without them, because a desk that read
//! "whatever is in the lake" would read data under terms nobody granted it.
//! An entitlement the round read with no regions in
//! `QIP_DEEPBRAIN_TICK_ENTITLEMENT_REGIONS` leaves its model card unissued
//! and its map unshipped: a licence that permits no location is a licence
//! misread, and the desk says so on the cycle line instead of guessing.
//!
//! Paper only. Nothing here can reach a venue; every execution is a replay.

use qip_ai::registry::ModelCard;
use qip_core::error::{Error, Result};
use qip_core::{Duration, ModelId, Timestamp};
use qip_kernel::fusion::{FusionConfig, MarketTrace, OpportunityMap, TracePoint, fuse};
use qip_simulation_engine::conditions::MessageBudget;
use qip_simulation_engine::tick_replay::{
    ExecutionModel, ReplayOrder, ReplayRun, ReplaySession, ReplayTick,
};
use qip_storage::BlobStore;
use qip_storage::lake::Lake;
use qip_storage::lake_codec::{CompressionReport, SegmentCodec, measure_partitions};
use qip_storage::lake_history::{HistoryQuery, MARKET_PREFIX};
use qip_training::microstructure::{
    BenchmarkReport, BookObservation, FillModel, LabelPlan, MicroExample, TradeObservation,
    VolatilityModel, benchmark_fill_model, build_examples, split_in_time, train_fill_model,
    train_volatility_model,
};
use qip_twin::futures::{FutureTree, ObservedMoves, TreeConfig, TwinState, WorldDriver};
use qip_world_model::graph::KnowledgeGraph;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const EVERY_VARIABLE: &str = "QIP_DEEPBRAIN_TICK_EVERY";
pub const ENTITLEMENTS_VARIABLE: &str = "QIP_DEEPBRAIN_TICK_ENTITLEMENTS";
pub const ENTITLEMENT_REGIONS_VARIABLE: &str = "QIP_DEEPBRAIN_TICK_ENTITLEMENT_REGIONS";
pub const MAP_REGIONS_VARIABLE: &str = "QIP_DEEPBRAIN_TICK_MAP_REGIONS";
pub const LOOKBACK_DAYS_VARIABLE: &str = "QIP_DEEPBRAIN_TICK_LOOKBACK_DAYS";
pub const LATENCY_MS_VARIABLE: &str = "QIP_DEEPBRAIN_TICK_LATENCY_MS";
pub const FEE_BPS_VARIABLE: &str = "QIP_DEEPBRAIN_TICK_FEE_BPS";
pub const IMPACT_BPS_VARIABLE: &str = "QIP_DEEPBRAIN_TICK_IMPACT_BPS";
pub const MEASURE_CODEC_VARIABLE: &str = "QIP_DEEPBRAIN_TICK_MEASURE_CODEC";

/// The blob namespace the desk writes its model cards and maps under.
pub const OUTPUT_NAMESPACE: &str = "tick-desk";

/// Bounds on what one round holds. A history beyond either is refused, not
/// cut — a replay over a prefix of a session is a different session.
pub const MAX_SEGMENTS: usize = 4_096;
pub const MAX_TICKS: usize = 2_000_000;

/// The distilled map's capacity and lifetime.
pub const MAP_CAPACITY: usize = 64;
pub const MAP_TTL: Duration = Duration::from_mins(15);

/// The desk's configuration, read once at the composition root.
#[derive(Clone, Debug, PartialEq)]
pub struct TickDeskConfig {
    /// Run every this many cycles; `0` is off.
    pub every_cycles: u64,
    pub granted: BTreeSet<String>,
    /// Where each entitlement's terms permit a model or its outputs to run.
    pub entitlement_regions: BTreeMap<String, BTreeSet<String>>,
    /// The regions the opportunity map is offered to.
    pub map_regions: BTreeSet<String>,
    pub lookback_days: i64,
    pub execution: ExecutionModel,
    /// A codec to measure each round, or none.
    pub measure_codec: Option<SegmentCodec>,
}

impl TickDeskConfig {
    /// The desk switched off.
    pub fn off() -> Self {
        Self {
            every_cycles: 0,
            granted: BTreeSet::new(),
            entitlement_regions: BTreeMap::new(),
            map_regions: BTreeSet::new(),
            lookback_days: 1,
            execution: ExecutionModel {
                latency: Duration::from_millis(5),
                fee_bps: 1.0,
                impact_bps_at_full_depth: 10.0,
                limit_ttl: Duration::from_secs(60),
                // A venue throttle on the replayed flow: a burst beyond it is
                // refused as the venue would refuse it, never filled.
                message_budget: Some(MessageBudget {
                    per_window: 50,
                    window: Duration::from_secs(1),
                }),
            },
            measure_codec: None,
        }
    }

    /// Read from `lookup` (the environment, at the root). Off when the
    /// cadence is unset or zero; refuses an enabled desk with no
    /// entitlement grant, and any value it cannot parse.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let mut config = Self::off();
        let Some(every) = lookup(EVERY_VARIABLE).filter(|v| !v.trim().is_empty()) else {
            return Ok(config);
        };
        config.every_cycles = every.trim().parse().map_err(|_| {
            Error::invalid(format!("{EVERY_VARIABLE}={every:?} is not a cycle count"))
        })?;
        if config.every_cycles == 0 {
            return Ok(config);
        }
        config.granted = lookup(ENTITLEMENTS_VARIABLE)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if config.granted.is_empty() {
            return Err(Error::invalid(format!(
                "{EVERY_VARIABLE} turns the tick desk on and {ENTITLEMENTS_VARIABLE} grants it \
                 nothing to read; name the entitlements the desk may replay, or set the cadence \
                 to 0"
            )));
        }
        if let Some(policy) = lookup(ENTITLEMENT_REGIONS_VARIABLE) {
            for clause in policy.split(';').map(str::trim).filter(|c| !c.is_empty()) {
                let (entitlement, regions) = clause.split_once('=').ok_or_else(|| {
                    Error::invalid(format!(
                        "{ENTITLEMENT_REGIONS_VARIABLE} clause {clause:?} is not \
                         entitlement=region|region"
                    ))
                })?;
                let regions: BTreeSet<String> = regions
                    .split('|')
                    .map(str::trim)
                    .filter(|r| !r.is_empty())
                    .map(str::to_string)
                    .collect();
                config
                    .entitlement_regions
                    .insert(entitlement.trim().to_string(), regions);
            }
        }
        config.map_regions = lookup(MAP_REGIONS_VARIABLE)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if let Some(days) = lookup(LOOKBACK_DAYS_VARIABLE) {
            config.lookback_days = days.trim().parse().map_err(|_| {
                Error::invalid(format!(
                    "{LOOKBACK_DAYS_VARIABLE}={days:?} is not a day count"
                ))
            })?;
            if !(1..=31).contains(&config.lookback_days) {
                return Err(Error::invalid(format!(
                    "{LOOKBACK_DAYS_VARIABLE} must be 1..=31 days; a longer replay is a batch \
                     job, not a desk round"
                )));
            }
        }
        let number = |name: &str| -> Result<Option<f64>> {
            lookup(name)
                .map(|v| {
                    v.trim()
                        .parse::<f64>()
                        .map_err(|_| Error::invalid(format!("{name}={v:?} is not a number")))
                })
                .transpose()
        };
        if let Some(ms) = number(LATENCY_MS_VARIABLE)? {
            if !ms.is_finite() || ms <= 0.0 {
                return Err(Error::invalid(format!(
                    "{LATENCY_MS_VARIABLE} must be a positive number of milliseconds"
                )));
            }
            config.execution.latency = Duration::from_micros((ms * 1_000.0).round() as i64);
        }
        if let Some(fee) = number(FEE_BPS_VARIABLE)? {
            config.execution.fee_bps = fee;
        }
        if let Some(impact) = number(IMPACT_BPS_VARIABLE)? {
            config.execution.impact_bps_at_full_depth = impact;
        }
        config.execution.validate()?;
        if let Some(codec) = lookup(MEASURE_CODEC_VARIABLE).filter(|v| !v.trim().is_empty()) {
            let codec = SegmentCodec::parse(&codec)?;
            if !codec.is_identity() {
                config.measure_codec = Some(codec);
            }
        }
        Ok(config)
    }

    /// Whether the desk runs at all.
    pub const fn enabled(&self) -> bool {
        self.every_cycles > 0
    }

    /// One line for the start-up banner.
    pub fn describe(&self) -> String {
        if !self.enabled() {
            return format!("disabled ({EVERY_VARIABLE}=0)");
        }
        format!(
            "every {} cycle(s), replaying the last {} day(s) of the tick lake under {} \
             entitlement(s) [{}]; maps offered to {} region(s); codec measurement {}",
            self.every_cycles,
            self.lookback_days,
            self.granted.len(),
            self.granted.iter().cloned().collect::<Vec<_>>().join(", "),
            self.map_regions.len(),
            self.measure_codec
                .map_or_else(|| "off".to_string(), |c| format!("{c:?}"))
        )
    }
}

/// The skill gate a fill model passes before a card is issued for it
/// (R12-TICK-2092): a model whose holdout Brier does not beat the base
/// rate's is refused, naming both figures, so no card — and so no
/// opportunity map, which a card authorises — is issued for a model that
/// knows nothing the base rate did not.
pub fn skill_gate(model: &FillModel) -> Result<()> {
    if model.beats_baseline() {
        return Ok(());
    }
    Err(Error::denied(format!(
        "the fill model's holdout Brier {:.4} does not beat its base-rate baseline {:.4} on {} \
         holdout example(s); no model card is issued for a model with no skill, and no map is \
         authorised from it",
        model.holdout_brier, model.baseline_brier, model.holdout_examples
    )))
}

/// What one round did. Every field is a count or a record a person can
/// check; nothing here is a claim the desk makes about itself.
#[derive(Clone, Debug, Default)]
pub struct TickRound {
    pub manifest: String,
    pub segments: usize,
    pub withheld: usize,
    pub ticks: usize,
    pub unparsed: usize,
    pub entitlements_read: BTreeSet<String>,
    pub replay: Option<ReplayRun>,
    pub examples: usize,
    /// The labelled rows themselves, each carrying the venue, venue sequence
    /// and entitlement of the message it was drawn at (R12-TICK-1049–1051),
    /// so whoever takes the round can audit what a model was trained on.
    pub labelled: Vec<MicroExample>,
    /// Ticks excluded because they were stamped knowable before the venue
    /// says they happened (R12-TICK-1053): one of the two clocks is wrong,
    /// and ordering on either reads the other's future.
    pub contaminated: usize,
    pub refused_unreliable: usize,
    pub refused_uncovered: usize,
    pub fill_model: Option<FillModel>,
    pub volatility_model: Option<VolatilityModel>,
    pub benchmark: Option<BenchmarkReport>,
    /// Why a model was not trained, where it was not.
    pub training_refused: Vec<String>,
    pub model_card: Option<ModelCard>,
    pub card_refused: Option<String>,
    pub fused_events: usize,
    pub mismatches: usize,
    pub map: Option<OpportunityMap>,
    pub shipped_to: BTreeSet<String>,
    pub refused_regions: BTreeMap<String, String>,
    pub branches_grown: usize,
    pub branches_live: usize,
    pub tree_log_score: Option<f64>,
    pub compression: Option<CompressionReport>,
}

impl TickRound {
    pub fn describe(&self) -> String {
        if self.segments == 0 {
            return format!(
                "tick desk: no market history under the granted entitlements ({} withheld); \
                 nothing replayed",
                self.withheld
            );
        }
        let replay = self.replay.as_ref().map_or_else(String::new, |r| {
            format!(
                "{} step(s), {} flagged unreliable in {} span(s), digest {}",
                r.steps,
                r.unreliable_steps.len(),
                r.spans.len(),
                &r.digest[..12.min(r.digest.len())]
            )
        });
        format!(
            "tick desk: manifest {} — {} segment(s), {} tick(s) ({} unparsed, {} withheld) \
             under [{}]; replay {replay}; {} example(s) ({} refused unreliable, {} uncovered); \
             fill model {}; card {}; {} event(s) fused, {} mismatch(es); map shipped to [{}], \
             refused {:?}; future tree {} branch(es), {} live{}",
            self.manifest,
            self.segments,
            self.ticks,
            self.unparsed,
            self.withheld,
            self.entitlements_read
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            self.examples,
            self.refused_unreliable,
            self.refused_uncovered,
            self.fill_model.as_ref().map_or_else(
                || format!("not trained ({})", self.training_refused.join("; ")),
                |m| format!(
                    "Brier {:.4} vs baseline {:.4} ({})",
                    m.holdout_brier,
                    m.baseline_brier,
                    if m.beats_baseline() {
                        "beats it"
                    } else {
                        "does not beat it"
                    }
                )
            ),
            self.model_card.as_ref().map_or_else(
                || format!(
                    "withheld ({})",
                    self.card_refused.clone().unwrap_or_default()
                ),
                ModelCard::reference
            ),
            self.fused_events,
            self.mismatches,
            self.shipped_to
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            self.refused_regions.keys().collect::<Vec<_>>(),
            self.branches_grown,
            self.branches_live,
            self.tree_log_score
                .map_or_else(String::new, |s| format!(", previous tree scored {s:.3}"))
        )
    }
}

/// The desk. See the module doc.
#[derive(Debug)]
pub struct TickDesk {
    config: TickDeskConfig,
    lake: Arc<dyn BlobStore>,
    outputs: Arc<dyn BlobStore>,
    rounds: u64,
    tree: Option<FutureTree>,
}

impl TickDesk {
    pub fn new(
        config: TickDeskConfig,
        lake: Arc<dyn BlobStore>,
        outputs: Arc<dyn BlobStore>,
    ) -> Self {
        Self {
            config,
            lake,
            outputs,
            rounds: 0,
            tree: None,
        }
    }

    pub const fn config(&self) -> &TickDeskConfig {
        &self.config
    }

    /// The synthetic future tree the last round grew.
    pub const fn tree(&self) -> Option<&FutureTree> {
        self.tree.as_ref()
    }

    /// Run a round if this cycle is on the cadence.
    pub fn maybe_round(
        &mut self,
        graph: &KnowledgeGraph,
        cycle: u64,
        now: Timestamp,
    ) -> Result<Option<TickRound>> {
        if !self.config.enabled() || !cycle.is_multiple_of(self.config.every_cycles) {
            return Ok(None);
        }
        self.round(graph, now, Vec::new()).map(Some)
    }

    /// One round over the lake as of `now`, replaying `decisions` against
    /// the recorded history beside the rebuild.
    pub fn round(
        &mut self,
        graph: &KnowledgeGraph,
        now: Timestamp,
        decisions: Vec<ReplayOrder>,
    ) -> Result<TickRound> {
        self.rounds += 1;
        let mut round = TickRound {
            manifest: format!("tick-desk-{}-{}", now.as_nanos(), self.rounds),
            ..TickRound::default()
        };
        let lake = Lake::new(self.lake.as_ref());
        let from = now.saturating_sub(Duration::from_days(self.config.lookback_days - 1));
        let query = HistoryQuery {
            from_date: from.to_date_string(),
            to_date: now.to_date_string(),
            venues: BTreeSet::new(),
            instruments: BTreeSet::new(),
            granted: self.config.granted.clone(),
            max_segments: MAX_SEGMENTS,
            max_ticks: MAX_TICKS,
        };
        if let Some(codec) = self.config.measure_codec {
            round.compression = Some(measure_partitions(
                self.lake.as_ref(),
                MARKET_PREFIX,
                codec,
            )?);
        }
        let listing = lake.pin_market_history(&round.manifest, &query)?;
        round.segments = listing.segments.len();
        round.withheld = listing.withheld.len();
        if listing.segments.is_empty() {
            return Ok(round);
        }
        let history = lake.read_market_history(&round.manifest, &self.config.granted, MAX_TICKS)?;
        round.unparsed = history.unparsed;
        round.entitlements_read = history.entitlements_read.clone();
        // A tick stamped knowable after `now` is in the lake and not in the
        // desk's present; reading it would be reading the future.
        //
        // A tick received before the venue says it happened is timestamp
        // contamination (R12-TICK-1053): it is excluded and counted rather
        // than replayed on whichever clock happens to be read.
        //
        // The entitlement each tick was obtained under is kept beside it by
        // its origin, so every training row can name it (R12-TICK-1051).
        let mut ticks: Vec<ReplayTick> = Vec::new();
        let mut lineage: BTreeMap<(String, String, u32, u64), String> = BTreeMap::new();
        for tick in history.ticks {
            if tick.knowable_at > now {
                continue;
            }
            if tick.knowable_at < tick.event_time {
                round.contaminated += 1;
                continue;
            }
            lineage.insert(origin_key(&tick.message), tick.entitlement);
            ticks.push(ReplayTick {
                message: tick.message,
                knowable_at: tick.knowable_at,
            });
        }
        round.ticks = ticks.len();

        // --- replay, and the observations every later step reads ----------
        let mut session =
            ReplaySession::new(self.config.execution, ticks.clone(), decisions, Vec::new())?;
        let ordered = qip_simulation_engine::tick_replay::sequence_ticks(ticks);
        let mut observations = Vec::new();
        let mut trades = Vec::new();
        let mut traces: BTreeMap<(String, String), Vec<TracePoint>> = BTreeMap::new();
        for tick in &ordered {
            if !session.step()? {
                break;
            }
            let instrument = tick.message.object_id.as_str().to_string();
            let venue = tick.message.origin.venue.as_str().to_string();
            if let qip_contracts::message::MessageBody::Trade { price, .. } = &tick.message.body {
                trades.push(TradeObservation {
                    at: tick.knowable_at,
                    instrument: instrument.clone(),
                    // Statistics from here on: prices cross to f64.
                    price: price.to_f64(),
                });
            }
            let unreliable = session.is_unreliable(&instrument);
            let Some(state) = session.book(&instrument) else {
                continue;
            };
            let (Some(bid), Some(ask)) = (state.best_bid(), state.best_ask()) else {
                continue;
            };
            let observation = BookObservation {
                at: tick.knowable_at,
                venue: venue.clone(),
                sequence: tick.message.origin.sequence,
                // Blank only if the tick was never in the lineage map, which
                // `build_examples` refuses rather than labels.
                entitlement: lineage
                    .get(&origin_key(&tick.message))
                    .cloned()
                    .unwrap_or_default(),
                instrument: instrument.clone(),
                best_bid: bid.price.to_f64(),
                best_ask: ask.price.to_f64(),
                bid_size: bid.size.to_f64(),
                ask_size: ask.size.to_f64(),
                unreliable,
            };
            if !unreliable && observation.best_ask > observation.best_bid {
                let mid = (observation.best_bid + observation.best_ask) / 2.0;
                traces
                    .entry((instrument, venue))
                    .or_default()
                    .push(TracePoint {
                        at: tick.knowable_at,
                        mid,
                        spread_bps: (observation.best_ask - observation.best_bid) / mid * 10_000.0,
                    });
            }
            observations.push(observation);
        }
        round.replay = Some(session.run_to_end()?);

        // --- labels, models, benchmark --------------------------------------
        let plan = LabelPlan {
            horizons: vec![Duration::from_secs(1), Duration::from_secs(10)],
            sample_every: 1,
            trailing: 5,
            volatile_above_bps: 5.0,
        };
        let labelled = build_examples(&observations, &trades, &plan)?;
        round.examples = labelled.examples.len();
        round.refused_unreliable = labelled.refused_unreliable;
        round.refused_uncovered = labelled.refused_uncovered;
        match train_fill_model(&labelled.examples, 0.3) {
            Ok(model) => {
                let (_, holdout) = split_in_time(&labelled.examples, 0.3)?;
                round.benchmark = Some(benchmark_fill_model(&model, &holdout));
                round.fill_model = Some(model);
            }
            Err(error) => round
                .training_refused
                .push(format!("fill: {}", error.message())),
        }
        match train_volatility_model(&labelled.examples, 0.3) {
            Ok(model) => round.volatility_model = Some(model),
            Err(error) => round
                .training_refused
                .push(format!("volatility: {}", error.message())),
        }
        round.labelled = labelled.examples;

        // --- the model card and its entitlements ------------------------------
        let mut permitted: Option<BTreeSet<String>> = None;
        for entitlement in &round.entitlements_read {
            let regions = self
                .config
                .entitlement_regions
                .get(entitlement)
                .cloned()
                .unwrap_or_default();
            permitted = Some(match permitted {
                None => regions,
                Some(so_far) => so_far.intersection(&regions).cloned().collect(),
            });
        }
        // A model whose holdout skill does not beat the base rate is refused
        // a card (R12-TICK-2092): the card is what authorises the map a
        // region is shipped, so issuing one for a model with no skill would
        // ship a map distilled from a model that knows nothing the base
        // rate did not. Until 2026-10-18 the desk printed `beats_baseline`
        // on the round line and issued the card regardless.
        let card = round.fill_model.as_ref().map(|model| {
            skill_gate(model)?;
            ModelCard::new(
                ModelId::from_string(format!("tick-fill-{}", round.manifest)),
                "tick-fill-probability",
                round.manifest.clone(),
                "qip-deepbrain/tick-desk",
                now,
            )
            .with_purpose(
                "probability a bid resting at the touch is traded through within the horizon",
            )
            .with_features(model.features.clone())
            .with_training_data(vec![format!("lake manifest {}", round.manifest)])
            .with_benchmark("training base rate")
            .with_data_entitlements(
                round.entitlements_read.clone(),
                permitted.clone().unwrap_or_default(),
            )
        });
        match card {
            Some(Ok(card)) => {
                self.outputs.put(
                    &format!("{OUTPUT_NAMESPACE}/model-cards/{}.json", round.manifest),
                    serde_json::to_vec(&card)?,
                )?;
                round.model_card = Some(card);
            }
            Some(Err(error)) => round.card_refused = Some(error.message().to_string()),
            None => round.card_refused = Some("no fill model was trained".to_string()),
        }

        // --- fusion and the distilled map ------------------------------------
        let traces: Vec<MarketTrace> = traces
            .into_iter()
            .map(|((instrument, venue), points)| MarketTrace {
                instrument,
                venue,
                points,
            })
            .collect();
        let fusion_config = FusionConfig {
            max_hops: 2,
            reaction_threshold_bps: 5.0,
            reaction_window: Duration::from_mins(5),
            expect_reaction_above: 0.5,
            weak_evidence_below: 0.4,
            max_events: 1_024,
        };
        let fused = fuse(graph, &traces, now, &fusion_config)?;
        round.fused_events = fused.events.len();
        round.mismatches = fused.mismatches().len();
        let map = fused.opportunity_map(MAP_CAPACITY, MAP_TTL)?;
        for region in &self.config.map_regions {
            let authorised = match &round.model_card {
                Some(card) => map.authorise_for(card, region),
                None => Err(Error::denied(
                    "no model card was issued for the data the map was distilled from",
                )),
            };
            match authorised {
                Ok(()) => {
                    self.outputs.put(
                        &format!(
                            "{OUTPUT_NAMESPACE}/opportunity-maps/region={region}/{}.json",
                            round.manifest
                        ),
                        serde_json::to_vec(&map)?,
                    )?;
                    round.shipped_to.insert(region.clone());
                }
                Err(error) => {
                    round
                        .refused_regions
                        .insert(region.clone(), error.message().to_string());
                }
            }
        }
        round.map = Some(map);

        // --- synthetic futures -------------------------------------------------
        let mut series: BTreeMap<String, Vec<(Timestamp, f64)>> = BTreeMap::new();
        for trace in &traces {
            series
                .entry(trace.instrument.clone())
                .or_default()
                .extend(trace.points.iter().map(|p| (p.at, p.mid)));
        }
        if let Some(previous) = self.tree.as_mut() {
            let mut moves = BTreeMap::new();
            for (instrument, points) in &series {
                let last = points.iter().max_by_key(|(at, _)| *at).map(|(_, m)| *m);
                let before = previous_last_mid(previous, instrument);
                if let (Some(last), Some(before)) = (last, before)
                    && before > 0.0
                {
                    moves.insert(instrument.clone(), (last / before - 1.0) * 10_000.0);
                }
            }
            if !moves.is_empty() && now > previous.grown_at {
                let report = previous.observe(&ObservedMoves {
                    at: now,
                    moves_bps: moves,
                })?;
                round.tree_log_score = Some(report.log_score);
            }
        }
        if let Ok(state) = TwinState::fit(&series, now) {
            let drivers: Vec<WorldDriver> = fused
                .events
                .iter()
                .map(|event| WorldDriver {
                    id: event.event.clone(),
                    sensitivities: event
                        .watch
                        .instruments
                        .values()
                        .map(|w| {
                            let direction = event.priors.first().map_or(1.0, |p| {
                                if p.expected_direction == 0.0 {
                                    1.0
                                } else {
                                    p.expected_direction
                                }
                            });
                            (w.instrument.clone(), w.exposure_confidence * direction)
                        })
                        .collect(),
                    evidence: vec![event.event.clone()],
                })
                .collect();
            let tree = FutureTree::grow(
                &state,
                &drivers,
                TreeConfig {
                    horizon: Duration::from_mins(5),
                    shock_sigmas: vec![1.0, 2.0, 4.0],
                    propagation: vec![0.0, 0.5, 1.0],
                    prune_below: 0.001,
                    split_beyond_sigmas: 3.0,
                    max_branches: 4_096,
                },
            )?;
            round.branches_grown = tree.branches.len();
            round.branches_live = tree.live().count();
            self.tree = Some(tree);
        }
        Ok(round)
    }
}

/// A message's place in its venue's stream: venue, feed, partition and the
/// venue's own sequence, which together name one message.
fn origin_key(message: &qip_contracts::message::MarketMessage) -> (String, String, u32, u64) {
    let origin = &message.origin;
    (
        origin.venue.as_str().to_string(),
        origin.feed.clone(),
        origin.partition,
        origin.sequence,
    )
}

/// The last mid the tree's twin state saw for `instrument`.
fn previous_last_mid(tree: &FutureTree, instrument: &str) -> Option<f64> {
    tree.state().instruments.get(instrument).map(|d| d.last_mid)
}
