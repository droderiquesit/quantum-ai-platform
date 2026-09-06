//! Seventy-two simulated hours through the connector runtime, on a manual
//! clock, with no socket and no wall-clock time spent.
//!
//! Ingestion's fifth capability is a sustained fetch through a deployed egress
//! proxy. That half cannot be shown here and this file does not pretend to:
//! nothing is applied, no process has an outbound HTTPS path, and a connector
//! proven against a recording is not a connector proven in a deployment. What
//! *can* be shown in process is the other half — that the two disciplines the
//! data domain names hold over a long run rather than over one poll:
//!
//! * **Bounded retention.** The dedup window and the quarantine are the only
//!   two structures a [`ConnectorRuntime`] carries between polls. Both are
//!   capacity-bounded, and a run whose volume is two orders of magnitude past
//!   the bound is the only way to tell a bound that holds from a bound that has
//!   simply never been reached. The existing suite proves eviction on ten
//!   fingerprints against a window of four; that is the mechanism, not the
//!   endurance.
//! * **Idempotent dedup.** Every source here is at-least-once, and the Coinbase
//!   ticker has no cursor at all: it re-serves its last print until a new one
//!   happens. So a poll loop that ran for three days would publish the same
//!   trade thousands of times without the fingerprint. This run counts what was
//!   delivered against what was absorbed and requires the two to agree exactly,
//!   rather than requiring only that *some* duplicate was noticed.
//! * **Point in time.** A record must not be readable before its knowable
//!   instant. The ECB's rates are the sharpest case in the tree — sixteen hours
//!   between the reference date a series is indexed by and the instant a
//!   consumer could have acted on it — so the run crosses that boundary four
//!   times and checks each record against its own [`MarketEventEnvelope`]
//!   accessors: released at the first poll whose horizon reaches its knowable
//!   instant, and at no poll before it.
//!
//! # What is recorded and what is derived
//!
//! Both bodies come from the committed recordings — `coinbase_ticker::FIXTURE`
//! and `frankfurter_rates::FIXTURE`, each captured from the live endpoint. The
//! run does not invent a payload shape: it takes the recorded body and moves
//! only the fields the live source moves. For Coinbase that is `trade_id`,
//! `time` and `price`; `ask`, `bid`, `size` and the rolling 24-hour `volume`
//! stay exactly as recorded. For Frankfurter it is `date` alone — the recorded
//! rates travel untouched, which makes the run demonstrate something a varying
//! table would hide: an unchanged rate on a *new* reference date is a new
//! observation and must not be swallowed as a redelivery.
//!
//! # Why the capacities here are small
//!
//! `RuntimeConfig`'s shipped default remembers 8,192 fingerprints. A
//! seventy-two-hour fixture run would never fill it, the window would never
//! evict, and the comparison between the first simulated hour and the last
//! would compare two numbers that were both still growing — which would pass
//! whatever the bound did. The capacities below are sized so that every bound
//! is *saturated inside the first simulated hour*, which is what makes "the
//! footprint at hour one bounds the footprint at hour seventy-two" a statement
//! about the bound rather than about the run being short.
//!
//! The Frankfurter window is three, which is not arbitrary either: one recorded
//! rate table fans out into three observations, and a window narrower than one
//! batch cannot absorb a re-served page — it would evict the first pair before
//! the page came round again and republish it. Three is the smallest window
//! that works for this source, and the run exercises exactly that edge.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, ManualClock, ObjectId, Timestamp};
use qip_market_ingestion::connector::emulator::SourceEmulator;
use qip_market_ingestion::connector::transport::SourceTransport;
use qip_market_ingestion::connector::{
    ConnectorRuntime, PollReport, RuntimeConfig, SourceConnector,
};
use qip_market_ingestion::connectors::{
    CoinbaseTickerConnector, FrankfurterRatesConnector, coinbase_ticker, frankfurter_rates,
};
use qip_transport::RecordingSleeper;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

// --- the shape of the run ----------------------------------------------------

/// The simulated span. Three days, so the ECB's sixteen-hour dissemination
/// delay is crossed four times rather than once.
const SIMULATED_HOURS: i64 = 72;

/// One Coinbase print a simulated minute.
const MINUTES: i64 = SIMULATED_HOURS * 60;

/// Fingerprints the ticker's runtime remembers. Full at simulated minute 29 —
/// thirty prints and the two late ones — and evicting from minute 30 onwards.
const COINBASE_DEDUP_CAPACITY: usize = 32;

/// Dead letters the ticker's runtime holds. Full at simulated minute 35, the
/// fourth maintenance page.
const COINBASE_QUARANTINE_CAPACITY: usize = 4;

/// One whole rate table. See the module note on why this is the floor.
const FRANKFURTER_DEDUP_CAPACITY: usize = 3;

const FRANKFURTER_QUARANTINE_CAPACITY: usize = 4;

/// The instant the run starts: the recorded rate table's own reference date
/// plus the sixteen hours the manifest declares. So the recording is knowable
/// exactly at the first poll, and everything the run withholds afterwards is
/// withheld because the clock had not reached the instant, not because the
/// fixture was stale.
const START: &str = "2026-09-04T16:00:00Z";

/// A print re-served on this cadence — the redelivery an at-least-once source
/// produces on every overlapping poll window.
const REDELIVER_EVERY: i64 = 10;

/// A print that arrives late, out of order, on this cadence.
const LATE_EVERY: i64 = 20;

/// A vendor maintenance page served with HTTP 200 on this cadence, offset from
/// the redelivery so the two never land on one instant.
const MAINTENANCE_EVERY: i64 = 10;
const MAINTENANCE_OFFSET: i64 = 5;

/// How far back a late print is stamped.
const LATENESS: i64 = 5;

/// Trade ids for the late prints, kept clear of the sequence the fresh prints
/// walk, so a late print is a genuinely new event rather than a redelivery of
/// one already seen.
const LATE_TRADE_ID_BASE: u64 = 900_000_000;

// --- helpers -----------------------------------------------------------------

fn at(text: &str) -> Timestamp {
    Timestamp::parse_rfc3339(text).expect("a literal RFC 3339 instant")
}

/// The one recorded body inside a fixture script, decoded.
///
/// Reads the committed recording as data rather than restating it as a Rust
/// literal: a re-recording that changed a field would change what this run
/// replays, which is the point of the fixture.
fn recorded_body(fixture: &str) -> Result<Value> {
    let script: Value = serde_json::from_str(fixture).map_err(|error| {
        Error::invalid(format!("the fixture is not a recorded script: {error}"))
    })?;
    let body = script
        .get("exchanges")
        .and_then(|exchanges| exchanges.get(0))
        .and_then(|exchange| exchange.get("answers"))
        .and_then(|answers| answers.get(0))
        .and_then(|answer| answer.get("body"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            Error::invalid("the fixture's first exchange carries no recorded answer body")
        })?;
    serde_json::from_str(body).map_err(|error| {
        Error::invalid(format!(
            "the recorded body is not the JSON it claims: {error}"
        ))
    })
}

/// A decimal string as whole cents.
///
/// Split rather than read through `f64`. This is money, and it is the one place
/// in this file where an exact decimal becomes an integer; doing it by string
/// keeps the derived prices exact rather than rounding the recording.
fn cents(text: &str) -> Result<i64> {
    let (whole, fraction) = text.split_once('.').ok_or_else(|| {
        Error::invalid(format!(
            "the recorded price {text:?} carries no decimal point"
        ))
    })?;
    if fraction.len() != 2 {
        return Err(Error::invalid(format!(
            "the recorded price {text:?} is not written to the cent, so deriving a stream from it \
             would change its precision"
        )));
    }
    let whole: i64 = whole
        .parse()
        .map_err(|_| Error::invalid(format!("the recorded price {text:?} has no whole part")))?;
    let fraction: i64 = fraction
        .parse()
        .map_err(|_| Error::invalid(format!("the recorded price {text:?} has no cents part")))?;
    Ok(whole * 100 + fraction)
}

fn price(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// The recorded ticker body with the three fields a live ticker moves.
fn ticker_body(recorded: &Value, trade_id: u64, at: Timestamp, cents: i64) -> Result<String> {
    let mut body = recorded.clone();
    let object = body
        .as_object_mut()
        .ok_or_else(|| Error::invalid("the recorded ticker body is not a JSON object"))?;
    object.insert("trade_id".to_string(), Value::from(trade_id));
    object.insert("time".to_string(), Value::from(at.to_rfc3339()));
    object.insert("price".to_string(), Value::from(price(cents)));
    Ok(body.to_string())
}

/// The recorded rate table stamped with a reference date.
fn rates_body(recorded: &Value, date: Timestamp) -> Result<String> {
    let mut body = recorded.clone();
    let object = body
        .as_object_mut()
        .ok_or_else(|| Error::invalid("the recorded rate table is not a JSON object"))?;
    object.insert("date".to_string(), Value::from(date.to_date_string()));
    Ok(body.to_string())
}

/// One poll, over a transport that exists for exactly that poll.
///
/// A fresh emulator each time so that nothing but the runtime survives a poll.
/// The emulator's own call log is deliberately unbounded — a test asserting a
/// health probe did not spend a fetch reads it — and a single emulator held for
/// five thousand polls would be a working set growing in the harness, which is
/// precisely the thing this file claims does not grow in the runtime.
fn poll_once(
    runtime: &mut ConnectorRuntime,
    connector: &mut dyn SourceConnector,
    target: &str,
    body: &str,
    at: Timestamp,
) -> Result<PollReport> {
    let mut emulator = SourceEmulator::serving(target, body);
    let transport: &mut dyn SourceTransport = &mut emulator;
    runtime.poll(connector, transport, at)
}

/// Everything a runtime still holds between polls, in entries.
///
/// The dedup window and the quarantine are the two structures that grow; every
/// other part of a [`ConnectorRuntime`] is a fixed-size counter, a cursor, a
/// token bucket or a heartbeat. Both lengths come from the runtime's own
/// accessors, so this measures what the process holds rather than what the
/// test believes it configured.
fn footprint(runtime: &ConnectorRuntime) -> usize {
    runtime.dedup().len() + runtime.quarantine().len()
}

/// What the run made of the point-in-time gate.
///
/// Bounded on purpose: the offending records are quoted up to a handful and
/// counted beyond that. A soak test that collected every violation would, under
/// the mutation that breaks the gate, spend its memory proving the gate is
/// broken instead of saying so.
#[derive(Debug, Default)]
struct PointInTime {
    /// Admitted records whose knowable instant was checked against the horizon
    /// they were released at.
    checked: u64,
    /// Of those, the ones whose knowable instant is strictly after the instant
    /// the fact was true. Without these the check is vacuous: a source with no
    /// dissemination delay passes it by construction.
    delayed: u64,
    /// Records released before they were knowable. The defect this file exists
    /// to detect.
    early: u64,
    quoted: Vec<String>,
}

impl PointInTime {
    /// Check every record one poll released, against its own accessors.
    fn absorb(&mut self, report: &PollReport) {
        for envelope in &report.admitted {
            self.checked = self.checked.saturating_add(1);
            if envelope.knowable_at() > envelope.event_time() {
                self.delayed = self.delayed.saturating_add(1);
            }
            // `ingest_time` is the horizon the poll was made at; a record whose
            // knowable instant is later than that is a record a decision could
            // read before the deployment was entitled to see it.
            if envelope.knowable_at() > envelope.ingest_time() {
                self.early = self.early.saturating_add(1);
                if self.quoted.len() < 8 {
                    self.quoted.push(format!(
                        "{} is knowable at {} and was released at {}",
                        envelope.upstream_key(),
                        envelope.knowable_at().to_rfc3339(),
                        envelope.ingest_time().to_rfc3339()
                    ));
                }
            }
        }
    }
}

/// One reference rate, and the poll that first made it readable.
#[derive(Clone, Debug)]
struct Release {
    knowable_at: Timestamp,
    first_readable_at: Timestamp,
}

// --- the run -----------------------------------------------------------------

#[test]
fn a_seventy_two_hour_replay_holds_every_declared_bound_absorbs_every_duplicate_and_publishes_nothing_before_it_is_knowable()
-> Result<()> {
    let start = at(START);
    // The one clock in the run. Nothing here reads a wall clock and nothing
    // sleeps: `RecordingSleeper` records a backoff instead of spending it, and
    // the assertion at the end that it recorded nothing is what says the run's
    // three days cost no real seconds.
    let clock = ManualClock::new(start);
    let sleeper = Arc::new(RecordingSleeper::new());

    let ticker_recording = recorded_body(coinbase_ticker::FIXTURE)?;
    let rates_recording = recorded_body(frankfurter_rates::FIXTURE)?;
    let first_trade_id = ticker_recording
        .get("trade_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::invalid("the Coinbase recording carries no integer trade_id"))?;
    let first_cents = cents(
        ticker_recording
            .get("price")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid("the Coinbase recording carries no price string"))?,
    )?;
    // Premise: the two derived streams start from the recording rather than
    // from constants written here, so a re-recorded fixture moves this run.
    assert!(
        first_trade_id > 0 && first_cents > 0,
        "the recording supplied no starting print: trade id {first_trade_id}, {first_cents} cents"
    );
    assert!(
        LATE_TRADE_ID_BASE > first_trade_id + MINUTES as u64,
        "the late prints' id range overlaps the fresh prints', so a late print could collide with \
         a print already seen and be counted as a redelivery of it"
    );

    let ticker_manifest = CoinbaseTickerConnector::shipped_manifest()?;
    let ticker_path = ticker_manifest.endpoint.path.clone();
    let mut ticker = CoinbaseTickerConnector::new(
        ticker_manifest.clone(),
        "BTC-USD",
        ObjectId::from_string("OBJ0000000000000000BTCUSD"),
        CoinbaseTickerConnector::VENUE,
    )?;
    let mut ticker_runtime = ConnectorRuntime::new(
        ticker_manifest,
        RuntimeConfig::seeded(0x50AC_0000_0000_0001)
            .with_sleeper(sleeper.clone())
            .with_dedup_capacity(COINBASE_DEDUP_CAPACITY)
            .with_quarantine_capacity(COINBASE_QUARANTINE_CAPACITY),
    )?;

    let rates_manifest = FrankfurterRatesConnector::shipped_manifest()?;
    let rates_path = rates_manifest.endpoint.path.clone();
    let publication_delay = rates_manifest.publication_delay();
    let mut rates = FrankfurterRatesConnector::new(rates_manifest.clone())?;
    let mut rates_runtime = ConnectorRuntime::new(
        rates_manifest,
        RuntimeConfig::seeded(0x50AC_0000_0000_0002)
            .with_sleeper(sleeper.clone())
            .with_dedup_capacity(FRANKFURTER_DEDUP_CAPACITY)
            .with_quarantine_capacity(FRANKFURTER_QUARANTINE_CAPACITY),
    )?;

    // Premise: the delay under test is the manifest's own, not a number
    // written into this file. Sixteen hours is what makes a reference rate
    // unreadable for two thirds of its own reference date.
    assert_eq!(publication_delay, Duration::from_hours(16));

    // Both sources connect against the recording itself, which is the body
    // their health path serves.
    {
        let mut emulator = SourceEmulator::serving(&ticker_path, ticker_recording.to_string());
        ticker_runtime.connect(&mut ticker, &mut emulator, start)?;
    }
    {
        let mut emulator = SourceEmulator::serving(&rates_path, rates_recording.to_string());
        rates_runtime.connect(&mut rates, &mut emulator, start)?;
    }

    // What the harness delivered, so the runtime's own counters can be checked
    // against it rather than against themselves.
    let mut delivered_distinct: u64 = 0;
    let mut delivered_duplicate: u64 = 0;
    let mut delivered_malformed: u64 = 0;
    let mut delivered_rate_events: u64 = 0;
    let mut delivered_rate_tables: u64 = 0;

    let mut point_in_time = PointInTime::default();
    let mut releases: BTreeMap<String, Release> = BTreeMap::new();

    let mut peak_ticker_footprint = 0usize;
    let mut peak_rates_footprint = 0usize;
    let mut footprint_at_hour_one = 0usize;
    let mut newest_print = start;

    for minute in 0..=MINUTES {
        // The manual clock is the only source of time in the run.
        clock.set(start.saturating_add(Duration::from_mins(minute)));
        let now = clock.now();

        // The print of this minute.
        let body = ticker_body(
            &ticker_recording,
            first_trade_id.saturating_add(minute as u64),
            now,
            // A sawtooth of a dollar either side of the recorded price, in
            // exact cents. Nothing reads the level; it exists so that
            // consecutive prints differ in the field a real ticker changes.
            first_cents + (minute % 200) - 100,
        )?;
        let report = poll_once(&mut ticker_runtime, &mut ticker, &ticker_path, &body, now)?;
        delivered_distinct += 1;
        newest_print = now;
        point_in_time.absorb(&report);

        // The redelivery: the identical body, the identical bytes, one
        // simulated second later. This is what a source with no cursor does on
        // every overlapping poll window.
        if minute % REDELIVER_EVERY == 0 {
            let again = now.saturating_add(Duration::from_secs(1));
            let report = poll_once(&mut ticker_runtime, &mut ticker, &ticker_path, &body, again)?;
            delivered_duplicate += 1;
            point_in_time.absorb(&report);
        }

        // The late print: a genuinely new trade, stamped five minutes back.
        // Out of order, and the runtime must admit it while refusing to let it
        // rewind the cursor.
        if minute % LATE_EVERY == 0 {
            let stamped = now.saturating_sub(Duration::from_mins(LATENESS));
            let body = ticker_body(
                &ticker_recording,
                LATE_TRADE_ID_BASE.saturating_add(minute as u64),
                stamped,
                first_cents + (minute % 200) - 100,
            )?;
            let horizon = now.saturating_add(Duration::from_secs(2));
            let report = poll_once(
                &mut ticker_runtime,
                &mut ticker,
                &ticker_path,
                &body,
                horizon,
            )?;
            delivered_distinct += 1;
            point_in_time.absorb(&report);
            assert_eq!(
                ticker_runtime.cursor().position.event_time(),
                Some(newest_print),
                "a print stamped five minutes back rewound the cursor at simulated minute \
                 {minute}, so the next fetch would re-read five minutes the window then absorbs \
                 in silence"
            );
        }

        // A vendor maintenance page, served with HTTP 200 as they are. It
        // decodes into nothing and must be held rather than dropped.
        if minute % MAINTENANCE_EVERY == MAINTENANCE_OFFSET {
            let horizon = now.saturating_add(Duration::from_secs(3));
            let report = poll_once(
                &mut ticker_runtime,
                &mut ticker,
                &ticker_path,
                "<html><body>scheduled maintenance</body></html>",
                horizon,
            )?;
            delivered_malformed += 1;
            assert_eq!(
                report.quarantined, 1,
                "a maintenance page was neither decoded nor held at simulated minute {minute}"
            );
        }

        // On the hour: the rate table for the day in progress.
        if minute % 60 == 0 {
            let body = rates_body(&rates_recording, now.start_of_day())?;
            let report = poll_once(&mut rates_runtime, &mut rates, &rates_path, &body, now)?;
            delivered_rate_tables += 1;
            delivered_rate_events += 3;
            point_in_time.absorb(&report);
            for envelope in &report.admitted {
                releases
                    .entry(envelope.upstream_key().to_string())
                    .or_insert(Release {
                        knowable_at: envelope.knowable_at(),
                        first_readable_at: envelope.ingest_time(),
                    });
            }
        }

        peak_ticker_footprint = peak_ticker_footprint.max(footprint(&ticker_runtime));
        peak_rates_footprint = peak_rates_footprint.max(footprint(&rates_runtime));

        // Both bounds are read from the runtime as the run goes, so a window
        // that overshot for one poll and settled back would still fail here.
        assert!(
            ticker_runtime.dedup().len() <= ticker_runtime.dedup().capacity(),
            "the ticker's dedup window holds {} fingerprints against a declared capacity of {} at \
             simulated minute {minute}",
            ticker_runtime.dedup().len(),
            ticker_runtime.dedup().capacity()
        );
        assert!(
            ticker_runtime.quarantine().len() <= ticker_runtime.quarantine().capacity(),
            "the ticker's quarantine holds {} entries against a declared capacity of {} at \
             simulated minute {minute}",
            ticker_runtime.quarantine().len(),
            ticker_runtime.quarantine().capacity()
        );
        assert!(
            rates_runtime.dedup().len() <= rates_runtime.dedup().capacity(),
            "the rate feed's dedup window holds {} fingerprints against a declared capacity of {} \
             at simulated minute {minute}",
            rates_runtime.dedup().len(),
            rates_runtime.dedup().capacity()
        );

        if minute == 60 {
            footprint_at_hour_one = footprint(&ticker_runtime) + footprint(&rates_runtime);
        }
    }

    let footprint_at_hour_seventy_two = footprint(&ticker_runtime) + footprint(&rates_runtime);

    // --- the premise: the run's volume is far past the bounds ----------------

    let ticker_dedup_capacity = ticker_runtime.dedup().capacity();
    let ticker_quarantine_capacity = ticker_runtime.quarantine().capacity();
    let rates_dedup_capacity = rates_runtime.dedup().capacity();
    let delivered_ticker_events = delivered_distinct + delivered_duplicate;

    // Premise before any count is compared: every poll the run made actually
    // went out. A rate limiter that deferred a poll, or a ladder that gave up
    // on one, would leave the harness's tally and the runtime's describing two
    // different runs — and the dedup arithmetic below would be checking
    // nothing.
    assert_eq!(
        ticker_runtime.stats().polls,
        delivered_distinct + delivered_duplicate + delivered_malformed,
        "the harness made one count of the ticker's polls and the runtime another"
    );
    assert_eq!(rates_runtime.stats().polls, delivered_rate_tables);
    assert_eq!(
        delivered_rate_tables,
        SIMULATED_HOURS as u64 + 1,
        "the rate feed was polled once an hour on the hour, including the last"
    );
    for (source, stats) in [
        ("the ticker", ticker_runtime.stats()),
        ("the rate feed", rates_runtime.stats()),
    ] {
        assert_eq!(
            stats.deferrals, 0,
            "{source} was deferred by its own rate limiter, so a poll this run counted never \
             reached the source"
        );
        assert_eq!(
            stats.refusals, 0,
            "{source} exhausted its retries on a poll, so a batch this run counted was never \
             served"
        );
    }

    assert!(
        delivered_ticker_events > 100 * ticker_dedup_capacity as u64,
        "the run delivered {delivered_ticker_events} prints through a window of \
         {ticker_dedup_capacity}, which is not enough volume to tell a bound that holds from a \
         bound nothing ever reached"
    );
    assert!(
        delivered_malformed > 100 * ticker_quarantine_capacity as u64,
        "the run served {delivered_malformed} maintenance pages into a quarantine of \
         {ticker_quarantine_capacity}, which never forces it to evict"
    );
    assert!(
        delivered_rate_events > 20 * rates_dedup_capacity as u64,
        "the run delivered {delivered_rate_events} rate observations through a window of \
         {rates_dedup_capacity}"
    );

    // --- bounded retention ---------------------------------------------------

    let ticker_dedup = ticker_runtime.dedup();
    assert_eq!(
        peak_ticker_footprint,
        ticker_dedup_capacity + ticker_quarantine_capacity,
        "the ticker runtime's largest footprint over the run was {peak_ticker_footprint} entries \
         and the two declared bounds sum to {}",
        ticker_dedup_capacity + ticker_quarantine_capacity
    );
    assert_eq!(
        ticker_dedup.len(),
        ticker_dedup_capacity,
        "the window did not fill, so nothing here proves it evicts"
    );
    assert_eq!(
        ticker_dedup.evicted(),
        ticker_dedup.admitted() - ticker_dedup_capacity as u64,
        "the window admitted {} fingerprints, holds {ticker_dedup_capacity} and counted {} \
         evictions; the three cannot all be true, and a deployment reads the eviction count to \
         learn its window is too small",
        ticker_dedup.admitted(),
        ticker_dedup.evicted()
    );
    assert_eq!(
        ticker_runtime.quarantine().len(),
        ticker_quarantine_capacity
    );
    assert_eq!(
        ticker_runtime.quarantine().overflowed(),
        delivered_malformed - ticker_quarantine_capacity as u64,
        "the quarantine held {delivered_malformed} maintenance pages in a store of \
         {ticker_quarantine_capacity} and reported {} dropped; a dead-letter store losing \
         evidence must say so itself",
        ticker_runtime.quarantine().overflowed()
    );
    assert_eq!(
        rates_runtime.dedup().len(),
        rates_dedup_capacity,
        "the rate window did not fill"
    );
    assert_eq!(
        peak_rates_footprint, rates_dedup_capacity,
        "the rate feed's largest footprint over the run was {peak_rates_footprint} entries \
         against a window of {rates_dedup_capacity} and a quarantine that held nothing"
    );
    assert_eq!(
        rates_runtime.dedup().evicted(),
        rates_runtime.dedup().admitted() - rates_dedup_capacity as u64,
        "a window exactly one batch wide must evict the previous day's table to hold today's, \
         and the eviction count says otherwise"
    );

    // --- the size measure does not grow --------------------------------------

    assert!(
        footprint_at_hour_one > 0,
        "the footprint at simulated hour one was zero, so the comparison below would hold for a \
         run that ingested nothing"
    );
    assert!(
        footprint_at_hour_one >= footprint_at_hour_seventy_two,
        "the two runtimes held {footprint_at_hour_one} entries after one simulated hour and \
         {footprint_at_hour_seventy_two} after seventy-two; a working set that is still growing \
         at hour seventy-two is a process that dies of memory on the day the source replays its \
         history"
    );

    // --- idempotent dedup ----------------------------------------------------

    assert!(
        delivered_duplicate > 0,
        "no redelivery was scripted, so the counts below would agree for a run with nothing to \
         deduplicate"
    );
    assert_eq!(
        ticker_dedup.duplicates(),
        delivered_duplicate,
        "the run delivered {delivered_duplicate} redeliveries and the window recognised {}",
        ticker_dedup.duplicates()
    );
    assert_eq!(
        ticker_runtime.stats().admitted,
        delivered_distinct,
        "the run delivered {delivered_distinct} distinct prints and the runtime published {}: a \
         source with no cursor re-serves its last print for as long as no newer one happens, and \
         without the fingerprint each poll would publish it again",
        ticker_runtime.stats().admitted
    );
    assert_eq!(
        ticker_runtime.stats().admitted + ticker_runtime.stats().duplicates,
        delivered_ticker_events,
        "the prints delivered and the prints accounted for disagree, so something was neither \
         published nor recognised as a redelivery"
    );
    assert_eq!(
        rates_runtime.stats().duplicates,
        delivered_rate_events - rates_runtime.stats().admitted - rates_runtime.stats().withheld,
        "every rate observation the run delivered is published once, withheld or recognised as a \
         redelivery, and these do not add up"
    );
    // The recorded rates never move in this run, only the reference date does.
    // A window that keyed on the value would swallow the second day's table
    // whole, and a rate that did not change would silently stop being observed.
    assert_eq!(
        rates_runtime.stats().admitted,
        delivered_rate_tables_becoming_knowable() * 3,
        "an unchanged rate on a new reference date was dropped as a redelivery of the day before"
    );

    // --- point in time -------------------------------------------------------

    assert!(
        point_in_time.checked > 0 && point_in_time.delayed > 0,
        "the point-in-time check saw {} records of which {} carried a dissemination delay; with \
         no delayed record the check passes by construction",
        point_in_time.checked,
        point_in_time.delayed
    );
    assert_eq!(
        point_in_time.early, 0,
        "{} record(s) were released before the instant a decision could have used them: {:?}",
        point_in_time.early, point_in_time.quoted
    );
    assert!(
        rates_runtime.stats().withheld > 0,
        "no rate observation was ever withheld, so the gate that withholds them was not exercised"
    );

    // Each rate became readable at the *first* poll whose horizon reached its
    // knowable instant — neither before it, which is look-ahead, nor at some
    // later poll, which would be a record lost to the window while it waited.
    assert_eq!(
        releases.len() as u64,
        rates_runtime.stats().admitted,
        "a published rate is missing from the record of what became readable"
    );
    for (key, release) in &releases {
        assert!(
            release.first_readable_at >= release.knowable_at,
            "{key} became readable at {} and is knowable only at {}",
            release.first_readable_at.to_rfc3339(),
            release.knowable_at.to_rfc3339()
        );
        assert!(
            release
                .first_readable_at
                .saturating_sub(Duration::from_hours(1))
                < release.knowable_at,
            "{key} is knowable at {} and was not released until {}, a whole poll later: the \
             window absorbed it while it was being withheld",
            release.knowable_at.to_rfc3339(),
            release.first_readable_at.to_rfc3339()
        );
    }

    // --- ordering ------------------------------------------------------------

    assert_eq!(
        ticker_runtime.cursor().position.event_time(),
        Some(newest_print),
        "after seventy-two hours of prints, the last of which arrived out of order, the cursor \
         sits somewhere other than the newest event the run delivered"
    );
    assert_eq!(
        ticker_runtime.cursor().events_seen,
        delivered_distinct,
        "the cursor's own count of what went past it disagrees with what was published, and it is \
         the number a reconciliation against the provider joins on"
    );

    // --- nothing was spent ---------------------------------------------------

    assert!(
        sleeper.recorded().is_empty(),
        "the run spent {} backoff interval(s); three simulated days that cost real seconds is a \
         soak nobody will keep running",
        sleeper.recorded().len()
    );

    // The run's shape, for whoever reads this suite's output rather than its
    // source. Every number here is one an assertion above depends on, so a
    // change in the run's volume is visible under `--nocapture` instead of
    // having to be re-derived from the constants.
    println!(
        "soak: {SIMULATED_HOURS} simulated hours\n  \
         ticker    polls {} | delivered {delivered_ticker_events} ({delivered_distinct} distinct, \
         {delivered_duplicate} redelivered) | published {} | duplicates {} | window {}/{} evicted \
         {} | maintenance pages {delivered_malformed} | quarantine {}/{} overflowed {}\n  \
         rate feed polls {} | delivered {delivered_rate_events} | published {} | withheld {} | \
         duplicates {} | window {}/{} evicted {}\n  \
         footprint hour 1 {footprint_at_hour_one} entries, hour {SIMULATED_HOURS} \
         {footprint_at_hour_seventy_two} | point-in-time checked {} of which {} delayed, {} early",
        ticker_runtime.stats().polls,
        ticker_runtime.stats().admitted,
        ticker_dedup.duplicates(),
        ticker_dedup.len(),
        ticker_dedup_capacity,
        ticker_dedup.evicted(),
        ticker_runtime.quarantine().len(),
        ticker_quarantine_capacity,
        ticker_runtime.quarantine().overflowed(),
        rates_runtime.stats().polls,
        rates_runtime.stats().admitted,
        rates_runtime.stats().withheld,
        rates_runtime.stats().duplicates,
        rates_runtime.dedup().len(),
        rates_dedup_capacity,
        rates_runtime.dedup().evicted(),
        point_in_time.checked,
        point_in_time.delayed,
        point_in_time.early,
    );
    Ok(())
}

/// Rate tables whose knowable instant falls inside the run.
///
/// The run starts at the recorded table's own knowable instant and covers three
/// further midnights, so four tables become readable: the recorded date's, and
/// one for each following day at sixteen hours past midnight.
const fn delivered_rate_tables_becoming_knowable() -> u64 {
    1 + (SIMULATED_HOURS / 24) as u64
}
