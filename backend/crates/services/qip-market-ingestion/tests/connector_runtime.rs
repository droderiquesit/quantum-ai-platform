//! The runtime behaviours, each exercised on its own.
//!
//! Every test here drives a real [`ConnectorRuntime`] over recorded fixtures.
//! Nothing opens a socket, nothing reads a clock and nothing spends a backoff
//! in real seconds — the instant is passed in, the jitter is seeded, and the
//! waiting goes through `qip_transport::RecordingSleeper`, which records what
//! it was asked to wait and returns. A suite that had to spend the backoff is
//! a suite that stops asserting on it.

#![allow(clippy::panic_in_result_fn)]

mod connector_common;

use connector_common::{TestConnector, at, body, delayed_manifest, manifest, manifest_json};
use qip_core::error::Result;
use qip_core::rng::Xoshiro256;
use qip_core::{Duration, Timestamp};
use qip_market_ingestion::connector::emulator::{RecordedAnswer, RecordedExchange, SourceEmulator};
use qip_market_ingestion::connector::runtime::ConnectorRuntime;
use qip_market_ingestion::connector::transport::SourceTransport;
use qip_market_ingestion::connector::{
    Admission, BackoffLadder, Checkpoint, Cursor, CursorPosition, DedupWindow, EventFingerprint,
    FailureKind, FeedHeartbeat, Liveness, Novelty, PollOutcome, Quarantine, QuarantineReason,
    RateLimiter, RetryDecision, RuntimeConfig, SourceManifest,
};
use qip_transport::RecordingSleeper;
use std::sync::Arc;

/// Mid-session, so nothing depends on a session boundary.
fn now() -> Timestamp {
    at("2026-08-24T15:00:00Z")
}

/// A runtime whose waiting is recorded rather than spent.
fn runtime_with(manifest: SourceManifest) -> Result<(ConnectorRuntime, Arc<RecordingSleeper>)> {
    let sleeper = Arc::new(RecordingSleeper::new());
    let config = RuntimeConfig::seeded(0x1234_5678_9abc_def0)
        .with_sleeper(sleeper.clone())
        .with_dedup_capacity(64)
        .with_quarantine_capacity(16);
    Ok((ConnectorRuntime::new(manifest, config)?, sleeper))
}

fn emulator_serving(body: &str) -> SourceEmulator {
    SourceEmulator::new(vec![
        RecordedExchange::always("/v1/health", RecordedAnswer::json(200, "{}")),
        RecordedExchange::always("/v1/events", RecordedAnswer::json(200, body)),
    ])
}

// --- bounded exponential backoff with jitter --------------------------------

#[test]
fn a_backoff_never_exceeds_the_manifest_ceiling() -> Result<()> {
    // The whole point of jittering downward. A maximum backoff that can be
    // exceeded is a number that gets quoted in a runbook and is then wrong.
    let manifest = manifest();
    let ceiling = manifest.retry.policy().max_backoff;
    let mut rng = Xoshiro256::seeded(99);
    let policy = manifest.retry.policy();

    for attempt in 1..=12u32 {
        for _ in 0..200 {
            let wait = policy.backoff(attempt, &mut rng);
            assert!(
                wait <= ceiling,
                "attempt {attempt} waited {wait:?}, above the manifest's ceiling of {ceiling:?}"
            );
            assert!(
                wait.as_nanos() >= 0,
                "attempt {attempt} waited a negative span {wait:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn the_ladder_stops_after_the_attempt_count_the_manifest_permits() -> Result<()> {
    let manifest = manifest();
    let limit = manifest.retry.max_attempts;
    let mut ladder = BackoffLadder::new(manifest.retry.policy(), 5)?;
    let failure = FailureKind::Transient {
        detail: "the source is down".into(),
    };

    let mut retries = 0u32;
    for _ in 0..(limit * 4) {
        match ladder.failed(&failure) {
            RetryDecision::Retry { .. } => retries += 1,
            RetryDecision::GiveUp { attempts, reason } => {
                assert_eq!(
                    attempts, limit,
                    "the ladder gave up after {attempts} attempts and the manifest permits {limit}"
                );
                assert!(
                    reason.contains("the source is down"),
                    "the refusal does not quote what the source did, so a dead letter would say \
                     only that retries were exhausted: {reason}"
                );
                assert_eq!(
                    retries,
                    limit - 1,
                    "an unbounded retry is a poll loop that never reports a dead source"
                );
                return Ok(());
            }
        }
    }
    panic!("the ladder never gave up, which is a retry with no bound");
}

#[test]
fn jitter_spreads_two_replicas_and_still_replays_identically_from_one_seed() -> Result<()> {
    // Nine regional cells reconnecting after a rollout must not all retry on
    // the same millisecond; the same cell replayed must retry on exactly the
    // millisecond it did live. Both are properties of a *seeded* jitter.
    let policy = manifest().retry.policy();
    let schedule = |seed: u64| -> Vec<Duration> {
        let mut rng = Xoshiro256::seeded(seed);
        (1..=6u32).map(|n| policy.backoff(n, &mut rng)).collect()
    };

    assert_ne!(
        schedule(1),
        schedule(2),
        "two replicas drew the same ladder, so a rollout would have them retrying together"
    );
    assert_eq!(
        schedule(1),
        schedule(1),
        "the same seed drew a different ladder, so a replay would diverge from the run it replays"
    );
    Ok(())
}

#[test]
fn a_failure_that_retrying_cannot_fix_is_not_retried_at_all() -> Result<()> {
    // A rejected credential retried four times is this client hammering a
    // source over a mistake of ours.
    let mut ladder = BackoffLadder::new(manifest().retry.policy(), 5)?;
    let decision = ladder.failed(&FailureKind::Permanent {
        detail: "HTTP 401".into(),
    });

    match decision {
        RetryDecision::GiveUp { attempts, reason } => {
            assert_eq!(attempts, 1, "a permanent failure was retried");
            assert!(reason.contains("HTTP 401"), "{reason}");
        }
        RetryDecision::Retry { after, .. } => {
            panic!("a rejected credential was retried after {after:?}")
        }
    }
    Ok(())
}

#[test]
fn a_source_that_names_its_own_pause_gets_it_rather_than_our_guess() -> Result<()> {
    // The ladder is a guess about when the source will serve again. A
    // `retry-after` is not a guess.
    let mut ladder = BackoffLadder::new(manifest().retry.policy(), 5)?;
    let asked = Duration::from_secs(30);
    let decision = ladder.failed(&FailureKind::RateLimited {
        retry_after: Some(asked),
        detail: "HTTP 429".into(),
    });

    assert_eq!(
        decision.wait(),
        Some(asked),
        "the connector chose its own shorter backoff over the pause the source asked for, which \
         is a client arguing with a rate limiter it is going to lose to"
    );
    Ok(())
}

// --- rate-limit handling ----------------------------------------------------

#[test]
fn the_limiter_admits_the_burst_and_then_defers_rather_than_refusing() -> Result<()> {
    let manifest = manifest();
    let burst = manifest.rate_limit.burst;
    let mut limiter = RateLimiter::new(manifest.rate_limit)?;
    let start = now();

    for spent in 0..burst {
        assert!(
            limiter.admit(start).is_allowed(),
            "the limiter refused request {spent} of a burst of {burst}"
        );
    }
    match limiter.admit(start) {
        Admission::Allowed => panic!("the limiter admitted more than the burst it was given"),
        Admission::Deferred { until, wait } => {
            assert!(
                wait.as_nanos() > 0,
                "a deferral with no wait is a spin loop"
            );
            assert!(until > start);
        }
    }
    Ok(())
}

#[test]
fn a_deferred_request_is_admitted_once_the_bucket_has_refilled() -> Result<()> {
    let manifest = manifest();
    let mut limiter = RateLimiter::new(manifest.rate_limit)?;
    let start = now();
    for _ in 0..manifest.rate_limit.burst {
        let _ = limiter.admit(start);
    }
    let Admission::Deferred { until, .. } = limiter.admit(start) else {
        panic!("the bucket was not empty after its whole burst was spent");
    };

    assert!(
        limiter.admit(until).is_allowed(),
        "the limiter named {until} as the instant a token exists and then refused at it, which \
         would leave a caller spinning on an instant that never arrives"
    );
    Ok(())
}

#[test]
fn a_pause_the_source_asked_for_outlasts_the_limiter_s_own_refill() -> Result<()> {
    // A source can ask for a longer pause than its published rate implies.
    // Honouring only the bucket would walk straight back into the ban.
    let manifest = manifest();
    let mut limiter = RateLimiter::new(manifest.rate_limit)?;
    let start = now();
    let barrier = start.saturating_add(Duration::from_secs(30));
    limiter.pause_until(barrier);

    assert!(
        !limiter.admit(start).is_allowed(),
        "the limiter admitted a request during a pause the source asked for"
    );
    assert!(
        !limiter
            .admit(start.saturating_add(Duration::from_secs(29)))
            .is_allowed(),
        "the bucket refilled and overrode the source's own pause"
    );
    assert!(
        limiter.admit(barrier).is_allowed(),
        "the pause outlasted the instant the source named"
    );
    Ok(())
}

#[test]
fn a_source_answering_429_pauses_the_next_poll_instead_of_being_hammered() -> Result<()> {
    let manifest = manifest();
    let (mut runtime, sleeper) = runtime_with(manifest)?;
    let mut connector = TestConnector::new(connector_common::manifest());
    let mut emulator = SourceEmulator::new(vec![
        RecordedExchange::always("/v1/health", RecordedAnswer::json(200, "{}")),
        RecordedExchange::new(
            "/v1/events",
            vec![
                RecordedAnswer::rate_limited(30),
                RecordedAnswer::json(200, body(&[("e1", "2026-08-24T14:59:00Z", "101.25")])),
            ],
        ),
    ]);
    let transport: &mut dyn SourceTransport = &mut emulator;

    let first = runtime.poll(&mut connector, transport, now())?;
    assert!(first.attempts >= 2, "the 429 was not retried at all");
    assert_eq!(
        sleeper.recorded().len(),
        (first.attempts - 1) as usize,
        "the runtime retried without waiting, which is what turns a rate limit into a ban"
    );
    assert!(
        sleeper
            .recorded()
            .iter()
            .any(|w| *w >= Duration::from_secs(30)),
        "the retry ignored the source's `retry-after: 30`: {:?}",
        sleeper.recorded()
    );

    // The next poll must not walk into the same wall.
    let second = runtime.poll(&mut connector, transport, now())?;
    match second.outcome {
        PollOutcome::Deferred { until } => assert!(until > now()),
        other => panic!("the poll after a 429 went straight out again: {other:?}"),
    }
    Ok(())
}

// --- heartbeat and stale-feed detection -------------------------------------

#[test]
fn a_feed_that_has_never_answered_is_not_reported_as_stale() -> Result<()> {
    // A rollout is not an incident, and paging somebody for one teaches them
    // to ignore the page that matters.
    let heartbeat = FeedHeartbeat::new("test-source", Duration::from_mins(1));
    assert_eq!(heartbeat.liveness(now()), Liveness::NeverStarted);
    assert!(!heartbeat.liveness(now()).is_alarming());
    Ok(())
}

#[test]
fn a_source_that_stopped_answering_is_silent_rather_than_stale() -> Result<()> {
    let mut heartbeat = FeedHeartbeat::new("test-source", Duration::from_mins(1));
    heartbeat.answered(at("2026-08-24T14:50:00Z"), Some(at("2026-08-24T14:50:00Z")));

    match heartbeat.liveness(now()) {
        Liveness::Silent { quiet_for, .. } => assert_eq!(quiet_for, Duration::from_mins(10)),
        other => panic!(
            "a source that has answered nothing for ten minutes was reported as {other:?}, which \
             sends the on-call engineer to the provider instead of to the socket"
        ),
    }
    Ok(())
}

#[test]
fn a_source_answering_with_old_events_is_stale_rather_than_silent() -> Result<()> {
    let mut heartbeat = FeedHeartbeat::new("test-source", Duration::from_mins(1));
    // Answering right now, with an event from an hour ago.
    heartbeat.answered(now(), Some(at("2026-08-24T14:00:00Z")));

    match heartbeat.liveness(now()) {
        Liveness::Stale { behind_by, .. } => assert_eq!(behind_by, Duration::from_hours(1)),
        other => panic!(
            "a source that is up and has stopped producing was reported as {other:?}, which sends \
             the on-call engineer to the network instead of to the provider"
        ),
    }
    Ok(())
}

#[test]
fn a_source_re_serving_an_old_page_cannot_make_the_feed_look_more_stale() -> Result<()> {
    let mut heartbeat = FeedHeartbeat::new("test-source", Duration::from_hours(2));
    heartbeat.answered(now(), Some(at("2026-08-24T14:59:00Z")));
    heartbeat.answered(now(), Some(at("2026-08-24T09:00:00Z")));

    assert_eq!(
        heartbeat.newest_event(),
        Some(at("2026-08-24T14:59:00Z")),
        "an older page rewound the newest event, so a source replaying history would raise a \
         staleness alarm about data it had already delivered"
    );
    Ok(())
}

// --- checkpoint and resume --------------------------------------------------

#[test]
fn a_checkpoint_round_trips_through_json_and_resumes_to_the_same_cursor() -> Result<()> {
    let manifest = manifest();
    let cursor = Cursor::at_event_time(at("2026-08-24T14:59:00Z"));
    let checkpoint = Checkpoint::new(&manifest, cursor.clone(), now());

    let restored = Checkpoint::from_json(&checkpoint.to_json()?)?;
    assert_eq!(restored.resume_into(&manifest)?, cursor);
    Ok(())
}

#[test]
fn a_checkpoint_keeps_sub_millisecond_precision_through_json() -> Result<()> {
    // `Timestamp`'s own JSON form is RFC 3339 truncated to milliseconds, which
    // is right for a log line a human reads and wrong for a cursor: a source
    // whose event times carry microseconds would resume a fraction of a
    // millisecond early on every restart, re-reading events the dedup window
    // then absorbs in silence — a feed doing extra work with nothing to show.
    let manifest = manifest();
    let exact = at("2026-08-24T14:59:41.812734Z");
    let checkpoint = Checkpoint::new(&manifest, Cursor::at_event_time(exact), now());

    let restored = Checkpoint::from_json(&checkpoint.to_json()?)?;
    assert_eq!(
        restored.resume_into(&manifest)?.position.event_time(),
        Some(exact),
        "the cursor lost precision through JSON"
    );
    Ok(())
}

#[test]
fn a_checkpoint_belonging_to_another_source_is_refused_rather_than_resumed() -> Result<()> {
    let mine = manifest();
    let theirs = SourceManifest::from_json(&manifest_json("other-source", 0, "1.0"))?;
    let checkpoint = Checkpoint::new(&theirs, Cursor::at_event_time(now()), now());

    let error = checkpoint
        .resume_into(&mine)
        .expect_err("one source resumed from another's cursor");
    assert!(
        error.message().contains("other-source") && error.message().contains("test-source"),
        "the refusal does not name both sources, so an operator cannot tell which store was \
         crossed: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_checkpoint_written_under_an_incompatible_schema_is_refused() -> Result<()> {
    // A cursor means something only under the schema that produced it: a token
    // the source no longer understands, or an event time in a field that has
    // since been retyped.
    let old = SourceManifest::from_json(&manifest_json("test-source", 0, "1.0"))?;
    let new = SourceManifest::from_json(&manifest_json("test-source", 0, "2.0"))?;
    let checkpoint = Checkpoint::new(&old, Cursor::at_event_time(now()), now());

    let error = checkpoint
        .resume_into(&new)
        .expect_err("a cursor from schema 1.0 was reinterpreted under schema 2.0");
    assert!(
        error.message().contains("schema 1.0"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_minor_schema_bump_still_resumes_because_an_added_field_is_not_a_break() -> Result<()> {
    let old = SourceManifest::from_json(&manifest_json("test-source", 0, "1.0"))?;
    let newer = SourceManifest::from_json(&manifest_json("test-source", 0, "1.4"))?;
    let checkpoint = Checkpoint::new(&newer, Cursor::at_event_time(now()), now());

    checkpoint
        .resume_into(&old)
        .expect("a source that added a field stopped a connector from resuming");
    Ok(())
}

#[test]
fn a_cursor_does_not_rewind_when_a_source_re_serves_an_older_page() -> Result<()> {
    let cursor = Cursor::at_event_time(at("2026-08-24T15:00:00Z"));
    let rewound = cursor.advanced_to(
        CursorPosition::EventTime {
            at: at("2026-08-24T09:00:00Z"),
        },
        0,
    );

    assert_eq!(
        rewound.position.event_time(),
        Some(at("2026-08-24T15:00:00Z")),
        "an older page rewound the cursor, so the next fetch would re-read six hours the dedup \
         window would then absorb silently — a feed doing twice the work with nothing to show"
    );
    Ok(())
}

#[test]
fn a_runtime_resumes_from_its_own_checkpoint_and_keeps_its_place() -> Result<()> {
    let manifest = manifest();
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = emulator_serving(&body(&[("e1", "2026-08-24T14:59:00Z", "101.25")]));
    let transport: &mut dyn SourceTransport = &mut emulator;

    runtime.poll(&mut connector, transport, now())?;
    let taken = runtime.checkpoint(now());
    assert_eq!(taken.cursor.events_seen, 1);

    let (mut restarted, _sleeper) = runtime_with(connector_common::manifest())?;
    restarted.resume(&mut connector, &Checkpoint::from_json(&taken.to_json()?)?)?;
    assert_eq!(
        restarted.cursor().position.event_time(),
        Some(at("2026-08-24T14:59:00Z")),
        "the restarted runtime did not pick up the position the checkpoint held"
    );
    Ok(())
}

// --- deduplication and idempotency ------------------------------------------

#[test]
fn the_same_event_served_twice_is_published_once() -> Result<()> {
    // The behaviour every overlapping poll window depends on. A source with no
    // cursor re-serves its last page on every call, and without this each poll
    // would publish the same trade again — a price that appears to have traded
    // once a second because nothing changed.
    let manifest = manifest();
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = emulator_serving(&body(&[("e1", "2026-08-24T14:59:00Z", "101.25")]));
    let transport: &mut dyn SourceTransport = &mut emulator;

    let first = runtime.poll(&mut connector, transport, now())?;
    assert_eq!(first.admitted.len(), 1, "the first poll published nothing");
    assert_eq!(first.duplicates, 0);

    let second = runtime.poll(&mut connector, transport, now())?;
    assert!(
        second.admitted.is_empty(),
        "the same event was published twice: {} envelope(s) came out of a re-served page",
        second.admitted.len()
    );
    assert_eq!(
        second.duplicates, 1,
        "the redelivery was not recognised as one, so nothing downstream can tell it apart"
    );
    Ok(())
}

#[test]
fn a_corrected_event_is_not_mistaken_for_a_redelivery_of_the_one_it_corrects() -> Result<()> {
    // A correction carries the same key and the same instant as the original
    // and a different value. A fingerprint over the key alone would swallow it.
    let manifest = manifest();
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = SourceEmulator::new(vec![
        RecordedExchange::always("/v1/health", RecordedAnswer::json(200, "{}")),
        RecordedExchange::new(
            "/v1/events",
            vec![
                RecordedAnswer::json(200, body(&[("e1", "2026-08-24T14:59:00Z", "101.25")])),
                RecordedAnswer::json(200, body(&[("e1", "2026-08-24T14:59:00Z", "101.75")])),
            ],
        ),
    ]);
    let transport: &mut dyn SourceTransport = &mut emulator;

    runtime.poll(&mut connector, transport, now())?;
    let corrected = runtime.poll(&mut connector, transport, now())?;
    assert_eq!(
        corrected.admitted.len(),
        1,
        "a corrected price was dropped as a duplicate of the price it corrects"
    );
    assert_eq!(corrected.duplicates, 0);
    Ok(())
}

#[test]
fn two_sources_reporting_one_event_are_two_facts_rather_than_a_duplicate() -> Result<()> {
    // Collapsing them would hide a disagreement between providers, which is
    // exactly the thing worth seeing.
    let body = serde_json::json!({"price": "101.25"});
    let first = EventFingerprint::of("source-a", "1.0", "e1", now(), &body);
    let second = EventFingerprint::of("source-b", "1.0", "e1", now(), &body);

    assert_ne!(first, second);
    Ok(())
}

#[test]
fn a_key_containing_the_encoding_s_separator_cannot_forge_another_event() -> Result<()> {
    // Length prefixes rather than a delimiter: with `a|b` concatenation, a key
    // of `x:1` and a key of `x` with a time starting `1` would collide.
    let body = serde_json::json!({});
    let first = EventFingerprint::of("s", "1.0", "12:ab", now(), &body);
    let second = EventFingerprint::of("s", "1.0", "12", now(), &body);

    assert_ne!(first, second);
    Ok(())
}

#[test]
fn the_dedup_window_evicts_rather_than_growing_without_bound() -> Result<()> {
    // An unbounded set of every fingerprint ever seen is a process that dies
    // of memory during the incident where the source replays its history.
    let mut window = DedupWindow::new(4)?;
    let body = serde_json::json!({});
    let mark = |n: u32| EventFingerprint::of("s", "1.0", &n.to_string(), now(), &body);

    for n in 0..10 {
        assert_eq!(window.observe(&mark(n)), Novelty::New);
    }
    assert_eq!(window.len(), 4, "the window grew past its capacity");
    assert_eq!(
        window.evicted(),
        6,
        "eviction happened without being counted"
    );
    assert!(
        !window.contains(&mark(0)),
        "the oldest fingerprint survived, so the newest must have been dropped instead"
    );
    assert!(window.contains(&mark(9)));
    Ok(())
}

// --- event time versus ingest time ------------------------------------------

#[test]
fn an_event_is_withheld_until_the_source_s_dissemination_delay_has_passed() -> Result<()> {
    // Not `event_time <= horizon`: the delay is what separates a record that
    // exists from one the deployment was entitled to see.
    let manifest = delayed_manifest(15 * 60 * 1000);
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = emulator_serving(&body(&[("e1", "2026-08-24T14:59:00Z", "101.25")]));
    let transport: &mut dyn SourceTransport = &mut emulator;

    let early = runtime.poll(&mut connector, transport, now())?;
    assert!(
        early.admitted.is_empty(),
        "a record on a fifteen-minute delayed feed was published one minute after the event, \
         which is a backtest reading the future"
    );
    assert_eq!(early.withheld, 1);
    Ok(())
}

#[test]
fn a_withheld_event_is_delivered_by_a_later_poll_rather_than_swallowed_as_a_duplicate() -> Result<()>
{
    // The ordering inside the runtime that this holds: knowability is checked
    // *before* the dedup window records the fingerprint. Recording it first
    // would mark the event seen while withholding it, and the poll that was
    // supposed to deliver it would drop it — a record lost with every counter
    // reading zero.
    let manifest = delayed_manifest(15 * 60 * 1000);
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = emulator_serving(&body(&[("e1", "2026-08-24T14:59:00Z", "101.25")]));
    let transport: &mut dyn SourceTransport = &mut emulator;

    let early = runtime.poll(&mut connector, transport, now())?;
    assert_eq!(early.withheld, 1);

    let later = runtime.poll(&mut connector, transport, at("2026-08-24T15:20:00Z"))?;
    assert_eq!(
        later.admitted.len(),
        1,
        "the withheld record never arrived: it was fingerprinted while being withheld and then \
         dropped as a duplicate by the very poll that should have delivered it"
    );
    Ok(())
}

#[test]
fn a_withheld_event_does_not_advance_the_cursor_past_itself() -> Result<()> {
    // A connector with a resume position asks a source for events after the
    // cursor — that is the documented pattern in `SourceConnector::fetch_request`
    // for any source with a cursor. If a withheld event's time were folded
    // into that cursor, the next poll would ask only for events after an
    // instant the deployment was never entitled to see the first one at, and
    // the record would never be asked for again once it finally became
    // knowable — a silent, permanent loss dressed up as `withheld`, whose own
    // doc comment promises "not a loss: the next poll's window covers them
    // again". This batch carries one admitted event and one withheld event
    // newer than it, on purpose, so the cursor's own position after the poll
    // — not just what the poll delivered — can be checked directly.
    let manifest = delayed_manifest(15 * 60 * 1000);
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = emulator_serving(&body(&[
        ("admitted", "2026-08-24T14:00:00Z", "101.25"),
        ("withheld", "2026-08-24T14:59:00Z", "101.30"),
    ]));
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, now())?;
    assert_eq!(report.admitted.len(), 1, "the premise: one event admitted");
    assert_eq!(report.withheld, 1, "the premise: one event withheld");

    assert_eq!(
        runtime.cursor().position.event_time(),
        Some(at("2026-08-24T14:00:00Z")),
        "the cursor moved to the withheld event's time rather than stopping at the last \
         admitted one, so a cursor-based fetch_request would never ask for the withheld \
         event again"
    );
    Ok(())
}

#[test]
fn the_envelope_keeps_event_time_ingest_time_and_knowable_time_apart() -> Result<()> {
    let manifest = delayed_manifest(15 * 60 * 1000);
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = emulator_serving(&body(&[("e1", "2026-08-24T14:00:00Z", "101.25")]));
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, now())?;
    let envelope = report
        .admitted
        .first()
        .expect("an event an hour old on a fifteen-minute feed is knowable");

    assert_eq!(envelope.event_time(), at("2026-08-24T14:00:00Z"));
    assert_eq!(
        envelope.ingest_time(),
        now(),
        "the ingest time was not the caller's horizon"
    );
    assert_eq!(envelope.knowable_at(), at("2026-08-24T14:15:00Z"));
    assert_eq!(envelope.ingestion_lag(), Duration::from_hours(1));
    assert_eq!(
        envelope.provenance().source,
        "test-source",
        "the record does not carry the source that produced it"
    );
    assert!(
        envelope.is_decision_grade(),
        "a public, clean record was not decision-grade"
    );
    Ok(())
}

// --- schema validation and version check ------------------------------------

#[test]
fn a_payload_missing_a_required_field_is_quarantined_rather_than_decoded() -> Result<()> {
    let manifest = manifest();
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = emulator_serving(r#"{"records":[]}"#);
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, now())?;
    assert_eq!(report.quarantined, 1);
    assert!(report.admitted.is_empty());
    let held = runtime
        .quarantine()
        .recent(1)
        .first()
        .copied()
        .cloned()
        .expect("the payload was refused and nothing was held");
    assert!(
        matches!(held.reason, QuarantineReason::SchemaViolation { .. }),
        "a payload with no `events` array was held as {:?} rather than as a schema violation",
        held.reason
    );
    Ok(())
}

#[test]
fn a_source_declaring_a_new_major_version_is_refused_rather_than_decoded() -> Result<()> {
    // The fields keep their names and change their meaning, so decoding would
    // produce records that look ordinary and are wrong.
    let manifest = manifest();
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = emulator_serving(
        r#"{"schema_version":"2.0","events":[{"key":"e1","at":"2026-08-24T14:59:00Z","price":"101.25"}]}"#,
    );
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, now())?;
    assert!(report.admitted.is_empty());
    let held = runtime
        .quarantine()
        .recent(1)
        .first()
        .copied()
        .cloned()
        .expect("a version mismatch was not held");
    assert!(
        matches!(held.reason, QuarantineReason::VersionMismatch { .. }),
        "held as {:?}",
        held.reason
    );
    Ok(())
}

#[test]
fn a_source_that_adds_a_field_does_not_stop_the_feed() -> Result<()> {
    // A source adding a field is not a fault, and a connector that stopped for
    // one would be a feed that goes down whenever a provider ships.
    let manifest = manifest();
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = emulator_serving(
        r#"{"schema_version":"1.3","cursor":"opaque","events":[{"key":"e1","at":"2026-08-24T14:59:00Z","price":"101.25","exchange":"new"}]}"#,
    );
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, now())?;
    assert_eq!(report.admitted.len(), 1, "an added field stopped the feed");
    assert_eq!(report.quarantined, 0);
    Ok(())
}

#[test]
fn a_price_that_arrives_as_a_json_number_is_refused_rather_than_rounded() -> Result<()> {
    // A price sent as a JSON float has already lost precision by the time this
    // code sees it. A presence check would admit it.
    let manifest = SourceManifest::from_json(&manifest_json("test-source", 0, "1.0").replace(
        r#"{ "path": "events", "kind": "array" }"#,
        r#"{ "path": "events.0.price", "kind": "decimal_string" }"#,
    ))?;
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator =
        emulator_serving(r#"{"events":[{"key":"e1","at":"2026-08-24T14:59:00Z","price":101.25}]}"#);
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, now())?;
    assert_eq!(
        report.quarantined, 1,
        "a price that arrived as a double was accepted as an exact one"
    );
    Ok(())
}

// --- quarantine and dead-lettering ------------------------------------------

#[test]
fn a_batch_whose_retries_are_exhausted_becomes_a_dead_letter_rather_than_a_silent_gap() -> Result<()>
{
    // The alternative is a poll that returns nothing and says nothing: the
    // record count falls, no error fires, and the first symptom is a model
    // trained on a feed that quietly halved.
    let manifest = manifest();
    let attempts_permitted = manifest.retry.max_attempts;
    let (mut runtime, sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = SourceEmulator::new(vec![
        RecordedExchange::always("/v1/health", RecordedAnswer::json(200, "{}")),
        RecordedExchange::always(
            "/v1/events",
            RecordedAnswer::json(503, r#"{"error":"the source is down"}"#),
        ),
    ]);
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, now())?;

    assert_eq!(report.outcome, PollOutcome::Refused);
    assert_eq!(
        report.attempts, attempts_permitted,
        "the runtime made {} attempts and the manifest permits {attempts_permitted}",
        report.attempts
    );
    assert_eq!(
        sleeper.recorded().len(),
        (attempts_permitted - 1) as usize,
        "the retries did not wait, which is a client hammering a source that is already failing"
    );
    assert_eq!(
        runtime.quarantine().len(),
        1,
        "a source that failed every attempt produced no dead letter, so the outage is a gap in \
         the data with nothing anywhere saying why"
    );
    let held = runtime
        .quarantine()
        .recent(1)
        .first()
        .copied()
        .cloned()
        .expect("the dead letter exists");
    match &held.reason {
        QuarantineReason::RetriesExhausted { attempts, detail } => {
            assert_eq!(*attempts, attempts_permitted);
            assert!(
                detail.contains("503"),
                "the dead letter does not say what the source did: {detail}"
            );
        }
        other => panic!("an exhausted batch was held as {other:?}"),
    }
    Ok(())
}

#[test]
fn a_quarantined_event_becomes_a_data_quality_failure_the_platform_already_alarms_on() -> Result<()>
{
    let mut quarantine = Quarantine::new("test-source", 8)?;
    quarantine.hold(
        "e1",
        None,
        QuarantineReason::ValidationFailure {
            issues: vec!["non-positive tick price -1".into()],
        },
        r#"{"price":"-1"}"#,
        now(),
    );

    let failures = quarantine.as_quality_failures("market.tick");
    let failure = failures.first().expect("one held event, one failure");
    assert_eq!(failure.source, "test-source");
    assert_eq!(failure.subject_id.as_deref(), Some("e1"));
    assert!(
        failure.rejected,
        "a quarantined event was reported as admitted, so it would be counted as an input"
    );
    assert_eq!(
        failure.issues,
        vec!["non-positive tick price -1".to_string()]
    );
    Ok(())
}

#[test]
fn the_quarantine_counts_what_it_had_to_drop_rather_than_hiding_the_overflow() -> Result<()> {
    // A source that has broken its schema breaks it for every event, so the
    // store fills. "We are losing dead letters too" has to be visible itself.
    let mut quarantine = Quarantine::new("test-source", 2)?;
    for n in 0..5 {
        quarantine.hold(
            format!("e{n}"),
            None,
            QuarantineReason::DecodeFailure {
                detail: "not JSON".into(),
            },
            "<html>",
            now(),
        );
    }

    assert_eq!(quarantine.len(), 2);
    assert_eq!(quarantine.overflowed(), 3);
    assert_eq!(
        quarantine.count_of(&QuarantineReason::DecodeFailure {
            detail: String::new()
        }),
        5,
        "the per-reason count did not survive eviction, so the size of the outage is lost with \
         the entries"
    );
    Ok(())
}

#[test]
fn a_body_that_is_not_json_is_held_with_an_excerpt_rather_than_dropped() -> Result<()> {
    let manifest = manifest();
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = SourceEmulator::new(vec![
        RecordedExchange::always("/v1/health", RecordedAnswer::json(200, "{}")),
        RecordedExchange::always(
            "/v1/events",
            RecordedAnswer::json(200, "<html><body>maintenance</body></html>"),
        ),
    ]);
    let transport: &mut dyn SourceTransport = &mut emulator;

    let report = runtime.poll(&mut connector, transport, now())?;
    let held = runtime
        .quarantine()
        .recent(1)
        .first()
        .copied()
        .cloned()
        .expect("an unparseable body was dropped rather than held");
    assert_eq!(report.quarantined, 1);
    assert!(
        held.payload_excerpt.contains("maintenance"),
        "the dead letter does not quote what arrived, so nobody can tell an outage page from a \
         schema change: {}",
        held.payload_excerpt
    );
    Ok(())
}

// --- connect is loud ---------------------------------------------------------

#[test]
fn a_source_that_will_not_answer_its_health_check_fails_at_connect_not_at_the_first_poll()
-> Result<()> {
    // A deployment missing a credential should fail while somebody is watching
    // the rollout, not an hour later inside a poll loop where it looks like a
    // feed that has gone quiet.
    let manifest = manifest();
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = SourceEmulator::new(vec![RecordedExchange::always(
        "/v1/health",
        RecordedAnswer::json(401, r#"{"error":"unauthorized"}"#),
    )]);
    let transport: &mut dyn SourceTransport = &mut emulator;

    let error = runtime
        .connect(&mut connector, transport, now())
        .expect_err("a source rejecting the health check was reported as connected");
    assert!(error.message().contains("401"), "{}", error.message());
    assert!(!runtime.is_connected());
    Ok(())
}

#[test]
fn a_health_check_does_not_spend_a_rate_limit_token() -> Result<()> {
    // A limiter that refused the health check would make a saturated feed
    // indistinguishable from a dead one, which is exactly when the answer
    // matters.
    let manifest = manifest();
    let burst = manifest.rate_limit.burst;
    let (mut runtime, _sleeper) = runtime_with(manifest.clone())?;
    let mut connector = TestConnector::new(manifest);
    let mut emulator = emulator_serving(&body(&[("e1", "2026-08-24T14:59:00Z", "101.25")]));
    let transport: &mut dyn SourceTransport = &mut emulator;

    for _ in 0..(burst * 2) {
        let _ = runtime.health(&mut connector, transport, now())?;
    }
    assert_eq!(
        runtime.limiter().available(now()),
        burst,
        "health checks drained the bucket the fetches need"
    );
    Ok(())
}
