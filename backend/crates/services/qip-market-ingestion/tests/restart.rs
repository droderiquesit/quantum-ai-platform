//! Seven simulated days of the ECB rate feed across eight processes, on a
//! manual clock, with no socket and no wall-clock time spent.
//!
//! # What this file adds to `soak.rs`, and why it is a different file
//!
//! `soak.rs` runs seventy-two simulated hours through **one**
//! [`ConnectorRuntime`] and proves what that run can prove: the bounds hold,
//! every redelivery is absorbed, nothing is readable before it is knowable.
//! Every one of those statements is about a process that never dies.
//!
//! A deployment asked to stream for seven days is not one process. It is a
//! revision rollout, an eviction, an out-of-memory kill and a redeploy, and at
//! each of them the runtime is rebuilt from nothing. Before the change these
//! tests accompany, that meant:
//!
//! * the dedup window was **empty**, and
//! * `FrankfurterRatesConnector::decode` takes no cursor — it re-decodes the
//!   whole rate table on every poll — so
//! * the first poll of every new process republished the entire table as new
//!   observations.
//!
//! The three ECB rates that went through the loop live on 2026-09-06
//! (`docs/ops/live-source-frankfurter-2026-09-06.md`) demonstrated the
//! in-process half of this and could not have demonstrated the other: the
//! second cycle released nothing because it was the *same* process. A restart
//! between those two cycles would have released three again, and the record
//! would have read as six observations of a table the ECB published once.
//!
//! The first test below establishes that as a premise rather than asserting it
//! from the outside: it drives the republication, on the same bodies, from a
//! checkpoint with its carry stripped — which is byte-for-byte what a
//! checkpoint written before this change looks like — and only then asserts
//! that the carried one suppresses it.
//!
//! # What is recorded and what is derived
//!
//! The body comes from the committed recording, `frankfurter_rates::FIXTURE`,
//! captured from the live endpoint. The run moves `date` and nothing else — the
//! recorded rates travel untouched — which is the same discipline `soak.rs`
//! states and for the same reason: an unchanged rate on a *new* reference date
//! is a new observation and must not be swallowed as a redelivery.
//!
//! The seven-day calendar is simulated and is **not** the ECB's. The real bank
//! publishes on business days and this run advances the reference date every
//! day, because what is under test is the span, the restart boundary and the
//! two time axes, none of which a weekend changes. Nothing here is evidence
//! about the vendor's behaviour, and the ledger it produces is the *shape* a
//! week of evidence would be recorded in rather than a week of evidence.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::kv::KeyValueStore;
use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_market_ingestion::adapter::DataAdapter;
use qip_market_ingestion::connector::emulator::SourceEmulator;
use qip_market_ingestion::connector::journal::StreamJournal;
use qip_market_ingestion::connector::transport::SourceTransport;
use qip_market_ingestion::connector::{
    Checkpoint, ConnectorRuntime, PollReport, RuntimeConfig, SourceConnector,
};
use qip_market_ingestion::connector_feed::ConnectorFeed;
use qip_market_ingestion::connectors::{FrankfurterRatesConnector, frankfurter_rates};
use qip_storage::kv::MemoryKeyValueStore;
use qip_transport::RecordingSleeper;
use serde_json::Value;
use std::sync::Arc;

// --- the shape of the run ----------------------------------------------------

/// The instant the run starts: the recorded table's reference date plus the
/// sixteen hours the manifest declares as the dissemination delay. So the first
/// table is knowable exactly at the first poll.
const START: &str = "2026-09-04T16:00:00Z";

/// The reference date the recording carries, which the run advances from.
const FIRST_REFERENCE_DATE: &str = "2026-09-04T00:00:00Z";

/// Hourly polls across seven days, both ends included. The manifest's own
/// `poll_interval_ms` is one hour, so this is the cadence a deployment runs at
/// rather than a cadence chosen to make a number come out.
const HOURS: i64 = 168;

/// Hours before a table is knowable that the vendor starts serving it. Four,
/// so that every day boundary in the run crosses the point-in-time gate with
/// polls on the wrong side of it, rather than the gate being a branch the run
/// never takes.
const SERVED_EARLY_HOURS: i64 = 4;

/// The hour of the day at which the process is restarted. Midday rather than
/// midnight on purpose: at midnight a new table arrives and a restart that
/// republished it would be indistinguishable from a restart that correctly
/// admitted it. At midday the table being served is one the *previous* process
/// already absorbed, so the poll immediately after a restart must admit
/// nothing, and admitting three is the defect.
const RESTART_AT_HOUR: i64 = 12;

/// One rate table fans out into three observations: the manifest asks for
/// `USD,GBP,JPY`.
const RATES_PER_TABLE: u64 = 3;

/// A window of one whole table. The floor, for the reason `soak.rs` states: a
/// window narrower than one batch evicts the first pair before the page comes
/// round again and republishes it.
const DEDUP_CAPACITY: usize = 3;

const QUARANTINE_CAPACITY: usize = 4;

/// Reference dates the run gets through: `2026-09-04` plus one for each day.
const TABLES: u64 = 8;

/// Restarts, one per day boundary crossed after the first midday.
const RESTARTS: u64 = 7;

// --- helpers -----------------------------------------------------------------

fn at(text: &str) -> Timestamp {
    Timestamp::parse_rfc3339(text).expect("a literal RFC 3339 instant")
}

/// The one recorded body inside the committed fixture script, decoded.
///
/// Read as data rather than restated as a Rust literal, so a re-recording that
/// changed a field changes what this run replays.
fn recorded_body() -> Result<Value> {
    let script: Value = serde_json::from_str(frankfurter_rates::FIXTURE).map_err(|error| {
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
    serde_json::from_str(body)
        .map_err(|error| Error::invalid(format!("the recorded body is not JSON: {error}")))
}

/// The recorded rate table stamped with a reference date, and nothing else
/// touched.
fn rates_body(recorded: &Value, date: Timestamp) -> Result<String> {
    let mut body = recorded.clone();
    let object = body
        .as_object_mut()
        .ok_or_else(|| Error::invalid("the recorded rate table is not a JSON object"))?;
    object.insert("date".to_string(), Value::from(date.to_date_string()));
    Ok(body.to_string())
}

/// One poll over a transport that exists for exactly that poll.
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

/// A connector and its runtime, connected against the body its health path
/// serves — one whole process's worth of ingestion state.
fn start_process(
    body: &str,
    sleeper: &Arc<RecordingSleeper>,
    at: Timestamp,
) -> Result<(FrankfurterRatesConnector, ConnectorRuntime, String)> {
    let manifest = FrankfurterRatesConnector::shipped_manifest()?;
    let path = manifest.endpoint.path.clone();
    let mut connector = FrankfurterRatesConnector::new(manifest.clone())?;
    let mut runtime = ConnectorRuntime::new(
        manifest,
        RuntimeConfig::seeded(0x0EC8_0000_0000_0001)
            .with_sleeper(sleeper.clone())
            .with_dedup_capacity(DEDUP_CAPACITY)
            .with_quarantine_capacity(QUARANTINE_CAPACITY),
    )?;
    let mut emulator = SourceEmulator::serving(&path, body);
    runtime.connect(&mut connector, &mut emulator, at)?;
    Ok((connector, runtime, path))
}

// --- the restart boundary ----------------------------------------------------

#[test]
fn a_restarted_connector_recognises_the_table_its_predecessor_absorbed_and_without_the_carry_republishes_it()
-> Result<()> {
    let first_poll = at(START);
    let second_poll = first_poll.saturating_add(Duration::from_hours(1));
    let recorded = recorded_body()?;
    let body = rates_body(&recorded, at(FIRST_REFERENCE_DATE))?;
    let sleeper = Arc::new(RecordingSleeper::new());
    let store: Arc<dyn KeyValueStore> = Arc::new(MemoryKeyValueStore::new());

    // --- process one -------------------------------------------------------
    let (mut connector, mut runtime, path) = start_process(&body, &sleeper, first_poll)?;
    let (mut journal, resumed) =
        StreamJournal::open(store.clone(), "frankfurter-ecb-reference-rates")?;
    assert!(
        resumed.is_none(),
        "an empty store must offer no checkpoint, or the restart below would be resuming from \
         something this test did not write"
    );

    let report = poll_once(&mut runtime, &mut connector, &path, &body, first_poll)?;
    journal.record(&report, first_poll)?;
    // Premise: the source really did deliver a whole table, so the suppression
    // asserted further down is a suppression of something rather than a poll
    // that happened to be empty.
    assert_eq!(
        report.admitted.len() as u64,
        RATES_PER_TABLE,
        "the recorded table must yield three observations, or this run proves nothing about \
         three observations being recognised again"
    );

    let checkpoint = runtime.checkpoint(second_poll);
    journal.commit(&checkpoint)?;
    assert_eq!(
        checkpoint.recent_fingerprints.len() as u64,
        RATES_PER_TABLE,
        "the checkpoint must carry the whole table, or the restart is being handed less than the \
         next poll will re-serve"
    );
    // The boundary event, which had a field on `Checkpoint` and no writer until
    // this change — a declared fact nothing produced.
    assert_eq!(
        checkpoint.last_fingerprint.as_deref(),
        checkpoint.recent_fingerprints.last().map(String::as_str),
        "the boundary fingerprint must be the newest one carried"
    );
    drop(runtime);
    drop(connector);
    drop(journal);

    // --- the control: the same restart from a checkpoint with no carry ------
    //
    // This is exactly what a checkpoint written before this change looks like:
    // `recent_fingerprints` absent, so `serde`'s default gives an empty vector.
    // It is here so that the assertion after it cannot pass for any reason
    // other than the carry.
    let mut stripped = checkpoint.clone();
    stripped.recent_fingerprints.clear();
    stripped.last_fingerprint = None;
    let (mut blind_connector, mut blind_runtime, blind_path) =
        start_process(&body, &sleeper, second_poll)?;
    let taken = blind_runtime.resume(&mut blind_connector, &stripped)?;
    assert_eq!(taken, 0, "a stripped checkpoint carries nothing to take");
    let blind = poll_once(
        &mut blind_runtime,
        &mut blind_connector,
        &blind_path,
        &body,
        second_poll,
    )?;
    assert_eq!(
        blind.admitted.len() as u64,
        RATES_PER_TABLE,
        "without a carried window the identical table is republished in full — this is the defect \
         the carry closes, and if this line ever fails the test below is proving nothing"
    );
    assert_eq!(blind.duplicates, 0);

    // --- process two, resuming properly ------------------------------------
    let (mut journal, resumed) =
        StreamJournal::open(store.clone(), "frankfurter-ecb-reference-rates")?;
    let resumed = resumed.ok_or_else(|| {
        Error::invalid("the journal committed a checkpoint and did not offer it back")
    })?;
    assert_eq!(
        resumed, checkpoint,
        "the checkpoint must survive the store unchanged, carry included"
    );

    let (mut connector, mut runtime, path) = start_process(&body, &sleeper, second_poll)?;
    let taken = runtime.resume(&mut connector, &resumed)?;
    assert_eq!(
        taken as u64, RATES_PER_TABLE,
        "the whole carried table must be taken into the new window"
    );

    let report = poll_once(&mut runtime, &mut connector, &path, &body, second_poll)?;
    journal.record(&report, second_poll)?;
    assert!(
        report.admitted.is_empty(),
        "the identical table after a restart must be recognised, not republished; {} record(s) \
         were released",
        report.admitted.len()
    );
    assert_eq!(
        report.duplicates, RATES_PER_TABLE,
        "and it must be recognised as three redeliveries rather than disappearing"
    );

    // The ledger is the operator-readable half: two sessions, two polls, three
    // records ever admitted, three redeliveries absorbed.
    let ledger = journal.ledger();
    assert_eq!(ledger.sessions, 2);
    assert_eq!(ledger.polls, 2);
    assert_eq!(ledger.admitted, RATES_PER_TABLE);
    assert_eq!(ledger.duplicates, RATES_PER_TABLE);
    assert_eq!(ledger.carried_fingerprints as u64, RATES_PER_TABLE);
    assert_eq!(
        sleeper.total(),
        Duration::ZERO,
        "no attempt failed, so no backoff should have been spent"
    );
    Ok(())
}

#[test]
fn a_checkpoint_from_another_source_is_refused_before_a_single_fingerprint_reaches_the_window()
-> Result<()> {
    let when = at(START);
    let recorded = recorded_body()?;
    let body = rates_body(&recorded, at(FIRST_REFERENCE_DATE))?;
    let sleeper = Arc::new(RecordingSleeper::new());

    let (mut connector, mut runtime, path) = start_process(&body, &sleeper, when)?;
    let report = poll_once(&mut runtime, &mut connector, &path, &body, when)?;
    assert_eq!(report.admitted.len() as u64, RATES_PER_TABLE);
    let mut foreign = runtime.checkpoint(when);
    // Premise: the carry is non-empty, so a resume that ignored the source
    // check would have something to wrongly install.
    assert_eq!(foreign.recent_fingerprints.len() as u64, RATES_PER_TABLE);
    foreign.source_id = "coinbase-spot-ticker".to_string();

    let (mut fresh_connector, mut fresh_runtime, _) = start_process(&body, &sleeper, when)?;
    let error = fresh_runtime
        .resume(&mut fresh_connector, &foreign)
        .expect_err("another source's checkpoint was accepted");
    assert!(
        error.message().contains("coinbase-spot-ticker"),
        "the refusal must name the checkpoint's own source: {}",
        error.message()
    );
    assert_eq!(
        fresh_runtime.dedup().len(),
        0,
        "a refused checkpoint must leave the window untouched — another feed's fingerprints in \
         this one's window would suppress real records with every counter reading zero"
    );
    Ok(())
}

#[test]
fn a_checkpoint_carrying_more_than_the_bound_or_something_that_is_not_a_fingerprint_is_refused()
-> Result<()> {
    let when = at(START);
    let recorded = recorded_body()?;
    let body = rates_body(&recorded, at(FIRST_REFERENCE_DATE))?;
    let sleeper = Arc::new(RecordingSleeper::new());
    let (mut connector, mut runtime, path) = start_process(&body, &sleeper, when)?;
    poll_once(&mut runtime, &mut connector, &path, &body, when)?;
    let good = runtime.checkpoint(when);
    // Premise: the honest checkpoint reads back cleanly, so each refusal below
    // is caused by the edit that follows and not by the fixture.
    assert_eq!(good.carried()?.len() as u64, RATES_PER_TABLE);

    let sample = good
        .recent_fingerprints
        .first()
        .cloned()
        .ok_or_else(|| Error::invalid("the checkpoint carried nothing"))?;

    let mut oversized = good.clone();
    oversized.recent_fingerprints = vec![sample.clone(); Checkpoint::CARRIED_FINGERPRINTS + 1];
    let error = oversized
        .carried()
        .expect_err("an unbounded carry was accepted");
    assert!(
        error
            .message()
            .contains(&Checkpoint::CARRIED_FINGERPRINTS.to_string()),
        "the refusal must name the bound: {}",
        error.message()
    );

    let mut truncated = good.clone();
    truncated.recent_fingerprints = vec![sample[..32].to_string()];
    let error = truncated
        .carried()
        .expect_err("a truncated fingerprint was accepted");
    assert!(
        error.message().contains("32 were given"),
        "the refusal must say what it got: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_window_that_has_already_polled_refuses_a_carry_rather_than_evicting_this_sessions_work()
-> Result<()> {
    let when = at(START);
    let recorded = recorded_body()?;
    let body = rates_body(&recorded, at(FIRST_REFERENCE_DATE))?;
    let sleeper = Arc::new(RecordingSleeper::new());
    let (mut connector, mut runtime, path) = start_process(&body, &sleeper, when)?;
    poll_once(&mut runtime, &mut connector, &path, &body, when)?;
    let checkpoint = runtime.checkpoint(when);
    // Premise: the carry is non-empty, so the refusal below is about the state
    // of the window and not about there being nothing to install.
    assert_eq!(checkpoint.recent_fingerprints.len() as u64, RATES_PER_TABLE);

    let (mut second_connector, mut second_runtime, second_path) =
        start_process(&body, &sleeper, when)?;
    let report = poll_once(
        &mut second_runtime,
        &mut second_connector,
        &second_path,
        &body,
        when,
    )?;
    // Premise: this window has genuinely observed something, which is the
    // condition the refusal is about.
    assert_eq!(report.admitted.len() as u64, RATES_PER_TABLE);

    let error = second_runtime
        .resume(&mut second_connector, &checkpoint)
        .expect_err("a running window accepted a carry");
    assert!(
        error.message().contains("before the first poll"),
        "the refusal must name what to do instead: {}",
        error.message()
    );
    Ok(())
}

// --- seven days across eight processes ---------------------------------------

#[test]
fn seven_simulated_days_across_eight_processes_leave_one_ledger_with_both_time_axes_in_it()
-> Result<()> {
    let start = at(START);
    let clock = ManualClock::new(start);
    let sleeper = Arc::new(RecordingSleeper::new());
    let store: Arc<dyn KeyValueStore> = Arc::new(MemoryKeyValueStore::new());
    let recorded = recorded_body()?;
    let first_date = at(FIRST_REFERENCE_DATE);

    // Premise: the manifest's own delay is what makes the two time axes differ.
    // Sixteen hours is why a reference rate is unreadable for two thirds of the
    // date it is indexed by, and a delay of zero would make every assertion
    // about the event axis below true of the ingest axis too.
    let manifest = FrankfurterRatesConnector::shipped_manifest()?;
    assert_eq!(manifest.publication_delay(), Duration::from_hours(16));
    assert_eq!(manifest.poll_interval(), Duration::from_hours(1));

    let (mut journal, resumed) =
        StreamJournal::open(store.clone(), "frankfurter-ecb-reference-rates")?;
    assert!(resumed.is_none());
    let opening_body = rates_body(&recorded, first_date)?;
    let (mut connector, mut runtime, path) = start_process(&opening_body, &sleeper, start)?;

    let mut served_events: u64 = 0;
    let mut restarts: u64 = 0;

    for hour in 0..=HOURS {
        clock.set(start.saturating_add(Duration::from_hours(hour)));
        let now = clock.now();

        // The vendor starts serving tomorrow's table four hours before anybody
        // may act on it, which is what puts polls on both sides of the
        // point-in-time gate at every day boundary.
        let table = (hour + SERVED_EARLY_HOURS) / 24;
        let body = rates_body(
            &recorded,
            first_date.saturating_add(Duration::from_days(table)),
        )?;

        // The process dies at midday and a new one comes up, resuming from the
        // store alone — nothing but the checkpoint and the ledger crosses this
        // boundary.
        if hour > 0 && hour % 24 == RESTART_AT_HOUR {
            let checkpoint = runtime.checkpoint(now);
            journal.commit(&checkpoint)?;
            drop(runtime);
            drop(connector);
            drop(journal);

            let (reopened, resumed) =
                StreamJournal::open(store.clone(), "frankfurter-ecb-reference-rates")?;
            journal = reopened;
            let resumed = resumed.ok_or_else(|| {
                Error::invalid("a committed checkpoint was not offered back after the restart")
            })?;
            let (fresh_connector, mut fresh_runtime, fresh_path) =
                start_process(&body, &sleeper, now)?;
            connector = fresh_connector;
            let taken = fresh_runtime.resume(&mut connector, &resumed)?;
            assert_eq!(
                taken as u64, RATES_PER_TABLE,
                "the restart at hour {hour} took {taken} fingerprint(s) forward; a restart that \
                 takes none republishes the table it is about to be re-served"
            );
            runtime = fresh_runtime;
            assert_eq!(fresh_path, path);
            restarts = restarts.saturating_add(1);
        }

        let report = poll_once(&mut runtime, &mut connector, &path, &body, now)?;
        served_events = served_events.saturating_add(RATES_PER_TABLE);
        journal.record(&report, now)?;

        // The poll immediately after a restart is the one this file exists for:
        // it is served a table the previous process already absorbed, and it
        // must recognise every record of it.
        if hour > 0 && hour % 24 == RESTART_AT_HOUR {
            assert!(
                report.admitted.is_empty(),
                "the poll after the restart at hour {hour} released {} record(s) of a table its \
                 predecessor had already absorbed",
                report.admitted.len()
            );
            assert_eq!(report.duplicates, RATES_PER_TABLE);
        }
    }

    let final_checkpoint = runtime.checkpoint(clock.now());
    journal.commit(&final_checkpoint)?;
    let ledger = journal.ledger().clone();

    // --- the counts, each derived from the run's stated shape --------------
    assert_eq!(
        restarts, RESTARTS,
        "the run must actually have restarted seven times"
    );
    assert_eq!(
        ledger.sessions,
        RESTARTS + 1,
        "one session for the opening process and one for each restart"
    );
    assert_eq!(
        ledger.polls,
        (HOURS + 1) as u64,
        "hourly polls across seven days, both ends included"
    );
    assert_eq!(
        ledger.delivered, ledger.polls,
        "the emulator answered every poll"
    );
    assert_eq!(
        ledger.deferred, 0,
        "hourly polls are inside a one-per-minute rate limit"
    );
    assert_eq!(ledger.refused, 0);
    assert_eq!(ledger.quarantined, 0);

    // Eight reference dates, three rates each.
    assert_eq!(ledger.admitted, TABLES * RATES_PER_TABLE);
    // Seven day boundaries, four hours served early, three rates each.
    assert_eq!(
        ledger.withheld,
        RESTARTS * SERVED_EARLY_HOURS as u64 * RATES_PER_TABLE
    );
    // Nothing may be lost between the two: every event the emulator served was
    // admitted, recognised, withheld or quarantined, and this identity is what
    // makes the three counts above a partition rather than three numbers.
    assert_eq!(
        ledger.admitted + ledger.duplicates + ledger.withheld + ledger.quarantined,
        served_events,
        "the ledger accounts for {} of the {served_events} event(s) served",
        ledger.admitted + ledger.duplicates + ledger.withheld + ledger.quarantined
    );
    assert!(
        ledger.duplicates > ledger.admitted,
        "an hourly poll of a daily table is mostly redelivery; {} duplicate(s) against {} \
         admitted says the window stopped recognising them",
        ledger.duplicates,
        ledger.admitted
    );

    // --- the two axes ------------------------------------------------------
    //
    // Asserted as instants and not as extents. Both spans are seven days long,
    // so a run that reported the event axis under the ingest axis's name would
    // pass every extent check; the sixteen hours between the two `first`s is
    // the only thing that tells them apart, and it is the manifest's delay.
    let ingested = ledger
        .ingested
        .ok_or_else(|| Error::invalid("seven days of admissions left no ingest span"))?;
    let event = ledger
        .event
        .ok_or_else(|| Error::invalid("seven days of admissions left no event span"))?;
    assert_eq!(ingested.first, at("2026-09-04T16:00:00Z"));
    assert_eq!(ingested.last, at("2026-09-11T16:00:00Z"));
    assert_eq!(event.first, at("2026-09-04T00:00:00Z"));
    assert_eq!(event.last, at("2026-09-11T00:00:00Z"));
    assert_eq!(
        ingested.first.since(event.first),
        manifest.publication_delay(),
        "the gap between when a fact was true and when this platform could act on it is the \
         manifest's dissemination delay, and it is what a backtest reading the event axis alone \
         would trade through"
    );

    assert!(
        ledger.spans_at_least_days(7),
        "the ingest span is {:?} and the completion plan's bar is seven days",
        ingested.describe()
    );
    assert!(
        !ledger.spans_at_least_days(8),
        "a run of exactly seven days must not answer yes to eight, or the bar means nothing"
    );

    // --- the operator's line -----------------------------------------------
    let line = ledger.describe();
    for expected in [
        "frankfurter-ecb-reference-rates",
        "8 session(s)",
        "169 poll(s)",
        "2026-09-04T16:00:00.000Z .. 2026-09-11T16:00:00.000Z",
        "2026-09-04T00:00:00.000Z .. 2026-09-11T00:00:00.000Z",
    ] {
        assert!(
            line.contains(expected),
            "the ledger an operator reads must state {expected:?}: {line}"
        );
    }

    // --- and it is on the store, not in this process ------------------------
    //
    // The whole claim of the ledger is that it outlives the run. Reading it
    // back through a fresh journal is the only way to assert that rather than
    // assert the struct this test has been holding.
    drop(journal);
    let (reader, _) = StreamJournal::open(store.clone(), "frankfurter-ecb-reference-rates")?;
    let stored = reader.ledger();
    assert_eq!(stored.polls, ledger.polls);
    assert_eq!(stored.admitted, ledger.admitted);
    assert_eq!(stored.duplicates, ledger.duplicates);
    assert_eq!(stored.ingested, ledger.ingested);
    assert_eq!(stored.event, ledger.event);
    assert_eq!(
        stored.sessions,
        ledger.sessions + 1,
        "opening the ledger to read it is itself a session, and a record that hid that would let \
         a crash loop read as a clean run"
    );

    assert_eq!(
        sleeper.total(),
        Duration::ZERO,
        "seven simulated days must cost no real time"
    );
    Ok(())
}

// --- the bridge the composition roots actually construct ---------------------
//
// Everything above drives `ConnectorRuntime` and `StreamJournal` directly, in
// the right order, by hand. No production code did that: `qip-fastbrain` and
// `qip-api` construct a `ConnectorFeed` and call `poll`, and the four calls
// that make a restart resume — open the journal, resume the checkpoint, record
// the report, commit the position — had exactly one caller in the workspace,
// and it was this file. A control assembled only by its own test is the shape
// of a control, which is the failure `MaxExpectedShortfall` is the house
// example of. The test below drives the bridge instead.

/// The point-in-time discipline is not re-asserted here — `soak.rs` owns it.
/// What is asserted is that the ordering survives being moved a seam down.
#[test]
fn a_feed_given_a_store_resumes_the_previous_processs_window_and_one_without_a_store_republishes_the_table()
-> Result<()> {
    let first_poll = at(START);
    let second_poll = first_poll.saturating_add(Duration::from_hours(1));
    let recorded = recorded_body()?;
    let body = rates_body(&recorded, at(FIRST_REFERENCE_DATE))?;
    let store: Arc<dyn KeyValueStore> = Arc::new(MemoryKeyValueStore::new());

    // --- process one, through the bridge ------------------------------------
    let mut feed = feed_over_emulator(&body, first_poll)?;
    // Premise: a feed with no store keeps no ledger at all, so every assertion
    // below about a ledger is `journal_to`'s doing.
    assert!(
        feed.ledger().is_none(),
        "a feed nobody gave a store must report no ledger rather than a ledger of zeroes"
    );
    let taken = feed.journal_to(store.clone())?;
    assert_eq!(
        taken, 0,
        "an empty store carries nothing forward, or the suppression below would be resuming from \
         something this test did not write"
    );

    let released = feed.poll(first_poll)?;
    assert_eq!(
        released.len() as u64,
        RATES_PER_TABLE,
        "the recorded table must reach the loop as three observations, or this run proves nothing \
         about three being recognised again"
    );
    let ledger = feed
        .ledger()
        .ok_or_else(|| Error::invalid("a journalled feed must report its ledger"))?;
    assert_eq!(ledger.sessions, 1);
    assert_eq!(ledger.admitted, RATES_PER_TABLE);
    assert_eq!(
        ledger.duplicates, 0,
        "nothing has been redelivered yet, so a non-zero count here would mean the ledger is \
         counting something other than redeliveries"
    );
    // Killed rather than stopped: dropped without `shutdown`, which is the
    // eviction and the out-of-memory kill rather than the rollout. If the
    // position were committed at shutdown instead of at each poll, everything
    // below would fail — and the failures worth surviving are exactly the ones
    // that never reach a shutdown.
    drop(feed);

    // --- the control: the same second poll, on a feed with no store ---------
    //
    // Byte-for-byte the behaviour of every restart before this change. It is
    // here so the assertion after it cannot pass because the emulator stopped
    // serving, the knowability gate closed, or the rate limiter deferred.
    let mut blind = feed_over_emulator(&body, second_poll)?;
    let republished = blind.poll(second_poll)?;
    assert_eq!(
        republished.len() as u64,
        RATES_PER_TABLE,
        "without a restored window the identical table is republished in full — this is the \
         defect the wiring closes, and if this line ever fails the assertion below proves nothing"
    );
    drop(blind);

    // --- process two, given the same store ----------------------------------
    let mut feed = feed_over_emulator(&body, second_poll)?;
    let taken = feed.journal_to(store.clone())?;
    assert_eq!(
        taken as u64, RATES_PER_TABLE,
        "the whole table the last process absorbed must be taken back into the window"
    );

    let released = feed.poll(second_poll)?;
    assert!(
        released.is_empty(),
        "the identical table after a restart must be recognised, not republished; {} record(s) \
         reached the loop",
        released.len()
    );

    let ledger = feed
        .ledger()
        .ok_or_else(|| Error::invalid("a journalled feed must report its ledger"))?;
    assert_eq!(
        ledger.sessions, 2,
        "two processes carried this stream and the durable record must say so, or seven days \
         across two hundred sessions would read as a clean run"
    );
    assert_eq!(
        ledger.admitted, RATES_PER_TABLE,
        "the table was published once and must be admitted once across both processes"
    );
    assert_eq!(
        ledger.duplicates, RATES_PER_TABLE,
        "the second process's redeliveries must be counted, not merely dropped"
    );
    assert_eq!(
        ledger.duplicate_ratio(),
        Some(0.5),
        "three admitted and three duplicate is half, and a ratio computed off the wrong \
         denominator is how a stream that republished everything reads as a healthy one"
    );
    // Both axes, across the restart. The knowledge axis spans the two polls;
    // the world's axis does not move, because the ECB published one table.
    let ingested = ledger
        .ingested
        .ok_or_else(|| Error::invalid("records were admitted, so the ingest span exists"))?;
    assert_eq!(ingested.first, first_poll);
    assert_eq!(ingested.last, first_poll);
    let event = ledger
        .event
        .ok_or_else(|| Error::invalid("records were admitted, so the event span exists"))?;
    assert_eq!(event.first, at(FIRST_REFERENCE_DATE));
    assert_eq!(event.last, at(FIRST_REFERENCE_DATE));
    Ok(())
}

/// The two ways a root can get the ordering wrong, and what happens instead of
/// a partial restore.
#[test]
fn a_feed_refuses_a_second_journal_and_refuses_to_resume_a_window_it_has_already_polled()
-> Result<()> {
    let first_poll = at(START);
    let recorded = recorded_body()?;
    let body = rates_body(&recorded, at(FIRST_REFERENCE_DATE))?;
    let store: Arc<dyn KeyValueStore> = Arc::new(MemoryKeyValueStore::new());

    // --- a second journal on the same feed ----------------------------------
    let mut feed = feed_over_emulator(&body, first_poll)?;
    feed.journal_to(store.clone())?;
    let refused = feed
        .journal_to(store.clone())
        .expect_err("a second journal on one feed must be refused");
    assert!(
        refused.message().contains("already keeps a journal"),
        "the refusal must name the cause: {}",
        refused.message()
    );

    // --- resuming after the window has been used ----------------------------
    //
    // The order this protects is not hypothetical: `journal_to` is the call a
    // root adds last, and adding it after the poll loop rather than before it
    // is the natural mistake. A window half-restored would suppress some
    // redeliveries and republish others, and nothing downstream would show
    // which, so this refuses instead.
    let released = feed.poll(first_poll)?;
    assert_eq!(
        released.len() as u64,
        RATES_PER_TABLE,
        "the premise: this feed has genuinely observed the table, so the refusal below is about a \
         used window rather than an empty one"
    );
    feed.shutdown(first_poll.saturating_add(Duration::from_mins(30)))?;
    drop(feed);

    let mut late = feed_over_emulator(&body, first_poll)?;
    let released = late.poll(first_poll)?;
    assert_eq!(
        released.len() as u64,
        RATES_PER_TABLE,
        "the premise again, on the feed that will be asked to resume too late"
    );
    let refused = late
        .journal_to(store.clone())
        .expect_err("a resume after a poll must be refused rather than partially applied");
    assert!(
        !refused.message().is_empty(),
        "a refusal with no message tells a root nothing to do instead"
    );
    Ok(())
}

/// A whole feed over the emulator: the production assembly with the transport
/// swapped, which is the only difference between this and a deployment.
fn feed_over_emulator(body: &str, at: Timestamp) -> Result<ConnectorFeed> {
    let manifest = FrankfurterRatesConnector::shipped_manifest()?;
    let connector = FrankfurterRatesConnector::new(manifest.clone())?;
    let transport = Box::new(SourceEmulator::serving(&manifest.endpoint.path, body));
    ConnectorFeed::over_transport(
        Box::new(connector),
        manifest,
        transport,
        0x0EC8_0000_0000_0002,
        at,
    )
}
