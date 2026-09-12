//! §22.1's retention classes as a type, and the one class that needed a
//! structure of its own: the fallback bar series.
//!
//! The blueprint's data policy is pass-through, not accumulation, and §22.1
//! tables the nine kinds of thing the platform keeps or deliberately does
//! not. Until this module the distinction that does the work — what only
//! this platform knows is kept because it cannot be recovered, what the
//! world already knows is not kept because it can — was enforced in the
//! event log's eviction order and stated nowhere as a vocabulary. A retention
//! class that exists only as a paragraph cannot be named by a health surface,
//! asserted by a test, or written beside a series to say why it is still
//! there.
//!
//! [`RetentionClass`] is the table. Nine variants, not ten: the row count
//! `docs/DELIVERY-STATUS.md` once gave for this section was a miscount, and
//! the enum's `ALL` is what to count now. Each variant answers
//! [`RetentionClass::retention`] with the row's own policy, in the row's own
//! words, and nothing here reinterprets a row it does not enforce — the
//! event-anchored 90-day roll and the per-class size accounting are
//! [`Retention`] values with no structure behind them yet, and the enum says
//! so rather than implying otherwise.
//!
//! # The fallback series
//!
//! §22.1's row: "One-minute OHLCV for instruments in an active class or
//! universe. Yes, three years. Insurance against a source withdrawing its
//! archive." §22.4's table repeats the insurance in its own words: "bar-level
//! fallback retained for three years on traded instruments." Daily bars
//! rather than one-minute here, and stated as a deviation: a minute series
//! for one instrument over three years is a million bars, and a platform with
//! no deployed collector and a bounded working set has no honest use for a
//! million-row insurance policy it cannot yet be shown to need. A daily series
//! is the smallest bar the research campaign can fall back to when a vendor
//! withdraws history, and it is bounded three ways below.
//!
//! Every bound is a stated constant, refused at zero, and enforced at the
//! seam that inserts: a series never holds a bar older than the retention
//! ceiling behind its newest, never more than the per-instrument count, and
//! the structure never holds more instruments than its ceiling — a new
//! instrument past that is refused by name rather than admitted by evicting
//! someone else's insurance.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_market::bar::{Bar, Interval};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The nine rows of §22.1's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    /// Raw ticks, book deltas, quote updates, source text.
    Transient,
    /// Features, moments, covariance, sketches, reservoirs.
    DerivedState,
    /// Own orders, fills, intents, verdicts, quotes, receipt timestamps,
    /// transfers.
    Irreplaceable,
    /// Per-strategy returns, family correlations, dispersion by venue pair,
    /// solver deltas, counterfactual scores.
    CompactDerived,
    /// Compressed state with outcome, indexed for retrieval.
    Episodic,
    /// Entities, relations, causal edges, beliefs, extracted facts.
    Semantic,
    /// Book state at each own order, fill, quote, veto, unwind.
    EventAnchored,
    /// Bars for instruments in an active class or universe.
    FallbackSeries,
    /// External market history, filings, registries.
    Referenced,
}

/// A row's answer to "retained?", in the table's own terms.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    /// A bounded ring measured in seconds, then gone.
    Never,
    /// In memory, fixed size regardless of throughput.
    InMemoryFixed,
    /// Permanently; only this platform has these.
    Permanent,
    /// Series, not observations.
    Series,
    /// Indefinitely; compressed meaning.
    Indefinite,
    /// A rolling window.
    Rolling(Duration),
    /// For a stated span behind the newest observation.
    For(Duration),
    /// A manifest with source, range and content hash; fetched on demand.
    ManifestOnly,
}

impl RetentionClass {
    /// The nine rows in the table's own order.
    pub const ALL: [Self; 9] = [
        Self::Transient,
        Self::DerivedState,
        Self::Irreplaceable,
        Self::CompactDerived,
        Self::Episodic,
        Self::Semantic,
        Self::EventAnchored,
        Self::FallbackSeries,
        Self::Referenced,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::DerivedState => "derived_state",
            Self::Irreplaceable => "irreplaceable",
            Self::CompactDerived => "compact_derived",
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
            Self::EventAnchored => "event_anchored",
            Self::FallbackSeries => "fallback_series",
            Self::Referenced => "referenced",
        }
    }

    /// The row's "what" column.
    pub const fn what(&self) -> &'static str {
        match self {
            Self::Transient => "raw ticks, book deltas, quote updates, source text",
            Self::DerivedState => "features, moments, covariance, sketches, reservoirs",
            Self::Irreplaceable => {
                "own orders, fills, intents, verdicts, quotes, receipt timestamps, transfers"
            }
            Self::CompactDerived => {
                "per-strategy returns, family correlations, dispersion by venue pair, solver \
                 deltas, counterfactual scores"
            }
            Self::Episodic => "compressed state with outcome, indexed for retrieval",
            Self::Semantic => "entities, relations, causal edges, beliefs, extracted facts",
            Self::EventAnchored => "book state at each own order, fill, quote, veto, unwind",
            Self::FallbackSeries => "bars for instruments in an active class or universe",
            Self::Referenced => "external market history, filings, registries",
        }
    }

    /// The row's "retained?" column.
    pub const fn retention(&self) -> Retention {
        match self {
            Self::Transient => Retention::Never,
            Self::DerivedState => Retention::InMemoryFixed,
            Self::Irreplaceable => Retention::Permanent,
            Self::CompactDerived => Retention::Series,
            Self::Episodic | Self::Semantic => Retention::Indefinite,
            Self::EventAnchored => Retention::Rolling(Duration::from_days(90)),
            Self::FallbackSeries => Retention::For(FALLBACK_RETENTION),
            Self::Referenced => Retention::ManifestOnly,
        }
    }

    /// Whether the row is kept because only this platform has it — the
    /// distinction §22.1 says does the work.
    pub const fn is_only_ours(&self) -> bool {
        matches!(
            self,
            Self::Irreplaceable | Self::CompactDerived | Self::Episodic | Self::Semantic
        )
    }
}

/// How far behind its newest bar a fallback series reaches: three years,
/// counted in days so a leap day does not shorten it.
pub const FALLBACK_RETENTION: Duration = Duration::from_days(3 * 365 + 1);

/// Daily bars held per instrument: three years of trading days is about
/// 756, three years of calendar days is 1,096, and a source that serves a
/// bar for every calendar day (a crypto venue does) must fit.
pub const FALLBACK_BARS_PER_INSTRUMENT: usize = 1_100;

/// Instruments the series holds bars for. Past this a new instrument is
/// refused rather than admitted by evicting another's insurance.
pub const FALLBACK_INSTRUMENTS: usize = 512;

/// What retaining one bar did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetainOutcome {
    /// A new bar for a period the series did not hold.
    Retained,
    /// A bar for a period already held, replaced by the newer observation.
    Replaced,
}

/// The bounded daily-bar series a research campaign falls back to when a
/// vendor withdraws history. See the module doc for the three bounds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FallbackSeries {
    retention: Duration,
    per_instrument: usize,
    instruments: usize,
    /// Per subject, bars in open-time order.
    series: BTreeMap<String, Vec<Bar>>,
    /// Bars evicted to stay within the bounds, over the series' life.
    evicted: u64,
}

impl FallbackSeries {
    /// A series with the stated bounds, refusing zero on any axis: a series
    /// that keeps nothing is not insurance, and one that admits no
    /// instrument insures nobody.
    pub fn new(retention: Duration, per_instrument: usize, instruments: usize) -> Result<Self> {
        if retention.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a fallback series needs a positive retention ceiling; a series that keeps \
                 nothing behind its newest bar is not insurance against anything",
            ));
        }
        if per_instrument == 0 || instruments == 0 {
            return Err(Error::invalid(
                "a fallback series must hold at least one bar for at least one instrument; a \
                 bound of zero is a prohibition, not a bound",
            ));
        }
        Ok(Self {
            retention,
            per_instrument,
            instruments,
            series: BTreeMap::new(),
            evicted: 0,
        })
    }

    /// The default bounds. See the constants.
    pub fn bounded() -> Self {
        Self {
            retention: FALLBACK_RETENTION,
            per_instrument: FALLBACK_BARS_PER_INSTRUMENT,
            instruments: FALLBACK_INSTRUMENTS,
            series: BTreeMap::new(),
            evicted: 0,
        }
    }

    pub const fn retention(&self) -> Duration {
        self.retention
    }

    pub const fn per_instrument(&self) -> usize {
        self.per_instrument
    }

    pub const fn instrument_bound(&self) -> usize {
        self.instruments
    }

    pub const fn evicted(&self) -> u64 {
        self.evicted
    }

    /// Instruments currently held.
    pub fn instruments(&self) -> usize {
        self.series.len()
    }

    /// Bars currently held across every instrument.
    pub fn total_bars(&self) -> usize {
        self.series.values().map(Vec::len).sum()
    }

    /// Keep a daily bar, evicting whatever the bounds no longer admit.
    ///
    /// Refuses a bar at any interval but a day — the series is daily by
    /// declaration, and a minute bar admitted "just this once" would make
    /// every count below a count of mixed things — and refuses a new
    /// instrument past the instrument bound. A bar for a period already held
    /// replaces the earlier observation, so a corrected bar is kept rather
    /// than kept beside the one it corrects.
    pub fn retain(&mut self, bar: Bar) -> Result<RetainOutcome> {
        if bar.interval != Interval::Day {
            return Err(Error::invalid(format!(
                "the fallback series holds daily bars and `{}` offered a {} bar; a series of \
                 mixed intervals is a series no count describes",
                bar.object_id.as_str(),
                bar.interval.as_str()
            )));
        }
        let subject = bar.object_id.as_str().to_string();
        if !self.series.contains_key(&subject) && self.series.len() >= self.instruments {
            return Err(Error::denied(format!(
                "the fallback series already holds its bound of {} instrument(s) and `{subject}` \
                 is not among them; it is refused rather than admitted by evicting another \
                 instrument's history",
                self.instruments
            )));
        }
        let bars = self.series.entry(subject).or_default();
        let outcome = match bars.binary_search_by(|held| held.open_time.cmp(&bar.open_time)) {
            Ok(index) => {
                bars[index] = bar;
                RetainOutcome::Replaced
            }
            Err(index) => {
                bars.insert(index, bar);
                RetainOutcome::Retained
            }
        };
        // Both bounds, against the newest bar the series now holds: first
        // the ceiling behind it, then the count.
        if let Some(newest) = bars.last().map(|held| held.open_time) {
            let floor = newest.saturating_sub(self.retention);
            let stale = bars
                .iter()
                .take_while(|held| held.open_time < floor)
                .count();
            if stale > 0 {
                bars.drain(..stale);
                self.evicted = self.evicted.saturating_add(stale as u64);
            }
        }
        if bars.len() > self.per_instrument {
            let excess = bars.len() - self.per_instrument;
            bars.drain(..excess);
            self.evicted = self.evicted.saturating_add(excess as u64);
        }
        Ok(outcome)
    }

    /// The bars held for `subject`, oldest first.
    pub fn bars(&self, subject: &str) -> &[Bar] {
        self.series.get(subject).map_or(&[], Vec::as_slice)
    }

    /// Every subject held, in id order.
    pub fn subjects(&self) -> impl Iterator<Item = &str> {
        self.series.keys().map(String::as_str)
    }

    /// The span the held bars for `subject` cover, oldest open to newest
    /// open.
    pub fn span(&self, subject: &str) -> Option<(Timestamp, Timestamp)> {
        let bars = self.series.get(subject)?;
        Some((bars.first()?.open_time, bars.last()?.open_time))
    }
}

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` on the gate it is exercising still has to
// assert, and the abort is the reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_core::{Decimal, ObjectId};
    use qip_financial::quality::DataQuality;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn daily(subject: &str, day: i64, close: i64) -> Bar {
        Bar {
            object_id: ObjectId::from_string(subject),
            venue: "XSIM".to_string(),
            interval: Interval::Day,
            open_time: start().saturating_add(Duration::from_days(day)),
            open: Decimal::from_int(close),
            high: Decimal::from_int(close + 1),
            low: Decimal::from_int(close - 1),
            close: Decimal::from_int(close),
            volume: Decimal::from_int(1_000),
            vwap: None,
            trade_count: 10,
            quality: DataQuality::default(),
        }
    }

    /// Every row answers with its own policy, the labels are distinct, and
    /// the count is nine — the number the status document once got wrong.
    #[test]
    fn the_nine_retention_classes_each_state_their_own_policy() {
        let labels: std::collections::BTreeSet<&str> = RetentionClass::ALL
            .iter()
            .map(RetentionClass::as_str)
            .collect();
        assert_eq!(labels.len(), 9);
        assert_eq!(RetentionClass::Transient.retention(), Retention::Never);
        assert_eq!(
            RetentionClass::Irreplaceable.retention(),
            Retention::Permanent
        );
        assert_eq!(
            RetentionClass::EventAnchored.retention(),
            Retention::Rolling(Duration::from_days(90))
        );
        assert_eq!(
            RetentionClass::FallbackSeries.retention(),
            Retention::For(FALLBACK_RETENTION)
        );
        assert_eq!(
            RetentionClass::Referenced.retention(),
            Retention::ManifestOnly
        );
        assert!(RetentionClass::Irreplaceable.is_only_ours());
        assert!(!RetentionClass::Referenced.is_only_ours());
    }

    /// The series never holds a bar older than the retention ceiling behind
    /// its newest, never more than the per-instrument bound, and never more
    /// instruments than its ceiling — and each eviction is counted.
    ///
    /// Mutated by deleting the `if bars.len() > self.per_instrument` block in
    /// `retain` — confirmed the count assertion then fails, then restored.
    /// Also mutated by deleting the `stale` drain — confirmed the ceiling
    /// assertion then fails, then restored.
    #[test]
    fn the_fallback_series_holds_its_three_bounds() -> Result<()> {
        let mut series = FallbackSeries::new(Duration::from_days(10), 5, 2)?;

        // The per-instrument count: eight daily bars against a bound of five.
        for day in 0..8 {
            assert_eq!(
                series.retain(daily("AAA", day, 100 + day))?,
                RetainOutcome::Retained
            );
        }
        assert_eq!(series.bars("AAA").len(), 5, "the count bound was exceeded");
        assert_eq!(series.evicted(), 3);
        assert_eq!(
            series.span("AAA").map(|(oldest, _)| oldest),
            Some(start().saturating_add(Duration::from_days(3))),
            "the oldest bars are the ones evicted"
        );

        // The retention ceiling: a bar thirty days on evicts all five bars
        // more than ten days behind it, even though the count bound alone
        // would have evicted one and kept the other four.
        series.retain(daily("AAA", 30, 200))?;
        assert_eq!(
            series.bars("AAA").len(),
            1,
            "bars older than the ceiling behind the newest survived"
        );
        assert_eq!(series.evicted(), 3 + 5);

        // A corrected bar for a held period replaces rather than duplicates.
        assert_eq!(
            series.retain(daily("AAA", 30, 201))?,
            RetainOutcome::Replaced
        );
        assert_eq!(series.bars("AAA").len(), 1);
        assert_eq!(series.bars("AAA")[0].close, Decimal::from_int(201));

        // The instrument bound: a second instrument is admitted, a third is
        // refused by name.
        series.retain(daily("BBB", 30, 50))?;
        let refused = series
            .retain(daily("CCC", 30, 50))
            .expect_err("a third instrument was admitted past a bound of two");
        assert_eq!(refused.code(), "denied", "got {refused:?}");
        assert!(refused.message().contains("CCC"));
        assert_eq!(series.instruments(), 2);

        // A minute bar is refused, whatever the instrument.
        let mut minute = daily("AAA", 31, 210);
        minute.interval = Interval::Minute;
        assert!(series.retain(minute).is_err());

        // Zero is refused on every axis.
        assert!(FallbackSeries::new(Duration::ZERO, 1, 1).is_err());
        assert!(FallbackSeries::new(Duration::from_days(1), 0, 1).is_err());
        assert!(FallbackSeries::new(Duration::from_days(1), 1, 0).is_err());
        Ok(())
    }
}
