//! The committed candidate catalogue, and the refusals that keep it honest.
//!
//! ADR 0054 names two things that would void the decision this file implements.
//! One is a route added without review, which no test here can see. The other
//! **is** testable and is the subject of most of this file: a catalogue that
//! admitted an entry with no reviewed route would produce a run in which some
//! candidates were assessed and others were "unreachable" for a reason that
//! reads as the publisher's fault and is ours.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{candidate, licensed_for, now};
use qip_contracts::governance::Usage;
use qip_core::error::Result;
use qip_core::time::Duration;
use qip_data_finder::catalogue::{CandidateEntry, load};

/// The committed catalogue as the deployment mounts it.
const COMMITTED: &str = include_str!("../../../../../data/datasets/source-candidates.json");

/// A serialised catalogue of one entry, with whatever route is given.
fn one(route: &str) -> Result<String> {
    let entry = CandidateEntry {
        candidate: candidate(
            "example-source",
            "https://example.com/v1/quotes",
            licensed_for(&[Usage::Derive, Usage::Trade])?,
            &["EUR/USD"],
        )?,
        egress_route: route.to_string(),
    };
    Ok(serde_json::to_string(&vec![entry]).expect("serialisable"))
}

#[test]
fn the_committed_catalogue_loads_and_every_entry_names_a_route() -> Result<()> {
    // The file a deployment actually mounts, parsed by the code that will
    // parse it. A catalogue that only round-trips through a fixture proves the
    // fixture.
    let loaded = load(COMMITTED, now())?;
    assert!(
        !loaded.is_empty(),
        "the committed catalogue parsed to nothing"
    );
    for entry in &loaded.entries {
        assert!(
            entry.egress_route.starts_with("http://"),
            "{} names the route {:?}, which is not a loopback egress address",
            entry.candidate.id(),
            entry.egress_route
        );
    }
    // The digest is over the bytes, so a run can say which catalogue it
    // assessed against. Two loads of the same text agree; a changed byte does
    // not.
    assert_eq!(loaded.digest, load(COMMITTED, now())?.digest);
    let altered = format!("{COMMITTED}\n");
    assert_ne!(
        loaded.digest,
        load(&altered, now())?.digest,
        "the digest did not move when the bytes did"
    );
    Ok(())
}

#[test]
fn an_entry_with_no_reviewed_route_is_refused_at_load_and_never_at_the_socket() -> Result<()> {
    // ADR 0054's named failure. The refusal has to happen here, loudly, naming
    // the source — not later at a connection that reads like the publisher
    // being down.
    for absent in ["", "   "] {
        let error = load(&one(absent)?, now()).expect_err("a routeless entry must be refused");
        assert!(
            error.message().contains("example-source"),
            "the refusal must name the source: {}",
            error.message()
        );
        assert!(
            error.message().contains("0054"),
            "the refusal must name the decision it enforces: {}",
            error.message()
        );
    }
    // The premise: a real route loads, so the refusals above are about the
    // route and not about the loader refusing everything.
    assert!(
        load(&one("http://127.0.0.1:9105")?, now()).is_ok(),
        "a reviewed route must load, or the refusals prove nothing"
    );
    Ok(())
}

#[test]
fn an_https_route_is_refused_because_a_route_is_not_a_destination() -> Result<()> {
    // The same confusion `NetworkProbe::through` refuses, caught one layer
    // earlier so a bad catalogue never reaches a probe. Downgrading instead
    // would send a plaintext request to port 443.
    let error = load(&one("https://api.frankfurter.dev")?, now())
        .expect_err("an https route must be refused");
    assert!(
        error.message().contains("plaintext to port 443"),
        "the refusal must say what would happen: {}",
        error.message()
    );
    // And a route naming no scheme is refused rather than having one
    // prepended, because a prepended scheme is a guess about which door was
    // meant.
    for bad in ["127.0.0.1:9105", "ftp://example.invalid"] {
        assert!(
            load(&one(bad)?, now()).is_err(),
            "`{bad}` was accepted as an egress route"
        );
    }
    Ok(())
}

#[test]
fn a_candidate_serde_wrote_directly_still_faces_the_constructors_guards() -> Result<()> {
    // `SourceCandidate`'s fields are private and `Deserialize` is derived, so
    // serde writes them past every check `new` makes. Without
    // `SourceCandidate::validate` a catalogue entry would be admitted on terms
    // a programmatically built candidate is refused on — and the guard that
    // matters here is `discovered_from`, whose whole purpose is that a source
    // which appeared from nowhere cannot be re-derived when its decision is
    // questioned.
    let text = one("http://127.0.0.1:9105")?;
    // The premise: it loads before the field is emptied.
    assert!(load(&text, now()).is_ok(), "the fixture must load first");
    let hollowed = text.replace(
        r#""discovered_from":"a curated directory of exchange data vendors""#,
        r#""discovered_from":"   ""#,
    );
    assert_ne!(hollowed, text, "the replacement matched nothing");
    let error = load(&hollowed, now()).expect_err("an unattributed candidate must be refused");
    assert!(
        error.message().contains("where it was discovered"),
        "the refusal must be the constructor's own: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_catalogue_that_is_empty_or_repeats_an_id_is_refused_rather_than_absorbed() -> Result<()> {
    // An empty catalogue and a catalogue nobody mounted produce the same
    // silent run, and only one of them is a deployment somebody meant.
    assert!(
        load("[]", now()).is_err(),
        "an empty catalogue was treated as `no sources to assess`"
    );

    // Two entries under one id is a record that cannot say which route a
    // decision was reached through.
    let entry = CandidateEntry {
        candidate: candidate(
            "example-source",
            "https://example.com/v1/quotes",
            licensed_for(&[Usage::Derive, Usage::Trade])?,
            &["EUR/USD"],
        )?,
        egress_route: "http://127.0.0.1:9105".to_string(),
    };
    let doubled = serde_json::to_string(&vec![entry.clone(), entry]).expect("serialisable");
    let error = load(&doubled, now()).expect_err("a repeated id must be refused");
    assert!(
        error.message().contains("twice"),
        "the refusal must say what is wrong: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_candidate_discovered_after_the_load_instant_is_refused_and_not_clamped() -> Result<()> {
    // A discovery instant in the future is a clock nobody can trust, and
    // clamping it would make the bitemporal record wrong in the direction that
    // hides a leak: a source would appear to have been knowable earlier than
    // it was.
    let text = one("http://127.0.0.1:9105")?;
    let error = load(&text, now().saturating_sub(Duration::from_days(1)))
        .expect_err("a candidate from the future must be refused");
    assert!(
        error.message().contains("after the load instant"),
        "the refusal must say what is wrong: {}",
        error.message()
    );
    Ok(())
}
