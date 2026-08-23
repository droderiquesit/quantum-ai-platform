//! The deterministic market simulator.
//!
//! Everything the rest of this crate models about a market that is not
//! behaving — the book, the conditions, the fills — is assembled here into one
//! object that answers three questions reproducibly: what was the market at
//! this instant, what would this order have done to it, and what did the run
//! end up owning.
//!
//! Two commitments shape the whole module.
//!
//! **The simulator is never more generous than reality.** An order is filled
//! by sweeping the book, never at the touch regardless of size; a fill can
//! never exceed the depth the book is showing; where the touch is inverted the
//! taker is charged the *worse* of the two prices; and everything paid beyond
//! the reference is scaled by the regime rather than discounted by it. The
//! direction is asserted, not asserted-to: see
//! [`crate::execution::ExecutionReport::adversity_bps`].
//!
//! **Determinism is the product.** There is no clock and no ambient RNG here.
//! Instants arrive as [`Timestamp`] parameters; every draw comes from a stream
//! seeded on the run seed. The synthetic path is generated once at
//! construction, so asking about instants out of order gives the same answers
//! as asking in order, and [`SimulationRun::digest`] turns "the same run"
//! into something a test can compare byte for byte.
//!
//! One detail is worth stating because a property depends on it: the simulated
//! book's *shape* is scale-free. The half-spread and the level spacing are
//! basis points of the mid and the level sizes are in units, so moving the
//! price level — which is exactly what a flash event does — leaves slippage in
//! basis points unchanged. That is what makes "injecting a condition never
//! improves the execution" an exact statement rather than an approximate one:
//! a condition that only moves the price cannot flatter the execution, and
//! every condition that touches the spread, the depth or the fill does so in
//! one direction.

use crate::conditions::{ConditionSchedule, FeedFault, Regime};
use crate::costs::CostModel;
use crate::execution::{
    ExecutionPlan, ExecutionReport, FillSlice, FillStatus, PlanReport, SimOrder,
};
use crate::venue::{Mark, MarkSource, SimBook};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_market::bar::Bar;
use qip_market::book::Side;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The calm market for one instrument: what the book looks like when nothing
/// is wrong with it.
///
/// Every field is an assumption, and the ones that decide whether a fill is
/// possible — `level_size` and `levels` — are quantities rather than
/// percentages, because that is the form in which a book either can or cannot
/// supply an order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstrumentSpec {
    pub object_id: String,
    /// Mid at the first instant of a synthetic run. Ignored on replay, where
    /// the bars supply the path.
    pub initial_price: Decimal,
    /// Calm half-spread, in basis points of the mid.
    pub half_spread_bps: f64,
    /// Distance between consecutive levels, in basis points of the mid.
    pub level_spacing_bps: f64,
    /// Size resting at each level in the calm market.
    pub level_size: Decimal,
    /// Levels published per side.
    pub levels: usize,
    /// Daily volume, for the participation term of the impact model.
    pub daily_volume: f64,
    /// Per-step return volatility in the calm market.
    pub step_volatility: f64,
    /// Per-step drift.
    pub step_drift: f64,
}

impl InstrumentSpec {
    /// A liquid instrument: a basis point of half-spread and ten levels a side.
    pub fn liquid(object_id: impl Into<String>, initial_price: Decimal) -> Self {
        Self {
            object_id: object_id.into(),
            initial_price,
            half_spread_bps: 1.0,
            level_spacing_bps: 2.0,
            level_size: Decimal::from_int(1_000),
            levels: 10,
            daily_volume: 5_000_000.0,
            step_volatility: 0.004,
            step_drift: 0.0,
        }
    }

    /// A thin instrument: a wide touch, few levels and little size on them.
    pub fn thin(object_id: impl Into<String>, initial_price: Decimal) -> Self {
        Self {
            object_id: object_id.into(),
            initial_price,
            half_spread_bps: 20.0,
            level_spacing_bps: 25.0,
            level_size: Decimal::from_int(100),
            levels: 3,
            daily_volume: 60_000.0,
            step_volatility: 0.02,
            step_drift: 0.0,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.object_id.trim().is_empty() {
            return Err(Error::invalid("an instrument needs an identifier"));
        }
        if !self.initial_price.is_positive() {
            return Err(Error::invalid(format!(
                "{} needs a positive initial price",
                self.object_id
            )));
        }
        if !self.half_spread_bps.is_finite() || self.half_spread_bps <= 0.0 {
            return Err(Error::invalid(format!(
                "{} needs a positive half-spread; a zero spread is a market that pays you to trade",
                self.object_id
            )));
        }
        if !self.level_spacing_bps.is_finite() || self.level_spacing_bps <= 0.0 {
            return Err(Error::invalid(format!(
                "{} needs a positive gap between levels",
                self.object_id
            )));
        }
        if !self.level_size.is_positive() || self.levels == 0 {
            return Err(Error::invalid(format!(
                "{} needs at least one level with size on it",
                self.object_id
            )));
        }
        if !self.daily_volume.is_finite() || self.daily_volume <= 0.0 {
            return Err(Error::invalid(format!(
                "{} needs a positive daily volume for the impact model",
                self.object_id
            )));
        }
        if !self.step_volatility.is_finite() || self.step_volatility < 0.0 {
            return Err(Error::invalid(format!(
                "{} needs a non-negative step volatility",
                self.object_id
            )));
        }
        if !self.step_drift.is_finite() {
            return Err(Error::invalid(format!(
                "{} has a non-finite drift",
                self.object_id
            )));
        }
        Ok(())
    }
}

/// A synthetic market to generate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyntheticMarket {
    pub start: Timestamp,
    pub step: Duration,
    pub steps: usize,
    pub venues: Vec<String>,
    pub instruments: Vec<InstrumentSpec>,
}

impl SyntheticMarket {
    /// One venue, one instrument, `steps` steps of `step`.
    pub fn single(
        start: Timestamp,
        step: Duration,
        steps: usize,
        venue: impl Into<String>,
        instrument: InstrumentSpec,
    ) -> Self {
        Self {
            start,
            step,
            steps,
            venues: vec![venue.into()],
            instruments: vec![instrument],
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.steps == 0 {
            return Err(Error::invalid("a synthetic market needs at least one step"));
        }
        if self.step.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a synthetic market's step must move forward",
            ));
        }
        if self.venues.is_empty() {
            return Err(Error::invalid(
                "a synthetic market needs at least one venue",
            ));
        }
        if self.instruments.is_empty() {
            return Err(Error::invalid(
                "a synthetic market needs at least one instrument",
            ));
        }
        for instrument in &self.instruments {
            instrument.validate()?;
        }
        Ok(())
    }
}

/// Where the undisturbed price path came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    /// A seeded random walk. Reproducible, and honest about being invented.
    Synthetic,
    /// Recorded bars, replayed at their close times.
    ///
    /// Credible because it happened, and limited for the same reason: it can
    /// only ever contain the conditions the recording contained, which is why
    /// the conditions in [`crate::conditions`] are injected on top rather than
    /// hoped for.
    Historical,
}

impl PriceSource {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Synthetic => "synthetic",
            Self::Historical => "historical",
        }
    }
}

/// One point on an instrument's undisturbed path.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct PathPoint {
    at: Timestamp,
    price: Decimal,
    volume: f64,
}

/// A read-only picture of the market at one instant, as the strategy sees it.
///
/// What the strategy sees is not what the simulator knows. Every price here
/// arrives as a [`Mark`] that carries its own age, so a strategy running
/// behind a delayed feed reads exactly what it would read in production —
/// including that the number is old.
#[derive(Debug)]
pub struct MarketView<'a> {
    at: Timestamp,
    marks: &'a BTreeMap<String, Mark>,
}

impl MarketView<'_> {
    pub fn at(&self) -> Timestamp {
        self.at
    }

    /// The mark for one instrument at one venue.
    pub fn mark(&self, object_id: &str, venue: &str) -> Option<&Mark> {
        self.marks.get(&mark_key(object_id, venue))
    }

    /// Every mark, keyed `instrument@venue`.
    pub fn marks(&self) -> &BTreeMap<String, Mark> {
        self.marks
    }

    /// The marks that are last-known values rather than current ones.
    pub fn stale(&self) -> Vec<&Mark> {
        self.marks.values().filter(|mark| mark.is_stale()).collect()
    }

    /// The marks taken while the touch was inverted.
    pub fn crossed(&self) -> Vec<&Mark> {
        self.marks
            .values()
            .filter(|mark| mark.is_crossed())
            .collect()
    }
}

/// What a strategy must provide to be run through conditions.
///
/// Separate from [`crate::backtest::BacktestStrategy`], which trades weights
/// over bars. This one places orders against a book, because the conditions
/// that matter here — depth, latency, an outage mid-order — are invisible to
/// anything that expresses itself as a target weight.
pub trait SimStrategy {
    fn name(&self) -> &str;

    /// Orders to submit at this instant.
    fn on_step(&mut self, view: &MarketView<'_>) -> Vec<SimOrder>;
}

/// What one run through the conditions produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulationRun {
    pub strategy: String,
    pub seed: u64,
    pub source: PriceSource,
    /// Fingerprint of the schedule that was injected.
    pub schedule_digest: String,
    /// Names of the conditions in the schedule, in order.
    pub conditions: Vec<String>,
    pub steps: usize,
    pub reports: Vec<ExecutionReport>,
    /// Net position per instrument, exact.
    pub positions: BTreeMap<String, Decimal>,
    /// Cash from trading, before marking the residual position.
    pub cash: Decimal,
    pub commission: Decimal,
    /// Last known price per instrument at the end of the run.
    pub final_marks: BTreeMap<String, Decimal>,
    /// Cash plus the position marked at the last known price.
    pub profit_and_loss: Decimal,
    /// Orders that met a venue that had stopped answering.
    pub unreachable_orders: usize,
    /// Steps at which some mark was a last-known value rather than a current
    /// one.
    pub stale_mark_steps: usize,
    /// Steps at which some book was crossed.
    pub crossed_market_steps: usize,
}

impl SimulationRun {
    /// Quantity filled across every order.
    pub fn filled_quantity(&self) -> Decimal {
        self.reports.iter().map(|report| report.filled).sum()
    }

    /// Quantity that was asked for and did not fill.
    pub fn residual_quantity(&self) -> Decimal {
        self.reports.iter().map(|report| report.residual).sum()
    }

    /// Mean adversity across the orders that were sent, in basis points.
    pub fn mean_adversity_bps(&self) -> f64 {
        if self.reports.is_empty() {
            return 0.0;
        }
        self.reports
            .iter()
            .map(ExecutionReport::adversity_bps)
            .sum::<f64>()
            / self.reports.len() as f64
    }

    /// A fingerprint over every outcome the run produced.
    ///
    /// Over the raw fixed-point integers and nanosecond counts, never over a
    /// formatted rendering, so the digest cannot change because a display
    /// convention did. This is what "same seed, same conditions, byte-identical
    /// outcome" is checked on.
    pub fn digest(&self) -> String {
        let mut bytes = Vec::with_capacity(256 + self.reports.len() * 96);
        bytes.extend_from_slice(self.strategy.as_bytes());
        bytes.extend_from_slice(&self.seed.to_le_bytes());
        bytes.extend_from_slice(self.source.as_str().as_bytes());
        bytes.extend_from_slice(self.schedule_digest.as_bytes());
        bytes.extend_from_slice(&(self.steps as u64).to_le_bytes());
        for report in &self.reports {
            bytes.extend_from_slice(report.object_id.as_bytes());
            bytes.extend_from_slice(report.venue.as_bytes());
            bytes.push(match report.side {
                Side::Buy => 1,
                Side::Sell => 2,
            });
            bytes.extend_from_slice(&(report.leg as u64).to_le_bytes());
            bytes.extend_from_slice(&report.submitted_at.as_nanos().to_le_bytes());
            bytes.extend_from_slice(&report.arrived_at.as_nanos().to_le_bytes());
            bytes.extend_from_slice(&report.requested.raw().to_le_bytes());
            bytes.extend_from_slice(&report.filled.raw().to_le_bytes());
            bytes.extend_from_slice(&report.residual.raw().to_le_bytes());
            bytes.extend_from_slice(&report.notional.raw().to_le_bytes());
            bytes.extend_from_slice(&report.commission.raw().to_le_bytes());
            bytes.extend_from_slice(report.status.as_str().as_bytes());
            bytes.extend_from_slice(report.book_condition.as_str().as_bytes());
            bytes.extend_from_slice(
                &report
                    .crossed_by
                    .unwrap_or(Decimal::ZERO)
                    .raw()
                    .to_le_bytes(),
            );
            bytes.extend_from_slice(&report.mark.as_of.as_nanos().to_le_bytes());
            bytes.extend_from_slice(&report.mark.observed_at.as_nanos().to_le_bytes());
        }
        for (object_id, quantity) in &self.positions {
            bytes.extend_from_slice(object_id.as_bytes());
            bytes.extend_from_slice(&quantity.raw().to_le_bytes());
        }
        for (object_id, price) in &self.final_marks {
            bytes.extend_from_slice(object_id.as_bytes());
            bytes.extend_from_slice(&price.raw().to_le_bytes());
        }
        bytes.extend_from_slice(&self.cash.raw().to_le_bytes());
        bytes.extend_from_slice(&self.commission.raw().to_le_bytes());
        bytes.extend_from_slice(&self.profit_and_loss.raw().to_le_bytes());
        qip_core::sha256_hex(&bytes)
    }

    pub fn summarise(&self) -> String {
        format!(
            "{} over {} step(s) of {} market [{}]: {} order(s), {} filled, {} residual, P&L {}, mean adversity {:.1}bp, {} unreachable, stale marks on {} step(s), crossed on {}",
            self.strategy,
            self.steps,
            self.source.as_str(),
            if self.conditions.is_empty() {
                "calm".to_string()
            } else {
                self.conditions.join(" + ")
            },
            self.reports.len(),
            self.filled_quantity(),
            self.residual_quantity(),
            self.profit_and_loss,
            self.mean_adversity_bps(),
            self.unreachable_orders,
            self.stale_mark_steps,
            self.crossed_market_steps
        )
    }
}

/// The simulator.
#[derive(Clone, Debug)]
pub struct MarketSimulator {
    seed: u64,
    source: PriceSource,
    venues: Vec<String>,
    instruments: BTreeMap<String, InstrumentSpec>,
    paths: BTreeMap<String, Vec<PathPoint>>,
    steps: Vec<Timestamp>,
    schedule: ConditionSchedule,
    costs: CostModel,
}

impl MarketSimulator {
    /// Generate a synthetic market from a seed.
    ///
    /// The whole path is generated here rather than lazily as the simulation
    /// walks, so that asking about an instant is a lookup. That is not an
    /// optimisation: a lazily advanced generator would make the price at an
    /// instant depend on how many other questions had been asked first, and
    /// the determinism guarantee would hold only for callers that asked in
    /// order.
    pub fn synthetic(market: SyntheticMarket, seed: u64) -> Result<Self> {
        market.validate()?;
        let mut instruments = BTreeMap::new();
        for spec in market.instruments {
            instruments.insert(spec.object_id.clone(), spec);
        }
        let steps: Vec<Timestamp> = (0..market.steps)
            .map(|index| market.start.saturating_add(market.step * index as i64))
            .collect();

        let mut paths = BTreeMap::new();
        for (object_id, spec) in &instruments {
            // Forked per instrument off the run seed, in map order, so adding
            // an instrument does not silently redraw the others' paths.
            let mut stream = Xoshiro256::seeded(seed).fork(object_id);
            let mut price = spec.initial_price.to_f64();
            let mut points = Vec::with_capacity(steps.len());
            for at in &steps {
                let decimal = Decimal::from_f64(price).unwrap_or(spec.initial_price);
                points.push(PathPoint {
                    at: *at,
                    price: if decimal.is_positive() {
                        decimal
                    } else {
                        spec.initial_price
                    },
                    volume: spec.daily_volume,
                });
                let shock = spec.step_drift + spec.step_volatility * stream.normal();
                price *= shock.exp();
                if !price.is_finite() || price <= 0.0 {
                    price = spec.initial_price.to_f64();
                }
            }
            paths.insert(object_id.clone(), points);
        }

        Ok(Self {
            seed,
            source: PriceSource::Synthetic,
            venues: market.venues,
            instruments,
            paths,
            steps,
            schedule: ConditionSchedule::new(),
            costs: CostModel::default(),
        })
    }

    /// Replay recorded bars.
    ///
    /// The path is the bars' closes at their close times — the same keying the
    /// point-in-time clock uses, so a bar stamped with a day does not exist
    /// until that session ends. Volume comes off the bars rather than off the
    /// spec, because the whole reason to replay real history is that its
    /// liquidity is real.
    pub fn replay(
        bars: Vec<Bar>,
        instruments: Vec<InstrumentSpec>,
        venues: Vec<String>,
        seed: u64,
    ) -> Result<Self> {
        if bars.is_empty() {
            return Err(Error::invalid("a replay needs at least one bar"));
        }
        if venues.is_empty() {
            return Err(Error::invalid("a replay needs at least one venue"));
        }
        let mut specs = BTreeMap::new();
        for spec in instruments {
            spec.validate()?;
            specs.insert(spec.object_id.clone(), spec);
        }
        if specs.is_empty() {
            return Err(Error::invalid(
                "a replay needs a book shape for at least one instrument",
            ));
        }

        let mut paths: BTreeMap<String, Vec<PathPoint>> = BTreeMap::new();
        for bar in bars {
            if !bar.is_coherent() {
                return Err(Error::invalid(format!(
                    "incoherent bar for {} at {}",
                    bar.object_id.as_str(),
                    bar.open_time
                )));
            }
            let key = bar.object_id.as_str().to_string();
            if !specs.contains_key(&key) {
                // Refused rather than defaulted: a book shape guessed for an
                // instrument is a fill price guessed for it.
                return Err(Error::invalid(format!(
                    "replay has bars for {key} but no book shape for it"
                )));
            }
            paths.entry(key).or_default().push(PathPoint {
                at: bar.close_time(),
                price: bar.close,
                volume: bar.volume.to_f64().max(1.0),
            });
        }

        let mut steps: Vec<Timestamp> = Vec::new();
        for points in paths.values_mut() {
            points.sort_by_key(|point| point.at);
            steps.extend(points.iter().map(|point| point.at));
        }
        steps.sort_unstable();
        steps.dedup();

        Ok(Self {
            seed,
            source: PriceSource::Historical,
            venues,
            instruments: specs,
            paths,
            steps,
            schedule: ConditionSchedule::new(),
            costs: CostModel::default(),
        })
    }

    /// Inject a schedule of conditions.
    pub fn with_conditions(mut self, schedule: ConditionSchedule) -> Result<Self> {
        schedule.validate()?;
        self.schedule = schedule;
        Ok(self)
    }

    /// Use a different cost model for the impact term and commissions.
    pub fn with_costs(mut self, costs: CostModel) -> Result<Self> {
        costs.validate()?;
        self.costs = costs;
        Ok(self)
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn source(&self) -> PriceSource {
        self.source
    }

    pub fn steps(&self) -> &[Timestamp] {
        &self.steps
    }

    pub fn venues(&self) -> &[String] {
        &self.venues
    }

    pub fn schedule(&self) -> &ConditionSchedule {
        &self.schedule
    }

    pub fn instrument(&self, object_id: &str) -> Option<&InstrumentSpec> {
        self.instruments.get(object_id)
    }

    /// The conditions collapsed at one instant and scope.
    pub fn regime_at(&self, at: Timestamp, venue: &str, object_id: &str, leg: usize) -> Regime {
        self.schedule.regime(at, venue, object_id, leg, self.seed)
    }

    /// The undisturbed price: the path before any condition touches it.
    ///
    /// The most recent point at or before `at`, so a request between steps
    /// reads the last thing that actually printed rather than interpolating a
    /// price that never existed.
    pub fn reference_price(&self, object_id: &str, at: Timestamp) -> Option<Decimal> {
        self.path_point(object_id, at).map(|point| point.price)
    }

    /// The book as it stands, with every active condition applied.
    pub fn book_at(
        &self,
        object_id: &str,
        venue: &str,
        at: Timestamp,
        leg: usize,
    ) -> Result<SimBook> {
        let regime = self.regime_at(at, venue, object_id, leg);
        self.build_book(object_id, venue, at, &regime)
    }

    /// The mark a strategy would read, with its true age.
    ///
    /// A delayed feed is applied *here* and not to [`Self::book_at`], which is
    /// the whole point of the distinction: your feed being late does not make
    /// the market late. You trade against the market as it is and price
    /// against the market as you last saw it, and the gap between the two is
    /// the loss a delayed feed causes.
    pub fn mark_at(&self, object_id: &str, venue: &str, at: Timestamp, leg: usize) -> Mark {
        let regime = self.regime_at(at, venue, object_id, leg);
        let observed_at = at.saturating_sub(regime.feed_delay);
        let mut mark = Mark::unavailable(object_id, venue, at);
        mark.as_of = observed_at;
        mark.faults = regime.feed_faults.clone();

        if regime.feed_faults.contains(&FeedFault::Malformed) {
            // Nothing can be recovered from a message that will not decode, so
            // the instant has no observation rather than a wrong one.
            return mark;
        }

        let observed_regime = self.regime_at(observed_at, venue, object_id, leg);
        let Ok(book) = self.build_book(object_id, venue, observed_at, &observed_regime) else {
            return mark;
        };
        mark.condition = book.condition();
        mark.crossed_by = book.crossed_by();
        match book.mid() {
            Some(mid) => {
                mark.price = Some(mid);
                mark.source = MarkSource::Book;
            }
            None => {
                // A crossed or one-sided book still has a touch. It is reported
                // as coming from one side so a reader cannot mistake it for a
                // mid, and the crossed condition travels with it.
                mark.price = book
                    .best_bid()
                    .map(|level| level.price)
                    .or_else(|| book.best_ask().map(|level| level.price));
                if mark.price.is_some() {
                    mark.source = MarkSource::OneSidedBook;
                }
            }
        }
        mark
    }

    /// Execute one order.
    pub fn execute(&self, order: &SimOrder, submitted_at: Timestamp) -> Result<ExecutionReport> {
        order.validate()?;
        let spec = self
            .instruments
            .get(&order.object_id)
            .ok_or_else(|| {
                Error::not_found(format!(
                    "no book shape for {}; the simulator will not guess one",
                    order.object_id
                ))
            })?
            .clone();
        if !self.venues.contains(&order.venue) {
            return Err(Error::not_found(format!(
                "{} is not a venue in this simulation",
                order.venue
            )));
        }

        let submit_regime = self.regime_at(submitted_at, &order.venue, &order.object_id, order.leg);
        let latency = submit_regime.order_latency;
        let arrived_at = submitted_at.saturating_add(latency);
        let arrival_regime = self.regime_at(arrived_at, &order.venue, &order.object_id, order.leg);
        let mark = self.mark_at(&order.object_id, &order.venue, arrived_at, order.leg);

        let arrival_book =
            self.build_book(&order.object_id, &order.venue, arrived_at, &arrival_regime)?;
        let taking = order.side.opposite();
        let mut report = ExecutionReport {
            object_id: order.object_id.clone(),
            venue: order.venue.clone(),
            side: order.side,
            leg: order.leg,
            requested: order.quantity,
            submitted_at,
            arrived_at,
            latency,
            filled: Decimal::ZERO,
            residual: order.quantity,
            notional: Decimal::ZERO,
            commission: Decimal::ZERO,
            status: FillStatus::NoLiquidity,
            reference: arrival_book.touch_midpoint(),
            slices: Vec::new(),
            mark,
            book_condition: arrival_book.condition(),
            crossed_by: arrival_book.crossed_by(),
            depth_available: arrival_book.depth(taking),
            conditions: arrival_regime.applied.clone(),
        };

        if arrival_regime.feed_faults.contains(&FeedFault::Malformed) {
            // Not sending is the conservative reading, and the conservative
            // reading is the one a simulator owes its caller: an order priced
            // off a message nobody could decode is a guess wearing a fill.
            report.status = FillStatus::FeedUnusable;
            return Ok(report);
        }
        if arrival_regime.venue_down {
            report.status = FillStatus::VenueUnreachable;
            report.slices.push(FillSlice {
                at: arrived_at,
                filled: Decimal::ZERO,
                notional: Decimal::ZERO,
                worst_price: None,
                levels_consumed: 0,
                depth_available: report.depth_available,
                venue_responding: false,
            });
            return Ok(report);
        }

        let slices = order.slices.max(1);
        let mut remaining = order.quantity;
        let mut hit_outage = false;
        let mut not_marketable = false;

        for index in 0..slices {
            if !remaining.is_positive() {
                break;
            }
            let slice_at = arrived_at.saturating_add(order.slice_interval * index as i64);
            let regime = self.regime_at(slice_at, &order.venue, &order.object_id, order.leg);
            if regime.venue_down {
                // The venue stopped answering part way through. Everything
                // after this is not filled and not sent; the residual is what
                // the caller still owns, and it is exact.
                hit_outage = true;
                report.slices.push(FillSlice {
                    at: slice_at,
                    filled: Decimal::ZERO,
                    notional: Decimal::ZERO,
                    worst_price: None,
                    levels_consumed: 0,
                    depth_available: Decimal::ZERO,
                    venue_responding: false,
                });
                break;
            }

            let mut book = self.build_book(&order.object_id, &order.venue, slice_at, &regime)?;
            let available = book.depth(taking);
            let target =
                self.slice_target(remaining, slices - index, &regime, available, &book, order);
            if !target.is_positive() {
                if available.is_positive() && order.limit_price.is_some() {
                    not_marketable = true;
                }
                report.slices.push(FillSlice {
                    at: slice_at,
                    filled: Decimal::ZERO,
                    notional: Decimal::ZERO,
                    worst_price: None,
                    levels_consumed: 0,
                    depth_available: available,
                    venue_responding: true,
                });
                continue;
            }

            let outcome = book.take(order.side, target, slice_at);
            if !outcome.filled.is_positive() {
                report.slices.push(FillSlice {
                    at: slice_at,
                    filled: Decimal::ZERO,
                    notional: Decimal::ZERO,
                    worst_price: None,
                    levels_consumed: 0,
                    depth_available: available,
                    venue_responding: true,
                });
                continue;
            }

            let priced = self.price_of(&spec, &book, order.side, &outcome, &regime);
            let notional = priced
                .checked_mul(outcome.filled)
                .unwrap_or(outcome.notional);
            remaining -= outcome.filled;
            report.filled += outcome.filled;
            report.notional += notional;
            report.slices.push(FillSlice {
                at: slice_at,
                filled: outcome.filled,
                notional,
                worst_price: outcome.worst_price,
                levels_consumed: outcome.levels_consumed,
                depth_available: available,
                venue_responding: true,
            });
        }

        report.residual = (order.quantity - report.filled).max(Decimal::ZERO);
        report.status = if hit_outage {
            FillStatus::VenueUnreachable
        } else if !report.filled.is_positive() {
            if not_marketable {
                FillStatus::NotMarketable
            } else {
                FillStatus::NoLiquidity
            }
        } else if report.residual.is_positive() {
            FillStatus::Partial
        } else {
            FillStatus::Complete
        };
        report.commission = self.commission_on(report.notional);
        Ok(report)
    }

    /// Execute a multi-leg plan, leg by leg.
    ///
    /// Legs are executed in order and every leg is reported, including the ones
    /// after a leg that failed. A plan that stopped at the first failure would
    /// hide the fact that the later legs *would* have filled, which is the
    /// information a caller needs to decide whether it is now holding one side
    /// of a spread.
    pub fn execute_plan(
        &self,
        plan: &ExecutionPlan,
        submitted_at: Timestamp,
    ) -> Result<PlanReport> {
        plan.validate()?;
        let mut legs = Vec::with_capacity(plan.legs().len());
        for order in plan.legs() {
            legs.push(self.execute(order, submitted_at)?);
        }
        Ok(PlanReport { legs })
    }

    /// Walk every step, asking the strategy for orders and executing them.
    pub fn run<S: SimStrategy>(&self, strategy: &mut S) -> Result<SimulationRun> {
        let mut reports: Vec<ExecutionReport> = Vec::new();
        let mut positions: BTreeMap<String, Decimal> = BTreeMap::new();
        let mut cash = Decimal::ZERO;
        let mut commission = Decimal::ZERO;
        let mut unreachable_orders = 0usize;
        let mut stale_mark_steps = 0usize;
        let mut crossed_market_steps = 0usize;

        for at in &self.steps {
            let marks = self.marks_at(*at);
            if marks.values().any(Mark::is_stale) {
                stale_mark_steps += 1;
            }
            if marks.values().any(Mark::is_crossed) {
                crossed_market_steps += 1;
            }
            let orders = {
                let view = MarketView {
                    at: *at,
                    marks: &marks,
                };
                strategy.on_step(&view)
            };
            for order in orders {
                let report = self.execute(&order, *at)?;
                if report.status == FillStatus::VenueUnreachable {
                    unreachable_orders += 1;
                }
                if report.filled.is_positive() {
                    let signed = match report.side {
                        Side::Buy => report.filled,
                        Side::Sell => -report.filled,
                    };
                    *positions.entry(report.object_id.clone()).or_default() += signed;
                    cash -= match report.side {
                        Side::Buy => report.notional,
                        Side::Sell => -report.notional,
                    };
                    cash -= report.commission;
                    commission += report.commission;
                }
                reports.push(report);
            }
        }

        let mut final_marks = BTreeMap::new();
        let mut profit_and_loss = cash;
        if let Some(last) = self.steps.last() {
            for object_id in self.instruments.keys() {
                let Some(price) = self.reference_price(object_id, *last) else {
                    continue;
                };
                final_marks.insert(object_id.clone(), price);
                if let Some(quantity) = positions.get(object_id) {
                    profit_and_loss += price.checked_mul(*quantity).unwrap_or(Decimal::ZERO);
                }
            }
        }

        Ok(SimulationRun {
            strategy: strategy.name().to_string(),
            seed: self.seed,
            source: self.source,
            schedule_digest: self.schedule.digest(),
            conditions: self
                .schedule
                .windows()
                .iter()
                .map(|window| window.condition.as_str().to_string())
                .collect(),
            steps: self.steps.len(),
            reports,
            positions,
            cash,
            commission,
            final_marks,
            profit_and_loss,
            unreachable_orders,
            stale_mark_steps,
            crossed_market_steps,
        })
    }

    /// Marks for every instrument at every venue at one instant.
    pub fn marks_at(&self, at: Timestamp) -> BTreeMap<String, Mark> {
        let mut marks = BTreeMap::new();
        for object_id in self.instruments.keys() {
            for venue in &self.venues {
                marks.insert(
                    mark_key(object_id, venue),
                    self.mark_at(object_id, venue, at, 0),
                );
            }
        }
        marks
    }

    /// The most recent path point at or before `at`.
    fn path_point(&self, object_id: &str, at: Timestamp) -> Option<PathPoint> {
        let points = self.paths.get(object_id)?;
        let cut = points.partition_point(|point| point.at <= at);
        if cut == 0 {
            // Before the first point there is no observation. Returning the
            // first one would be a day of hindsight dressed as a default.
            return None;
        }
        points.get(cut - 1).copied()
    }

    /// Build the book at an instant under a regime.
    fn build_book(
        &self,
        object_id: &str,
        venue: &str,
        at: Timestamp,
        regime: &Regime,
    ) -> Result<SimBook> {
        let spec = self
            .instruments
            .get(object_id)
            .ok_or_else(|| Error::not_found(format!("no book shape for {object_id}")))?;
        let mut book = SimBook::new(ObjectId::from_string(object_id.to_string()), venue, at);
        let Some(point) = self.path_point(object_id, at) else {
            return Ok(book);
        };
        let displaced = point
            .price
            .checked_mul(Decimal::from_f64(regime.price_multiplier).unwrap_or(Decimal::ONE))
            .unwrap_or(point.price);
        if !displaced.is_positive() {
            return Ok(book);
        }

        let half_spread = displaced.apply_bps(spec.half_spread_bps * regime.spread_multiplier);
        // A crossed market is built symmetrically about the true mid: the bid
        // rises by half the cross and the ask falls by half. The touch inverts
        // and the *midpoint of the touch does not move*, which is what lets an
        // execution measured against it show the cross as a cost rather than
        // as a shifted yardstick.
        let cross_half = if regime.crossed_by_bps > 0.0 {
            displaced.apply_bps(regime.crossed_by_bps / 2.0) + half_spread
        } else {
            Decimal::ZERO
        };
        let best_bid = displaced - half_spread + cross_half;
        let best_ask = displaced + half_spread - cross_half;

        let size = displayed_size(spec.level_size, regime.depth_fraction);
        if !size.is_positive() {
            return Ok(book);
        }
        // Two resting orders per level, so time priority is a fact about the
        // book rather than a property of a queue of one.
        let per_order = size
            .checked_div(Decimal::from_int(2))
            .unwrap_or(size)
            .max(Decimal::from_raw(1));

        let mut entered = at.saturating_sub(Duration::from_nanos((spec.levels as i64) * 4));
        for index in 0..spec.levels {
            let step = displaced.apply_bps(spec.level_spacing_bps * index as f64);
            for (side, price) in [(Side::Buy, best_bid - step), (Side::Sell, best_ask + step)] {
                if !price.is_positive() {
                    continue;
                }
                for _ in 0..2 {
                    book.rest(side, price, per_order, entered)?;
                    entered = entered.saturating_add(Duration::from_nanos(1));
                }
            }
        }
        Ok(book)
    }

    /// How much of the remaining quantity this slice will try for.
    ///
    /// Capped three ways, all of them downward: the share of the order this
    /// slice is responsible for, the depth the book is actually showing, and
    /// the regime's fill ceiling. Nothing here can raise the number.
    #[allow(clippy::too_many_arguments)]
    fn slice_target(
        &self,
        remaining: Decimal,
        slices_left: usize,
        regime: &Regime,
        available: Decimal,
        book: &SimBook,
        order: &SimOrder,
    ) -> Decimal {
        let share = if slices_left <= 1 {
            remaining
        } else {
            remaining
                .checked_div(Decimal::from_int(slices_left as i64))
                .unwrap_or(remaining)
        };
        let mut target = share.min(remaining);
        // The venue's own ceiling comes off what was asked for, not off the
        // depth: the queue an aggregated book does not show sits in front of
        // this order regardless of how deep the book behind it is.
        target = displayed_size(target, regime.fill_fraction_cap);
        let reachable = match order.limit_price {
            Some(limit) => depth_within_limit(book, order.side, limit),
            None => available,
        };
        target.min(reachable).max(Decimal::ZERO)
    }

    /// The price a slice actually prints at.
    ///
    /// Three things happen here and every one of them can only make the price
    /// worse for the taker:
    ///
    /// 1. The book is swept, so the price is the volume-weighted walk rather
    ///    than the touch.
    /// 2. The result is floored at the *worse* of the two touch prices, which
    ///    is a no-op on a normal book and is what stops a crossed market being
    ///    read as free money.
    /// 3. Everything beyond the reference — the walk plus a square-root impact
    ///    term for the size the book does not show — is scaled by the regime's
    ///    slippage multiplier.
    fn price_of(
        &self,
        spec: &InstrumentSpec,
        book: &SimBook,
        side: Side,
        outcome: &crate::venue::SweepOutcome,
        regime: &Regime,
    ) -> Decimal {
        let Some(swept) = outcome.average_price() else {
            return Decimal::ZERO;
        };
        let touch = book.taker_touch(side).unwrap_or(swept);
        let walked = match side {
            Side::Buy => swept.max(touch),
            Side::Sell => swept.min(touch),
        };
        let Some(reference) = book.touch_midpoint() else {
            return walked;
        };
        if !reference.is_positive() {
            return walked;
        }

        let walk_bps = match side {
            Side::Buy => (walked - reference).to_f64(),
            Side::Sell => (reference - walked).to_f64(),
        } / reference.to_f64()
            * 10_000.0;
        let volatility = spec.step_volatility * regime.volatility_multiplier;
        let participation = (outcome.filled.to_f64() / spec.daily_volume).max(0.0);
        let impact_bps = if volatility > 0.0 && participation > 0.0 {
            self.costs.impact_coefficient * volatility * participation.sqrt() * 10_000.0
        } else {
            0.0
        };
        let total_bps = (walk_bps.max(0.0) + impact_bps) * regime.slippage_multiplier;
        let adjustment = reference.apply_bps(total_bps);
        match side {
            Side::Buy => reference + adjustment,
            Side::Sell => (reference - adjustment).max(Decimal::from_raw(1)),
        }
    }

    /// Commission on a filled notional, from the platform's own cost model.
    fn commission_on(&self, notional: Decimal) -> Decimal {
        if !notional.is_positive() {
            return Decimal::ZERO;
        }
        let charged =
            (notional.to_f64() * self.costs.commission_rate).max(self.costs.minimum_commission);
        Decimal::from_f64(charged).unwrap_or(Decimal::ZERO)
    }
}

/// Size left showing after a depth collapse, rounded *down*.
///
/// Down rather than to nearest: rounding a collapsing book up would hand back
/// liquidity the condition was supposed to remove, and the whole point of the
/// condition is that the size is not there.
fn displayed_size(base: Decimal, fraction: f64) -> Decimal {
    if fraction >= 1.0 {
        return base;
    }
    if fraction <= 0.0 {
        return Decimal::ZERO;
    }
    let scaled = base.to_f64() * fraction;
    Decimal::from_f64(scaled)
        .unwrap_or(Decimal::ZERO)
        .min(base)
        .max(Decimal::ZERO)
}

/// Depth reachable without trading through a limit price.
///
/// The sweep walks best price first, so the acceptable levels are exactly the
/// prefix of the walk and capping the quantity is enough to enforce the limit.
fn depth_within_limit(book: &SimBook, side: Side, limit: Decimal) -> Decimal {
    book.levels(side.opposite())
        .into_iter()
        .filter(|level| match side {
            Side::Buy => level.price <= limit,
            Side::Sell => level.price >= limit,
        })
        .map(|level| level.size)
        .sum()
}

/// The key a mark is filed under in a [`MarketView`].
pub fn mark_key(object_id: &str, venue: &str) -> String {
    format!("{object_id}@{venue}")
}
