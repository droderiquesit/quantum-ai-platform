//! §7.6.6: registration is not ingestion.
//!
//! The crawler samples enough to register a feed and its schema. What it may
//! keep afterwards is the shape and a manifest with a hash — never the
//! publisher's text (§56.4, rule 36).
//!
//! The failure these tests prevent has already happened here. `ProbeEvidence`
//! held the whole `PayloadSample`, body included; `Source` holds a
//! `ProbeEvidence`; `RegisteredSource` holds a `Source`; and all three derive
//! `Serialize`. So every source the finder registered carried a verbatim copy
//! of the publisher's own text into the catalogue, and out through anything
//! that wrote a registered source to a journal, a status body or a snapshot.
//! Nothing in the crate refused it, because nothing was looking: the rule was
//! scored as having nothing to enforce.
//!
//! These tests assert against the **serialised** registered source rather than
//! against the accessors, because the accessors are not what leaks. A field
//! that no getter exposes still appears in the JSON, and the JSON is what a
//! journal writes.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{AGENT, candidate, licensed_for, now, ok_head, permissive_robots};
use qip_contracts::governance::Usage;
use qip_core::Duration;
use qip_core::error::Result;
use qip_data_finder::finder::{DataFinder, FinderConfig};
use qip_data_finder::legal::RateLimit;
use qip_data_finder::probe::{InMemoryProbe, PayloadSample};

const URL: &str = "https://example.com/data/prices.json";

/// A payload whose values are not derivable from its field names, so a test
/// asserting the values are gone cannot be satisfied by the schema.
///
/// `PRICE_TEXT` is the trap this file exists for: a delimited token that
/// appears nowhere else in a registered source, so `contains` over it is a
/// real question. The field *names* deliberately survive — they are the
/// schema, which registration is entitled to keep — and asserting on a name
/// would therefore prove nothing.
const PRICE_TEXT: &str = "Kalgoorlie consignment held at the assay office";
const QUOTED_BID: &str = "10.253917";

fn sampled_payload() -> String {
    format!(r#"{{"symbol":"EU0001","bid":{QUOTED_BID},"note":"{PRICE_TEXT}"}}"#)
}

fn probe() -> InMemoryProbe {
    InMemoryProbe::new()
        .with_robots("example.com", permissive_robots())
        .with_head(URL, ok_head())
        .with_sample(
            URL,
            PayloadSample {
                body: sampled_payload(),
                media_type: "application/json".to_string(),
                payload_at: Some(now()),
                latency: Duration::from_millis(55),
            },
        )
}

fn registering_finder(seed: u64) -> Result<DataFinder> {
    let config = FinderConfig::new(AGENT, Usage::Derive, "market-data", seed)?
        .with_default_rate_limit(RateLimit::new(60, Duration::from_mins(1))?);
    Ok(DataFinder::new(config))
}

/// Register one source through the ordinary lifecycle and return its JSON.
fn registered_source_json(seed: u64) -> Result<String> {
    let mut finder = registering_finder(seed)?;
    let mut probe = probe();
    let decisions = finder.assess(
        vec![candidate(
            "assay-feed",
            URL,
            licensed_for(&[Usage::Research, Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    // The premise. A deferred or rejected candidate never becomes a
    // `RegisteredSource`, and a test that serialised nothing would find no
    // text in it and pass for ever.
    assert!(
        decisions[0].is_registered(),
        "the candidate was not registered, so nothing was serialised: {}",
        decisions[0].reasoning().describe()
    );
    let registered = finder
        .registered("assay-feed")
        .ok_or_else(|| qip_core::error::Error::not_found("the source just registered"))?;
    serde_json::to_string(registered)
        .map_err(|error| qip_core::error::Error::invalid(error.to_string()))
}

#[test]
fn a_registered_source_carries_no_copy_of_the_text_the_probe_sampled() -> Result<()> {
    let json = registered_source_json(11)?;

    // Premise: the text really was in the payload the probe served, so its
    // absence below is the crate dropping it rather than the fixture never
    // having had it.
    assert!(
        sampled_payload().contains(PRICE_TEXT),
        "the fixture payload does not contain the text this test tracks"
    );
    assert!(sampled_payload().contains(QUOTED_BID));

    // The property. Neither the prose nor the quoted number survives
    // registration, in any field, under any name.
    assert!(
        !json.contains(PRICE_TEXT),
        "the registered source retains the publisher's text: {json}"
    );
    assert!(
        !json.contains(QUOTED_BID),
        "the registered source retains a value from the sampled body: {json}"
    );

    // And the schema — what registration is entitled to keep — did survive,
    // so the assertions above are not passing because nothing was probed.
    assert!(
        json.contains("symbol") && json.contains("note"),
        "the schema the probe read is missing, so the source was not probed: {json}"
    );
    Ok(())
}

#[test]
fn what_registration_keeps_of_a_sample_is_a_manifest_whose_hash_is_of_the_bytes_served()
-> Result<()> {
    let mut finder = registering_finder(12)?;
    let mut probe = probe();
    let decisions = finder.assess(
        vec![candidate(
            "assay-feed",
            URL,
            licensed_for(&[Usage::Research, Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    assert!(decisions[0].is_registered());
    let registered = finder
        .registered("assay-feed")
        .ok_or_else(|| qip_core::error::Error::not_found("the source just registered"))?;
    let manifest = registered.source().evidence().sample();

    let body = sampled_payload();
    // Premise: the body is not empty, so a hash of it is not the hash of the
    // empty string, which is what a manifest built from a dropped body would
    // carry.
    assert!(!body.is_empty());
    assert_ne!(
        qip_core::hash::sha256_hex(body.as_bytes()),
        qip_core::hash::sha256_hex(b""),
        "the fixture body hashes as empty, so the assertion below proves nothing"
    );

    // The property: the manifest identifies the bytes as served, so a later
    // probe can be compared against it without either sample being kept.
    assert_eq!(
        manifest.content_hash(),
        qip_core::hash::sha256_hex(body.as_bytes()),
        "the manifest hash is not of the bytes the probe served"
    );
    assert_eq!(manifest.bytes(), body.len());
    assert_eq!(manifest.media_type(), "application/json");
    assert_eq!(manifest.payload_at(), Some(now()));
    Ok(())
}
