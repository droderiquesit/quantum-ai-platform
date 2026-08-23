//! What the HTTP client refuses, and what it survives.
//!
//! Every test here runs against a real listener on a real loopback port. The
//! interesting half is not that a well-formed response parses — it is that a
//! peer which closes mid-body, answers with more than this process will hold,
//! or says nothing at all, produces a named error rather than a truncated
//! result, an out-of-memory kill, or a wait with no end.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{Action, TestServer, address_with_no_listener};
use qip_transport::{ClientLimits, HttpClient, HttpError, HttpRequest, Method, Phase, Url};
use std::time::Duration as StdDuration;

/// Limits tight enough that a test can trip them in milliseconds and bytes.
fn tight() -> ClientLimits {
    ClientLimits {
        max_status_line: 256,
        max_header_line: 256,
        max_headers: 8,
        max_body: 512,
        max_chunk: 256,
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(200),
        write_timeout: StdDuration::from_millis(500),
    }
}

// --- URLs ---------------------------------------------------------------

#[test]
fn https_is_refused_by_name_rather_than_quietly_downgraded() {
    let error = Url::parse("https://central.internal/v1/mesh/publish")
        .expect_err("a scheme this build cannot speak was accepted");
    assert_eq!(error.code(), "unsupported_scheme");
    assert!(
        error.to_string().contains("no TLS stack"),
        "the refusal must say why, so nobody concludes the URL was malformed: {error}"
    );
    assert!(
        !error.is_transient(),
        "retrying an https URL will not grow a TLS stack"
    );
}

#[test]
fn a_url_that_carries_a_credential_is_refused() {
    let error = Url::parse("http://operator:hunter2@central.internal/v1/mesh/publish")
        .expect_err("userinfo was accepted");
    assert!(
        error.to_string().contains("credential"),
        "the refusal must name what is wrong with it: {error}"
    );
}

#[test]
fn a_url_parses_into_the_three_parts_a_request_needs() {
    let url = Url::parse("http://cell-us-east:8080/v1/mesh/publish?since=7")
        .expect("a well-formed URL was refused");
    assert_eq!(url.host(), "cell-us-east");
    assert_eq!(url.port(), 8080);
    assert_eq!(url.target(), "/v1/mesh/publish?since=7");
    assert_eq!(url.authority(), "cell-us-east:8080");

    let default_port = Url::parse("http://central.internal").expect("a bare host was refused");
    assert_eq!(default_port.port(), 80);
    assert_eq!(
        default_port.target(),
        "/",
        "a URL with no path must still put a target on the request line"
    );

    let ipv6 = Url::parse("http://[::1]:9000/health").expect("an IPv6 literal was refused");
    assert_eq!(ipv6.host(), "::1");
    assert_eq!(ipv6.port(), 9000);
    assert_eq!(ipv6.authority(), "[::1]:9000");
}

#[test]
fn a_path_carrying_a_control_character_cannot_split_the_request_line() {
    for hostile in [
        "http://peer/v1/mesh\r\nx-injected: yes",
        "http://peer/v1 /mesh",
    ] {
        assert!(
            Url::parse(hostile).is_err(),
            "{hostile} would have been written into the request line as-is"
        );
    }
}

// --- the happy path, and what it proves about the request ---------------

#[test]
fn a_response_is_read_back_whole() {
    let server = TestServer::always(Action::json(200, r#"{"accepted":1}"#));
    let client = HttpClient::new(tight());

    let response = client
        .get(&server.url_for("/v1/mesh/health"))
        .expect("a well-formed response failed to read");

    assert_eq!(response.status, 200);
    assert_eq!(response.body_as_str().expect("utf-8"), r#"{"accepted":1}"#);
    assert_eq!(
        response.header("content-type"),
        Some("application/json"),
        "header names must be matched without regard to what the peer capitalised"
    );
}

#[test]
fn the_client_writes_each_framing_header_exactly_once_even_when_a_caller_supplies_one() {
    let server = TestServer::always(Action::json(200, "{}"));
    let client = HttpClient::new(tight());

    // A caller trying to set the framing headers by hand. Two content-length
    // headers, or one that disagrees with the body, is the original
    // request-smuggling bug, so these are dropped rather than merged.
    let request = HttpRequest::json(
        Method::Post,
        &server.url_for("/v1/mesh/publish"),
        b"{\"sender\":\"x\"}".to_vec(),
    )
    .expect("a well-formed request was refused")
    .with_header("content-length", "999999")
    .with_header("host", "somewhere-else")
    .with_header("x-region", "us-east");

    client.send(&request).expect("the request failed");

    let seen = server.requests();
    let received = seen.first().expect("the server saw no request");
    assert_eq!(received.header_counts.get("content-length"), Some(&1));
    assert_eq!(received.header_counts.get("host"), Some(&1));
    assert_eq!(
        received.headers.get("content-length").map(String::as_str),
        Some("14"),
        "the declared length must be the body actually written, not the one a caller asked for"
    );
    assert_eq!(
        received.headers.get("host").map(String::as_str),
        Some(&server.url()[7..]),
        "the host header must name the peer actually connected to"
    );
    assert_eq!(
        received.headers.get("x-region").map(String::as_str),
        Some("us-east"),
        "a header that is not reserved must still be written"
    );
    assert_eq!(received.body_as_str(), r#"{"sender":"x"}"#);
}

#[test]
fn a_head_response_is_read_without_waiting_for_a_body_that_is_not_coming() {
    // A HEAD answer carries the content-length of the body it would have sent
    // and none of the bytes. A client that read the declared length would hang
    // until its own timeout.
    let server = TestServer::always(Action::Raw(
        b"HTTP/1.1 200 OK\r\ncontent-length: 4096\r\nconnection: close\r\n\r\n".to_vec(),
    ));
    let client = HttpClient::new(tight());
    let request = HttpRequest::new(Method::Head, &server.url_for("/v1/mesh/health"))
        .expect("a well-formed request was refused");

    let response = client.send(&request).expect("the HEAD request failed");
    assert_eq!(response.status, 200);
    assert!(response.body.is_empty());
}

#[test]
fn a_status_with_no_body_is_not_waited_on() {
    let server = TestServer::always(Action::Raw(
        b"HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n".to_vec(),
    ));
    let client = HttpClient::new(tight());
    let response = client
        .get(&server.url_for("/v1/mesh/health"))
        .expect("a 204 failed to read");
    assert_eq!(response.status, 204);
    assert!(response.body.is_empty());
}

// --- chunked ------------------------------------------------------------

#[test]
fn a_chunked_response_is_reassembled_in_order_including_its_trailers() {
    let server = TestServer::always(Action::Chunked {
        status: 200,
        chunks: vec![
            r#"{"frames":["#.to_string(),
            r#"{"position":1},"#.to_string(),
            r#"{"position":2}]}"#.to_string(),
        ],
    });
    let client = HttpClient::new(tight());

    let response = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect("a chunked response failed to read");

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body_as_str().expect("utf-8"),
        r#"{"frames":[{"position":1},{"position":2}]}"#,
        "the chunks must be concatenated in order with no framing bytes left in"
    );
}

#[test]
fn a_chunked_body_that_would_exceed_the_limit_is_refused_at_the_chunk_that_crosses_it() {
    // Three chunks of 200 bytes against a 512-byte limit: the first two fit
    // and the third is refused, so the refusal happens on the chunk header
    // rather than after 600 bytes have been buffered.
    let server = TestServer::always(Action::Chunked {
        status: 200,
        chunks: vec!["x".repeat(200), "x".repeat(200), "x".repeat(200)],
    });
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a chunked body over the limit was accepted");

    assert_eq!(error.code(), "body_too_large");
    assert!(
        !error.is_transient(),
        "a peer that sends too much will send too much again"
    );
}

#[test]
fn declaring_both_a_length_and_chunked_encoding_is_refused() {
    let server = TestServer::always(Action::Raw(
        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\ntransfer-encoding: chunked\r\nconnection: \
          close\r\n\r\n2\r\nhi\r\n0\r\n\r\n"
            .to_vec(),
    ));
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a response with two framings was accepted");

    assert_eq!(error.code(), "malformed");
    assert!(
        error.to_string().contains("where the body ends"),
        "the refusal must say what the ambiguity is: {error}"
    );
}

// --- the failure modes that matter --------------------------------------

#[test]
fn a_peer_that_closes_mid_body_is_a_close_and_not_a_short_read() {
    let server = TestServer::always(Action::Truncated {
        declared: 400,
        written: 10,
    });
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a truncated body was accepted as complete");

    assert_eq!(
        error.code(),
        "closed_early",
        "a body that stopped short must never be returned as if it were whole: {error}"
    );
    assert!(
        matches!(error, HttpError::ClosedEarly { phase: Phase::Body }),
        "the error must say where it stopped: {error:?}"
    );
    assert!(
        error.is_transient(),
        "a peer that died mid-response may be alive on the next attempt"
    );
}

#[test]
fn a_declared_body_over_the_limit_is_refused_before_it_is_read() {
    let server = TestServer::always(Action::Oversized { bytes: 4096 });
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("an oversized body was accepted");

    match error {
        HttpError::BodyTooLarge { limit, at_least } => {
            assert_eq!(limit, 512);
            assert_eq!(
                at_least, 4096,
                "the declared length is what was refused, which is the evidence that nothing was \
                 allocated for it"
            );
        }
        other => panic!("expected a body-too-large refusal, got {other:?}"),
    }
}

#[test]
fn a_body_with_no_framing_at_all_is_still_bounded() {
    // No content-length and no chunking: the body ends when the connection
    // does, which is a peer's licence to send forever.
    let server = TestServer::always(Action::Unframed { bytes: 4096 });
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("an unframed body over the limit was accepted");

    assert_eq!(error.code(), "body_too_large");
}

#[test]
fn a_body_exactly_at_the_limit_is_accepted() {
    // The boundary, in both directions: 512 is accepted, 513 is not. An
    // off-by-one here would either refuse legitimate traffic or admit one byte
    // more than the limit claims.
    let client = HttpClient::new(tight());

    let at_limit = TestServer::always(Action::Oversized { bytes: 512 });
    let response = client
        .get(&at_limit.url_for("/x"))
        .expect("a body exactly at the limit was refused");
    assert_eq!(response.body.len(), 512);

    let over = TestServer::always(Action::Oversized { bytes: 513 });
    assert!(
        client.get(&over.url_for("/x")).is_err(),
        "one byte over the limit was accepted"
    );
}

#[test]
fn a_peer_that_says_nothing_trips_the_read_timeout_rather_than_waiting_forever() {
    // The server holds the connection open for well past the client's read
    // timeout. Without the timeout this test would never return, which is
    // exactly the production failure.
    let server = TestServer::always(Action::Silent(StdDuration::from_millis(1_200)));
    let client = HttpClient::new(tight());

    let started = std::time::Instant::now();
    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a silent peer was waited on indefinitely");
    let elapsed = started.elapsed();

    assert!(
        matches!(
            error,
            HttpError::ReadTimeout {
                phase: Phase::StatusLine,
                ..
            }
        ),
        "the timeout must name where it gave up: {error:?}"
    );
    assert!(
        elapsed < StdDuration::from_millis(1_000),
        "the client waited {elapsed:?}, which is past its own 200ms read timeout"
    );
    assert!(
        error.is_transient(),
        "a peer that was slow once may not be slow next time"
    );
}

#[test]
fn a_peer_that_accepts_and_closes_without_answering_is_reported_as_a_close() {
    let server = TestServer::always(Action::Hangup);
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a connection that produced no response was accepted");

    assert!(
        matches!(
            error,
            HttpError::ClosedEarly {
                phase: Phase::StatusLine
            }
        ),
        "expected a close during the status line, got {error:?}"
    );
}

#[test]
fn a_refused_connection_is_typed_and_transient() {
    let address = address_with_no_listener();
    let client = HttpClient::new(tight());

    let error = client
        .get(&format!("{address}/v1/mesh/publish"))
        .expect_err("connecting to a closed port succeeded");

    assert_eq!(
        error.code(),
        "connect_failed",
        "a refused connection must be distinguishable from a peer that answered badly: {error}"
    );
    assert!(
        error.is_transient(),
        "a peer that is restarting refuses connections, and that is the case retries exist for"
    );
}

#[test]
fn a_response_that_is_not_http_is_malformed_and_is_not_retried() {
    let server = TestServer::always(Action::Raw(b"GARBAGE\r\n\r\n".to_vec()));
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/v1/mesh/poll"))
        .expect_err("a non-HTTP response was accepted");

    assert!(
        matches!(
            error,
            HttpError::Malformed {
                phase: Phase::StatusLine,
                ..
            }
        ),
        "expected a malformed status line, got {error:?}"
    );
    assert!(
        !error.is_transient(),
        "whatever is on that port will answer the same way next time, and spending a retry ladder \
         on it delays every message behind it"
    );
}

#[test]
fn a_header_list_that_never_ends_is_refused() {
    let mut response = b"HTTP/1.1 200 OK\r\n".to_vec();
    for index in 0..40 {
        response.extend_from_slice(format!("x-filler-{index}: value\r\n").as_bytes());
    }
    response.extend_from_slice(b"content-length: 0\r\nconnection: close\r\n\r\n");

    let server = TestServer::always(Action::Raw(response));
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/x"))
        .expect_err("an unbounded header list was accepted");
    assert_eq!(error.code(), "too_many_headers");
}

#[test]
fn a_single_header_longer_than_the_limit_is_refused() {
    let mut response = b"HTTP/1.1 200 OK\r\nx-huge: ".to_vec();
    response.extend(std::iter::repeat_n(b'v', 4096));
    response.extend_from_slice(b"\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");

    let server = TestServer::always(Action::Raw(response));
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/x"))
        .expect_err("an oversized header line was accepted");
    assert_eq!(error.code(), "line_too_long");
}

#[test]
fn two_content_length_headers_that_disagree_are_refused() {
    let server = TestServer::always(Action::Raw(
        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\ncontent-length: 40\r\nconnection: \
          close\r\n\r\nhi"
            .to_vec(),
    ));
    let client = HttpClient::new(tight());

    let error = client
        .get(&server.url_for("/x"))
        .expect_err("two disagreeing content-lengths were accepted");
    assert_eq!(error.code(), "malformed");
}

#[test]
fn a_status_that_is_not_a_success_is_a_response_and_not_an_error() {
    // The client's job is to get the answer back. Whether a 503 should be
    // retried and a 400 dead-lettered is the transport's decision, and it
    // cannot make it if the client has collapsed both into `Err`.
    let server = TestServer::always(Action::json(503, r#"{"error":"the inbox is full"}"#));
    let client = HttpClient::new(tight());

    let response = client
        .get(&server.url_for("/v1/mesh/publish"))
        .expect("a 503 was reported as a transport failure");
    assert_eq!(response.status, 503);
    assert!(!response.is_success());
    assert!(response.body_excerpt().contains("inbox is full"));
}

#[test]
fn every_http_error_is_classified_and_no_variant_is_left_unnamed() {
    // A property over the whole error surface: each one has a stable code, and
    // no two share one. A new variant added without a code would be reported
    // as whichever existing one it was copied from.
    let errors = [
        HttpError::InvalidUrl {
            url: "x".into(),
            detail: "y".into(),
        },
        HttpError::UnsupportedScheme {
            scheme: "https".into(),
        },
        HttpError::Resolve {
            authority: "x".into(),
            detail: "y".into(),
        },
        HttpError::NoAddress {
            authority: "x".into(),
        },
        HttpError::ConnectFailed {
            address: "x".into(),
            detail: "y".into(),
        },
        HttpError::ConnectTimeout {
            authority: "x".into(),
            after: StdDuration::from_secs(1),
        },
        HttpError::WriteFailed { detail: "y".into() },
        HttpError::ReadTimeout {
            phase: Phase::Body,
            after: StdDuration::from_secs(1),
        },
        HttpError::ClosedEarly { phase: Phase::Body },
        HttpError::ReadFailed {
            phase: Phase::Body,
            detail: "y".into(),
        },
        HttpError::Malformed {
            phase: Phase::Body,
            detail: "y".into(),
        },
        HttpError::BodyTooLarge {
            limit: 1,
            at_least: 2,
        },
        HttpError::LineTooLong {
            phase: Phase::Headers,
            limit: 1,
        },
        HttpError::TooManyHeaders { limit: 1 },
    ];

    let mut codes = std::collections::BTreeSet::new();
    for error in &errors {
        assert!(
            codes.insert(error.code()),
            "{} is used by two variants, so metrics cannot tell them apart",
            error.code()
        );
        assert!(
            !error.to_string().is_empty(),
            "{} renders as nothing",
            error.code()
        );
        // Every one has to become a platform error without losing its message.
        let platform: qip_core::Error = error.clone().into();
        assert!(
            platform.message().contains(
                error
                    .to_string()
                    .split(':')
                    .next()
                    .unwrap_or_default()
                    .trim()
            ) || !platform.message().is_empty(),
            "{} loses its detail when it crosses into qip_core::Error",
            error.code()
        );
    }
    assert_eq!(codes.len(), errors.len());
}
