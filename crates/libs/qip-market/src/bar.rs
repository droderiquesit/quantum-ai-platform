//! OHLCV bars and bar series.

use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use qip_financial::quality::DataQuality;
use serde::{Deserialize, Serialize};

/// Bar sampling interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interval {
    Second,
    Minute,
    FiveMinutes,
    FifteenMinutes,
    Hour,
    Day,
    Week,
    Month,
}

impl Interval {
    pub fn duration(&self) -> Duration {
        match self {
            Self::Second => Duration::from_secs(1),
            Self::Minute => Duration::from_mins(1),
            Self::FiveMinutes => Duration::from_mins(5),
            Self::FifteenMinutes => Duration::from_mins(15),
            Self::Hour => Duration::from_hours(1),
            Self::Day => Duration::from_days(1),
            Self::Week => Duration::from_days(7),
            // Calendar months vary; 30 days is the sampling approximation and
            // is only used for bucketing, never for accrual.
            Self::Month => Duration::from_days(30),
        }
    }

    /// Bars per year, for annualising a volatility estimated at this interval.
    pub fn periods_per_year(&self) -> f64 {
        match self {
            Self::Second => 252.0 * 6.5 * 3600.0,
            Self::Minute => 252.0 * 390.0,
            Self::FiveMinutes => 252.0 * 78.0,
            Self::FifteenMinutes => 252.0 * 26.0,
            Self::Hour => 252.0 * 6.5,
            Self::Day => 252.0,
            Self::Week => 52.0,
            Self::Month => 12.0,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Second => "1s",
            Self::Minute => "1m",
            Self::FiveMinutes => "5m",
            Self::FifteenMinutes => "15m",
            Self::Hour => "1h",
            Self::Day => "1d",
            Self::Week => "1w",
            Self::Month => "1mo",
        }
    }
}

/// An OHLCV bar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub object_id: ObjectId,
    pub venue: String,
    pub interval: Interval,
    /// Start of the bucket the bar covers.
    pub open_time: Timestamp,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
    /// Volume-weighted average price over the bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vwap: Option<Decimal>,
    #[serde(default)]
    pub trade_count: u64,
    #[serde(default)]
    pub quality: DataQuality,
}

impl Bar {
    pub fn close_time(&self) -> Timestamp {
        self.open_time.saturating_add(self.interval.duration())
    }

    pub fn range(&self) -> Decimal {
        self.high - self.low
    }

    /// Simple return over the bar.
    pub fn return_pct(&self) -> f64 {
        if !self.open.is_positive() {
            return 0.0;
        }
        self.close.to_f64() / self.open.to_f64() - 1.0
    }

    pub fn typical_price(&self) -> Decimal {
        (self.high + self.low + self.close)
            .checked_div(Decimal::from_int(3))
            .unwrap_or(self.close)
    }

    /// True when the bar's own values are mutually consistent.
    pub fn is_coherent(&self) -> bool {
        self.high >= self.low
            && self.high >= self.open
            && self.high >= self.close
            && self.low <= self.open
            && self.low <= self.close
            && !self.volume.is_negative()
    }

    /// Merge with the next bar in time, producing the coarser bar.
    pub fn merged_with(&self, later: &Bar) -> Bar {
        Bar {
            object_id: self.object_id.clone(),
            venue: self.venue.clone(),
            interval: self.interval,
            open_time: self.open_time,
            open: self.open,
            high: self.high.max(later.high),
            low: self.low.min(later.low),
            close: later.close,
            volume: self.volume + later.volume,
            // Recombine the VWAPs by volume; without both, drop it rather than
            // report a number that is not actually volume-weighted.
            vwap: match (self.vwap, later.vwap) {
                (Some(a), Some(b)) => {
                    let total = self.volume + later.volume;
                    (a * self.volume + b * later.volume).checked_div(total)
                }
                _ => None,
            },
            trade_count: self.trade_count + later.trade_count,
            quality: self.quality.combined_with(&later.quality),
        }
    }
}

impl EventBody for Bar {
    const TOPIC: Topic = Topic::MarketBar;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}:{}",
            self.object_id,
            self.venue,
            self.interval.as_str(),
            self.open_time.as_nanos()
        ))
    }
}

/// A time-ordered series of bars for one instrument.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BarSeries {
    bars: Vec<Bar>,
}

impl BarSeries {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_bars(mut bars: Vec<Bar>) -> Self {
        bars.sort_by_key(|b| b.open_time.as_nanos());
        Self { bars }
    }

    /// Append a bar, keeping the series ordered and replacing a bar with the
    /// same open time (a late correction from the venue).
    pub fn push(&mut self, bar: Bar) {
        match self
            .bars
            .binary_search_by_key(&bar.open_time.as_nanos(), |b| b.open_time.as_nanos())
        {
            Ok(existing) => self.bars[existing] = bar,
            Err(position) => self.bars.insert(position, bar),
        }
    }

    pub fn len(&self) -> usize {
        self.bars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    pub fn bars(&self) -> &[Bar] {
        &self.bars
    }

    pub fn last(&self) -> Option<&Bar> {
        self.bars.last()
    }

    /// Bars strictly before `as_of` — the point-in-time view.
    ///
    /// A bar is only knowable once its bucket has closed, so the comparison is
    /// against `close_time`. Using `open_time` here is the classic look-ahead
    /// bug: it lets a strategy see the bar it is trading into.
    pub fn as_of(&self, as_of: Timestamp) -> &[Bar] {
        let count = self
            .bars
            .iter()
            .take_while(|b| b.close_time() <= as_of)
            .count();
        &self.bars[..count]
    }

    pub fn closes(&self) -> Vec<f64> {
        self.bars.iter().map(|b| b.close.to_f64()).collect()
    }

    pub fn volumes(&self) -> Vec<f64> {
        self.bars.iter().map(|b| b.volume.to_f64()).collect()
    }

    /// Simple returns between consecutive closes.
    pub fn returns(&self) -> Vec<f64> {
        qip_numerics::stats::simple_returns(&self.closes())
    }

    /// Log returns between consecutive closes.
    pub fn log_returns(&self) -> Vec<f64> {
        qip_numerics::stats::log_returns(&self.closes())
    }

    /// Annualised volatility of returns at this series' interval.
    pub fn annualised_volatility(&self) -> f64 {
        let returns = self.log_returns();
        if returns.len() < 2 {
            return 0.0;
        }
        let interval = self.bars.first().map_or(Interval::Day, |b| b.interval);
        qip_numerics::stats::stddev(&returns) * interval.periods_per_year().sqrt()
    }

    /// Volume-weighted average price across the series.
    pub fn vwap(&self) -> Option<Decimal> {
        let total_volume: Decimal = self.bars.iter().map(|b| b.volume).sum();
        if !total_volume.is_positive() {
            return None;
        }
        let weighted: Decimal = self
            .bars
            .iter()
            .map(|b| b.vwap.unwrap_or_else(|| b.typical_price()) * b.volume)
            .sum();
        weighted.checked_div(total_volume)
    }

    /// Aggregate to a coarser interval by merging consecutive buckets.
    pub fn resample(&self, target: Interval) -> BarSeries {
        if self.bars.is_empty() {
            return BarSeries::new();
        }
        let bucket = target.duration();
        let mut out: Vec<Bar> = Vec::new();
        for bar in &self.bars {
            let start = bar.open_time.floor_to(bucket);
            match out.last_mut() {
                Some(current) if current.open_time == start => {
                    *current = current.merged_with(bar);
                }
                _ => {
                    let mut coarse = bar.clone();
                    coarse.open_time = start;
                    coarse.interval = target;
                    out.push(coarse);
                }
            }
        }
        BarSeries { bars: out }
    }

    /// Gaps in the series: buckets with no bar between the first and last.
    ///
    /// Reported rather than filled — an imputed bar is a fact the platform
    /// invented, and the data-quality contract requires that be visible.
    pub fn missing_buckets(&self) -> Vec<Timestamp> {
        if self.bars.len() < 2 {
            return Vec::new();
        }
        let step = self.bars[0].interval.duration().as_nanos();
        if step <= 0 {
            return Vec::new();
        }
        let mut gaps = Vec::new();
        for pair in self.bars.windows(2) {
            let mut expected = pair[0].open_time.as_nanos() + step;
            while expected < pair[1].open_time.as_nanos() {
                gaps.push(Timestamp::from_nanos(expected));
                expected += step;
                if gaps.len() > 100_000 {
                    return gaps;
                }
            }
        }
        gaps
    }
}
