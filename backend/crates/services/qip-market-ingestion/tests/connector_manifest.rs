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
use qip_market_ingestion::connectors::{
    alpaca_bars, coinbase_ticker, frankfurter_rates, kalshi_markets,
};

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
        companion: None,
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
    // The versioned path on the host the vendor moved to. `api.frankfurter.app`
    // answered every request with a 301 from 2026-09-04, which the transport
    // refuses to follow, and the unversioned `/latest` answers 404 on the new
    // host — so the old pair would fail the health probe, and the shipped
    // manifest, the Envoy cluster and the Terraform allowlist must name the
    // new host together.
    assert_eq!(frankfurter.endpoint.path, "/v1/latest");
    assert_eq!(frankfurter.endpoint.health_path(), "/v1/latest");
    assert!(
        frankfurter.provider.contains(&format!(
            "({})",
            frankfurter_rates::FrankfurterRatesConnector::UPSTREAM_HOST
        )),
        "the manifest documents a host other than the one the code names: {}",
        frankfurter.provider
    );
    assert_eq!(
        frankfurter.publication_delay(),
        Duration::from_hours(16),
        "the ECB publishes these rates hours after the date they are stamped with, and a delay \
         of zero would make each one knowable at midnight on its own reference date"
    );
    Ok(())
}

#[test]
fn the_kalshi_manifest_is_open_polls_the_recorded_query_and_probes_the_cheap_status_path()
-> Result<()> {
    // The manifest's query is what the fixture was recorded with, so the
    // harness exercises the request a deployment would make; the health path
    // is the status endpoint so a liveness probe is not a 40 KB page.
    let kalshi = kalshi_markets::KalshiMarketsConnector::shipped_manifest()?;
    assert_eq!(kalshi.source_id, "kalshi-markets");
    assert_eq!(kalshi.protocol, Protocol::Poll);
    assert_eq!(kalshi.auth.scheme, AuthScheme::None);
    assert!(
        kalshi.auth.secret.is_none(),
        "an open endpoint named a credential"
    );
    assert_eq!(kalshi.endpoint.path, "/trade-api/v2/markets");
    assert_eq!(
        kalshi.endpoint.health_path(),
        "/trade-api/v2/exchange/status"
    );
    let query: Vec<(&str, &str)> = kalshi
        .endpoint
        .query
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        query,
        [
            ("limit", "20"),
            ("mve_filter", "exclude"),
            ("status", "open")
        ]
    );
    assert!(
        kalshi.provider.contains(&format!(
            "({})",
            kalshi_markets::KalshiMarketsConnector::UPSTREAM_HOST
        )),
        "{}",
        kalshi.provider
    );
    assert!(
        !kalshi.is_configured(),
        "the shipped manifest must not carry an address: the host is in no allowlist"
    );
    Ok(())
}

#[test]
fn the_alpaca_manifest_names_its_credential_by_reference_and_the_deployment_must_supply_both()
-> Result<()> {
    // The credential is a variable name and never a value; an unconfigured
    // deployment is told about the address and the credential separately.
    let alpaca = alpaca_bars::AlpacaBarsConnector::shipped_manifest()?;
    assert_eq!(alpaca.source_id, "alpaca-daily-bars");
    assert_eq!(alpaca.protocol, Protocol::Rest);
    assert_eq!(alpaca.auth.scheme, AuthScheme::Header);
    assert_eq!(
        alpaca.auth.header.as_deref(),
        Some(alpaca_bars::AlpacaBarsConnector::SECRET_KEY_HEADER)
    );
    let secret = alpaca
        .auth
        .secret
        .as_ref()
        .expect("an authenticated manifest names its secret");
    assert_eq!(secret.variable(), "QIP_ALPACA_API_SECRET_KEY");
    let companion = alpaca
        .auth
        .companion
        .as_ref()
        .expect("Alpaca reads two headers, and the manifest names the second");
    assert_eq!(
        companion.header,
        alpaca_bars::AlpacaBarsConnector::KEY_ID_HEADER
    );
    assert_eq!(companion.secret.variable(), "QIP_ALPACA_API_KEY_ID");
    // The manifest text itself carries no field that could hold a value: the
    // only places a credential's name appears are the two `variable` fields.
    let text: serde_json::Value = serde_json::from_str(alpaca_bars::MANIFEST)?;
    assert_eq!(
        text["auth"]
            .as_object()
            .map(|auth| auth.keys().cloned().collect::<Vec<_>>()),
        Some(vec![
            "companion".to_string(),
            "header".to_string(),
            "scheme".to_string(),
            "secret".to_string()
        ])
    );
    assert_eq!(
        text["auth"]["companion"]
            .as_object()
            .map(|companion| companion.keys().cloned().collect::<Vec<_>>()),
        Some(vec!["header".to_string(), "secret".to_string()])
    );
    assert!(
        alpaca.provider.contains(&format!(
            "({})",
            alpaca_bars::AlpacaBarsConnector::UPSTREAM_HOST
        )),
        "{}",
        alpaca.provider
    );

    // Three things missing, each on its own line: the address, then the two
    // credentials in the order the transport sends them. A deployment that
    // mounted the secret key and forgot the key id reads the third line.
    assert!(!alpaca.is_configured());
    let missing = alpaca.missing_configuration();
    assert_eq!(missing.len(), 3, "{missing:?}");
    assert!(missing[0].contains("base_url"), "{}", missing[0]);
    assert!(
        missing[1].contains("`QIP_ALPACA_API_SECRET_KEY`")
            && missing[1].contains("`QIP_ALPACA_API_SECRET_KEY_FILE`"),
        "the missing-credential message does not name the variable and its _FILE form: {}",
        missing[1]
    );
    assert!(
        missing[2].contains("`QIP_ALPACA_API_KEY_ID`")
            && missing[2].contains("`QIP_ALPACA_API_KEY_ID_FILE`"),
        "the companion credential is not reported on its own line: {}",
        missing[2]
    );
    Ok(())
}

// --- two credential headers ------------------------------------------------

use qip_market_ingestion::connector::manifest::CompanionHeader;
use std::collections::BTreeMap;

/// The manifest fixture with a two-header `auth` stanza in Alpaca's shape.
fn two_header_manifest_json() -> String {
    manifest_json("test-source", 0, "1.0").replace(
        r#""auth": { "scheme": "none" }"#,
        r#""auth": {
    "scheme": "header",
    "header": "x-secret-key",
    "secret": { "variable": "QIP_TEST_SECRET_KEY" },
    "companion": { "header": "x-key-id", "secret": { "variable": "QIP_TEST_KEY_ID" } }
  }"#,
    )
}

/// Two credential files as the Secret Manager volume would project them,
/// each with the trailing newline an editor leaves, and a lookup that names
/// them through the `_FILE` variables and nothing else.
fn two_credential_files(tag: &str) -> Result<(std::path::PathBuf, BTreeMap<String, String>)> {
    let dir = std::env::temp_dir().join(format!("qip-two-header-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("secret-key"), "test-secret-value-9f2a\n")?;
    std::fs::write(dir.join("key-id"), "test-key-id-7b3c\n")?;
    let lookup: BTreeMap<String, String> = [
        (
            "QIP_TEST_SECRET_KEY_FILE",
            dir.join("secret-key").display().to_string(),
        ),
        (
            "QIP_TEST_KEY_ID_FILE",
            dir.join("key-id").display().to_string(),
        ),
    ]
    .into_iter()
    .map(|(name, path)| (name.to_string(), path))
    .collect();
    Ok((dir, lookup))
}

#[test]
fn a_manifest_naming_two_credential_headers_validates_and_resolves_both_from_files() -> Result<()> {
    // Alpaca reads an account id in one header and a secret key in another,
    // and the connector shipped at 712c5d3 could name only the second, so a
    // deployment would have authenticated with half a credential and read
    // 401 at the health check. Both must be nameable, both by reference,
    // and both must resolve through the `_FILE` indirection the deployment
    // actually uses.
    let manifest = SourceManifest::from_json(&two_header_manifest_json())?;
    let companion = manifest
        .auth
        .companion
        .as_ref()
        .expect("the companion header parsed");
    assert_eq!(companion.header, "x-key-id");
    assert_eq!(companion.secret.variable(), "QIP_TEST_KEY_ID");
    assert_eq!(
        manifest
            .auth
            .secrets()
            .iter()
            .map(|secret| secret.variable())
            .collect::<Vec<_>>(),
        ["QIP_TEST_SECRET_KEY", "QIP_TEST_KEY_ID"],
        "the secrets are reported in the order the transport sends them"
    );

    // With nothing supplied, both credentials are reported, each on its own
    // line, so an operator with one of two learns which is left.
    let nothing = |_: &str| None;
    let missing = manifest.missing_configuration_with(&|secret| secret.resolve_with(&nothing));
    assert_eq!(missing.len(), 2, "{missing:?}");
    assert!(
        missing[0].contains("`QIP_TEST_SECRET_KEY_FILE`"),
        "{}",
        missing[0]
    );
    assert!(
        missing[1].contains("`QIP_TEST_KEY_ID_FILE`"),
        "{}",
        missing[1]
    );

    // With only the secret key mounted, the one line left names the key id.
    let (dir, lookup) = two_credential_files("manifest")?;
    let only_secret_key = |name: &str| {
        (name == "QIP_TEST_SECRET_KEY_FILE")
            .then(|| lookup.get(name).cloned())
            .flatten()
    };
    let missing =
        manifest.missing_configuration_with(&|secret| secret.resolve_with(&only_secret_key));
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(missing[0].contains("`QIP_TEST_KEY_ID`"), "{}", missing[0]);

    // With both, nothing is missing and each value is the file's contents
    // with the editor's newline stripped.
    let both = |name: &str| lookup.get(name).cloned();
    let resolve = |secret: &SecretRef| secret.resolve_with(&both);
    assert!(manifest.missing_configuration_with(&resolve).is_empty());
    let secret_key = manifest
        .auth
        .secret
        .as_ref()
        .expect("premise: the primary secret is named");
    assert_eq!(
        secret_key.resolve_with(&both)?.as_deref(),
        Some("test-secret-value-9f2a")
    );
    assert_eq!(
        companion.secret.resolve_with(&both)?.as_deref(),
        Some("test-key-id-7b3c")
    );

    // And the two-header shape survives a JSON round trip with the companion
    // still there; a serialiser that dropped it would ship a manifest that
    // validates and authenticates with one header.
    let restored = SourceManifest::from_json(&manifest.to_json()?)?;
    assert_eq!(restored, manifest);
    assert!(restored.to_json()?.contains("\"companion\""));

    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn a_manifest_that_names_the_same_header_twice_is_refused() -> Result<()> {
    // Two values under one name reach the vendor as whichever its parser
    // keeps; the deployment would authenticate with a credential nobody
    // chose, and with the wrong one half the time.
    let text = two_header_manifest_json()
        .replace(r#""header": "x-key-id""#, r#""header": "x-secret-key""#);
    // Premise: the edit produced the duplicate, and nothing else changed.
    assert_eq!(text.matches(r#""header": "x-secret-key""#).count(), 2);

    let error = SourceManifest::from_json(&text)
        .expect_err("a manifest routing two secrets to one header was accepted");
    assert!(
        error
            .message()
            .contains("both the credential header and its companion"),
        "{}",
        error.message()
    );

    // Built in code rather than parsed, the same shape is refused by the same
    // rule: there is no constructor that skips it.
    let spec = AuthSpec::two_headers(
        "x-api-key",
        SecretRef::new("QIP_TEST_SECRET_KEY")?,
        CompanionHeader::new("x-api-key", SecretRef::new("QIP_TEST_KEY_ID")?),
    );
    spec.validate()
        .expect_err("AuthSpec::two_headers with one name twice validated");
    Ok(())
}

#[test]
fn two_credential_headers_reading_one_variable_are_refused() -> Result<()> {
    // One value sent under two names means one of the two headers carries
    // the wrong thing — the secret key sent as the key id, which the vendor
    // logs as an identifier.
    let text = two_header_manifest_json().replace(
        r#""secret": { "variable": "QIP_TEST_KEY_ID" }"#,
        r#""secret": { "variable": "QIP_TEST_SECRET_KEY" }"#,
    );
    assert_eq!(
        text.matches("QIP_TEST_SECRET_KEY").count(),
        2,
        "premise: both read one variable"
    );

    let error = SourceManifest::from_json(&text)
        .expect_err("a manifest sending one secret under two names was accepted");
    assert!(
        error.message().contains("both read `QIP_TEST_SECRET_KEY`"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_secret_routed_to_a_header_the_transport_owns_is_refused() -> Result<()> {
    // `qip_transport::HttpRequest::with_header` drops `host`, `content-length`,
    // `connection` and `transfer-encoding` silently, because it writes them
    // itself. A manifest routing the key id to `host` would send no key id,
    // answer 401 at the vendor, and never say why. `accept` is sent — twice —
    // and logged by every proxy as content negotiation.
    for owned in ["host", "content-length", "accept"] {
        let text = two_header_manifest_json().replace(
            r#""header": "x-key-id""#,
            &format!(r#""header": "{owned}""#),
        );
        assert!(
            text.contains(&format!(r#""header": "{owned}""#)),
            "premise: the companion now names `{owned}`"
        );
        let error = SourceManifest::from_json(&text).expect_err(&format!(
            "a manifest routing a secret to `{owned}` was accepted"
        ));
        assert!(
            error
                .message()
                .contains(&format!("`{owned}` is not a credential header")),
            "{}",
            error.message()
        );
    }
    // The same rule holds the primary header; it is not a companion-only check.
    let spec = AuthSpec::header("host", SecretRef::new("QIP_TEST_SECRET_KEY")?);
    let error = spec
        .validate()
        .expect_err("the primary credential was routed to `host`");
    assert!(
        error
            .message()
            .contains("`host` is not a credential header"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn the_one_header_manifests_written_before_the_companion_existed_still_validate() -> Result<()> {
    // The field defaults so that no manifest already in the tree has to
    // change: Coinbase, Frankfurter and Kalshi are open, and the generic
    // one-header shape parses with no companion and serialises without
    // inventing one.
    let one_header = manifest_json("test-source", 0, "1.0").replace(
        r#""auth": { "scheme": "none" }"#,
        r#""auth": { "scheme": "header", "header": "x-api-key", "secret": { "variable": "QIP_VENDOR_API_KEY" } }"#,
    );
    assert!(
        !one_header.contains("companion"),
        "premise: the text names no companion"
    );
    let manifest = SourceManifest::from_json(&one_header)?;
    assert_eq!(manifest.auth.scheme, AuthScheme::Header);
    assert!(manifest.auth.companion.is_none());
    assert_eq!(manifest.auth.secrets().len(), 1);
    assert!(
        !manifest.to_json()?.contains("companion"),
        "a manifest with no companion serialised one"
    );

    for shipped in [
        coinbase_ticker::CoinbaseTickerConnector::shipped_manifest()?,
        frankfurter_rates::FrankfurterRatesConnector::shipped_manifest()?,
        kalshi_markets::KalshiMarketsConnector::shipped_manifest()?,
    ] {
        assert!(
            shipped.auth.companion.is_none(),
            "{} grew a companion",
            shipped.source_id
        );
        assert!(
            shipped.auth.secrets().is_empty(),
            "{} names a secret",
            shipped.source_id
        );
        shipped.validate()?;
    }
    Ok(())
}
