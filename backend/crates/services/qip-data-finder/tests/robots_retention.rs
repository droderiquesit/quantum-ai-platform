//! R12-SEC-9065: the catalogue keeps what a robots.txt said about access,
//! never the publisher's text. `ProbeEvidence` is serialised into every
//! registered source; it used to hold the whole `RobotsFetch`, so the body
//! went with it.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{candidate, licensed_for, now, probe_for, robots_served};
use qip_contracts::governance::Usage;
use qip_core::error::Result;
use qip_data_finder::probe::{ProbeEvidence, RobotsRecord};

/// A comment: the policy parser keeps rules (the path verdicts the legality
/// check needs, so those survive by design) and drops everything else, so a
/// canary in a comment is text only the retained body could carry.
const CANARY: &str = "# canary-publisher-text-7f3a";

#[test]
fn a_served_robots_body_never_reaches_the_serialised_evidence() -> Result<()> {
    let url = "https://example.com/data";
    let c = candidate(
        "robots-retention",
        url,
        licensed_for(&[Usage::Research])?,
        &["EUR/USD"],
    )?;
    let body = format!("User-agent: *\n{CANARY}\nDisallow: /private/\n");
    let mut probe = probe_for(url, "example.com", robots_served(&body));
    let evidence = ProbeEvidence::gather(&mut probe, c.endpoint(), now())?;

    // Premise: the text was served and parsed — the policy exists and the
    // record knows its size — so absence below is the narrowing's doing.
    assert!(evidence.robots_policy().is_some());
    assert!(matches!(
        evidence.robots(),
        RobotsRecord::Served { bytes, .. } if *bytes == body.len()
    ));
    let json = serde_json::to_string(&evidence).unwrap();
    assert!(json.contains("\"bytes\""), "{json}");
    assert!(
        !json.contains("canary-publisher-text"),
        "body leaked: {json}"
    );
    assert!(!json.contains("\"body\""), "{json}");
    Ok(())
}
