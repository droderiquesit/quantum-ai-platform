//! The node's issuer of the lattice's microsecond row (R12-WORLD-3084,
//! ADR 0133 decision 2, ADR 0506).
//!
//! The central lattice refuses this row from the central lane, so before this
//! module the row had a promotion target and no producer: nothing issued a
//! microsecond forecast anywhere, and "promoted only to the local reflex"
//! described outputs that did not exist. The cell is the only party that
//! sees its own fills, queue and post-fill moves at microsecond resolution,
//! so it issues them here, as [`ReflexForecast`] contracts, from what it has
//! itself observed.
//!
//! # The model, and why it is this one
//!
//! The issuer is a bounded empirical-frequency model — `reflex-frequency/1`.
//! An event target (fill probability, adverse selection) is forecast as the
//! Laplace-smoothed frequency of the event over the last [`REFLEX_WINDOW`]
//! observations for that venue, instrument and target; a magnitude target
//! (queue movement, executable edge) as the window's empirical 10th, 50th and
//! 90th percentiles. That is the least model that is calibrated by
//! construction on a stationary venue, and a reflex lane's first obligation is
//! to be calibrated, not clever: a sharper model earns its place by beating
//! this one on the score below. It never states 0 or 100 %, because Laplace
//! smoothing keeps both outcomes possible on any finite window, and it refuses
//! to state anything at all before [`MIN_OBSERVATIONS`].
//!
//! # Calibration is measured, not asserted
//!
//! Every issued forecast can be resolved against what happened
//! ([`ReflexIssuer::resolve_event`], [`ReflexIssuer::resolve_magnitude`]),
//! and the issuer keeps per target a Brier score for events, a reliability
//! table in ten bins, and per-quantile coverage for magnitudes. The next
//! forecast carries the Brier score as its [`Calibration`]. The centre
//! re-scores reflex forecasts after the fact from outcomes that reach it
//! (ADR 0133), so this figure is the node's own claim, checked elsewhere.
//!
//! # A record and evidence, not a decision input
//!
//! ADR 0195 decision 1 adds no second forecast form to the cell's decisions:
//! the expected edge a rule gates on is a promoted distillate whose
//! calibration ships on the model, never a number the cell adjusts itself.
//! This issuer does adjust its numbers from what it observes, which is why
//! nothing in the cell's pass reads what it issues — no gate, no sizing, no
//! venue choice. Its forecasts are the microsecond row stated and scored,
//! for the centre to re-score and for a future distillate to be measured
//! against. `qip-acceptance`'s `reflex_forecast_boundary` suite refuses any
//! other `qip-edge` source naming it.
//!
//! # Bounds
//!
//! Each series keeps [`REFLEX_WINDOW`] observations, oldest dropped; the
//! issuer keeps at most [`MAX_SERIES`] series and refuses a new one past it
//! rather than evicting a live one, because an evicted series silently
//! restarts its warm-up and the forecast it stops issuing is not announced.

use qip_contracts::forecast::{Calibration, OutcomeDistribution, OutcomeMass, QuantilePoint};
use qip_contracts::reflex_forecast::{
    MAX_REFLEX_HORIZON_US, NOT_OCCURRED, OCCURRED, ReflexForecast, ReflexForecastFields,
    ReflexPromotion, ReflexTarget,
};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use std::collections::{BTreeMap, VecDeque};

/// Observations kept per venue, instrument and target.
pub const REFLEX_WINDOW: usize = 256;
/// Observations a series needs before the issuer states anything from it.
pub const MIN_OBSERVATIONS: usize = 20;
/// Series the issuer keeps; a new one past this is refused.
pub const MAX_SERIES: usize = 1_024;
/// The model name every forecast from this issuer carries.
pub const MODEL: &str = "reflex-frequency/1";
/// The quantiles a magnitude forecast states, in basis points.
pub const MAGNITUDE_QUANTILES_BP: [u32; 3] = [1_000, 5_000, 9_000];
/// Reliability bins for event targets, each a tenth of probability.
pub const RELIABILITY_BINS: usize = 10;

const BP: i64 = 10_000;

type SeriesKey = (VenueId, String, ReflexTarget);

#[derive(Debug, Default)]
struct Series {
    events: VecDeque<bool>,
    magnitudes: VecDeque<Decimal>,
}

/// One bin of a reliability table: the forecasts that fell in it, what they
/// said on average, and how often the event then happened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReliabilityBin {
    pub forecasts: u64,
    /// Sum of the stated probabilities, in basis points.
    pub stated_bp_sum: u64,
    pub occurred: u64,
}

impl ReliabilityBin {
    /// The mean stated probability in basis points; `None` for an empty bin.
    pub fn mean_stated_bp(&self) -> Option<u64> {
        self.stated_bp_sum.checked_div(self.forecasts)
    }

    /// The observed frequency in basis points; `None` for an empty bin.
    pub fn observed_bp(&self) -> Option<u64> {
        (self.occurred * BP as u64).checked_div(self.forecasts)
    }
}

#[derive(Debug, Default)]
struct EventScore {
    resolved: u64,
    /// Σ (stated − outcome)², both in basis points.
    squared_error_bp2: u128,
    bins: [ReliabilityBin; RELIABILITY_BINS],
}

#[derive(Debug, Default)]
struct MagnitudeScore {
    resolved: u64,
    /// Per stated quantile, how many outcomes fell at or below it.
    at_or_below: [u64; MAGNITUDE_QUANTILES_BP.len()],
}

/// The cell's issuer of microsecond forecasts.
#[derive(Debug)]
pub struct ReflexIssuer {
    cell: String,
    horizon_us: u64,
    series: BTreeMap<SeriesKey, Series>,
    events: BTreeMap<ReflexTarget, EventScore>,
    magnitudes: BTreeMap<ReflexTarget, MagnitudeScore>,
    issued: u64,
}

impl ReflexIssuer {
    /// An issuer for `cell` stating forecasts `horizon_us` ahead. Refuses a
    /// blank cell and a horizon outside the microsecond row, so a
    /// misconfigured node fails at construction rather than at every issue.
    pub fn new(cell: impl Into<String>, horizon_us: u64) -> Result<Self> {
        let cell = cell.into();
        if cell.trim().is_empty() {
            return Err(Error::invalid(
                "a reflex issuer needs the cell it issues for; name the cell",
            ));
        }
        if horizon_us == 0 || horizon_us > MAX_REFLEX_HORIZON_US {
            return Err(Error::invalid(format!(
                "a reflex issuer's horizon is {horizon_us} µs; the microsecond row speaks from 1 \
                 to {MAX_REFLEX_HORIZON_US} µs"
            )));
        }
        Ok(Self {
            cell,
            horizon_us,
            series: BTreeMap::new(),
            events: BTreeMap::new(),
            magnitudes: BTreeMap::new(),
            issued: 0,
        })
    }

    fn series_mut(&mut self, key: SeriesKey) -> Result<&mut Series> {
        if !self.series.contains_key(&key) && self.series.len() >= MAX_SERIES {
            return Err(Error::invalid(format!(
                "the reflex issuer already keeps {MAX_SERIES} series; a new one for {} {} {} is \
                 refused rather than evicting a live series",
                key.0.as_str(),
                key.1,
                key.2.as_str()
            )));
        }
        Ok(self.series.entry(key).or_default())
    }

    /// Record whether an event target happened within the horizon.
    pub fn observe_event(
        &mut self,
        venue: &VenueId,
        instrument: &str,
        target: ReflexTarget,
        occurred: bool,
    ) -> Result<()> {
        if !target.is_event() {
            return Err(Error::invalid(format!(
                "{} is a magnitude; observe it with observe_magnitude",
                target.as_str()
            )));
        }
        let s = self.series_mut((venue.clone(), instrument.to_string(), target))?;
        s.events.push_back(occurred);
        if s.events.len() > REFLEX_WINDOW {
            s.events.pop_front();
        }
        Ok(())
    }

    /// Record a magnitude target's realised value over the horizon.
    pub fn observe_magnitude(
        &mut self,
        venue: &VenueId,
        instrument: &str,
        target: ReflexTarget,
        value: Decimal,
    ) -> Result<()> {
        if target.is_event() {
            return Err(Error::invalid(format!(
                "{} is an event; observe it with observe_event",
                target.as_str()
            )));
        }
        let s = self.series_mut((venue.clone(), instrument.to_string(), target))?;
        s.magnitudes.push_back(value);
        if s.magnitudes.len() > REFLEX_WINDOW {
            s.magnitudes.pop_front();
        }
        Ok(())
    }

    /// Observations held for a series.
    pub fn observations(&self, venue: &VenueId, instrument: &str, target: ReflexTarget) -> usize {
        self.series
            .get(&(venue.clone(), instrument.to_string(), target))
            .map_or(0, |s| s.events.len() + s.magnitudes.len())
    }

    /// Issue a forecast at `now`, or `None` while the series holds fewer than
    /// [`MIN_OBSERVATIONS`]: a frequency over a handful of outcomes is noise
    /// stated as a probability.
    pub fn issue(
        &mut self,
        venue: &VenueId,
        instrument: &str,
        target: ReflexTarget,
        now: Timestamp,
    ) -> Result<Option<ReflexForecast>> {
        let key = (venue.clone(), instrument.to_string(), target);
        let Some(series) = self.series.get(&key) else {
            return Ok(None);
        };
        let distribution = if target.is_event() {
            let n = series.events.len();
            if n < MIN_OBSERVATIONS {
                return Ok(None);
            }
            let k = series.events.iter().filter(|o| **o).count();
            let p = laplace_bp(k as u64, n as u64);
            OutcomeDistribution::Categorical {
                outcomes: vec![
                    OutcomeMass {
                        outcome: OCCURRED.into(),
                        probability_bp: p,
                    },
                    OutcomeMass {
                        outcome: NOT_OCCURRED.into(),
                        probability_bp: BP as u32 - p,
                    },
                ],
            }
        } else {
            let n = series.magnitudes.len();
            if n < MIN_OBSERVATIONS {
                return Ok(None);
            }
            let mut sorted: Vec<Decimal> = series.magnitudes.iter().copied().collect();
            sorted.sort_unstable();
            let points = MAGNITUDE_QUANTILES_BP
                .iter()
                .map(|q| QuantilePoint {
                    quantile_bp: *q,
                    value: sorted[(*q as usize * (n - 1)) / BP as usize],
                })
                .collect();
            OutcomeDistribution::Quantiles { points }
        };
        self.issued += 1;
        let calibration = self.calibration(target, now);
        let horizon = Duration::from_micros(i64::try_from(self.horizon_us).unwrap_or(i64::MAX));
        ReflexForecast::new(ReflexForecastFields {
            schema_version: 1,
            forecast_id: format!("{}-rf-{}", self.cell, self.issued),
            cell: self.cell.clone(),
            venue: venue.clone(),
            instrument: instrument.to_string(),
            target,
            horizon_us: self.horizon_us,
            issued_at: now,
            expires_at: now.saturating_add(horizon),
            distribution,
            calibration,
            model: MODEL.to_string(),
            promotion: ReflexPromotion::LocalReflexOnly,
        })
        .map(Some)
    }

    fn own(&self, forecast: &ReflexForecast) -> Result<()> {
        let f = forecast.fields();
        if f.cell != self.cell || f.model != MODEL {
            return Err(Error::invalid(format!(
                "forecast {} was issued by {} under {}, not by this issuer; score a forecast \
                 against the model that issued it",
                f.forecast_id, f.cell, f.model
            )));
        }
        Ok(())
    }

    /// Score an event forecast against whether the event happened.
    pub fn resolve_event(&mut self, forecast: &ReflexForecast, occurred: bool) -> Result<()> {
        self.own(forecast)?;
        let f = forecast.fields();
        let Some(stated) = f.occurred_bp() else {
            return Err(Error::invalid(format!(
                "forecast {} is a magnitude; resolve it with resolve_magnitude",
                f.forecast_id
            )));
        };
        let outcome: i64 = if occurred { BP } else { 0 };
        let err = i64::from(stated) - outcome;
        let score = self.events.entry(f.target).or_default();
        score.resolved += 1;
        score.squared_error_bp2 += (err * err) as u128;
        let bin = (stated as usize * RELIABILITY_BINS / BP as usize).min(RELIABILITY_BINS - 1);
        let b = &mut score.bins[bin];
        b.forecasts += 1;
        b.stated_bp_sum += u64::from(stated);
        b.occurred += u64::from(occurred);
        Ok(())
    }

    /// Score a magnitude forecast against the realised value.
    pub fn resolve_magnitude(&mut self, forecast: &ReflexForecast, value: Decimal) -> Result<()> {
        self.own(forecast)?;
        let f = forecast.fields();
        let OutcomeDistribution::Quantiles { points } = &f.distribution else {
            return Err(Error::invalid(format!(
                "forecast {} is an event; resolve it with resolve_event",
                f.forecast_id
            )));
        };
        if points.len() != MAGNITUDE_QUANTILES_BP.len() {
            return Err(Error::invalid(format!(
                "forecast {} states {} quantiles, not the issuer's {}",
                f.forecast_id,
                points.len(),
                MAGNITUDE_QUANTILES_BP.len()
            )));
        }
        let score = self.magnitudes.entry(f.target).or_default();
        score.resolved += 1;
        for (count, p) in score.at_or_below.iter_mut().zip(points) {
            *count += u64::from(value <= p.value);
        }
        Ok(())
    }

    /// The Brier score of resolved event forecasts for `target`, in [0, 1];
    /// `None` before any is resolved.
    pub fn brier(&self, target: ReflexTarget) -> Option<Decimal> {
        let s = self.events.get(&target)?;
        if s.resolved == 0 {
            return None;
        }
        let num = Decimal::from(i64::try_from(s.squared_error_bp2).ok()?);
        let den = Decimal::from(i64::try_from(s.resolved).ok()?) * Decimal::from(BP * BP);
        num.checked_div(den)
    }

    /// The reliability table for an event target.
    pub fn reliability(&self, target: ReflexTarget) -> [ReliabilityBin; RELIABILITY_BINS] {
        self.events
            .get(&target)
            .map_or([ReliabilityBin::default(); RELIABILITY_BINS], |s| s.bins)
    }

    /// Per stated quantile, the share of resolved outcomes at or below it, in
    /// basis points; a calibrated magnitude model's shares match the stated
    /// quantiles. `None` before any is resolved.
    pub fn coverage_bp(&self, target: ReflexTarget) -> Option<[u64; MAGNITUDE_QUANTILES_BP.len()]> {
        let s = self.magnitudes.get(&target)?;
        if s.resolved == 0 {
            return None;
        }
        let mut out = [0; MAGNITUDE_QUANTILES_BP.len()];
        for (o, c) in out.iter_mut().zip(s.at_or_below) {
            *o = c * BP as u64 / s.resolved;
        }
        Some(out)
    }

    /// What the next forecast at `target` states about its own record.
    fn calibration(&self, target: ReflexTarget, now: Timestamp) -> Calibration {
        if target.is_event() {
            if let (Some(score), Some(s)) = (self.brier(target), self.events.get(&target)) {
                return Calibration::Measured {
                    method: "brier".into(),
                    score,
                    resolved: s.resolved,
                    evaluated_at: now,
                };
            }
        } else if let (Some(cov), Some(s)) =
            (self.coverage_bp(target), self.magnitudes.get(&target))
        {
            // Mean absolute gap between stated and observed coverage, as a
            // fraction of one.
            let gap: u64 = cov
                .iter()
                .zip(MAGNITUDE_QUANTILES_BP)
                .map(|(c, q)| c.abs_diff(u64::from(q)))
                .sum();
            let den = BP * MAGNITUDE_QUANTILES_BP.len() as i64;
            if let Some(score) = Decimal::from(i64::try_from(gap).unwrap_or(i64::MAX))
                .checked_div(Decimal::from(den))
            {
                return Calibration::Measured {
                    method: "quantile_coverage_gap".into(),
                    score,
                    resolved: s.resolved,
                    evaluated_at: now,
                };
            }
        }
        Calibration::Uncalibrated
    }
}

/// `(k + 1) / (n + 2)` in basis points, rounded to nearest. Strictly between
/// 0 and 10,000 for every `n`, so neither outcome is ever stated impossible.
fn laplace_bp(k: u64, n: u64) -> u32 {
    let den = n + 2;
    let bp = ((k + 1) * BP as u64 + den / 2) / den;
    // (k+1)/(n+2) < 1 for k <= n, so bp <= 10_000 and fits.
    u32::try_from(bp).unwrap_or(BP as u32 - 1)
}
