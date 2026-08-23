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
use qip_data_finder::probe::{InMemoryProbe, NetworkProbe, SourceProbe};

#[test]
fn the_network_probe_reports_unavailable_and_names_its_missing_configuration() -> Result<()> {
    let mut probe = NetworkProbe::unconfigured();
    let endpoint = endpoint("https://example.com/data/prices.json")?;

    let attempts: Vec<Error> = vec![
        probe.robots("example.com", now()).unwrap_err(),
        probe.head(&endpoint, now()).unwrap_err(),
        probe.sample(&endpoint, now()).unwrap_err(),
    ];

    for error in &attempts {
        assert!(
            matches!(error, Error::Unavailable(_)),
            "a caller distinguishing `not configured here` from `broken` needs the code, \
             and got {error:?}"
        );
        let message = error.message();
        for requirement in [
            "TLS",
            "egress policy",
            "user-agent identity",
            "trust root",
            "per-host credentials",
        ] {
            assert!(
                message.contains(requirement),
                "the error must name `{requirement}`, and said: {message}"
            );
        }
    }
    Ok(())
}

#[test]
fn configuring_the_network_probe_shortens_the_list_without_ever_emptying_it() -> Result<()> {
    // The transport is a build-time fact, not a setting. A list that could
    // empty itself would suggest this probe starts working once the
    // environment is right.
    let configured = NetworkProbe::unconfigured()
        .identified_as("qip-data-finder/1.0 (+https://example.invalid/crawler)")
        .through_egress("egress-policy: data-finder")
        .trusting("/etc/ssl/certs/ca-certificates.crt")
        .with_credential("vendor.example", "secret://vendor-api-key");

    let missing = configured.missing_configuration();
    assert_eq!(missing.len(), 1);
    assert!(missing[0].contains("no transport is linked into this build"));
    assert!(missing[0].contains("0009-tiered-dependency-policy"));

    assert!(
        NetworkProbe::unconfigured().missing_configuration().len() > missing.len(),
        "configuring the probe must account for something"
    );
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
