//! Tests for the live surface.
//!
//! The properties worth holding: that a frame is syntactically an event and
//! cannot be split by its own payload, that a quiet stream still proves it is
//! alive, that a client leaving ends the loop instead of raising an incident,
//! that the two numbers on every event mean what the contract says they mean,
//! and that a stream is not a way around the authorisation in front of the
//! REST route serving the same data.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_api::auth::{Authenticator, Credential, RateLimiter, Role};
use qip_api::cells::CellRegistry;
use qip_api::http::{Handler, Method, Request, Response, Server, ServerLimits, StreamDecision};
use qip_api::routes::{Api, ROUTES};
use qip_api::stream::{
    Emission, EventSource, EventStream, LAST_EVENT_ID, Poll, Resume, SseEvent, StreamKind,
    StreamLimits, select,
};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, CorrelationId, EventId, Lineage, ManualClock};
use qip_events::envelope::AnyEvent;
use qip_events::log::LogRecord;
use qip_events::topic::Topic;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(now()))
}

/// Limits that make a live connection observable inside a test.
///
/// The production defaults heartbeat every ten seconds and close after five
/// minutes, which are right for a browser behind a proxy and wrong for a test:
/// a test that had to wait out the real lifetime bound to prove the connection
/// ends is a test that gets deleted.
fn brisk() -> StreamLimits {
    StreamLimits {
        poll_interval: std::time::Duration::from_millis(1),
        heartbeat_after: std::time::Duration::from_millis(5),
        max_duration: std::time::Duration::from_millis(150),
        max_events_per_poll: 16,
        backlog: 8,
        retry_after_millis: 1_000,
    }
}

// --- a source the test drives ------------------------------------------------

/// A source that hands out exactly what the test scripted.
///
/// Lets the framing, the numbering and the ending be checked without a
/// platform, so a failure names the stream machinery rather than whatever the
/// intelligence loop happened to emit that run.
#[derive(Debug)]
struct Scripted {
    resume: Resume,
    polls: VecDeque<Poll>,
    /// Returned once the script runs out. `Poll::Events(vec![])` keeps the
    /// stream alive and quiet, which is what a real idle source looks like.
    then: Poll,
}

impl Scripted {
    fn quiet() -> Self {
        Self {
            resume: Resume {
                cursor: 0,
                skipped: Some(0),
                note: "a scripted source".to_string(),
            },
            polls: VecDeque::new(),
            then: Poll::Events(Vec::new()),
        }
    }

    fn delivering(batches: Vec<Vec<Emission>>) -> Self {
        Self {
            polls: batches.into_iter().map(Poll::Events).collect(),
            ..Self::quiet()
        }
    }
}

impl EventSource for Scripted {
    fn resume(&mut self, _after: Option<u64>) -> Resume {
        self.resume.clone()
    }

    fn since(&mut self, _after: u64, _limit: usize) -> Poll {
        self.polls.pop_front().unwrap_or_else(|| self.then.clone())
    }
}

fn emission(cursor: u64, event_type: &str, payload: serde_json::Value) -> Emission {
    Emission {
        cursor,
        event_type: event_type.to_string(),
        occurred_at: now(),
        ingested_at: now().saturating_add(Duration::from_millis(7)),
        correlation_id: format!("corr-{cursor}"),
        payload,
    }
}

fn open(source: Scripted, limits: StreamLimits, last_event_id: Option<&str>) -> EventStream {
    EventStream::open(
        StreamKind::Orders,
        Box::new(source),
        clock(),
        limits,
        last_event_id,
    )
}

/// Every frame a stream produces until it ends, as text.
fn drain(stream: &mut EventStream) -> Vec<String> {
    let mut frames = Vec::new();
    let mut body = qip_api::ResponseStream::next_frame(stream);
    while let Some(frame) = body {
        frames.push(String::from_utf8(frame).expect("frames are UTF-8"));
        body = qip_api::ResponseStream::next_frame(stream);
    }
    frames
}

/// The `data:` object of every frame that carries one.
fn events(frames: &[String]) -> Vec<serde_json::Value> {
    frames
        .iter()
        .filter_map(|frame| {
            let data = frame.lines().find_map(|line| line.strip_prefix("data: "))?;
            serde_json::from_str(data).ok()
        })
        .collect()
}

// --- framing -----------------------------------------------------------------

#[test]
fn an_event_frame_is_well_formed_server_sent_event_syntax() {
    // Three fields and a blank line. A client parses this with a two-line
    // state machine, and anything else it silently drops.
    let event = SseEvent {
        stream: "orders",
        sequence: 3,
        emission: emission(41, "order.filled", serde_json::json!({"order": "ord-1"})),
    };
    let frame = event.frame();

    assert!(frame.starts_with("id: 41\n"), "{frame:?}");
    assert!(frame.contains("\nevent: order.filled\n"), "{frame:?}");
    assert!(
        frame.ends_with("\n\n"),
        "an event ends with a blank line: {frame:?}"
    );
    assert_eq!(
        frame.matches("\n\n").count(),
        1,
        "one event is one frame: {frame:?}"
    );

    let data: serde_json::Value = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .map(serde_json::from_str)
        .expect("a data line")
        .expect("valid JSON");
    assert_eq!(data["stream"], serde_json::json!("orders"));
    assert_eq!(data["type"], serde_json::json!("order.filled"));
    assert_eq!(data["sequence"], serde_json::json!(3));
    assert_eq!(data["cursor"], serde_json::json!(41));
    assert_eq!(data["correlation_id"], serde_json::json!("corr-41"));
    // Both clocks, and they are not the same clock: the gap between them is
    // the ingestion latency, and a client that cannot see it cannot tell a
    // slow feed from a quiet market.
    assert_eq!(data["event_time"], serde_json::json!(now().to_rfc3339()));
    assert_eq!(
        data["ingest_time"],
        serde_json::json!(now().saturating_add(Duration::from_millis(7)).to_rfc3339())
    );
    assert_eq!(data["payload"]["order"], serde_json::json!("ord-1"));
}

#[test]
fn a_payload_containing_a_newline_cannot_split_one_event_into_two_frames() {
    // A `data:` line ends at the first newline. A rationale, an error message
    // or an agent's reasoning containing one would hand the client half an
    // object and make the remainder look like a second, malformed event.
    let event = SseEvent {
        stream: "orders",
        sequence: 1,
        emission: emission(
            1,
            "order.rejected",
            serde_json::json!({"reason": "refused\nby: risk\r\ndata: injected"}),
        ),
    };
    let frame = event.frame();

    assert_eq!(
        frame.matches("\n\n").count(),
        1,
        "the payload split the frame: {frame:?}"
    );
    let data = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("a data line");
    assert!(!data.contains('\n') && !data.contains('\r'), "{data:?}");
    let decoded: serde_json::Value = serde_json::from_str(data).expect("valid JSON");
    assert_eq!(
        decoded["payload"]["reason"],
        serde_json::json!("refused\nby: risk\r\ndata: injected"),
        "the newline must survive as content, escaped, not as framing"
    );
}

#[test]
fn every_event_carries_the_six_fields_a_client_needs_to_detect_a_gap_and_reconnect() {
    // The contract the descriptor advertises, checked against what is actually
    // sent. A field named in the descriptor and absent from the wire is worse
    // than one that was never promised.
    let mut stream = open(
        Scripted::delivering(vec![vec![emission(
            9,
            "order.submitted",
            serde_json::json!({}),
        )]]),
        brisk(),
        None,
    );
    let frames = drain(&mut stream);
    let event = events(&frames).into_iter().next().expect("an event");
    for field in [
        "type",
        "sequence",
        "cursor",
        "event_time",
        "ingest_time",
        "correlation_id",
    ] {
        assert!(!event[field].is_null(), "{field} is missing from {event}");
        assert!(
            StreamKind::Orders.descriptor().contains(field),
            "{field} is sent but not declared in the descriptor"
        );
    }
}

// --- the connection ----------------------------------------------------------

#[test]
fn a_stream_opens_with_a_reconnect_delay_and_a_note_saying_what_it_is_replaying() {
    let mut stream = open(Scripted::quiet(), brisk(), None);
    let first = qip_api::ResponseStream::next_frame(&mut stream).expect("an opening frame");
    let text = String::from_utf8(first).expect("UTF-8");
    // `retry:` is the field that tells a browser how long to wait before
    // reconnecting. Without it every client uses its own default, and a
    // deployment cannot slow a reconnect storm down.
    assert!(text.contains("retry: 1000"), "{text:?}");
    assert!(text.starts_with(':'), "the note is a comment: {text:?}");
    assert!(text.contains("a scripted source"), "{text:?}");
}

#[test]
fn a_quiet_stream_emits_a_heartbeat_comment_rather_than_going_silent() {
    // The failure this prevents: a proxy with a thirty- or sixty-second idle
    // timeout closes a stream that both ends still believe is open, and the
    // client sees a feed that stopped without an error.
    let mut stream = open(Scripted::quiet(), brisk(), None);
    let frames = drain(&mut stream);
    let heartbeats: Vec<&String> = frames
        .iter()
        .filter(|frame| frame.starts_with(": heartbeat"))
        .collect();
    assert!(
        !heartbeats.is_empty(),
        "a source that produced nothing must still prove the connection is alive: {frames:?}"
    );
    // A comment, so a client never has to understand or de-duplicate it.
    for heartbeat in &heartbeats {
        assert!(heartbeat.ends_with("\n\n"), "{heartbeat:?}");
        assert!(!heartbeat.contains("\ndata:"), "{heartbeat:?}");
    }
}

#[test]
fn sequence_numbers_increase_by_exactly_one_on_every_delivered_event() {
    // The two numbers do different jobs. Cursors are log positions, so on a
    // filtered stream they are sparse by construction — the log interleaves
    // every topic. The sequence is the count of what this connection was sent,
    // so a hole in it is a genuine loss and nothing else.
    let mut stream = open(
        Scripted::delivering(vec![
            vec![
                emission(10, "order.proposed", serde_json::json!({})),
                emission(20, "order.approved", serde_json::json!({})),
            ],
            vec![emission(35, "order.filled", serde_json::json!({}))],
        ]),
        brisk(),
        None,
    );
    let frames = drain(&mut stream);
    let delivered: Vec<(u64, u64)> = events(&frames)
        .iter()
        .filter(|event| {
            event["type"]
                .as_str()
                .is_some_and(|t| t.starts_with("order."))
        })
        .filter_map(|event| Some((event["sequence"].as_u64()?, event["cursor"].as_u64()?)))
        .collect();

    assert_eq!(
        delivered,
        vec![(1, 10), (2, 20), (3, 35)],
        "sequences must be contiguous while cursors follow the log: {delivered:?}"
    );
    for pair in delivered.windows(2) {
        assert_eq!(
            pair[1].0,
            pair[0].0 + 1,
            "a sequence skipped: {delivered:?}"
        );
        assert!(
            pair[1].1 > pair[0].1,
            "a cursor went backwards: {delivered:?}"
        );
    }
}

#[test]
fn a_connection_ends_at_its_lifetime_bound_naming_the_cursor_to_resume_from() {
    // A stream with no end is a thread, a socket and a slot against the
    // server's concurrency limit held for as long as the process runs.
    let mut stream = open(
        Scripted::delivering(vec![vec![emission(
            77,
            "order.filled",
            serde_json::json!({}),
        )]]),
        brisk(),
        None,
    );
    let frames = drain(&mut stream);
    let closing = events(&frames)
        .into_iter()
        .find(|event| event["type"] == serde_json::json!("stream.closing"))
        .expect("the connection must say why it ended");
    assert_eq!(
        closing["payload"]["resume_from"],
        serde_json::json!(77),
        "the closing event must name the cursor a client resumes from: {closing}"
    );
    assert!(
        closing["payload"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("Last-Event-ID")),
        "{closing}"
    );
}

#[test]
fn a_source_that_cannot_be_read_ends_the_stream_with_a_fault_rather_than_going_quiet() {
    // An empty batch means nothing happened; a fault means nobody can tell
    // whether anything happened. A stream that reported the two identically
    // would render a crashed platform as a calm market.
    let mut source = Scripted::quiet();
    source.then = Poll::Faulted("the platform is in an inconsistent state".to_string());
    let mut stream = open(source, brisk(), None);
    let frames = drain(&mut stream);
    let fault = events(&frames)
        .into_iter()
        .find(|event| event["type"] == serde_json::json!("stream.fault"))
        .expect("a fault must be an event the client cannot miss");
    assert!(
        fault["payload"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("inconsistent state")),
        "{fault}"
    );
    assert!(
        !frames.iter().any(|frame| frame.starts_with(": heartbeat")),
        "a faulted stream must not keep heartbeating as though it were healthy: {frames:?}"
    );
}

// --- the disconnect ----------------------------------------------------------

/// A sink that stops accepting writes, the way a socket does when its peer has
/// gone.
#[derive(Debug)]
struct GoesAway {
    accepted: usize,
    limit: usize,
}

impl Write for GoesAway {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.accepted >= self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the client went away",
            ));
        }
        self.accepted += 1;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A source that always has one more event, so only the sink can end the loop.
#[derive(Debug)]
struct Endless(u64);

impl EventSource for Endless {
    fn resume(&mut self, _after: Option<u64>) -> Resume {
        Resume {
            cursor: 0,
            skipped: Some(0),
            note: "endless".to_string(),
        }
    }

    fn since(&mut self, _after: u64, _limit: usize) -> Poll {
        self.0 += 1;
        Poll::Events(vec![emission(
            self.0,
            "order.filled",
            serde_json::json!({}),
        )])
    }
}

#[test]
fn a_client_that_stops_reading_ends_the_send_loop_and_is_reported_as_a_disconnect() {
    // The disconnect is the write failure: a client leaving an event stream
    // does not say so, it stops reading. Handling it as an ordinary ending is
    // what keeps a closed browser tab from becoming an incident — and what
    // keeps the thread from being held until the lifetime bound.
    let mut stream = EventStream::open(
        StreamKind::Orders,
        Box::new(Endless(0)),
        clock(),
        StreamLimits {
            // Long enough that only the sink can end this.
            max_duration: std::time::Duration::from_secs(3_600),
            ..brisk()
        },
        None,
    );
    let mut sink = GoesAway {
        accepted: 0,
        limit: 4,
    };
    let outcome = qip_api::pump(&mut sink, &mut stream);

    assert_eq!(
        outcome.end,
        qip_api::StreamEnd::ClientDisconnected,
        "a broken pipe is the normal end of a stream"
    );
    assert!(
        outcome.frames < 4,
        "the loop kept writing past the failure: {outcome:?}"
    );
}

#[test]
fn a_client_that_leaves_before_the_head_is_written_ends_the_loop_too() {
    // The narrowest window there is: the peer closed between the accept and
    // the first byte of the response. Nothing has been sent, so nothing can be
    // reported as sent.
    let mut stream = open(Scripted::quiet(), brisk(), None);
    let mut sink = GoesAway {
        accepted: 0,
        limit: 0,
    };
    let outcome = qip_api::pump(&mut sink, &mut stream);
    assert_eq!(outcome.end, qip_api::StreamEnd::ClientDisconnected);
    assert_eq!(outcome.frames, 0);
    assert_eq!(outcome.bytes, 0);
}

// --- the response head -------------------------------------------------------

#[test]
fn the_response_head_declares_an_event_stream_with_no_length_and_no_caching() {
    let mut stream = open(Scripted::quiet(), brisk(), None);
    let mut sink: Vec<u8> = Vec::new();
    let _ = qip_api::pump(&mut sink, &mut stream);
    let written = String::from_utf8(sink).expect("UTF-8");
    let head = written
        .split("\r\n\r\n")
        .next()
        .expect("a head")
        .to_string();

    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert!(head.contains("content-type: text/event-stream"), "{head}");
    assert!(head.contains("cache-control: no-cache"), "{head}");
    assert!(head.contains("connection: keep-alive"), "{head}");
    // nginx buffers a proxied response by default, which for a stream means
    // events arriving in batches whenever the buffer fills.
    assert!(head.contains("x-accel-buffering: no"), "{head}");
    // A length declared now would be wrong later, and a client would stop
    // reading at the wrong byte and treat a truncated event as a whole one.
    assert!(
        !head.to_ascii_lowercase().contains("content-length"),
        "{head}"
    );
    // The stream's own cache directive replaces the default rather than
    // joining it: a proxy given both is entitled to honour either.
    assert!(
        !head.contains("no-store"),
        "two cache directives reached the wire: {head}"
    );
    // And the security headers every other response carries are still here.
    assert!(head.contains("default-src 'none'"), "{head}");
    assert!(head.contains("x-content-type-options: nosniff"), "{head}");
}

// --- resuming ----------------------------------------------------------------

#[test]
fn a_last_event_id_resumes_after_the_cursor_it_names() {
    let records = log_records();
    let mut source = qip_api::stream::LoggedEvents::new(
        Arc::new(Mutex::new(platform().expect("a platform"))),
        StreamKind::Orders.topics(),
        8,
    );
    // The selection rule directly, against records built by hand: what a
    // resume returns is what `select` returns above the resumed cursor.
    let resumed = select(&records, StreamKind::Orders.topics(), 3, 16);
    assert!(
        resumed.iter().all(|emission| emission.cursor > 3),
        "a resume must not re-send what the client already has"
    );
    assert_eq!(
        source.resume(Some(3)).cursor,
        3,
        "an in-range resume starts exactly where the client stopped"
    );
}

#[test]
fn a_cursor_the_log_can_no_longer_replay_opens_with_a_gap_naming_what_was_lost() {
    // The log evicts its oldest lossy-tolerable records when it reaches its
    // capacity bound — market ticks, quotes and books, which is exactly the
    // market stream. A client resuming from an evicted cursor must be told,
    // because it will otherwise carry on believing its sequence is contiguous.
    let mut source = Scripted::quiet();
    source.resume = Resume {
        cursor: 500,
        skipped: Some(37),
        note: "37 log record(s) cannot be replayed".to_string(),
    };
    let mut stream = open(source, brisk(), None);
    let frames = drain(&mut stream);
    let gap = events(&frames)
        .into_iter()
        .find(|event| event["type"] == serde_json::json!("stream.gap"))
        .expect("a gap must be an event, not a comment a client has to parse");
    assert_eq!(gap["payload"]["skipped"], serde_json::json!(37), "{gap}");
    assert_eq!(
        gap["payload"]["resume_from"],
        serde_json::json!(500),
        "{gap}"
    );
    assert_eq!(
        gap["sequence"],
        serde_json::json!(1),
        "the gap is the first thing the client is told: {gap}"
    );
}

#[test]
fn a_last_event_id_that_will_not_parse_is_treated_as_absent_rather_than_refused() {
    // The header is a browser replaying what the server sent it, so a value
    // that will not parse means something rewrote it in transit. Refusing the
    // connection would lose the stream; starting at the live edge loses only
    // the history.
    let mut stream = open(Scripted::quiet(), brisk(), Some("not-a-number"));
    let first = qip_api::ResponseStream::next_frame(&mut stream).expect("an opening frame");
    assert!(
        String::from_utf8(first)
            .expect("UTF-8")
            .contains("open at cursor")
    );
}

// --- selection ---------------------------------------------------------------

fn record(sequence: u64, topic: Topic) -> LogRecord {
    LogRecord {
        sequence,
        previous_hash: "0".repeat(64),
        record_hash: "1".repeat(64),
        event: AnyEvent {
            event_id: EventId::from_string(format!("evt-{sequence}")),
            topic,
            schema_version: 1,
            occurred_at: now(),
            recorded_at: now().saturating_add(Duration::from_millis(3)),
            sequence,
            lineage: Lineage::root(
                CorrelationId::from_string(format!("corr-{sequence}")),
                "test",
            ),
            idempotency_key: None,
            payload: serde_json::json!({"sequence": sequence}),
            payload_hash: "2".repeat(64),
        },
    }
}

fn log_records() -> Vec<LogRecord> {
    vec![
        record(1, Topic::MarketTick),
        record(2, Topic::OrderProposed),
        record(3, Topic::SignalGenerated),
        record(4, Topic::OrderFilled),
        record(5, Topic::PositionUpdated),
        record(6, Topic::MarketQuote),
        record(7, Topic::OrderCancelled),
    ]
}

#[test]
fn a_stream_carries_its_own_topics_and_nothing_else() {
    // The rule that decides whether an order transition reaches a dashboard.
    // Checked against records built by hand, so it is checked for every topic
    // rather than only for the ones a running platform happens to emit.
    let records = log_records();
    for (kind, expected) in [
        (StreamKind::Orders, vec![2u64, 4, 7]),
        (StreamKind::Market, vec![1, 6]),
        (StreamKind::Signals, vec![3]),
        (StreamKind::Positions, vec![5]),
    ] {
        let cursors: Vec<u64> = select(&records, kind.topics(), 0, 64)
            .iter()
            .map(|emission| emission.cursor)
            .collect();
        assert_eq!(
            cursors,
            expected,
            "{} selected the wrong records",
            kind.name()
        );
    }
    // The two halves of a trade are on separate streams on purpose: a
    // position update is not an order transition, and a dashboard wants them
    // apart.
    assert!(
        StreamKind::Orders
            .topics()
            .iter()
            .all(|topic| !StreamKind::Positions.topics().contains(topic)),
        "the order and position streams overlap"
    );
}

#[test]
fn a_selection_is_capped_so_one_poll_cannot_turn_the_whole_log_into_one_batch() {
    let records = log_records();
    let capped = select(&records, StreamKind::Orders.topics(), 0, 2);
    assert_eq!(capped.len(), 2);
    assert_eq!(capped[0].cursor, 2);
    // Every field a client is promised comes off the record rather than off a
    // clock read at send time: the whole value of `event_time` is that it is
    // the platform's, not the stream's.
    assert_eq!(capped[0].occurred_at, now());
    assert_eq!(
        capped[0].ingested_at,
        now().saturating_add(Duration::from_millis(3))
    );
    assert_eq!(capped[0].correlation_id, "corr-2");
    assert_eq!(capped[0].event_type, "order.proposed");
}

// --- the surface -------------------------------------------------------------

fn credentials() -> Vec<Credential> {
    [
        ("monitor@example.com", Role::Monitor, "monitor-token"),
        ("viewer@example.com", Role::Viewer, "viewer-token"),
    ]
    .into_iter()
    .map(|(subject, role, token)| {
        Credential::from_token(
            subject,
            role,
            token.to_string(),
            now(),
            now().saturating_add(Duration::from_days(30)),
        )
    })
    .collect()
}

fn platform() -> Result<Platform> {
    let clock = clock();
    let config = PlatformConfig::default();
    let context = qip_core::Context::new(clock.clone(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

fn api() -> Result<Api> {
    Ok(Api::new(
        Arc::new(Mutex::new(platform()?)),
        Arc::new(Authenticator::new(credentials())),
        Arc::new(RateLimiter::new(Duration::from_secs(60), 1000)),
        clock(),
    )
    .with_cells(Arc::new(CellRegistry::default()))
    .with_stream_limits(brisk()))
}

fn request(path: &str, token: Option<&str>) -> Request {
    let mut headers = BTreeMap::new();
    if let Some(token) = token {
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
    }
    Request {
        method: Method::Get,
        path: path.to_string(),
        query: BTreeMap::new(),
        headers,
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    }
}

fn refusal(decision: StreamDecision) -> Response {
    match decision {
        StreamDecision::Refused(response) => response,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn every_stream_is_in_the_route_table_under_the_authority_its_data_requires() -> Result<()> {
    // A stream is the equivalent REST route over time, so it is not a lower
    // bar than that route. The table is where a security review reads this.
    for kind in StreamKind::ALL {
        let route = ROUTES
            .iter()
            .find(|route| route.method == Method::Get && route.pattern == kind.pattern())
            .unwrap_or_else(|| panic!("{} is not in the route table", kind.name()));
        assert!(
            route.required_role >= Role::Monitor,
            "{} is readable without a credential",
            kind.name()
        );
        assert_eq!(route.summary, kind.summary());
    }
    // Portfolio data needs portfolio authority; liveness does not.
    for kind in [
        StreamKind::Market,
        StreamKind::Signals,
        StreamKind::Orders,
        StreamKind::Positions,
    ] {
        let route = ROUTES
            .iter()
            .find(|route| route.pattern == kind.pattern())
            .expect("in the table");
        assert_eq!(route.required_role, Role::Viewer, "{}", kind.name());
    }
    Ok(())
}

#[test]
fn a_stream_refuses_an_unauthenticated_caller_and_one_below_its_role() -> Result<()> {
    // A stream head is a 200 written before the first event and cannot be
    // taken back, so the check has to happen before the connection is taken
    // over — not after.
    let api = api()?;
    assert_eq!(
        refusal(api.stream(&request("/api/v1/stream/orders", None))).status,
        401
    );
    assert_eq!(
        refusal(api.stream(&request("/api/v1/stream/orders", Some("monitor-token")))).status,
        403,
        "a monitoring token holds no portfolio authority"
    );
    assert_eq!(
        refusal(api.stream(&request("/api/v1/stream/orders", Some("wrong-token")))).status,
        401
    );
    // The one stream a monitor may read, matching `/health`.
    assert!(matches!(
        api.stream(&request("/api/v1/stream/health", Some("monitor-token"))),
        StreamDecision::Accepted(_)
    ));
    // And a path that is not a stream is not one.
    assert!(matches!(
        api.stream(&request("/api/v1/orders", Some("viewer-token"))),
        StreamDecision::NotAStream
    ));
    Ok(())
}

#[test]
fn a_stream_asked_for_through_the_handler_answers_with_its_contract() -> Result<()> {
    // Reached by a client library that buffers a whole response before
    // returning it. The contract is more use than a refusal, and it states the
    // reconnect semantics rather than implying them.
    let api = api()?;
    let response = api.handle(&request("/api/v1/stream/market", Some("viewer-token")));
    assert_eq!(response.status, 200);
    let body: serde_json::Value =
        serde_json::from_slice(&response.body).expect("the descriptor is JSON");
    assert_eq!(body["stream"], serde_json::json!("market"));
    assert_eq!(body["content_type"], serde_json::json!("text/event-stream"));
    assert_eq!(body["replays"], serde_json::json!(true));
    assert!(
        body["topics"]
            .as_array()
            .is_some_and(|topics| topics.contains(&serde_json::json!("market.tick"))),
        "{body}"
    );

    // The health stream says plainly that it cannot replay, and what that
    // costs a reconnecting client.
    let health = api.handle(&request("/api/v1/stream/health", Some("monitor-token")));
    let health: serde_json::Value = serde_json::from_slice(&health.body).expect("JSON");
    assert_eq!(health["replays"], serde_json::json!(false));
    assert!(
        health["reconnect"]
            .as_str()
            .is_some_and(|note| note.contains("does not replay")),
        "{health}"
    );
    Ok(())
}

#[test]
fn the_health_stream_reports_this_process_and_repeats_itself_only_when_it_changes() -> Result<()> {
    // Sourced from the process's own state rather than from the log, because
    // the platform records no event for some of the conditions a health stream
    // exists to show. What it must not do is re-send an unchanged reading: a
    // client cannot tell a repeated state from a new transition.
    let api = api()?;
    let StreamDecision::Accepted(mut body) =
        api.stream(&request("/api/v1/stream/health", Some("monitor-token")))
    else {
        panic!("a monitor may read the health stream");
    };
    let mut frames = Vec::new();
    while let Some(frame) = body.next_frame() {
        frames.push(String::from_utf8(frame).expect("UTF-8"));
    }
    let readings: Vec<serde_json::Value> = events(&frames)
        .into_iter()
        .filter(|event| event["type"] == serde_json::json!("health.changed"))
        .collect();

    assert_eq!(
        readings.len(),
        1,
        "an unchanging platform must produce exactly one reading, not one per poll: {frames:?}"
    );
    let reading = &readings[0];
    assert_eq!(reading["payload"]["status"], serde_json::json!("ok"));
    assert_eq!(reading["payload"]["halted"], serde_json::json!(false));
    assert_eq!(reading["payload"]["live_capable"], serde_json::json!(false));
    assert_eq!(reading["payload"]["cells_reporting"], serde_json::json!(0));
    assert_eq!(reading["payload"]["chain_intact"], serde_json::json!(true));
    // The stream stayed alive between the reading and the close.
    assert!(
        frames.iter().any(|frame| frame.starts_with(": heartbeat")),
        "{frames:?}"
    );
    Ok(())
}

// --- over a socket -----------------------------------------------------------

#[test]
fn a_stream_served_over_a_socket_writes_a_head_then_events_and_closes_at_its_bound() -> Result<()> {
    // The whole path: parse, authorise, take the connection over, write an
    // open-ended head, write frames, end. Everything above this test exercises
    // one piece of it.
    let server = Server::bind("127.0.0.1:0", Arc::new(api()?), ServerLimits::default())?;
    let address = server.local_address()?;
    let handle = std::thread::spawn(move || {
        let _ = server.serve_once();
    });

    let mut socket = TcpStream::connect(&address).expect("connects");
    socket
        .write_all(
            b"GET /api/v1/stream/health HTTP/1.1\r\nhost: localhost\r\n\
              authorization: Bearer monitor-token\r\n\r\n",
        )
        .expect("writes");
    socket.flush().expect("flushes");
    let mut response = String::new();
    let _ = socket.read_to_string(&mut response);
    let _ = handle.join();

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(
        response.contains("content-type: text/event-stream"),
        "{response}"
    );
    assert!(response.contains("cache-control: no-cache"), "{response}");
    assert!(response.contains("connection: keep-alive"), "{response}");
    assert!(response.contains("\nevent: health.changed\n"), "{response}");
    assert!(response.contains("\nevent: stream.closing\n"), "{response}");
    assert!(
        !response.to_ascii_lowercase().contains("content-length"),
        "{response}"
    );
    Ok(())
}

#[test]
fn an_unauthorised_stream_request_over_a_socket_is_refused_as_an_ordinary_response() -> Result<()> {
    // The refusal takes the buffered path, so a caller without a credential
    // gets a 401 with a length and a closed connection rather than an empty
    // event stream held open.
    let server = Server::bind("127.0.0.1:0", Arc::new(api()?), ServerLimits::default())?;
    let address = server.local_address()?;
    let handle = std::thread::spawn(move || {
        let _ = server.serve_once();
    });

    let mut socket = TcpStream::connect(&address).expect("connects");
    socket
        .write_all(b"GET /api/v1/stream/orders HTTP/1.1\r\nhost: localhost\r\n\r\n")
        .expect("writes");
    socket.flush().expect("flushes");
    let mut response = String::new();
    let _ = socket.read_to_string(&mut response);
    let _ = handle.join();

    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    assert!(response.contains("www-authenticate: Bearer"), "{response}");
    assert!(
        response.to_ascii_lowercase().contains("content-length"),
        "{response}"
    );
    assert!(!response.contains("text/event-stream"), "{response}");
    Ok(())
}

#[test]
fn a_head_request_to_a_stream_is_not_answered_by_holding_the_connection_open() -> Result<()> {
    // A HEAD asks for headers without a body. Answering it by writing events
    // for the lifetime of the connection is the opposite of what was asked,
    // and would hold a thread against the server's concurrency limit for a
    // probe.
    let server = Server::bind("127.0.0.1:0", Arc::new(api()?), ServerLimits::default())?;
    let address = server.local_address()?;
    let handle = std::thread::spawn(move || {
        let _ = server.serve_once();
    });

    let mut socket = TcpStream::connect(&address).expect("connects");
    socket
        .write_all(
            b"HEAD /api/v1/stream/health HTTP/1.1\r\nhost: localhost\r\n\
              authorization: Bearer monitor-token\r\n\r\n",
        )
        .expect("writes");
    socket.flush().expect("flushes");
    let mut response = String::new();
    let _ = socket.read_to_string(&mut response);
    let _ = handle.join();

    assert!(!response.contains("\nevent: "), "{response}");
    assert!(
        response.to_ascii_lowercase().contains("content-length"),
        "{response}"
    );
    Ok(())
}

#[test]
fn the_last_event_id_header_is_read_from_the_request() {
    // The name a browser actually sends, lower-cased the way the parser stores
    // every header. A mismatch here is a reconnect that silently replays from
    // the live edge every time.
    let mut headers = BTreeMap::new();
    headers.insert(LAST_EVENT_ID.to_string(), "42".to_string());
    let request = Request {
        method: Method::Get,
        path: "/api/v1/stream/orders".to_string(),
        query: BTreeMap::new(),
        headers,
        body: Vec::new(),
        peer: "127.0.0.1:1".to_string(),
    };
    assert_eq!(LAST_EVENT_ID, "last-event-id");
    assert_eq!(request.header("Last-Event-ID"), Some("42"));
}
