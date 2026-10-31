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
use qip_core::{Decimal, Duration, ModelId, Timestamp};
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_kernel::fusion::{FusionConfig, MarketTrace, OpportunityMap, TracePoint, fuse};
use qip_kernel::platform::FillScore;
use qip_simulation_engine::conditions::MessageBudget;
use qip_simulation_engine::tick_replay::{
    ExecutionModel, OrderSide, ReplayOrder, ReplayRun, ReplaySession, ReplayTick,
};
use qip_storage::BlobStore;
use qip_storage::lake::Lake;
use qip_storage::lake_codec::{CompressionReport, SegmentCodec, measure_partitions};
use qip_storage::lake_history::{HistoryQuery, MARKET_PREFIX};
use qip_training::microstructure::{
    BenchmarkReport, BookObservation, FillModel, FillResidualObservation, LabelPlan, LabelledSet,
    MicroExample, ResidualExample, TradeObservation, VolatilityModel, benchmark_fill_model,
    build_examples, join_fill_residuals, split_in_time, train_fill_model, train_volatility_model,
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
pub const OUTPUT_NAMESPACE: &str = qip_storage::classes::TICK_DESK_NAMESPACE;

/// Bounds on what one round holds. A history beyond either is refused, not
/// cut — a replay over a prefix of a session is a different session.
pub const MAX_SEGMENTS: usize = 4_096;
pub const MAX_TICKS: usize = 2_000_000;

/// The distilled map's capacity and lifetime.
pub const MAP_CAPACITY: usize = 64;

/// The most outside priors one round hands REASON; the kernel refuses more.
pub const MAX_OUTSIDE_PRIORS: usize = 1_024;
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
    /// The venue fills this round was handed, and the rows they became
    /// (R12-TICK-6014): each fill whose twin entry was off, joined to the
    /// labelled instant it followed. The training and evaluation rows the
    /// platform's own execution produced, beside the replayed ones.
    pub residuals_offered: usize,
    pub residual_examples: Vec<ResidualExample>,
    /// Fills with no replayed history on their venue and instrument.
    pub residuals_unmatched: usize,
    /// The fill model scored on the residual rows that fall in its holdout,
    /// and only those: a row from the fitted portion would grade the model
    /// on data it saw.
    pub residual_benchmark: Option<BenchmarkReport>,
    /// The platform's own orders the round replayed against the recorded
    /// market, each beside what the venue actually filled it at
    /// (R12-TICK-2073): the strategy judged against the market that existed,
    /// not against bars built after the fact.
    pub order_rows: Vec<OrderReplayRow>,
    /// Orders in the window the replay cannot express (a scheduled
    /// algorithm with no single limit), counted rather than replayed as a
    /// market order nobody sent.
    pub unreplayable_orders: usize,
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
    /// Live branches written to the outputs store as §24.9 `WorldBranch`
    /// contracts (ADR 0151 decision 5: the tick desk is where trees are grown,
    /// so it is where they are journaled), and the live ones the contract
    /// refused to state: the baseline, which is its parent's own future, and
    /// any whose propagation no basis-point strength can state. The two add up
    /// to `branches_live`, so a branch is never silently absent.
    pub world_branches: usize,
    pub world_branches_refused: usize,
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
             under [{}]; replay {replay}; {} example(s) ({} refused unreliable, {} uncovered), \
             {} of {} venue fill(s) joined as residual rows ({} unmatched); \
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
            self.residual_examples.len(),
            self.residuals_offered,
            self.residuals_unmatched,
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

/// The fills whose twin entry could be compared with the venue's, as the
/// training side reads them. A fill the twin did not simulate has no error
/// and so no row; the desk does not invent a zero for it.
pub fn fill_residuals(fills: &[FillScore]) -> Vec<FillResidualObservation> {
    fills
        .iter()
        .filter_map(|fill| {
            Some(FillResidualObservation {
                venue: fill.venue.clone(),
                instrument: fill.object_id.as_str().to_string(),
                filled_at: fill.filled_at,
                error_bps: fill.trade_error_bps?,
            })
        })
        .collect()
}

/// The desk. See the module doc.
#[derive(Debug)]
pub struct TickDesk {
    config: TickDeskConfig,
    lake: Arc<dyn BlobStore>,
    outputs: Arc<dyn BlobStore>,
    rounds: u64,
    tree: Option<FutureTree>,
    /// The strongest fused posterior per instrument from the last round
    /// (`FusedState::posteriors`), which the evolution engine reads as a
    /// search prior (R12-TICK-2085).
    posteriors: BTreeMap<String, f64>,
    /// The same fused state as beliefs REASON may be narrowed by, never
    /// raised (ADR 0151 decision 3, R12-TICK-1002).
    outside_priors: Vec<qip_kernel::fusion::OutsidePrior>,
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
            posteriors: BTreeMap::new(),
            outside_priors: Vec::new(),
        }
    }

    pub const fn config(&self) -> &TickDeskConfig {
        &self.config
    }

    /// What the last round's fused events believe about each instrument.
    pub const fn posteriors(&self) -> &BTreeMap<String, f64> {
        &self.posteriors
    }

    /// What the last round's fused events would have REASON be careful of: the
    /// strongest directional posterior per instrument, at most
    /// [`MAX_OUTSIDE_PRIORS`], ranked by strength so the cap drops the weakest
    /// rather than whichever sorted last.
    pub fn outside_priors(&self) -> &[qip_kernel::fusion::OutsidePrior] {
        &self.outside_priors
    }

    /// The synthetic future tree the last round grew.
    pub const fn tree(&self) -> Option<&FutureTree> {
        self.tree.as_ref()
    }

    /// Run a round if this cycle is on the cadence, replaying the platform's
    /// own orders from the lookback window against the recorded market and
    /// recording, per order, the replayed fill beside the venue's
    /// (R12-TICK-2073).
    ///
    /// Until 2026-10-24 the desk's only caller handed the round no
    /// decisions, so the replay rebuilt books and traded nothing of the
    /// platform's: no strategy was ever evaluated against the market that
    /// existed.
    pub fn maybe_round_with_orders<'a>(
        &mut self,
        graph: &KnowledgeGraph,
        cycle: u64,
        now: Timestamp,
        orders: impl IntoIterator<Item = &'a Order>,
        fills: &[FillScore],
    ) -> Result<Option<TickRound>> {
        if !self.config.enabled() || !cycle.is_multiple_of(self.config.every_cycles) {
            return Ok(None);
        }
        let from = now.saturating_sub(Duration::from_days(self.config.lookback_days));
        let replayable = replay_orders_of(orders, from, now);
        let mut round = self.round_with_fills(graph, now, replayable.decisions, fills)?;
        round.unreplayable_orders = replayable.unreplayable;
        round.order_rows = order_rows(round.replay.as_ref(), &replayable.actual);
        Ok(Some(round))
    }

    /// Run a round if this cycle is on the cadence.
    pub fn maybe_round(
        &mut self,
        graph: &KnowledgeGraph,
        cycle: u64,
        now: Timestamp,
    ) -> Result<Option<TickRound>> {
        self.maybe_round_with_fills(graph, cycle, now, &[])
    }

    /// [`Self::maybe_round`] handing the round the platform's scored fills.
    pub fn maybe_round_with_fills(
        &mut self,
        graph: &KnowledgeGraph,
        cycle: u64,
        now: Timestamp,
        fills: &[FillScore],
    ) -> Result<Option<TickRound>> {
        if !self.config.enabled() || !cycle.is_multiple_of(self.config.every_cycles) {
            return Ok(None);
        }
        self.round_with_fills(graph, now, Vec::new(), fills)
            .map(Some)
    }

    /// One round over the lake as of `now`, replaying `decisions` against
    /// the recorded history beside the rebuild.
    pub fn round(
        &mut self,
        graph: &KnowledgeGraph,
        now: Timestamp,
        decisions: Vec<ReplayOrder>,
    ) -> Result<TickRound> {
        self.round_with_fills(graph, now, decisions, &[])
    }

    /// [`Self::round`] with the platform's scored venue fills, whose twin
    /// error becomes residual rows (R12-TICK-6014).
    pub fn round_with_fills(
        &mut self,
        graph: &KnowledgeGraph,
        now: Timestamp,
        decisions: Vec<ReplayOrder>,
        fills: &[FillScore],
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
        // Each present tick's capture-clock doubt, for the precision the
        // labels below need (R12-RES-7103). Taken before the loop consumes
        // the history, over the same ticks the loop keeps.
        let precision_events: Vec<(Timestamp, Duration)> = history
            .ticks
            .iter()
            .filter(|tick| tick.knowable_at <= now)
            .map(|tick| (tick.knowable_at, tick.clock_uncertainty))
            .collect();
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
                        // Sizes are the top of book as the replay held it.
                        depth: Some(observation.bid_size + observation.ask_size),
                        imbalance: (observation.bid_size + observation.ask_size > 0.0).then(|| {
                            (observation.bid_size - observation.ask_size)
                                / (observation.bid_size + observation.ask_size)
                        }),
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
        // A window captured under a clock less precise than the shortest
        // label horizon needs is not labelled at all (R12-RES-7103): an
        // ordering the clock cannot vouch for would become a label a model
        // learns from as if it were exact. Refused whole and recorded, so
        // nothing below trains or promotes from it.
        let labelled = match plan
            .clock_precision()?
            .admit(&round.manifest, precision_events)
        {
            Ok(_) => build_examples(&observations, &trades, &plan)?,
            Err(imprecise) => {
                round
                    .training_refused
                    .push(format!("labels: {}", Error::from(imprecise).message()));
                LabelledSet::default()
            }
        };
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

        // --- the platform's own fills, as rows --------------------------------
        let residuals = fill_residuals(fills);
        round.residuals_offered = residuals.len();
        let joined = join_fill_residuals(&round.labelled, &residuals)?;
        round.residuals_unmatched = joined.unmatched;
        if let (Some(model), Ok((_, holdout))) =
            (&round.fill_model, split_in_time(&round.labelled, 0.3))
            && let Some(first) = holdout.first().map(|e| e.at)
        {
            let held: Vec<MicroExample> = joined
                .rows
                .iter()
                .filter(|row| row.example.at >= first)
                .map(|row| row.example.clone())
                .collect();
            if !held.is_empty() {
                round.residual_benchmark = Some(benchmark_fill_model(model, &held));
            }
        }
        round.residual_examples = joined.rows;

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
        self.posteriors = fused.posteriors();
        let mut priors = fused.outside_priors();
        // Strongest first: the smallest multiplier either way is the prior that
        // can narrow most, and a stable sort keeps instrument order on ties.
        let strength = |prior: &qip_kernel::fusion::OutsidePrior| {
            prior.narrowing_for(1.0).min(prior.narrowing_for(-1.0))
        };
        priors.sort_by(|a, b| strength(a).total_cmp(&strength(b)));
        priors.truncate(MAX_OUTSIDE_PRIORS);
        self.outside_priors = priors;
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
            let mut contracts = Vec::new();
            for branch in tree.live() {
                match branch.to_world_branch() {
                    Ok(contract) => contracts.push(contract),
                    Err(_) => round.world_branches_refused += 1,
                }
            }
            round.world_branches = contracts.len();
            if !contracts.is_empty() {
                self.outputs.put(
                    &format!("{OUTPUT_NAMESPACE}/world-branches/{}.json", round.manifest),
                    serde_json::to_vec(&contracts)?,
                )?;
            }
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

/// The most orders one round replays: the newest in the window are kept.
pub const MAX_REPLAY_ORDERS: usize = 1_024;

/// One of the platform's orders replayed against the recorded market.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderReplayRow {
    pub order_id: String,
    pub instrument: String,
    /// Volume-weighted price the replay filled at, impact included; `None`
    /// when the recorded market filled nothing.
    pub replayed_price: Option<Decimal>,
    /// Volume-weighted price the venue filled the order at; `None` when it
    /// has no fill.
    pub actual_price: Option<Decimal>,
    /// `(replayed - actual) / actual` in basis points, where both exist. A
    /// statistic: the money figures cross to `f64` here and nowhere else.
    pub gap_bps: Option<f64>,
}

/// The platform's orders, as the replay can take them.
#[derive(Debug, Default)]
pub struct ReplayableOrders {
    pub decisions: Vec<ReplayOrder>,
    /// Each replayed order's actual volume-weighted fill price, if filled.
    pub actual: BTreeMap<String, Decimal>,
    pub unreplayable: usize,
}

/// The orders created in `[from, now]` as replay decisions, newest
/// [`MAX_REPLAY_ORDERS`] kept. A market order replays as one; a limit order
/// at its limit; a scheduled algorithm (time-, volume-weighted,
/// participation) has no single instruction the replay can send and is
/// counted, never replayed as a market order nobody sent.
pub fn replay_orders_of<'a>(
    orders: impl IntoIterator<Item = &'a Order>,
    from: Timestamp,
    now: Timestamp,
) -> ReplayableOrders {
    let mut window: Vec<&Order> = orders
        .into_iter()
        .filter(|order| order.created_at >= from && order.created_at <= now)
        .collect();
    window.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.order_id.as_str().cmp(b.order_id.as_str()))
    });
    let skip = window.len().saturating_sub(MAX_REPLAY_ORDERS);
    let mut out = ReplayableOrders::default();
    for order in window.into_iter().skip(skip) {
        let limit = match order.order_type {
            OrderType::Market => None,
            OrderType::Limit { price } => Some(price),
            OrderType::TimeWeighted { .. }
            | OrderType::VolumeWeighted { .. }
            | OrderType::Participation { .. } => {
                out.unreplayable += 1;
                continue;
            }
        };
        let id = order.order_id.as_str().to_string();
        let filled: Decimal = order.fills.iter().map(|fill| fill.quantity).sum();
        let notional: Decimal = order
            .fills
            .iter()
            .map(|fill| fill.price * fill.quantity)
            .sum();
        if let Some(price) = notional.checked_div(filled) {
            out.actual.insert(id.clone(), price);
        }
        out.decisions.push(ReplayOrder {
            id,
            instrument: order.object_id.as_str().to_string(),
            side: match order.side {
                Side::Buy => OrderSide::Buy,
                Side::Sell => OrderSide::Sell,
            },
            quantity: order.quantity,
            limit,
            decided_at: order.created_at,
        });
    }
    out
}

/// One row per replayed order: what the recorded market gave it beside what
/// the venue did.
fn order_rows(
    replay: Option<&ReplayRun>,
    actual: &BTreeMap<String, Decimal>,
) -> Vec<OrderReplayRow> {
    let Some(replay) = replay else {
        return Vec::new();
    };
    replay
        .executions
        .iter()
        .map(|execution| {
            let replayed = execution.average_price();
            let real = actual.get(&execution.order_id).copied();
            let gap_bps = match (replayed, real) {
                (Some(r), Some(a)) if a.is_positive() => {
                    Some((r.to_f64() - a.to_f64()) / a.to_f64() * 10_000.0)
                }
                _ => None,
            };
            OrderReplayRow {
                order_id: execution.order_id.clone(),
                instrument: execution.instrument.clone(),
                replayed_price: replayed,
                actual_price: real,
                gap_bps,
            }
        })
        .collect()
}
