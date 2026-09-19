//! §7.6.6's sixth rule, driven through the lifecycle a production caller runs.
//!
//! The unit tests beside `personal_data.rs` prove the screen classifies a
//! field name. These prove the thing that actually matters: that the screen
//! sits on the path `DataFinder::assess` takes, that a source it refuses does
//! not reach the registry, and — the half a gate is usually missing — that a
//! source it clears still registers. A gate that refuses everything passes
//! every "it refused" test ever written and is useless, which is why each
//! refusal here is paired with an admission that differs in exactly one thing.
//!
//! The chain these run on is the one §7.4 credits with a production caller:
//! `qip-deepbrain`'s `DiscoveryDesk` calls `Platform::assess_sources`, which
//! calls `DataFinder::assess`. No deployed process runs it — nothing has been
//! applied and the deep brain needs an operator's candidate file — so this is
//! a production *path*, not a deployed one.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{AGENT, candidate, licensed_for, now, ok_head, robots_served, sample};
use qip_contracts::governance::Usage;
use qip_core::Duration;
use qip_core::error::Result;
use qip_data_finder::decision::DecisionOutcome;
use qip_data_finder::finder::{DataFinder, FinderConfig};
use qip_data_finder::legal::RateLimit;
use qip_data_finder::probe::InMemoryProbe;

const URL: &str = "https://example.com/data/prices.json";
const HOST: &str = "example.com";

fn polite_robots() -> qip_data_finder::probe::RobotsFetch {
    robots_served("User-agent: *\nAllow: /data/\nDisallow: /admin/\nCrawl-delay: 4\n")
}

fn finder() -> Result<DataFinder> {
    Ok(DataFinder::new(
        FinderConfig::new(AGENT, Usage::Derive, "market-data", 11)?
            .with_default_rate_limit(RateLimit::new(60, Duration::from_mins(1))?),
    ))
}

/// Assess one candidate whose probe serves `body`, and give back the outcome
/// and whether the finder's registry ended up holding it.
///
/// Everything but the sampled body is held identical across the tests below,
/// so a difference in outcome is attributable to the payload and to nothing
/// else. That is the whole design of these tests: the refusing case and the
/// admitting case differ in one variable.
fn assess_with_body(id: &str, body: &str) -> Result<(DecisionOutcome, bool)> {
    let mut finder = finder()?;
    // Built directly rather than from `probe_for`, which already enqueues a
    // clean quote payload: `InMemoryProbe` scripts samples as a FIFO queue,
    // so adding one to that helper appends behind it and the assessment reads
    // the helper's body instead of this test's. That is how the first draft
    // of this suite passed a test it was not actually running.
    let mut probe = InMemoryProbe::new()
        .with_robots(HOST, polite_robots())
        .with_head(URL, ok_head())
        .with_sample(URL, sample(body));
    let decisions = finder.assess(
        vec![candidate(
            id,
            URL,
            licensed_for(&[Usage::Research, Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    assert_eq!(decisions.len(), 1, "premise: one candidate, one decision");
    let held = finder.registered(id).is_some();
    Ok((decisions[0].outcome().clone(), held))
}

/// The rule itself: a sampled payload naming a natural person's identifiers is
/// refused, and the source never reaches the registry the rest of the platform
/// reads as "currently collected".
///
/// Until this existed, §7.6.6's sixth rule was a sentence in a governance
/// table and nothing in the tree could refuse anything on it — the shape
/// `MaxExpectedShortfall` shipped in.
#[test]
fn a_source_whose_sampled_payload_carries_personal_data_is_not_registered() -> Result<()> {
    let (outcome, held) = assess_with_body(
        "subscriber-list",
        r#"{"subscriber_email":"a@b.example","date_of_birth":"1980-01-01","balance":41.0}"#,
    )?;
    match &outcome {
        DecisionOutcome::Rejected { reason } => {
            // Delimited, not a bare substring: "personal data" appears in the
            // module's own prose, and a `contains` on a word the surrounding
            // text always carries is the trap this repository has already
            // been bitten by once.
            assert!(
                reason.contains("`subscriber_email` carries a contact detail"),
                "the refusal must name the offending field and its class, got {reason}"
            );
            assert!(
                reason.contains("`date_of_birth` carries a birth detail"),
                "every finding is reported, not only the first, got {reason}"
            );
        }
        other => panic!("a payload carrying personal data must be rejected, got {other:?}"),
    }
    assert!(
        !held,
        "a refused source must not reach the registry that backs DataFinder::registered"
    );
    Ok(())
}

/// The other half of a working gate, and the half usually missing: the same
/// source, the same licence, the same robots, the same everything — with a
/// payload that names no personal identifier — still registers.
///
/// Without this, a screen that rejected unconditionally would satisfy the test
/// above and quietly empty the catalogue.
#[test]
fn a_source_whose_sampled_payload_names_no_personal_identifier_still_registers() -> Result<()> {
    let (outcome, held) = assess_with_body(
        "quotes",
        r#"{"symbol":"EU0001","bid":10.25,"ask":10.27,"volume":41000}"#,
    )?;
    assert!(
        matches!(outcome, DecisionOutcome::Registered { .. }),
        "a clean payload must still register, or the screen refuses everything: {outcome:?}"
    );
    assert!(held, "a registered source belongs in the registry");
    Ok(())
}

/// A corporate registry is the source class §7.1 names as missing, and it is
/// full of names and addresses that belong to companies rather than to people.
/// A screen keyed on a bare `name` or `address` would refuse the very class
/// the platform wants next, which is a gate that refuses everything wearing a
/// governance rule's clothes.
#[test]
fn a_corporate_filing_naming_directors_and_a_registered_office_still_registers() -> Result<()> {
    let (outcome, held) = assess_with_body(
        "filings",
        r#"{"company_name":"Acme plc","registered_address":"1 Example Way","director_name":"A Director","filing_id":"F-1"}"#,
    )?;
    assert!(
        matches!(outcome, DecisionOutcome::Registered { .. }),
        "a corporate filing is not personal data on a private individual: {outcome:?}"
    );
    assert!(held, "a registered source belongs in the registry");
    Ok(())
}

/// Fail closed on what was not examined.
///
/// A payload this phase cannot parse into named fields is filed by
/// `ProbeEvidence` as a schema with nothing in it, and a screen that read "no
/// field carries an identifier" off that would clear every non-JSON source in
/// the catalogue while reporting the same verdict as a feed that was actually
/// looked at. This crate already refuses the identical inference for
/// robots.txt: the absence of a file is not a permission.
#[test]
fn a_payload_that_could_not_be_screened_is_refused_rather_than_read_as_clean() -> Result<()> {
    let (outcome, held) = assess_with_body("opaque", "<html><body>prices</body></html>")?;
    match &outcome {
        DecisionOutcome::Rejected { reason } => assert!(
            reason.contains("nothing was screened for personal data"),
            "the refusal must say nothing was examined rather than that nothing was found, \
             got {reason}"
        ),
        other => panic!("an unscreenable payload must not register, got {other:?}"),
    }
    assert!(!held, "an unscreened source must not reach the registry");
    Ok(())
}

/// The substring trap, through the whole lifecycle rather than through the
/// matcher alone.
///
/// `microphone` contains `phone` and a product page is a source class §7.6.4
/// names by name — the marketplace observation. A rule table matched on
/// substrings would refuse it. This is the delimited-token property asserted
/// where it can actually cost something.
#[test]
fn a_product_page_selling_microphones_is_not_mistaken_for_a_phone_number() -> Result<()> {
    let (outcome, held) = assess_with_body(
        "marketplace",
        r#"{"product_microphone_price":41.0,"currency_dobra_rate":1.5}"#,
    )?;
    assert!(
        matches!(outcome, DecisionOutcome::Registered { .. }),
        "`microphone` is not a phone number and `dobra` is not a date of birth: {outcome:?}"
    );
    assert!(held, "a registered source belongs in the registry");
    Ok(())
}

/// The refusal is recorded in the lifecycle trail, not only returned.
///
/// A decision has to be auditable six months later. A source refused for
/// personal data whose record does not say so is a refusal nobody can review,
/// and the reviewer's question — "why is this feed not in the catalogue" —
/// has no answer in the record.
#[test]
fn the_personal_data_screen_records_its_finding_in_the_lifecycle_trail() -> Result<()> {
    let mut finder = finder()?;
    let mut probe = InMemoryProbe::new()
        .with_robots(HOST, polite_robots())
        .with_head(URL, ok_head())
        .with_sample(
            URL,
            sample(r#"{"holder_passport_number":"X","price":41.0}"#),
        );
    let decisions = finder.assess(
        vec![candidate(
            "passports",
            URL,
            licensed_for(&[Usage::Research, Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    let trail = decisions[0].reasoning().describe();
    assert!(
        trail.contains("holder_passport_number"),
        "the trail must name the field that caused the refusal, got {trail}"
    );
    assert!(
        trail.contains("national identifier"),
        "the trail must name the class of identifier, got {trail}"
    );
    Ok(())
}

/// A finder that has been given an `InMemoryProbe` serving no sample at all
/// defers rather than registering, and that path is unchanged by the screen.
///
/// Asserted because the screen was inserted between the probe and the scoring
/// step, and a check placed there could have swallowed the deferral the probe
/// failure is supposed to produce — turning "we could not reach this source"
/// into "we refused this source", which are different findings an operator
/// acts on differently.
#[test]
fn a_source_the_probe_cannot_reach_is_still_deferred_rather_than_refused_on_personal_data()
-> Result<()> {
    let mut finder = finder()?;
    let mut probe = InMemoryProbe::new().with_robots(HOST, polite_robots());
    let decisions = finder.assess(
        vec![candidate(
            "unreachable",
            URL,
            licensed_for(&[Usage::Research, Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    match decisions[0].outcome() {
        DecisionOutcome::Deferred { .. } => Ok(()),
        other => panic!("an unreachable source is deferred, not refused: {other:?}"),
    }
}
