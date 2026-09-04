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
//! * **Every observation carries two instants.** `at` is when the fact was
//!   true in the world — the bar's close — and `known_at` is when it became
//!   knowable to a consumer. [`TapeFeed::poll`] releases by `known_at`, never
//!   by `at`, and [`Tape::parse`] refuses a tape in which any `known_at`
//!   precedes its `at`: a bar knowable before it closed is look-ahead, and a
//!   backtest that reads it is measuring the leak, however good the number.
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
//! What this is not: market data. A tape here is a synthetic fixture for
//! demonstrating that the loop runs end to end on data with a detectable
//! structure in it. Its descriptor says [`LicensingClass::Synthetic`], which
//! the object model bars from any production decision, and nothing here can
//! change that class.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ManualClock, ObjectId, Timestamp};
use qip_events::Topic;
use qip_financial::quality::{DataQuality, LicensingClass};
use qip_market::bar::{Bar, Interval};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use crate::adapter::{DataAdapter, SensedRecord, SourceDescriptor};

/// The one document layout this module reads. Bumped when a field changes
/// meaning, so an old tape is refused rather than misread.
pub const SCHEMA_VERSION: u32 = 1;

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
}

/// One observation after validation: the bar and its two instants.
#[derive(Clone, Debug, PartialEq)]
pub struct TapeEntry {
    pub at: Timestamp,
    pub known_at: Timestamp,
    pub bar: Bar,
}

/// A validated tape, in the order it was written.
#[derive(Clone, Debug, PartialEq)]
pub struct Tape {
    name: String,
    interval: Interval,
    entries: Vec<TapeEntry>,
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
    /// empty tape, an unparsable instant, a `known_at` before its `at`
    /// (leakage), a `known_at` earlier than the one before it (out of order),
    /// a bar whose own instant runs backwards for its instrument, and a bar
    /// that is not coherent. None of them is repaired.
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
        let mut previous_known_at: Option<Timestamp> = None;
        let mut last_at_per_instrument: std::collections::BTreeMap<String, Timestamp> =
            std::collections::BTreeMap::new();
        for (index, observation) in document.observations.iter().enumerate() {
            let at = instant(&observation.at, index, "at")?;
            let known_at = instant(&observation.known_at, index, "known_at")?;

            // The leak. A bar knowable before it closed is a bar the consumer
            // read from the future, and refusing it is the entire reason the
            // tape carries two instants rather than one.
            if known_at < at {
                return Err(Error::invalid(format!(
                    "observation {index} ({}) is knowable at {} but true only at {}: a bar \
                     knowable before it closed is look-ahead, and the tape is refused rather \
                     than read with the leak in it",
                    observation.object_id,
                    known_at.to_rfc3339(),
                    at.to_rfc3339()
                )));
            }
            // The order. Refused, never sorted: a file whose knowable instants
            // run backwards is a file whose account of knowability is not to
            // be trusted, and sorting would hide that.
            if let Some(previous) = previous_known_at
                && known_at < previous
            {
                return Err(Error::invalid(format!(
                    "observation {index} is knowable at {}, before observation {} at {}: the \
                     tape is out of order and is refused rather than sorted",
                    known_at.to_rfc3339(),
                    index - 1,
                    previous.to_rfc3339()
                )));
            }
            previous_known_at = Some(known_at);

            if let Some(last) = last_at_per_instrument.get(&observation.object_id)
                && at <= *last
            {
                return Err(Error::invalid(format!(
                    "observation {index} for {} is at {}, not after the instrument's previous \
                     bar at {}: a tape may not restate or reorder a bar",
                    observation.object_id,
                    at.to_rfc3339(),
                    last.to_rfc3339()
                )));
            }
            last_at_per_instrument.insert(observation.object_id.clone(), at);

            let bar = Bar {
                object_id: ObjectId::from_string(observation.object_id.as_str()),
                venue: observation.venue.clone(),
                interval: document.interval,
                open_time: at.saturating_sub(document.interval.duration()),
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
            entries.push(TapeEntry { at, known_at, bar });
        }

        Ok(Self {
            name: document.name,
            interval: document.interval,
            entries,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn interval(&self) -> Interval {
        self.interval
    }

    pub fn entries(&self) -> &[TapeEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The instruments the tape prices, in id order.
    pub fn instruments(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .map(|entry| entry.bar.object_id.as_str().to_string())
            .collect()
    }

    /// Distinct knowable instants — the number of cycles a replay takes.
    pub fn periods(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| entry.known_at.as_nanos())
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// First and last knowable instants.
    pub fn span(&self) -> Option<(Timestamp, Timestamp)> {
        Some((
            self.entries.first()?.known_at,
            self.entries.last()?.known_at,
        ))
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
}

fn instant(text: &str, index: usize, field: &str) -> Result<Timestamp> {
    Timestamp::parse_rfc3339(text).ok_or_else(|| {
        Error::invalid(format!(
            "observation {index} has an unreadable {field} of {text:?}; RFC 3339 is required"
        ))
    })
}

/// A [`DataAdapter`] over a [`Tape`] that owns the clock it is read on.
#[derive(Debug)]
pub struct TapeFeed {
    tape: Tape,
    cursor: usize,
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
            cursor: 0,
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

    pub fn remaining(&self) -> usize {
        self.tape.len().saturating_sub(self.cursor)
    }

    /// The knowable instant of the next unreleased observation.
    pub fn next_known_at(&self) -> Option<Timestamp> {
        self.tape
            .entries
            .get(self.cursor)
            .map(|entry| entry.known_at)
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
            topics: vec![Topic::MarketBar],
            expected_latency: Duration::ZERO,
            production_requirement: Some(
                "a licensed market-data feed; a tape demonstrates the loop and prices nothing \
                 real"
                    .to_string(),
            ),
        }
    }

    /// Everything knowable by `until`, in tape order. Released by `known_at`:
    /// a bar whose close is in the past but whose publication is not stays on
    /// the tape.
    fn poll(&mut self, until: Timestamp) -> Result<Vec<SensedRecord>> {
        let mut out = Vec::new();
        while let Some(entry) = self.tape.entries.get(self.cursor) {
            if entry.known_at > until {
                break;
            }
            out.push(SensedRecord::Bar(Box::new(entry.bar.clone())));
            self.cursor += 1;
        }
        Ok(out)
    }

    /// Move the clock to the next knowable instant and return it, or `None`
    /// once the tape is spent. The clock never moves backwards, which
    /// [`Tape::parse`]'s order check already guarantees the tape cannot ask.
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

    fn document(observations: Vec<TapeObservation>) -> TapeDocument {
        TapeDocument {
            schema_version: SCHEMA_VERSION,
            name: "unit".to_string(),
            description: "a unit-test tape".to_string(),
            interval: Interval::Day,
            observations,
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

    /// The committed demonstration tape, regenerated.
    ///
    /// Four of the catalogue's instruments over 320 hourly periods, so a
    /// replay sizes into the same exposure buckets a deployment does. Hourly
    /// and not daily because the roster's manifests are reviewed at assembly
    /// and lapse ninety days later at tape time: a 320-day tape convened its
    /// first panel on day 103 and every agent was refused from then on.
    /// Three hundred and twenty hours is thirteen days, inside the window,
    /// and long enough that a five-day price-move horizon recorded at period
    /// 100 resolves on the tape at period 220. Two structures are planted,
    /// each aimed at one detector and each small enough that the controls
    /// stay controls:
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
    /// is firing on nothing. The noise is a fixed irrational rotation rather
    /// than an RNG, so the file reproduces from this function byte for byte.
    fn demonstration_document() -> TapeDocument {
        const INSTRUMENTS: [(&str, f64); 4] = [
            ("OBJ00000000000000000NWSC", 142.50),
            ("OBJ00000000000000000VNTG", 318.75),
            ("OBJ00000000000000000MRDN", 88.20),
            ("OBJ00000000000000000ATFB", 54.10),
        ];
        const PERIODS: usize = 320;
        let first_close = Timestamp::parse_rfc3339("2025-01-06T21:00:00Z").expect("an instant");
        let publication_delay = Duration::from_mins(15);

        let cents = |value: f64| -> Decimal {
            Decimal::parse(&format!("{value:.2}")).expect("a two-decimal price parses")
        };

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

        TapeDocument {
            schema_version: SCHEMA_VERSION,
            name: "loop-demonstration".to_string(),
            description: "A synthetic fixture for demonstrating that the decision loop runs end \
                          to end on data with a detectable structure in it. Not market data: \
                          every price is generated by qip-market-ingestion's tape tests from a \
                          fixed rotation, so there is no source, no licence and no question of \
                          either. Hourly, 320 periods, so it ends inside the roster's 90-day \
                          review window. NWSC jumps +1.5% on period 100 (its five-day claim \
                          resolves on tape at period 220); MRDN drifts +0.6% a period over \
                          periods 180-239 (its ninety-day claim does not resolve on tape); VNTG \
                          and ATFB are noise. Regenerate with `cargo test -p \
                          qip-market-ingestion demonstration_tape`, which fails naming the \
                          expected file when this one drifts."
                .to_string(),
            interval: Interval::Hour,
            observations,
        }
    }

    /// The document as the file is written: a readable header, one
    /// observation per line, so a diff of the fixture is a diff of bars.
    fn render(document: &TapeDocument) -> String {
        let mut text = String::new();
        text.push_str("{\n");
        text.push_str(&format!(
            "  \"schema_version\": {},\n  \"name\": {},\n  \"description\": {},\n  \"interval\": {},\n  \"observations\": [\n",
            document.schema_version,
            serde_json::to_string(&document.name).expect("a string serialises"),
            serde_json::to_string(&document.description).expect("a string serialises"),
            serde_json::to_string(&document.interval).expect("an interval serialises"),
        ));
        for (index, observation) in document.observations.iter().enumerate() {
            text.push_str("    ");
            text.push_str(&serde_json::to_string(observation).expect("an observation serialises"));
            if index + 1 < document.observations.len() {
                text.push(',');
            }
            text.push('\n');
        }
        text.push_str("  ]\n}\n");
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
        assert_eq!(tape.periods(), 320);
        assert_eq!(tape.instruments().len(), 4);
        assert_eq!(tape.len(), 1_280);
    }

    #[test]
    fn a_tape_is_synthetic_and_may_not_drive_a_capital_decision() {
        let feed = TapeFeed::new(Tape::from_document(document(two_days())).expect("loads"));
        let descriptor = feed.descriptor();
        assert!(!descriptor.is_production_grade());
        assert!(descriptor.production_requirement.is_some());
    }
}
