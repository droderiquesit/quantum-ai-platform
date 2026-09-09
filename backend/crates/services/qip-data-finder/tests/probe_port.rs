//! The probe port: what production needs, and what the offline probe refuses
//! to invent.
//!
//! The property that matters is the absence of a silent fallback. A probe that
//! quietly returned a stub when the network was unavailable would let a
//! legality verdict be reached against a robots.txt nobody fetched, and the
//! decision record would look exactly like one that was checked.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{endpoint, now, ok_head, sample};
use qip_core::error::{Error, Result};
use qip_data_finder::probe::{InMemoryProbe, NetworkProbe, RobotsFetch, SourceProbe};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

/// A one-shot HTTP server on loopback, answering every request with `response`.
///
/// Real sockets rather than a mock, because the property under test is that the
/// probe speaks HTTP through a route — and a mock of the client would prove the
/// mock. `std::net` only: no dependency, and the listener binds port zero so
/// two tests never contend.
fn serving(response: &'static str, requests: usize) -> Result<(String, JoinHandle<Vec<String>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| Error::io(format!("no loopback port: {error}")))?;
    let base = format!(
        "http://{}",
        listener
            .local_addr()
            .map_err(|error| Error::io(format!("no local address: {error}")))?
    );
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..requests {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            // Read just the request line and headers; nothing here sends a
            // body, and reading to EOF would block until the client closed.
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => request.push(byte[0]),
                }
            }
            seen.push(String::from_utf8_lossy(&request).to_string());
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
        seen
    });
    Ok((base, handle))
}

#[test]
fn a_probe_cannot_be_built_without_a_user_agent_a_publisher_could_block() -> Result<()> {
    // A publisher's only means of asking this platform to stop is to block a
    // user agent, so an anonymous probe is one that cannot be told no. The
    // refusal is at construction rather than per call, so a misconfigured
    // deployment fails at assembly and not at its first source.
    for anonymous in ["", "   "] {
        assert!(
            NetworkProbe::through("http://127.0.0.1:9105", anonymous).is_err(),
            "a probe was built with the user agent {anonymous:?}"
        );
    }
    // The premise: a named one does build, so the assertion above is about the
    // name and not about the constructor refusing everything.
    assert!(
        NetworkProbe::through("http://127.0.0.1:9105", "qip-data-finder/1.0").is_ok(),
        "a named probe must build, or the refusals above prove nothing"
    );
    Ok(())
}

#[test]
fn a_probe_addresses_an_egress_route_and_refuses_to_be_handed_a_destination() -> Result<()> {
    // ADR 0054. The client has no TLS stack; the proxy behind the route
    // originates TLS upstream. An `https` base URL is a caller who has confused
    // the route with the destination, and that distinction is the whole reason
    // this type cannot become a crawler — so it is refused by name rather than
    // downgraded, which would silently send a plaintext request to port 443.
    let refused = NetworkProbe::through("https://api.frankfurter.dev", "qip-data-finder/1.0")
        .expect_err("an https base URL must be refused");
    assert!(
        refused.message().contains("no TLS stack"),
        "the refusal must say why: {}",
        refused.message()
    );
    assert!(
        refused.message().contains("0054"),
        "the refusal must name the decision it enforces: {}",
        refused.message()
    );
    // And a base URL naming no scheme this client speaks is refused too, rather
    // than being concatenated into something that parses by accident.
    for bad in ["api.frankfurter.dev", "ftp://example.invalid", ""] {
        assert!(
            NetworkProbe::through(bad, "qip-data-finder/1.0").is_err(),
            "`{bad}` was accepted as an egress route"
        );
    }
    Ok(())
}

#[test]
fn a_route_that_answers_nothing_is_reported_unreachable_and_never_invented() -> Result<()> {
    // The property this file was written for, and it outlives the stub that
    // used to carry it: a probe that quietly returned something when the route
    // was dead would let a legality verdict be reached against a robots.txt
    // nobody fetched, and the decision record would look exactly like one that
    // was checked.
    //
    // Port 1 on loopback: nothing binds it, so this is a connection refused
    // rather than a timeout, and the test does not wait.
    let mut probe = NetworkProbe::through("http://127.0.0.1:1", "qip-data-finder/1.0")?;
    let endpoint = endpoint("https://example.com/data/prices.json")?;

    match probe.robots("example.com", now())? {
        RobotsFetch::Unreachable { reason } => {
            assert!(
                reason.contains("example.com") && reason.contains("127.0.0.1:1"),
                "the reason must name both the source and the route it was tried through: {reason}"
            );
        }
        // `Absent` would be the dangerous answer: it means the host answered
        // and has no policy, which licenses a crawl.
        other => panic!("a dead route produced {other:?} rather than an unreachable"),
    }
    let head = probe.head(&endpoint, now()).unwrap_err();
    assert!(matches!(head, Error::Unavailable(_)), "got {head:?}");
    let sample = probe.sample(&endpoint, now()).unwrap_err();
    assert!(matches!(sample, Error::Unavailable(_)), "got {sample:?}");
    Ok(())
}

#[test]
fn the_probe_reads_a_real_robots_a_real_head_and_a_real_payload_over_a_socket() -> Result<()> {
    // End to end over TCP, because every other test here proves the probe's
    // refusals and none proves it can fetch. A probe that only ever refuses is
    // the stub this replaced.
    let (base, handle) = serving(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 24\r\n\r\n\
         User-agent: *\nDisallow:\n",
        1,
    )?;
    let mut probe = NetworkProbe::through(&base, "qip-data-finder/1.0")?;
    let fetched = probe.robots("example.com", now())?;
    let RobotsFetch::Served { body, .. } = &fetched else {
        panic!("a served robots.txt came back as {fetched:?}");
    };
    assert!(body.contains("User-agent: *"), "body was {body:?}");
    // The policy parses out of what was actually served, rather than the probe
    // reporting a fetch nobody could read.
    assert!(
        fetched.policy().is_some(),
        "the served body parsed to no policy"
    );

    let requests = handle.join().map_err(|_| Error::io("server thread"))?;
    let request = requests.first().cloned().unwrap_or_default();
    assert!(
        request.starts_with("GET /robots.txt "),
        "the probe asked for something other than robots.txt: {request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("user-agent: qip-data-finder/1.0"),
        "the probe did not identify itself, so a publisher cannot block it: {request}"
    );

    // A payload, with the charset parameter stripped off the media type: the
    // schema fingerprint keys on `application/json`, and
    // `application/json; charset=utf-8` is the same media type wearing a
    // parameter.
    let (base, handle) = serving(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\n\
         Content-Length: 9\r\n\r\n{\"a\":1.0}",
        1,
    )?;
    let mut probe = NetworkProbe::through(&base, "qip-data-finder/1.0")?;
    let endpoint = endpoint("https://example.com/data/prices.json")?;
    let payload = probe.sample(&endpoint, now())?;
    assert_eq!(payload.media_type, "application/json");
    assert_eq!(payload.body, r#"{"a":1.0}"#);
    let _ = handle.join();

    // A non-2xx body is refused rather than fingerprinted. Admitting a source
    // on the shape of its own 404 is how a schema gets recorded for a page
    // nobody meant to publish.
    let (base, handle) = serving("HTTP/1.1 404 Not Found\r\nContent-Length: 3\r\n\r\nno!", 1)?;
    let mut probe = NetworkProbe::through(&base, "qip-data-finder/1.0")?;
    let refused = probe.sample(&endpoint, now()).unwrap_err();
    assert!(
        refused.message().contains("404"),
        "the refusal must name the status: {}",
        refused.message()
    );
    let _ = handle.join();
    Ok(())
}

#[test]
fn the_offline_probe_refuses_to_invent_a_response_it_was_not_given() -> Result<()> {
    let mut probe = InMemoryProbe::new();
    let endpoint = endpoint("https://example.com/data/prices.json")?;

    let error = probe.robots("example.com", now()).unwrap_err();
    assert!(matches!(error, Error::NotFound(_)));
    assert!(error.message().contains("will not invent one"));

    assert!(probe.head(&endpoint, now()).is_err());
    assert!(probe.sample(&endpoint, now()).is_err());
    Ok(())
}

#[test]
fn scripted_responses_are_consumed_in_order_and_the_last_one_repeats() -> Result<()> {
    // What lets a test say "this source served shape A and then shape B"
    // without the probe having to model time.
    let url = "https://example.com/data/prices.json";
    let endpoint = endpoint(url)?;
    let mut probe = InMemoryProbe::new()
        .with_sample(url, sample(r#"{"a":1}"#))
        .with_sample(url, sample(r#"{"a":"1"}"#))
        .with_head(url, ok_head());

    assert_eq!(probe.sample(&endpoint, now())?.body, r#"{"a":1}"#);
    assert_eq!(probe.sample(&endpoint, now())?.body, r#"{"a":"1"}"#);
    assert_eq!(
        probe.sample(&endpoint, now())?.body,
        r#"{"a":"1"}"#,
        "the final scripted response repeats rather than running out"
    );
    Ok(())
}

#[test]
fn the_offline_probe_records_every_call_it_was_asked_to_make() -> Result<()> {
    let url = "https://example.com/data/prices.json";
    let endpoint = endpoint(url)?;
    let mut probe = InMemoryProbe::new()
        .with_robots("example.com", common::permissive_robots())
        .with_head(url, ok_head())
        .with_sample(url, sample(common::QUOTE_PAYLOAD));

    probe.robots("example.com", now())?;
    probe.head(&endpoint, now())?;
    probe.sample(&endpoint, now())?;

    assert_eq!(
        probe.calls(),
        [
            "robots example.com".to_string(),
            format!("head {url}"),
            format!("sample {url}"),
        ]
    );
    Ok(())
}

#[test]
fn gathering_evidence_asks_for_robots_before_it_reads_the_payload() -> Result<()> {
    // Reading first and asking permission afterwards would make the check
    // ceremonial.
    use qip_data_finder::probe::ProbeEvidence;
    let url = "https://example.com/data/prices.json";
    let endpoint = endpoint(url)?;
    let mut probe = InMemoryProbe::new()
        .with_robots("example.com", common::permissive_robots())
        .with_head(url, ok_head())
        .with_sample(url, sample(common::QUOTE_PAYLOAD));

    let evidence = ProbeEvidence::gather(&mut probe, &endpoint, now())?;
    assert!(evidence.robots_policy().is_some());
    assert_eq!(
        probe.calls().first().map(String::as_str),
        Some("robots example.com")
    );
    assert!(!evidence.schema().is_empty());
    Ok(())
}

#[test]
fn an_endpoint_parses_into_the_host_every_legal_check_is_keyed_on() -> Result<()> {
    let parsed = endpoint("HTTPS://API.Example.COM:8443/v2/quotes?since=1")?;
    assert_eq!(parsed.host(), "api.example.com");
    assert_eq!(parsed.port(), Some(8443));
    assert_eq!(parsed.path(), "/v2/quotes?since=1");
    assert_eq!(
        parsed.robots_url(),
        "https://api.example.com:8443/robots.txt"
    );

    // A permissive parser that guessed a host would be one that guessed past
    // a denylist.
    for bad in ["example.com/data", "://example.com", "https://"] {
        assert!(
            endpoint(bad).is_err(),
            "`{bad}` must not parse into an endpoint"
        );
    }
    Ok(())
}
