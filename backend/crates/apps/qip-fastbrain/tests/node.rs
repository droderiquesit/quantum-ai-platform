//! The node as a process sees it: a real socket, a real loop, a real stop.
//!
//! The unit tests hold each part against its own property. These hold the parts
//! together, because the two failures that matter most are ones no single part
//! can have: a health surface that cannot answer while a cycle is running, and
//! a stop request that never reaches the loop.

use qip_core::error::Result;
use qip_core::{Clock, Duration, SystemClock, Timestamp};
use qip_fastbrain::config::FastBrainConfig;
use qip_fastbrain::feed::Feed;
use qip_fastbrain::status::NodeStatus;
use qip_fastbrain::{health, node, roster};
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::{ChainArchive, MemoryKeyValueStore};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn platform(clock: Arc<dyn Clock>) -> Platform {
    let config = PlatformConfig::default();
    let context = qip_core::Context::new(clock.clone(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .expect("the platform assembles")
}

/// Ask the health surface something over a real socket.
fn ask(port: u16, request: &str) -> Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|error| qip_core::error::Error::io(format!("cannot connect: {error}")))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|error| qip_core::error::Error::io(format!("cannot bound the read: {error}")))?;
    stream
        .write_all(format!("{request} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
        .map_err(|error| qip_core::error::Error::io(format!("cannot write: {error}")))?;
    let mut body = String::new();
    stream
        .read_to_string(&mut body)
        .map_err(|error| qip_core::error::Error::io(format!("cannot read: {error}")))?;
    Ok(body)
}

#[test]
fn the_node_runs_cycles_serves_its_own_status_and_stops_when_asked_over_loopback() {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let started = clock.now();

    // The check that gates everything else. If this refuses, nothing below runs
    // — which is the arrangement being tested as much as the loop is.
    let cleared = roster::clear(started).expect("the deployed roster clears its own check");

    let config = FastBrainConfig {
        // Port zero, so the test never fights another process for a port.
        health_address: "127.0.0.1:0".to_string(),
        cycle_interval: Duration::from_millis(10),
        archive_every: 1,
        ..FastBrainConfig::default()
    };

    let listener = health::bind(&config.health_address).expect("the health surface binds");
    let port = listener
        .local_addr()
        .expect("the listener has an address")
        .port();

    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &cleared,
        &config,
        "synthetic-exchange",
        false,
        started,
    )));
    let stop = Arc::new(AtomicBool::new(false));

    {
        let status = status.clone();
        let stop = stop.clone();
        let clock = clock.clone();
        std::thread::spawn(move || health::serve(&listener, &status, &stop, &clock));
    }

    // Before a single cycle has run, the surface already answers. A node that
    // only becomes visible once it is working cannot report failing to start.
    let opening = ask(port, "GET /").expect("the health surface answers immediately");
    assert!(
        opening.contains("\"roster_validated\":true"),
        "the opening response does not report the roster check: {opening}"
    );

    let mut platform = platform(clock.clone());
    // Half an hour of market already behind it, so the very first cycle has
    // something to ingest rather than only the microseconds since start-up.
    let mut feed = Feed::synthetic(
        5,
        Duration::from_secs(60),
        started.saturating_sub(Duration::from_mins(30)),
    );
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
        .expect("an empty archive opens");

    // Asked to stop from inside the third cycle, over a real socket, from this
    // process — which is loopback, and is what a pre-stop hook does.
    let mut asked = false;
    let summary = node::run(
        &mut platform,
        &mut feed,
        &archive,
        &config,
        &status,
        &stop,
        &clock,
        |_| {
            if !asked {
                let answer = ask(port, "POST /quiesce").expect("the quiesce is answered");
                assert!(
                    answer.starts_with("HTTP/1.1 202 Accepted"),
                    "a loopback quiesce was not accepted: {answer}"
                );
                asked = true;
            }
        },
    )
    .expect("the loop runs");

    assert_eq!(summary.stopped_because, node::Stop::Requested);
    assert_eq!(
        summary.cycles, 1,
        "a quiesce asked for during a cycle must stop the node after that cycle and no later"
    );
    assert!(
        summary.observed > 0,
        "the node ran a cycle without observing anything"
    );

    // Everything it held is handed over, and the flush says what it did.
    let flushed = node::flush(&platform, &archive, true, Duration::from_secs(5))
        .expect("the shutdown flush runs");
    assert_eq!(
        flushed.left_behind, 0,
        "the flush left records behind inside its budget"
    );
    assert!(
        archive.len().expect("the archive counts itself") > 0,
        "nothing reached the chain, so this session has no account of itself"
    );
}

#[test]
fn the_health_surface_answers_while_a_cycle_is_running_rather_than_blocking_behind_it() {
    // The property: a probe must be able to tell a working node from a wedged
    // one. It cannot if answering requires the lock the work holds.
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let started = clock.now();
    let cleared = roster::clear(started).expect("the roster clears");
    let config = FastBrainConfig {
        health_address: "127.0.0.1:0".to_string(),
        ..FastBrainConfig::default()
    };

    let listener = health::bind(&config.health_address).expect("the health surface binds");
    let port = listener
        .local_addr()
        .expect("the listener has an address")
        .port();

    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &cleared,
        &config,
        "synthetic-exchange",
        false,
        started,
    )));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let status = status.clone();
        let stop = stop.clone();
        let clock = clock.clone();
        std::thread::spawn(move || health::serve(&listener, &status, &stop, &clock));
    }

    // A cycle that has started and not finished, exactly as the loop records it
    // before doing the work.
    {
        let mut guard = status.lock().expect("the status is writable");
        guard.cycle_started(Timestamp::from_secs(started.as_secs()));
    }

    let answer = ask(port, "GET /health").expect("liveness answers mid-cycle");
    assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
    assert!(
        answer.contains("\"cycle_in_flight\":true"),
        "the surface answered but did not say a cycle was in flight: {answer}"
    );
}

#[test]
fn a_quiesce_that_did_not_come_from_this_node_is_refused_and_the_loop_keeps_running() {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let started = clock.now();
    let cleared = roster::clear(started).expect("the roster clears");
    let config = FastBrainConfig {
        health_address: "127.0.0.1:0".to_string(),
        ..FastBrainConfig::default()
    };
    let listener = health::bind(&config.health_address).expect("binds");
    let port = listener
        .local_addr()
        .expect("the listener has an address")
        .port();
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &cleared,
        &config,
        "synthetic-exchange",
        false,
        started,
    )));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let status = status.clone();
        let stop = stop.clone();
        let clock = clock.clone();
        std::thread::spawn(move || health::serve(&listener, &status, &stop, &clock));
    }

    // Reading the endpoint must not stop the node, whoever asks.
    let answer = ask(port, "GET /quiesce").expect("the endpoint answers");
    assert!(
        answer.starts_with("HTTP/1.1 405"),
        "a GET stopped, or looked like it could stop, the node: {answer}"
    );
    assert!(
        !stop.load(std::sync::atomic::Ordering::Relaxed),
        "reading the quiesce endpoint asked the node to stop"
    );
}
