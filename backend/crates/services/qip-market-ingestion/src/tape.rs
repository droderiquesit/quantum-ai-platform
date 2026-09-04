//! A committed, bitemporal tape that owns the clock it is replayed on.
//!
//! [`crate::replay`] replays a recording against whatever clock the caller
//! holds, which is the right shape for reproducing a session and the wrong
//! one for demonstrating the loop: a node on the wall clock hands a recorded
//! tape to the platform in one poll — every record is already in the past —
//! runs one cycle over all of it, and stops. Nothing that takes tape time to
//! happen ever happens. A hypothesis with a five-day horizon recorded at
//! cycle one is never scored, because there is no cycle two.
//!
//! A [`Tape`] is different in three ways, and each is a refusal rather than
//! a convenience:
//!
//! * **Every record carries two instants.** `at` is when the fact was true
//!   in the world — the bar's close, the macro figure's reference period, the
//!   day the reading was observed — and `known_at` is when it became knowable
//!   to a consumer. [`TapeFeed::poll`] releases by `known_at`, never by `at`,
//!   and [`Tape::parse`] refuses a tape in which any `known_at` precedes its
//!   `at`: a bar knowable before it closed is look-ahead, and a backtest that
//!   reads it is measuring the leak, however good the number.
//! * **Order is checked, not imposed.** [`crate::replay::ReplayAdapter`]
//!   sorts what it reads. A tape whose `known_at` instants go backwards is
//!   refused, naming the two positions, because a file that had to be sorted
//!   to be replayed is a file whose author's account of when things became
//!   knowable cannot be trusted — and the sort would hide exactly that.
//! * **The tape owns time.** [`TapeFeed::advance`] moves a [`ManualClock`]
//!   to the next `known_at` and returns it, so a node can run one cycle per
//!   tape period at whatever wall-clock cadence it likes, and a prediction
//!   with a five-day horizon resolves five periods later. The same clock is
//!   what the platform's `Context` should be built on; a platform reasoning
//!   at wall time over a tape from last year would price every opportunity
//!   as already expired.
//!
//! A tape carries three kinds of record in three sections — bars, macro
//! releases and alternative-data readings — each validated under the same
//! refusals and each released by its own `known_at`. The bars set the clock:
//! a release or a reading knowable before the tape's first bar is refused,
//! because the platform starts at that bar and nothing is knowable to it
//! earlier. History already published by then — thirty-six monthly prints
//! the macro analyst needs before its first standardisation means anything —
//! is stamped knowable at the tape's first instant, which is when this
//! consumer could first have read it, and carries its true reference date as
//! `at`. Stamping it later than its publication is conservative; stamping it
//! earlier than the clock would be a cycle before the tape began.
//!
//! What this is not: market data. A tape here is a synthetic fixture for
//! demonstrating that the loop runs end to end on data with a detectable
//! structure in it. Its descriptor says [`LicensingClass::Synthetic`], which
//! the object model bars from any production decision, and nothing here can
//! change that class.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ManualClock, ObjectId, Timestamp};
use qip_events::Topic;
use qip_financial::intelligence::{AlternativeDataPoint, MacroObservation};
use qip_financial::quality::{DataQuality, LicensingClass, Provenance};
use qip_market::bar::{Bar, Interval};
use qip_market::corporate_action::{CorporateAction, CorporateActionKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use crate::adapter::{DataAdapter, SensedRecord, SourceDescriptor};

/// The one document layout this module reads. Bumped when a field changes
/// meaning, so an old tape is refused rather than misread.
///
/// Two, since the tape gained its macro and alternative-data sections. A
/// reader that knew only the bars would have replayed a version-two tape
/// with both sections silently dropped, and a run that reported the macro
/// analyst had nothing would have read as the analyst's defect rather than
/// the reader's.
pub const SCHEMA_VERSION: u32 = 2;

/// One closed bar and the two instants that place it in time.
///
/// Prices and volume travel as decimal strings, the same way the instrument
/// catalogue spells them, so a tape never carries a binary float that two
/// readers could round differently.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TapeObservation {
    pub object_id: String,
    pub venue: String,
    /// When the bar closed — the instant the fact was true. RFC 3339.
    pub at: String,
    /// When the bar became knowable. RFC 3339; never before `at`.
    pub known_at: String,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
}

/// One macro release and the two instants that place it in time.
///
/// The value is a statistic, not money, so it travels as the `f64` the
/// observation carries; `serde_json` writes the shortest text that reads
/// back to the same float, so two readers cannot disagree about it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TapeRelease {
    /// The vendor's series id, `{region}.{CODE}`. The world model's
    /// vocabulary recognises the codes the macro analyst reads; any other is
    /// recorded raw and read by nothing.
    pub series_id: String,
    /// ISO country or region code — the key the release lands under.
    pub region: String,
    /// The reference period the figure describes. RFC 3339.
    pub at: String,
    /// When it was published. RFC 3339; never before `at`, and never before
    /// the tape's first bar is knowable.
    pub known_at: String,
    pub value: f64,
    pub unit: String,
    /// Consensus ahead of the release, where the fixture states one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consensus: Option<f64>,
}

/// One alternative-data reading and the two instants that place it in time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TapeReading {
    /// The dataset the licence is held under.
    pub dataset: String,
    /// The metric, which is the feature name the analyst reads.
    pub metric: String,
    /// The instrument or entity the reading concerns.
    pub subject_id: String,
    /// When the phenomenon was observed. RFC 3339.
    pub at: String,
    /// When the reading was published. RFC 3339; never before `at`, and
    /// never before the tape's first bar is knowable.
    pub known_at: String,
    pub value: f64,
    pub unit: String,
}

/// The committed document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TapeDocument {
    pub schema_version: u32,
    pub name: String,
    /// What the tape is for and what it is not. Read by nothing; written for
    /// the person who opens the file.
    pub description: String,
    pub interval: Interval,
    pub observations: Vec<TapeObservation>,
    /// Defaulted so a bars-only tape needs no empty section.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macro_releases: Vec<TapeRelease>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternative_data: Vec<TapeReading>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dividend_declarations: Vec<TapeDeclaration>,
}

/// One cash-dividend declaration and the two instants that place it in time.
///
/// The one corporate action a tape can carry, and it is carried for a reason
/// that is the catalyst detector's: once a tape holds any intelligence
/// record at all, the detector treats the event stream as watched and calls
/// a large move with no knowable event on its instrument *unexplained* —
/// and the platform, by design, forms no hypothesis about an unexplained
/// move. A corporate action is the one record kind whose event lands on the
/// instrument's own id, so it is the one catalyst a bar-keyed jump can have.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TapeDeclaration {
    pub object_id: String,
    /// When the dividend was announced — the instant the fact was true.
    /// RFC 3339.
    pub at: String,
    /// When the announcement became knowable. RFC 3339; never before `at`,
    /// and never before the tape's first bar is knowable.
    pub known_at: String,
    /// First date the instrument trades without the entitlement. RFC 3339;
    /// never before `at`.
    pub ex_date: String,
    pub amount: Decimal,
}

/// One bar after validation, and its two instants.
#[derive(Clone, Debug, PartialEq)]
pub struct TapeEntry {
    pub at: Timestamp,
    pub known_at: Timestamp,
    pub bar: Bar,
}

/// One macro release after validation, and its two instants.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseEntry {
    pub at: Timestamp,
    pub known_at: Timestamp,
    pub observation: MacroObservation,
}

/// One alternative-data reading after validation, and its two instants.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadingEntry {
    pub at: Timestamp,
    pub known_at: Timestamp,
    pub point: AlternativeDataPoint,
}

/// One dividend declaration after validation, and its two instants.
#[derive(Clone, Debug, PartialEq)]
pub struct DeclarationEntry {
    pub at: Timestamp,
    pub known_at: Timestamp,
    pub action: CorporateAction,
}

/// A validated tape, each section in the order it was written.
#[derive(Clone, Debug, PartialEq)]
pub struct Tape {
    name: String,
    interval: Interval,
    entries: Vec<TapeEntry>,
    releases: Vec<ReleaseEntry>,
    readings: Vec<ReadingEntry>,
    declarations: Vec<DeclarationEntry>,
}

/// The two instants of one record, parsed and held to the refusals every
/// section shares: no leak, no disorder, no restatement within a series, and
/// nothing knowable before the tape's clock starts.
struct Placement {
    at: Timestamp,
    known_at: Timestamp,
}

/// The shared order-and-leak checks, for one section.
struct SectionCheck<'a> {
    section: &'a str,
    previous_known_at: Option<Timestamp>,
    last_at_per_series: BTreeMap<String, Timestamp>,
    /// The tape's clock origin, which no record outside the bars may
    /// precede. `None` while checking the bars themselves.
    clock_starts: Option<Timestamp>,
}

impl<'a> SectionCheck<'a> {
    fn new(section: &'a str, clock_starts: Option<Timestamp>) -> Self {
        Self {
            section,
            previous_known_at: None,
            last_at_per_series: BTreeMap::new(),
            clock_starts,
        }
    }

    fn place(&mut self, index: usize, series: &str, at: &str, known_at: &str) -> Result<Placement> {
        let section = self.section;
        let at = instant(at, section, index, "at")?;
        let known_at = instant(known_at, section, index, "known_at")?;

        // The leak. A record knowable before it was true is a record the
        // consumer read from the future, and refusing it is the entire
        // reason the tape carries two instants rather than one.
        if known_at < at {
            return Err(Error::invalid(format!(
                "{section} {index} ({series}) is knowable at {} but true only at {}: a record \
                 knowable before it happened is look-ahead, and the tape is refused rather \
                 than read with the leak in it",
                known_at.to_rfc3339(),
                at.to_rfc3339()
            )));
        }
        // The order. Refused, never sorted: a file whose knowable instants
        // run backwards is a file whose account of knowability is not to be
        // trusted, and sorting would hide that.
        if let Some(previous) = self.previous_known_at
            && known_at < previous
        {
            return Err(Error::invalid(format!(
                "{section} {index} is knowable at {}, before {section} {} at {}: the tape is \
                 out of order and is refused rather than sorted",
                known_at.to_rfc3339(),
                index - 1,
                previous.to_rfc3339()
            )));
        }
        self.previous_known_at = Some(known_at);

        // The clock. The bars start it; a release or reading knowable earlier
        // would be a cycle before the tape began, and history the platform
        // loads at its first instant is stamped knowable then.
        if let Some(clock_starts) = self.clock_starts
            && known_at < clock_starts
        {
            return Err(Error::invalid(format!(
                "{section} {index} ({series}) is knowable at {}, before the tape's first bar is \
                 knowable at {}: the tape's clock starts at its first bar, so history already \
                 published by then is stamped knowable at that instant, not earlier",
                known_at.to_rfc3339(),
                clock_starts.to_rfc3339()
            )));
        }

        if let Some(last) = self.last_at_per_series.get(series)
            && at <= *last
        {
            return Err(Error::invalid(format!(
                "{section} {index} for {series} is at {}, not after the series' previous record \
                 at {}: a tape may not restate or reorder a record",
                at.to_rfc3339(),
                last.to_rfc3339()
            )));
        }
        self.last_at_per_series.insert(series.to_string(), at);
        Ok(Placement { at, known_at })
    }
}

impl Tape {
    /// Read a tape from disk. Every refusal names the file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::io(format!("cannot read the tape at {}: {e}", path.display())))?;
        Self::parse(&text)
            .map_err(|e| Error::invalid(format!("the tape at {}: {}", path.display(), e.message())))
    }

    /// Validate a document's text into a tape, or refuse it.
    ///
    /// The refusals, in the order they are checked: an unknown schema, an
    /// empty tape, and then per section — an unparsable instant, a
    /// `known_at` before its `at` (leakage), a `known_at` earlier than the
    /// one before it (out of order), a record knowable before the tape's
    /// first bar (for the two non-bar sections), a record whose own instant
    /// runs backwards for its series, and a record that is not coherent.
    /// None of them is repaired.
    pub fn parse(text: &str) -> Result<Self> {
        let document: TapeDocument = serde_json::from_str(text)
            .map_err(|e| Error::invalid(format!("the tape does not parse: {e}")))?;
        Self::from_document(document)
    }

    /// Validate a decoded document. See [`Self::parse`] for the refusals.
    pub fn from_document(document: TapeDocument) -> Result<Self> {
        if document.schema_version != SCHEMA_VERSION {
            return Err(Error::invalid(format!(
                "tape schema version {} is not the {SCHEMA_VERSION} this build reads",
                document.schema_version
            )));
        }
        if document.observations.is_empty() {
            return Err(Error::invalid(
                "the tape holds no observation; a tape with nothing on it is a wrong path, \
                 not an empty session",
            ));
        }

        let mut entries = Vec::with_capacity(document.observations.len());
        let mut check = SectionCheck::new("observation", None);
        for (index, observation) in document.observations.iter().enumerate() {
            let placed = check.place(
                index,
                &observation.object_id,
                &observation.at,
                &observation.known_at,
            )?;
            let bar = Bar {
                object_id: ObjectId::from_string(observation.object_id.as_str()),
                venue: observation.venue.clone(),
                interval: document.interval,
                open_time: placed.at.saturating_sub(document.interval.duration()),
                open: observation.open,
                high: observation.high,
                low: observation.low,
                close: observation.close,
                volume: observation.volume,
                vwap: None,
                trade_count: 0,
                // A fixture asserts a perfect measurement; it is the licensing
                // class on the descriptor, not the quality, that keeps it out
                // of a production decision.
                quality: DataQuality::default(),
            };
            let issues = SensedRecord::Bar(Box::new(bar.clone())).validate();
            if !issues.is_empty() {
                return Err(Error::invalid(format!(
                    "observation {index} ({}) is not a usable bar: {}",
                    observation.object_id,
                    issues.join("; ")
                )));
            }
            entries.push(TapeEntry {
                at: placed.at,
                known_at: placed.known_at,
                bar,
            });
        }
        // The premise of every non-bar refusal below: the bars are non-empty,
        // so there is a first one.
        let clock_starts = entries.first().map(|entry| entry.known_at);
        let source = format!("tape:{}", document.name);

        let mut releases = Vec::with_capacity(document.macro_releases.len());
        let mut check = SectionCheck::new("release", clock_starts);
        for (index, release) in document.macro_releases.iter().enumerate() {
            let placed = check.place(index, &release.series_id, &release.at, &release.known_at)?;
            let observation = MacroObservation {
                series_id: release.series_id.clone(),
                region: release.region.clone(),
                value: release.value,
                unit: release.unit.clone(),
                reference_date: placed.at,
                consensus: release.consensus,
                previous: None,
                is_revision: false,
                provenance: Provenance::new(source.clone(), placed.at, placed.known_at)
                    .with_licensing(LicensingClass::Synthetic),
                quality: DataQuality::default(),
            };
            let issues = SensedRecord::Macro(Box::new(observation.clone())).validate();
            if !issues.is_empty() {
                return Err(Error::invalid(format!(
                    "release {index} ({}) is not a usable release: {}",
                    release.series_id,
                    issues.join("; ")
                )));
            }
            releases.push(ReleaseEntry {
                at: placed.at,
                known_at: placed.known_at,
                observation,
            });
        }

        let mut readings = Vec::with_capacity(document.alternative_data.len());
        let mut check = SectionCheck::new("reading", clock_starts);
        for (index, reading) in document.alternative_data.iter().enumerate() {
            let series = format!(
                "{}/{}@{}",
                reading.dataset, reading.metric, reading.subject_id
            );
            let placed = check.place(index, &series, &reading.at, &reading.known_at)?;
            let point = AlternativeDataPoint {
                dataset: reading.dataset.clone(),
                subject_id: reading.subject_id.clone(),
                metric: reading.metric.clone(),
                value: reading.value,
                unit: reading.unit.clone(),
                observed_at: placed.at,
                // A fixture has measured no lead and no correlation against
                // any fundamental, and says so with zeros rather than a
                // number that would read as a measurement.
                lead_days: 0.0,
                proxy_correlation: 0.0,
                proxies_for: None,
                provenance: Provenance::new(source.clone(), placed.at, placed.known_at)
                    .with_licensing(LicensingClass::Synthetic),
                quality: DataQuality::default(),
            };
            let issues = SensedRecord::AlternativeData(Box::new(point.clone())).validate();
            if !issues.is_empty() {
                return Err(Error::invalid(format!(
                    "reading {index} ({series}) is not a usable reading: {}",
                    issues.join("; ")
                )));
            }
            readings.push(ReadingEntry {
                at: placed.at,
                known_at: placed.known_at,
                point,
            });
        }

        let mut declarations = Vec::with_capacity(document.dividend_declarations.len());
        let mut check = SectionCheck::new("declaration", clock_starts);
        for (index, declaration) in document.dividend_declarations.iter().enumerate() {
            let placed = check.place(
                index,
                &declaration.object_id,
                &declaration.at,
                &declaration.known_at,
            )?;
            let ex_date = instant(&declaration.ex_date, "declaration", index, "ex_date")?;
            // An entitlement cannot lapse before it was declared. Not a
            // leak in the bitemporal sense — nothing is read early — but a
            // record that contradicts itself, and a fixture is the one
            // place that has no excuse for one.
            if ex_date < placed.at {
                return Err(Error::invalid(format!(
                    "declaration {index} ({}) goes ex at {}, before it was declared at {}: a \
                     dividend cannot lapse before it exists",
                    declaration.object_id,
                    ex_date.to_rfc3339(),
                    placed.at.to_rfc3339()
                )));
            }
            if !declaration.amount.is_positive() {
                return Err(Error::invalid(format!(
                    "declaration {index} ({}) declares {} per share; a dividend is positive or \
                     it is not a dividend",
                    declaration.object_id, declaration.amount
                )));
            }
            let action = CorporateAction {
                object_id: ObjectId::from_string(declaration.object_id.as_str()),
                ex_date,
                record_date: None,
                payment_date: None,
                kind: CorporateActionKind::CashDividend {
                    amount: declaration.amount,
                },
                announced_at: placed.at,
            };
            declarations.push(DeclarationEntry {
                at: placed.at,
                known_at: placed.known_at,
                action,
            });
        }

        Ok(Self {
            name: document.name,
            interval: document.interval,
            entries,
            releases,
            readings,
            declarations,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn interval(&self) -> Interval {
        self.interval
    }

    /// The bars, in tape order.
    pub fn entries(&self) -> &[TapeEntry] {
        &self.entries
    }

    /// The macro releases, in tape order.
    pub fn releases(&self) -> &[ReleaseEntry] {
        &self.releases
    }

    /// The alternative-data readings, in tape order.
    pub fn readings(&self) -> &[ReadingEntry] {
        &self.readings
    }

    /// The dividend declarations, in tape order.
    pub fn declarations(&self) -> &[DeclarationEntry] {
        &self.declarations
    }

    /// How many bars the tape holds. The other sections are counted by
    /// [`Self::releases`], [`Self::readings`] and [`Self::declarations`].
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every record across the four sections.
    pub fn record_count(&self) -> usize {
        self.entries.len() + self.releases.len() + self.readings.len() + self.declarations.len()
    }

    /// The instruments the tape prices, in id order.
    pub fn instruments(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .map(|entry| entry.bar.object_id.as_str().to_string())
            .collect()
    }

    fn known_instants(&self) -> impl Iterator<Item = Timestamp> + '_ {
        self.entries
            .iter()
            .map(|entry| entry.known_at)
            .chain(self.releases.iter().map(|entry| entry.known_at))
            .chain(self.readings.iter().map(|entry| entry.known_at))
            .chain(self.declarations.iter().map(|entry| entry.known_at))
    }

    /// Distinct knowable instants across every section — the number of
    /// cycles a replay takes.
    pub fn periods(&self) -> usize {
        self.known_instants()
            .map(|instant| instant.as_nanos())
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// First and last knowable instants across every section. The first is
    /// the first bar's, by the clock rule every other section is held to.
    pub fn span(&self) -> Option<(Timestamp, Timestamp)> {
        let first = self.known_instants().min()?;
        let last = self.known_instants().max()?;
        Some((first, last))
    }

    /// The closes of one instrument, oldest first — the series the detectors
    /// will read, for a test to assert its structure before asserting what
    /// the platform made of it.
    pub fn closes(&self, object_id: &str) -> Vec<f64> {
        self.entries
            .iter()
            .filter(|entry| entry.bar.object_id.as_str() == object_id)
            .map(|entry| entry.bar.close.to_f64())
            .collect()
    }

    /// The values of one macro series, oldest first, with each one's
    /// knowable instant — for a test to assert what the macro analyst could
    /// read at a given cycle before asserting what it read.
    pub fn series(&self, series_id: &str) -> Vec<(Timestamp, f64)> {
        self.releases
            .iter()
            .filter(|entry| entry.observation.series_id == series_id)
            .map(|entry| (entry.known_at, entry.observation.value))
            .collect()
    }
}

fn instant(text: &str, section: &str, index: usize, field: &str) -> Result<Timestamp> {
    Timestamp::parse_rfc3339(text).ok_or_else(|| {
        Error::invalid(format!(
            "{section} {index} has an unreadable {field} of {text:?}; RFC 3339 is required"
        ))
    })
}

/// A [`DataAdapter`] over a [`Tape`] that owns the clock it is read on.
#[derive(Debug)]
pub struct TapeFeed {
    tape: Tape,
    /// One cursor per section. Each section is in its own `known_at` order;
    /// a poll releases from all four up to the same instant, bars first.
    bars: usize,
    releases: usize,
    readings: usize,
    declarations: usize,
    /// The clock tape time is written to. Shared with whoever builds the
    /// platform, so the cycle's `now` and the platform's own clock agree.
    clock: Arc<ManualClock>,
}

impl TapeFeed {
    /// Wrap a tape. The clock starts at the first knowable instant so a
    /// platform assembled on it is assembled at tape time, not at 1970.
    pub fn new(tape: Tape) -> Self {
        let start = tape
            .span()
            .map_or(Timestamp::from_secs(0), |(first, _)| first);
        Self {
            tape,
            bars: 0,
            releases: 0,
            readings: 0,
            declarations: 0,
            clock: Arc::new(ManualClock::new(start)),
        }
    }

    pub fn tape(&self) -> &Tape {
        &self.tape
    }

    /// The clock tape time is written to. Build the platform's `Context` on
    /// this, or the platform will reason at one time and observe at another.
    pub fn clock(&self) -> Arc<ManualClock> {
        Arc::clone(&self.clock)
    }

    /// Records not yet released, across every section.
    pub fn remaining(&self) -> usize {
        self.tape.entries.len().saturating_sub(self.bars)
            + self.tape.releases.len().saturating_sub(self.releases)
            + self.tape.readings.len().saturating_sub(self.readings)
            + self
                .tape
                .declarations
                .len()
                .saturating_sub(self.declarations)
    }

    /// The knowable instant of the next unreleased record in any section.
    pub fn next_known_at(&self) -> Option<Timestamp> {
        [
            self.tape.entries.get(self.bars).map(|entry| entry.known_at),
            self.tape
                .releases
                .get(self.releases)
                .map(|entry| entry.known_at),
            self.tape
                .readings
                .get(self.readings)
                .map(|entry| entry.known_at),
            self.tape
                .declarations
                .get(self.declarations)
                .map(|entry| entry.known_at),
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

impl DataAdapter for TapeFeed {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            name: format!("tape:{}", self.tape.name),
            provider: "a committed synthetic tape replayed on its own clock".to_string(),
            // Not configurable. A tape is generated data whatever its author
            // says, and the class is what keeps it out of a capital decision.
            licensing: LicensingClass::Synthetic,
            topics: vec![
                Topic::MarketBar,
                Topic::MarketCorporateAction,
                Topic::MacroUpdated,
                Topic::AlternativeDataReceived,
            ],
            expected_latency: Duration::ZERO,
            production_requirement: Some(
                "a licensed market-data feed; a tape demonstrates the loop and prices nothing \
                 real"
                    .to_string(),
            ),
        }
    }

    /// Everything knowable by `until`, bars first and then each other
    /// section, each in tape order. Released by `known_at`: a bar whose close
    /// is in the past but whose publication is not stays on the tape, and so
    /// does a release whose reference period has ended but whose print has
    /// not landed.
    fn poll(&mut self, until: Timestamp) -> Result<Vec<SensedRecord>> {
        let mut out = Vec::new();
        while let Some(entry) = self.tape.entries.get(self.bars) {
            if entry.known_at > until {
                break;
            }
            out.push(SensedRecord::Bar(Box::new(entry.bar.clone())));
            self.bars += 1;
        }
        while let Some(entry) = self.tape.declarations.get(self.declarations) {
            if entry.known_at > until {
                break;
            }
            out.push(SensedRecord::CorporateAction(Box::new(
                entry.action.clone(),
            )));
            self.declarations += 1;
        }
        while let Some(entry) = self.tape.releases.get(self.releases) {
            if entry.known_at > until {
                break;
            }
            out.push(SensedRecord::Macro(Box::new(entry.observation.clone())));
            self.releases += 1;
        }
        while let Some(entry) = self.tape.readings.get(self.readings) {
            if entry.known_at > until {
                break;
            }
            out.push(SensedRecord::AlternativeData(Box::new(entry.point.clone())));
            self.readings += 1;
        }
        Ok(out)
    }

    /// Move the clock to the next knowable instant in any section and return
    /// it, or `None` once the tape is spent. The clock never moves backwards,
    /// which [`Tape::parse`]'s order and clock checks already guarantee the
    /// tape cannot ask.
    fn advance(&mut self) -> Option<Timestamp> {
        let next = self.next_known_at()?;
        self.clock.set(next);
        Some(next)
    }

    fn owns_time(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::Clock;

    fn observation(
        object: &str,
        at: &str,
        known_at: &str,
        open: &str,
        close: &str,
    ) -> TapeObservation {
        let open_d = Decimal::parse(open).expect("a decimal literal");
        let close_d = Decimal::parse(close).expect("a decimal literal");
        TapeObservation {
            object_id: object.to_string(),
            venue: "XNYS".to_string(),
            at: at.to_string(),
            known_at: known_at.to_string(),
            open: open_d,
            high: open_d.max(close_d),
            low: open_d.min(close_d),
            close: close_d,
            volume: Decimal::from_int(1_000),
        }
    }

    fn release(series: &str, at: &str, known_at: &str, value: f64) -> TapeRelease {
        TapeRelease {
            series_id: series.to_string(),
            region: "US".to_string(),
            at: at.to_string(),
            known_at: known_at.to_string(),
            value,
            unit: "percent".to_string(),
            consensus: None,
        }
    }

    fn reading(at: &str, known_at: &str, value: f64) -> TapeReading {
        TapeReading {
            dataset: "web-traffic".to_string(),
            metric: "web_traffic_index".to_string(),
            subject_id: "OBJ-A".to_string(),
            at: at.to_string(),
            known_at: known_at.to_string(),
            value,
            unit: "index".to_string(),
        }
    }

    fn document(observations: Vec<TapeObservation>) -> TapeDocument {
        TapeDocument {
            schema_version: SCHEMA_VERSION,
            name: "unit".to_string(),
            description: "a unit-test tape".to_string(),
            interval: Interval::Day,
            observations,
            macro_releases: Vec::new(),
            alternative_data: Vec::new(),
            dividend_declarations: Vec::new(),
        }
    }

    fn declaration(at: &str, known_at: &str, ex_date: &str, amount: &str) -> TapeDeclaration {
        TapeDeclaration {
            object_id: "OBJ-A".to_string(),
            at: at.to_string(),
            known_at: known_at.to_string(),
            ex_date: ex_date.to_string(),
            amount: Decimal::parse(amount).expect("a decimal literal"),
        }
    }

    fn two_days() -> Vec<TapeObservation> {
        vec![
            observation(
                "OBJ-A",
                "2025-01-06T21:00:00Z",
                "2025-01-06T21:15:00Z",
                "100",
                "101",
            ),
            observation(
                "OBJ-A",
                "2025-01-07T21:00:00Z",
                "2025-01-07T21:15:00Z",
                "101",
                "102",
            ),
        ]
    }

    #[test]
    fn a_bar_knowable_before_it_closed_is_refused_as_look_ahead() {
        // The leak this tape format exists to refuse. A backtest fed a bar
        // fifteen minutes before it closed would read tomorrow's close today,
        // and the number it produced would look excellent.
        let mut observations = two_days();
        observations[1].known_at = "2025-01-07T20:00:00Z".to_string();
        let refusal = Tape::from_document(document(observations))
            .expect_err("a known_at before its at was admitted");
        assert!(
            refusal.message().contains("look-ahead"),
            "the refusal does not name the leak: {}",
            refusal.message()
        );
        // And the premise: the same tape with the instants in order is fine.
        Tape::from_document(document(two_days())).expect("an ordered tape loads");
    }

    #[test]
    fn a_tape_whose_knowable_instants_run_backwards_is_refused_not_sorted() {
        let mut observations = two_days();
        observations.swap(0, 1);
        let refusal = Tape::from_document(document(observations))
            .expect_err("an out-of-order tape was admitted");
        assert!(
            refusal.message().contains("out of order"),
            "the refusal does not say the tape is out of order: {}",
            refusal.message()
        );
        assert!(
            refusal.message().contains("observation 1"),
            "the refusal does not name the position: {}",
            refusal.message()
        );
    }

    #[test]
    fn an_empty_tape_and_an_unknown_schema_are_refused() {
        let empty = Tape::from_document(document(Vec::new())).expect_err("an empty tape opened");
        assert!(empty.message().contains("no observation"));

        let mut wrong = document(two_days());
        wrong.schema_version = SCHEMA_VERSION + 1;
        let refusal = Tape::from_document(wrong).expect_err("an unknown schema was read");
        assert!(refusal.message().contains("schema version"));
    }

    #[test]
    fn an_incoherent_bar_is_refused_by_position() {
        let mut observations = two_days();
        observations[0].high = Decimal::parse("90").expect("a literal");
        let refusal = Tape::from_document(document(observations))
            .expect_err("a bar whose high is below its close was admitted");
        assert!(
            refusal.message().contains("observation 0"),
            "the refusal does not name the position: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_release_and_a_reading_are_held_to_the_same_leak_and_order_refusals_as_a_bar() {
        // The same three refusals, on the two new sections, each with the
        // admitting premise beside it: a check that only refuses proves
        // nothing.
        let mut good = document(two_days());
        good.macro_releases = vec![
            release(
                "US.POLICY_RATE",
                "2024-12-01T00:00:00Z",
                "2025-01-06T21:15:00Z",
                4.5,
            ),
            release(
                "US.POLICY_RATE",
                "2025-01-01T00:00:00Z",
                "2025-01-07T21:15:00Z",
                4.75,
            ),
        ];
        good.alternative_data = vec![
            reading("2025-01-06T00:00:00Z", "2025-01-06T21:15:00Z", 100.0),
            reading("2025-01-07T00:00:00Z", "2025-01-07T06:15:00Z", 98.0),
        ];
        let tape = Tape::from_document(good.clone()).expect("the premise: both sections load");
        assert_eq!(tape.releases().len(), 2);
        assert_eq!(tape.readings().len(), 2);
        assert_eq!(tape.record_count(), 6);
        // The reading at 06:15 is its own period, between the two bars.
        assert_eq!(tape.periods(), 3);

        // The leak, on a release: published before its reference period.
        let mut leak = good.clone();
        leak.macro_releases[1].known_at = "2024-12-31T00:00:00Z".to_string();
        let refusal = Tape::from_document(leak).expect_err("a leaking release was admitted");
        assert!(
            refusal.message().contains("look-ahead") && refusal.message().contains("release 1"),
            "{}",
            refusal.message()
        );

        // The order, on a reading.
        let mut disorder = good.clone();
        disorder.alternative_data.swap(0, 1);
        let refusal = Tape::from_document(disorder).expect_err("a disordered reading was admitted");
        assert!(
            refusal.message().contains("out of order") && refusal.message().contains("reading 1"),
            "{}",
            refusal.message()
        );

        // The restatement, on a release: the same reference period twice.
        let mut restated = good.clone();
        restated.macro_releases[1].at = restated.macro_releases[0].at.clone();
        let refusal = Tape::from_document(restated).expect_err("a restated release was admitted");
        assert!(
            refusal.message().contains("restate") && refusal.message().contains("release 1"),
            "{}",
            refusal.message()
        );

        // A non-finite value is not a usable release.
        let mut broken = good;
        broken.macro_releases[0].value = f64::NAN;
        let refusal = Tape::from_document(broken).expect_err("a NaN release was admitted");
        assert!(
            refusal.message().contains("release 0"),
            "{}",
            refusal.message()
        );
    }

    #[test]
    fn a_declaration_is_released_as_a_corporate_action_and_a_self_contradicting_one_is_refused() {
        let mut good = document(two_days());
        good.dividend_declarations = vec![declaration(
            "2025-01-06T22:00:00Z",
            "2025-01-06T22:15:00Z",
            "2025-01-20T00:00:00Z",
            "0.45",
        )];
        let tape = Tape::from_document(good.clone()).expect("the premise: it loads");
        assert_eq!(tape.declarations().len(), 1);
        assert_eq!(tape.periods(), 3, "the declaration is its own period");
        let mut feed = TapeFeed::new(tape);
        let _ = feed.advance();
        let _ = feed.poll(Timestamp::parse_rfc3339("2025-01-06T21:15:00Z").expect("an instant"));
        let second = feed.advance().expect("the declaration's period");
        let released = feed.poll(second).expect("polls");
        assert_eq!(released.len(), 1);
        assert!(
            matches!(&released[0], SensedRecord::CorporateAction(action)
                if matches!(action.kind, CorporateActionKind::CashDividend { .. })),
            "the declaration did not release as a cash dividend: {released:?}"
        );

        // Ex before declared: the record contradicts itself.
        let mut backwards = good.clone();
        backwards.dividend_declarations[0].ex_date = "2025-01-01T00:00:00Z".to_string();
        let refusal =
            Tape::from_document(backwards).expect_err("an ex-date before the declaration loaded");
        assert!(
            refusal.message().contains("before it was declared"),
            "{}",
            refusal.message()
        );
        // And the shared refusals apply: knowable before announced is a leak.
        let mut leak = good;
        leak.dividend_declarations[0].known_at = "2025-01-06T21:30:00Z".to_string();
        let refusal = Tape::from_document(leak).expect_err("a leaking declaration loaded");
        assert!(
            refusal.message().contains("look-ahead") && refusal.message().contains("declaration 0"),
            "{}",
            refusal.message()
        );
    }

    #[test]
    fn a_release_knowable_before_the_first_bar_is_refused_because_the_bars_own_the_clock() {
        // History a platform loads at start-up is stamped knowable at the
        // tape's first instant. Stamped earlier, the feed would advance the
        // clock to it and run a cycle before the tape began — and the roster,
        // reviewed at that first cycle, would lapse before the bars arrived.
        let mut early = document(two_days());
        early.macro_releases = vec![release(
            "US.POLICY_RATE",
            "2024-12-01T00:00:00Z",
            "2024-12-15T13:30:00Z",
            4.5,
        )];
        let refusal = Tape::from_document(early).expect_err("a pre-clock release was admitted");
        assert!(
            refusal.message().contains("first bar") && refusal.message().contains("release 0"),
            "{}",
            refusal.message()
        );
        // The premise on the other side: stamped at the first bar's instant,
        // the same history loads.
        let mut stamped = document(two_days());
        stamped.macro_releases = vec![release(
            "US.POLICY_RATE",
            "2024-12-01T00:00:00Z",
            "2025-01-06T21:15:00Z",
            4.5,
        )];
        Tape::from_document(stamped).expect("history stamped at the clock's start loads");
    }

    #[test]
    fn the_feed_releases_by_known_at_and_moves_its_clock_one_period_at_a_time() {
        // The property the tape adds over a replay: a poll at the first
        // bar's close, before its publication, releases nothing; advancing
        // moves the clock to the publication instant and the poll releases
        // exactly that bar.
        let tape = Tape::from_document(document(two_days())).expect("loads");
        assert_eq!(tape.periods(), 2, "the premise: two knowable instants");
        let mut feed = TapeFeed::new(tape);
        let clock = feed.clock();

        let at_close = Timestamp::parse_rfc3339("2025-01-06T21:00:00Z").expect("an instant");
        assert!(
            feed.poll(at_close).expect("polls").is_empty(),
            "a bar was released at its close, before it was knowable"
        );

        let first = feed.advance().expect("the tape has a first period");
        assert_eq!(
            clock.now(),
            first,
            "advancing did not move the shared clock"
        );
        let released = feed.poll(first).expect("polls");
        assert_eq!(released.len(), 1, "one period releases one bar here");
        assert_eq!(feed.remaining(), 1);

        let second = feed.advance().expect("a second period");
        assert!(second > first, "tape time did not move forward");
        // Asserted on the clock and not only on the returned instant: the
        // clock starts at the first knowable instant, so the first check
        // above holds even when `advance` never writes to it, and a mutation
        // that dropped the write survived until this line existed.
        assert_eq!(
            clock.now(),
            second,
            "the shared clock did not follow the second period"
        );
        assert_eq!(feed.poll(second).expect("polls").len(), 1);
        assert_eq!(feed.remaining(), 0);
        assert!(
            feed.advance().is_none(),
            "a spent tape still claims a next period"
        );
    }

    #[test]
    fn the_feed_releases_every_section_by_its_own_known_at_and_a_release_can_be_its_own_period() {
        // A release published between two bars is a period of its own, and
        // a reading whose observation is in the past but whose publication
        // is not stays on the tape at the earlier period.
        let mut doc = document(two_days());
        doc.macro_releases = vec![
            release(
                "US.POLICY_RATE",
                "2024-12-01T00:00:00Z",
                "2025-01-06T21:15:00Z",
                4.5,
            ),
            release(
                "US.POLICY_RATE",
                "2025-01-01T00:00:00Z",
                "2025-01-07T13:15:00Z",
                4.75,
            ),
        ];
        doc.alternative_data = vec![reading(
            "2025-01-07T00:00:00Z",
            "2025-01-07T21:15:00Z",
            98.0,
        )];
        let tape = Tape::from_document(doc).expect("loads");
        assert_eq!(tape.periods(), 3, "the premise: the release adds a period");
        let mut feed = TapeFeed::new(tape);

        let first = feed.advance().expect("first period");
        let released = feed.poll(first).expect("polls");
        assert_eq!(released.len(), 2, "the first bar and the history release");
        assert!(matches!(released[0], SensedRecord::Bar(_)));
        assert!(matches!(released[1], SensedRecord::Macro(_)));
        // The reading was observed before this instant and is not released:
        // it is not yet published.
        assert!(
            !released
                .iter()
                .any(|r| matches!(r, SensedRecord::AlternativeData(_))),
            "a reading was released by its observation instant rather than its publication"
        );

        let second = feed.advance().expect("second period");
        assert_eq!(
            second,
            Timestamp::parse_rfc3339("2025-01-07T13:15:00Z").expect("an instant"),
            "the release did not become its own period"
        );
        let released = feed.poll(second).expect("polls");
        assert_eq!(released.len(), 1);
        assert!(matches!(released[0], SensedRecord::Macro(_)));

        let third = feed.advance().expect("third period");
        let released = feed.poll(third).expect("polls");
        assert_eq!(released.len(), 2, "the second bar and the reading");
        assert!(matches!(released[1], SensedRecord::AlternativeData(_)));
        assert_eq!(feed.remaining(), 0);
        assert!(feed.advance().is_none());
    }

    /// The committed demonstration tape, regenerated.
    ///
    /// Four of the catalogue's instruments over 600 hourly periods, so a
    /// replay sizes into the same exposure buckets a deployment does. Hourly
    /// and not daily because the roster's manifests are reviewed at assembly
    /// and lapse ninety days later at tape time: a 320-day tape convened its
    /// first panel on day 103 and every agent was refused from then on.
    /// Six hundred hours is twenty-five days, inside the window, and long
    /// enough that the claim recorded on the NWSC jump at period 100
    /// resolves on the tape: an opportunity takes the longest horizon of
    /// every anomaly on it, and once the jump has a catalyst that horizon
    /// is the catalyst detector's twenty days, resolving at period 580. The
    /// tape was 320 periods while the jump was a bare price move with a
    /// five-day horizon. Two structures are planted in the bars, each aimed
    /// at one detector and each small enough that the controls stay
    /// controls:
    ///
    /// * `NWSC` jumps +1.5% on period 100, which with that period's own
    ///   noise prints as about +2.4%: one outlier in an otherwise ordinary
    ///   series, for the return-anomaly detector — about 3.6 robust sigma
    ///   against its 3.0 threshold, the robust scale being a MAD of roughly
    ///   0.67%. No larger, because an opportunity takes the longest horizon
    ///   of every anomaly on it, and a jump near 3% also trips the
    ///   volatility-shift detector, whose twenty-day horizon outruns the
    ///   tape — a +2.84% print did exactly that. At +2.4% the volatility
    ///   ratio's z stays near 2.2, under that detector's 2.5 bar, so the
    ///   price-move claim alone is recorded, with its five-day horizon, and
    ///   is the one that resolves on tape.
    /// * `MRDN` drifts +0.6% a period over periods 180–239 against noise of
    ///   ±0.9%. That is the persistent shift the CUSUM structural-break
    ///   detector exists to find and the return-anomaly detector exists to
    ///   ignore — no single period is an outlier. Its ninety-day horizon
    ///   does not resolve on a thirteen-day tape, and the file says so.
    ///
    /// `VNTG` and `ATFB` carry noise only, so a detector that fires on them
    /// is firing on nothing.
    ///
    /// Two more structures are planted in the other sections, both placed
    /// so the panel convened on the NWSC jump can read them, and both
    /// leaning the way that jump's claim leans — the jump is positive, so
    /// the claim the platform forms is that NWSC is overvalued and reverts:
    ///
    /// * Four US macro series — the policy rate, inflation, growth and the
    ///   aggregate credit spread — with thirty-six monthly prints of history
    ///   stamped knowable at the tape's first instant, and a December print
    ///   published at period 88 that is hawkish on every series by about
    ///   2.5 sigma of its own history: rate and inflation up, growth down,
    ///   spreads wider. The macro analyst needs thirty observations before
    ///   its standardisation means anything, which is why the history is
    ///   there; the December print is what it reads as a view.
    /// * Daily `web_traffic_index` readings for `NWSC` from the
    ///   `web-traffic` dataset, forty-five days of history at the tape's
    ///   first instant and one per tape day published at 06:15, with the
    ///   reading observed on 10 January — the day before the jump —
    ///   collapsing to 70 against a level near 100. A licence for that
    ///   dataset is what the alternative-data analyst needs before it reads
    ///   the series; the platform's default licenses nothing, and the tape
    ///   cannot grant one.
    ///
    /// Every non-bar `known_at` sits on a bar's knowable instant, so the
    /// tape still runs in exactly 320 periods. The noise is a fixed
    /// irrational rotation rather than an RNG, so the file reproduces from
    /// this function byte for byte.
    fn demonstration_document() -> TapeDocument {
        const INSTRUMENTS: [(&str, f64); 4] = [
            ("OBJ00000000000000000NWSC", 142.50),
            ("OBJ00000000000000000VNTG", 318.75),
            ("OBJ00000000000000000MRDN", 88.20),
            ("OBJ00000000000000000ATFB", 54.10),
        ];
        const PERIODS: usize = 600;
        /// Whole tape days, for the one-reading-per-day section.
        const TAPE_DAYS: i64 = PERIODS as i64 / 24;
        /// Series code, level, wobble amplitude, the December print's
        /// displacement in sigmas of that wobble, and the unit. The codes
        /// are the world model's vocabulary in the vendor's spelling; the
        /// fast brain's tape test holds them to it, because this crate does
        /// not depend on that one.
        const MACRO: [(&str, f64, f64, f64, &str); 4] = [
            ("POLICY_RATE", 4.50, 0.20, 2.5, "percent"),
            ("INFLATION_YOY", 2.80, 0.30, 2.5, "percent"),
            ("GROWTH_YOY", 2.10, 0.40, -2.5, "percent"),
            ("CREDIT_SPREAD_BPS", 110.0, 8.0, 2.5, "bps"),
        ];
        const ECONOMY: &str = "US";
        const HISTORY_MONTHS: u32 = 36;
        const TRAFFIC_HISTORY_DAYS: i64 = 45;
        let first_close = Timestamp::parse_rfc3339("2025-01-06T21:00:00Z").expect("an instant");
        let publication_delay = Duration::from_mins(15);
        let clock_starts = first_close.saturating_add(publication_delay);

        let cents = |value: f64| -> Decimal {
            Decimal::parse(&format!("{value:.2}")).expect("a two-decimal price parses")
        };
        let round4 = |value: f64| -> f64 { (value * 10_000.0).round() / 10_000.0 };

        let mut prices: Vec<f64> = INSTRUMENTS.iter().map(|(_, price)| *price).collect();
        let mut observations = Vec::with_capacity(PERIODS * INSTRUMENTS.len());
        for period in 0..PERIODS {
            let at = first_close.saturating_add(Duration::from_hours(period as i64));
            let known_at = at.saturating_add(publication_delay);
            for (index, (object_id, _)) in INSTRUMENTS.iter().enumerate() {
                let phase = period as f64 * 0.754_877_666_2 + index as f64 * 0.381_966_011_3 + 0.5;
                let noise = (phase % 1.0 - 0.5) * 0.018;
                let planted = match (index, period) {
                    (2, 180..=239) => 0.006,
                    (0, 100) => 0.015,
                    _ => 0.0,
                };
                let open = prices[index];
                let close = open * (1.0 + noise + planted);
                prices[index] = close;
                let volume = 900_000 + ((period * 7 + index * 13) % 50) * 10_000;
                observations.push(TapeObservation {
                    object_id: (*object_id).to_string(),
                    venue: "XNYS".to_string(),
                    at: at.to_rfc3339(),
                    known_at: known_at.to_rfc3339(),
                    open: cents(open),
                    high: cents(open.max(close) * 1.003),
                    low: cents(open.min(close) * 0.997),
                    close: cents(close),
                    volume: Decimal::from_int(volume as i64),
                });
            }
        }

        // Monthly reference periods: the first of each month from December
        // 2021 to November 2024 are history; December 2024 prints on tape
        // at period 88 — 2025-01-10T13:15Z, a bar's knowable instant.
        let month_start = |offset: u32| -> Timestamp {
            let months = 12 * 2021 + 11 + offset; // December 2021 is offset 0
            Timestamp::from_civil((months / 12) as i32, months % 12 + 1, 1)
        };
        let december_print = first_close
            .saturating_add(Duration::from_hours(88))
            .saturating_add(publication_delay);
        let mut macro_releases = Vec::with_capacity((HISTORY_MONTHS as usize + 1) * MACRO.len());
        for (series_index, (code, level, amplitude, _, unit)) in MACRO.iter().enumerate() {
            for offset in 0..HISTORY_MONTHS {
                let phase =
                    f64::from(offset) * 0.754_877_666_2 + series_index as f64 * 0.381_966_011_3;
                let value = level + (phase % 1.0 - 0.5) * amplitude;
                macro_releases.push(TapeRelease {
                    series_id: format!("{ECONOMY}.{code}"),
                    region: ECONOMY.to_string(),
                    at: month_start(offset).to_rfc3339(),
                    known_at: clock_starts.to_rfc3339(),
                    value: round4(value),
                    unit: (*unit).to_string(),
                    consensus: None,
                });
            }
        }
        for (code, level, amplitude, december_sigma, unit) in MACRO {
            // The wobble is uniform on ±amplitude/2, whose standard deviation
            // is amplitude/sqrt(12); the print sits that many sigmas out.
            let value = level + december_sigma * amplitude / 12_f64.sqrt();
            macro_releases.push(TapeRelease {
                series_id: format!("{ECONOMY}.{code}"),
                region: ECONOMY.to_string(),
                at: month_start(HISTORY_MONTHS).to_rfc3339(),
                known_at: december_print.to_rfc3339(),
                value: round4(value),
                unit: unit.to_string(),
                consensus: Some(level),
            });
        }

        // Daily web traffic for NWSC: history observed at midnight and
        // stamped knowable at the clock's start, then one reading per tape
        // day published at 06:15 — a bar's knowable instant — with the
        // 10 January reading collapsed.
        let first_tape_day = Timestamp::parse_rfc3339("2025-01-07T00:00:00Z").expect("an instant");
        let collapsed_day = Timestamp::parse_rfc3339("2025-01-10T00:00:00Z").expect("an instant");
        let publication = Duration::from_hours(6) + publication_delay;
        let mut alternative_data = Vec::new();
        for day in -TRAFFIC_HISTORY_DAYS..TAPE_DAYS {
            let observed = first_tape_day.saturating_add(Duration::from_days(day));
            let known_at = if day < 0 {
                clock_starts
            } else {
                observed.saturating_add(publication)
            };
            let phase = (day + TRAFFIC_HISTORY_DAYS) as f64 * 0.754_877_666_2 + 0.25;
            let value = if observed == collapsed_day {
                70.0
            } else {
                100.0 + (phase % 1.0 - 0.5) * 6.0
            };
            alternative_data.push(TapeReading {
                dataset: "web-traffic".to_string(),
                metric: "web_traffic_index".to_string(),
                subject_id: INSTRUMENTS[0].0.to_string(),
                at: observed.to_rfc3339(),
                known_at: known_at.to_rfc3339(),
                value: round4(value),
                unit: "index".to_string(),
            });
        }

        // The jump's catalyst: a dividend declared at period 70, thirty hours
        // before the period-100 bar, knowable when announced. Without it the
        // catalyst detector — watching the stream now that the tape carries
        // intelligence at all — calls the jump unexplained, and the platform
        // forms no hypothesis about an unexplained move by design. Thirty
        // hours and not the evening before because the kernel hands the
        // detector no bar interval, so the detector measures the move
        // against its one-day default: an event inside that day is part of
        // the move, not its explanation, and one up to three days before it
        // is. A declaration at period 97 was tried first and explained
        // nothing.
        let declared = first_close
            .saturating_add(Duration::from_hours(70))
            .saturating_add(publication_delay);
        let dividend_declarations = vec![TapeDeclaration {
            object_id: INSTRUMENTS[0].0.to_string(),
            at: declared.to_rfc3339(),
            known_at: declared.to_rfc3339(),
            ex_date: Timestamp::parse_rfc3339("2025-01-24T00:00:00Z")
                .expect("an instant")
                .to_rfc3339(),
            amount: cents(0.45),
        }];

        TapeDocument {
            schema_version: SCHEMA_VERSION,
            name: "loop-demonstration".to_string(),
            description: "A synthetic fixture for demonstrating that the decision loop runs end \
                          to end on data with a detectable structure in it. Not market data: \
                          every value is generated by qip-market-ingestion's tape tests from a \
                          fixed rotation, so there is no source, no licence and no question of \
                          either. Hourly, 600 periods, so it ends inside the roster's 90-day \
                          review window. NWSC jumps +1.5% on period 100 (its twenty-day claim \
                          resolves on tape at period 580), thirty hours after a dividend \
                          declaration at period 70 that the catalyst detector reads as the \
                          jump's explanation; MRDN drifts +0.6% a period over periods 180-239 \
                          (its ninety-day claim does not resolve on tape); VNTG and ATFB are \
                          noise. Four US macro series carry 36 months of history knowable at \
                          the tape's first instant and a hawkish December print at period 88; \
                          NWSC's web_traffic_index carries 45 days of history and a daily \
                          reading, the 10 January one collapsed. Regenerate with `cargo test \
                          -p qip-market-ingestion demonstration_tape`, which fails naming the \
                          expected file when this one drifts."
                .to_string(),
            interval: Interval::Hour,
            observations,
            macro_releases,
            alternative_data,
            dividend_declarations,
        }
    }

    /// The document as the file is written: a readable header, one record
    /// per line under each section, so a diff of the fixture is a diff of
    /// records.
    fn render(document: &TapeDocument) -> String {
        fn section<T: Serialize>(text: &mut String, name: &str, records: &[T], last: bool) {
            text.push_str(&format!("  \"{name}\": [\n"));
            for (index, record) in records.iter().enumerate() {
                text.push_str("    ");
                text.push_str(&serde_json::to_string(record).expect("a record serialises"));
                if index + 1 < records.len() {
                    text.push(',');
                }
                text.push('\n');
            }
            text.push_str(if last { "  ]\n" } else { "  ],\n" });
        }
        let mut text = String::new();
        text.push_str("{\n");
        text.push_str(&format!(
            "  \"schema_version\": {},\n  \"name\": {},\n  \"description\": {},\n  \"interval\": {},\n",
            document.schema_version,
            serde_json::to_string(&document.name).expect("a string serialises"),
            serde_json::to_string(&document.description).expect("a string serialises"),
            serde_json::to_string(&document.interval).expect("an interval serialises"),
        ));
        section(&mut text, "observations", &document.observations, false);
        section(&mut text, "macro_releases", &document.macro_releases, false);
        section(
            &mut text,
            "alternative_data",
            &document.alternative_data,
            false,
        );
        section(
            &mut text,
            "dividend_declarations",
            &document.dividend_declarations,
            true,
        );
        text.push_str("}\n");
        text
    }

    #[test]
    fn the_committed_demonstration_tape_is_the_generator_output_and_loads() {
        // The fixture is data the repository asserts about itself. If a hand
        // edit changed a price, this is what says so; if the generator
        // changed, this is what says the file has to follow.
        let expected = render(&demonstration_document());
        let committed_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../data/datasets/loop-demonstration-tape.json");
        let committed = std::fs::read_to_string(&committed_path).unwrap_or_default();
        if committed != expected {
            let target = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../target/loop-demonstration-tape.expected.json");
            std::fs::write(&target, &expected).expect("the expected tape is written");
            panic!(
                "{} does not match its generator; the expected text was written to {}",
                committed_path.display(),
                target.display()
            );
        }

        // And it loads under every refusal above — the premise every other
        // reader of the file rests on.
        let tape = Tape::parse(&committed).expect("the committed tape loads");
        assert_eq!(
            tape.periods(),
            600,
            "a non-bar record sits off a bar's instant"
        );
        assert_eq!(tape.instruments().len(), 4);
        assert_eq!(tape.len(), 2_400);
        assert_eq!(tape.releases().len(), 4 * 37);
        assert_eq!(tape.readings().len(), 45 + 25);
        // The catalyst: declared and knowable at period 70, before the
        // one-day move window that ends at the jump's bar, and inside the
        // detector's three-day explanation window.
        let declaration = tape.declarations().first().expect("one declaration");
        assert_eq!(
            declaration.known_at,
            Timestamp::parse_rfc3339("2025-01-09T19:15:00Z").expect("an instant")
        );
        assert_eq!(
            declaration.action.object_id.as_str(),
            "OBJ00000000000000000NWSC"
        );
        // The December print is knowable at period 88 and after thirty-six
        // months of history, which is what the macro analyst needs.
        let policy = tape.series("US.POLICY_RATE");
        assert_eq!(policy.len(), 37);
        let (printed_at, printed) = policy[36];
        assert_eq!(
            printed_at,
            Timestamp::parse_rfc3339("2025-01-10T13:15:00Z").expect("an instant")
        );
        let history: Vec<f64> = policy[..36].iter().map(|(_, v)| *v).collect();
        let mean = history.iter().sum::<f64>() / 36.0;
        let sigma = (history.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / 35.0).sqrt();
        let z = (printed - mean) / sigma;
        assert!(
            z > 2.0,
            "the December policy-rate print is {z:.2} sigma, not hawkish"
        );
        // The collapsed reading precedes the jump.
        let collapsed = tape
            .readings()
            .iter()
            .find(|r| r.point.value < 80.0)
            .expect("a collapsed reading");
        assert_eq!(
            collapsed.known_at,
            Timestamp::parse_rfc3339("2025-01-10T06:15:00Z").expect("an instant")
        );
    }

    #[test]
    fn a_tape_is_synthetic_and_may_not_drive_a_capital_decision() {
        let feed = TapeFeed::new(Tape::from_document(document(two_days())).expect("loads"));
        let descriptor = feed.descriptor();
        assert!(!descriptor.is_production_grade());
        assert!(descriptor.production_requirement.is_some());
    }
}
