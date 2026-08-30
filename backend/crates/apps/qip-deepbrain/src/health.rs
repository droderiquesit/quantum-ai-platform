//! The health surface.
//!
//! Deliberately tiny, deliberately blocking, and deliberately not the
//! platform's HTTP server: a node whose liveness probe depends on the API crate
//! has coupled the two, and the probe is what tells an orchestrator whether
//! this node is alive at all. One thread, one listener, an explicit timeout on
//! every read and write, and no async runtime — the same shape `qip-fastbrain`
//! and `qip-edge-node` use, for the same reason.
//!
//! Four questions have four answers, because they are four different
//! questions:
//!
//! * `/health` — is the process alive and answering? Always 200 while this
//!   thread runs. On this node that is not merely defensible, it is the only
//!   safe answer: a deep-brain cycle may legitimately run for minutes, so a
//!   liveness probe that could go red while one is in flight would restart the
//!   node in the middle of every long analysis and it would never finish one.
//! * `/ready` — should this node be consulted? 503 while it is stopping,
//!   halted, stalled, persistently failing, or still warming up. Note that
//!   *slow* is not on that list; see [`crate::status::Unready`].
//! * `/` — everything, as JSON, for a person rather than a probe.
//! * `/metrics` — what the node has recorded, in Prometheus text exposition.
//!   Not a probe and not a health question: it is the surface a scrape reads,
//!   and it exists here because this node runs the cycle. A process that
//!   records metrics nowhere anything can read them is exactly as observable
//!   as one that records none.
//!
//! `/quiesce` is the only request that changes anything, and it is refused
//! unless it arrived over loopback and used POST. A stop button reachable from
//! anywhere the health port is reachable is not a health surface.

use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::status::NodeStatus;

/// How long a health request may take to arrive or to be written.
///
/// A probe that hangs holds this thread, and a held thread makes every later
/// probe time out — which reads as a dead node. Two seconds is far longer than
/// a local probe needs and far shorter than a probe's own timeout. It is
/// unrelated to how long a *cycle* may take, which is the point: this surface
/// never waits on the loop.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The largest request line this surface will read.
const MAXIMUM_REQUEST: usize = 2048;

/// What a caller asked for, once the socket is out of the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// Whether the caller reached this node over loopback. The quiesce
    /// request's only gate: a pod's own pre-stop hook is loopback, and anything
    /// arriving over the pod network is not.
    pub from_loopback: bool,
}

impl Request {
    /// Parse a request line. Anything malformed is not a request.
    pub fn parse(line: &str, from_loopback: bool) -> Option<Self> {
        let mut parts = line.split_whitespace();
        let method = parts.next()?.to_string();
        let target = parts.next()?;
        // The query string is not part of routing here, and a probe that
        // appends a cache-buster should still reach the endpoint it named.
        let path = target.split(['?', '#']).next().unwrap_or(target);
        Some(Self {
            method,
            path: path.to_string(),
            from_loopback,
        })
    }
}

/// The answer, before it is bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub code: u16,
    pub reason: &'static str,
    pub body: String,
    /// What the body is, on the wire.
    ///
    /// Carried rather than assumed since the scrape surface joined the probes.
    /// Every answer here used to be JSON and the renderer said so unread; a
    /// Prometheus exposition served under `application/json` is one a scraper
    /// refuses and a person debugging it cannot see the reason for, because
    /// the body it prints looks exactly right.
    pub content_type: &'static str,
    /// Whether answering this request means the node has been asked to stop.
    pub quiesce_requested: bool,
}

impl Response {
    fn json(code: u16, reason: &'static str, body: String) -> Self {
        Self {
            code,
            reason,
            body,
            content_type: "application/json",
            quiesce_requested: false,
        }
    }

    /// Prometheus text exposition. The version is part of the media type and a
    /// scraper reads it to decide how to parse what follows.
    fn exposition(body: String) -> Self {
        Self {
            code: 200,
            reason: "OK",
            body,
            content_type: "text/plain; version=0.0.4; charset=utf-8",
            quiesce_requested: false,
        }
    }

    /// The bytes on the wire.
    pub fn render(&self) -> String {
        format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nConnection: close\r\n\r\n{}",
            self.code,
            self.reason,
            self.content_type,
            self.body.len(),
            self.body
        )
    }
}

/// Answer one request from the node's own view of itself.
///
/// Pure, so every route and every status code is asserted without a socket.
pub fn respond(request: &Request, status: &NodeStatus, now: Timestamp) -> Response {
    let view = status.view(now);
    let full = serde_json::to_string(&view)
        // A status that cannot be serialised is still a status, and a probe
        // that got no answer at all could not tell that from a dead node.
        .unwrap_or_else(|error| format!(r#"{{"error":"the status did not render: {error}"}}"#));

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") | ("HEAD", "/") => Response::json(200, "OK", full),
        ("GET", "/health") | ("HEAD", "/health") => Response::json(
            200,
            "OK",
            format!(
                r#"{{"alive":true,"cycles":{},"cycle_in_flight":{}}}"#,
                view.cycles, view.cycle_in_flight
            ),
        ),
        ("GET", "/ready") | ("HEAD", "/ready") => {
            if view.ready {
                Response::json(200, "OK", full)
            } else {
                Response::json(503, "Service Unavailable", full)
            }
        }
        ("POST", "/quiesce") => {
            if !request.from_loopback {
                return Response::json(
                    403,
                    "Forbidden",
                    r#"{"error":"a quiesce may only be asked for over loopback"}"#.to_string(),
                );
            }
            Response {
                code: 202,
                reason: "Accepted",
                content_type: "application/json",
                body: r#"{"quiescing":true,"note":"the node stops after the cycle in flight, which on this node may be minutes"}"#
                    .to_string(),
                quiesce_requested: true,
            }
        }
        // What this node has recorded, for a scrape. Served from the same
        // registry the cycle writes to, so a number here is a number something
        // put there rather than one this surface computed from the status it
        // was already rendering. Those would be two claims about one fact, and
        // the status view is a snapshot while the registry is a history: a
        // refusal that happened and was superseded is gone from the first and
        // permanent in the second.
        //
        // Empty until the first cycle records something, which is the honest
        // answer for a node that has not run one yet rather than a bank of
        // zeroes it has no evidence for.
        ("GET" | "HEAD", "/metrics") => Response::exposition(status.metrics().snapshot().to_prometheus()),
        ("GET" | "HEAD", "/quiesce") => Response::json(
            405,
            "Method Not Allowed",
            r#"{"error":"a quiesce is a POST; reading this endpoint must not stop the node"}"#
                .to_string(),
        ),
        _ => Response::json(
            404,
            "Not Found",
            r#"{"error":"no such endpoint; this node serves /, /health, /ready, /metrics and /quiesce"}"#
                .to_string(),
        ),
    }
}

/// Bind the health port.
///
/// Separated from serving so the binary fails on a busy port before it
/// assembles anything, and so a test can bind port zero and ask what it got.
pub fn bind(address: &str) -> Result<TcpListener> {
    TcpListener::bind(address).map_err(|error| {
        Error::io(format!(
            "cannot bind the health surface to {address}: {error}"
        ))
    })
}

/// Serve until the listener fails.
///
/// Runs on its own thread and holds no lock the run loop needs for longer than
/// it takes to render a status. That matters more here than on the fast path:
/// a cycle holds the platform for minutes at a time, and this surface has to
/// keep answering throughout or an orchestrator will conclude the node died
/// mid-thought.
pub fn serve(
    listener: &TcpListener,
    status: &Arc<Mutex<NodeStatus>>,
    stop: &Arc<AtomicBool>,
    clock: &Arc<dyn Clock>,
) {
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if let Err(error) = answer(stream, status, stop, clock) {
                    eprintln!("qip-deepbrain: health request failed: {}", error.message());
                }
            }
            // One bad connection is not a reason to stop researching.
            Err(error) => eprintln!("qip-deepbrain: health accept failed: {error}"),
        }
        if stop.load(Ordering::Relaxed) {
            // The node is on its way out. The accepted request has been
            // answered; the next one belongs to whatever replaces this process.
            return;
        }
    }
}

fn answer(
    mut stream: TcpStream,
    status: &Arc<Mutex<NodeStatus>>,
    stop: &Arc<AtomicBool>,
    clock: &Arc<dyn Clock>,
) -> Result<()> {
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(REQUEST_TIMEOUT)))
        .map_err(|error| Error::io(format!("cannot bound the health request: {error}")))?;

    let from_loopback = stream
        .peer_addr()
        .map(|address| address.ip().is_loopback())
        .unwrap_or(false);

    let mut buffer = [0u8; MAXIMUM_REQUEST];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let text = String::from_utf8_lossy(&buffer[..read]);
    let line = text.lines().next().unwrap_or_default();

    let now = clock.now();
    let response = match Request::parse(line, from_loopback) {
        Some(request) => {
            // Cloned, and the guard dropped before a byte is rendered. The
            // surface used to hold this lock across the whole render, which was
            // tolerable while every answer was a small status blob and is not
            // now that one of them serialises the entire metric registry. This
            // is the slow path and a stalled cycle here is judged against a
            // generous ceiling, which is exactly why it matters: a render that
            // blocked the loop would be charged to the loop, and the surface
            // reporting the stall would be the thing causing it.
            let snapshot = match status.lock() {
                Ok(guard) => guard.clone(),
                // A poisoned reporting lock must not silence the surface that
                // says whether the node is alive.
                Err(poisoned) => poisoned.into_inner().clone(),
            };
            respond(&request, &snapshot, now)
        }
        None => Response::json(
            400,
            "Bad Request",
            r#"{"error":"that is not an HTTP request line"}"#.to_string(),
        ),
    };

    if response.quiesce_requested {
        stop.store(true, Ordering::Relaxed);
    }

    stream
        .write_all(response.render().as_bytes())
        .map_err(|error| Error::io(format!("cannot write the health response: {error}")))?;
    stream
        .flush()
        .map_err(|error| Error::io(format!("cannot flush the health response: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DeepBrainConfig;
    use crate::status::CycleRecord;
    use qip_core::Duration;
    use qip_observability::Metrics;
    use qip_observability::metrics::{labels, names};
    use std::sync::Arc;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn record(elapsed: Duration, traversed: bool) -> CycleRecord {
        CycleRecord {
            started_at: start(),
            finished_at: start(),
            elapsed,
            traversed_every_stage: traversed,
            problems: Vec::new(),
            archived: 0,
            halted: false,
        }
    }

    /// A node that has completed one cycle, which is the first moment it is
    /// ready at all.
    fn status() -> NodeStatus {
        let roster = crate::roster::clear(start()).expect("the deployed roster clears");
        let mut status = NodeStatus::opening(&roster, &DeepBrainConfig::default(), start());
        status.cycle_finished(&record(Duration::from_secs(90), true));
        status
    }

    fn get(path: &str) -> Request {
        Request {
            method: "GET".to_string(),
            path: path.to_string(),
            from_loopback: false,
        }
    }

    #[test]
    fn the_full_status_names_the_cycle_count_the_roster_result_and_the_node_it_belongs_to() {
        let response = respond(&get("/"), &status(), start());
        assert_eq!(response.code, 200);
        let body: serde_json::Value =
            serde_json::from_str(&response.body).expect("the body is JSON");
        assert_eq!(body["cycles"], 1);
        assert_eq!(body["roster_validated"], true);
        assert_eq!(body["node"], "qip-deepbrain");
        assert_eq!(body["reaches_a_venue"], false);
    }

    #[test]
    fn liveness_answers_two_hundred_while_a_cycle_that_has_run_for_hours_is_still_in_flight() {
        // The property this node most depends on. A liveness probe that could
        // go red mid-cycle would restart every long analysis before it
        // finished, and the node would never complete one.
        let mut status = status();
        // Started after the previous cycle finished, which is what makes it the
        // cycle in flight rather than a re-report of the last one.
        let began = start().saturating_add(Duration::from_mins(1));
        status.cycle_started(began);
        let hours_later = began.saturating_add(Duration::from_hours(3));
        assert!(
            status.cycle_in_flight(),
            "the premise: a cycle is running and has not returned"
        );
        assert!(
            !status.is_ready(hours_later),
            "the premise: this node is not ready by then"
        );
        assert_eq!(
            respond(&get("/health"), &status, hours_later).code,
            200,
            "a node in the middle of a long analysis was reported dead"
        );
        assert_eq!(
            respond(&get("/ready"), &status, hours_later).code,
            503,
            "a node that has produced nothing for hours is still being consulted"
        );
    }

    #[test]
    fn readiness_refuses_a_node_that_has_not_finished_its_first_cycle_and_says_it_is_warming() {
        let roster = crate::roster::clear(start()).expect("the roster clears");
        let fresh = NodeStatus::opening(&roster, &DeepBrainConfig::default(), start());
        let response = respond(&get("/ready"), &fresh, start());
        assert_eq!(response.code, 503);
        let body: serde_json::Value =
            serde_json::from_str(&response.body).expect("the body is JSON");
        assert_eq!(body["unready_because"], "warming");
        assert_eq!(
            respond(&get("/health"), &fresh, start()).code,
            200,
            "a node that is starting up was reported dead rather than warming"
        );
    }

    #[test]
    fn readiness_answers_two_hundred_once_a_cycle_has_landed_however_long_it_took() {
        let mut status = status();
        status.cycle_finished(&record(Duration::from_mins(45), true));
        let response = respond(&get("/ready"), &status, start());
        assert_eq!(
            response.code, 200,
            "a forty-five minute cycle took the node out of rotation; that is the fast brain's rule"
        );
        let body: serde_json::Value =
            serde_json::from_str(&response.body).expect("the body is JSON");
        assert_eq!(body["ready"], true);
        assert_eq!(body["cycle_overruns"], 1);
    }

    #[test]
    fn a_readiness_refusal_says_which_of_the_five_reasons_it_is() {
        let mut status = status();
        status.stopping();
        let response = respond(&get("/ready"), &status, start());
        assert_eq!(response.code, 503);
        let body: serde_json::Value =
            serde_json::from_str(&response.body).expect("the body is JSON");
        assert_eq!(body["unready_because"], "stopping");
    }

    #[test]
    fn a_quiesce_from_off_the_node_is_refused_and_stops_nothing() {
        let request = Request {
            method: "POST".to_string(),
            path: "/quiesce".to_string(),
            from_loopback: false,
        };
        let response = respond(&request, &status(), start());
        assert_eq!(response.code, 403);
        assert!(
            !response.quiesce_requested,
            "a refused quiesce still asked the node to stop"
        );
    }

    #[test]
    fn a_quiesce_over_loopback_is_accepted_and_says_how_long_stopping_may_take() {
        let request = Request {
            method: "POST".to_string(),
            path: "/quiesce".to_string(),
            from_loopback: true,
        };
        let response = respond(&request, &status(), start());
        assert_eq!(response.code, 202);
        assert!(response.quiesce_requested);
        assert!(
            response.body.contains("may be minutes"),
            "a caller is not told that a deep-brain cycle can take minutes to finish: {}",
            response.body
        );
    }

    #[test]
    fn reading_the_quiesce_endpoint_does_not_stop_the_node() {
        // A crawler, a link checker or a curious operator issuing GET must not
        // be able to stop a research node part way through a run.
        let request = Request {
            method: "GET".to_string(),
            path: "/quiesce".to_string(),
            from_loopback: true,
        };
        let response = respond(&request, &status(), start());
        assert_eq!(response.code, 405);
        assert!(!response.quiesce_requested);
    }

    #[test]
    fn an_unknown_path_is_a_four_oh_four_that_names_the_endpoints_that_exist() {
        // `/portfolio` rather than `/metrics`: this test used `/metrics` as its
        // unknown path until the node began serving one, at which point it was
        // asserting that a route which exists returns 404. The example has to
        // be a path this surface genuinely does not serve, and the realistic
        // mistake is reaching a node with a path the API serves.
        let response = respond(&get("/portfolio"), &status(), start());
        assert_eq!(response.code, 404);
        assert!(response.body.contains("/ready"));
    }

    #[test]
    fn a_probe_that_appends_a_query_string_still_reaches_the_endpoint_it_named() {
        let request =
            Request::parse("GET /ready?t=1712 HTTP/1.1", false).expect("a valid request line");
        assert_eq!(request.path, "/ready");
        assert_eq!(respond(&request, &status(), start()).code, 200);
    }

    #[test]
    fn something_that_is_not_an_http_request_line_is_not_a_request() {
        assert!(Request::parse("", false).is_none());
        assert!(Request::parse("GET", false).is_none());
    }

    #[test]
    fn the_rendered_response_declares_the_length_of_the_body_it_carries() {
        let response = respond(&get("/"), &status(), start());
        let rendered = response.render();
        assert!(rendered.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(rendered.contains(&format!("Content-Length: {}", response.body.len())));
        let Some((_, body)) = rendered.split_once("\r\n\r\n") else {
            panic!("the response has no body separator");
        };
        assert_eq!(body.len(), response.body.len());
    }

    #[test]
    fn the_scrape_surface_serves_what_was_recorded_and_says_it_is_not_json() {
        // The node's whole reason for serving this: the registry the cycle
        // writes to is the one a scrape reads. A surface wired to a registry of
        // its own would answer every scrape empty forever while the loop
        // recorded diligently into another — which is the defect this endpoint
        // was added to close, and it would look identical from outside.
        let registry = Arc::new(Metrics::new("test"));

        // The premise: before anything is recorded the surface answers, and
        // answers with nothing. A test that only checked the populated case
        // could not tell an endpoint that works from one that always returns
        // the same body.
        let empty = respond(
            &get("/metrics"),
            &status().with_metrics(registry.clone()),
            start(),
        );
        assert_eq!(empty.code, 200);
        assert!(
            empty.body.is_empty(),
            "a node that has recorded nothing must serve nothing, not zeroes it has no \
             evidence for: {}",
            empty.body
        );

        registry.count(
            names::ORDERS_REFUSED,
            labels([("control", "pre-trade-risk")]),
        );
        let response = respond(&get("/metrics"), &status().with_metrics(registry), start());
        assert_eq!(response.code, 200);
        assert!(
            response
                .body
                .contains("qip_orders_refused_total{control=\"pre-trade-risk\"} 1"),
            "the scrape surface did not serve the recorded series: {}",
            response.body
        );
        // A scraper reads the media type to decide how to parse the body, and
        // `application/json` in front of `# HELP` is a refusal a person
        // debugging it cannot see, because the body looks exactly right.
        assert!(
            response.content_type.starts_with("text/plain"),
            "a Prometheus exposition served as {}",
            response.content_type
        );
        assert!(
            response.render().contains("Content-Type: text/plain"),
            "the rendered response still claims JSON: {}",
            response.render()
        );
    }

    #[test]
    fn a_probe_answer_is_still_json_after_the_scrape_surface_was_added() {
        // The content type became a field when the exposition arrived. Every
        // other answer must be unaffected, or adding observability broke the
        // probes it was added to support.
        let response = respond(&get("/ready"), &status(), start());
        assert_eq!(response.content_type, "application/json");
        assert!(response.render().contains("Content-Type: application/json"));
    }
}
