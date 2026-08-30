//! The manifest, and the things it must refuse.
//!
//! A manifest is checked into git, printed in a support ticket and pasted into
//! a chat window. Most of these tests are about what cannot be written in one.

#![allow(clippy::panic_in_result_fn)]

mod connector_common;

use connector_common::manifest_json;
use qip_core::Duration;
use qip_core::error::Result;
use qip_market_ingestion::connector::manifest::{AuthScheme, AuthSpec, SchemaVersion, SecretRef};
use qip_market_ingestion::connector::transport::HttpSourceTransport;
use qip_market_ingestion::connector::{Protocol, SourceManifest};
use qip_market_ingestion::connectors::{coinbase_ticker, frankfurter_rates};

#[test]
fn a_manifest_cannot_carry_a_credential_because_no_field_can_hold_one() -> Result<()> {
    // The failure prevented is a live key in git history and in every support
    // ticket that quotes the config. `deny_unknown_fields` is what turns the
    // attempt into a parse error rather than an ignored field.
    let text = manifest_json("test-source", 0, "1.0").replace(
        r#""auth": { "scheme": "none" }"#,
        r#""auth": { "scheme": "header", "header": "x-api-key", "value": "sk-live-9f2ab41c" }"#,
    );

    let error = SourceManifest::from_json(&text)
        .expect_err("a manifest with an inline credential was accepted");
    assert!(
        error.message().contains("value"),
        "the refusal does not name the offending field: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_pasted_key_written_where_a_variable_name_belongs_is_refused() -> Result<()> {
    // The second line of the same defence: the field that *is* there holds a
    // name, and a name is SCREAMING_SNAKE_CASE — which a key is not.
    for pasted in ["sk-live-9f2ab41c", "AKIAIOSFODNN7/EXAMPLE", "token", "a"] {
        SecretRef::new(pasted).expect_err(&format!(
            "{pasted:?} was accepted as a deployment variable name, so a credential could be \
             written where its name belongs"
        ));
    }
    SecretRef::new("QIP_VENDOR_API_KEY").expect("a real variable name was refused");
    Ok(())
}

#[test]
fn an_open_endpoint_may_not_quietly_name_a_credential() -> Result<()> {
    let spec = AuthSpec {
        scheme: AuthScheme::None,
        header: None,
        secret: Some(SecretRef::new("QIP_VENDOR_API_KEY")?),
    };

    let error = spec
        .validate()
        .expect_err("a source declared open was allowed to carry a credential");
    assert!(error.message().contains("audits"), "{}", error.message());
    Ok(())
}

#[test]
fn a_misspelled_field_is_refused_rather_than_silently_defaulted() -> Result<()> {
    // `poll_interval_millis` would otherwise parse, take the default and poll
    // at a cadence nobody chose: a rate-limit ban found in production instead
    // of a parse error found in review.
    let text = manifest_json("test-source", 0, "1.0").replace(
        r#""poll_interval_ms": 1000"#,
        r#""poll_interval_millis": 1000"#,
    );

    SourceManifest::from_json(&text)
        .expect_err("a manifest with a misspelled field parsed and took a default");
    Ok(())
}

#[test]
fn a_manifest_that_polls_faster_than_its_own_rate_limit_is_refused() -> Result<()> {
    let text = manifest_json("test-source", 0, "1.0").replace(
        r#""requests": 10, "per_ms": 1000"#,
        r#""requests": 1, "per_ms": 60000"#,
    );

    let error = SourceManifest::from_json(&text)
        .expect_err("a manifest breaching its own rate limit was accepted");
    assert!(
        error.message().contains("ban"),
        "the refusal does not say what happens next: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_freshness_sla_shorter_than_the_poll_interval_is_refused() -> Result<()> {
    // The feed would be stale between every pair of polls, so the alarm would
    // fire on the schedule rather than on the source — and then be muted.
    let text = manifest_json("test-source", 0, "1.0")
        .replace(r#""freshness_sla_ms": 60000"#, r#""freshness_sla_ms": 500"#);

    let error = SourceManifest::from_json(&text)
        .expect_err("an SLA shorter than the poll interval was accepted");
    assert!(
        error.message().contains("stale between"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn an_endpoint_path_may_not_carry_its_own_query() -> Result<()> {
    // The connector builds the query; a second `?` puts the parameters where
    // the source does not read them — and is how a credential ends up in a URL.
    let text = manifest_json("test-source", 0, "1.0")
        .replace(r#""path": "/v1/events""#, r#""path": "/v1/events?key=abc""#);

    SourceManifest::from_json(&text).expect_err("an endpoint path with a query was accepted");
    Ok(())
}

#[test]
fn a_manifest_round_trips_through_json_without_losing_a_field() -> Result<()> {
    let original = SourceManifest::from_json(&manifest_json("test-source", 900_000, "2.7"))?;
    let restored = SourceManifest::from_json(&original.to_json()?)?;

    assert_eq!(original, restored);
    assert_eq!(restored.schema.version, SchemaVersion::new(2, 7));
    assert_eq!(restored.publication_delay(), Duration::from_mins(15));
    Ok(())
}

#[test]
fn a_schema_version_admits_a_newer_minor_and_refuses_a_newer_major() -> Result<()> {
    let written_for = SchemaVersion::new(1, 2);

    assert!(written_for.admits(SchemaVersion::new(1, 2)));
    assert!(
        written_for.admits(SchemaVersion::new(1, 9)),
        "a source adding an optional field stopped a connector, so the feed goes down whenever a \
         provider ships"
    );
    assert!(
        !written_for.admits(SchemaVersion::new(2, 0)),
        "a connector kept decoding across a major version, where the fields keep their names and \
         change their meaning"
    );
    assert!(
        !written_for.admits(SchemaVersion::new(1, 1)),
        "a connector that needs a field introduced in 1.2 accepted a payload from 1.1"
    );
    Ok(())
}

#[test]
fn an_unconfigured_manifest_names_each_thing_the_deployment_must_supply() -> Result<()> {
    // Each on its own, so an operator with two of three learns which one is
    // left instead of re-checking all of them.
    let manifest = coinbase_ticker::CoinbaseTickerConnector::shipped_manifest()?;

    assert!(!manifest.is_configured());
    let missing = manifest.missing_configuration();
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(
        missing[0].contains("base_url") && missing[0].contains("TLS"),
        "the missing-configuration message does not say why a plaintext address is needed: {}",
        missing[0]
    );
    Ok(())
}

#[test]
fn an_unconfigured_manifest_refuses_to_open_a_socket_rather_than_guessing_an_address() -> Result<()>
{
    let manifest = coinbase_ticker::CoinbaseTickerConnector::shipped_manifest()?;

    let error = HttpSourceTransport::connect(&manifest)
        .expect_err("a transport was built for a manifest with no address");
    assert!(
        error
            .message()
            .contains("will not substitute recorded data"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn an_https_address_is_refused_by_name_rather_than_downgraded_to_plaintext() -> Result<()> {
    // The connector must not silently send a request — and, for a source that
    // needed one, a credential — across the internet in clear text.
    let mut manifest = coinbase_ticker::CoinbaseTickerConnector::shipped_manifest()?;
    manifest.endpoint.base_url = Some("https://api.exchange.coinbase.com".to_string());

    let error = HttpSourceTransport::connect(&manifest)
        .expect_err("an https address was accepted by a transport with no TLS stack");
    assert!(
        error.message().contains("scheme"),
        "the refusal does not name the scheme: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn the_shipped_example_manifests_validate_and_describe_free_public_sources() -> Result<()> {
    // The examples are compiled and validated rather than pasted into a
    // document, so an example that has stopped being true fails the build.
    let coinbase = coinbase_ticker::CoinbaseTickerConnector::shipped_manifest()?;
    assert_eq!(coinbase.source_id, "coinbase-spot-ticker");
    assert_eq!(coinbase.protocol, Protocol::Rest);
    assert_eq!(coinbase.auth.scheme, AuthScheme::None);
    assert!(
        coinbase.auth.secret.is_none(),
        "a free endpoint named a credential"
    );

    let frankfurter = frankfurter_rates::FrankfurterRatesConnector::shipped_manifest()?;
    assert_eq!(frankfurter.source_id, "frankfurter-ecb-reference-rates");
    assert_eq!(frankfurter.protocol, Protocol::Poll);
    assert_eq!(frankfurter.auth.scheme, AuthScheme::None);
    assert_eq!(
        frankfurter.publication_delay(),
        Duration::from_hours(16),
        "the ECB publishes these rates hours after the date they are stamped with, and a delay \
         of zero would make each one knowable at midnight on its own reference date"
    );
    Ok(())
}
