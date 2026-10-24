//! Microstructure labels, models and benchmarks from replayed tick history
//! (R12-TICK-2065, 2068, 2069; consumes R12-TICK-2061's flags).
//!
//! [`crate::labels::LabelBuilder`] computed every outcome label by hand and
//! nothing called it; no fill, slippage or volatility model was ever fitted
//! on microstructure; and nothing benchmarked a model by venue, asset, regime
//! and horizon. This module is the job those three needed: it takes the
//! book observations a tick replay produced, builds labelled examples through
//! the label builder, fits a fill-probability model and a short-horizon
//! volatility model against their naive baselines on a held-out slice in
//! time, and reports the fill model one row per venue × asset × regime ×
//! horizon.
//!
//! # No point-in-time leakage
//!
//! An example at instant `t` is built from the observations of its own
//! instrument that were **knowable at or before `t`** — the features — and
//! labelled from what happened in `(t, t + h]`. The regime is classified from
//! the trailing window, never from a statistic of the whole history (a
//! threshold taken from the full sample's median reads the future). The
//! holdout is the last slice *in time*, never a random subset.
//!
//! # Unreliable history is not ground truth
//!
//! An observation the replay flagged unreliable — inside a gap the venue's
//! sequence proves, until the next reset — is never a feature and never a
//! label. An example whose trailing window or label window touches one is
//! refused and counted in [`LabelledSet::refused_unreliable`]: a model
//! trained on a fabricated book learns the fabrication.
//!
//! Every number here is a statistic and is `f64`. Prices cross from
//! `Decimal` at the caller, when it builds a [`BookObservation`].

use crate::clock_precision::ClockPrecision;
use crate::labels::{LabelBuilder, MidSeries, Print, Side};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Feature names, in the order every model's weights are kept.
pub const FEATURES: [&str; 3] = ["spread_bps", "touch_imbalance", "trailing_vol_bps"];

/// The book of one instrument as a replay held it at one knowable instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookObservation {
    /// When this state became knowable to the platform.
    pub at: Timestamp,
    pub venue: String,
    /// The venue's own sequence number for the message that left the book in
    /// this state (R12-TICK-1049). Carried to the example so a training row
    /// can be traced to the message it was read at, and a replay compared
    /// against it.
    pub sequence: u64,
    /// The entitlement the message was obtained under (R12-TICK-1051). A
    /// row with no entitlement is a row nobody can say the platform may use,
    /// so [`build_examples`] refuses one rather than labelling it.
    pub entitlement: String,
    pub instrument: String,
    pub best_bid: f64,
    pub best_ask: f64,
    pub bid_size: f64,
    pub ask_size: f64,
    /// Inside a span the replay could not vouch for.
    pub unreliable: bool,
}

impl BookObservation {
    fn mid(&self) -> f64 {
        (self.best_bid + self.best_ask) / 2.0
    }

    fn is_usable(&self) -> bool {
        [self.best_bid, self.best_ask, self.bid_size, self.ask_size]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
            && self.best_bid > 0.0
            && self.best_ask > self.best_bid
    }
}

/// A trade print on one instrument, at the instant it became knowable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TradeObservation {
    pub at: Timestamp,
    pub instrument: String,
    pub price: f64,
}

/// How examples are drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabelPlan {
    /// The horizons every example is labelled at.
    pub horizons: Vec<Duration>,
    /// Take every n-th usable observation as a decision instant.
    pub sample_every: usize,
    /// Trailing observations a regime and trailing volatility are read from.
    pub trailing: usize,
    /// Trailing volatility at or above which the regime is `volatile`. A
    /// fixed setting and not a quantile of the data: a quantile of the whole
    /// history is a statistic of the future.
    pub volatile_above_bps: f64,
}

impl LabelPlan {
    /// The clock precision this plan's labels need, from its shortest
    /// horizon (R12-RES-7103): a window of recorded events whose clock
    /// uncertainty exceeds it is refused before it is labelled, through
    /// [`ClockPrecision::admit`]. Validates the plan first, so a plan with
    /// no positive horizon is refused for that and not for a precision.
    pub fn clock_precision(&self) -> Result<ClockPrecision> {
        self.validate()?;
        let shortest =
            self.horizons.iter().copied().min().ok_or_else(|| {
                Error::invalid("a label plan needs at least one positive horizon")
            })?;
        ClockPrecision::for_label_horizon(shortest)
    }

    fn validate(&self) -> Result<()> {
        if self.horizons.is_empty() || self.horizons.iter().any(|h| h.as_nanos() <= 0) {
            return Err(Error::invalid(
                "a label plan needs at least one positive horizon",
            ));
        }
        if self.sample_every == 0 || self.trailing < 2 {
            return Err(Error::invalid(
                "a label plan samples every n >= 1 observations over a trailing window of at \
                 least 2",
            ));
        }
        if !self.volatile_above_bps.is_finite() || self.volatile_above_bps <= 0.0 {
            return Err(Error::invalid(
                "the volatile-regime threshold must be a positive number of basis points",
            ));
        }
        Ok(())
    }
}

/// One labelled decision instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MicroExample {
    /// Venue identity (R12-TICK-1050), sequence (1049) and entitlement
    /// (1051) of the observation the example was drawn at: the lineage
    /// without which a row cannot be replayed, licensed or trusted.
    pub venue: String,
    pub sequence: u64,
    pub entitlement: String,
    pub instrument: String,
    pub at: Timestamp,
    pub horizon: Duration,
    /// `calm` or `volatile`, from the trailing window.
    pub regime: String,
    /// In [`FEATURES`] order.
    pub features: Vec<f64>,
    /// Whether a bid resting at the touch at `at` was traded through in
    /// `(at, at + horizon]` — the outcome a fill-probability model learns.
    pub fill: f64,
    pub future_return: f64,
    pub realized_volatility: f64,
    /// Against a fill at the touch bid at `at`.
    pub adverse_selection: f64,
    /// Of a buy decided at `at`, from the mid.
    pub market_impact: f64,
}

/// What [`build_examples`] produced.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LabelledSet {
    pub examples: Vec<MicroExample>,
    /// Decision instants refused because their feature or label window
    /// touched a span the replay flagged unreliable.
    pub refused_unreliable: usize,
    /// Decision instants refused because history did not cover the horizon.
    pub refused_uncovered: usize,
}

fn trailing_vol_bps(window: &[&BookObservation]) -> f64 {
    let mut sum = 0.0;
    for pair in window.windows(2) {
        let r = (pair[1].mid() / pair[0].mid()).ln();
        sum += r * r;
    }
    sum.sqrt() * 10_000.0
}

/// Build labelled examples per instrument. See the module doc for what is
/// refused and why.
pub fn build_examples(
    observations: &[BookObservation],
    trades: &[TradeObservation],
    plan: &LabelPlan,
) -> Result<LabelledSet> {
    plan.validate()?;
    if let Some(blank) = observations
        .iter()
        .find(|o| o.entitlement.trim().is_empty() || o.venue.trim().is_empty())
    {
        return Err(Error::invalid(format!(
            "an observation of {} at {} carries no venue or no entitlement; a training row \
             without that lineage cannot be licensed or replayed, so name both upstream",
            blank.instrument, blank.at
        )));
    }
    let mut by_instrument: BTreeMap<&str, Vec<&BookObservation>> = BTreeMap::new();
    for observation in observations {
        by_instrument
            .entry(observation.instrument.as_str())
            .or_default()
            .push(observation);
    }
    let mut out = LabelledSet::default();
    for (instrument, mut series) in by_instrument {
        series.sort_by_key(|o| o.at);
        // One mid per instant: the last state knowable at it.
        let mut mids: Vec<(Timestamp, f64)> = Vec::new();
        for observation in series.iter().filter(|o| o.is_usable() && !o.unreliable) {
            match mids.last_mut() {
                Some(last) if last.0 == observation.at => last.1 = observation.mid(),
                _ => mids.push((observation.at, observation.mid())),
            }
        }
        if mids.len() < 2 {
            continue;
        }
        let mid_series = MidSeries::new(mids)?;
        let builder = LabelBuilder::new(&mid_series);
        let prints: Vec<Print> = trades
            .iter()
            .filter(|t| t.instrument == instrument && t.price.is_finite() && t.price > 0.0)
            .map(|t| Print {
                at: t.at,
                price: t.price,
            })
            .collect();
        let flagged: Vec<Timestamp> = series
            .iter()
            .filter(|o| o.unreliable)
            .map(|o| o.at)
            .collect();
        for (index, observation) in series.iter().enumerate() {
            if index % plan.sample_every != 0 || index + 1 < plan.trailing {
                continue;
            }
            if !observation.is_usable() {
                continue;
            }
            let window: Vec<&BookObservation> = series[index + 1 - plan.trailing..=index].to_vec();
            let at = observation.at;
            for horizon in &plan.horizons {
                let end = at.saturating_add(*horizon);
                let window_start = window[0].at;
                if observation.unreliable
                    || flagged.iter().any(|f| *f >= window_start && *f <= end)
                    || window.iter().any(|o| !o.is_usable())
                {
                    out.refused_unreliable += 1;
                    continue;
                }
                let labels = (|| -> Result<(f64, f64, f64, f64, f64)> {
                    Ok((
                        builder.fill_outcome(
                            Side::Buy,
                            observation.best_bid,
                            at,
                            *horizon,
                            &prints,
                        )?,
                        builder.future_return(at, *horizon)?,
                        builder.realized_volatility(at, *horizon)?,
                        builder.adverse_selection(Side::Buy, observation.best_bid, at, *horizon)?,
                        builder.market_impact(Side::Buy, at, *horizon)?,
                    ))
                })();
                let (fill, future_return, realized_volatility, adverse_selection, market_impact) =
                    match labels {
                        Ok(labels) => labels,
                        Err(error) if error.code() == "unavailable" => {
                            out.refused_uncovered += 1;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                let spread_bps =
                    (observation.best_ask - observation.best_bid) / observation.mid() * 10_000.0;
                let depth = observation.bid_size + observation.ask_size;
                let imbalance = if depth > 0.0 {
                    observation.bid_size / depth
                } else {
                    0.5
                };
                let vol = trailing_vol_bps(&window);
                out.examples.push(MicroExample {
                    venue: observation.venue.clone(),
                    sequence: observation.sequence,
                    entitlement: observation.entitlement.clone(),
                    instrument: instrument.to_string(),
                    at,
                    horizon: *horizon,
                    regime: if vol >= plan.volatile_above_bps {
                        "volatile".to_string()
                    } else {
                        "calm".to_string()
                    },
                    features: vec![spread_bps, imbalance, vol],
                    fill,
                    future_return,
                    realized_volatility,
                    adverse_selection,
                    market_impact,
                });
            }
        }
    }
    out.examples.sort_by(|a, b| {
        a.at.cmp(&b.at)
            .then_with(|| a.instrument.cmp(&b.instrument))
            .then_with(|| a.horizon.cmp(&b.horizon))
    });
    Ok(out)
}

/// A venue fill whose simulated entry the twin got wrong, as the training
/// side needs to know it: where, in what, when, and by how much.
///
/// The learning round already scores every fill against the twin
/// (`qip_venue_fill_error_bps`) and charts it. A chart is not a training
/// set: until [`join_fill_residuals`] the error entered no row a model is
/// fitted or evaluated on, so the microstructure models were judged on
/// replayed books alone and never on the fills the platform actually got.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillResidualObservation {
    pub venue: String,
    pub instrument: String,
    /// When the venue reported the fill.
    pub filled_at: Timestamp,
    /// Signed basis points, as `FillScore::trade_error_bps`: negative where
    /// the twin filled better than the venue did.
    pub error_bps: f64,
}

/// A labelled decision instant tied to the real fill that followed it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResidualExample {
    /// The example drawn at the last instant at or before the fill, on the
    /// same venue and instrument: never one drawn after, which would hand
    /// the row the fill's own aftermath as a feature.
    pub example: MicroExample,
    pub filled_at: Timestamp,
    pub error_bps: f64,
}

/// What [`join_fill_residuals`] produced.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResidualSet {
    pub rows: Vec<ResidualExample>,
    /// Residuals with no example at or before their fill on that venue and
    /// instrument — a fill on a venue the replayed history does not cover.
    /// Counted, never matched to a neighbour: a row joined across venues
    /// would attribute one venue's queue to another's.
    pub unmatched: usize,
}

/// Join each residual to the example it follows, as of the fill.
///
/// Refuses a residual whose error is not a finite number rather than
/// dropping it: a fill that cannot be scored is a measurement problem the
/// caller should see, not a row that is quietly absent.
pub fn join_fill_residuals(
    examples: &[MicroExample],
    residuals: &[FillResidualObservation],
) -> Result<ResidualSet> {
    if let Some(bad) = residuals.iter().find(|r| !r.error_bps.is_finite()) {
        return Err(Error::invalid(format!(
            "the fill residual on {} at {} is not a finite number; refuse it upstream",
            bad.venue, bad.filled_at
        )));
    }
    let mut out = ResidualSet::default();
    for residual in residuals {
        // The latest instant at or before the fill; at that instant the
        // shortest horizon, so one residual is one row.
        let matched = examples
            .iter()
            .filter(|e| {
                e.venue == residual.venue
                    && e.instrument == residual.instrument
                    && e.at <= residual.filled_at
            })
            .max_by(|a, b| a.at.cmp(&b.at).then_with(|| b.horizon.cmp(&a.horizon)));
        match matched {
            Some(example) => out.rows.push(ResidualExample {
                example: example.clone(),
                filled_at: residual.filled_at,
                error_bps: residual.error_bps,
            }),
            None => out.unmatched += 1,
        }
    }
    Ok(out)
}

/// The last `holdout_fraction` of examples in time, and the rest. Examples
/// sharing an instant fall on one side together.
pub fn split_in_time(
    examples: &[MicroExample],
    holdout_fraction: f64,
) -> Result<(Vec<MicroExample>, Vec<MicroExample>)> {
    if !(holdout_fraction > 0.0 && holdout_fraction < 1.0) {
        return Err(Error::invalid(
            "the holdout fraction must lie strictly between 0 and 1",
        ));
    }
    let mut sorted = examples.to_vec();
    sorted.sort_by_key(|e| e.at);
    // Statistics: a count crosses to f64 for a fraction only.
    let mut cut = ((sorted.len() as f64) * (1.0 - holdout_fraction)).floor() as usize;
    while cut > 0 && cut < sorted.len() && sorted[cut].at == sorted[cut - 1].at {
        cut += 1;
    }
    let holdout = sorted.split_off(cut.min(sorted.len()));
    Ok((sorted, holdout))
}

/// Standardisation fitted on the training split only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scaling {
    pub means: Vec<f64>,
    pub scales: Vec<f64>,
}

impl Scaling {
    fn fit(rows: &[&[f64]]) -> Self {
        let width = rows.first().map_or(0, |r| r.len());
        let n = rows.len().max(1) as f64;
        let mut means = vec![0.0; width];
        for row in rows {
            for (m, v) in means.iter_mut().zip(row.iter()) {
                *m += v / n;
            }
        }
        let mut scales = vec![0.0; width];
        for row in rows {
            for ((s, v), m) in scales.iter_mut().zip(row.iter()).zip(means.iter()) {
                *s += (v - m).powi(2) / n;
            }
        }
        let scales = scales
            .into_iter()
            .map(|v| if v.sqrt() > 1e-12 { v.sqrt() } else { 1.0 })
            .collect();
        Self { means, scales }
    }

    fn apply(&self, row: &[f64]) -> Vec<f64> {
        row.iter()
            .zip(self.means.iter().zip(self.scales.iter()))
            .map(|(v, (m, s))| (v - m) / s)
            .collect()
    }
}

/// A fitted fill-probability model, with its holdout skill against the
/// naive base-rate baseline beside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillModel {
    pub features: Vec<String>,
    pub scaling: Scaling,
    pub intercept: f64,
    pub weights: Vec<f64>,
    /// The training base rate — the baseline's constant prediction.
    pub base_rate: f64,
    pub train_examples: usize,
    pub holdout_examples: usize,
    /// Mean squared error of the model's probability on the holdout.
    pub holdout_brier: f64,
    /// The same for the base rate.
    pub baseline_brier: f64,
}

impl FillModel {
    /// Probability that a bid resting at the touch fills within the horizon.
    pub fn predict(&self, features: &[f64]) -> f64 {
        let x = self.scaling.apply(features);
        let z = self.intercept
            + self
                .weights
                .iter()
                .zip(x.iter())
                .map(|(w, v)| w * v)
                .sum::<f64>();
        1.0 / (1.0 + (-z).exp())
    }

    /// Whether the model beat the base rate out of sample. A model that does
    /// not is not a discovery and is not to be served.
    pub fn beats_baseline(&self) -> bool {
        self.holdout_examples > 0 && self.holdout_brier < self.baseline_brier
    }
}

fn brier(pairs: impl Iterator<Item = (f64, f64)>) -> (f64, usize) {
    let mut sum = 0.0;
    let mut n = 0usize;
    for (p, y) in pairs {
        sum += (p - y).powi(2);
        n += 1;
    }
    (if n == 0 { f64::NAN } else { sum / n as f64 }, n)
}

/// Fit a logistic fill model by deterministic full-batch gradient descent
/// with an L2 penalty, on the earlier slice, and score it and the base rate
/// on the later. Refuses a split with either side empty or a training slice
/// holding one outcome only — a constant label has no model to fit.
pub fn train_fill_model(examples: &[MicroExample], holdout_fraction: f64) -> Result<FillModel> {
    let (train, holdout) = split_in_time(examples, holdout_fraction)?;
    if train.is_empty() || holdout.is_empty() {
        return Err(Error::invalid(format!(
            "{} example(s) cannot be split into a training and a holdout slice",
            examples.len()
        )));
    }
    let positives = train.iter().filter(|e| e.fill >= 0.5).count();
    if positives == 0 || positives == train.len() {
        return Err(Error::invalid(
            "every training example has the same fill outcome; there is no fill model to fit, \
             only a constant",
        ));
    }
    let rows: Vec<&[f64]> = train.iter().map(|e| e.features.as_slice()).collect();
    let scaling = Scaling::fit(&rows);
    let x: Vec<Vec<f64>> = train.iter().map(|e| scaling.apply(&e.features)).collect();
    let y: Vec<f64> = train.iter().map(|e| e.fill).collect();
    let width = FEATURES.len();
    let mut weights = vec![0.0; width];
    let base_rate = positives as f64 / train.len() as f64;
    let mut intercept = (base_rate / (1.0 - base_rate)).ln();
    let n = train.len() as f64;
    let (rate, l2, iterations) = (0.5, 1e-3, 500);
    for _ in 0..iterations {
        let mut grad_w = vec![0.0; width];
        let mut grad_b = 0.0;
        for (row, target) in x.iter().zip(y.iter()) {
            let z = intercept + weights.iter().zip(row).map(|(w, v)| w * v).sum::<f64>();
            let error = 1.0 / (1.0 + (-z).exp()) - target;
            grad_b += error / n;
            for (g, v) in grad_w.iter_mut().zip(row) {
                *g += error * v / n;
            }
        }
        intercept -= rate * grad_b;
        for (w, g) in weights.iter_mut().zip(grad_w) {
            *w -= rate * (g + l2 * *w);
        }
    }
    if !intercept.is_finite() || weights.iter().any(|w| !w.is_finite()) {
        return Err(Error::numeric(
            "the fill model diverged; its weights are not finite",
        ));
    }
    let mut model = FillModel {
        features: FEATURES.iter().map(|s| (*s).to_string()).collect(),
        scaling,
        intercept,
        weights,
        base_rate,
        train_examples: train.len(),
        holdout_examples: holdout.len(),
        holdout_brier: f64::NAN,
        baseline_brier: f64::NAN,
    };
    model.holdout_brier = brier(holdout.iter().map(|e| (model.predict(&e.features), e.fill))).0;
    model.baseline_brier = brier(holdout.iter().map(|e| (base_rate, e.fill))).0;
    Ok(model)
}

/// A short-horizon realised-volatility model: least squares of the label on
/// trailing volatility, scored against the training mean on the holdout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VolatilityModel {
    pub intercept: f64,
    pub slope: f64,
    pub holdout_mse: f64,
    pub baseline_mse: f64,
}

impl VolatilityModel {
    pub fn beats_baseline(&self) -> bool {
        self.holdout_mse < self.baseline_mse
    }
}

/// Fit [`VolatilityModel`] on the earlier slice, score on the later.
pub fn train_volatility_model(
    examples: &[MicroExample],
    holdout_fraction: f64,
) -> Result<VolatilityModel> {
    let (train, holdout) = split_in_time(examples, holdout_fraction)?;
    if train.len() < 2 || holdout.is_empty() {
        return Err(Error::invalid(
            "a volatility model needs two training examples and a holdout",
        ));
    }
    let xs: Vec<f64> = train.iter().map(|e| e.features[2]).collect();
    let ys: Vec<f64> = train.iter().map(|e| e.realized_volatility).collect();
    let n = xs.len() as f64;
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let sxx: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let slope = if sxx > 1e-18 { sxy / sxx } else { 0.0 };
    let intercept = my - slope * mx;
    let holdout_mse = holdout
        .iter()
        .map(|e| (intercept + slope * e.features[2] - e.realized_volatility).powi(2))
        .sum::<f64>()
        / holdout.len() as f64;
    let baseline_mse = holdout
        .iter()
        .map(|e| (my - e.realized_volatility).powi(2))
        .sum::<f64>()
        / holdout.len() as f64;
    Ok(VolatilityModel {
        intercept,
        slope,
        holdout_mse,
        baseline_mse,
    })
}

/// One benchmark cell.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BenchmarkKey {
    pub venue: String,
    pub asset: String,
    pub regime: String,
    pub horizon_nanos: i64,
}

/// A model's score in one cell, beside the baseline's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkRow {
    pub examples: usize,
    pub brier: f64,
    pub baseline_brier: f64,
}

/// One row per venue × asset × regime × horizon, never an average across
/// them: an average across venues hides the venue where the model is wrong.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub rows: BTreeMap<BenchmarkKey, BenchmarkRow>,
}

impl BenchmarkReport {
    /// Cells where the model lost to the baseline.
    pub fn losing_cells(&self) -> Vec<&BenchmarkKey> {
        self.rows
            .iter()
            .filter(|(_, row)| row.brier >= row.baseline_brier)
            .map(|(key, _)| key)
            .collect()
    }
}

/// Benchmark `model` on `examples` (normally its holdout), sliced.
pub fn benchmark_fill_model(model: &FillModel, examples: &[MicroExample]) -> BenchmarkReport {
    let mut cells: BTreeMap<BenchmarkKey, Vec<&MicroExample>> = BTreeMap::new();
    for example in examples {
        cells
            .entry(BenchmarkKey {
                venue: example.venue.clone(),
                asset: example.instrument.clone(),
                regime: example.regime.clone(),
                horizon_nanos: example.horizon.as_nanos(),
            })
            .or_default()
            .push(example);
    }
    let rows = cells
        .into_iter()
        .map(|(key, cell)| {
            let (model_brier, n) = brier(cell.iter().map(|e| (model.predict(&e.features), e.fill)));
            let (baseline, _) = brier(cell.iter().map(|e| (model.base_rate, e.fill)));
            (
                key,
                BenchmarkRow {
                    examples: n,
                    brier: model_brier,
                    baseline_brier: baseline,
                },
            )
        })
        .collect();
    BenchmarkReport { rows }
}
